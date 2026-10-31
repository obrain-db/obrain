//! Zone `SimplexDir` — stockage persistant des simplexes (T4, plan
//! « Topologie simpliciale native », décision 2d461b89 option B, spec
//! 271180c3).
//!
//! ## Disposition
//!
//! `substrate.simplex_dir` ([`crate::file::Zone::SimplexDir`]) :
//!
//! ```text
//!   0..64      en-tête (magic "SPXD", version, high_water)
//!   64..       tableau de SimplexRecord (32 B chacun), simplex_id = index
//! ```
//!
//! `substrate.simplex_heap` (side-car nommé, [`SIMPLEX_HEAP_FILENAME`]) :
//!
//! ```text
//!   0..64      en-tête (magic "SPXH", version, payload_used)
//!   64..       snapshots append-only : [membres: n×u32 LE][props: octets]
//! ```
//!
//! Un répertoire fixe façon engram est impossible ici — l'espace d'ids est
//! u32 (spec : lever la borne u16 des engrams) — d'où l'adaptation : le
//! tableau de records croît en place (offset O(1) = 64 + id×32) et le
//! payload n-aire vit dans un side-car append-only à la discipline
//! d'`engram.rs` (snapshot append + réécriture du record, anciens snapshots
//! orphelins jusqu'à [`compact_heap`]).
//!
//! ## Invariants (spec §3)
//!
//! * **I1 canonicité** — membres strictement croissants (sauf ORDERED,
//!   hyperarêtes seulement), pairwise distincts : [`canonicalize_members`].
//! * **I2 unicité** — (kind, type_id, key_hash) unique, vérification par
//!   comparaison des membres en collision : [`SimplexKeyIndex`].
//! * **I3 bornes** — hyperarêtes 3..=64 membres ; groupe hors borne rejeté
//!   ENTIER (jamais tronqué), comptage loggé.
//! * **I4 déterminisme** — [`compact_heap`] réécrit le payload dans l'ordre
//!   croissant des ids : mêmes simplexes vivants ⇒ mêmes octets, quelle
//!   qu'ait été l'histoire des snapshots.
//! * **I7 durabilité** — ce module n'écrit JAMAIS le WAL : les chemins
//!   WAL-first vivent dans [`crate::writer::Writer`] ; le rejeu
//!   ([`crate::replay`]) rappelle les mêmes primitives `*_raw`, donc les
//!   zones sont reconstructibles depuis le WAL seul (invariant engram.rs).
//!
//! Les slots ne sont jamais réutilisés : un id tombstoné le reste (la
//! réparation offline façon cbm est la seule voie de renumérotation).

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::ZoneFile;
use crate::record::{
    SIMPLEX_HYPEREDGE_MAX_MEMBERS, SIMPLEX_HYPEREDGE_MIN_MEMBERS, SimplexRecord, simplex_flags,
    simplex_kind,
};
use crate::writer::ensure_room;
use std::collections::HashMap;

/// Nom du side-car heap des hyperarêtes (membres + propriétés).
pub const SIMPLEX_HEAP_FILENAME: &str = "substrate.simplex_heap";

/// Magic de la zone répertoire : `b"SPXD"` little-endian.
pub const SIMPLEX_DIR_MAGIC: u32 = u32::from_le_bytes(*b"SPXD");
/// Magic du side-car heap : `b"SPXH"` little-endian.
pub const SIMPLEX_HEAP_MAGIC: u32 = u32::from_le_bytes(*b"SPXH");
/// Version on-disk courante des deux fichiers.
pub const SIMPLEX_ZONE_VERSION: u32 = 1;
/// Taille de l'en-tête fixe des deux fichiers (octets).
pub const SIMPLEX_HEADER_SIZE: usize = 64;
/// Alignement des snapshots heap (u32).
pub const HEAP_ALIGN: usize = 4;

/// Identifiant de simplexe : index du record dans la zone `SimplexDir`.
pub type SimplexId = u32;

/// Vue matérialisée d'un simplexe vivant (API de lecture).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimplexView {
    pub id: SimplexId,
    pub kind: u8,
    pub flags: u8,
    pub type_id: u16,
    /// Ordre canonique croissant — ou ordre d'émission gelé sous ORDERED.
    pub members: Vec<u32>,
    /// Octets de propriétés (hyperarêtes ; vide pour les kinds 2-4).
    pub props: Vec<u8>,
}

/// Hash de clé canonique : xxh3 64 bits des octets LE des membres TRIÉS
/// (spec §1). Contrat on-disk — changer d'algorithme casse le format.
pub fn key_hash_members(sorted_members: &[u32]) -> u64 {
    let mut bytes = Vec::with_capacity(sorted_members.len() * 4);
    for m in sorted_members {
        bytes.extend_from_slice(&m.to_le_bytes());
    }
    twox_hash::XxHash3_64::oneshot(&bytes)
}

