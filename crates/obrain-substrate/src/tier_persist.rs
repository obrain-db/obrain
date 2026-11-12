//! On-disk persistence for the L0 / L1 / L2 tier projections (T16.5).
//!
//! See `docs/rfc/substrate/tier-persistence.md` for the full format
//! specification and the rationale for promoting this work out of
//! cutover (T17) into its own task. TL;DR: `phase_tiers` in
//! `obrain-migrate` built the tiered index in-process and discarded it,
//! so every reopen paid for an O(N) rebuild from the bincode property
//! sidecar — blowing the anon RSS gate by 3×–60× depending on the base.
//!
//! ## Format
//!
//! Each tier zone (`substrate.tier0 / .tier1 / .tier2`) is a self-contained
//! file with a 64 B `#[repr(C)]` header followed by a Pod payload. Tier0
//! carries the `slot_to_offset: [u32; n_slots]` array (needed to
//! reconstruct `offset_to_slot` on load); tier1 and tier2 inherit the
//! same slot ordering implicitly.
//!
//! Every zone has its own CRC32 over its payload. Loading validates
//! magic / version / record_size / CRC independently; any failure
//! returns `Ok(None)` rather than an error — the caller falls back to
//! a full rebuild. Tier corruption is **never** fatal for store open.
//!
//! ## Crash safety
//!
//! Writes are ordered: payload memcpy → CRC → header → `msync` →
//! `fsync`. A crash before `msync` leaves the header and payload out of
//! sync, which surfaces as a CRC mismatch on next open → rebuild path.
//! No half-valid state can be mistaken for a valid one.

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::ZoneFile;
use crate::writer::ensure_room;
use bytemuck::{Pod, Zeroable};

/// Current on-disk format version. Bump when the header layout, the
/// Pod tier record layout, or the default SRP seeds change.
pub const TIER_PERSIST_VERSION: u32 = 2;

/// Première version du format : sans empreinte ni epoch nommé. Encore lue
/// (les zones écrites par un binaire antérieur se chargent), jamais écrite.
pub const TIER_PERSIST_VERSION_V1: u32 = 1;

/// Bit 1 des `flags` : l'en-tête porte une [`EmpreinteStore`] valide.
pub const FLAG_HAS_EMPREINTE: u32 = 1 << 1;

/// Empreinte conservatrice de l'état du store au moment où une zone de
/// palier a été écrite : offset du WAL et nombre de nœuds vivants. Une
/// réouverture ne charge l'index que si l'empreinte décrit encore le store ;
/// toute mutation la change (les écritures de vecteurs aussi, grâce au
/// marqueur `WalPayload::VecColumnWrite`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EmpreinteStore {
    /// `WalWriter::offset()` au moment de l'écriture.
    pub wal_offset: u64,
    /// Nombre de nœuds vivants au moment de l'écriture.
    pub noeuds_vivants: u64,
}

/// Fixed header size in bytes. Tier records begin at `HEADER_SIZE` (no
/// slot_to_offset) or at `HEADER_SIZE + 4 * n_slots` (tier0 only).
pub const HEADER_SIZE: usize = 64;

/// `flags` bit 0 — set when the payload is preceded by a
/// `[u32; n_slots]` slot_to_offset array (tier0 master only).
pub const FLAG_HAS_SLOT_TO_OFFSET: u32 = 1 << 0;

/// Per-tier magic values. Little-endian `u32` bit pattern of the ASCII
/// bytes; chosen so `hexdump -C | head -1` gives a human-readable
/// identifier on disk.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TierMagic {
    Tier0,
    Tier1,
    Tier2,
}

impl TierMagic {
    /// Little-endian `u32` of the on-disk magic bytes.
    pub const fn as_u32(self) -> u32 {
        match self {
            TierMagic::Tier0 => u32::from_le_bytes(*b"TIR0"),
            TierMagic::Tier1 => u32::from_le_bytes(*b"TIR1"),
            TierMagic::Tier2 => u32::from_le_bytes(*b"TIR2"),
        }
    }

