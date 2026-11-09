//! Retrieval façade — `VectorIndex` trait + substrate-backed implementation.
//!
//! The trait defines the public retrieval contract: search top-K by cosine
//! similarity over a corpus of f32 embeddings, plus incremental insert /
//! delete / rebuild. The implementation ([`SubstrateTieredIndex`]) uses the
//! [`tiers`](crate::tiers) L0 / L1 / L2 cascade and the bucket-capture
//! scanner from [`tiered_scan`](crate::tiered_scan) to meet the T8 latency
//! gate (≤ 1 ms p95 on 10⁶ nodes single-thread on AVX-512 hardware;
//! bandwidth-bound ~850 µs on Apple M2).
//!
//! ## Why abstract behind a trait?
//!
//! Callers (EngramRetriever, search modules, context loader) should bind
//! to the *behavior* — "find the k closest vectors" — rather than the
//! concrete representation. That lets us swap the in-memory-Vec impl for
//! a fully mmap'd one in T11 without touching a single caller. It also
//! kills the residual dependency on HNSW: the `retrieval` crate was
//! previously the only public path to hnswlib; once every caller goes
//! through `VectorIndex`, the HNSW implementation can be deleted outright
//! (T8 Step 6).
//!
//! ## API shape
//!
//! ```rust,ignore
//! use obrain_substrate::retrieval::{VectorIndex, SubstrateTieredIndex};
//!
//! let mut index = SubstrateTieredIndex::new(384);
//! index.insert(0, &embedding_for_node_0);
//! index.insert(1, &embedding_for_node_1);
//!
//! let hits = index.search_top_k(&query_embedding, 10);
//! for (node_offset, cosine) in hits {
//!     // cosine ∈ [-1, 1], higher = closer
//! }
//! ```

use crate::error::SubstrateResult;
use crate::file::{SubstrateFile, Zone};
use crate::tier_persist::{self, TierMagic};
use crate::tiered_scan::{ScanConfig, TieredQuery, scan_tiered};
use crate::tiers::{L2_DIM, Tier0, Tier0Builder, Tier1, Tier1Builder, Tier2, Tier2Builder};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

// (2026-08-13, T17b) `NodeOffset` et `VectorIndex` ont ete remontes dans
// `obrain-core::index::vector` — le trait ne mentionnait que u32, &[f32] et
// usize, donc rien qui appartienne au substrat, et `obrain-core::graph::traits`
// avait besoin de pouvoir le nommer pour cesser de typer ses methodes d'index
// sur le `HnswIndex` de LpgStore, retire depuis T17 Step 15.
//
// On les re-exporte ici pour que tout ce qui ecrivait
// `obrain_substrate::retrieval::VectorIndex` continue de compiler sans changement.
pub use obrain_core::index::vector::{NodeOffset, VectorIndex};

// ---------------------------------------------------------------------------
// SubstrateTieredIndex — in-memory implementation on top of the L0/L1/L2
// cascade. Mmap'd-backed variant comes in T11 (hilbert page ordering).
// ---------------------------------------------------------------------------

/// In-memory tiered vector index.
///
/// Storage layout:
///
/// ```text
///   offset_to_slot: HashMap<NodeOffset, usize>   // sparse → dense
///   slot_to_offset: Vec<NodeOffset>              // dense → sparse
///   l0 / l1 / l2:   Vec<TierN>                   // indexed by slot
/// ```
///
/// `slot` is the position in the tier vectors; `NodeOffset` is the
/// substrate-visible identity of the indexed node. Insert appends a new
/// slot; delete swap-removes and fixes up the mapping for the node that
/// was at the last slot.
///
/// ### Concurrency
///
/// The whole state is behind a single `RwLock` for simplicity. Reads
/// (search) take a shared lock, so concurrent queries fan out. Inserts /
/// deletes / rebuilds take the exclusive lock. In the hot path the
/// caller's Engine/UserBrain already serializes mutations per-db, so
/// contention is rare.
pub struct SubstrateTieredIndex {
    inner: RwLock<Inner>,
    b0: Tier0Builder,
    b1: Tier1Builder,
    b2: Tier2Builder,
    dim: usize,
    /// Chantier SlotRemappable — epoch de repermutation observé quand les
    /// identifiants de slots de cet index ont été établis.
    ///
    /// `None` = index construit EN MÉMOIRE (`new`) à partir de l'état
    /// courant : aucune garde, comportement historique inchangé.
    /// `Some(e)` = index CHARGÉ depuis les zones alors que la base était à
    /// l'epoch `e`. Si une repermutation survient ensuite, les slots que
    /// cet index détient sont PÉRIMÉS ; réécrire sa table écraserait la
    /// table déjà remappée sur disque par des valeurs fausses.
    /// [`Self::persist_to_zones`] refuse alors d'écrire.
    slots_epoch_at_load: Option<u32>,
}

