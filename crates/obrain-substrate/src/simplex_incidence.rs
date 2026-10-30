//! Zone `SimplexIncidence` — incidence sommet→simplexes (T4, décision
//! 2d461b89, spec 271180c3 §2).
//!
//! L'équivalent natif d'`idx_hyperedge_members_node` de cbm : pour chaque
//! slot de nœud, la liste des `simplex_id` dont il est membre, lisible en
//! O(degré simplicial) — jamais de scan global.
//!
//! ## Disposition
//!
//! `substrate.simplex_incidence` ([`crate::file::Zone::SimplexIncidence`]) —
//! répertoire PLAT indexé par slot de nœud (croissance triviale, comme la
//! zone nodes) :
//!
//! ```text
//!   0..64      en-tête (magic "SPXI", version)
//!   64..       entrées 16 B par slot : { head U48, tail U48, count u32 }
//!              head/tail = offset OCTET de BLOC dans le side-car postings
//!              (0 = aucun — le premier bloc réel vit à 4104)
//! ```
//!
//! `substrate.simplex_postings` (side-car nommé,
//! [`SIMPLEX_POSTINGS_FILENAME`]) — pages 4 KiB à CRC32, découpées en
//! BLOCS de 32 octets chaînés par U48 (discipline props_zone : pages 4 KiB,
//! CRC32, chaînage U48 — mais le grain d'allocation est le bloc, pas la
//! page : un nœud au degré simplicial 1 coûte 32 B, pas 4 Kio ; c'est ce
//! qui tient le budget de la décision, ~8 B amortis par posting sur la
//! distribution de référence) :
//!
//! ```text
//!   0..4096      en-tête du side-car (magic "SPXP", version,
//!                next_block_off u64)
//!   4096..       pages de 4096 octets :
//!                  0..4    magic de page "SXPG"
//!                  4..8    crc32 (sur les octets 0..4 ++ 8..4096)
//!                  8..4072 127 blocs de 32 octets :
//!                            0..6   next (U48, offset octet du bloc
//!                                   suivant de la chaîne, 0 = fin)
//!                            6      used (u8, entrées occupées, ≤ 6)
//!                            7      réservé
//!                            8..32  6 entrées simplex_id u32 LE
//!                  4072..4096 remplissage
//! ```
//!
//! ## Sémantique
//!
//! Les postings sont APPEND-ONLY : le tombstone d'un simplexe ne retire pas
//! son id des chaînes — la lecture filtre contre le répertoire
//! (`SimplexDir`), et [`SimplexIncidenceZone::compact_postings`] réécrit
//! les chaînes sans les morts (ordre des nœuds croissant, déterministe —
//! I4). Un id vivant trouvé dans la chaîne d'un nœud contient toujours ce
//! nœud : les postings ne sont posés que pour les membres réels, et
//! l'ensemble des membres d'un id vivant ne change jamais (la clé EST la
//! liste triée des membres).

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::ZoneFile;
use crate::record::U48;
use crate::simplex_zone::SimplexId;
use crate::writer::ensure_room;

/// Nom du side-car des pages de postings.
pub const SIMPLEX_POSTINGS_FILENAME: &str = "substrate.simplex_postings";

/// Magic de la zone répertoire d'incidence : `b"SPXI"`.
pub const SIMPLEX_INCIDENCE_MAGIC: u32 = u32::from_le_bytes(*b"SPXI");
/// Magic du side-car postings : `b"SPXP"`.
pub const SIMPLEX_POSTINGS_MAGIC: u32 = u32::from_le_bytes(*b"SPXP");
/// Magic d'une page de postings : `b"SXPG"`.
pub const POSTING_PAGE_MAGIC: u32 = u32::from_le_bytes(*b"SXPG");
/// Version on-disk courante.
pub const SIMPLEX_INCIDENCE_VERSION: u32 = 1;

/// Taille de l'en-tête du répertoire (octets).
pub const INCIDENCE_HEADER_SIZE: usize = 64;
/// Taille d'une entrée de répertoire : head U48 + tail U48 + count u32.
pub const INCIDENCE_ENTRY_SIZE: usize = 16;

