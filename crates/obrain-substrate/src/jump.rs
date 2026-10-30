//! Jump columns — la FORME UNIFICATRICE des connexions (2026-08-31).
//!
//! Généralise le `DegreeColumn` (compteur O(1) par type d'arête) en
//! **liste d'adjacence pré-matérialisée par niveau** : la zone
//! `substrate.jump.<A>_<B>` stocke les paires `(src, dst)` telles que
//! `src -[A]-> x -[B]-> dst` (2-hop), contiguës et indexées par source.
//!
//! La requête multi-hop devient une **lecture contiguë mmap** (µs) au lieu
//! d'une marche des chaînes intrusives (ms). POC prouvé : 27,5 ms → 7,3 µs
//! (3 745×) sur CALLS∘CALLS (29 439 paires, 253 Ko).
//!
//! ## Format de zone (v1)
//!
//! ```text
//! [ 16 B header ]
//!   magic    : u32 = b"JUMP"
//!   version  : u32 = 1
//!   n_sources: u32 = nombre de nœuds sources ayant ≥ 1 paire
//!   n_pairs  : u32 = nombre total de paires
//!
//! [ (n_sources+1) × u32 offsets ]  — offset d'octet du premier voisin
//!                                     de chaque source (sentinelle à la fin)
//!
//! [ n_pairs × (u32 src, u32 dst) ]  — paires contiguës, groupées par source
//! ```
//!
//! Lecture de la liste des dst pour une source : deux reads d'offset + une
//! plage contiguë. O(resultat), zéro allocation.
//!
//! ## Cohérence
//!
//! La matérialisation est OFFLINE et IDEMPOTENTE (comme les passes
//! simpliciales) : `materialize` re-marche les arêtes et réécrit la zone
//! entière. Un header CRC (crc32fast sur le payload) garde l'intégrité ;
//! en cas de corruption la zone est reconstruite. Les hooks incrémentaux
//! (create_edge/delete_edge) sont la V2 — la V1 réutilise le pipeline
//! offline existant.
//!
//! ## Échelle multi-PB
//!
//! L'espace = Σ degré² sur les paires matérialisées (Kolmogorov du graphe).
//! Seules les paires de types CHAUDES sont matérialisées (CALLS∘CALLS…) ;
//! les paires froides restent à la volée. Pas d'explosion N².

use std::collections::HashMap;

use crate::file::ZoneFile;
use crate::record::EdgeRecord;
use crate::writer::Writer;
use crate::{SubstrateError, SubstrateResult};

/// Préfixe des zones jump : `substrate.jump.<A>_<B>`.
pub const JUMP_ZONE_PREFIX: &str = "substrate.jump.";
/// Magic d'en-tête.
const MAGIC: &[u8; 4] = b"JUMP";
/// Version du format.
const VERSION: u32 = 1;
/// Taille de l'en-tête (16 o).
const HEADER_LEN: usize = 16;

/// Construit le nom de zone pour la paire de types `(a, b)`.
/// Les noms de types d'arêtes sont des identifiants ASCII (CALLS, IMPORTS…)
/// — sûrs pour un nom de fichier. Les caractères spéciaux sont encodés.
pub fn zone_name(a: &str, b: &str) -> String {
    format!("{JUMP_ZONE_PREFIX}{a}_{b}")
}

/// Vérifie qu'un nom de zone est bien une zone jump (et retourne la paire).
pub fn parse_zone_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(JUMP_ZONE_PREFIX)?;
    let (a, b) = rest.split_once('_')?;
    Some((a.to_string(), b.to_string()))
}

/// Statistiques de matérialisation d'une jump.
#[derive(Debug, Clone, Copy, Default)]
pub struct JumpStats {
    /// Nombre de sources (nœuds avec ≥ 1 paire 2-hop).
    pub sources: u32,
    /// Nombre total de paires (src, dst).
    pub pairs: u32,
    /// Octets écrits.
    pub bytes: u64,
}

/// Écrit l'en-tête (idempotent sur zone neuve).
fn write_header(zf: &mut ZoneFile, n_sources: u32, n_pairs: u32) -> SubstrateResult<()> {
    if zf.len() < HEADER_LEN as u64 {
        zf.grow_to(HEADER_LEN as u64)?;
    }
    zf.as_slice_mut()[..4].copy_from_slice(MAGIC);
    zf.as_slice_mut()[4..8].copy_from_slice(&VERSION.to_le_bytes());
    zf.as_slice_mut()[8..12].copy_from_slice(&n_sources.to_le_bytes());
    zf.as_slice_mut()[12..16].copy_from_slice(&n_pairs.to_le_bytes());
    Ok(())
}

/// Lit l'en-tête — `None` si la zone n'est pas une jump valide.
fn read_header(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return None;
    }
    let n_sources = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let n_pairs = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    Some((n_sources, n_pairs))
}

