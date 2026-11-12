//! # Blob columns — per-prop-key variable-length mmap storage
//!
//! **T16.7 Step 4 — foundation.** Implements the on-disk format,
//! writer and reader for "blob columns": one pair of mmap'd zones per
//! variable-length property key, generalising the dense
//! [`crate::vec_column`] pattern to payloads that do not all share the
//! same width.
//!
//! ## Why
//!
//! Two families of properties dominated `substrate.props` anon-RSS on
//! PO after T16.7 Step 3 landed vector routing:
//!
//! | Key        | Total  | Count  | Avg B | Kind        |
//! |-----------:|-------:|-------:|------:|-------------|
//! | `data`     | 928 MB | 681 k  | 1361  | `Value::String` (chat payload blobs) |
//! | `file_path`| 29 MB  | 295 k  | 97    | `Value::String` (< threshold, scalar) |
//!
//! Vectors could be routed because every vector for a given key has
//! the same `(dim, dtype)` — a single dense mmap is enough. Blobs are
//! variable-length, so they need an explicit `(offset, len)` per slot.
//!
//! ## On-disk layout (v1)
//!
//! Two files per column:
//!
//! ```text
//! substrate.blobcol.<kind>.<key_id_hex04>.idx
//!   ┌──────────────────────────────────────────────────────────────┐
//!   │ [64 B  BlobColumnHeader]    magic "BLB1", version, spec,     │
//!   │                             n_slots, arena_bytes, both CRCs  │
//!   │ [ n_slots × 16 B SlotEntry ] arena_offset u64, len u32, pad  │
//!   └──────────────────────────────────────────────────────────────┘
//!
//! substrate.blobcol.<kind>.<key_id_hex04>.dat
//!   ┌──────────────────────────────────────────────────────────────┐
//!   │ [ arena_bytes × u8 ]       concatenated raw payloads,        │
//!   │                            not padded, not aligned           │
//!   └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! Two files rather than one because the two regions grow on
//! different triggers: the index grows with `n_slots` (step up in
//! multiples of 16 B per newly-seen slot), the arena grows with
//! payload bytes (variable increments). Keeping them separate means
//! each file is `ensure_room`'d independently and no region-shift
//! dance is needed at grow time.
//!
//! **Absence semantics.** A `SlotEntry` with `len == 0` means
//! "absent" — indistinguishable from the zero-initialised bytes a
//! freshly-grown file carries. Empty blobs are rejected on write, the
//! same way empty vectors are rejected in
//! [`crate::vec_column::VecColumnWriter::write_slot`]. Callers who
//! need to represent "present but empty" should use [`Value::Null`](obrain_common::types::Value::Null)
//! or remove the key.
//!
//! ## Crash safety
//!
//! Same model as [`crate::vec_column`] and [`crate::tier_persist`]:
//!
//!   1. `ensure_room` grows both files.
//!   2. Payload bytes are memcpy'd into the arena; slot entry is
//!      written into the idx.
//!   3. CRC32 is computed over both live regions.
//!   4. Header is written into the idx file last, with both CRCs +
//!      the final `n_slots` + `arena_bytes`.
//!   5. `msync` + `fsync` make both files durable.
//!
//! A crash between steps 2 and 4 leaves a header with a stale
//! `(n_slots, arena_bytes, CRCs)` — the reader rejects it and the
//! column degrades to "absent", same as the vec-column contract.
//!
//! ## Scope of Step 4a
//!
//! This module delivers the **format** and the **I/O primitives**.
//! It is not yet called from `SubstrateStore`: routing of oversized
//! `Value::String` / `Value::Bytes` is Step 4d, and `obrain-migrate`
//! phase_nodes / phase_edges wiring is Step 4e. The tests here
//! exercise the module end-to-end in isolation via a temporary
//! substrate directory.

use bytemuck::{Pod, Zeroable};

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, ZoneFile};
use crate::vec_column::EntityKind;
use crate::writer::ensure_room;

// ---------------------------------------------------------------------------
// Format constants
// ---------------------------------------------------------------------------

/// Little-endian `u32` of the ASCII bytes `b"BLB1"`. Used as the
/// magic word in [`BlobColumnHeader::magic`]; chosen so `hexdump -C`
/// shows a human-readable identifier as the first 4 bytes of every
/// blob-column idx file.
pub const BLOB_COLUMN_MAGIC: u32 = u32::from_le_bytes(*b"BLB1");

/// Current on-disk format version. Bump together with the header
/// layout, the slot layout, or the arena encoding.
pub const BLOB_COLUMN_VERSION: u32 = 1;

/// Fixed header size in bytes. Same convention as [`crate::vec_column`]:
/// 64 B gives room for future non-mandatory fields without a version
/// bump (via the trailing `_reserved` block).
pub const BLOB_HEADER_SIZE: usize = 64;

/// Wire size of one [`BlobSlotEntry`].
pub const BLOB_SLOT_STRIDE: usize = 16;

// ---------------------------------------------------------------------------
// Spec + filenames
// ---------------------------------------------------------------------------

/// Descriptor of one blob column. The two fields are the primary key
/// of the on-disk zone pair: different prop-keys or different entity
/// kinds all land in **distinct file pairs**.
///
/// `prop_key_id` is the interned id assigned by the store's
/// `PropKeyRegistry` — stable across reopens because the registry
/// itself is persisted in `substrate.dict`. Unlike
/// [`crate::vec_column::VecColSpec`], there is no `dim` / `dtype`
/// field: blobs are variable-length opaque bytes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlobColSpec {
    pub prop_key_id: u16,
    pub entity_kind: EntityKind,
}

impl BlobColSpec {
    /// Filename of the slot-index file for this column.
    pub fn idx_filename(&self) -> String {
        format!(
            "substrate.blobcol.{}.{:04x}.idx",
            self.entity_kind.as_suffix(),
            self.prop_key_id
        )
    }

    /// Filename of the byte-arena file for this column.
    pub fn dat_filename(&self) -> String {
        format!(
            "substrate.blobcol.{}.{:04x}.dat",
            self.entity_kind.as_suffix(),
            self.prop_key_id
        )
    }
}

// ---------------------------------------------------------------------------
// On-disk types
// ---------------------------------------------------------------------------

/// 64 B `#[repr(C)]` header written at the start of every blob-column
/// idx file. All fields are little-endian.
///
/// Field order is frozen by [`BLOB_COLUMN_VERSION`]; the `_reserved`
/// tail gives 32 bytes for future extensions without reshuffling
/// offsets.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct BlobColumnHeader {
    /// [`BLOB_COLUMN_MAGIC`] — b"BLB1" little-endian.
    pub magic: u32,
    /// [`BLOB_COLUMN_VERSION`] at write time.
    pub version: u32,
    /// Matches [`BlobColSpec::prop_key_id`].
    pub prop_key_id: u16,
    /// 0 for node, 1 for edge. See [`EntityKind`].
    pub entity_kind: u8,
    /// Zero-filled padding to align `n_slots` on 4 B.
    pub _pad0: u8,
    /// Number of allocated slots. Slot `i` lives at
    /// `BLOB_HEADER_SIZE + i * BLOB_SLOT_STRIDE` in the idx file.
    pub n_slots: u32,
    /// Total bytes written into the arena file. The live payload
    /// occupies `[0, arena_bytes)` of the dat file.
    pub arena_bytes: u64,
    /// CRC32 (IEEE polynomial) over the idx slot region —
    /// `idx[BLOB_HEADER_SIZE .. BLOB_HEADER_SIZE + n_slots * 16]`.
    pub idx_crc32: u32,
    /// CRC32 (IEEE polynomial) over the live arena region —
    /// `dat[0 .. arena_bytes]`.
    pub arena_crc32: u32,
    /// Zero-filled padding to bring the header to 64 B.
    pub _reserved: [u32; 8],
}

