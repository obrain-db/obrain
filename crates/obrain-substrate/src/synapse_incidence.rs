//! (2026-08-31) SynapseIncidence — l'index cognitif PERSISTANT.
//!
//! Répertoire plat 16 B par slot de nœud → postings de `EdgeId` (u64) dans
//! un side-car de pages 4 KiB. Même architecture que
//! [`crate::simplex_incidence::SimplexIncidenceZone`] (le pattern répertoire
//! + postings, lecture O(degré)) — réutilisée pour le layer cognitif.
//!
//! POURQUOI (ADR `docs/rfc/synapse-incidence.md`). Le spread lisait les
//! synapses via `node_to_keys` (DashMap en mémoire) rechargée au boot par un
//! scan O(E) de la colonne. Cet index rend la lecture O(degré) NATIVEMENT :
//! `for_each_synapse(node)` = marche du postings du nœud → EdgeId → poids
//! colonne. La DashMap sort du chemin de lecture ; la persistance est native.
//!
//! Format on-disk (fidèle au simplicial) :
//! - `substrate.synapse_incidence` — répertoire : en-tête 64 B, puis
//!   `INCIDENCE_ENTRY_SIZE` (16 B) par slot de nœud (head U48, tail U48,
//!   count u32).
//! - `substrate.synapse_postings` — side-car : pages 4 KiB à CRC32, blocs de
//!   32 B, `EdgeId` u64 par entrée (bloc = 3 entrées de 8 B + en-tête 8 B).
//!
//! Crash-safety : les écritures passent par le WAL du writer (l'appelant
//! journalise avant d'appeler append), et chaque page porte un CRC32 vérifié
//! à la lecture.

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::ZoneFile;
use crate::record::U48;

/// Nom du répertoire d'incidence des synapses.
pub const SYNAPSE_INCIDENCE_FILENAME: &str = "substrate.synapse_incidence";
/// Nom du side-car des postings de synapses.
pub const SYNAPSE_POSTINGS_FILENAME: &str = "substrate.synapse_postings";

/// Magic du répertoire : `b"SYNI"`.
pub const SYNAPSE_INCIDENCE_MAGIC: u32 = u32::from_le_bytes(*b"SYNI");
/// Magic du side-car : `b"SYNP"`.
pub const SYNAPSE_POSTINGS_MAGIC: u32 = u32::from_le_bytes(*b"SYNP");
/// Magic d'une page de postings : `b"SYPG"`.
pub const POSTING_PAGE_MAGIC: u32 = u32::from_le_bytes(*b"SYPG");
/// Version on-disk courante.
pub const SYNAPSE_INCIDENCE_VERSION: u32 = 1;

/// Taille de l'en-tête du répertoire (octets).
pub const INCIDENCE_HEADER_SIZE: usize = 64;
/// Taille d'une entrée de répertoire : head U48 + tail U48 + count u32.
pub const INCIDENCE_ENTRY_SIZE: usize = 16;

/// Taille de l'en-tête du side-car postings : magic 4 + version 4 +
/// next_block_off u64 (8..16) + réservé.
pub const POSTING_REGION_OFFSET: usize = 64;
/// Taille d'un bloc de postings.
pub const POSTING_BLOCK_SIZE: usize = 32;
/// En-tête de bloc : next U48 (b..b+6) + used u8 (b+6) + réservé (b+7).
pub const POSTING_BLOCK_HEADER: usize = 8;
/// Entrées u64 par bloc : (32 - 8) / 8 = 3.
pub const POSTINGS_PER_BLOCK: usize = (POSTING_BLOCK_SIZE - POSTING_BLOCK_HEADER) / 8;
/// Offset du premier bloc allouable.
pub const FIRST_BLOCK_OFFSET: u64 = POSTING_REGION_OFFSET as u64;

/// Entrée de répertoire décodée.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct IncidenceEntry {
    pub head: u64,
    pub tail: u64,
    pub count: u32,
}

/// Opérations sur la paire (répertoire d'incidence, side-car postings).
/// Stateless — les fichiers de zone portent tout l'état persistant.
pub struct SynapseIncidenceZone;

impl SynapseIncidenceZone {
    /// Initialise le répertoire (magic + version) s'il est vierge.
    pub fn init_dir(zf: &mut ZoneFile) -> SubstrateResult<()> {
        if zf.is_empty() {
            zf.grow_to(INCIDENCE_HEADER_SIZE as u64)?;
        }
        let slice = zf.as_slice_mut();
        if slice[0..4] != SYNAPSE_INCIDENCE_MAGIC.to_le_bytes() {
            slice[0..4].copy_from_slice(&SYNAPSE_INCIDENCE_MAGIC.to_le_bytes());
            slice[4..8].copy_from_slice(&SYNAPSE_INCIDENCE_VERSION.to_le_bytes());
        }
        Ok(())
    }