/// Taille d'une page de postings.
pub const POSTING_PAGE_SIZE: usize = 4096;
/// En-tête de page : magic + crc.
pub const POSTING_PAGE_HEADER: usize = 8;
/// Taille d'un bloc de postings.
pub const POSTING_BLOCK_SIZE: usize = 32;
/// En-tête de bloc : next U48 + used u8 + réservé u8.
pub const POSTING_BLOCK_HEADER: usize = 8;
/// Entrées u32 par bloc.
pub const POSTINGS_PER_BLOCK: usize = (POSTING_BLOCK_SIZE - POSTING_BLOCK_HEADER) / 4;
/// Blocs par page.
pub const BLOCKS_PER_PAGE: usize = (POSTING_PAGE_SIZE - POSTING_PAGE_HEADER) / POSTING_BLOCK_SIZE;
/// Offset de la première page dans le side-car (l'en-tête du fichier occupe
/// la première tranche de 4 KiB ; l'offset 0 sert de sentinelle « aucun »).
pub const POSTING_REGION_OFFSET: usize = 4096;
/// Offset du premier bloc allouable.
pub const FIRST_BLOCK_OFFSET: u64 = (POSTING_REGION_OFFSET + POSTING_PAGE_HEADER) as u64;

/// Entrée de répertoire décodée.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct IncidenceEntry {
    pub head: u64,
    pub tail: u64,
    pub count: u32,
}

/// Opérations sur la paire (répertoire d'incidence, side-car postings).
/// Stateless — les fichiers de zone portent tout l'état persistant.
pub struct SimplexIncidenceZone;

impl SimplexIncidenceZone {
    // ---- répertoire -------------------------------------------------------

    pub fn init_dir(zf: &mut ZoneFile) -> SubstrateResult<()> {
        if (zf.len() as usize) < INCIDENCE_HEADER_SIZE {
            ensure_room(zf, INCIDENCE_HEADER_SIZE + (INCIDENCE_ENTRY_SIZE << 8), 0)?;
            let slice = zf.as_slice_mut();
            slice[..4].copy_from_slice(&SIMPLEX_INCIDENCE_MAGIC.to_le_bytes());
            slice[4..8].copy_from_slice(&SIMPLEX_INCIDENCE_VERSION.to_le_bytes());
            for b in &mut slice[8..INCIDENCE_HEADER_SIZE] {
                *b = 0;
            }
            return Ok(());
        }
        Self::validate_dir(zf)
    }

    fn validate_dir(zf: &ZoneFile) -> SubstrateResult<()> {
        let slice = zf.as_slice();
        if u32::from_le_bytes(slice[0..4].try_into().unwrap()) != SIMPLEX_INCIDENCE_MAGIC {
            return Err(SubstrateError::BadMagic);
        }
        let version = u32::from_le_bytes(slice[4..8].try_into().unwrap());
        if version != SIMPLEX_INCIDENCE_VERSION {
            return Err(SubstrateError::UnsupportedVersion(
                version,
                SIMPLEX_INCIDENCE_VERSION,
            ));
        }
        Ok(())
    }

    /// Lit l'entrée du slot `node`. `None` si le répertoire ne couvre pas
    /// encore ce slot (jamais aucun posting).
    pub fn entry(zf: &ZoneFile, node: u32) -> SubstrateResult<Option<IncidenceEntry>> {
        if (zf.len() as usize) < INCIDENCE_HEADER_SIZE {
            return Ok(None);
        }
        Self::validate_dir(zf)?;
        let off = INCIDENCE_HEADER_SIZE + node as usize * INCIDENCE_ENTRY_SIZE;
        let slice = zf.as_slice();
        if off + INCIDENCE_ENTRY_SIZE > slice.len() {
            return Ok(None);
        }
        let mut h = [0u8; 6];
        h.copy_from_slice(&slice[off..off + 6]);
        let mut t = [0u8; 6];
        t.copy_from_slice(&slice[off + 6..off + 12]);
        let count = u32::from_le_bytes(slice[off + 12..off + 16].try_into().unwrap());
        Ok(Some(IncidenceEntry {
            head: U48(h).to_u64(),
            tail: U48(t).to_u64(),
            count,
        }))
    }

