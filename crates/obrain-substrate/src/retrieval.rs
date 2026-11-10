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
pub use crate::tier_persist::EmpreinteStore;
use crate::tier_persist::{self, TierMagic};
use crate::tiered_scan::{PARALLEL_THRESHOLD, ScanConfig, TieredQuery, scan_tiered};
use crate::tiers::{L2_DIM, Tier0, Tier0Builder, Tier1, Tier1Builder, Tier2, Tier2Builder};
use crate::vec_projection::Projection;
use parking_lot::RwLock;
use rayon::prelude::*;
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
// Largeurs du cascade et source des vecteurs natifs (rerank exact)
// ---------------------------------------------------------------------------

/// Largeur par défaut du préfiltre L0 : candidats conservés après le balayage
/// des empreintes 128 bits.
///
/// MESURÉ (spike cascade-768, 10/11/2026, projection PCA 768→384 + rerank
/// exact) : recall@10 = 0,986 sur les 32 521 vecteurs réels de PO, 0,997 à
/// 1 M de vecteurs structurés ; à 1 000, respectivement 0,97 et 0,972.
pub const K0_DEFAUT: usize = 4_000;

/// Largeur par défaut après le re-classement L1 : ce sont ces candidats que
/// le niveau L2 (f16) classe, puis que le rerank exact départage quand une
/// [`SourceNative`] est branchée. Coût du rerank : `k1` produits scalaires
/// en dimension native (≈ 0,3 ms pour 400 vecteurs 768-d lus en mmap).
pub const K1_DEFAUT: usize = 400;

/// Largeurs du cascade pour une recherche : `k0` candidats gardés après L0,
/// `k1` après L1. Le scan borne toujours `k ≤ k1 ≤ k0 ≤ n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeursCascade {
    /// Candidats conservés après le préfiltre L0.
    pub k0: usize,
    /// Candidats conservés après L1 — et soumis au rerank exact.
    pub k1: usize,
}

impl Default for LargeursCascade {
    fn default() -> Self {
        Self {
            k0: K0_DEFAUT,
            k1: K1_DEFAUT,
        }
    }
}

impl LargeursCascade {
    /// Largeurs dérivées d'une seule largeur de candidats — le `ef` des API
    /// de recherche : `k0 = ef`, `k1 = max(1, ef / 10)` (le rapport des
    /// défauts 4 000 / 400).
    pub fn depuis_k0(k0: usize) -> Self {
        let k0 = k0.max(1);
        Self {
            k0,
            k1: (k0 / 10).max(1),
        }
    }
}

/// Accès aux vecteurs NATIFS (dimension d'entrée, f32) des nœuds indexés,
/// pour le rerank exact des candidats du cascade.
///
/// L'index ne copie jamais ces vecteurs : la colonne (ou toute autre source
/// de vérité) reste seule détentrice. `lire_natif` remplit `dest` (vidé
/// d'abord) et rend `false` si le nœud n'a pas de vecteur — le candidat
/// garde alors son score L2, il n'est pas écarté en silence.
///
/// Toute fermeture `Fn(NodeOffset, &mut Vec<f32>) -> bool + Send + Sync`
/// implémente ce trait.
pub trait SourceNative: Send + Sync {
    /// Copie le vecteur natif du nœud `offset` dans `dest`. `false` si absent.
    fn lire_natif(&self, offset: NodeOffset, dest: &mut Vec<f32>) -> bool;
}

impl<F> SourceNative for F
where
    F: Fn(NodeOffset, &mut Vec<f32>) -> bool + Send + Sync,
{
    fn lire_natif(&self, offset: NodeOffset, dest: &mut Vec<f32>) -> bool {
        self(offset, dest)
    }
}

// ---------------------------------------------------------------------------
// SubstrateTieredIndex — in-memory implementation on top of the L0/L1/L2
// cascade. Mmap'd-backed variant comes in T11 (hilbert page ordering).
// ---------------------------------------------------------------------------