// Compile-time sanity: the header is exactly 64 B.
const _: [(); 1] = [(); (core::mem::size_of::<BlobColumnHeader>() == BLOB_HEADER_SIZE) as usize];

impl BlobColumnHeader {
    fn new(spec: &BlobColSpec, n_slots: u32, arena_bytes: u64) -> Self {
        Self {
            magic: BLOB_COLUMN_MAGIC,
            version: BLOB_COLUMN_VERSION,
            prop_key_id: spec.prop_key_id,
            entity_kind: spec.entity_kind as u8,
            _pad0: 0,
            n_slots,
            arena_bytes,
            idx_crc32: 0,
            arena_crc32: 0,
            _reserved: [0u32; 8],
        }
    }

    /// Parse a header out of the first [`BLOB_HEADER_SIZE`] bytes of
    /// a mmap region. Returns `None` if the region is too short or
    /// fails alignment; never panics.
    pub(crate) fn try_read(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < BLOB_HEADER_SIZE {
            return None;
        }
        bytemuck::try_pod_read_unaligned::<BlobColumnHeader>(&bytes[..BLOB_HEADER_SIZE]).ok()
    }

    /// Validate the header matches the expected spec. Any mismatch
    /// is reported as a descriptive `&'static str`; the reader turns
    /// this into `Ok(None)` downstream.
    fn validate_against(&self, spec: &BlobColSpec) -> Result<(), &'static str> {
        if self.magic != BLOB_COLUMN_MAGIC {
            return Err("magic mismatch");
        }
        if self.version != BLOB_COLUMN_VERSION {
            return Err("version mismatch");
        }
        if self.prop_key_id != spec.prop_key_id {
            return Err("prop_key_id mismatch");
        }
        if self.entity_kind != spec.entity_kind as u8 {
            return Err("entity_kind mismatch");
        }
        Ok(())
    }
}

/// 16 B `#[repr(C)]` slot-directory entry written at
/// `BLOB_HEADER_SIZE + slot * 16` in the idx file.
///
/// `len == 0` means "absent" — the slot has never been written. The
/// writer rejects zero-length payloads so this marker is unambiguous.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct BlobSlotEntry {
    /// Offset into the dat file where this slot's payload begins.
    pub arena_offset: u64,
    /// Payload length in bytes. Zero = absent.
    pub len: u32,
    /// Zero-filled padding to bring the entry to 16 B.
    pub _pad: u32,
}

const _: [(); 1] = [(); (core::mem::size_of::<BlobSlotEntry>() == BLOB_SLOT_STRIDE) as usize];

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Variable-length writer for one blob column.
///
/// The writer keeps handles to both files (`.idx` and `.dat`). Each
/// `write_slot` appends the payload to the arena, updates the
/// matching slot entry in the idx, and bumps the high-water marks
/// (`n_slots`, `arena_end`). Durability is batched into [`Self::sync`]
/// (called from `SubstrateStore::flush`).
///
/// ## Usage pattern
///
/// ```ignore
/// let spec = BlobColSpec { prop_key_id: 7, entity_kind: EntityKind::Node };
/// let mut w = BlobColumnWriter::create(&sub, spec)?;
/// w.write_slot(42, b"hello world")?;
/// w.sync()?;
/// ```
///
/// Writes are **not** transactional at the entry level — a crash
/// between `write_slot` and the next `sync` leaves the on-disk
/// `n_slots` / `arena_bytes` counters behind the actual written
/// extent, which the reader will either truncate to or reject
/// depending on CRC state. In the worst case the last few writes
/// since the last sync are lost; no corruption of earlier data.
#[derive(Debug)]
pub struct BlobColumnWriter {
    spec: BlobColSpec,
    idx: ZoneFile,
    dat: ZoneFile,
    /// Highest slot index ever written +1. Monotonic.
    n_slots: u32,
    /// Bytes written into the arena so far. Monotonic.
    arena_end: u64,
    /// Running CRC32 over `dat[..arena_end]`, maintained INCRÉMENTALEMENT.
    ///
    /// (2026-08-17, mission perf DB) `sync` recalculait ce CRC sur toute
    /// l'arène à CHAQUE appel — donc relisait l'intégralité de la colonne
    /// même quand un seul payload avait bougé. Mesuré : 11,6 ms de flush
    /// pour une arène de 4 Mo, 106,4 ms pour 1 Go, en réécrivant 64 Ko.
    /// Coût LINÉAIRE EN L'ARÈNE, pas dans le volume écrit.
    ///
    /// En production (obrain-tmn appelle `checkpoint_all()` toutes les 60 s,
    /// jusqu'à 24 bases), la base `agent` porte ~36,5 Go d'arènes vives :
    /// ~608 Mo/s de relecture permanente, sous le verrou d'ÉCRITURE de
    /// chaque colonne — donc toute lecture de la colonne bloquait derrière.
    /// Mesuré sur le service : 2,83 To lus en 88 min (872 Mo/s soutenus),
    /// `read_bytes` ≫ `rchar`, et un p50 de requête à 1 622 ms.
    ///
    /// L'arène est append-only sous `arena_end` (cf. `write_slot`), donc
    /// `crc32(dat[..a+n]) = combine(crc32(dat[..a]), crc32(new), n)`. La
    /// valeur produite est IDENTIQUE au recalcul complet — invariant
    /// verrouillé par `incremental_arena_crc_matches_full_recompute`.
    arena_crc: u32,
    /// Y a-t-il eu une écriture depuis le dernier `sync` réussi ?
    ///
    /// Sans ce drapeau, `sync_all()` payait msync + fsync + CRC de l'idx sur
    /// TOUTES les colonnes à chaque checkpoint, y compris celles que
    /// personne n'avait touchées. Une colonne froide est désormais gratuite.
    dirty: bool,
    /// Epoch de la dernière repermutation de slots appliquée à cette
    /// colonne (chantier SlotRemappable). Persisté dans
    /// `header._reserved[0]` par chaque écriture d'en-tête, hérité au
    /// `create` — 0 = « jamais remappée ». Voir `crate::slot_remap`.
    remap_epoch: u32,
}