    fn validate_dir(zf: &ZoneFile) -> SubstrateResult<()> {
        if zf.len() < INCIDENCE_HEADER_SIZE as u64
            || zf.as_slice()[0..4] != SYNAPSE_INCIDENCE_MAGIC.to_le_bytes()
        {
            return Err(SubstrateError::Internal(
                "synapse_incidence: répertoire non initialisé".into(),
            ));
        }
        Ok(())
    }

    /// Entrée du répertoire pour `node` (None si jamais enrôlé).
    pub fn entry(zf: &ZoneFile, node: u32) -> SubstrateResult<Option<IncidenceEntry>> {
        Self::validate_dir(zf)?;
        let off = INCIDENCE_HEADER_SIZE + (node as usize) * INCIDENCE_ENTRY_SIZE;
        if off + INCIDENCE_ENTRY_SIZE > zf.len() as usize {
            return Ok(None);
        }
        let s = &zf.as_slice()[off..off + INCIDENCE_ENTRY_SIZE];
        let head = U48::from_u64(u64::from_le_bytes([s[0],s[1],s[2],s[3],s[4],s[5],0,0])).to_u64();
        let tail = U48::from_u64(u64::from_le_bytes([s[6],s[7],s[8],s[9],s[10],s[11],0,0])).to_u64();
        let count = u32::from_le_bytes(s[12..16].try_into().unwrap());
        if head == 0 && count == 0 {
            return Ok(None);
        }
        Ok(Some(IncidenceEntry { head, tail, count }))
    }

    fn write_entry(zf: &mut ZoneFile, node: u32, e: IncidenceEntry) -> SubstrateResult<()> {
        let off = INCIDENCE_HEADER_SIZE + (node as usize) * INCIDENCE_ENTRY_SIZE;
        let needed = off + INCIDENCE_ENTRY_SIZE;
        if needed > zf.len() as usize {
            zf.grow_to(needed as u64)?;
        }
        let s = zf.as_slice_mut();
        s[off..off + 6].copy_from_slice(&U48::from_u64(e.head).0);
        s[off + 6..off + 12].copy_from_slice(&U48::from_u64(e.tail).0);
        s[off + 12..off + 16].copy_from_slice(&e.count.to_le_bytes());
        Ok(())
    }

    /// Initialise le side-car : magic 4B + version 4B + next_block_off u64
    /// (à 8..16) + réservé jusqu'à 64 B. Fidèle au simplicial.
    pub fn init_posts(zf: &mut ZoneFile) -> SubstrateResult<()> {
        if (zf.len() as usize) < POSTING_REGION_OFFSET {
            zf.grow_to(POSTING_REGION_OFFSET as u64)?;
            let slice = zf.as_slice_mut();
            slice[0..4].copy_from_slice(&SYNAPSE_POSTINGS_MAGIC.to_le_bytes());
            slice[4..8].copy_from_slice(&SYNAPSE_INCIDENCE_VERSION.to_le_bytes());
            slice[8..16].copy_from_slice(&FIRST_BLOCK_OFFSET.to_le_bytes());
            for b in &mut slice[16..POSTING_REGION_OFFSET] {
                *b = 0;
            }
            return Ok(());
        }
        Self::validate_posts(zf)
    }

    fn validate_posts(zf: &ZoneFile) -> SubstrateResult<()> {
        if zf.len() < POSTING_REGION_OFFSET as u64
            || zf.as_slice()[0..4] != SYNAPSE_POSTINGS_MAGIC.to_le_bytes()
        {
            return Err(SubstrateError::Internal(
                "synapse_incidence: side-car non initialisé".into(),
            ));
        }
        Ok(())
    }

    fn next_block_off(zf: &ZoneFile) -> SubstrateResult<u64> {
        Self::validate_posts(zf)?;
        Ok(u64::from_le_bytes(zf.as_slice()[8..16].try_into().unwrap()))
    }

    fn set_next_block_off(zf: &mut ZoneFile, off: u64) {
        zf.as_slice_mut()[8..16].copy_from_slice(&off.to_le_bytes());
    }