struct Inner {
    offset_to_slot: HashMap<NodeOffset, usize>,
    slot_to_offset: Vec<NodeOffset>,
    l0: Vec<Tier0>,
    l1: Vec<Tier1>,
    l2: Vec<Tier2>,
}

impl SubstrateTieredIndex {
    /// Create a new empty index for embeddings of dimensionality `dim`.
    ///
    /// Uses the canonical default seeds for the L0 / L1 projection
    /// matrices — two indexes built with the same `dim` project to
    /// bit-identical tiers. This matters for the substrate import
    /// pipeline: the builder seed is the substrate equivalent of an
    /// "index schema version".
    ///
    /// # Panics
    ///
    /// Panics if `dim != L2_DIM` (currently 384). The L2 tier uses a
    /// fixed-length f16 representation tied to the canonical embedding
    /// width; supporting other widths would require either dimensionality
    /// reduction (loss of recall) or a ragged L2, both of which break the
    /// `#[repr(C)]` ABI contract. Callers with a different native width
    /// should project to L2_DIM before indexing.
    pub fn new(dim: usize) -> Self {
        assert_eq!(
            dim, L2_DIM,
            "SubstrateTieredIndex currently only supports dim = L2_DIM = {L2_DIM}; got {dim}"
        );
        Self {
            inner: RwLock::new(Inner {
                offset_to_slot: HashMap::new(),
                slot_to_offset: Vec::new(),
                l0: Vec::new(),
                l1: Vec::new(),
                l2: Vec::new(),
            }),
            b0: Tier0Builder::with_default_seed(dim),
            b1: Tier1Builder::with_default_seed(dim),
            b2: Tier2Builder::new(),
            dim,
            slots_epoch_at_load: None,
        }
    }

    /// Share the index across threads. Convenience for wiring into the
    /// trait-object-taking callers (`Arc<dyn VectorIndex>`).
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Pre-reserve capacity for at least `additional` more entries. Does
    /// nothing if the index already has room.
    pub fn reserve(&self, additional: usize) {
        let mut guard = self.inner.write();
        guard.offset_to_slot.reserve(additional);
        guard.slot_to_offset.reserve(additional);
        guard.l0.reserve(additional);
        guard.l1.reserve(additional);
        guard.l2.reserve(additional);
    }

    /// Construct an index from pre-computed parts. Used by
    /// [`Self::load_from_zones`] to re-hydrate an index from the
    /// on-disk tier zones without re-running the projection
    /// pipeline. Callers are responsible for coherence:
    ///
    /// * `slot_to_offset.len() == l0.len() == l1.len() == l2.len()`
    /// * `offset_to_slot[slot_to_offset[i]] == i` for every `i`
    ///
    /// Violations surface as incorrect search results, not panics.
    pub fn from_parts(
        dim: usize,
        slot_to_offset: Vec<NodeOffset>,
        offset_to_slot: HashMap<NodeOffset, usize>,
        l0: Vec<Tier0>,
        l1: Vec<Tier1>,
        l2: Vec<Tier2>,
    ) -> Self {
        assert_eq!(
            dim, L2_DIM,
            "SubstrateTieredIndex currently only supports dim = L2_DIM = {L2_DIM}; got {dim}"
        );
        Self {
            inner: RwLock::new(Inner {
                offset_to_slot,
                slot_to_offset,
                l0,
                l1,
                l2,
            }),
            b0: Tier0Builder::with_default_seed(dim),
            b1: Tier1Builder::with_default_seed(dim),
            b2: Tier2Builder::new(),
            dim,
            // Parts fournies par l'appelant, sans base de reference :
            // pas de garde (voir `load_from_zones`, qui estampille).
            slots_epoch_at_load: None,
        }
    }