impl BlobColumnWriter {
    /// Create (or re-open) a blob-column pair for the given spec.
    ///
    /// If the column already exists on disk, the writer inherits its
    /// current `n_slots` / `arena_bytes` from the idx header so
    /// subsequent `write_slot`s past that point grow the column. An
    /// unreadable / mismatching idx header is treated as a fresh
    /// start — the old contents are overwritten.
    pub fn create(sub: &SubstrateFile, spec: BlobColSpec) -> SubstrateResult<Self> {
        let idx = sub.open_named_zone(&spec.idx_filename())?;
        let dat = sub.open_named_zone(&spec.dat_filename())?;
        // Le CRC d'arène stocké porte EXACTEMENT sur `dat[..arena_bytes]`,
        // et on reprend `arena_end = arena_bytes` : la valeur est donc
        // directement réutilisable comme préfixe de la chaîne incrémentale.
        // En-tête absent ou invalide → (0, 0, 0), et crc32(&[]) == 0.
        let (n_slots, arena_end, arena_crc, remap_epoch) =
            match BlobColumnHeader::try_read(idx.as_slice()) {
                Some(h) if h.validate_against(&spec).is_ok() => {
                    (h.n_slots, h.arena_bytes, h.arena_crc32, h._reserved[0])
                }
                // Colonne fraîche : estampillée avec l'epoch de remap
                // courant (même rationale que VecColumnWriter::create).
                _ => (0, 0, 0, crate::slot_remap::current_remap_epoch(sub)?),
            };
        // (obrain#120, relecture de #123) LA FIN D'ARÈNE DE L'EN-TÊTE PEUT ÊTRE
        // PÉRIMÉE. Elle n'est réécrite qu'au `sync` ; une écriture postérieure
        // (non suivie de sync : processus tué, écriture blob non journalisée
        // — kill entre la colonne et la trame WAL, valeur au-delà de
        // `BLOB_WAL_MAX_BYTES`, écrivain en écriture partagée qui ne rejoue
        // pas) a déjà ajouté ses octets à l'arène ET mis à jour l'entrée
        // d'index de son slot, toutes deux dans le mmap, donc survivantes.
        // Reprendre à `arena_bytes` ferait écrire l'ajout suivant PAR-DESSUS
        // ces octets : l'entrée relirait alors la valeur d'un autre nœud
        // (contamination). La fin d'arène reprise est donc le maximum de
        // l'en-tête et de `offset + len` de toute entrée d'index qui tient
        // dans l'arène. Le parcours porte sur TOUT l'idx, pas seulement
        // `n_slots` : une entrée au-delà de `n_slots` deviendrait visible le
        // jour où un slot plus grand fait monter `n_slots`. Coût : 16 o par
        // slot relus à l'ouverture, plus, seulement quand la fin a bougé (après
        // un arrêt sans sync), un recalcul du CRC sur l'arène.
        let (arena_end, arena_crc, dirty) = {
            let idx_octets = idx.as_slice();
            let dat_len = dat.as_slice().len() as u64;
            let mut fin = arena_end;
            let mut k = BLOB_HEADER_SIZE;
            while k + BLOB_SLOT_STRIDE <= idx_octets.len() {
                if let Ok(e) = bytemuck::try_pod_read_unaligned::<BlobSlotEntry>(
                    &idx_octets[k..k + BLOB_SLOT_STRIDE],
                ) && e.len > 0
                {
                    let bout = e.arena_offset.saturating_add(e.len as u64);
                    if bout <= dat_len {
                        fin = fin.max(bout);
                    }
                }
                k += BLOB_SLOT_STRIDE;
            }
            if fin > arena_end {
                let mut h = crc32fast::Hasher::new();
                h.update(&dat.as_slice()[..fin as usize]);
                (fin, h.finalize(), true)
            } else {
                (arena_end, arena_crc, false)
            }
        };
        Ok(Self {
            spec,
            idx,
            dat,
            n_slots,
            arena_end,
            arena_crc,
            dirty,
            remap_epoch,
        })
    }

    /// Append `bytes` as a new payload and record the `(offset, len)`
    /// in slot `slot`. Overwriting an existing slot does NOT reclaim
    /// the old arena bytes — they are left as garbage. This matches
    /// the append-only discipline of log-structured stores.
    ///
    /// Rejects empty payloads: `len == 0` is the absence marker in
    /// the on-disk slot directory.
    pub fn write_slot(&mut self, slot: u32, bytes: &[u8]) -> SubstrateResult<()> {
        if bytes.is_empty() {
            return Err(SubstrateError::WalBadFrame(
                "blob_column: cannot store empty payload (len=0 is the absence marker)".into(),
            ));
        }
        if bytes.len() > u32::MAX as usize {
            return Err(SubstrateError::WalBadFrame(format!(
                "blob_column: payload length {} exceeds u32::MAX",
                bytes.len()
            )));
        }

        let offset = self.arena_end;
        let len = bytes.len() as u32;

        // (1) Append to arena.
        let arena_needed_end = (offset as usize) + (len as usize);
        ensure_room(&mut self.dat, arena_needed_end, 0)?;
        {
            let region = self.dat.as_slice_mut();
            region[offset as usize..arena_needed_end].copy_from_slice(bytes);
        }
        // Prolonge le CRC courant au lieu de le refaire au prochain `sync`.
        // Valide UNIQUEMENT parce que rien sous `arena_end` n'est jamais
        // réécrit : c'est la discipline append-only documentée ci-dessus, et
        // toute réutilisation d'extent en place la casserait silencieusement.
        {
            let mut acc = crc32fast::Hasher::new_with_initial_len(self.arena_crc, self.arena_end);
            let mut tail = crc32fast::Hasher::new();
            tail.update(bytes);
            acc.combine(&tail);
            self.arena_crc = acc.finalize();
        }
        self.arena_end = offset + (len as u64);

        // (2) Write slot entry into idx.
        let slot_offset = BLOB_HEADER_SIZE + (slot as usize) * BLOB_SLOT_STRIDE;
        let slot_end = slot_offset + BLOB_SLOT_STRIDE;
        ensure_room(&mut self.idx, slot_end, 0)?;
        let entry = BlobSlotEntry {
            arena_offset: offset,
            len,
            _pad: 0,
        };
        {
            let region = self.idx.as_slice_mut();
            region[slot_offset..slot_end].copy_from_slice(bytemuck::bytes_of(&entry));
        }

        // (3) Bump n_slots HWM.
        if slot + 1 > self.n_slots {
            self.n_slots = slot + 1;
        }
        self.dirty = true;
        Ok(())
    }

    /// Marque le slot `slot` ABSENT (`len == 0`) sans toucher l'arène.
    ///
    /// ── POURQUOI (2026-08-20) ──
    ///
    /// Il n'existait AUCUN moyen d'effacer une valeur d'une colonne blob :
    /// `write_slot` REFUSE la charge vide, `len == 0` étant la sentinelle
    /// d'absence. Une valeur blob était donc ineffaçable — et comme l'ordre
    /// de lecture est vec → blob → v2 → DashMap, elle MASQUAIT toute
    /// réécriture ultérieure plus courte.
    ///
    /// MESURÉ (2026-08-20, base v2 neuve) : `SET n.body = <300 o>` puis
    /// `SET n.body = "court"` rend les 300 octets d'origine. Le défaut est
    /// ANTÉRIEUR à ce correctif et porte déjà sur `String`, `Bytes` et les
    /// listes routées le 19/08 ; il devient bloquant ici parce que router
    /// les cartes vers les blobs y exposerait une famille de plus.
    ///
    /// L'ARÈNE N'EST PAS TOUCHÉE — les octets restent en déchet, exactement
    /// comme lors d'un écrasement par `write_slot`. Le CRC d'arène, qui est
    /// prolongé de façon incrémentale, reste donc valide : c'est précisément
    /// la discipline append-only qui rend cette opération sûre. Seule
    /// l'entrée du répertoire `idx` change, et son CRC est recalculé en
    /// entier au prochain `sync`.
    pub fn clear_slot(&mut self, slot: u32) -> SubstrateResult<()> {
        if slot >= self.n_slots {
            // Jamais écrit sur cette colonne : déjà absent. On n'étend pas
            // `idx` pour y inscrire un zéro qu'il porte déjà.
            return Ok(());
        }
        let slot_offset = BLOB_HEADER_SIZE + (slot as usize) * BLOB_SLOT_STRIDE;
        let slot_end = slot_offset + BLOB_SLOT_STRIDE;
        ensure_room(&mut self.idx, slot_end, 0)?;
        let entry = BlobSlotEntry {
            arena_offset: 0,
            len: 0,
            _pad: 0,
        };
        {
            let region = self.idx.as_slice_mut();
            region[slot_offset..slot_end].copy_from_slice(bytemuck::bytes_of(&entry));
        }
        self.dirty = true;
        Ok(())
    }