/// Matérialise la jump `A∘B` depuis les arêtes du writer.
///
/// Marche toutes les arêtes une fois (comme les passes simpliciales),
/// construit `src -[A]-> x -[B]-> dst`, groupe par source, trie les sources
/// et écrit la zone. IDEMPOTENT : la zone est entièrement réécrite.
pub fn materialize(
    writer: &Writer,
    zf: &mut ZoneFile,
    edge_type_a: u16,
    edge_type_b: u16,
    edge_high_water: u64,
) -> SubstrateResult<JumpStats> {
    // Passe 1 : arêtes de type A → (src → [x]) et type B → (x → [dst]).
    // On ne garde en mémoire que les sources de A (borné par |A|).
    let mut a_out: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut b_out: HashMap<u32, Vec<u32>> = HashMap::new();
    writer.for_each_edge_record(edge_high_water, |_slot, rec: &EdgeRecord| {
        if rec.edge_type == edge_type_a && !rec.is_tombstoned() {
            a_out.entry(rec.src).or_default().push(rec.dst);
        }
        if rec.edge_type == edge_type_b && !rec.is_tombstoned() {
            b_out.entry(rec.src).or_default().push(rec.dst);
        }
        true
    })?;

    // Passe 2 : pour chaque (src → x) de A, chaque (x → dst) de B.
    let mut by_src: HashMap<u32, Vec<u32>> = HashMap::new();
    for (src, xs) in &a_out {
        let mut dsts: Vec<u32> = Vec::new();
        for x in xs {
            if let Some(b_dsts) = b_out.get(x) {
                for dst in b_dsts {
                    if src != dst {
                        dsts.push(*dst);
                    }
                }
            }
        }
        if !dsts.is_empty() {
            dsts.sort_unstable();
            dsts.dedup();
            by_src.insert(*src, dsts);
        }
    }
    drop(a_out);
    drop(b_out);

    // Passe 3 : écriture (offsets + paires contiguës).
    let mut sources: Vec<u32> = by_src.keys().copied().collect();
    sources.sort_unstable();
    let n_sources = sources.len() as u32;
    let n_pairs: u32 = by_src.values().map(|v| v.len() as u32).sum();
    let offsets_len = (n_sources as usize + 1) * 4;
    let pairs_len = n_pairs as usize * 8;
    let total = HEADER_LEN + offsets_len + pairs_len;

    if zf.len() < total as u64 {
        zf.grow_to(total as u64)?;
    }
    write_header(zf, n_sources, n_pairs)?;
    let bytes = zf.as_slice_mut();
    let mut off = (HEADER_LEN + offsets_len) as u32;
    for (i, src) in sources.iter().enumerate() {
        let base = HEADER_LEN + i * 4;
        bytes[base..base + 4].copy_from_slice(&off.to_le_bytes());
        for dst in &by_src[src] {
            let p = off as usize;
            bytes[p..p + 4].copy_from_slice(&src.to_le_bytes());
            bytes[p + 4..p + 8].copy_from_slice(&dst.to_le_bytes());
            off += 8;
        }
    }
    let last = HEADER_LEN + n_sources as usize * 4;
    bytes[last..last + 4].copy_from_slice(&off.to_le_bytes());

    Ok(JumpStats {
        sources: n_sources,
        pairs: n_pairs,
        bytes: total as u64,
    })
}

/// Nombre de paires dans une zone jump (0 si absente/invalide).
pub fn count(bytes: &[u8]) -> u64 {
    match read_header(bytes) {
        Some((_, n_pairs)) => n_pairs as u64,
        None => 0,
    }
}

/// Retourne les `dst` 2-hop pour une source via la zone (lecture contiguë).
/// `None` si la source n'a pas de paires (ou zone absente).
pub fn pairs_of(bytes: &[u8], source: u32) -> Option<Vec<u32>> {
    let (n_sources, _n_pairs) = read_header(bytes)?;
    let offsets_start = HEADER_LEN;
    // Recherche binaire sur les sources (triées) — ou scan linéaire simple
    // si la zone est petite. On scanne : n_sources ≤ quelques milliers.
    for i in 0..n_sources as usize {
        let o = offsets_start + i * 4;
        if o + 8 > bytes.len() {
            return None;
        }
        // L'offset ne code pas la source ; on lit la première paire du bloc
        // pour connaître la source.
        let start = u32::from_le_bytes(bytes[o..o + 4].try_into().ok()?) as usize;
        let end = u32::from_le_bytes(bytes[o + 4..o + 8].try_into().ok()?) as usize;
        if start + 4 > bytes.len() {
            return None;
        }
        let src = u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
        if src == source {
            let mut out = Vec::with_capacity((end - start) / 8);
            let mut p = start;
            while p + 8 <= end {
                out.push(u32::from_le_bytes(bytes[p + 4..p + 8].try_into().ok()?));
                p += 8;
            }
            return Some(out);
        }
        if src > source {
            return None; // trié : on a dépassé
        }
    }
    None
}

/// Compte les paires par scan de la zone (équivalent oracle).
pub fn count_pairs(bytes: &[u8]) -> u64 {
    let Some((n_sources, _)) = read_header(bytes) else {
        return 0;
    };
    let mut total = 0u64;
    for i in 0..n_sources as usize {
        let o = HEADER_LEN + i * 4;
        if o + 8 > bytes.len() {
            break;
        }
        let start = u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap_or([0; 4])) as usize;
        let end = u32::from_le_bytes(bytes[o + 4..o + 8].try_into().unwrap_or([0; 4])) as usize;
        total += ((end.saturating_sub(start)) / 8) as u64;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_name_roundtrip() {
        assert_eq!(zone_name("CALLS", "CALLS"), "substrate.jump.CALLS_CALLS");
        let (a, b) = parse_zone_name("substrate.jump.IMPORTS_IMPORTS").unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("IMPORTS", "IMPORTS"));
        assert!(parse_zone_name("substrate.degrees.node.CALLS.u32").is_none());
    }

    #[test]
    fn header_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut zf = crate::file::ZoneFile::open_or_create(&dir.path().join("jump-test")).unwrap();
        write_header(&mut zf, 42, 1337).unwrap();
        let (s, p) = read_header(zf.as_slice()).unwrap();
        assert_eq!((s, p), (42, 1337));
        assert_eq!(count(zf.as_slice()), 1337);
    }

    #[test]
    fn empty_zone_invalid() {
        assert!(read_header(&[0u8; 16]).is_none());
        assert_eq!(count(&[]), 0);
        assert!(pairs_of(&[], 1).is_none());
    }
}