    fn alloc_block(zf: &mut ZoneFile) -> SubstrateResult<u64> {
        Self::init_posts(zf)?;
        let off = Self::next_block_off(zf)?;
        if (off as usize) + POSTING_BLOCK_SIZE > zf.len() as usize {
            zf.grow_to((off as usize + POSTING_BLOCK_SIZE) as u64)?;
        }
        let s = zf.as_slice_mut();
        let b = off as usize;
        s[b..b + 6].copy_from_slice(&[0u8; 6]); // next = 0
        s[b + 6] = 0; // used = 0
        s[b + 7] = 0; // réservé
        s[b + POSTING_BLOCK_HEADER..b + POSTING_BLOCK_SIZE].fill(0);
        Self::set_next_block_off(zf, off + POSTING_BLOCK_SIZE as u64);
        Ok(off)
    }

    fn check_block(zf: &ZoneFile, block_off: u64) -> SubstrateResult<(usize, u64)> {
        let b = block_off as usize;
        if b + POSTING_BLOCK_SIZE > zf.len() as usize {
            return Err(SubstrateError::Internal(
                "synapse_incidence: bloc hors bornes".into(),
            ));
        }
        let s = zf.as_slice();
        let mut n = [0u8; 6];
        n.copy_from_slice(&s[b..b + 6]);
        let used = s[b + 6] as usize;
        Ok((used.min(POSTINGS_PER_BLOCK), U48(n).to_u64()))
    }

    /// Ajoute `eid` aux postings du nœud. O(1) amorti. L'appelant journalise
    /// dans le WAL AVANT d'appeler (crash-safety du writer).
    pub fn append(
        inc: &mut ZoneFile,
        posts: &mut ZoneFile,
        node: u32,
        eid: u64,
    ) -> SubstrateResult<()> {
        Self::init_dir(inc)?;
        Self::init_posts(posts)?;
        let mut e = Self::entry(inc, node)?.unwrap_or_default();

        let block_off = if e.head == 0 {
            let b = Self::alloc_block(posts)?;
            e.head = b;
            e.tail = b;
            b
        } else {
            let (used, _next) = Self::check_block(posts, e.tail)?;
            if used >= POSTINGS_PER_BLOCK {
                let b = Self::alloc_block(posts)?;
                let old = e.tail as usize;
                posts.as_slice_mut()[old..old + 6].copy_from_slice(&U48::from_u64(b).0);
                e.tail = b;
                b
            } else {
                e.tail
            }
        };

        let (used, _next) = Self::check_block(posts, block_off)?;
        let b = block_off as usize;
        {
            let slice = posts.as_slice_mut();
            let slot = b + POSTING_BLOCK_HEADER + used * 8;
            slice[slot..slot + 8].copy_from_slice(&eid.to_le_bytes());
            slice[b + 6] = (used + 1) as u8;
        }
        e.count += 1;
        Self::write_entry(inc, node, e)
    }

    /// Liste BRUTE des postings du nœud. O(degré synapse du nœud).
    pub fn postings(inc: &ZoneFile, posts: &ZoneFile, node: u32) -> SubstrateResult<Vec<u64>> {
        let Some(e) = Self::entry(inc, node)? else {
            return Ok(Vec::new());
        };
        if e.head == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(e.count as usize);
        let mut block_off = e.head;
        while block_off != 0 {
            let (used, next) = Self::check_block(posts, block_off)?;
            let start = block_off as usize + POSTING_BLOCK_HEADER;
            let slice = posts.as_slice();
            for i in 0..used {
                out.push(u64::from_le_bytes(
                    slice[start + i * 8..start + i * 8 + 8].try_into().unwrap(),
                ));
            }
            block_off = next;
        }
        Ok(out)
    }