    /// Estampille l'epoch de repermutation auquel les slots de cet index
    /// ont été établis (chantier SlotRemappable). Voir
    /// [`Self::persist_to_zones`] pour l'usage.
    fn with_slots_epoch(mut self, epoch: u32) -> Self {
        self.slots_epoch_at_load = Some(epoch);
        self
    }

    /// Index neuf, LIÉ à l'état de repermutation courant de la base.
    ///
    /// À utiliser partout où un index est bâti depuis les données vivantes
    /// puis persisté (reconstruction après un tier0 absent, migration).
    /// La liaison atteste que son contenu a été établi APRÈS la dernière
    /// repermutation ; sans elle, [`Self::persist_to_zones`] refuse de
    /// persister sur une base déjà repermutée.
    ///
    /// L'appelant doit construire l'index (`rebuild` / `insert`) SANS
    /// qu'une repermutation ne survienne entre cet appel et la
    /// persistance — ce que garantit la fenêtre calme des passes de
    /// maintenance.
    ///
    /// (Revue #3) Rend `Result` : une zone d'epoch presente mais illisible
    /// ne doit pas se traduire par un index estampille `epoch 0`, qui se
    /// croirait ensuite autorise a ecraser une table remappee.
    pub fn new_lie_a_la_base(sub: &SubstrateFile, dim: usize) -> SubstrateResult<Self> {
        let epoch = crate::slot_remap::current_remap_epoch(sub)?;
        Ok(Self::new(dim).with_slots_epoch(epoch))
    }

    /// Persist the current state of the index to the three tier zones
    /// (`substrate.tier0 / .tier1 / .tier2`) of `sub`.
    ///
    /// The index holds a read lock for the duration of the three
    /// zone writes — concurrent searches continue uninterrupted but
    /// inserts / deletes / rebuilds block until persistence is done.
    /// Each zone is written with its own header + CRC; see
    /// [`crate::tier_persist`] for the format.
    ///
    /// The write pattern is **one shot**: the full Vec is dumped to
    /// disk atomically at each call. There's no delta path. For the
    /// migration pipeline that's fine (persistence runs once at the
    /// end of `phase_tiers`); runtime hot paths that need delta
    /// persistence belong in T17.
    pub fn persist_to_zones(&self, sub: &SubstrateFile) -> SubstrateResult<()> {
        let guard = self.inner.read();
        let n_slots = guard.slot_to_offset.len() as u32;
        // Chantier SlotRemappable : la table écrite reflète la
        // disposition ACTUELLE des slots nœuds — estampillée avec
        // l'epoch de remap courant pour que le rejeu d'une
        // repermutation antérieure ne la re-permute pas.
        let remap_epoch = crate::slot_remap::current_remap_epoch(sub)?;

        // GARDE — index chargé AVANT une repermutation.
        //
        // Cet index détient des identifiants de slots figés au moment de
        // son chargement. Si la base a été repermutée depuis, sa table
        // `slot_to_offset` est PÉRIMÉE : l'écrire écraserait la table déjà
        // remappée sur disque par des slots faux, et l'estamperait de
        // l'epoch courant — donc sans plus aucun moyen de s'en apercevoir.
        //
        // On REFUSE d'écrire : la table sur disque, elle, est correcte
        // (remappée par la passe). L'index en mémoire reste périmé pour
        // les recherches jusqu'à son rechargement — c'est la limite
        // connue de son NON-ENRÔLEMENT (il appartient à `obrain-engine`,
        // pas au substrat). Mieux vaut un index périmé qu'une table
        // corrompue sur disque.
        // Deux refus, tous deux OBSERVABLES (`Err`, pas `Ok(())`) : une
        // persistance qui meurt en silence est un piège — l'appelant
        // croit avoir sauvegardé.
        match self.slots_epoch_at_load {
            // (a) Index CHARGÉ à un epoch donné, base repermutée depuis.
            Some(loaded_at) if loaded_at != remap_epoch => {
                return Err(crate::error::SubstrateError::Internal(format!(
                    "tier index: refus de persister — index charge a l'epoch de remap \
                     {loaded_at}, base desormais a l'epoch {remap_epoch}. Sa table \
                     slot_to_offset est PERIMEE et ecraserait la table deja remappee. \
                     Rechargez l'index (load_from_zones) avant de persister."
                )));
            }
            // (b) Index NON LIÉ (`new` / `from_parts`) sur une base qui a
            //     déjà été repermutée au moins une fois. Rien ne prouve
            //     que son contenu a été bâti APRÈS la dernière
            //     repermutation ; c'est exactement le chemin du rebuild
            //     quand tier0 est absent. On exige une liaison explicite.
            None if remap_epoch != 0 => {
                return Err(crate::error::SubstrateError::Internal(format!(
                    "tier index: refus de persister — index non lie a un epoch de remap \
                     sur une base qui en est a l'epoch {remap_epoch}. Construisez-le via \
                     SubstrateTieredIndex::new_lie_a_la_base(&sub, dim) pour attester \
                     qu'il a ete bati APRES la derniere repermutation."
                )));
            }
            _ => {}
        }

        // Tier0 carries slot_to_offset (master); tier1 and tier2 do not.
        {
            let mut zf0 = sub.open_zone(Zone::Tier0)?;
            tier_persist::write_tier_zone_with_remap_epoch::<Tier0>(
                &mut zf0,
                TierMagic::Tier0,
                n_slots,
                Some(&guard.slot_to_offset),
                &guard.l0,
                remap_epoch,
            )?;
        }
        {
            let mut zf1 = sub.open_zone(Zone::Tier1)?;
            tier_persist::write_tier_zone::<Tier1>(
                &mut zf1,
                TierMagic::Tier1,
                n_slots,
                None,
                &guard.l1,
            )?;
        }
        {
            let mut zf2 = sub.open_zone(Zone::Tier2)?;
            tier_persist::write_tier_zone::<Tier2>(
                &mut zf2,
                TierMagic::Tier2,
                n_slots,
                None,
                &guard.l2,
            )?;
        }
        Ok(())
    }