    /// Read slot `slot` through the writer's own mmap. Returns `None`
    /// if the slot is past the current high-water mark or marked
    /// absent (`len == 0`).
    ///
    /// Mirrors [`BlobColumnReader::read_slot`] but serves reads
    /// through the same mmap that writes go through, so the T16.7
    /// routing path does not need to maintain a separate reader
    /// handle per spec. Between `write_slot` and the next `sync` the
    /// on-disk header still carries stale counters + CRCs, so a
    /// fresh [`BlobColumnReader`] would refuse to open — but the
    /// payload bytes are live in the mmap and safe to re-read here.
    pub fn read_slot(&self, slot: u32) -> Option<&[u8]> {
        if slot >= self.n_slots {
            return None;
        }
        let slot_offset = BLOB_HEADER_SIZE + (slot as usize) * BLOB_SLOT_STRIDE;
        let slot_end = slot_offset + BLOB_SLOT_STRIDE;
        let idx_slice = self.idx.as_slice();
        if slot_end > idx_slice.len() {
            return None;
        }
        let entry =
            bytemuck::try_pod_read_unaligned::<BlobSlotEntry>(&idx_slice[slot_offset..slot_end])
                .ok()?;
        if entry.len == 0 {
            return None;
        }
        let begin = entry.arena_offset as usize;
        let end = begin + entry.len as usize;
        let dat_slice = self.dat.as_slice();
        if end > dat_slice.len() {
            return None;
        }
        Some(&dat_slice[begin..end])
    }

    /// Force-finalise current state: compute both CRCs, write header
    /// into the idx file, `msync` + `fsync` both files. The writer
    /// stays alive so subsequent `write_slot`s can extend the column.
    /// Called from `SubstrateStore::flush`.
    pub fn sync(&mut self) -> SubstrateResult<()> {
        // Colonne froide : rien n'a bougé depuis le dernier sync réussi, donc
        // l'en-tête sur disque est déjà exact. On ne paie ni CRC, ni msync,
        // ni fsync. `sync_all()` balaie TOUTES les colonnes de la base à
        // chaque checkpoint (60 s en prod) — la plupart sont froides.
        if !self.dirty {
            return Ok(());
        }

        // Ensure idx file has room for the header + all slots.
        let idx_end = BLOB_HEADER_SIZE + (self.n_slots as usize) * BLOB_SLOT_STRIDE;
        ensure_room(&mut self.idx, idx_end, 0)?;

        // Ensure dat file has room for the live arena.
        ensure_room(&mut self.dat, self.arena_end as usize, 0)?;

        // CRC over slot region. Recalculé intégralement : le répertoire de
        // slots est modifié EN PLACE (une réécriture change l'entrée du slot),
        // donc la combinaison incrémentale ne s'y applique pas. C'est
        // acceptable : 16 o par slot, soit ~3 ordres de grandeur sous l'arène.
        let idx_crc = {
            let slice = self.idx.as_slice();
            let payload = &slice[BLOB_HEADER_SIZE..idx_end];
            let mut h = crc32fast::Hasher::new();
            h.update(payload);
            h.finalize()
        };

        // CRC over arena region — tenu à jour au fil des appends par
        // `write_slot`. Plus aucune relecture de l'arène ici : c'est CE
        // recalcul qui imposait ~608 Mo/s de lecture disque permanente en
        // production. Voir le commentaire de `arena_crc`.
        let arena_crc = self.arena_crc;

        let mut header = BlobColumnHeader::new(&self.spec, self.n_slots, self.arena_end);
        header.idx_crc32 = idx_crc;
        header.arena_crc32 = arena_crc;
        // Préservation de l'epoch de remap : un flush ne doit jamais le
        // remettre à zéro (sinon un rejeu ré-appliquerait la permutation).
        header._reserved[0] = self.remap_epoch;
        {
            let slice = self.idx.as_slice_mut();
            slice[..BLOB_HEADER_SIZE].copy_from_slice(bytemuck::bytes_of(&header));
        }

        self.idx.msync()?;
        self.idx.fsync()?;
        self.dat.msync()?;
        self.dat.fsync()?;
        // Uniquement après que TOUT a réussi : un `?` plus haut laisse la
        // colonne sale, et le prochain checkpoint réessaiera.
        self.dirty = false;
        Ok(())
    }

    /// Drop-consuming variant of [`Self::sync`]. Included for
    /// symmetry with [`crate::vec_column::VecColumnWriter::finalize`].
    pub fn finalize(mut self) -> SubstrateResult<()> {
        self.sync()
    }

    /// Current high-water mark (number of slots written so far).
    pub fn n_slots(&self) -> u32 {
        self.n_slots
    }

    /// Current high-water mark of the arena (bytes written so far).
    pub fn arena_end(&self) -> u64 {
        self.arena_end
    }

    /// The spec this writer targets.
    pub fn spec(&self) -> &BlobColSpec {
        &self.spec
    }