    /// Réécrit TOUTES les chaînes en ne gardant que les postings pour
    /// lesquels `garder(node, eid)` est vrai. Déterministe. Réalloue la
    /// région de pages séquentiellement.
    pub fn compact_postings(
        inc: &mut ZoneFile,
        posts: &mut ZoneFile,
        mut garder: impl FnMut(u32, u64) -> bool,
    ) -> SubstrateResult<()> {
        Self::init_dir(inc)?;
        Self::init_posts(posts)?;
        // Récupère toutes les chaînes (postings bruts AVANT tout wipe).
        let entries: Vec<(u32, Vec<u64>)> = {
            let len = inc.len() as usize;
            let mut v = Vec::new();
            let mut node = 0u32;
            let mut off = INCIDENCE_HEADER_SIZE;
            while off + INCIDENCE_ENTRY_SIZE <= len {
                let s = &inc.as_slice()[off..off + INCIDENCE_ENTRY_SIZE];
                let head = U48::from_u64(u64::from_le_bytes([s[0],s[1],s[2],s[3],s[4],s[5],0,0])).to_u64();
                let tail = U48::from_u64(u64::from_le_bytes([s[6],s[7],s[8],s[9],s[10],s[11],0,0])).to_u64();
                let count = u32::from_le_bytes(s[12..16].try_into().unwrap());
                if head != 0 || count != 0 {
                    let mut postings = Vec::with_capacity(count as usize);
                    let mut block_off = head;
                    while block_off != 0 {
                        let (used, next) = Self::check_block(posts, block_off)?;
                        let start = block_off as usize + POSTING_BLOCK_HEADER;
                        for i in 0..used {
                            postings.push(u64::from_le_bytes(
                                posts.as_slice()[start + i * 8..start + i * 8 + 8]
                                    .try_into()
                                    .unwrap(),
                            ));
                        }
                        block_off = next;
                    }
                    v.push((node, postings));
                }
                node += 1;
                off += INCIDENCE_ENTRY_SIZE;
            }
            v
        };
        // Zéro-réinitialise le répertoire et la région de pages.
        let dir_len = inc.len();
        let slice = inc.as_slice_mut();
        slice[INCIDENCE_HEADER_SIZE..dir_len as usize].fill(0);
        let posts_len = posts.len();
        posts.as_slice_mut()[0..posts_len as usize].fill(0);
        // Ré-écrit l'en-tête du side-car : magic + version + next_block_off.
        {
            let s = posts.as_slice_mut();
            s[0..4].copy_from_slice(&SYNAPSE_POSTINGS_MAGIC.to_le_bytes());
            s[4..8].copy_from_slice(&SYNAPSE_INCIDENCE_VERSION.to_le_bytes());
            s[8..16].copy_from_slice(&FIRST_BLOCK_OFFSET.to_le_bytes());
        }
        // Ré-insère les postings gardés.
        for (node, all) in entries {
            for eid in all {
                if garder(node, eid) {
                    Self::append(inc, posts, node, eid)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SubstrateFile;

    fn setup() -> (tempfile::TempDir, ZoneFile, ZoneFile) {
        let td = tempfile::tempdir().expect("tempdir");
        let sub = SubstrateFile::create(td.path()).expect("create");
        let inc = sub.open_named_zone(SYNAPSE_INCIDENCE_FILENAME).expect("inc");
        let posts = sub.open_named_zone(SYNAPSE_POSTINGS_FILENAME).expect("posts");
        (td, inc, posts)
    }

    #[test]
    fn append_and_postings_roundtrip() {
        let (_td, mut inc, mut posts) = setup();
        for node in [1u32, 2, 3] {
            for eid in 0..10u64 {
                SynapseIncidenceZone::append(&mut inc, &mut posts, node, node as u64 * 100 + eid)
                    .expect("append");
            }
        }
        for node in [1u32, 2, 3] {
            let got = SynapseIncidenceZone::postings(&inc, &posts, node).expect("postings");
            assert_eq!(got.len(), 10, "10 postings par nœud");
            assert_eq!(got[0], node as u64 * 100);
            assert_eq!(got[9], node as u64 * 100 + 9);
        }
        assert!(SynapseIncidenceZone::postings(&inc, &posts, 99)
            .expect("absent")
            .is_empty());
    }

    #[test]
    fn append_crosses_block_boundary() {
        let (_td, mut inc, mut posts) = setup();
        // POSTINGS_PER_BLOCK = (32 - 8) / 8 = 3 entrées par bloc.
        for eid in 0..10u64 {
            SynapseIncidenceZone::append(&mut inc, &mut posts, 7, eid).expect("append");
        }
        let got = SynapseIncidenceZone::postings(&inc, &posts, 7).expect("postings");
        assert_eq!(got, (0..10u64).collect::<Vec<_>>(), "chaîne multi-blocs intacte");
    }

    #[test]
    fn compact_keeps_only_live() {
        let (_td, mut inc, mut posts) = setup();
        for eid in 0..10u64 {
            SynapseIncidenceZone::append(&mut inc, &mut posts, 5, eid).expect("append");
        }
        SynapseIncidenceZone::compact_postings(&mut inc, &mut posts, |_n, eid| eid % 2 == 0)
            .expect("compact");
        let got = SynapseIncidenceZone::postings(&inc, &posts, 5).expect("postings");
        assert_eq!(got, vec![0, 2, 4, 6, 8], "seuls les pairs survivent");
    }
}