/// I1 + I3 — canonicalise et valide un ensemble de membres.
///
/// Retourne `(stockés, clé_triée)` :
/// * kinds 2-4 — exactement 3/4/5 membres, triés strictement croissants
///   (le tri est appliqué ici : l'appelant peut fournir n'importe quel
///   ordre), `type_id` implicite 0, ORDERED interdit ;
/// * hyperarête — 3..=64 membres (I3 : rejet ENTIER + log), triés sauf
///   ORDERED (l'ordre d'émission est alors gelé, la clé reste le tri) ;
/// * doublons rejetés dans tous les cas (jamais dédupliqués en silence).
pub fn canonicalize_members(
    kind: u8,
    flags: u8,
    members: &[u32],
) -> SubstrateResult<(Vec<u32>, Vec<u32>)> {
    let ordered = flags & simplex_flags::ORDERED != 0;
    match kind {
        simplex_kind::TRIANGLE | simplex_kind::K4 | simplex_kind::K5 => {
            let expected = match kind {
                simplex_kind::TRIANGLE => 3,
                simplex_kind::K4 => 4,
                _ => 5,
            };
            if members.len() != expected {
                return Err(SubstrateError::WalBadFrame(format!(
                    "simplexe kind {kind} : {} membres, {expected} attendus",
                    members.len()
                )));
            }
            if ordered {
                return Err(SubstrateError::WalBadFrame(
                    "flag ORDERED interdit sur les kinds 2-4 (cliques canoniques)".into(),
                ));
            }
            let sorted = sort_distinct(members)?;
            Ok((sorted.clone(), sorted))
        }
        simplex_kind::HYPEREDGE => {
            if members.len() < SIMPLEX_HYPEREDGE_MIN_MEMBERS
                || members.len() > SIMPLEX_HYPEREDGE_MAX_MEMBERS
            {
                // I3 : rejet ENTIER, jamais tronqué — et comptage loggé.
                tracing::warn!(
                    member_count = members.len(),
                    min = SIMPLEX_HYPEREDGE_MIN_MEMBERS,
                    max = SIMPLEX_HYPEREDGE_MAX_MEMBERS,
                    "hyperarête hors bornes : groupe rejeté entier (I3)"
                );
                return Err(SubstrateError::WalBadFrame(format!(
                    "hyperarête : {} membres hors bornes [{SIMPLEX_HYPEREDGE_MIN_MEMBERS}, \
                     {SIMPLEX_HYPEREDGE_MAX_MEMBERS}] — groupe rejeté entier",
                    members.len()
                )));
            }
            let sorted = sort_distinct(members)?;
            let stored = if ordered {
                members.to_vec()
            } else {
                sorted.clone()
            };
            Ok((stored, sorted))
        }
        other => Err(SubstrateError::WalBadFrame(format!(
            "kind de simplexe inconnu : {other}"
        ))),
    }
}

/// Trie et vérifie la distinction pairwise. Doublon ⇒ erreur (I1).
fn sort_distinct(members: &[u32]) -> SubstrateResult<Vec<u32>> {
    let mut sorted = members.to_vec();
    sorted.sort_unstable();
    if sorted.windows(2).any(|w| w[0] == w[1]) {
        return Err(SubstrateError::WalBadFrame(
            "membres de simplexe non pairwise-distincts (I1)".into(),
        ));
    }
    Ok(sorted)
}