    fn write_entry(zf: &mut ZoneFile, node: u32, e: IncidenceEntry) -> SubstrateResult<()> {
        Self::init_dir(zf)?;
        let off = INCIDENCE_HEADER_SIZE + node as usize * INCIDENCE_ENTRY_SIZE;
        ensure_room(zf, off + INCIDENCE_ENTRY_SIZE, 1 << 20)?;
        let slice = zf.as_slice_mut();
        slice[off..off + 6].copy_from_slice(&U48::from_u64(e.head).0);
        slice[off + 6..off + 12].copy_from_slice(&U48::from_u64(e.tail).0);
        slice[off + 12..off + 16].copy_from_slice(&e.count.to_le_bytes());
        Ok(())
    }

    // ---- side-car postings ------------------------------------------------

    pub fn init_posts(zf: &mut ZoneFile) -> SubstrateResult<()> {
        if (zf.len() as usize) < POSTING_REGION_OFFSET {
            ensure_room(zf, POSTING_REGION_OFFSET, 0)?;
            let slice = zf.as_slice_mut();
            slice[..4].copy_from_slice(&SIMPLEX_POSTINGS_MAGIC.to_le_bytes());
            slice[4..8].copy_from_slice(&SIMPLEX_INCIDENCE_VERSION.to_le_bytes());
            slice[8..16].copy_from_slice(&FIRST_BLOCK_OFFSET.to_le_bytes());
            for b in &mut slice[16..64] {
                *b = 0;
            }
            return Ok(());
        }
        Self::validate_posts(zf)
    }

    fn validate_posts(zf: &ZoneFile) -> SubstrateResult<()> {
        let slice = zf.as_slice();
        if u32::from_le_bytes(slice[0..4].try_into().unwrap()) != SIMPLEX_POSTINGS_MAGIC {
            return Err(SubstrateError::BadMagic);
        }
        let version = u32::from_le_bytes(slice[4..8].try_into().unwrap());
        if version != SIMPLEX_INCIDENCE_VERSION {
            return Err(SubstrateError::UnsupportedVersion(
                version,
                SIMPLEX_INCIDENCE_VERSION,
            ));
        }
        Ok(())
    }

    /// Offset octet où le prochain bloc sera alloué.
    fn next_block_off(zf: &ZoneFile) -> SubstrateResult<u64> {
        Self::validate_posts(zf)?;
        Ok(u64::from_le_bytes(zf.as_slice()[8..16].try_into().unwrap()))
    }

    fn set_next_block_off(zf: &mut ZoneFile, off: u64) {
        zf.as_slice_mut()[8..16].copy_from_slice(&off.to_le_bytes());
    }

    /// Début de la page 4 KiB contenant l'offset `off`.
    #[inline]
    fn page_of(off: u64) -> u64 {
        off & !(POSTING_PAGE_SIZE as u64 - 1)
    }

    /// Alloue un bloc vierge (bump-allocateur : les blocs se suivent dans
    /// les pages ; une page neuve est initialisée au passage) et retourne
    /// son offset octet.
    fn alloc_block(zf: &mut ZoneFile) -> SubstrateResult<u64> {
        Self::init_posts(zf)?;
        let off = Self::next_block_off(zf)?;
        let page = Self::page_of(off);
        ensure_room(zf, (page as usize) + POSTING_PAGE_SIZE, 1 << 20)?;
        if off == page + POSTING_PAGE_HEADER as u64 {
            // Première allocation dans cette page : poser magic + CRC.
            let p = &mut zf.as_slice_mut()[page as usize..page as usize + POSTING_PAGE_SIZE];
            p.fill(0);
            p[0..4].copy_from_slice(&POSTING_PAGE_MAGIC.to_le_bytes());
            Self::stamp_crc(zf, page);
        }
        // Prochain bloc : dans la même page s'il reste de la place, sinon
        // au premier bloc de la page suivante.
        let mut next = off + POSTING_BLOCK_SIZE as u64;
        if next + POSTING_BLOCK_SIZE as u64 > page + POSTING_PAGE_SIZE as u64 {
            next = page + POSTING_PAGE_SIZE as u64 + POSTING_PAGE_HEADER as u64;
        }
        Self::set_next_block_off(zf, next);
        Ok(off)
    }