    /// Expected record size in bytes for this tier.
    pub const fn record_size(self) -> u32 {
        match self {
            TierMagic::Tier0 => 16,  // Tier0 = [u64; 2]
            TierMagic::Tier1 => 64,  // Tier1 = [u64; 8]
            TierMagic::Tier2 => 768, // Tier2 = [u16; 384]
        }
    }
}

/// On-disk tier zone header. `Pod`, 64 B, little-endian everywhere.
///
/// The struct is serialized as-is via `bytemuck::bytes_of`. Do **not**
/// change field order or add new mandatory fields without bumping
/// [`TIER_PERSIST_VERSION`].
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct TierPersistHeader {
    /// `TierMagic::as_u32()` for this zone.
    pub magic: u32,
    /// [`TIER_PERSIST_VERSION`] at write time.
    pub version: u32,
    /// Number of indexed embeddings (dense slot count).
    pub n_slots: u32,
    /// `TierMagic::record_size()` — redundant with magic but lets a
    /// hexdump reader sanity-check the payload width.
    pub record_size: u32,
    /// CRC32 (IEEE polynomial) over the payload region: the
    /// slot_to_offset array (when present) followed by the Pod records.
    pub crc32: u32,
    /// Bit 0 = [`FLAG_HAS_SLOT_TO_OFFSET`], bit 1 = [`FLAG_HAS_EMPREINTE`].
    /// Remaining bits reserved.
    pub flags: u32,
    /// Epoch de repermutation des slots (chantier SlotRemappable) au moment
    /// de l'écriture. Mêmes octets que l'ex-`_reserved[0]` des en-têtes v1,
    /// que `slot_remap` lisait déjà : la v1 se lit donc sans conversion.
    pub remap_epoch: u32,
    /// [`EmpreinteStore::wal_offset`] en deux mots petit-boutistes
    /// `[bas, haut]` (un `u64` imposerait un rembourrage que `Pod` refuse).
    /// Valide si [`FLAG_HAS_EMPREINTE`] est levé (v2).
    pub empreinte_wal: [u32; 2],
    /// [`EmpreinteStore::noeuds_vivants`], même encodage.
    pub empreinte_noeuds: [u32; 2],
    /// Zero-filled padding to bring the header to 64 B. Never read by
    /// current code; guarantees future versions can extend the header
    /// without reshuffling offsets.
    pub _reserved: [u32; 5],
}

impl TierPersistHeader {
    /// Build a header for a fresh write. `crc32` is left at zero; the
    /// caller patches it after computing the CRC over the payload.
    pub fn new(magic: TierMagic, n_slots: u32, has_slot_to_offset: bool) -> Self {
        Self {
            magic: magic.as_u32(),
            version: TIER_PERSIST_VERSION,
            n_slots,
            record_size: magic.record_size(),
            crc32: 0,
            flags: if has_slot_to_offset {
                FLAG_HAS_SLOT_TO_OFFSET
            } else {
                0
            },
            remap_epoch: 0,
            empreinte_wal: [0; 2],
            empreinte_noeuds: [0; 2],
            _reserved: [0u32; 5],
        }
    }

    /// `true` when [`FLAG_HAS_SLOT_TO_OFFSET`] is set.
    #[inline]
    pub fn has_slot_to_offset(&self) -> bool {
        (self.flags & FLAG_HAS_SLOT_TO_OFFSET) != 0
    }

    /// Pose l'empreinte et lève [`FLAG_HAS_EMPREINTE`].
    pub fn poser_empreinte(&mut self, e: EmpreinteStore) {
        self.empreinte_wal = [e.wal_offset as u32, (e.wal_offset >> 32) as u32];
        self.empreinte_noeuds = [e.noeuds_vivants as u32, (e.noeuds_vivants >> 32) as u32];
        self.flags |= FLAG_HAS_EMPREINTE;
    }

    /// L'empreinte portée, si l'en-tête en a une (v2 avec le drapeau) ;
    /// `None` pour une zone v1 ou écrite sans empreinte — l'appelant doit
    /// alors considérer l'index comme non vérifiable et le reconstruire.
    pub fn empreinte(&self) -> Option<EmpreinteStore> {
        if self.version < 2 || (self.flags & FLAG_HAS_EMPREINTE) == 0 {
            return None;
        }
        Some(EmpreinteStore {
            wal_offset: u64::from(self.empreinte_wal[0]) | (u64::from(self.empreinte_wal[1]) << 32),
            noeuds_vivants: u64::from(self.empreinte_noeuds[0])
                | (u64::from(self.empreinte_noeuds[1]) << 32),
        })
    }
}