    /// Chantier SlotRemappable — applique une repermutation des slots
    /// nœuds à cette colonne (chemin vif). Seul le RÉPERTOIRE idx est
    /// permuté : l'arène est adressée par offset, elle ne bouge pas.
    ///
    /// ── M1 (revue adversariale #2, 2026-08-24) : POURQUOI PAR RENAME ──
    ///
    /// La version precedente ecrivait le repertoire permute EN PLACE dans
    /// le mmap idx vivant, puis appelait `sync()`. Un rejet de pages sales
    /// avant l'ecriture de l'en-tete laissait un idx a moitie permute avec
    /// un CRC perime : la colonne devenait « absente » au prochain open.
    ///
    /// Or « absent » n'a pas le meme prix selon la famille de side-car :
    ///
    /// * veccol : « absent » = embedding a re-migrer, la source existe ;
    /// * degres : `rebuild_from_scan` reconstruit depuis les aretes ;
    /// * **blob : il n'existe AUCUN chemin de reconstruction.** L'arene
    ///   `.dat` est intacte, mais le repertoire qui l'adresse est perdu —
    ///   donc toutes les proprietes volumineuses de cette cle le sont
    ///   aussi, definitivement. C'etait la SEULE famille ou un crash
    ///   mi-remap perdait des donnees sans recours.
    ///
    /// Le modele temporaire + `rename` (celui deja ecrit pour veccol)
    /// supprime la fenetre : le fichier idx vivant n'est jamais touche
    /// avant la bascule, qui est atomique. Un crash avant le rename laisse
    /// la colonne intacte en etat pre-permutation (epoch d'en-tete
    /// inchange), et le rejeu refait le travail.
    ///
    /// Le cout est ici derisoire — a 72 Mo de repertoire, le doublement
    /// transitoire est deux ordres de grandeur sous celui deja accepte
    /// pour `embedding_vec` (3,8 Go).
    pub fn remap_node_slots(
        &mut self,
        sub: &SubstrateFile,
        old_to_new: &[u32],
        epoch: u32,
    ) -> SubstrateResult<()> {
        if self.spec.entity_kind != EntityKind::Node {
            return Ok(());
        }
        if epoch <= self.remap_epoch {
            return Ok(()); // défensif — déjà appliquée
        }
        // La colonne peut porter des ecritures non encore synchronisees :
        // le temporaire est bati depuis le mmap VIVANT (etat authoritaire),
        // et l'arene doit etre rendue durable AVANT la bascule du
        // repertoire, sinon un idx neuf pourrait pointer dans une arene
        // dont la queue n'est pas sur disque.
        self.sync()?;

        let idx_end = BLOB_HEADER_SIZE + (self.n_slots as usize) * BLOB_SLOT_STRIDE;
        ensure_room(&mut self.idx, idx_end, 0)?;
        let permuted = {
            let slots = &self.idx.as_slice()[BLOB_HEADER_SIZE..idx_end];
            crate::slot_remap::permute_dense_payload(slots, BLOB_SLOT_STRIDE, old_to_new)
        };
        let n_slots = (permuted.len() / BLOB_SLOT_STRIDE) as u32;

        let idx_name = self.spec.idx_filename();
        // Temporaire DATE par l'epoch — cf. `vec_column` et
        // `slot_remap::nom_temporaire_remap` (revue #4).
        let tmp_name = crate::slot_remap::nom_temporaire_remap(&idx_name, epoch);
        let live_path = sub.path().join(&idx_name);
        let tmp_path = sub.path().join(&tmp_name);
        crate::slot_remap::effacer_temporaires_de_colonne(sub, &idx_name);
        {
            let mut tmp = sub.open_named_zone(&tmp_name)?;
            ensure_room(&mut tmp, BLOB_HEADER_SIZE + permuted.len(), 0)?;
            tmp.as_slice_mut()[BLOB_HEADER_SIZE..BLOB_HEADER_SIZE + permuted.len()]
                .copy_from_slice(&permuted);
            let idx_crc = {
                let mut h = crc32fast::Hasher::new();
                h.update(&tmp.as_slice()[BLOB_HEADER_SIZE..BLOB_HEADER_SIZE + permuted.len()]);
                h.finalize()
            };
            let mut header = BlobColumnHeader::new(&self.spec, n_slots, self.arena_end);
            header.idx_crc32 = idx_crc;
            header.arena_crc32 = self.arena_crc;
            header._reserved[0] = epoch;
            tmp.as_slice_mut()[..BLOB_HEADER_SIZE].copy_from_slice(bytemuck::bytes_of(&header));
            tmp.msync()?;
            tmp.fsync()?;
        }
        std::fs::rename(&tmp_path, &live_path).map_err(|e| {
            crate::error::SubstrateError::Internal(format!(
                "blob_column: rename du remap ({tmp_name}) a echoue: {e}"
            ))
        })?;
        // M2 — durabilite de la bascule (voir `slot_remap::fsync_repertoire`).
        crate::slot_remap::fsync_repertoire(&live_path);
        // L'ancien mmap pointe encore sur l'inode remplace : il ne doit
        // plus servir.
        self.idx = sub.open_named_zone(&idx_name)?;
        self.n_slots = n_slots;
        self.remap_epoch = epoch;
        // L'en-tete sur disque est exact (ecrit dans le temporaire) : la
        // colonne n'est PAS sale.
        self.dirty = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Remap de slots — chemin rejeu (fichier seul, sans registre en mémoire)
// ---------------------------------------------------------------------------

impl BlobColSpec {
    /// Inverse de [`Self::idx_filename`] — parse un nom de fichier
    /// `substrate.blobcol.<kind>.<key_id_hex04>.idx` en spec. `None`
    /// pour tout nom qui ne colle pas au schéma.
    pub fn parse_idx_filename(name: &str) -> Option<Self> {
        let rest = name.strip_prefix("substrate.blobcol.")?;
        let mut parts = rest.split('.');
        let kind = match parts.next()? {
            "node" => EntityKind::Node,
            "edge" => EntityKind::Edge,
            _ => return None,
        };
        let prop_key_id = u16::from_str_radix(parts.next()?, 16).ok()?;
        if parts.next()? != "idx" || parts.next().is_some() {
            return None;
        }
        Some(Self {
            prop_key_id,
            entity_kind: kind,
        })
    }
}

/// Chantier SlotRemappable — remap d'une colonne blob sur disque, sans
/// registre en mémoire (chemin rejeu après crash). Exige les DEUX CRC
/// valides (idx + arène) : une colonne dont l'arène est invalide est déjà
/// « absente » pour tout lecteur — permuter son répertoire n'apporterait
/// rien et graverait un état non vérifiable.
pub fn remap_node_file_for_replay(
    sub: &SubstrateFile,
    spec: BlobColSpec,
    old_to_new: &[u32],
    epoch: u32,
) -> SubstrateResult<crate::vec_column::VecRemapOutcome> {
    use crate::vec_column::VecRemapOutcome as Outcome;
    if spec.entity_kind != EntityKind::Node {
        return Ok(Outcome::Absent);
    }
    let idx = sub.open_named_zone(&spec.idx_filename())?;
    let dat = sub.open_named_zone(&spec.dat_filename())?;
    if idx.is_empty() {
        return Ok(Outcome::Absent);
    }
    let Some(header) = BlobColumnHeader::try_read(idx.as_slice()) else {
        return Ok(Outcome::Absent);
    };
    if header.validate_against(&spec).is_err() {
        return Ok(Outcome::Absent);
    }
    if header._reserved[0] >= epoch {
        return Ok(Outcome::AlreadyApplied);
    }
    let idx_end = BLOB_HEADER_SIZE + (header.n_slots as usize) * BLOB_SLOT_STRIDE;
    if idx.as_slice().len() < idx_end || (dat.as_slice().len() as u64) < header.arena_bytes {
        return Ok(Outcome::Absent);
    }
    let idx_crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&idx.as_slice()[BLOB_HEADER_SIZE..idx_end]);
        h.finalize()
    };
    if idx_crc != header.idx_crc32 {
        return Ok(Outcome::Absent);
    }
    let arena_crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&dat.as_slice()[..header.arena_bytes as usize]);
        h.finalize()
    };
    if arena_crc != header.arena_crc32 {
        return Ok(Outcome::Absent);
    }

    // Chemin REJEU : exactement le meme code que le chemin vif — c'est la
    // meilleure garantie que les deux ne divergent pas (et donc que le
    // rejeu beneficie aussi de la bascule par rename, M1).
    let mut writer = BlobColumnWriter {
        spec,
        idx,
        dat,
        n_slots: header.n_slots,
        arena_end: header.arena_bytes,
        arena_crc: header.arena_crc32,
        dirty: false,
        remap_epoch: header._reserved[0],
    };
    writer.remap_node_slots(sub, old_to_new, epoch)?;
    Ok(Outcome::Applied)
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Mmap-backed reader for one blob column.
///
/// Opening the reader validates:
///
/// 1. idx header magic / version / spec match,
/// 2. idx file is long enough to cover `n_slots * 16 B` past the header,
/// 3. dat file is long enough to cover `arena_bytes`,
/// 4. CRC32 over both regions matches the stored CRCs.
///
/// On any failure, `open` returns `Ok(None)` and the caller is
/// expected to fall back to "absent column" semantics. No
/// half-readable columns are surfaced.
///
/// [`Self::read_slot`] returns a zero-copy `&[u8]` slice into the
/// dat mmap, valid for the lifetime of the reader handle.
pub struct BlobColumnReader {
    spec: BlobColSpec,
    idx: ZoneFile,
    dat: ZoneFile,
    header: BlobColumnHeader,
}