    /// Recalcule et écrit le CRC32 d'une page (octets 0..4 ++ 8..4096).
    fn stamp_crc(zf: &mut ZoneFile, page_off: u64) {
        let crc = {
            let page = &zf.as_slice()[page_off as usize..page_off as usize + POSTING_PAGE_SIZE];
            let mut h = crc32fast::Hasher::new();
            h.update(&page[0..4]);
            h.update(&page[8..POSTING_PAGE_SIZE]);
            h.finalize()
        };
        zf.as_slice_mut()[page_off as usize + 4..page_off as usize + 8]
            .copy_from_slice(&crc.to_le_bytes());
    }

    /// Valide magic + CRC de la page portant le bloc, puis lit l'en-tête du
    /// bloc : `(used, next)`.
    fn check_block(zf: &ZoneFile, block_off: u64) -> SubstrateResult<(usize, u64)> {
        let slice = zf.as_slice();
        let page = Self::page_of(block_off) as usize;
        if page + POSTING_PAGE_SIZE > slice.len() {
            return Err(SubstrateError::WalBadFrame(format!(
                "bloc de postings hors zone : off={block_off} len={}",
                slice.len()
            )));
        }
        let p = &slice[page..page + POSTING_PAGE_SIZE];
        if u32::from_le_bytes(p[0..4].try_into().unwrap()) != POSTING_PAGE_MAGIC {
            return Err(SubstrateError::WalBadFrame(format!(
                "page de postings sans magic à l'offset {page}"
            )));
        }
        let claimed = u32::from_le_bytes(p[4..8].try_into().unwrap());
        let actual = {
            let mut h = crc32fast::Hasher::new();
            h.update(&p[0..4]);
            h.update(&p[8..POSTING_PAGE_SIZE]);
            h.finalize()
        };
        if claimed != actual {
            return Err(SubstrateError::WalCrcMismatch { claimed, actual });
        }
        let b = block_off as usize;
        let mut n = [0u8; 6];
        n.copy_from_slice(&slice[b..b + 6]);
        let used = slice[b + 6] as usize;
        Ok((used, U48(n).to_u64()))
    }

    // ---- opérations -------------------------------------------------------