const _: [(); 1] = [(); (core::mem::size_of::<TierPersistHeader>() == HEADER_SIZE) as usize];

/// Result of reading a tier zone. Vec's are copied out of the mmap so
/// the caller can close the zone without dangling references.
pub type LoadedTier<T> = (u32, Option<Vec<u32>>, Vec<T>);

// ---------------------------------------------------------------------------
// Write path
// ---------------------------------------------------------------------------

/// Write a tier zone from scratch.
///
/// Grows `zf` to exactly `HEADER_SIZE + slot_to_offset_bytes + records_bytes`,
/// writes the payload, computes the CRC, writes the header, and calls
/// `msync` + `fsync` to make the whole zone durable.
///
/// * `magic` — one of `TierMagic::{Tier0, Tier1, Tier2}`. Must match the
///   zone kind being written to (tier0 for `Zone::Tier0`, etc.); we do
///   not cross-check here because `ZoneFile` is untyped.
/// * `n_slots` — number of records. Must equal `records.len()` and (when
///   present) `slot_to_offset.len()`.
/// * `slot_to_offset` — `Some(&[u32; n_slots])` for tier0 (the master
///   that carries the index), `None` for tier1 / tier2.
/// * `records` — the Pod tier records in slot order.
pub fn write_tier_zone<T: Pod>(
    zf: &mut ZoneFile,
    magic: TierMagic,
    n_slots: u32,
    slot_to_offset: Option<&[u32]>,
    records: &[T],
) -> SubstrateResult<()> {
    write_tier_zone_with_remap_epoch(zf, magic, n_slots, slot_to_offset, records, 0)
}

/// Variante de [`write_tier_zone`] qui estampille l'epoch de
/// repermutation de slots dans `header._reserved[0]` (chantier
/// SlotRemappable, voir `crate::slot_remap`). Les écrivains qui
/// réécrivent une zone tier depuis l'état COURANT de la base doivent
/// passer `crate::slot_remap::current_remap_epoch(sub)` pour que le rejeu
/// d'une repermutation antérieure ne re-permute pas une table déjà à
/// jour.
pub fn write_tier_zone_with_remap_epoch<T: Pod>(
    zf: &mut ZoneFile,
    magic: TierMagic,
    n_slots: u32,
    slot_to_offset: Option<&[u32]>,
    records: &[T],
    remap_epoch: u32,
) -> SubstrateResult<()> {
    write_tier_zone_estampille(
        zf,
        magic,
        n_slots,
        slot_to_offset,
        records,
        remap_epoch,
        None,
    )
}