impl BlobColumnReader {
    /// Open the blob-column pair matching `spec`. Returns:
    ///
    /// * `Ok(Some(reader))` on success,
    /// * `Ok(None)` if the zones do not exist, have a mismatching
    ///   header, or fail any CRC check — the caller is expected to
    ///   rebuild or fall back to an empty column,
    /// * `Err(_)` only on underlying I/O errors from the mmap
    ///   subsystem (i.e. "we couldn't even read the bytes").
    pub fn open(sub: &SubstrateFile, spec: BlobColSpec) -> SubstrateResult<Option<Self>> {
        let idx = sub.open_named_zone(&spec.idx_filename())?;
        let dat = sub.open_named_zone(&spec.dat_filename())?;

        // Empty idx file — fresh column.
        if idx.is_empty() {
            return Ok(None);
        }

        let idx_slice = idx.as_slice();
        let Some(header) = BlobColumnHeader::try_read(idx_slice) else {
            return Ok(None);
        };
        if header.validate_against(&spec).is_err() {
            return Ok(None);
        }

        // Length checks.
        let idx_end = BLOB_HEADER_SIZE + (header.n_slots as usize) * BLOB_SLOT_STRIDE;
        if idx_slice.len() < idx_end {
            return Ok(None);
        }
        let dat_slice = dat.as_slice();
        if (dat_slice.len() as u64) < header.arena_bytes {
            return Ok(None);
        }

        // CRC checks.
        let idx_crc = {
            let payload = &idx_slice[BLOB_HEADER_SIZE..idx_end];
            let mut h = crc32fast::Hasher::new();
            h.update(payload);
            h.finalize()
        };
        if idx_crc != header.idx_crc32 {
            return Ok(None);
        }
        let arena_crc = {
            let payload = &dat_slice[..header.arena_bytes as usize];
            let mut h = crc32fast::Hasher::new();
            h.update(payload);
            h.finalize()
        };
        if arena_crc != header.arena_crc32 {
            return Ok(None);
        }

        Ok(Some(Self {
            spec,
            idx,
            dat,
            header,
        }))
    }

    /// Number of slots currently stored.
    pub fn n_slots(&self) -> u32 {
        self.header.n_slots
    }

    /// Bytes written into the arena.
    pub fn arena_bytes(&self) -> u64 {
        self.header.arena_bytes
    }

    /// The spec this reader targets.
    pub fn spec(&self) -> &BlobColSpec {
        &self.spec
    }

    /// Borrow the raw bytes for one slot, or `None` if the slot is
    /// out of range or marked absent.
    ///
    /// The returned slice aliases the mmap directly — cheap in both
    /// latency (no copy) and memory (no anon allocation).
    pub fn read_slot(&self, slot: u32) -> Option<&[u8]> {
        if slot >= self.header.n_slots {
            return None;
        }
        let slot_offset = BLOB_HEADER_SIZE + (slot as usize) * BLOB_SLOT_STRIDE;
        let slot_end = slot_offset + BLOB_SLOT_STRIDE;
        let idx_slice = self.idx.as_slice();
        if slot_end > idx_slice.len() {
            return None;
        }
        let entry =
            bytemuck::try_pod_read_unaligned::<BlobSlotEntry>(&idx_slice[slot_offset..slot_end])
                .ok()?;
        if entry.len == 0 {
            return None;
        }
        let begin = entry.arena_offset as usize;
        let end = begin + entry.len as usize;
        let dat_slice = self.dat.as_slice();
        if end > dat_slice.len() {
            return None;
        }
        Some(&dat_slice[begin..end])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec(prop_key_id: u16) -> BlobColSpec {
        BlobColSpec {
            prop_key_id,
            entity_kind: EntityKind::Node,
        }
    }

    #[test]
    fn header_size_matches_constant() {
        assert_eq!(core::mem::size_of::<BlobColumnHeader>(), BLOB_HEADER_SIZE);
    }

    #[test]
    fn slot_stride_matches_constant() {
        assert_eq!(core::mem::size_of::<BlobSlotEntry>(), BLOB_SLOT_STRIDE);
    }

    #[test]
    fn magic_bytes_are_ascii_blb1() {
        assert_eq!(BLOB_COLUMN_MAGIC.to_le_bytes(), *b"BLB1");
    }

    /// L'INVARIANT du CRC incrémental (2026-08-17, mission perf DB).
    ///
    /// `sync` ne relit plus l'arène : il écrit le CRC tenu à jour au fil des
    /// appends. Ce test compare cette valeur au recalcul complet sur les
    /// octets réellement présents sur disque. S'il casse, les colonnes
    /// écrites deviennent illisibles au prochain open (le lecteur refuse un
    /// `arena_crc32` qui ne correspond pas) — donc perte de données.
    #[test]
    fn incremental_arena_crc_matches_full_recompute() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(11);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();

        // Tailles hétérogènes, réécritures du même slot, ordre non trivial :
        // on veut sortir du cas « un append bien aligné ».
        for i in 0..40u32 {
            let payload = vec![(i % 251) as u8; 1 + (i as usize * 37) % 900];
            w.write_slot(i % 9, &payload).unwrap();
            if i % 7 == 0 {
                w.sync().unwrap();
            }
        }
        w.sync().unwrap();

        let full = {
            let dat = sub.open_named_zone(&spec.dat_filename()).unwrap();
            let mut h = crc32fast::Hasher::new();
            h.update(&dat.as_slice()[..w.arena_end() as usize]);
            h.finalize()
        };
        assert_eq!(
            w.arena_crc, full,
            "le CRC incrémental doit être IDENTIQUE au recalcul complet"
        );

        // Et le juge qui compte vraiment : un lecteur neuf accepte le fichier.
        let r = BlobColumnReader::open(&sub, spec)
            .unwrap()
            .expect("le lecteur doit accepter la colonne");
        assert_eq!(r.arena_bytes(), w.arena_end());
    }