/// In-memory tiered vector index.
///
/// ### Dimension native et projection
///
/// Les paliers L0/L1/L2 travaillent en [`L2_DIM`] (384). L'index accepte
/// néanmoins des vecteurs de n'importe quelle dimension **native** grâce à
/// une [`Projection`] qu'il possède : identité + rembourrage de zéros pour
/// `dim ≤ 384` ([`Self::new`]), PCA apprise pour `dim > 384`
/// ([`Self::avec_projection`]). `insert`, `rebuild` et `search_top_k`
/// prennent et rendent la dimension native ; la projection est un détail
/// interne.
///
/// ### Rerank exact
///
/// Avec une [`SourceNative`] branchée ([`Self::definir_source_native`]), la
/// recherche demande `k1` candidats au cascade puis les reclasse par cosinus
/// exact sur les vecteurs natifs ; le score rendu est alors ce cosinus
/// exact. Sans source, le score est le cosinus L2 (f16, projeté) comme
/// avant. Les largeurs se règlent par [`Self::definir_largeurs`] ou par
/// requête via [`Self::chercher`] / [`VectorIndex::search_top_k_ef`].
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
    /// Dimension NATIVE acceptée par `insert` / `rebuild` / `search_top_k`
    /// (= `projection.dim_entree()`). Les paliers, eux, sont en `L2_DIM`.
    dim: usize,
    /// Projection dimension native → `L2_DIM`, appliquée à l'insertion et à
    /// la requête.
    projection: Projection,
    /// Largeurs par défaut des recherches passant par `search_top_k`.
    largeurs: RwLock<LargeursCascade>,
    /// Source des vecteurs natifs pour le rerank exact ; `None` = score L2.
    source: RwLock<Option<Arc<dyn SourceNative>>>,
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
    /// `dim ≤ L2_DIM` : les vecteurs sont plongés dans les 384 dimensions
    /// des paliers par rembourrage de zéros (cosinus préservé au bit près).
    /// Pour une dimension native supérieure (768, 1 024…), passer par
    /// [`Self::avec_projection`] avec une projection apprise.
    ///
    /// # Panics
    ///
    /// Panics if `dim == 0` or `dim > L2_DIM` (currently 384). The L2 tier
    /// uses a fixed-length f16 representation tied to the canonical
    /// embedding width; wider inputs need an explicit, learned projection
    /// — doing it silently here would change result quality without anyone
    /// having asked for it.
    pub fn new(dim: usize) -> Self {
        let projection = Projection::identite(dim).unwrap_or_else(|e| {
            panic!(
                "SubstrateTieredIndex::new({dim}) : dimension hors de 1..={L2_DIM} ({e}) ; \
                 au-dela de {L2_DIM}, utiliser avec_projection(Projection::Apprise(..))"
            )
        });
        Self::avec_projection(projection)
    }

    /// Index vide dont les vecteurs natifs sont de dimension
    /// `projection.dim_entree()` et sont projetés en `L2_DIM` par
    /// `projection` avant d'alimenter les paliers.
    pub fn avec_projection(projection: Projection) -> Self {
        Self {
            inner: RwLock::new(Inner {
                offset_to_slot: HashMap::new(),
                slot_to_offset: Vec::new(),
                l0: Vec::new(),
                l1: Vec::new(),
                l2: Vec::new(),
            }),
            b0: Tier0Builder::with_default_seed(L2_DIM),
            b1: Tier1Builder::with_default_seed(L2_DIM),
            b2: Tier2Builder::new(),
            dim: projection.dim_entree(),
            projection,
            largeurs: RwLock::new(LargeursCascade::default()),
            source: RwLock::new(None),
            slots_epoch_at_load: None,
        }
    }

    /// La projection dimension native → `L2_DIM` que porte cet index.
    pub fn projection(&self) -> &Projection {
        &self.projection
    }

    /// Largeurs par défaut des recherches (`search_top_k`).
    pub fn largeurs(&self) -> LargeursCascade {
        *self.largeurs.read()
    }

    /// Règle les largeurs par défaut des recherches.
    pub fn definir_largeurs(&self, largeurs: LargeursCascade) {
        *self.largeurs.write() = largeurs;
    }

    /// Branche (ou débranche, avec `None`) la source des vecteurs natifs
    /// qui active le rerank exact.
    pub fn definir_source_native(&self, source: Option<Arc<dyn SourceNative>>) {
        *self.source.write() = source;
    }

    /// `true` si une source native est branchée (les scores rendus sont
    /// alors des cosinus exacts en dimension native).
    pub fn a_source_native(&self) -> bool {
        self.source.read().is_some()
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
        let projection = Projection::identite(dim).unwrap_or_else(|e| {
            panic!(
                "SubstrateTieredIndex::from_parts : dimension {dim} hors de 1..={L2_DIM} ({e}) ; \
                 au-dela de {L2_DIM}, utiliser from_parts_avec_projection"
            )
        });
        Self::from_parts_avec_projection(projection, slot_to_offset, offset_to_slot, l0, l1, l2)
    }

    /// Comme [`Self::from_parts`], avec une projection explicite : les
    /// paliers fournis DOIVENT avoir été calculés avec cette même
    /// projection (une projection différente rend des résultats faux, pas
    /// une panique).
    pub fn from_parts_avec_projection(
        projection: Projection,
        slot_to_offset: Vec<NodeOffset>,
        offset_to_slot: HashMap<NodeOffset, usize>,
        l0: Vec<Tier0>,
        l1: Vec<Tier1>,
        l2: Vec<Tier2>,
    ) -> Self {
        Self {
            inner: RwLock::new(Inner {
                offset_to_slot,
                slot_to_offset,
                l0,
                l1,
                l2,
            }),
            b0: Tier0Builder::with_default_seed(L2_DIM),
            b1: Tier1Builder::with_default_seed(L2_DIM),
            b2: Tier2Builder::new(),
            dim: projection.dim_entree(),
            projection,
            largeurs: RwLock::new(LargeursCascade::default()),
            source: RwLock::new(None),
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
        // `dim` est la dimension native d'un index à projection IDENTITÉ
        // (≤ L2_DIM) ; `from_parts` panique hors bornes. Le chargement d'un
        // index à projection apprise (zone `substrate.vecproj.*`) est l'objet
        // de la persistance branchée au moteur (C3).

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

    // -----------------------------------------------------------------------
    // Zones NOMMÉES par index (label, propriété) — O-VEC C3
    //
    // Les zones `substrate.tier0/1/2` non nommées sont celles de l'index
    // unique historique de la base (`persist_to_zones` / `load_from_zones`).
    // Un moteur qui tient PLUSIEURS index vectoriels (un par label et
    // propriété) les range ici, chacun sous ses quatre zones :
    // `substrate.tier{0,1,2}.<label>.<prop>` et `substrate.vecproj.<label>.<prop>`
    // (cette dernière seulement pour une projection apprise). L'en-tête de
    // chaque palier porte l'epoch de remap et une empreinte du store ; le
    // chargement exige leur concordance, sinon `Ok(None)` et l'appelant
    // reconstruit depuis la colonne.
    // -----------------------------------------------------------------------

    /// Les quatre zones d'un index nommé : paliers 0, 1, 2, puis projection.
    pub fn noms_de_zones(label: &str, propriete: &str) -> [String; 4] {
        [
            crate::file::zone::tier_nomme(0, label, propriete),
            crate::file::zone::tier_nomme(1, label, propriete),
            crate::file::zone::tier_nomme(2, label, propriete),
            Projection::nom_fichier(label, propriete),
        ]
    }

    /// Atteste que le contenu de cet index a été bâti APRÈS la dernière
    /// repermutation de slots de `sub` (pendant de [`Self::new_lie_a_la_base`]
    /// pour un index construit par `avec_projection` + `rebuild`). Sans cette
    /// liaison, la persistance refuse d'écrire sur une base déjà repermutée.
    pub fn lier_a_l_epoch_courant(mut self, sub: &SubstrateFile) -> SubstrateResult<Self> {
        self.slots_epoch_at_load = Some(crate::slot_remap::current_remap_epoch(sub)?);
        Ok(self)
    }

    /// Garde commune aux persistances (voir le long commentaire de
    /// [`Self::persist_to_zones`]) ; rend l'epoch de remap courant.
    fn epoch_pour_persister(&self, sub: &SubstrateFile) -> SubstrateResult<u32> {
        let remap_epoch = crate::slot_remap::current_remap_epoch(sub)?;
        match self.slots_epoch_at_load {
            Some(loaded_at) if loaded_at != remap_epoch => {
                Err(crate::error::SubstrateError::Internal(format!(
                    "tier index: refus de persister — index charge a l'epoch de remap \
                     {loaded_at}, base desormais a l'epoch {remap_epoch}. Sa table \
                     slot_to_offset est PERIMEE et ecraserait la table deja remappee. \
                     Rechargez l'index (load_from_zones) avant de persister."
                )))
            }
            None if remap_epoch != 0 => Err(crate::error::SubstrateError::Internal(format!(
                "tier index: refus de persister — index non lie a un epoch de remap \
                 sur une base qui en est a l'epoch {remap_epoch}. Construisez-le via \
                 SubstrateTieredIndex::new_lie_a_la_base(&sub, dim) ou liez-le par \
                 lier_a_l_epoch_courant pour attester qu'il a ete bati APRES la \
                 derniere repermutation."
            ))),
            _ => Ok(remap_epoch),
        }
    }

    /// Persiste l'index dans ses zones nommées, estampillées de `empreinte`
    /// (état du store au moment de l'écriture, fourni par l'appelant qui
    /// sait quand ses écritures sont stables). Mêmes gardes d'epoch que
    /// [`Self::persist_to_zones`]. La projection n'est écrite que si elle
    /// est apprise (l'identité se déduit de la dimension).
    pub fn persister_nomme(
        &self,
        sub: &SubstrateFile,
        label: &str,
        propriete: &str,
        empreinte: EmpreinteStore,
    ) -> SubstrateResult<()> {
        let remap_epoch = self.epoch_pour_persister(sub)?;
        let [n0, n1, n2, _] = Self::noms_de_zones(label, propriete);
        {
            let guard = self.inner.read();
            let n_slots = guard.slot_to_offset.len() as u32;
            let mut zf0 = sub.open_named_zone(&n0)?;
            tier_persist::write_tier_zone_estampille::<Tier0>(
                &mut zf0,
                TierMagic::Tier0,
                n_slots,
                Some(&guard.slot_to_offset),
                &guard.l0,
                remap_epoch,
                Some(empreinte),
            )?;
            let mut zf1 = sub.open_named_zone(&n1)?;
            tier_persist::write_tier_zone_estampille::<Tier1>(
                &mut zf1,
                TierMagic::Tier1,
                n_slots,
                None,
                &guard.l1,
                remap_epoch,
                Some(empreinte),
            )?;
            let mut zf2 = sub.open_named_zone(&n2)?;
            tier_persist::write_tier_zone_estampille::<Tier2>(
                &mut zf2,
                TierMagic::Tier2,
                n_slots,
                None,
                &guard.l2,
                remap_epoch,
                Some(empreinte),
            )?;
        }
        if matches!(self.projection, Projection::Apprise(_)) {
            self.projection.ecrire_zone(sub, label, propriete)?;
        }
        Ok(())
    }

    /// Charge un index nommé si, et seulement si, ses zones décrivent encore
    /// la base : en-têtes v2 valides (CRC), même `empreinte` sur les trois
    /// paliers et égale à `empreinte_attendue`, epoch de remap égal à
    /// l'epoch courant, longueurs concordantes, et — pour `dim_native >
    /// L2_DIM` — projection présente et de cette dimension. Sinon `Ok(None)` :
    /// l'appelant reconstruit depuis la colonne. Une zone absente ou
    /// illisible compte comme absente (`Ok(None)`), jamais comme une erreur.
    pub fn charger_nomme(
        sub: &SubstrateFile,
        label: &str,
        propriete: &str,
        dim_native: usize,
        empreinte_attendue: EmpreinteStore,
    ) -> SubstrateResult<Option<Self>> {
        let [n0, n1, n2, _] = Self::noms_de_zones(label, propriete);
        let Ok(zf0) = sub.open_named_zone_read_only(&n0) else {
            return Ok(None);
        };
        let Some(((n_0, s2o, l0), h0)) =
            tier_persist::read_tier_zone_avec_entete::<Tier0>(&zf0, TierMagic::Tier0)?
        else {
            return Ok(None);
        };
        let Some(slot_to_offset) = s2o else {
            return Ok(None);
        };
        if h0.empreinte() != Some(empreinte_attendue) {
            return Ok(None);
        }
        let epoch = crate::slot_remap::current_remap_epoch(sub)?;
        if h0.remap_epoch != epoch {
            return Ok(None);
        }
        let Ok(zf1) = sub.open_named_zone_read_only(&n1) else {
            return Ok(None);
        };
        let Some(((n_1, _, l1), h1)) =
            tier_persist::read_tier_zone_avec_entete::<Tier1>(&zf1, TierMagic::Tier1)?
        else {
            return Ok(None);
        };
        let Ok(zf2) = sub.open_named_zone_read_only(&n2) else {
            return Ok(None);
        };
        let Some(((n_2, _, l2), h2)) =
            tier_persist::read_tier_zone_avec_entete::<Tier2>(&zf2, TierMagic::Tier2)?
        else {
            return Ok(None);
        };
        if n_0 != n_1
            || n_0 != n_2
            || h1.empreinte() != Some(empreinte_attendue)
            || h2.empreinte() != Some(empreinte_attendue)
            || h1.remap_epoch != epoch
            || h2.remap_epoch != epoch
        {
            return Ok(None);
        }
        let attendu = n_0 as usize;
        if slot_to_offset.len() != attendu
            || l0.len() != attendu
            || l1.len() != attendu
            || l2.len() != attendu
        {
            return Ok(None);
        }
        let projection = if dim_native <= L2_DIM {
            match Projection::identite(dim_native) {
                Ok(p) => p,
                Err(_) => return Ok(None),
            }
        } else {
            match Projection::lire_zone(sub, label, propriete) {
                Ok(Some(p)) if p.dim_entree() == dim_native => p,
                Ok(_) => return Ok(None),
                Err(e) => {
                    tracing::info!(
                        erreur = %e, label, propriete,
                        "projection illisible — index vectoriel a reconstruire"
                    );
                    return Ok(None);
                }
            }
        };
        let mut offset_to_slot = HashMap::with_capacity(attendu);
        for (slot, &offset) in slot_to_offset.iter().enumerate() {
            offset_to_slot.insert(offset, slot);
        }
        Ok(Some(
            Self::from_parts_avec_projection(
                projection,
                slot_to_offset,
                offset_to_slot,
                l0,
                l1,
                l2,
            )
            .with_slots_epoch(epoch),
        ))
    }

    /// Retire les zones nommées d'un index (suppression de l'index, ou index
    /// jugé périmé à la fermeture). En mode fichier les fichiers sont
    /// supprimés ; en mode mémoire leur en-tête est effacé (une zone RAM ne
    /// se retire pas), ce qui suffit à ce que [`Self::charger_nomme`] rende
    /// `None`. Une zone déjà absente n'est pas une erreur.
    pub fn supprimer_nomme(
        sub: &SubstrateFile,
        label: &str,
        propriete: &str,
    ) -> SubstrateResult<()> {
        for nom in Self::noms_de_zones(label, propriete) {
            if sub.is_memory() {
                if let Ok(mut zf) = sub.open_named_zone_read_only(&nom) {
                    let n = zf.len().min(tier_persist::HEADER_SIZE as u64) as usize;
                    zf.as_slice_mut()[..n].fill(0);
                }
            } else {
                match std::fs::remove_file(sub.path().join(&nom)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }

    /// Projette un vecteur natif en `L2_DIM`. Panique si la longueur n'est
    /// pas la dimension native de l'index (contrat du trait).
    fn projeter(&self, v: &[f32]) -> Vec<f32> {
        assert_eq!(
            v.len(),
            self.dim,
            "embedding.len() = {} but index dim = {}",
            v.len(),
            self.dim
        );
        self.projection
            .projeter(v)
            .expect("longueur verifiee juste au-dessus : la projection ne peut pas echouer")
    }

    fn project_query(&self, query: &[f32]) -> TieredQuery {
        let p = self.projeter(query);
        TieredQuery {
            l0: self.b0.project(&p),
            l1: self.b1.project(&p),
            l2: self.b2.project(&p),
        }
    }

    /// Recherche avec des largeurs explicites pour cet appel.
    ///
    /// Sans source native : le cascade rend directement `k` résultats au
    /// score L2 (f16, projeté). Avec source native : le cascade rend
    /// `max(k, k1)` candidats, reclassés par cosinus exact sur les vecteurs
    /// natifs, puis tronqués à `k`. Le rerank s'exécute HORS du verrou de
    /// lecture de l'index. Ties : score décroissant puis offset croissant.
    pub fn chercher(
        &self,
        query: &[f32],
        k: usize,
        largeurs: LargeursCascade,
    ) -> Vec<(NodeOffset, f32)> {
        if k == 0 {
            return Vec::new();
        }
        let q = self.project_query(query);
        let source = self.source.read().clone();
        let candidats: Vec<(NodeOffset, f32)> = {
            let guard = self.inner.read();
            let n = guard.slot_to_offset.len();
            if n == 0 {
                return Vec::new();
            }
            // Clamp k to the corpus size — the cascade panics otherwise.
            let k_eff = k.min(n);
            let demande = if source.is_some() {
                k_eff.max(largeurs.k1).min(n)
            } else {
                k_eff
            };
            let config = ScanConfig {
                k0: largeurs.k0,
                k1: largeurs.k1,
                parallel_threshold: PARALLEL_THRESHOLD,
            };
            let hits = scan_tiered(&q, &guard.l0, &guard.l1, &guard.l2, demande, config);
            hits.into_iter()
                .map(|h| (guard.slot_to_offset[h.node_offset as usize], h.cosine))
                .collect()
        };
        let mut out = match source {
            Some(src) => self.rerank(src.as_ref(), query, candidats),
            None => candidats,
        };
        // Translate slot indices back into NodeOffsets. The cascade's
        // tie-break is `slot` ascending, which aligns with NodeOffset
        // ascending *iff* insertions happened in offset order. To keep
        // the trait's contract ("ties broken by ascending NodeOffset")
        // we do a final stable sort — the working set is ≤ max(k, k1),
        // so the cost is negligible.
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out.truncate(k);
        out
    }

    /// Reclasse `candidats` par cosinus exact entre `query` (natif) et le
    /// vecteur natif de chaque candidat. Un candidat sans vecteur natif, ou
    /// de longueur inattendue, garde son score du cascade.
    fn rerank(
        &self,
        source: &dyn SourceNative,
        query: &[f32],
        candidats: Vec<(NodeOffset, f32)>,
    ) -> Vec<(NodeOffset, f32)> {
        let norme_q = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norme_q <= 0.0 || !norme_q.is_finite() {
            return candidats;
        }
        let mut tampon: Vec<f32> = Vec::with_capacity(self.dim);
        let mut absents = 0usize;
        let out: Vec<(NodeOffset, f32)> = candidats
            .into_iter()
            .map(|(offset, score_cascade)| {
                tampon.clear();
                if !source.lire_natif(offset, &mut tampon) || tampon.len() != query.len() {
                    absents += 1;
                    return (offset, score_cascade);
                }
                let mut ps = 0f32;
                let mut nv = 0f32;
                for (a, b) in query.iter().zip(&tampon) {
                    ps += a * b;
                    nv += b * b;
                }
                if nv > 0.0 && nv.is_finite() {
                    (offset, ps / (norme_q * nv.sqrt()))
                } else {
                    (offset, score_cascade)
                }
            })
            .collect();
        if absents > 0 {
            tracing::debug!(
                absents,
                "rerank exact : candidats sans vecteur natif, score du cascade conserve"
            );
        }
        out
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
        let p = self.projeter(embedding);
        let tier0 = self.b0.project(&p);
        let tier1 = self.b1.project(&p);
        let tier2 = self.b2.project(&p);

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
        // Projection + trois paliers EN PARALLÈLE, par paire : `collect`
        // préserve l'ordre et chaque paire est traitée par un seul thread,
        // donc le résultat est identique au bit près quel que soit le nombre
        // de threads (jamais de fold/reduce flottant ici).
        let projetes: Vec<(Tier0, Tier1, Tier2)> = pairs
            .par_iter()
            .enumerate()
            .map(|(i, (_, emb))| {
                assert_eq!(
                    emb.len(),
                    self.dim,
                    "pair {i}: embedding.len() = {} but index dim = {}",
                    emb.len(),
                    self.dim
                );
                let p = self.projeter(emb);
                (
                    self.b0.project(&p),
                    self.b1.project(&p),
                    self.b2.project(&p),
                )
            })
            .collect();
        let mut offsets: Vec<NodeOffset> = Vec::with_capacity(n);
        let mut l0: Vec<Tier0> = Vec::with_capacity(n);
        let mut l1: Vec<Tier1> = Vec::with_capacity(n);
        let mut l2: Vec<Tier2> = Vec::with_capacity(n);
        let mut map: HashMap<NodeOffset, usize> = HashMap::with_capacity(n);
        for (i, ((off, _), (t0, t1, t2))) in pairs.iter().zip(projetes).enumerate() {
            offsets.push(*off);
            l0.push(t0);
            l1.push(t1);
            l2.push(t2);
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
        self.chercher(query, k, *self.largeurs.read())
    }

    fn search_top_k_ef(&self, query: &[f32], k: usize, ef: usize) -> Vec<(NodeOffset, f32)> {
        self.chercher(query, k, LargeursCascade::depuis_k0(ef))
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
        assert!(
            idx.search_top_k(&emb(1, DIM), 0).is_empty(),
            "résultat attendu vide"
        );
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

#[cfg(test)]
mod tests_rerank {
    //! C2 (plan O-VEC) — projection possédée par l'index, rerank exact sur
    //! les vecteurs natifs, largeurs k0/k1. Vecteurs 768-d structurés
    //! (recette du dépôt), vérité terrain par force brute.
    use super::*;
    use crate::vec_projection::{ProjectionApprise, jeux};

    const D: usize = 768;
    const N: usize = 2_000;
    const NQ: usize = 100;
    const K: usize = 10;
    const CENTRES: usize = 50;

    struct Jeu {
        corpus: Vec<f32>,
        requetes: Vec<f32>,
    }

    /// `N` vecteurs indexés et `NQ` requêtes tirées des MÊMES centres (les
    /// requêtes ne sont pas dans le corpus : pas d'auto-rappel trivial).
    fn jeu() -> Jeu {
        let tout = jeux::structures(N + NQ, CENTRES, D, 2026);
        let (corpus, requetes) = tout.split_at(N * D);
        Jeu {
            corpus: corpus.to_vec(),
            requetes: requetes.to_vec(),
        }
    }

    fn cosinus(a: &[f32], b: &[f32]) -> f32 {
        let (mut ps, mut na, mut nb) = (0f32, 0f32, 0f32);
        for (x, y) in a.iter().zip(b) {
            ps += x * y;
            na += x * x;
            nb += y * y;
        }
        ps / (na.sqrt() * nb.sqrt())
    }

    fn vecteur(corpus: &[f32], off: NodeOffset) -> &[f32] {
        let i = off as usize;
        &corpus[i * D..(i + 1) * D]
    }

    fn force_brute(corpus: &[f32], q: &[f32], k: usize) -> Vec<(NodeOffset, f32)> {
        let mut tous: Vec<(NodeOffset, f32)> = corpus
            .as_chunks::<D>()
            .0
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, cosinus(q, v)))
            .collect();
        tous.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then_with(|| a.0.cmp(&b.0)));
        tous.truncate(k);
        tous
    }

    fn recall(verite: &[(NodeOffset, f32)], rendu: &[(NodeOffset, f32)]) -> f32 {
        let trouves = verite
            .iter()
            .filter(|(o, _)| rendu.iter().any(|(r, _)| r == o))
            .count();
        trouves as f32 / verite.len() as f32
    }

    fn index(jeu: &Jeu, avec_source: bool) -> SubstrateTieredIndex {
        let pca = ProjectionApprise::ajuster(&jeu.corpus, D, 7).unwrap();
        let idx = SubstrateTieredIndex::avec_projection(Projection::Apprise(pca));
        assert_eq!(idx.dim(), D);
        let pairs: Vec<(NodeOffset, Vec<f32>)> = jeu
            .corpus
            .as_chunks::<D>()
            .0
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.to_vec()))
            .collect();
        idx.rebuild(&pairs);
        assert_eq!(idx.len(), N);
        if avec_source {
            let corpus = Arc::new(jeu.corpus.clone());
            idx.definir_source_native(Some(Arc::new(
                move |off: NodeOffset, dest: &mut Vec<f32>| {
                    if off as usize >= N {
                        return false;
                    }
                    dest.extend_from_slice(vecteur(&corpus, off));
                    true
                },
            )));
        }
        idx
    }

    fn recall_moyen(idx: &SubstrateTieredIndex, jeu: &Jeu, largeurs: LargeursCascade) -> f32 {
        let mut total = 0f32;
        for q in jeu.requetes.as_chunks::<D>().0.iter() {
            let verite = force_brute(&jeu.corpus, q, K);
            let rendu = idx.chercher(q, K, largeurs);
            assert_eq!(rendu.len(), K);
            total += recall(&verite, &rendu);
        }
        total / NQ as f32
    }

    #[test]
    fn le_rerank_exact_atteint_la_porte_de_recall_en_768() {
        let jeu = jeu();
        let avec = index(&jeu, true);
        let sans = index(&jeu, false);
        let largeurs = LargeursCascade::default();
        let r_avec = recall_moyen(&avec, &jeu, largeurs);
        let r_sans = recall_moyen(&sans, &jeu, largeurs);
        println!("recall@10 : avec rerank = {r_avec:.4}, sans rerank (temoin) = {r_sans:.4}");
        assert!(r_avec >= 0.95, "recall@10 avec rerank = {r_avec:.4} < 0,95");
        assert!(
            r_sans < 0.90,
            "temoin sans rerank = {r_sans:.4} : attendu < 0,90 (MESURE 0,677 le 10/11/2026)"
        );
        assert!(
            r_sans < r_avec,
            "le temoin sans rerank ({r_sans:.4}) doit rester sous le rerank ({r_avec:.4}) : \
             la difference prouve l'etage"
        );
    }

    #[test]
    fn les_scores_sont_les_cosinus_exacts_en_dimension_native() {
        let jeu = jeu();
        let idx = index(&jeu, true);
        assert!(idx.a_source_native());
        for q in jeu.requetes.as_chunks::<D>().0.iter() {
            for (off, score) in idx.search_top_k(q, K) {
                let exact = cosinus(q, vecteur(&jeu.corpus, off));
                assert!(
                    (score - exact).abs() < 1e-5,
                    "offset {off} : score {score} vs cosinus exact {exact}"
                );
            }
        }
    }

    #[test]
    fn la_largeur_k0_agit_et_passe_par_le_trait() {
        let jeu = jeu();
        let idx = index(&jeu, true);
        let etroit = recall_moyen(&idx, &jeu, LargeursCascade::depuis_k0(100));
        let mille = recall_moyen(&idx, &jeu, LargeursCascade::depuis_k0(1_000));
        let large = recall_moyen(&idx, &jeu, LargeursCascade::depuis_k0(16_000));
        println!("recall@10 : k0 = 100 -> {etroit:.4}, 1 000 -> {mille:.4}, 16 000 -> {large:.4}");
        assert!(
            large >= mille && mille >= etroit,
            "elargir ne doit jamais faire perdre du recall"
        );
        assert!(large >= 0.95);
        // Le même chemin par le trait (`ef`) : résultats identiques.
        let q = &jeu.requetes[..D];
        let via_trait: Arc<dyn VectorIndex> = Arc::new(index(&jeu, true));
        assert_eq!(
            via_trait.search_top_k_ef(q, K, 16_000),
            idx.chercher(q, K, LargeursCascade::depuis_k0(16_000))
        );
        // Sans `ef`, les largeurs par défaut de l'index s'appliquent.
        idx.definir_largeurs(LargeursCascade::depuis_k0(16_000));
        assert_eq!(
            idx.search_top_k(q, K),
            idx.chercher(q, K, LargeursCascade::depuis_k0(16_000))
        );
    }

    #[test]
    fn un_candidat_sans_vecteur_natif_garde_son_score_du_cascade() {
        let jeu = jeu();
        let idx = index(&jeu, false);
        // Source qui ne connaît que les offsets pairs.
        let corpus = Arc::new(jeu.corpus.clone());
        idx.definir_source_native(Some(Arc::new(
            move |off: NodeOffset, dest: &mut Vec<f32>| {
                if off % 2 == 1 {
                    return false;
                }
                dest.extend_from_slice(vecteur(&corpus, off));
                true
            },
        )));
        let q = &jeu.requetes[..D];
        let rendu = idx.search_top_k(q, K);
        assert_eq!(rendu.len(), K, "aucun candidat n'est ecarte en silence");
        for (off, score) in &rendu {
            assert!(score.is_finite());
            if off % 2 == 0 {
                let exact = cosinus(q, vecteur(&jeu.corpus, *off));
                assert!(
                    (score - exact).abs() < 1e-5,
                    "offset pair {off} : rerank exact attendu"
                );
            }
        }
    }

    #[test]
    fn rebuild_parallele_et_insert_sequentiel_donnent_le_meme_index() {
        let jeu = jeu();
        let par_rebuild = index(&jeu, false);
        let pca = ProjectionApprise::ajuster(&jeu.corpus, D, 7).unwrap();
        let par_insert = SubstrateTieredIndex::avec_projection(Projection::Apprise(pca));
        for (i, v) in jeu.corpus.as_chunks::<D>().0.iter().enumerate() {
            par_insert.insert(i as u32, v);
        }
        for q in jeu.requetes.as_chunks::<D>().0.iter().take(20) {
            assert_eq!(
                par_rebuild
                    .chercher(q, K, LargeursCascade::default())
                    .iter()
                    .map(|h| h.0)
                    .collect::<Vec<_>>(),
                par_insert
                    .search_top_k(q, K)
                    .iter()
                    .map(|h| h.0)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn new_accepte_une_dimension_inferieure_par_identite() {
        let idx = SubstrateTieredIndex::new(5);
        assert_eq!(idx.dim(), 5);
        let a = [1.0, 0.0, 0.0, 0.0, 0.0];
        let b = [0.0, 1.0, 0.0, 0.0, 0.0];
        let c = [0.0, 0.0, 1.0, 0.0, 0.0];
        idx.insert(1, &a);
        idx.insert(2, &b);
        idx.insert(3, &c);
        let hits = idx.search_top_k(&b, 1);
        assert_eq!(hits[0].0, 2);
        assert!(hits[0].1 > 0.99);
    }

    #[test]
    #[should_panic(expected = "hors de 1..=384")]
    fn new_refuse_au_dela_de_l2_dim() {
        let _ = SubstrateTieredIndex::new(L2_DIM + 1);
    }

    /// Coût du rerank seul, imprimé (pas de seuil : la charge machine n'est pas
    /// maîtrisée en test). À lire en `--release` avec `--nocapture`.
    #[test]
    fn cout_du_rerank_imprime() {
        let jeu = jeu();
        let idx = index(&jeu, false);
        let mesurer = |largeurs: LargeursCascade| {
            let debut = std::time::Instant::now();
            for q in jeu.requetes.as_chunks::<D>().0.iter() {
                std::hint::black_box(idx.chercher(q, K, largeurs));
            }
            debut.elapsed().as_secs_f64() * 1e6 / NQ as f64
        };
        let sans_400 = mesurer(LargeursCascade { k0: 4_000, k1: 400 });
        let corpus = Arc::new(jeu.corpus.clone());
        idx.definir_source_native(Some(Arc::new(
            move |off: NodeOffset, dest: &mut Vec<f32>| {
                dest.extend_from_slice(vecteur(&corpus, off));
                true
            },
        )));
        let avec_400 = mesurer(LargeursCascade { k0: 4_000, k1: 400 });
        let avec_1600 = mesurer(LargeursCascade {
            k0: 4_000,
            k1: 1_600,
        });
        println!(
            "cout par requete (N = {N}, D = {D}) : sans rerank {sans_400:.0} us ; avec rerank k1 = 400 : {avec_400:.0} us (+{:.0}) ; k1 = 1 600 : {avec_1600:.0} us (+{:.0})",
            avec_400 - sans_400,
            avec_1600 - sans_400
        );
    }

    #[test]
    fn depuis_k0_derive_k1_au_dixieme() {
        assert_eq!(
            LargeursCascade::depuis_k0(4_000),
            LargeursCascade { k0: 4_000, k1: 400 }
        );
        assert_eq!(
            LargeursCascade::depuis_k0(5),
            LargeursCascade { k0: 5, k1: 1 }
        );
        assert_eq!(
            LargeursCascade::depuis_k0(0),
            LargeursCascade { k0: 1, k1: 1 }
        );
        assert_eq!(
            LargeursCascade::default(),
            LargeursCascade {
                k0: K0_DEFAUT,
                k1: K1_DEFAUT
            }
        );
    }
}

#[cfg(test)]
mod tests_zones_nommees {
    //! O-VEC C3 — persistance par index (label, propriété) : zones nommées,
    //! empreinte du store, epoch de remap, projection apprise.
    use super::*;
    use crate::vec_projection::{ProjectionApprise, jeux};

    const D: usize = 768;

    fn emb(seed: u32) -> Vec<f32> {
        (0..L2_DIM)
            .map(|i| ((seed as f32 + 1.0) * 0.011 * (i as f32 + 1.0)).sin())
            .collect()
    }

    fn empreinte(wal: u64, noeuds: u64) -> EmpreinteStore {
        EmpreinteStore {
            wal_offset: wal,
            noeuds_vivants: noeuds,
        }
    }

    fn index_identite(offsets: &[NodeOffset]) -> SubstrateTieredIndex {
        let idx = SubstrateTieredIndex::new(L2_DIM);
        for &o in offsets {
            idx.insert(o, &emb(o));
        }
        idx
    }

    fn index_apprise(n: usize) -> (SubstrateTieredIndex, Vec<f32>) {
        let corpus = jeux::structures(n, 20, D, 99);
        let pca = ProjectionApprise::ajuster(&corpus, D, 5).unwrap();
        let idx = SubstrateTieredIndex::avec_projection(Projection::Apprise(pca));
        let pairs: Vec<(NodeOffset, Vec<f32>)> = corpus
            .as_chunks::<D>()
            .0
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.to_vec()))
            .collect();
        idx.rebuild(&pairs);
        (idx, corpus)
    }

    fn memes_resultats(a: &SubstrateTieredIndex, b: &SubstrateTieredIndex, requetes: &[Vec<f32>]) {
        for q in requetes {
            assert_eq!(a.search_top_k(q, 5), b.search_top_k(q, 5));
        }
    }

    #[test]
    fn aller_retour_identite_avec_empreinte() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let offsets = [3u32, 8, 21, 55, 144];
        let src = index_identite(&offsets);
        src.persister_nomme(&sub, "Note", "embedding", empreinte(1_000, 5))
            .unwrap();

        let noms = sub.zone_names().unwrap();
        for n in &SubstrateTieredIndex::noms_de_zones("Note", "embedding")[..3] {
            assert!(
                noms.contains(n),
                "zone {n} absente de zone_names : {noms:?}"
            );
        }
        assert!(
            !noms.contains(&Projection::nom_fichier("Note", "embedding")),
            "pas de zone de projection pour l'identité"
        );

        let charge = SubstrateTieredIndex::charger_nomme(
            &sub,
            "Note",
            "embedding",
            L2_DIM,
            empreinte(1_000, 5),
        )
        .unwrap()
        .expect("empreinte identique ⇒ chargement");
        assert_eq!(charge.len(), 5);
        let requetes: Vec<Vec<f32>> = offsets.iter().map(|&o| emb(o)).collect();
        memes_resultats(&src, &charge, &requetes);

        // Empreinte différente (une écriture a eu lieu depuis) ⇒ None.
        assert!(
            SubstrateTieredIndex::charger_nomme(
                &sub,
                "Note",
                "embedding",
                L2_DIM,
                empreinte(1_001, 5)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            SubstrateTieredIndex::charger_nomme(
                &sub,
                "Note",
                "embedding",
                L2_DIM,
                empreinte(1_000, 6)
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn aller_retour_projection_apprise_768() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let (src, corpus) = index_apprise(300);
        let e = empreinte(42, 300);
        src.persister_nomme(&sub, "File", "emb", e).unwrap();
        let noms = sub.zone_names().unwrap();
        for n in SubstrateTieredIndex::noms_de_zones("File", "emb") {
            assert!(noms.contains(&n), "zone {n} absente : {noms:?}");
        }
        let charge = SubstrateTieredIndex::charger_nomme(&sub, "File", "emb", D, e)
            .unwrap()
            .expect("projection + paliers cohérents ⇒ chargement");
        assert_eq!(charge.dim(), D);
        assert_eq!(charge.len(), 300);
        assert_eq!(charge.projection().dim_entree(), D);
        let requetes: Vec<Vec<f32>> = corpus.as_chunks::<D>().0[..20]
            .iter()
            .map(|v| v.to_vec())
            .collect();
        memes_resultats(&src, &charge, &requetes);
    }

    #[test]
    fn projection_absente_ou_de_mauvaise_dimension_empeche_le_chargement() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let (src, _) = index_apprise(100);
        let e = empreinte(7, 100);
        src.persister_nomme(&sub, "File", "emb", e).unwrap();
        // Dimension native annoncée différente de celle de la projection.
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "File", "emb", 1_024, e)
                .unwrap()
                .is_none()
        );
        // Zone de projection effacée.
        std::fs::remove_file(sub.path().join(Projection::nom_fichier("File", "emb"))).unwrap();
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "File", "emb", D, e)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn un_epoch_de_remap_different_empeche_le_chargement() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let src = index_identite(&[1, 2, 3]);
        let e = empreinte(1, 3);
        src.persister_nomme(&sub, "Note", "embedding", e).unwrap();
        crate::slot_remap::commit_remap_epoch(&sub, 1).unwrap();
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
                .unwrap()
                .is_none(),
            "les zones nommées ne sont pas enrôlées dans le remap : epoch différent ⇒ reconstruction"
        );
        // Un index non lié ne peut plus persister sur une base repermutée…
        assert!(src.persister_nomme(&sub, "Note", "embedding", e).is_err());
        // … un index lié à l'epoch courant, si.
        let lie = index_identite(&[1, 2, 3])
            .lier_a_l_epoch_courant(&sub)
            .unwrap();
        lie.persister_nomme(&sub, "Note", "embedding", e).unwrap();
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn un_crc_corrompu_sur_un_palier_empeche_le_chargement() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let src = index_identite(&[10, 20, 30, 40]);
        let e = empreinte(5, 4);
        src.persister_nomme(&sub, "Note", "embedding", e).unwrap();
        {
            let [_, n1, _, _] = SubstrateTieredIndex::noms_de_zones("Note", "embedding");
            let mut zf = sub.open_named_zone(&n1).unwrap();
            zf.as_slice_mut()[tier_persist::HEADER_SIZE + 3] ^= 0xFF;
            zf.msync().unwrap();
        }
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn deux_index_coexistent_et_la_suppression_ne_touche_que_le_sien() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let a = index_identite(&[1, 2, 3]);
        let b = index_identite(&[7, 8, 9, 10, 11]);
        let e = empreinte(9, 8);
        a.persister_nomme(&sub, "Note", "embedding", e).unwrap();
        b.persister_nomme(&sub, "File", "embedding", e).unwrap();
        let ca = SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
            .unwrap()
            .unwrap();
        let cb = SubstrateTieredIndex::charger_nomme(&sub, "File", "embedding", L2_DIM, e)
            .unwrap()
            .unwrap();
        assert_eq!(ca.len(), 3);
        assert_eq!(cb.len(), 5);
        SubstrateTieredIndex::supprimer_nomme(&sub, "Note", "embedding").unwrap();
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
                .unwrap()
                .is_none()
        );
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "File", "embedding", L2_DIM, e)
                .unwrap()
                .is_some()
        );
        // Supprimer un index absent n'est pas une erreur.
        SubstrateTieredIndex::supprimer_nomme(&sub, "Note", "embedding").unwrap();
    }

    #[test]
    fn en_memoire_la_suppression_efface_l_en_tete() {
        let sub = SubstrateFile::open_memory().unwrap();
        let a = index_identite(&[1, 2]);
        let e = empreinte(1, 2);
        a.persister_nomme(&sub, "Note", "embedding", e).unwrap();
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
                .unwrap()
                .is_some()
        );
        SubstrateTieredIndex::supprimer_nomme(&sub, "Note", "embedding").unwrap();
        assert!(
            SubstrateTieredIndex::charger_nomme(&sub, "Note", "embedding", L2_DIM, e)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn les_zones_non_nommees_restent_intactes() {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let historique = index_identite(&[100, 200]);
        historique.persist_to_zones(&sub).unwrap();
        let nomme = index_identite(&[1, 2, 3]);
        nomme
            .persister_nomme(&sub, "Note", "embedding", empreinte(1, 3))
            .unwrap();
        let relu = SubstrateTieredIndex::load_from_zones(&sub, L2_DIM)
            .unwrap()
            .unwrap();
        assert_eq!(
            relu.len(),
            2,
            "l'index historique de la base n'est pas touché"
        );
    }
}