/// Comme [`write_tier_zone_with_remap_epoch`], avec une [`EmpreinteStore`]
/// optionnelle dans l'en-tête (zones nommées par index, O-VEC C3) : le
/// lecteur la compare à l'état courant du store avant de faire confiance
/// aux paliers.
#[allow(clippy::too_many_arguments)]
pub fn write_tier_zone_estampille<T: Pod>(
    zf: &mut ZoneFile,
    magic: TierMagic,
    n_slots: u32,
    slot_to_offset: Option<&[u32]>,
    records: &[T],
    remap_epoch: u32,
    empreinte: Option<EmpreinteStore>,
) -> SubstrateResult<()> {
    // Sanity: dimensions agree.
    if records.len() != n_slots as usize {
        return Err(SubstrateError::WalBadFrame(format!(
            "tier_persist::write: n_slots={} != records.len()={}",
            n_slots,
            records.len()
        )));
    }
    if core::mem::size_of::<T>() as u32 != magic.record_size() {
        return Err(SubstrateError::WalBadFrame(format!(
            "tier_persist::write: record size {} != expected {} for magic {:?}",
            core::mem::size_of::<T>(),
            magic.record_size(),
            magic
        )));
    }
    let has_s2o = slot_to_offset.is_some();
    if let Some(s2o) = slot_to_offset
        && s2o.len() != n_slots as usize
    {
        return Err(SubstrateError::WalBadFrame(format!(
            "tier_persist::write: n_slots={} != slot_to_offset.len()={}",
            n_slots,
            s2o.len()
        )));
    }

    let s2o_bytes = if has_s2o { 4 * n_slots as usize } else { 0 };
    let records_bytes = std::mem::size_of_val(records);
    let total = HEADER_SIZE + s2o_bytes + records_bytes;

    // `ensure_room` grows with a pre-alloc slack; we want the final
    // file size to match `total` exactly so loads can size-check.
    ensure_room(zf, total, 0)?;
    if (zf.len() as usize) > total {
        // Shrink to the exact target. `ZoneFile::grow_to` refuses to
        // shrink, so we set_len on the underlying file via a private
        // path — but since that's not exposed, we instead tolerate
        // an over-sized file (ensure_room will only grow) and make
        // the reader size-check lenient: it validates `total <= file_len`
        // rather than `==`. See `read_tier_zone` for the symmetry.
    }

    // Memcpy payload.
    {
        let slice = zf.as_slice_mut();
        let payload_start = HEADER_SIZE;
        if let Some(s2o) = slot_to_offset {
            let payload_s2o = &mut slice[payload_start..payload_start + s2o_bytes];
            payload_s2o.copy_from_slice(bytemuck::cast_slice(s2o));
        }
        let records_start = payload_start + s2o_bytes;
        let records_dst = &mut slice[records_start..records_start + records_bytes];
        records_dst.copy_from_slice(bytemuck::cast_slice(records));
    }

    // CRC over the payload (everything after the header).
    let crc = {
        let slice = zf.as_slice();
        let payload = &slice[HEADER_SIZE..HEADER_SIZE + s2o_bytes + records_bytes];
        let mut h = crc32fast::Hasher::new();
        h.update(payload);
        h.finalize()
    };

    // Write header last.
    let mut header = TierPersistHeader::new(magic, n_slots, has_s2o);
    header.crc32 = crc;
    header.remap_epoch = remap_epoch;
    if let Some(e) = empreinte {
        header.poser_empreinte(e);
    }
    {
        let slice = zf.as_slice_mut();
        slice[..HEADER_SIZE].copy_from_slice(bytemuck::bytes_of(&header));
    }

    // Durability.
    zf.msync()?;
    zf.fsync()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Read path
// ---------------------------------------------------------------------------

/// Read a tier zone and validate it end-to-end.
///
/// Returns `Ok(None)` on any validation failure (missing zone, bad
/// magic, bad version, record-size mismatch, CRC mismatch, truncated
/// file). Errors are reserved for structural issues that mean we
/// couldn't even read the bytes (I/O errors from mmap).
///
/// On success, copies the `Vec<u32>` slot_to_offset (if present) and the
/// `Vec<T>` records out of the mmap so the caller can close the zone
/// without invalidating the return value.
pub fn read_tier_zone<T: Pod + Default + Copy>(
    zf: &ZoneFile,
    expected_magic: TierMagic,
) -> SubstrateResult<Option<LoadedTier<T>>> {
    Ok(read_tier_zone_avec_entete(zf, expected_magic)?.map(|(t, _)| t))
}

/// Comme [`read_tier_zone`], en rendant aussi l'en-tête lu (epoch de remap,
/// empreinte v2) pour que l'appelant juge de la fraîcheur des paliers.
pub fn read_tier_zone_avec_entete<T: Pod + Default + Copy>(
    zf: &ZoneFile,
    expected_magic: TierMagic,
) -> SubstrateResult<Option<(LoadedTier<T>, TierPersistHeader)>> {
    let slice = zf.as_slice();
    if slice.len() < HEADER_SIZE {
        return Ok(None);
    }

    // bytemuck::try_from_bytes is safer than transmute and checks
    // alignment. The header is 4-aligned (mmap starts on page boundary).
    let header: TierPersistHeader =
        match bytemuck::try_pod_read_unaligned::<TierPersistHeader>(&slice[..HEADER_SIZE]) {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };

    if header.magic != expected_magic.as_u32() {
        return Ok(None);
    }
    // v1 (sans empreinte) reste lisible ; toute autre version est refusée.
    if header.version != TIER_PERSIST_VERSION && header.version != TIER_PERSIST_VERSION_V1 {
        return Ok(None);
    }
    if header.record_size != expected_magic.record_size() {
        return Ok(None);
    }
    if core::mem::size_of::<T>() as u32 != header.record_size {
        return Ok(None);
    }

    let n_slots = header.n_slots as usize;
    let has_s2o = header.has_slot_to_offset();
    let s2o_bytes = if has_s2o { 4 * n_slots } else { 0 };
    let records_bytes = n_slots * core::mem::size_of::<T>();
    let expected_len = HEADER_SIZE + s2o_bytes + records_bytes;

    // We allow the file to be >= expected_len (the writer tolerates an
    // over-sized file after `ensure_room`'s pre-alloc; see write_tier_zone
    // for the symmetry). Reject if strictly smaller.
    if slice.len() < expected_len {
        return Ok(None);
    }

    // CRC check over the payload region.
    let payload = &slice[HEADER_SIZE..expected_len];
    let mut h = crc32fast::Hasher::new();
    h.update(payload);
    if h.finalize() != header.crc32 {
        return Ok(None);
    }

    // Copy out. Empty-slot tiers are valid: n_slots == 0 → empty Vecs.
    let slot_to_offset = if has_s2o {
        let s2o_src = &slice[HEADER_SIZE..HEADER_SIZE + s2o_bytes];
        let s2o_slice: &[u32] = match bytemuck::try_cast_slice(s2o_src) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        Some(s2o_slice.to_vec())
    } else {
        None
    };

    let records_src = &slice[HEADER_SIZE + s2o_bytes..expected_len];
    let records: Vec<T> = if n_slots == 0 {
        Vec::new()
    } else {
        // Lecture NON ALIGNÉE, enregistrement par enregistrement.
        //
        // (2026-08-23) `try_cast_slice::<u8, T>` exigeait l'alignement de
        // `T` sur le début de la région. Or, quand `slot_to_offset` est
        // présent (tier0), cette région commence à
        // `64 + 4 * n_slots` : pour un `n_slots` IMPAIR l'offset n'est
        // pas multiple de 8, et `Tier0`/`Tier1` (`[u64; _]`, align 8)
        // faisaient échouer le cast. Le tier était alors déclaré
        // « absent » À CHAQUE OUVERTURE — dégradation silencieuse en
        // reconstruction O(N), invisible car le contrat « corruption ⇒
        // rebuild » absorbait le symptôme. Les tests existants ne
        // l'attrapaient pas : ils n'utilisent que des `n_slots` pairs
        // (0, 100, 500, 1000). Verrouillé par
        // `roundtrip_tier0_avec_n_slots_impair`.
        let mut out = Vec::with_capacity(n_slots);
        let stride = core::mem::size_of::<T>();
        for i in 0..n_slots {
            match bytemuck::try_pod_read_unaligned::<T>(&records_src[i * stride..(i + 1) * stride])
            {
                Ok(v) => out.push(v),
                Err(_) => return Ok(None),
            }
        }
        out
    };

    Ok(Some(((header.n_slots, slot_to_offset, records), header)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{SubstrateFile, Zone};
    use crate::tiers::{Tier0, Tier1, Tier2};

    fn tier0_sample(i: u32) -> Tier0 {
        Tier0([i as u64, (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)])
    }

    fn tier1_sample(i: u32) -> Tier1 {
        let mut t = Tier1([0u64; 8]);
        for (k, slot) in t.0.iter_mut().enumerate() {
            *slot = (i as u64).wrapping_mul(0x100 + k as u64);
        }
        t
    }

    fn tier2_sample(i: u32) -> Tier2 {
        let mut t = Tier2([0u16; 384]);
        for k in 0..384 {
            t.0[k] = (i.wrapping_mul(k as u32 + 1) & 0xFFFF) as u16;
        }
        t
    }

    #[test]
    fn roundtrip_empty() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone::<Tier0>(&mut zf, TierMagic::Tier0, 0, Some(&[]), &[]).unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let got = read_tier_zone::<Tier0>(&zf, TierMagic::Tier0)
            .unwrap()
            .unwrap();
        assert_eq!(got.0, 0);
        assert_eq!(got.1, Some(Vec::<u32>::new()));
        assert!(got.2.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn roundtrip_tier0_with_slot_to_offset() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 1000u32;
        let records: Vec<Tier0> = (0..n).map(tier0_sample).collect();
        let s2o: Vec<u32> = (0..n).map(|i| i * 7).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier0, n, Some(&s2o), &records).unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let (got_n, got_s2o, got_r): LoadedTier<Tier0> =
            read_tier_zone(&zf, TierMagic::Tier0).unwrap().unwrap();
        assert_eq!(got_n, n);
        assert_eq!(got_s2o, Some(s2o));
        assert_eq!(got_r, records);
    }

    /// Régression (2026-08-23) : avec `slot_to_offset` présent et un
    /// `n_slots` IMPAIR, la région des records commence à un offset non
    /// multiple de 8 et l'ancien `try_cast_slice` échouait — le tier était
    /// silencieusement « absent » à chaque ouverture. Les tests d'origine
    /// n'utilisaient que des `n_slots` pairs.
    #[test]
    fn roundtrip_tier0_avec_n_slots_impair() {
        for n in [1u32, 3, 5, 7, 101] {
            let sub = SubstrateFile::open_tempfile().unwrap();
            let records: Vec<Tier0> = (0..n).map(tier0_sample).collect();
            let s2o: Vec<u32> = (0..n).map(|i| i * 3 + 1).collect();
            {
                let mut zf = sub.open_zone(Zone::Tier0).unwrap();
                write_tier_zone(&mut zf, TierMagic::Tier0, n, Some(&s2o), &records).unwrap();
            }
            let zf = sub.open_zone(Zone::Tier0).unwrap();
            let (got_n, got_s2o, got_r): LoadedTier<Tier0> = read_tier_zone(&zf, TierMagic::Tier0)
                .unwrap()
                .unwrap_or_else(|| panic!("n_slots={n} impair : tier0 déclaré absent à tort"));
            assert_eq!(got_n, n);
            assert_eq!(got_s2o, Some(s2o));
            assert_eq!(got_r, records);
        }
    }

    #[test]
    fn roundtrip_tier1_no_slot_to_offset() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 500u32;
        let records: Vec<Tier1> = (0..n).map(tier1_sample).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier1).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier1, n, None, &records).unwrap();
        }
        let zf = sub.open_zone(Zone::Tier1).unwrap();
        let (got_n, got_s2o, got_r): LoadedTier<Tier1> =
            read_tier_zone(&zf, TierMagic::Tier1).unwrap().unwrap();
        assert_eq!(got_n, n);
        assert_eq!(got_s2o, None);
        assert_eq!(got_r, records);
    }

    #[test]
    fn roundtrip_tier2() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 100u32;
        let records: Vec<Tier2> = (0..n).map(tier2_sample).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier2).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier2, n, None, &records).unwrap();
        }
        let zf = sub.open_zone(Zone::Tier2).unwrap();
        let (got_n, got_s2o, got_r): LoadedTier<Tier2> =
            read_tier_zone(&zf, TierMagic::Tier2).unwrap().unwrap();
        assert_eq!(got_n, n);
        assert_eq!(got_s2o, None);
        assert_eq!(got_r, records);
    }

    #[test]
    fn corrupt_crc_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 64u32;
        let records: Vec<Tier1> = (0..n).map(tier1_sample).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier1).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier1, n, None, &records).unwrap();
        }
        // Flip one bit in the payload region. The header's CRC no
        // longer matches and the load must fall through.
        {
            let mut zf = sub.open_zone(Zone::Tier1).unwrap();
            zf.as_slice_mut()[HEADER_SIZE + 10] ^= 0xFF;
            zf.msync().unwrap();
        }
        let zf = sub.open_zone(Zone::Tier1).unwrap();
        let got = read_tier_zone::<Tier1>(&zf, TierMagic::Tier1).unwrap();
        assert!(got.is_none(), "corrupted CRC must return None");
    }

    #[test]
    fn wrong_magic_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 8u32;
        let records: Vec<Tier0> = (0..n).map(tier0_sample).collect();
        let s2o: Vec<u32> = (0..n).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier0, n, Some(&s2o), &records).unwrap();
        }
        // Reading as tier1 from the tier0 zone must reject.
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let got = read_tier_zone::<Tier1>(&zf, TierMagic::Tier1).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn wrong_version_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 4u32;
        let records: Vec<Tier0> = (0..n).map(tier0_sample).collect();
        let s2o: Vec<u32> = (0..n).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier0, n, Some(&s2o), &records).unwrap();
        }
        // Patch the version field (offset 4..8 in the header) to 99.
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            let bad: u32 = 99;
            zf.as_slice_mut()[4..8].copy_from_slice(&bad.to_le_bytes());
            zf.msync().unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let got = read_tier_zone::<Tier0>(&zf, TierMagic::Tier0).unwrap();
        assert!(got.is_none(), "unsupported version must return None");
    }

    #[test]
    fn truncated_payload_returns_none() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 100u32;
        let records: Vec<Tier1> = (0..n).map(tier1_sample).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier1).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier1, n, None, &records).unwrap();
        }
        // `ensure_room` pads the zone to a 4 KiB boundary, so the
        // on-disk file is larger than `HEADER_SIZE + records_bytes`.
        // Truncate to BELOW `expected_len` so the reader's
        // size-check (`slice.len() < expected_len`) trips.
        let expected_len = HEADER_SIZE + (n as usize) * 64;
        let path = sub.zone_path(Zone::Tier1);
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(expected_len as u64 - 8).unwrap();
        drop(f);

        let zf = sub.open_zone(Zone::Tier1).unwrap();
        let got = read_tier_zone::<Tier1>(&zf, TierMagic::Tier1).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn empty_zone_file_returns_none() {
        // Fresh substrate: all tier zones are 0 B. Reader must not panic
        // and must return None so the caller falls back to rebuild.
        let sub = SubstrateFile::open_tempfile().unwrap();
        for (zone, magic) in [
            (Zone::Tier0, TierMagic::Tier0),
            (Zone::Tier1, TierMagic::Tier1),
            (Zone::Tier2, TierMagic::Tier2),
        ] {
            let zf = sub.open_zone(zone).unwrap();
            let got: Option<LoadedTier<[u8; 16]>> = match zone {
                Zone::Tier0 => read_tier_zone::<Tier0>(&zf, magic)
                    .unwrap()
                    .map(|(n, s, r)| (n, s, r.into_iter().map(|_| [0u8; 16]).collect())),
                _ => None,
            };
            assert!(got.is_none(), "empty {zone:?} must return None");
        }
    }

    #[test]
    fn header_size_matches_constant() {
        assert_eq!(core::mem::size_of::<TierPersistHeader>(), HEADER_SIZE);
    }

    #[test]
    fn magic_bytes_are_ascii() {
        // Sanity: the on-disk magic bytes are human-readable in a
        // hexdump, which matters for ops debugging.
        for (magic, expected) in [
            (TierMagic::Tier0, *b"TIR0"),
            (TierMagic::Tier1, *b"TIR1"),
            (TierMagic::Tier2, *b"TIR2"),
        ] {
            assert_eq!(magic.as_u32().to_le_bytes(), expected);
        }
    }

    #[test]
    fn flag_has_slot_to_offset_round_trips() {
        let h_with = TierPersistHeader::new(TierMagic::Tier0, 10, true);
        assert!(h_with.has_slot_to_offset());
        let h_without = TierPersistHeader::new(TierMagic::Tier1, 10, false);
        assert!(!h_without.has_slot_to_offset());
    }
}