    /// Le CRC courant doit être repris depuis l'en-tête à la réouverture,
    /// sinon la chaîne incrémentale repart d'un préfixe faux et produit un
    /// fichier que plus personne ne peut ouvrir.
    #[test]
    fn incremental_arena_crc_survives_reopen() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(12);

        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"premiere generation").unwrap();
        w.sync().unwrap();
        drop(w);

        // Réouverture : hérite n_slots / arena_end / arena_crc de l'en-tête.
        let mut w2 = BlobColumnWriter::create(&sub, spec).unwrap();
        w2.write_slot(1, b"seconde generation, apres reouverture")
            .unwrap();
        w2.sync().unwrap();

        let full = {
            let dat = sub.open_named_zone(&spec.dat_filename()).unwrap();
            let mut h = crc32fast::Hasher::new();
            h.update(&dat.as_slice()[..w2.arena_end() as usize]);
            h.finalize()
        };
        assert_eq!(w2.arena_crc, full, "CRC faux après réouverture");

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.read_slot(0).unwrap(), b"premiere generation");
        assert_eq!(
            r.read_slot(1).unwrap(),
            b"seconde generation, apres reouverture"
        );
    }

    /// Une colonne froide ne doit plus rien coûter — mais elle doit rester
    /// parfaitement lisible. C'est l'autre moitié du gain : `sync_all()`
    /// balaie toutes les colonnes de la base à chaque checkpoint.
    #[test]
    fn cold_column_sync_is_a_noop_and_stays_readable() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(13);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"donnee durable").unwrap();
        w.sync().unwrap();
        assert!(!w.dirty, "sync réussi doit rendre la colonne propre");

        // Dix checkpoints sans une seule écriture : tous no-op.
        for _ in 0..10 {
            w.sync().unwrap();
            assert!(!w.dirty);
        }

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.read_slot(0).unwrap(), b"donnee durable");

        // Une écriture resalit, et le sync suivant reprend son travail.
        w.write_slot(1, b"seconde").unwrap();
        assert!(w.dirty, "une écriture doit resalir la colonne");
        w.sync().unwrap();
        let r2 = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r2.read_slot(1).unwrap(), b"seconde");
    }

    #[test]
    fn spec_filename_is_deterministic_and_descriptive() {
        let s = BlobColSpec {
            prop_key_id: 0x2A,
            entity_kind: EntityKind::Node,
        };
        assert_eq!(s.idx_filename(), "substrate.blobcol.node.002a.idx");
        assert_eq!(s.dat_filename(), "substrate.blobcol.node.002a.dat");

        let e = BlobColSpec {
            prop_key_id: 7,
            entity_kind: EntityKind::Edge,
        };
        assert_eq!(e.idx_filename(), "substrate.blobcol.edge.0007.idx");
        assert_eq!(e.dat_filename(), "substrate.blobcol.edge.0007.dat");
    }

    #[test]
    fn empty_zones_return_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let reader = BlobColumnReader::open(&sub, spec).unwrap();
        assert!(reader.is_none(), "fresh substrate has no blob column");
    }

    #[test]
    fn roundtrip_single_slot() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let payload = b"hello, substrate blob column!";

        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, payload).unwrap();
        assert_eq!(w.n_slots(), 1);
        assert_eq!(w.arena_end() as usize, payload.len());
        // Writer-served read works before sync.
        assert_eq!(w.read_slot(0).unwrap(), payload);
        assert!(w.read_slot(1).is_none());
        w.sync().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.n_slots(), 1);
        assert_eq!(r.arena_bytes() as usize, payload.len());
        assert_eq!(r.read_slot(0).unwrap(), payload);
        assert!(r.read_slot(1).is_none());
    }

    #[test]
    fn roundtrip_many_slots_variable_length() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(7);
        let n = 50u32;

        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        let mut expected: Vec<Vec<u8>> = Vec::with_capacity(n as usize);
        for slot in 0..n {
            let payload: Vec<u8> = (0..(slot as u8 + 1) * 3)
                .map(|i| i.wrapping_add(slot as u8))
                .collect();
            expected.push(payload.clone());
            w.write_slot(slot, &payload).unwrap();
        }
        assert_eq!(w.n_slots(), n);
        w.sync().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.n_slots(), n);
        for slot in 0..n {
            assert_eq!(r.read_slot(slot).unwrap(), &expected[slot as usize][..]);
        }
    }

    #[test]
    fn sparse_writes_have_absent_gaps() {
        // Write slot 0 and slot 10; slots 1..10 should be reported
        // as absent by the reader (len=0 SlotEntry from zero-init).
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(3);
        let p0 = b"first payload";
        let p10 = b"tenth payload, different length";

        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, p0).unwrap();
        w.write_slot(10, p10).unwrap();
        assert_eq!(w.n_slots(), 11, "n_slots tracks highest+1");
        w.sync().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.n_slots(), 11);
        assert_eq!(r.read_slot(0).unwrap(), p0);
        assert_eq!(r.read_slot(10).unwrap(), p10);
        for mid in 1..10 {
            assert!(
                r.read_slot(mid).is_none(),
                "slot {mid} must read as absent, got {:?}",
                r.read_slot(mid)
            );
        }
    }

    #[test]
    fn empty_payload_is_rejected() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        let err = w.write_slot(0, b"").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("empty"), "unexpected err: {msg}");
    }

    #[test]
    fn writer_reopen_inherits_counters() {
        // Create, write slots 0..5, sync. Reopen a new writer —
        // it must observe n_slots=5 and the arena_end so appending
        // to slot 5 doesn't clobber the live arena.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);

        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        for slot in 0..5 {
            let p = format!("payload-{slot}").into_bytes();
            w.write_slot(slot, &p).unwrap();
        }
        let n1 = w.n_slots();
        let arena1 = w.arena_end();
        w.sync().unwrap();
        drop(w);

        let mut w2 = BlobColumnWriter::create(&sub, spec).unwrap();
        assert_eq!(w2.n_slots(), n1, "reopen inherits prior n_slots");
        assert_eq!(w2.arena_end(), arena1, "reopen inherits prior arena_end");

        w2.write_slot(5, b"new entry").unwrap();
        assert_eq!(w2.n_slots(), 6);
        assert_eq!(w2.arena_end(), arena1 + b"new entry".len() as u64);
        w2.sync().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.n_slots(), 6);
        for slot in 0..5 {
            let expected = format!("payload-{slot}");
            assert_eq!(r.read_slot(slot).unwrap(), expected.as_bytes());
        }
        assert_eq!(r.read_slot(5).unwrap(), b"new entry");
    }

    /// (obrain#120, relecture de #123) Un écrivain relâché SANS `sync` laisse
    /// exactement l'état d'un processus tué : en-tête périmé (fin d'arène du
    /// dernier sync), entrées d'index et octets à jour dans le mmap. Le slot 0
    /// est réécrit et le slot 1 créé après le sync ; l'écrivain suivant ajoute
    /// au slot 2 une charge de même taille. Sans le correctif, il repartait de
    /// la fin d'arène de l'en-tête et écrivait par-dessus les octets du slot 0,
    /// qui relisait alors la valeur du slot 2 (contamination).
    #[test]
    fn un_ajout_apres_un_arret_sans_sync_n_ecrase_pas_les_octets_d_un_autre_slot() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(3);
        let a = vec![b'A'; 300];
        let b = vec![b'B'; 300];
        let c = vec![b'C'; 300];
        let d = vec![b'D'; 300];
        {
            let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
            w.write_slot(0, &a).unwrap();
            w.sync().unwrap();
            w.write_slot(0, &b).unwrap();
            w.write_slot(1, &c).unwrap();
            // Relâché sans sync : l'en-tête dit encore n_slots=1, arène=300.
        }
        let mut w2 = BlobColumnWriter::create(&sub, spec).unwrap();
        assert_eq!(
            w2.arena_end(),
            900,
            "la fin d'arène reprise couvre les octets des écritures non synchronisées"
        );
        w2.write_slot(2, &d).unwrap();
        assert_eq!(w2.read_slot(0).unwrap(), b.as_slice(), "slot 0 contaminé");
        assert_eq!(w2.read_slot(1).unwrap(), c.as_slice(), "slot 1 contaminé");
        assert_eq!(w2.read_slot(2).unwrap(), d.as_slice());
        w2.sync().unwrap();
        drop(w2);
        // Le CRC recalculé à l'ouverture puis prolongé reste valide : un lecteur
        // frais accepte la colonne et relit les trois valeurs.
        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.read_slot(0).unwrap(), b.as_slice());
        assert_eq!(r.read_slot(1).unwrap(), c.as_slice());
        assert_eq!(r.read_slot(2).unwrap(), d.as_slice());
    }

    /// Point 6 de la relecture : une entrée d'index AU-DELÀ de `n_slots` (écrite
    /// sans sync) est protégée par le scan (ses octets ne sont pas écrasés) mais
    /// pas effacée : quand un slot plus haut est écrit, `n_slots` monte et le
    /// slot protégé redevient lisible, avec la DERNIÈRE valeur écrite dedans.
    /// Verrouille ce comportement pour qu'un changement soit voulu.
    #[test]
    fn une_entree_au_dela_de_n_slots_ressurgit_avec_sa_derniere_valeur_si_n_slots_monte() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(5);
        let (a, b, c) = (vec![b'a'; 300], vec![b'b'; 300], vec![b'c'; 300]);
        {
            let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
            w.write_slot(0, &a).unwrap();
            w.sync().unwrap();
            w.write_slot(5, &b).unwrap(); // au-delà de n_slots=1, sans sync
        }
        let mut w2 = BlobColumnWriter::create(&sub, spec).unwrap();
        assert!(w2.read_slot(5).is_none(), "invisible tant que n_slots=1");
        w2.write_slot(9, &c).unwrap(); // n_slots monte à 10
        assert_eq!(w2.read_slot(5).unwrap(), b.as_slice(), "ressurgit, intacte");
        assert_eq!(w2.read_slot(9).unwrap(), c.as_slice());
        assert_eq!(w2.read_slot(0).unwrap(), a.as_slice());
    }

    /// NON-RÉGRESSION seulement (elle passe aussi sans le correctif) : à
    /// l'ouverture d'une colonne synchronisée, la fin d'arène et le CRC de
    /// l'en-tête sont repris tels quels (pas de recalcul, colonne froide).
    #[test]
    fn une_colonne_synchronisee_rouvre_sans_recalcul() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(4);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, &[7u8; 400]).unwrap();
        w.write_slot(0, &[8u8; 400]).unwrap();
        w.sync().unwrap();
        let (fin, crc) = (w.arena_end(), w.arena_crc);
        drop(w);
        let w2 = BlobColumnWriter::create(&sub, spec).unwrap();
        assert_eq!((w2.arena_end(), w2.arena_crc, w2.dirty), (fin, crc, false));
    }

    #[test]
    fn corrupt_idx_crc_returns_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"payload").unwrap();
        w.sync().unwrap();

        // Flip a byte inside the slot region of the idx file.
        {
            let mut zf = sub.open_named_zone(&spec.idx_filename()).unwrap();
            zf.as_slice_mut()[BLOB_HEADER_SIZE + 3] ^= 0xFF;
            zf.msync().unwrap();
        }

        let r = BlobColumnReader::open(&sub, spec).unwrap();
        assert!(r.is_none(), "corrupt idx CRC must degrade to None");
    }

    #[test]
    fn corrupt_arena_crc_returns_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"payload-to-corrupt").unwrap();
        w.sync().unwrap();

        // Flip a byte inside the live arena region of the dat file.
        {
            let mut zf = sub.open_named_zone(&spec.dat_filename()).unwrap();
            zf.as_slice_mut()[3] ^= 0xFF;
            zf.msync().unwrap();
        }

        let r = BlobColumnReader::open(&sub, spec).unwrap();
        assert!(r.is_none(), "corrupt arena CRC must degrade to None");
    }

    #[test]
    fn corrupt_magic_returns_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"X").unwrap();
        w.sync().unwrap();

        {
            let mut zf = sub.open_named_zone(&spec.idx_filename()).unwrap();
            zf.as_slice_mut()[0] ^= 0xFF;
            zf.msync().unwrap();
        }

        assert!(BlobColumnReader::open(&sub, spec).unwrap().is_none());
    }

    #[test]
    fn bumped_version_returns_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"X").unwrap();
        w.sync().unwrap();

        // Bump the version field (u32 at offset 4) to a value the
        // reader does not know.
        {
            let mut zf = sub.open_named_zone(&spec.idx_filename()).unwrap();
            let bad: u32 = BLOB_COLUMN_VERSION + 99;
            zf.as_slice_mut()[4..8].copy_from_slice(&bad.to_le_bytes());
            zf.msync().unwrap();
        }

        assert!(BlobColumnReader::open(&sub, spec).unwrap().is_none());
    }

    #[test]
    fn truncated_idx_returns_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let n = 10u32;
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        for slot in 0..n {
            w.write_slot(slot, format!("p{slot}").as_bytes()).unwrap();
        }
        w.sync().unwrap();

        // Truncate the idx file below its expected length.
        let expected = BLOB_HEADER_SIZE + (n as usize) * BLOB_SLOT_STRIDE;
        let path = sub.path().join(spec.idx_filename());
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(expected as u64 - 4).unwrap();
        drop(f);

        assert!(BlobColumnReader::open(&sub, spec).unwrap().is_none());
    }

    #[test]
    fn truncated_dat_returns_none_on_open() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"a-fairly-long-payload-to-truncate")
            .unwrap();
        w.sync().unwrap();

        // Truncate the dat file below arena_bytes.
        let path = sub.path().join(spec.dat_filename());
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(5).unwrap();
        drop(f);

        assert!(BlobColumnReader::open(&sub, spec).unwrap().is_none());
    }

    #[test]
    fn node_and_edge_same_key_use_distinct_files() {
        // Same prop_key_id but different entity_kind → distinct
        // files, no collision. Verifies the invariant that lets the
        // store route node-properties and edge-properties
        // independently.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let node_spec = BlobColSpec {
            prop_key_id: 5,
            entity_kind: EntityKind::Node,
        };
        let edge_spec = BlobColSpec {
            prop_key_id: 5,
            entity_kind: EntityKind::Edge,
        };
        assert_ne!(node_spec.idx_filename(), edge_spec.idx_filename());
        assert_ne!(node_spec.dat_filename(), edge_spec.dat_filename());

        let p_node = b"node payload";
        let p_edge = b"edge payload";

        let mut wn = BlobColumnWriter::create(&sub, node_spec).unwrap();
        wn.write_slot(0, p_node).unwrap();
        wn.sync().unwrap();
        let mut we = BlobColumnWriter::create(&sub, edge_spec).unwrap();
        we.write_slot(0, p_edge).unwrap();
        we.sync().unwrap();

        let rn = BlobColumnReader::open(&sub, node_spec).unwrap().unwrap();
        let re = BlobColumnReader::open(&sub, edge_spec).unwrap().unwrap();
        assert_eq!(rn.read_slot(0).unwrap(), p_node);
        assert_eq!(re.read_slot(0).unwrap(), p_edge);
    }

    #[test]
    fn overwrite_appends_new_bytes_and_updates_slot() {
        // Blob columns are append-only — overwriting a slot appends
        // the new payload to the arena and rewrites the slot entry
        // to point at it. The old bytes are left as garbage in the
        // arena but are no longer reachable through the slot entry.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();

        let p_old = b"the original payload";
        let p_new = b"a completely different replacement payload of a different length";
        w.write_slot(0, p_old).unwrap();
        let arena_after_old = w.arena_end();
        w.write_slot(0, p_new).unwrap();
        // Arena must have grown by len(new), not replaced.
        assert_eq!(w.arena_end(), arena_after_old + p_new.len() as u64);
        w.sync().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.read_slot(0).unwrap(), p_new);
    }

    #[test]
    fn writer_sync_then_read_through_fresh_reader() {
        // After sync(), a fresh reader must see everything written
        // so far. Subsequent writes through the same writer must
        // also become visible after another sync.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(2);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();

        w.write_slot(0, b"alpha").unwrap();
        w.sync().unwrap();
        {
            let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
            assert_eq!(r.read_slot(0).unwrap(), b"alpha");
        }

        w.write_slot(1, b"beta").unwrap();
        w.sync().unwrap();
        {
            let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
            assert_eq!(r.n_slots(), 2);
            assert_eq!(r.read_slot(0).unwrap(), b"alpha");
            assert_eq!(r.read_slot(1).unwrap(), b"beta");
        }
    }

    #[test]
    fn finalize_consumes_writer_and_syncs() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(1);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();
        w.write_slot(0, b"durable-on-drop").unwrap();
        w.finalize().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.read_slot(0).unwrap(), b"durable-on-drop");
    }

    #[test]
    fn large_payload_roundtrips() {
        // ~1 MiB payload, verifying the ensure_room growth policy
        // handles arenas that blow past the initial allocation.
        let sub = SubstrateFile::open_tempfile().unwrap();
        let spec = sample_spec(9);
        let mut w = BlobColumnWriter::create(&sub, spec).unwrap();

        let payload: Vec<u8> = (0..(1usize << 20)).map(|i| (i & 0xFF) as u8).collect();
        w.write_slot(0, &payload).unwrap();
        w.sync().unwrap();

        let r = BlobColumnReader::open(&sub, spec).unwrap().unwrap();
        assert_eq!(r.arena_bytes() as usize, payload.len());
        assert_eq!(r.read_slot(0).unwrap(), &payload[..]);
    }
}