    /// Reconstruct an index from the tier zones of `sub`, or return
    /// `Ok(None)` when any zone is missing / corrupt / inconsistent.
    ///
    /// A successful load means **no projection work** is done — the
    /// tier records are copied out of the mmap verbatim. For a 5 M
    /// node PO substrate that's a ~4 GB memcpy (tier2 alone is
    /// 5 M × 768 B ≈ 3.8 GB), measured at 1-2 s vs 60 s for a
    /// full rebuild-from-properties path.
    ///
    /// All failures are soft: the caller falls back to
    /// [`Self::rebuild`]. Errors are only raised on I/O problems
    /// that mean we couldn't even touch the zone file (mmap
    /// permission denied, etc).
    pub fn load_from_zones(sub: &SubstrateFile, dim: usize) -> SubstrateResult<Option<Self>> {
        assert_eq!(
            dim, L2_DIM,
            "SubstrateTieredIndex currently only supports dim = L2_DIM = {L2_DIM}; got {dim}"
        );

        // Tier0 — includes slot_to_offset master.
        let zf0 = sub.open_zone(Zone::Tier0)?;
        let Some((n0, s2o, l0)) = tier_persist::read_tier_zone::<Tier0>(&zf0, TierMagic::Tier0)?
        else {
            return Ok(None);
        };
        let Some(slot_to_offset) = s2o else {
            // Tier0 without slot_to_offset is a format violation.
            return Ok(None);
        };

        // Tier1.
        let zf1 = sub.open_zone(Zone::Tier1)?;
        let Some((n1, _, l1)) = tier_persist::read_tier_zone::<Tier1>(&zf1, TierMagic::Tier1)?
        else {
            return Ok(None);
        };
        if n0 != n1 {
            return Ok(None);
        }

        // Tier2.
        let zf2 = sub.open_zone(Zone::Tier2)?;
        let Some((n2, _, l2)) = tier_persist::read_tier_zone::<Tier2>(&zf2, TierMagic::Tier2)?
        else {
            return Ok(None);
        };
        if n0 != n2 {
            return Ok(None);
        }

        // Sanity check vector lengths — read_tier_zone already guarantees
        // this, but the assertion is cheap and catches a class of future
        // refactors that might drift.
        let expected = n0 as usize;
        if slot_to_offset.len() != expected
            || l0.len() != expected
            || l1.len() != expected
            || l2.len() != expected
        {
            return Ok(None);
        }

        // Reconstruct offset_to_slot from slot_to_offset.
        let mut offset_to_slot = HashMap::with_capacity(expected);
        for (slot, &offset) in slot_to_offset.iter().enumerate() {
            offset_to_slot.insert(offset, slot);
        }

        // Chantier SlotRemappable : mémorise l'epoch auquel ces slots ont
        // été lus. Une repermutation ultérieure les rend PÉRIMÉS, et
        // `persist_to_zones` refusera alors d'écraser la table remappée.
        let epoch = crate::slot_remap::current_remap_epoch(sub)?;
        Ok(Some(
            Self::from_parts(dim, slot_to_offset, offset_to_slot, l0, l1, l2)
                .with_slots_epoch(epoch),
        ))
    }