    /// Poste `id` dans la chaîne du nœud `node`. O(1) amorti : l'entrée de
    /// répertoire pointe le bloc de queue.
    pub fn append(
        inc: &mut ZoneFile,
        posts: &mut ZoneFile,
        node: u32,
        id: SimplexId,
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
                // Chaîner l'ancienne queue vers la neuve.
                let old = e.tail as usize;
                posts.as_slice_mut()[old..old + 6].copy_from_slice(&U48::from_u64(b).0);
                Self::stamp_crc(posts, Self::page_of(e.tail));
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
            let slot = b + POSTING_BLOCK_HEADER + used * 4;
            slice[slot..slot + 4].copy_from_slice(&id.to_le_bytes());
            slice[b + 6] = (used + 1) as u8;
        }
        Self::stamp_crc(posts, Self::page_of(block_off));
        e.count += 1;
        Self::write_entry(inc, node, e)
    }

    /// Liste BRUTE des postings du nœud (ids tombstonés compris — le
    /// filtrage de vie appartient à l'appelant, qui tient le répertoire
    /// des simplexes). O(degré simplicial du nœud).
    pub fn postings(
        inc: &ZoneFile,
        posts: &ZoneFile,
        node: u32,
    ) -> SubstrateResult<Vec<SimplexId>> {
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
                out.push(u32::from_le_bytes(
                    slice[start + i * 4..start + i * 4 + 4].try_into().unwrap(),
                ));
            }
            block_off = next;
        }
        Ok(out)
    }

    /// Réécrit TOUTES les chaînes en ne gardant que les postings pour
    /// lesquels `garder(node, id)` est vrai. Ordre déterministe (nœuds
    /// croissants, postings dans leur ordre d'origine) — I4. La région de
    /// pages est zéro-remplie puis réallouée séquentiellement.
    pub fn compact_postings(
        inc: &mut ZoneFile,
        posts: &mut ZoneFile,
        garder: impl Fn(u32, SimplexId) -> bool,
    ) -> SubstrateResult<()> {
        if (inc.len() as usize) < INCIDENCE_HEADER_SIZE {
            return Ok(());
        }
        Self::validate_dir(inc)?;
        let slot_cap = (inc.len() as usize - INCIDENCE_HEADER_SIZE) / INCIDENCE_ENTRY_SIZE;

        // (1) Collecter les chaînes survivantes.
        let mut chains: Vec<(u32, Vec<SimplexId>)> = Vec::new();
        for node in 0..slot_cap as u32 {
            let Some(e) = Self::entry(inc, node)? else {
                continue;
            };
            if e.head == 0 {
                continue;
            }
            let ids = Self::postings(inc, posts, node)?;
            let kept: Vec<SimplexId> = ids.into_iter().filter(|&id| garder(node, id)).collect();
            chains.push((node, kept));
        }

        // (2) Effacer la région pages et réallouer séquentiellement.
        let old_end = Self::page_of(
            Self::next_block_off(posts).unwrap_or(FIRST_BLOCK_OFFSET) + POSTING_PAGE_SIZE as u64
                - 1,
        ) as usize;
        {
            let slice = posts.as_slice_mut();
            let end = old_end.min(slice.len());
            for b in &mut slice[POSTING_REGION_OFFSET.min(end)..end] {
                *b = 0;
            }
        }
        Self::set_next_block_off(posts, FIRST_BLOCK_OFFSET);

        for (node, ids) in chains {
            Self::write_entry(inc, node, IncidenceEntry::default())?;
            for id in ids {
                Self::append(inc, posts, node, id)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{SubstrateFile, Zone};

    fn zones() -> (SubstrateFile, ZoneFile, ZoneFile) {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let inc = sub.open_zone(Zone::SimplexIncidence).unwrap();
        let posts = sub.open_named_zone(SIMPLEX_POSTINGS_FILENAME).unwrap();
        (sub, inc, posts)
    }

    #[test]
    fn noeud_sans_postings_liste_vide() {
        let (_s, inc, posts) = zones();
        assert!(
            SimplexIncidenceZone::postings(&inc, &posts, 42)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn append_et_relecture() {
        let (_s, mut inc, mut posts) = zones();
        for id in [7u32, 9, 11] {
            SimplexIncidenceZone::append(&mut inc, &mut posts, 5, id).unwrap();
        }
        SimplexIncidenceZone::append(&mut inc, &mut posts, 6, 100).unwrap();
        assert_eq!(
            SimplexIncidenceZone::postings(&inc, &posts, 5).unwrap(),
            vec![7, 9, 11]
        );
        assert_eq!(
            SimplexIncidenceZone::postings(&inc, &posts, 6).unwrap(),
            vec![100]
        );
        assert!(
            SimplexIncidenceZone::postings(&inc, &posts, 7)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn un_noeud_au_degre_1_coute_un_bloc_pas_une_page() {
        let (_s, mut inc, mut posts) = zones();
        // 100 nœuds à un posting chacun : tous les blocs tiennent dans UNE
        // page de postings (127 blocs/page) — c'est la densité qui tient le
        // budget disque de la décision.
        for node in 0..100u32 {
            SimplexIncidenceZone::append(&mut inc, &mut posts, node, node + 1_000).unwrap();
        }
        let next = SimplexIncidenceZone::next_block_off(&posts).unwrap();
        assert!(
            next < (POSTING_REGION_OFFSET + POSTING_PAGE_SIZE) as u64,
            "100 blocs doivent tenir dans la première page (next={next})"
        );
        for node in 0..100u32 {
            assert_eq!(
                SimplexIncidenceZone::postings(&inc, &posts, node).unwrap(),
                vec![node + 1_000]
            );
        }
    }

    #[test]
    fn chainage_multi_blocs_et_multi_pages() {
        let (_s, mut inc, mut posts) = zones();
        // Assez d'entrées pour chaîner des blocs À TRAVERS une frontière de
        // page : 127 blocs/page × 6 entrées = 762 ; 800 en traverse une.
        let n = BLOCKS_PER_PAGE * POSTINGS_PER_BLOCK + 38;
        for id in 0..n as u32 {
            SimplexIncidenceZone::append(&mut inc, &mut posts, 3, id).unwrap();
        }
        let got = SimplexIncidenceZone::postings(&inc, &posts, 3).unwrap();
        assert_eq!(got.len(), n);
        assert_eq!(got, (0..n as u32).collect::<Vec<_>>());
        let e = SimplexIncidenceZone::entry(&inc, 3).unwrap().unwrap();
        assert_eq!(e.count as usize, n);
        assert!(
            SimplexIncidenceZone::page_of(e.tail) > SimplexIncidenceZone::page_of(e.head),
            "la chaîne doit traverser une frontière de page"
        );
    }

    #[test]
    fn crc_detecte_la_corruption() {
        let (_s, mut inc, mut posts) = zones();
        SimplexIncidenceZone::append(&mut inc, &mut posts, 1, 42).unwrap();
        let e = SimplexIncidenceZone::entry(&inc, 1).unwrap().unwrap();
        // Corrompre un octet de données du bloc.
        posts.as_slice_mut()[e.head as usize + POSTING_BLOCK_HEADER] ^= 0xFF;
        let err = SimplexIncidenceZone::postings(&inc, &posts, 1).unwrap_err();
        assert!(matches!(err, SubstrateError::WalCrcMismatch { .. }));
    }

    #[test]
    fn compaction_filtre_et_reste_deterministe() {
        let (_s, mut inc, mut posts) = zones();
        for node in [2u32, 5, 9] {
            for id in 0..10u32 {
                SimplexIncidenceZone::append(&mut inc, &mut posts, node, id).unwrap();
            }
        }
        // Garder les ids pairs seulement.
        SimplexIncidenceZone::compact_postings(&mut inc, &mut posts, |_n, id| id % 2 == 0).unwrap();
        for node in [2u32, 5, 9] {
            assert_eq!(
                SimplexIncidenceZone::postings(&inc, &posts, node).unwrap(),
                vec![0, 2, 4, 6, 8]
            );
        }

        // Déterminisme : une seconde base au même contenu vivant produit la
        // même région de pages à l'octet près.
        let (_s2, mut inc2, mut posts2) = zones();
        for node in [2u32, 5, 9] {
            for id in [0u32, 2, 4, 6, 8] {
                SimplexIncidenceZone::append(&mut inc2, &mut posts2, node, id).unwrap();
            }
        }
        SimplexIncidenceZone::compact_postings(&mut inc2, &mut posts2, |_, _| true).unwrap();
        let end1 = SimplexIncidenceZone::next_block_off(&posts).unwrap() as usize;
        let end2 = SimplexIncidenceZone::next_block_off(&posts2).unwrap() as usize;
        assert_eq!(end1, end2);
        assert_eq!(
            &posts.as_slice()[POSTING_REGION_OFFSET..end1],
            &posts2.as_slice()[POSTING_REGION_OFFSET..end2],
            "I4 : mêmes chaînes vivantes ⇒ pages byte-identiques après compaction"
        );
    }
}