/// Valide les flags hors ORDERED (déjà couvert par la canonicalisation) :
/// CYCLIC est réservé aux triangles, TOMBSTONED n'est jamais fourni en
/// entrée d'upsert.
pub fn validate_flags(kind: u8, flags: u8) -> SubstrateResult<()> {
    if flags & simplex_flags::TOMBSTONED != 0 {
        return Err(SubstrateError::WalBadFrame(
            "flag TOMBSTONED interdit à l'upsert".into(),
        ));
    }
    if flags & simplex_flags::CYCLIC != 0 && kind != simplex_kind::TRIANGLE {
        return Err(SubstrateError::WalBadFrame(
            "flag CYCLIC réservé aux triangles (orientation 3-cycle)".into(),
        ));
    }
    let known = simplex_flags::TOMBSTONED | simplex_flags::CYCLIC | simplex_flags::ORDERED;
    if flags & !known != 0 {
        return Err(SubstrateError::WalBadFrame(format!(
            "flags de simplexe inconnus : {flags:#04x}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Zone répertoire (substrate.simplex_dir)
// ---------------------------------------------------------------------------

/// Opérations sur la zone répertoire. Stateless — le fichier de zone porte
/// tout l'état persistant (gabarit [`crate::engram::EngramZone`]).
pub struct SimplexDirZone;

impl SimplexDirZone {
    /// Initialise l'en-tête si la zone est vierge ; sinon le valide.
    pub fn init(zf: &mut ZoneFile) -> SubstrateResult<()> {
        if (zf.len() as usize) < SIMPLEX_HEADER_SIZE {
            ensure_room(zf, SIMPLEX_HEADER_SIZE + (SimplexRecord::SIZE << 7), 0)?;
            write_header(zf, SIMPLEX_DIR_MAGIC);
            return Ok(());
        }
        validate_header(zf, SIMPLEX_DIR_MAGIC, "simplex_dir")
    }

    /// Prochain `simplex_id` à allouer (= nombre de slots consommés).
    pub fn high_water(zf: &ZoneFile) -> SubstrateResult<u32> {
        validate_header(zf, SIMPLEX_DIR_MAGIC, "simplex_dir")?;
        Ok(u32::from_le_bytes(zf.as_slice()[8..12].try_into().unwrap()))
    }

    fn set_high_water(zf: &mut ZoneFile, hw: u32) {
        zf.as_slice_mut()[8..12].copy_from_slice(&hw.to_le_bytes());
    }

    /// Lit le record du slot `id`. `None` si le slot est au-delà du
    /// high-water ou jamais matérialisé.
    pub fn read(zf: &ZoneFile, id: SimplexId) -> SubstrateResult<Option<SimplexRecord>> {
        if zf.is_empty() {
            return Ok(None);
        }
        let hw = Self::high_water(zf)?;
        if id >= hw {
            return Ok(None);
        }
        let off = SIMPLEX_HEADER_SIZE + id as usize * SimplexRecord::SIZE;
        let rec: SimplexRecord =
            *bytemuck::from_bytes(&zf.as_slice()[off..off + SimplexRecord::SIZE]);
        if rec.is_empty_slot() {
            return Ok(None);
        }
        Ok(Some(rec))
    }

    /// Écrit le record au slot `id` (chemin brut, PAS de WAL — voir la doc
    /// de module) et avance le high-water si nécessaire.
    pub fn write_raw(zf: &mut ZoneFile, id: SimplexId, rec: &SimplexRecord) -> SubstrateResult<()> {
        Self::init(zf)?;
        let off = SIMPLEX_HEADER_SIZE + id as usize * SimplexRecord::SIZE;
        ensure_room(zf, off + SimplexRecord::SIZE, 1 << 20)?;
        zf.as_slice_mut()[off..off + SimplexRecord::SIZE].copy_from_slice(bytemuck::bytes_of(rec));
        let hw = Self::high_water(zf)?;
        if id >= hw {
            Self::set_high_water(zf, id + 1);
        }
        Ok(())
    }

    /// Pose le flag TOMBSTONED sur `id`. Retourne `false` si le slot est
    /// vide ou déjà tombstoné (idempotence de rejeu).
    pub fn tombstone_raw(zf: &mut ZoneFile, id: SimplexId) -> SubstrateResult<bool> {
        let Some(mut rec) = Self::read(zf, id)? else {
            return Ok(false);
        };
        if rec.is_tombstoned() {
            return Ok(false);
        }
        rec.set_tombstoned();
        let off = SIMPLEX_HEADER_SIZE + id as usize * SimplexRecord::SIZE;
        zf.as_slice_mut()[off..off + SimplexRecord::SIZE].copy_from_slice(bytemuck::bytes_of(&rec));
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Side-car heap (substrate.simplex_heap)
// ---------------------------------------------------------------------------

/// Opérations sur le side-car heap des hyperarêtes.
pub struct SimplexHeapZone;

impl SimplexHeapZone {
    pub fn init(zf: &mut ZoneFile) -> SubstrateResult<()> {
        if (zf.len() as usize) < SIMPLEX_HEADER_SIZE {
            ensure_room(zf, SIMPLEX_HEADER_SIZE + (1 << 12), 0)?;
            write_header(zf, SIMPLEX_HEAP_MAGIC);
            return Ok(());
        }
        validate_header(zf, SIMPLEX_HEAP_MAGIC, "simplex_heap")
    }

    /// Octets consommés dans la région payload (le prochain snapshot
    /// s'écrit à cet offset, aligné sur [`HEAP_ALIGN`]).
    pub fn payload_used(zf: &ZoneFile) -> SubstrateResult<u64> {
        validate_header(zf, SIMPLEX_HEAP_MAGIC, "simplex_heap")?;
        Ok(u64::from_le_bytes(zf.as_slice()[8..16].try_into().unwrap()))
    }

    fn set_payload_used(zf: &mut ZoneFile, used: u64) {
        zf.as_slice_mut()[8..16].copy_from_slice(&used.to_le_bytes());
    }

    /// Append un snapshot `[membres × u32 LE][props]` ; retourne l'offset
    /// payload-relatif du snapshot. L'ancien snapshot du même simplexe
    /// devient orphelin — [`compact_heap`] le réclame.
    pub fn append_snapshot(
        zf: &mut ZoneFile,
        members: &[u32],
        props: &[u8],
    ) -> SubstrateResult<u64> {
        Self::init(zf)?;
        let mut used = Self::payload_used(zf)? as usize;
        let misalign = used % HEAP_ALIGN;
        if misalign != 0 {
            used += HEAP_ALIGN - misalign;
        }
        let snap_len = members.len() * 4 + props.len();
        let start = SIMPLEX_HEADER_SIZE + used;
        ensure_room(zf, start + snap_len, 1 << 20)?;
        {
            let slice = zf.as_slice_mut();
            for (i, &m) in members.iter().enumerate() {
                slice[start + i * 4..start + i * 4 + 4].copy_from_slice(&m.to_le_bytes());
            }
            let p0 = start + members.len() * 4;
            slice[p0..p0 + props.len()].copy_from_slice(props);
        }
        Self::set_payload_used(zf, (used + snap_len) as u64);
        Ok(used as u64)
    }

    /// Relit `(membres, props)` d'un snapshot.
    pub fn read_snapshot(
        zf: &ZoneFile,
        payload_off: u64,
        member_count: usize,
        props_len: usize,
    ) -> SubstrateResult<(Vec<u32>, Vec<u8>)> {
        validate_header(zf, SIMPLEX_HEAP_MAGIC, "simplex_heap")?;
        let start = SIMPLEX_HEADER_SIZE + payload_off as usize;
        let end = start + member_count * 4 + props_len;
        let slice = zf.as_slice();
        if end > slice.len() {
            return Err(SubstrateError::WalBadFrame(format!(
                "snapshot simplexe hors zone : off={payload_off} membres={member_count} \
                 props={props_len} len={}",
                slice.len()
            )));
        }
        let members: Vec<u32> = slice[start..start + member_count * 4]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let props = slice[start + member_count * 4..end].to_vec();
        Ok((members, props))
    }
}

fn write_header(zf: &mut ZoneFile, magic: u32) {
    let slice = zf.as_slice_mut();
    slice[..4].copy_from_slice(&magic.to_le_bytes());
    slice[4..8].copy_from_slice(&SIMPLEX_ZONE_VERSION.to_le_bytes());
    for b in &mut slice[8..SIMPLEX_HEADER_SIZE] {
        *b = 0;
    }
}

fn validate_header(zf: &ZoneFile, magic: u32, name: &str) -> SubstrateResult<()> {
    if (zf.len() as usize) < SIMPLEX_HEADER_SIZE {
        return Err(SubstrateError::WalBadFrame(format!(
            "zone {name} trop courte : {} < {SIMPLEX_HEADER_SIZE}",
            zf.len()
        )));
    }
    let slice = zf.as_slice();
    if u32::from_le_bytes(slice[0..4].try_into().unwrap()) != magic {
        return Err(SubstrateError::BadMagic);
    }
    let version = u32::from_le_bytes(slice[4..8].try_into().unwrap());
    if version != SIMPLEX_ZONE_VERSION {
        return Err(SubstrateError::UnsupportedVersion(
            version,
            SIMPLEX_ZONE_VERSION,
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Application d'un état absolu (chemin partagé writer / rejeu)
// ---------------------------------------------------------------------------

/// Résultat de [`apply_set_raw`] — pilote la mise à jour de l'incidence.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum AppliedSimplex {
    /// Le slot portait déjà exactement cet état (rejeu idempotent) — ou un
    /// état identique déjà tombstoné (on ne ressuscite jamais).
    Unchanged,
    /// Même identité (kind, type_id, ensemble de membres) : flags, ordre
    /// d'émission ou propriétés réécrits — l'incidence ne bouge pas.
    Rewritten,
    /// Slot matérialisé — l'appelant doit poster `id` dans l'incidence de
    /// chaque membre.
    Materialized,
}

/// Applique l'état absolu d'un `WalPayload::SimplexSet` aux zones dir +
/// heap. `members` est l'ordre STOCKÉ (canonique, ou d'émission sous
/// ORDERED) ; `props` doit être vide pour les kinds 2-4.
///
/// Idempotence (I7) : ré-appliquer un état déjà présent est un no-op — y
/// compris quand le slot a été tombstoné APRÈS (le rejeu ne ressuscite
/// pas : le contenu est comparé flags-de-vie exclus).
// Les champs sont ceux du payload WAL, naturellement individuels — les
// regrouper n'apporterait rien (même choix que `replay::apply`).
#[allow(clippy::too_many_arguments)]
pub fn apply_set_raw(
    dir: &mut ZoneFile,
    heap: &mut ZoneFile,
    id: SimplexId,
    kind: u8,
    flags: u8,
    type_id: u16,
    members: &[u32],
    props: &[u8],
) -> SubstrateResult<AppliedSimplex> {
    if kind != simplex_kind::HYPEREDGE && !props.is_empty() {
        return Err(SubstrateError::WalBadFrame(
            "propriétés interdites sur les kinds 2-4 (record inline)".into(),
        ));
    }
    SimplexDirZone::init(dir)?;
    let existing = SimplexDirZone::read(dir, id)?;

    if let Some(rec) = existing.as_ref() {
        // Identité d'un slot = (kind, type_id, ENSEMBLE de membres) — la
        // clé d'unicité I2. Flags, ordre d'émission (ORDERED) et props
        // sont du CONTENU réinscriptible sous la même identité.
        let mut incoming_sorted = members.to_vec();
        incoming_sorted.sort_unstable();
        let (cur_members, cur_props) = read_members_props(rec, heap)?;
        let mut cur_sorted = cur_members.clone();
        cur_sorted.sort_unstable();
        let same_identity =
            rec.kind == kind && rec.type_id == type_id && cur_sorted == incoming_sorted;
        if same_identity {
            let same_content = (rec.flags & !simplex_flags::TOMBSTONED)
                == (flags & !simplex_flags::TOMBSTONED)
                && cur_members == members
                && cur_props == props;
            if same_content || rec.is_tombstoned() {
                // Idempotence — et jamais de résurrection : un contenu déjà
                // dépassé par un tombstone reste tombstoné.
                return Ok(AppliedSimplex::Unchanged);
            }
            // Réécriture sous la même identité (flags, ordre ORDERED,
            // props) : l'incidence ne bouge pas.
            let mut updated = *rec;
            updated.flags = flags & !simplex_flags::TOMBSTONED;
            if rec.is_hyperedge() {
                let off = SimplexHeapZone::append_snapshot(heap, members, props)?;
                updated.set_payload_off(off);
                updated.set_props_len(props.len() as u32);
            }
            SimplexDirZone::write_raw(dir, id, &updated)?;
            return Ok(AppliedSimplex::Rewritten);
        }
        if rec.is_tombstoned() {
            // Slot mort avec une autre identité : les ids ne sont jamais
            // réutilisés — refuser plutôt que d'écraser un mort.
            return Err(SubstrateError::WalBadFrame(format!(
                "SimplexSet sur le slot tombstoné {id} avec un contenu différent"
            )));
        }
        // Identité divergente sur un slot vivant : impossible par le
        // writer (nouvelle clé ⇒ nouvel id) — cas de réparation. On
        // applique l'état absolu du WAL, en le rendant VISIBLE.
        tracing::error!(
            simplex_id = id,
            "SimplexSet réécrit un slot vivant avec une autre identité — \
             état absolu du WAL appliqué (réparation)"
        );
    }

    // Matérialisation (slot vierge — ou identité divergente ci-dessus).
    let mut rec = SimplexRecord {
        kind,
        flags: flags & !simplex_flags::TOMBSTONED,
        type_id,
        member_count: members.len() as u16,
        _reserved: 0,
        body: [0; 24],
    };
    if kind == simplex_kind::HYPEREDGE {
        let mut sorted = members.to_vec();
        sorted.sort_unstable();
        rec.set_stored_key_hash(key_hash_members(&sorted));
        let off = SimplexHeapZone::append_snapshot(heap, members, props)?;
        rec.set_payload_off(off);
        rec.set_props_len(props.len() as u32);
        rec.member_count = members.len() as u16;
    } else {
        rec.set_inline_members(members);
    }
    SimplexDirZone::write_raw(dir, id, &rec)?;
    Ok(AppliedSimplex::Materialized)
}

/// Relit `(membres stockés, props)` d'un record.
pub fn read_members_props(
    rec: &SimplexRecord,
    heap: &ZoneFile,
) -> SubstrateResult<(Vec<u32>, Vec<u8>)> {
    if rec.is_hyperedge() {
        SimplexHeapZone::read_snapshot(
            heap,
            rec.payload_off(),
            rec.member_count as usize,
            rec.props_len() as usize,
        )
    } else {
        Ok((rec.inline_members(), Vec::new()))
    }
}

/// Clé d'unicité effective d'un record (hash stocké pour les hyperarêtes,
/// recalculé sur les membres inline — déjà triés — pour les kinds 2-4).
pub fn record_key_hash(rec: &SimplexRecord) -> u64 {
    if rec.is_hyperedge() {
        rec.stored_key_hash()
    } else {
        key_hash_members(&rec.inline_members())
    }
}

// ---------------------------------------------------------------------------
// Index d'unicité (I2) — RAM, reconstruit paresseusement
// ---------------------------------------------------------------------------

/// Index `(kind, type_id, key_hash) → ids` des simplexes VIVANTS.
///
/// Persisté nulle part : reconstruit paresseusement au premier upsert par
/// balayage du répertoire (borné par le nombre de simplexes, spec §2 —
/// l'ouverture ne paie que mmap + en-tête). Les collisions de hash sont
/// portées par le `Vec` et levées par comparaison des membres (I2).
#[derive(Default)]
pub struct SimplexKeyIndex {
    map: HashMap<(u8, u16, u64), Vec<SimplexId>>,
}

impl SimplexKeyIndex {
    /// Balaye le répertoire et indexe tous les records vivants.
    pub fn build(dir: &ZoneFile) -> SubstrateResult<Self> {
        let mut idx = Self::default();
        if dir.is_empty() {
            return Ok(idx);
        }
        let hw = SimplexDirZone::high_water(dir)?;
        for id in 0..hw {
            if let Some(rec) = SimplexDirZone::read(dir, id)?
                && !rec.is_tombstoned()
            {
                idx.insert(rec.kind, rec.type_id, record_key_hash(&rec), id);
            }
        }
        Ok(idx)
    }

    pub fn insert(&mut self, kind: u8, type_id: u16, key_hash: u64, id: SimplexId) {
        self.map
            .entry((kind, type_id, key_hash))
            .or_default()
            .push(id);
    }

    pub fn remove(&mut self, kind: u8, type_id: u16, key_hash: u64, id: SimplexId) {
        if let Some(v) = self.map.get_mut(&(kind, type_id, key_hash)) {
            v.retain(|&x| x != id);
            if v.is_empty() {
                self.map.remove(&(kind, type_id, key_hash));
            }
        }
    }

    /// Résout la clé vers l'id VÉRIFIÉ : en cas de collision de hash, les
    /// membres (clé triée) départagent — jamais le hash seul (I2).
    pub fn find_verified(
        &self,
        dir: &ZoneFile,
        heap: &ZoneFile,
        kind: u8,
        type_id: u16,
        key_hash: u64,
        sorted_members: &[u32],
    ) -> SubstrateResult<Option<SimplexId>> {
        let Some(candidates) = self.map.get(&(kind, type_id, key_hash)) else {
            return Ok(None);
        };
        for &id in candidates {
            let Some(rec) = SimplexDirZone::read(dir, id)? else {
                continue;
            };
            let (stored, _props) = read_members_props(&rec, heap)?;
            let mut stored_sorted = stored;
            stored_sorted.sort_unstable();
            if stored_sorted == sorted_members {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Compaction déterministe (I4)
// ---------------------------------------------------------------------------

/// Réécrit la région payload du heap dans l'ordre croissant des ids, en ne
/// conservant que le snapshot COURANT de chaque hyperarête VIVANTE ; les
/// offsets des records sont réancrés, la région libérée est zéro-remplie
/// (les fichiers de zone ne rétrécissent jamais — `grow_to` le refuse).
///
/// Déterminisme (I4) : deux bases portant les mêmes simplexes vivants ont,
/// après compaction, des régions `[0, 64 + payload_used)` identiques à
/// l'octet près, quelle qu'ait été l'histoire des snapshots orphelins.
pub fn compact_heap(dir: &mut ZoneFile, heap: &mut ZoneFile) -> SubstrateResult<u64> {
    if dir.is_empty() {
        return Ok(0);
    }
    SimplexHeapZone::init(heap)?;
    let hw = SimplexDirZone::high_water(dir)?;
    let old_used = SimplexHeapZone::payload_used(heap)? as usize;

    // (1) Collecte des snapshots vivants dans l'ordre des ids.
    let mut plan: Vec<(SimplexId, SimplexRecord, Vec<u32>, Vec<u8>)> = Vec::new();
    for id in 0..hw {
        if let Some(rec) = SimplexDirZone::read(dir, id)?
            && rec.is_hyperedge()
            && !rec.is_tombstoned()
        {
            let (members, props) = read_members_props(&rec, heap)?;
            plan.push((id, rec, members, props));
        }
    }

    // (2) Réécriture séquentielle depuis l'offset 0.
    let mut cursor = 0usize;
    let mut fresh: Vec<u8> = Vec::new();
    let mut anchors: Vec<(SimplexId, SimplexRecord)> = Vec::with_capacity(plan.len());
    for (id, mut rec, members, props) in plan {
        let misalign = cursor % HEAP_ALIGN;
        if misalign != 0 {
            let pad = HEAP_ALIGN - misalign;
            fresh.extend(std::iter::repeat_n(0u8, pad));
            cursor += pad;
        }
        rec.set_payload_off(cursor as u64);
        rec.set_props_len(props.len() as u32);
        for m in &members {
            fresh.extend_from_slice(&m.to_le_bytes());
        }
        fresh.extend_from_slice(&props);
        cursor += members.len() * 4 + props.len();
        anchors.push((id, rec));
    }

    // (3) Écriture en place : payload frais, queue zéro-remplie, ancres.
    ensure_room(heap, SIMPLEX_HEADER_SIZE + cursor.max(old_used), 0)?;
    {
        let slice = heap.as_slice_mut();
        slice[SIMPLEX_HEADER_SIZE..SIMPLEX_HEADER_SIZE + cursor].copy_from_slice(&fresh);
        let zero_end = (SIMPLEX_HEADER_SIZE + old_used).min(slice.len());
        for b in &mut slice[(SIMPLEX_HEADER_SIZE + cursor).min(zero_end)..zero_end] {
            *b = 0;
        }
    }
    SimplexHeapZone::set_payload_used(heap, cursor as u64);
    for (id, rec) in anchors {
        SimplexDirZone::write_raw(dir, id, &rec)?;
    }
    Ok(cursor as u64)
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
        let dir = sub.open_zone(Zone::SimplexDir).unwrap();
        let heap = sub.open_named_zone(SIMPLEX_HEAP_FILENAME).unwrap();
        (sub, dir, heap)
    }

    #[test]
    fn record_est_pod_32_octets() {
        assert_eq!(core::mem::size_of::<SimplexRecord>(), 32);
        let mut rec = SimplexRecord {
            kind: simplex_kind::HYPEREDGE,
            flags: simplex_flags::ORDERED,
            type_id: 7,
            member_count: 4,
            _reserved: 0,
            body: [0; 24],
        };
        rec.set_stored_key_hash(0xDEAD_BEEF_CAFE_BABE);
        rec.set_payload_off(0x0000_1234_5678);
        rec.set_props_len(99);
        let bytes = bytemuck::bytes_of(&rec).to_vec();
        let back: SimplexRecord = *bytemuck::from_bytes(&bytes);
        assert_eq!(back, rec);
        assert_eq!(back.stored_key_hash(), 0xDEAD_BEEF_CAFE_BABE);
        assert_eq!(back.payload_off(), 0x0000_1234_5678);
        assert_eq!(back.props_len(), 99);
    }

    #[test]
    fn canonicite_tri_et_doublons() {
        // I1 : l'ordre d'entrée est canonisé par tri strict.
        let (stored, key) = canonicalize_members(simplex_kind::TRIANGLE, 0, &[30, 10, 20]).unwrap();
        assert_eq!(stored, vec![10, 20, 30]);
        assert_eq!(key, vec![10, 20, 30]);
        // Doublons rejetés, jamais dédupliqués.
        assert!(canonicalize_members(simplex_kind::TRIANGLE, 0, &[10, 10, 20]).is_err());
        // Arité stricte des cliques.
        assert!(canonicalize_members(simplex_kind::K4, 0, &[1, 2, 3]).is_err());
        assert!(canonicalize_members(simplex_kind::K5, 0, &[1, 2, 3, 4, 5]).is_ok());
    }

    #[test]
    fn ordered_gele_l_ordre_mais_pas_la_cle() {
        let (stored, key) = canonicalize_members(
            simplex_kind::HYPEREDGE,
            simplex_flags::ORDERED,
            &[42, 7, 19],
        )
        .unwrap();
        assert_eq!(stored, vec![42, 7, 19], "ordre d'émission gelé");
        assert_eq!(key, vec![7, 19, 42], "la clé reste le tri");
        assert_eq!(key_hash_members(&key), key_hash_members(&[7, 19, 42]));
        // ORDERED interdit sur les cliques.
        assert!(
            canonicalize_members(simplex_kind::TRIANGLE, simplex_flags::ORDERED, &[1, 2, 3])
                .is_err()
        );
    }

    #[test]
    fn caps_hyperaretes_rejet_entier() {
        // I3 : < 3 et > 64 rejetés entiers.
        assert!(canonicalize_members(simplex_kind::HYPEREDGE, 0, &[1, 2]).is_err());
        let gros: Vec<u32> = (0..65).collect();
        assert!(canonicalize_members(simplex_kind::HYPEREDGE, 0, &gros).is_err());
        let limite: Vec<u32> = (0..64).collect();
        assert!(canonicalize_members(simplex_kind::HYPEREDGE, 0, &limite).is_ok());
    }

    #[test]
    fn flags_valides() {
        assert!(validate_flags(simplex_kind::TRIANGLE, simplex_flags::CYCLIC).is_ok());
        assert!(validate_flags(simplex_kind::K4, simplex_flags::CYCLIC).is_err());
        assert!(validate_flags(simplex_kind::HYPEREDGE, simplex_flags::TOMBSTONED).is_err());
        assert!(validate_flags(simplex_kind::HYPEREDGE, 0x80).is_err());
    }

    #[test]
    fn triangle_inline_roundtrip() {
        let (_sub, mut dir, mut heap) = zones();
        let r = apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::TRIANGLE,
            simplex_flags::CYCLIC,
            0,
            &[10, 20, 30],
            &[],
        )
        .unwrap();
        assert_eq!(r, AppliedSimplex::Materialized);
        let rec = SimplexDirZone::read(&dir, 0).unwrap().unwrap();
        assert_eq!(rec.kind, simplex_kind::TRIANGLE);
        assert_eq!(rec.flags, simplex_flags::CYCLIC);
        let (members, props) = read_members_props(&rec, &heap).unwrap();
        assert_eq!(members, vec![10, 20, 30]);
        assert!(props.is_empty());
        assert_eq!(SimplexDirZone::high_water(&dir).unwrap(), 1);
        // Idempotence : ré-appliquer le même état = Unchanged.
        let r2 = apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::TRIANGLE,
            simplex_flags::CYCLIC,
            0,
            &[10, 20, 30],
            &[],
        )
        .unwrap();
        assert_eq!(r2, AppliedSimplex::Unchanged);
    }

    #[test]
    fn hyperarete_heap_roundtrip_et_props() {
        let (_sub, mut dir, mut heap) = zones();
        let membres = vec![5u32, 9, 100, 4000];
        let props = b"{\"line\":42}".to_vec();
        apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::HYPEREDGE,
            0,
            3,
            &membres,
            &props,
        )
        .unwrap();
        let rec = SimplexDirZone::read(&dir, 0).unwrap().unwrap();
        assert!(rec.is_hyperedge());
        assert_eq!(rec.stored_key_hash(), key_hash_members(&membres));
        let (m, p) = read_members_props(&rec, &heap).unwrap();
        assert_eq!(m, membres);
        assert_eq!(p, props);
        // Réécriture des props seules : Rewritten, membres intacts.
        let r = apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::HYPEREDGE,
            0,
            3,
            &membres,
            b"{\"line\":43}",
        )
        .unwrap();
        assert_eq!(r, AppliedSimplex::Rewritten);
        let rec2 = SimplexDirZone::read(&dir, 0).unwrap().unwrap();
        let (m2, p2) = read_members_props(&rec2, &heap).unwrap();
        assert_eq!(m2, membres);
        assert_eq!(p2, b"{\"line\":43}");
    }

    #[test]
    fn props_interdites_sur_cliques() {
        let (_sub, mut dir, mut heap) = zones();
        let err = apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::TRIANGLE,
            0,
            0,
            &[1, 2, 3],
            b"nope",
        )
        .unwrap_err();
        assert!(matches!(err, SubstrateError::WalBadFrame(_)));
    }

    #[test]
    fn tombstone_idempotent_et_pas_de_resurrection() {
        let (_sub, mut dir, mut heap) = zones();
        apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::TRIANGLE,
            0,
            0,
            &[1, 2, 3],
            &[],
        )
        .unwrap();
        assert!(SimplexDirZone::tombstone_raw(&mut dir, 0).unwrap());
        assert!(!SimplexDirZone::tombstone_raw(&mut dir, 0).unwrap());
        // Rejouer le SimplexSet du même contenu ne ressuscite pas (I7).
        let r = apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::TRIANGLE,
            0,
            0,
            &[1, 2, 3],
            &[],
        )
        .unwrap();
        assert_eq!(r, AppliedSimplex::Unchanged);
        assert!(
            SimplexDirZone::read(&dir, 0)
                .unwrap()
                .unwrap()
                .is_tombstoned()
        );
    }

    #[test]
    fn index_unicite_et_collision_de_hash() {
        let (_sub, mut dir, mut heap) = zones();
        // Deux hyperarêtes distinctes, hashes réels distincts.
        apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[1, 2, 3],
            &[],
        )
        .unwrap();
        apply_set_raw(
            &mut dir,
            &mut heap,
            1,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[4, 5, 6],
            &[],
        )
        .unwrap();
        let idx = SimplexKeyIndex::build(&dir).unwrap();
        let h = key_hash_members(&[1, 2, 3]);
        assert_eq!(
            idx.find_verified(&dir, &heap, simplex_kind::HYPEREDGE, 1, h, &[1, 2, 3])
                .unwrap(),
            Some(0)
        );
        // Même clé, autre type_id : absent (l'unicité est PAR type).
        assert_eq!(
            idx.find_verified(&dir, &heap, simplex_kind::HYPEREDGE, 2, h, &[1, 2, 3])
                .unwrap(),
            None
        );

        // COLLISION FABRIQUÉE (I2) : deux records au même key_hash stocké
        // mais aux membres différents — write_raw contourne le calcul du
        // hash, exactement ce qu'une collision xxh3 réelle produirait.
        let mut a = SimplexRecord {
            kind: simplex_kind::HYPEREDGE,
            flags: 0,
            type_id: 9,
            member_count: 3,
            _reserved: 0,
            body: [0; 24],
        };
        a.set_stored_key_hash(0x1111);
        let off_a = SimplexHeapZone::append_snapshot(&mut heap, &[10, 11, 12], &[]).unwrap();
        a.set_payload_off(off_a);
        SimplexDirZone::write_raw(&mut dir, 2, &a).unwrap();
        let mut b = a;
        let off_b = SimplexHeapZone::append_snapshot(&mut heap, &[20, 21, 22], &[]).unwrap();
        b.set_payload_off(off_b);
        SimplexDirZone::write_raw(&mut dir, 3, &b).unwrap();

        let idx = SimplexKeyIndex::build(&dir).unwrap();
        // La résolution départage par les MEMBRES, jamais par le hash seul.
        assert_eq!(
            idx.find_verified(
                &dir,
                &heap,
                simplex_kind::HYPEREDGE,
                9,
                0x1111,
                &[10, 11, 12]
            )
            .unwrap(),
            Some(2)
        );
        assert_eq!(
            idx.find_verified(
                &dir,
                &heap,
                simplex_kind::HYPEREDGE,
                9,
                0x1111,
                &[20, 21, 22]
            )
            .unwrap(),
            Some(3)
        );
        assert_eq!(
            idx.find_verified(
                &dir,
                &heap,
                simplex_kind::HYPEREDGE,
                9,
                0x1111,
                &[30, 31, 32]
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn index_ignore_les_tombstones() {
        let (_sub, mut dir, mut heap) = zones();
        apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::TRIANGLE,
            0,
            0,
            &[1, 2, 3],
            &[],
        )
        .unwrap();
        SimplexDirZone::tombstone_raw(&mut dir, 0).unwrap();
        let idx = SimplexKeyIndex::build(&dir).unwrap();
        let h = key_hash_members(&[1, 2, 3]);
        assert_eq!(
            idx.find_verified(&dir, &heap, simplex_kind::TRIANGLE, 0, h, &[1, 2, 3])
                .unwrap(),
            None
        );
    }

    #[test]
    fn compaction_deterministe_i4() {
        // Base A : props écrites en deux temps (snapshot orphelin).
        let (_sa, mut dir_a, mut heap_a) = zones();
        apply_set_raw(
            &mut dir_a,
            &mut heap_a,
            0,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[1, 2, 3],
            b"v1",
        )
        .unwrap();
        apply_set_raw(
            &mut dir_a,
            &mut heap_a,
            0,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[1, 2, 3],
            b"v2",
        )
        .unwrap();
        apply_set_raw(
            &mut dir_a,
            &mut heap_a,
            1,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[4, 5, 6],
            b"w",
        )
        .unwrap();
        // Base B : mêmes simplexes vivants, histoire directe.
        let (_sb, mut dir_b, mut heap_b) = zones();
        apply_set_raw(
            &mut dir_b,
            &mut heap_b,
            0,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[1, 2, 3],
            b"v2",
        )
        .unwrap();
        apply_set_raw(
            &mut dir_b,
            &mut heap_b,
            1,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[4, 5, 6],
            b"w",
        )
        .unwrap();

        let used_a = compact_heap(&mut dir_a, &mut heap_a).unwrap();
        let used_b = compact_heap(&mut dir_b, &mut heap_b).unwrap();
        assert_eq!(used_a, used_b);
        let end = SIMPLEX_HEADER_SIZE + used_a as usize;
        assert_eq!(
            &heap_a.as_slice()[..end],
            &heap_b.as_slice()[..end],
            "I4 : mêmes simplexes vivants ⇒ heap byte-identique après compaction"
        );
        // Les records réancrés relisent le bon contenu.
        let rec = SimplexDirZone::read(&dir_a, 0).unwrap().unwrap();
        let (m, p) = read_members_props(&rec, &heap_a).unwrap();
        assert_eq!((m, p), (vec![1, 2, 3], b"v2".to_vec()));
        // La queue au-delà de payload_used est zéro-remplie.
        assert!(heap_a.as_slice()[end..].iter().all(|&b| b == 0));
    }

    #[test]
    fn compaction_purge_les_tombstones() {
        let (_s, mut dir, mut heap) = zones();
        apply_set_raw(
            &mut dir,
            &mut heap,
            0,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[1, 2, 3],
            b"x",
        )
        .unwrap();
        apply_set_raw(
            &mut dir,
            &mut heap,
            1,
            simplex_kind::HYPEREDGE,
            0,
            1,
            &[4, 5, 6],
            b"y",
        )
        .unwrap();
        SimplexDirZone::tombstone_raw(&mut dir, 0).unwrap();
        let used = compact_heap(&mut dir, &mut heap).unwrap();
        // Seul le snapshot de l'id 1 survit : 3×4 + 1 octet.
        assert_eq!(used, 13);
        let rec = SimplexDirZone::read(&dir, 1).unwrap().unwrap();
        let (m, p) = read_members_props(&rec, &heap).unwrap();
        assert_eq!((m, p), (vec![4, 5, 6], b"y".to_vec()));
    }
}