    fn project_query(&self, query: &[f32]) -> TieredQuery {
        assert_eq!(
            query.len(),
            self.dim,
            "query.len() = {} but index dim = {}",
            query.len(),
            self.dim
        );
        TieredQuery {
            l0: self.b0.project(query),
            l1: self.b1.project(query),
            l2: self.b2.project(query),
        }
    }
}

impl VectorIndex for SubstrateTieredIndex {
    fn dim(&self) -> usize {
        self.dim
    }

    fn len(&self) -> usize {
        self.inner.read().slot_to_offset.len()
    }

    fn insert(&self, node_offset: NodeOffset, embedding: &[f32]) {
        assert_eq!(
            embedding.len(),
            self.dim,
            "embedding.len() = {} but index dim = {}",
            embedding.len(),
            self.dim
        );
        let tier0 = self.b0.project(embedding);
        let tier1 = self.b1.project(embedding);
        let tier2 = self.b2.project(embedding);

        let mut guard = self.inner.write();
        if let Some(&slot) = guard.offset_to_slot.get(&node_offset) {
            // Update in place — keep the slot stable so ongoing scans
            // (if any — but we hold the write lock, so none) see the
            // fresh fingerprint.
            guard.l0[slot] = tier0;
            guard.l1[slot] = tier1;
            guard.l2[slot] = tier2;
            return;
        }
        let slot = guard.slot_to_offset.len();
        guard.slot_to_offset.push(node_offset);
        guard.l0.push(tier0);
        guard.l1.push(tier1);
        guard.l2.push(tier2);
        guard.offset_to_slot.insert(node_offset, slot);
    }

    fn delete(&self, node_offset: NodeOffset) {
        let mut guard = self.inner.write();
        let Some(slot) = guard.offset_to_slot.remove(&node_offset) else {
            return;
        };
        let last = guard.slot_to_offset.len() - 1;
        if slot != last {
            // Swap-remove: move the last slot into `slot` and update
            // its offset → slot mapping. This keeps the tier vectors
            // dense without a shift.
            let swapped = guard.slot_to_offset[last];
            guard.slot_to_offset.swap(slot, last);
            guard.l0.swap(slot, last);
            guard.l1.swap(slot, last);
            guard.l2.swap(slot, last);
            guard.offset_to_slot.insert(swapped, slot);
        }
        guard.slot_to_offset.pop();
        guard.l0.pop();
        guard.l1.pop();
        guard.l2.pop();
    }

    fn rebuild(&self, pairs: &[(NodeOffset, Vec<f32>)]) {
        // Pre-project off the write lock — building 3 tiers per pair
        // is the slowest step and shouldn't block readers any longer
        // than necessary.
        let n = pairs.len();
        let mut offsets: Vec<NodeOffset> = Vec::with_capacity(n);
        let mut l0: Vec<Tier0> = Vec::with_capacity(n);
        let mut l1: Vec<Tier1> = Vec::with_capacity(n);
        let mut l2: Vec<Tier2> = Vec::with_capacity(n);
        let mut map: HashMap<NodeOffset, usize> = HashMap::with_capacity(n);
        for (i, (off, emb)) in pairs.iter().enumerate() {
            assert_eq!(
                emb.len(),
                self.dim,
                "pair {i}: embedding.len() = {} but index dim = {}",
                emb.len(),
                self.dim
            );
            offsets.push(*off);
            l0.push(self.b0.project(emb));
            l1.push(self.b1.project(emb));
            l2.push(self.b2.project(emb));
            map.insert(*off, i);
        }

        // Swap in the new state under the write lock. Cheap — just
        // moves of Vec headers.
        let mut guard = self.inner.write();
        guard.slot_to_offset = offsets;
        guard.l0 = l0;
        guard.l1 = l1;
        guard.l2 = l2;
        guard.offset_to_slot = map;
    }