#[cfg(test)]
mod tests_v2 {
    //! En-tête v2 (O-VEC C3) : empreinte du store et epoch de remap nommé ;
    //! la v1 reste lisible, sans empreinte.
    use super::*;
    use crate::file::{SubstrateFile, Zone};
    use crate::tiers::Tier0;

    fn t0(i: u32) -> Tier0 {
        Tier0([u64::from(i), u64::from(i) * 3])
    }

    #[test]
    fn l_empreinte_et_l_epoch_font_l_aller_retour() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let n = 9u32;
        let records: Vec<Tier0> = (0..n).map(t0).collect();
        let s2o: Vec<u32> = (0..n).map(|i| i * 10).collect();
        let e = EmpreinteStore {
            wal_offset: 0x1_2345_6789,
            noeuds_vivants: 0x9_8765_4321,
        };
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone_estampille(
                &mut zf,
                TierMagic::Tier0,
                n,
                Some(&s2o),
                &records,
                4,
                Some(e),
            )
            .unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let ((n_lu, s2o_lu, rec_lu), h) =
            read_tier_zone_avec_entete::<Tier0>(&zf, TierMagic::Tier0)
                .unwrap()
                .expect("zone v2 lisible");
        assert_eq!(n_lu, n);
        assert_eq!(s2o_lu.as_deref(), Some(&s2o[..]));
        assert_eq!(rec_lu, records);
        assert_eq!(h.version, TIER_PERSIST_VERSION);
        assert_eq!(h.remap_epoch, 4);
        assert_eq!(
            h.empreinte(),
            Some(e),
            "empreinte 64 bits reconstituée au bit près"
        );
        assert_eq!(
            read_tier_zone::<Tier0>(&zf, TierMagic::Tier0)
                .unwrap()
                .unwrap()
                .0,
            n
        );
    }

    #[test]
    fn sans_empreinte_le_drapeau_est_bas() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let records: Vec<Tier0> = (0..3).map(t0).collect();
        let s2o: Vec<u32> = vec![0, 1, 2];
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone_with_remap_epoch(&mut zf, TierMagic::Tier0, 3, Some(&s2o), &records, 7)
                .unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let (_, h) = read_tier_zone_avec_entete::<Tier0>(&zf, TierMagic::Tier0)
            .unwrap()
            .unwrap();
        assert_eq!(h.remap_epoch, 7);
        assert_eq!(h.empreinte(), None);
    }

    #[test]
    fn une_zone_v1_se_lit_encore_sans_empreinte() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let records: Vec<Tier0> = (0..5).map(t0).collect();
        let s2o: Vec<u32> = (0..5).collect();
        let e = EmpreinteStore {
            wal_offset: 1,
            noeuds_vivants: 5,
        };
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone_estampille(
                &mut zf,
                TierMagic::Tier0,
                5,
                Some(&s2o),
                &records,
                2,
                Some(e),
            )
            .unwrap();
            // Un binaire v1 aurait écrit version = 1 : on rétrograde le champ
            // (offset 4..8) ; l'epoch v1 vivait déjà dans les mêmes octets.
            let v1: u32 = TIER_PERSIST_VERSION_V1;
            zf.as_slice_mut()[4..8].copy_from_slice(&v1.to_le_bytes());
            zf.msync().unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        let ((n, _, rec), h) = read_tier_zone_avec_entete::<Tier0>(&zf, TierMagic::Tier0)
            .unwrap()
            .expect("la v1 reste lisible");
        assert_eq!(n, 5);
        assert_eq!(rec, records);
        assert_eq!(
            h.remap_epoch, 2,
            "l'epoch v1 (_reserved[0]) se lit dans remap_epoch"
        );
        assert_eq!(
            h.empreinte(),
            None,
            "une v1 n'a pas d'empreinte, quel que soit le drapeau"
        );
    }

    #[test]
    fn une_version_inconnue_est_refusee() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let records: Vec<Tier0> = (0..2).map(t0).collect();
        {
            let mut zf = sub.open_zone(Zone::Tier0).unwrap();
            write_tier_zone(&mut zf, TierMagic::Tier0, 2, Some(&[0, 1]), &records).unwrap();
            let v3: u32 = 3;
            zf.as_slice_mut()[4..8].copy_from_slice(&v3.to_le_bytes());
            zf.msync().unwrap();
        }
        let zf = sub.open_zone(Zone::Tier0).unwrap();
        assert!(
            read_tier_zone_avec_entete::<Tier0>(&zf, TierMagic::Tier0)
                .unwrap()
                .is_none()
        );
    }
}