    fn search_top_k(&self, query: &[f32], k: usize) -> Vec<(NodeOffset, f32)> {
        if k == 0 {
            return Vec::new();
        }
        let q = self.project_query(query);
        let guard = self.inner.read();
        if guard.slot_to_offset.is_empty() {
            return Vec::new();
        }
        // Clamp k to the corpus size — the cascade panics otherwise.
        let effective_k = k.min(guard.slot_to_offset.len());
        let hits = scan_tiered(
            &q,
            &guard.l0,
            &guard.l1,
            &guard.l2,
            effective_k,
            ScanConfig::default(),
        );
        // Translate slot indices back into NodeOffsets. The cascade's
        // tie-break is `slot` ascending, which aligns with NodeOffset
        // ascending *iff* insertions happened in offset order. To keep
        // the trait's contract ("ties broken by ascending NodeOffset")
        // we do a final stable sort — the working set is ≤ k ≤ 1 000,
        // so the cost is negligible.
        let mut out: Vec<(NodeOffset, f32)> = hits
            .into_iter()
            .map(|h| (guard.slot_to_offset[h.node_offset as usize], h.cosine))
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // The L2 tier is fixed at 384 f16 (see `tiers::L2_DIM`), so the index
    // only supports that width. Use it for all tests to exercise the real
    // code path — a smaller test dim would need a separate mini-tier.
    const DIM: usize = L2_DIM;

    fn emb(seed: u32, dim: usize) -> Vec<f32> {
        // Deterministic "embedding": a rotating sine pattern keyed on seed.
        // Not a real embedding — just enough structure that distinct seeds
        // produce distinct projections with high probability.
        (0..dim)
            .map(|i| (seed as f32 * 0.3 + i as f32 * 0.15).sin())
            .collect()
    }

    #[test]
    fn empty_index_returns_empty_top_k() {
        let idx = SubstrateTieredIndex::new(DIM);
        assert!(idx.is_empty(), "résultat attendu vide");
        assert_eq!(idx.search_top_k(&emb(0, DIM), 10), Vec::new());
    }

    #[test]
    fn insert_then_self_recall() {
        let idx = SubstrateTieredIndex::new(DIM);
        idx.insert(42, &emb(42, DIM));
        idx.insert(7, &emb(7, DIM));
        idx.insert(1234, &emb(1234, DIM));
        assert_eq!(idx.len(), 3);

        let q = emb(42, DIM);
        let hits = idx.search_top_k(&q, 3);
        assert_eq!(hits.len(), 3);
        // Self-recall: the query is an indexed embedding verbatim — must
        // come back as top-1 with cosine ≈ 1.
        assert_eq!(hits[0].0, 42);
        assert!(hits[0].1 > 0.99, "self-cosine = {}", hits[0].1);
    }

    #[test]
    fn update_in_place_rewrites_fingerprint() {
        let idx = SubstrateTieredIndex::new(DIM);
        idx.insert(42, &emb(42, DIM));
        // Overwrite with a totally different embedding.
        idx.insert(42, &emb(99, DIM));
        assert_eq!(idx.len(), 1, "duplicate insert must not append a new slot");

        // Now self-recall with emb(99) should win — emb(42) should not.
        let hits_99 = idx.search_top_k(&emb(99, DIM), 1);
        assert_eq!(hits_99[0].0, 42);
        assert!(hits_99[0].1 > 0.99, "updated embedding not visible");
    }

    #[test]
    fn delete_removes_offset() {
        let idx = SubstrateTieredIndex::new(DIM);
        for n in [10, 20, 30, 40] {
            idx.insert(n, &emb(n, DIM));
        }
        assert_eq!(idx.len(), 4);

        idx.delete(20);
        assert_eq!(idx.len(), 3);

        // The deleted offset must never surface in results, even on a
        // query crafted from its embedding.
        let hits = idx.search_top_k(&emb(20, DIM), 4);
        assert!(
            hits.iter().all(|(o, _)| *o != 20),
            "deleted offset resurfaced: {hits:?}"
        );
        // The other three must still be reachable.
        for n in [10, 30, 40] {
            let hits = idx.search_top_k(&emb(n, DIM), 1);
            assert_eq!(hits[0].0, n);
        }
    }

    #[test]
    fn delete_of_missing_offset_is_noop() {
        let idx = SubstrateTieredIndex::new(DIM);
        idx.insert(1, &emb(1, DIM));
        idx.delete(9999); // never indexed
        assert_eq!(idx.len(), 1);
        // The live offset is still reachable.
        let hits = idx.search_top_k(&emb(1, DIM), 1);
        assert_eq!(hits[0].0, 1);
    }

    #[test]
    fn rebuild_replaces_contents() {
        let idx = SubstrateTieredIndex::new(DIM);
        for n in 0..10 {
            idx.insert(n, &emb(n, DIM));
        }
        assert_eq!(idx.len(), 10);

        // Rebuild with a totally different set.
        let pairs: Vec<(NodeOffset, Vec<f32>)> = (100..105).map(|n| (n, emb(n, DIM))).collect();
        idx.rebuild(&pairs);
        assert_eq!(idx.len(), 5);

        // Old offsets are gone.
        for n in 0..10 {
            let hits = idx.search_top_k(&emb(n, DIM), 1);
            // The cascade may still return something (nearest in the new
            // set) but must never return a pre-rebuild offset.
            assert!(hits[0].0 >= 100);
        }
        // New offsets are reachable.
        for n in 100..105 {
            let hits = idx.search_top_k(&emb(n, DIM), 1);
            assert_eq!(hits[0].0, n);
        }
    }

    #[test]
    fn search_top_k_clamps_to_corpus() {
        let idx = SubstrateTieredIndex::new(DIM);
        for n in 0..3 {
            idx.insert(n, &emb(n, DIM));
        }
        // Asking for more than the corpus holds — cascade must clamp,
        // not panic.
        let hits = idx.search_top_k(&emb(0, DIM), 10);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn search_top_k_zero_returns_empty() {
        let idx = SubstrateTieredIndex::new(DIM);
        idx.insert(1, &emb(1, DIM));
        assert!(idx.search_top_k(&emb(1, DIM), 0).is_empty());
    }

    #[test]
    fn trait_object_is_usable() {
        // Compile-time check: `SubstrateTieredIndex` is usable behind a
        // `dyn VectorIndex`. This mirrors how the cognitive layer holds
        // the index (`Arc<dyn VectorIndex>`).
        let idx: Arc<dyn VectorIndex> = Arc::new(SubstrateTieredIndex::new(DIM));
        idx.insert(5, &emb(5, DIM));
        let hits = idx.search_top_k(&emb(5, DIM), 1);
        assert_eq!(hits[0].0, 5);
    }

    #[test]
    fn persist_load_roundtrip() {
        // Build an index, persist it to the tier zones of a tempfile
        // substrate, then load a fresh instance from the same zones
        // and verify the two agree on every search result.
        let src = SubstrateTieredIndex::new(DIM);
        let offsets: Vec<NodeOffset> = vec![5, 11, 23, 47, 89, 100, 250, 777];
        for &o in &offsets {
            src.insert(o, &emb(o, DIM));
        }

        let sub = crate::file::SubstrateFile::open_tempfile().unwrap();
        src.persist_to_zones(&sub).unwrap();

        // Fresh instance from zones.
        let loaded = SubstrateTieredIndex::load_from_zones(&sub, DIM)
            .unwrap()
            .expect("tier zones must load successfully after persist");

        assert_eq!(loaded.len(), src.len());

        // Self-recall behaviour must match bit-for-bit (both indexes
        // use the same default SRP seeds, so projections are identical
        // and cascade ordering is deterministic).
        for &o in &offsets {
            let q = emb(o, DIM);
            let src_hits = src.search_top_k(&q, 5);
            let loaded_hits = loaded.search_top_k(&q, 5);
            assert_eq!(
                src_hits, loaded_hits,
                "search disagree for offset {o}: src={src_hits:?} loaded={loaded_hits:?}"
            );
            // And the top-1 should still be the self-recall offset.
            assert_eq!(loaded_hits[0].0, o);
        }
    }

    #[test]
    fn load_from_empty_zones_returns_none() {
        // Fresh substrate: tier zones are 0 B. Load must return None
        // so the caller falls back to rebuild-from-properties.
        let sub = crate::file::SubstrateFile::open_tempfile().unwrap();
        let got = SubstrateTieredIndex::load_from_zones(&sub, DIM).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn persist_empty_index_is_valid() {
        // An index with zero entries persists + loads cleanly — the
        // header tracks n_slots=0 and the payload is zero bytes.
        let src = SubstrateTieredIndex::new(DIM);
        let sub = crate::file::SubstrateFile::open_tempfile().unwrap();
        src.persist_to_zones(&sub).unwrap();

        let loaded = SubstrateTieredIndex::load_from_zones(&sub, DIM)
            .unwrap()
            .expect("empty-index persist must be loadable");
        assert_eq!(loaded.len(), 0);
        assert!(loaded.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn determinism_of_ties() {
        // Craft tied embeddings: identical at every tier. They should
        // come back in ascending NodeOffset order.
        let idx = SubstrateTieredIndex::new(DIM);
        let v = emb(123, DIM);
        for n in [50, 10, 30, 20, 40] {
            idx.insert(n, &v);
        }
        let hits = idx.search_top_k(&v, 5);
        let offsets: Vec<NodeOffset> = hits.iter().map(|h| h.0).collect();
        assert_eq!(offsets, vec![10, 20, 30, 40, 50]);
    }
}

#[cfg(test)]
mod tests_slot_remap {
    use super::*;
    use obrain_core::index::vector::VectorIndex;

    const DIM: usize = L2_DIM;

    fn emb(seed: u32) -> Vec<f32> {
        (0..DIM)
            .map(|i| ((seed as f32 + 1.0) * 0.017 * (i as f32 + 1.0)).sin())
            .collect()
    }

    /// Chantier SlotRemappable — un index CHARGÉ avant une repermutation
    /// ne doit jamais écraser la table remappée sur disque avec ses slots
    /// périmés. Sans cette garde, la corruption serait silencieuse ET
    /// estampillée de l'epoch courant, donc indétectable ensuite.
    #[test]
    fn un_index_charge_avant_un_remap_refuse_d_ecraser_la_table() {
        let sub = SubstrateFile::open_tempfile().unwrap();

        // Index initial persisté à l'epoch 0.
        let index = SubstrateTieredIndex::new(DIM);
        for offset in 1..5u32 {
            index.insert(offset, &emb(offset));
        }
        index.persist_to_zones(&sub).unwrap();

        // Rechargement : l'index mémorise l'epoch 0.
        let charge = SubstrateTieredIndex::load_from_zones(&sub, DIM)
            .unwrap()
            .expect("tier0 doit se recharger");

        // Une repermutation survient : la table sur disque est remappée
        // (1 -> 3, 3 -> 1) et la base passe à l'epoch 1.
        let perm: Vec<u32> = vec![0, 3, 2, 1, 4];
        crate::slot_remap::remap_tier0_slot_table(&sub, &perm, 1).unwrap();
        crate::slot_remap::commit_remap_epoch(&sub, 1).unwrap();

        let apres_remap = {
            let zf = sub.open_zone(Zone::Tier0).unwrap();
            let (_, s2o, _) = tier_persist::read_tier_zone::<Tier0>(&zf, TierMagic::Tier0)
                .unwrap()
                .expect("table lisible apres remap");
            s2o.unwrap()
        };

        // L'index périmé tente de persister : REFUS OBSERVABLE (Err), et
        // la table remappée reste intacte.
        let refus = charge.persist_to_zones(&sub);
        assert!(
            refus.is_err(),
            "le refus doit etre OBSERVABLE : une persistance qui meurt en \
             silence laisse croire a l'appelant qu'il a sauvegarde"
        );
        let apres_tentative = {
            let zf = sub.open_zone(Zone::Tier0).unwrap();
            let (_, s2o, _) = tier_persist::read_tier_zone::<Tier0>(&zf, TierMagic::Tier0)
                .unwrap()
                .expect("table toujours lisible");
            s2o.unwrap()
        };
        assert_eq!(
            apres_tentative, apres_remap,
            "un index perime a ecrase la table remappee — corruption silencieuse"
        );

        // Un index rechargé APRÈS le remap, lui, persiste normalement.
        let frais = SubstrateTieredIndex::load_from_zones(&sub, DIM)
            .unwrap()
            .expect("rechargement post-remap");
        frais.persist_to_zones(&sub).unwrap();
    }
}
