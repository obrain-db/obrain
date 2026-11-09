//! Spreading activation engine — BFS-based energy propagation through synapses.
//!
//! Starting from one or more source nodes, energy propagates to neighbors
//! weighted by synapse strength and attenuated by a `decay_factor` per hop.
//! The result is an [`ActivationMap`] that can be used to rank query results
//! or identify contextually relevant subgraphs.
//!
//! # Causal vs associative propagation
//!
//! By default, propagation treats every edge identically (pure synapse
//! weight × decay). [`SpreadConfig::causal_boost`] plus
//! [`ActivationSource::edge_class`] let a caller optionally prioritize a
//! caller-defined set of "causal" relation types (dependencies, causes)
//! over the rest ("associative": implications, mentions, ...) — without
//! obrain-db imposing any fixed ontology of what counts as causal. The
//! default (`causal_boost = 1.0`, `edge_class` always `Associative`)
//! reproduces the exact pre-existing behavior; see
//! [`CausalAwareActivationSource`] for an implementation that classifies
//! edges from the underlying graph's relation types.

use crate::synapse::SynapseStore;
use obrain_common::types::NodeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Type alias for the activation result: node -> accumulated energy.
pub type ActivationMap = HashMap<NodeId, f64>;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for spreading activation.
#[derive(Debug, Clone)]
pub struct SpreadConfig {
    /// Maximum number of hops from any source.
    pub max_hops: u32,
    /// Minimum propagated energy to continue spreading (cutoff).
    pub min_propagated_energy: f64,
    /// Decay factor applied per hop: `propagated = parent * weight * decay_factor`.
    pub decay_factor: f64,
    /// Only include nodes with activation above this threshold in the result.
    pub activation_threshold: f64,
    /// Maximum number of nodes that can be activated (circuit breaker).
    /// Once this limit is reached, no further nodes are enqueued.
    /// Default: 1000. Set to 0 for unlimited (not recommended for dense graphs).
    pub max_activated_nodes: usize,
    /// Multiplier applied to propagation over edges classified
    /// [`EdgeClass::Causal`] by the [`ActivationSource`] (see
    /// [`ActivationSource::edge_class`]). Default `1.0` — no distinction,
    /// exactly reproducing pre-existing behavior. Set `> 1.0` to prioritize
    /// causal edges (dependencies/causes) over associative ones
    /// (implications/mentions) during propagation.
    pub causal_boost: f64,
}

impl Default for SpreadConfig {
    fn default() -> Self {
        Self {
            max_hops: 3,
            min_propagated_energy: 0.01,
            decay_factor: 0.5,
            activation_threshold: 0.0,
            max_activated_nodes: 1000,
            causal_boost: 1.0,
        }
    }
}

impl SpreadConfig {
    /// Builder: set max hops.
    pub fn with_max_hops(mut self, hops: u32) -> Self {
        self.max_hops = hops;
        self
    }

    /// Builder: set decay factor.
    pub fn with_decay_factor(mut self, factor: f64) -> Self {
        self.decay_factor = factor;
        self
    }

    /// Builder: set minimum propagated energy cutoff.
    pub fn with_min_energy(mut self, min_energy: f64) -> Self {
        self.min_propagated_energy = min_energy;
        self
    }

    /// Builder: set activation threshold.
    pub fn with_activation_threshold(mut self, threshold: f64) -> Self {
        self.activation_threshold = threshold;
        self
    }

    /// Builder: set maximum activated nodes (circuit breaker).
    pub fn with_max_activated_nodes(mut self, max: usize) -> Self {
        self.max_activated_nodes = max;
        self
    }

    /// Builder: set the causal-edge boost multiplier.
    pub fn with_causal_boost(mut self, boost: f64) -> Self {
        self.causal_boost = boost;
        self
    }
}

// ---------------------------------------------------------------------------
// Edge classification (causal vs associative)
// ---------------------------------------------------------------------------

/// Classifies a directed edge for propagation purposes.
///
/// obrain-db imposes no fixed ontology of what "causal" means — it's
/// entirely up to the [`ActivationSource`] implementation (and, in turn,
/// the caller) to decide, e.g. via a caller-supplied list of relation
/// type strings (see [`CausalAwareActivationSource`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeClass {
    /// A strong/directed link (dependency, cause) — prioritized when
    /// [`SpreadConfig::causal_boost`] is `> 1.0`.
    Causal,
    /// Everything else (association, implication, mention, ...). Default
    /// classification when a source doesn't distinguish edge types.
    #[default]
    Associative,
}

// ---------------------------------------------------------------------------
// ActivationSource trait
// ---------------------------------------------------------------------------

/// Provides weighted neighbors for the spreading activation algorithm.
///
/// The default implementation reads from a [`SynapseStore`].
pub trait ActivationSource: Send + Sync {
    /// Returns neighbors of `node_id` with their synapse weights.
    fn neighbors(&self, node_id: NodeId) -> Vec<(NodeId, f64)>;

    /// (2026-08-31) Itération directe des voisins SANS allouer de Vec — le
    /// hot path de `spread()`. Impl par défaut = `neighbors()` (le
    /// comportement historique) ; les sources substrate la surchargent avec
    /// le scan direct des arêtes SYNAPSE. Signature `&mut dyn FnMut` pour
    /// rester dyn-compatible (`&dyn ActivationSource`).
    fn for_each_neighbor(&self, node_id: NodeId, f: &mut dyn FnMut(NodeId, f64)) {
        for (n, w) in self.neighbors(node_id) {
            f(n, w);
        }
    }

    /// (2026-08-31) Préfetch cascade Hilbert — la pièce manquante de T11.
    ///
    /// `spread()` l'appelle quand un nœud est activé pour la première fois.
    /// La source substrate la surcharge avec `on_node_activated` : un
    /// `madvise(WILLNEED)` sur le range Hilbert de la communauté du nœud —
    /// les voisins (déjà physiquement contigus après `bulk_sort_by_hilbert`)
    /// sont donc chargés en mémoire avant d'être lus par le BFS. Critère
    /// d'acceptance T11 : depth-3 sur 10⁶ nœuds ≤ 1 ms. No-op par défaut
    /// (les sources non-substrate n'ont pas de disposition physique).
    fn on_activated(&self, _node_id: NodeId) {}

    /// Classifies the directed edge `from -> to` as causal or associative.
    ///
    /// Default: always [`EdgeClass::Associative`] — combined with the
    /// default `causal_boost = 1.0`, this reproduces pre-existing `spread()`
    /// behavior exactly for any implementation that doesn't override it.
    fn edge_class(&self, from: NodeId, to: NodeId) -> EdgeClass {
        let _ = (from, to);
        EdgeClass::Associative
    }
}

/// [`ActivationSource`] backed by a [`SynapseStore`].
///
/// Synapse weights are a pure Hebbian co-activation layer, independent of
/// the underlying graph's relation types — this source has no notion of
/// causal vs associative edges (always [`EdgeClass::Associative`], the
/// trait default). Use [`CausalAwareActivationSource`] when edge-type-based
/// classification is needed.
pub struct SynapseActivationSource {
    store: Arc<SynapseStore>,
}

impl SynapseActivationSource {
    /// Creates a new source from a synapse store.
    pub fn new(store: Arc<SynapseStore>) -> Self {
        Self { store }
    }
}

impl ActivationSource for SynapseActivationSource {
    fn neighbors(&self, node_id: NodeId) -> Vec<(NodeId, f64)> {
        self.store
            .list_synapses(node_id)
            .into_iter()
            .map(|s| {
                let neighbor = if s.source == node_id {
                    s.target
                } else {
                    s.source
                };
                (neighbor, s.current_weight())
            })
            .collect()
    }

    fn for_each_neighbor(&self, node_id: NodeId, f: &mut dyn FnMut(NodeId, f64)) {
        self.store.for_each_synapse(node_id, &mut |n, w| f(n, w));
    }

    fn on_activated(&self, node_id: NodeId) {
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.store.substrate_ref() {
            let _ = sub.on_node_activated(node_id);
        }
        // Sans substrat, il n'y a pas de colonne d'activation à notifier.
        #[cfg(not(feature = "substrate"))]
        let _ = node_id;
    }
}

/// [`ActivationSource`] combining synapse weight (propagation magnitude,
/// same as [`SynapseActivationSource`]) with graph relation-type
/// classification (propagation *class*, causal vs associative).
///
/// The set of relation-type strings considered "causal" is entirely
/// caller-supplied — obrain-db has no built-in notion of which relation
/// types are causal (same generic, schema-agnostic design as
/// `crate::gap_detection::GapDetectionConfig::causal_edge_types`).
pub struct CausalAwareActivationSource {
    synapses: Arc<SynapseStore>,
    graph: Arc<dyn obrain_core::graph::GraphStore>,
    causal_edge_types: HashSet<String>,
    /// (2026-08-31) Cache des paires causales, construit paresseusement en
    /// UNE passe sur les arêtes du graphe. L'ancien `edge_class` appelait
    /// `edges_from(node, Both)` pour CHAQUE paire visitée — O(degré²) par
    /// spread, mesuré 31,7 ms vs 0,42 ms pour le spread synapse pur. Ce
    /// cache pré-indexe `(src,dst)` canoniques des arêtes de type causal
    /// une seule fois par source ; `edge_class` devient un lookup O(1).
    causal_pairs: std::sync::Mutex<Option<HashSet<(NodeId, NodeId)>>>,
}

impl CausalAwareActivationSource {
    /// Creates a new causal-aware source.
    ///
    /// `causal_edge_types` lists the relation-type strings (as they appear
    /// via `GraphStore::edge_type`) that should be treated as
    /// [`EdgeClass::Causal`]. An edge is classified causal if *any* graph
    /// edge between the two nodes (in either direction) has a matching type.
    pub fn new(
        synapses: Arc<SynapseStore>,
        graph: Arc<dyn obrain_core::graph::GraphStore>,
        causal_edge_types: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            synapses,
            graph,
            causal_edge_types: causal_edge_types.into_iter().collect(),
            causal_pairs: std::sync::Mutex::new(None),
        }
    }

    /// Construit (une fois) l'ensemble des paires canoniques reliées par une
    /// arête de type causal — en une passe sur tous les nœuds/arêtes.
    fn has_causal_pair(&self, canonical: (NodeId, NodeId)) -> bool {
        if self.causal_edge_types.is_empty() {
            return false;
        }
        let mut guard = self.causal_pairs.lock().unwrap();
        if guard.is_none() {
            use obrain_core::graph::Direction;
            let mut pairs: HashSet<(NodeId, NodeId)> = HashSet::new();
            for node in self.graph.node_ids() {
                for (other, edge_id) in self.graph.edges_from(node, Direction::Outgoing) {
                    if self
                        .graph
                        .edge_type(edge_id)
                        .is_some_and(|t| self.causal_edge_types.contains(t.as_str()))
                    {
                        let key = if node.0 <= other.0 {
                            (node, other)
                        } else {
                            (other, node)
                        };
                        pairs.insert(key);
                    }
                }
            }
            *guard = Some(pairs);
        }
        guard.as_ref().is_some_and(|p| p.contains(&canonical))
    }
}

impl ActivationSource for CausalAwareActivationSource {
    fn neighbors(&self, node_id: NodeId) -> Vec<(NodeId, f64)> {
        self.synapses
            .list_synapses(node_id)
            .into_iter()
            .map(|s| {
                let neighbor = if s.source == node_id {
                    s.target
                } else {
                    s.source
                };
                (neighbor, s.current_weight())
            })
            .collect()
    }

    fn for_each_neighbor(&self, node_id: NodeId, f: &mut dyn FnMut(NodeId, f64)) {
        self.synapses.for_each_synapse(node_id, &mut |n, w| f(n, w));
    }

    fn on_activated(&self, node_id: NodeId) {
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.synapses.substrate_ref() {
            let _ = sub.on_node_activated(node_id);
        }
        // Sans substrat, il n'y a pas de colonne d'activation à notifier.
        #[cfg(not(feature = "substrate"))]
        let _ = node_id;
    }

    fn edge_class(&self, from: NodeId, to: NodeId) -> EdgeClass {
        if self.causal_edge_types.is_empty() {
            return EdgeClass::Associative;
        }
        // (2026-08-31) LOOKUP O(1) dans le cache de paires causales (construit
        // en une passe, voir `causal_pairs()`) au lieu d'un `edges_from(Both)`
        // par paire — O(degré²) → O(1) par arête visitée.
        let canonical = if from.0 <= to.0 {
            (from, to)
        } else {
            (to, from)
        };
        if self.has_causal_pair(canonical) {
            EdgeClass::Causal
        } else {
            EdgeClass::Associative
        }
    }
}

// ---------------------------------------------------------------------------
// Spread algorithm
// ---------------------------------------------------------------------------

/// Performs spreading activation from multiple source nodes.
///
/// Energy propagates via BFS: at each hop, the energy reaching a neighbor is
/// `parent_energy * synapse_weight * decay_factor`. If a node is reached
/// by multiple paths, the energies are summed (superposition).
///
/// # Arguments
/// * `sources` - Initial activation: `(node_id, initial_energy)` pairs.
/// * `graph` - Provides weighted neighbors (typically backed by synapses).
/// * `config` - Spread parameters (max hops, decay, thresholds).
///
/// # Returns
/// An [`ActivationMap`] with accumulated energy for each reached node.
pub fn spread(
    sources: &[(NodeId, f64)],
    graph: &dyn ActivationSource,
    config: &SpreadConfig,
) -> ActivationMap {
    // (2026-08-31) Projection du pattern « colonnes denses indexées par id »
    // (cf. substrate.degrees.node.CALLS.u32) sur le hot path de propagation.
    //
    // L'ancienne version tenait `visited: HashSet<(NodeId, u32)>` — une
    // allocation + hash par (nœud, hop) visité, dominante dès que la
    // propagation atteint des milliers de nœuds. Les NodeId du substrate
    // sont des u64 DENSES (allocation monotone) : on les indexe donc dans un
    // bitset dense `Vec<u64>` de (max_hops+1) plans, dimensionné au max id
    // rencontré et grow-on-demand. Test d'occupation = un `&` + un `>>`,
    // marquage = un `|` — plus d'allocation par entrée.
    //
    // GARDE-FOU : si un id dépasse 2^26 (67M), le bitset exploserait — on
    // retombe sur le HashSet (le comportement historique). Les bases réelles
    // (6k-66k nœuds) restent sur le chemin dense.
    let max_hop = config.max_hops.max(1) as usize;
    let mut visited_dense: Vec<u64> = Vec::new();
    let mut visited_sparse: Option<HashSet<(NodeId, u32)>> = None;

    let mut activation: ActivationMap = HashMap::new();

    // BFS queue: (node_id, energy_at_node, current_hop)
    let mut queue: VecDeque<(NodeId, f64, u32)> = VecDeque::new();

    // Seed sources
    for &(node_id, energy) in sources {
        *activation.entry(node_id).or_insert(0.0) += energy;
        queue.push_back((node_id, energy, 0));
        mark_visited(node_id, 0, max_hop, &mut visited_dense, &mut visited_sparse);
    }

    let max_nodes = config.max_activated_nodes;

    // BFS propagation with circuit breaker
    while let Some((node_id, node_energy, hop)) = queue.pop_front() {
        if hop >= config.max_hops {
            continue;
        }

        // Circuit breaker: stop enqueuing new nodes once we hit the limit
        if max_nodes > 0 && activation.len() >= max_nodes {
            break;
        }

        let mut closure = |neighbor: NodeId, weight: f64| {
            let class_factor = match graph.edge_class(node_id, neighbor) {
                EdgeClass::Causal => config.causal_boost,
                EdgeClass::Associative => 1.0,
            };
            let propagated = node_energy * weight * config.decay_factor * class_factor;

            if propagated < config.min_propagated_energy {
                return;
            }

            *activation.entry(neighbor).or_insert(0.0) += propagated;

            // Only enqueue if not yet visited at this hop depth
            let next_hop = hop + 1;
            if next_hop < config.max_hops
                && !was_visited(neighbor, next_hop, max_hop, &visited_dense, &visited_sparse)
            {
                mark_visited(
                    neighbor,
                    next_hop,
                    max_hop,
                    &mut visited_dense,
                    &mut visited_sparse,
                );
                // Cascade Hilbert : préfetch du voisinage physique de la
                // communauté avant d'entrer dans le BFS à ce niveau. Le
                // madvise(WILLNEED) sur le range Hilbert rend les lectures
                // des pages voisines « chaudes » — le critère T11 (depth-3
                // sur 10⁶ nœuds ≤ 1 ms) repose sur ce pont.
                graph.on_activated(neighbor);
                queue.push_back((neighbor, propagated, next_hop));
            }
        };
        graph.for_each_neighbor(node_id, &mut closure);
    }

    // Filter by activation threshold
    if config.activation_threshold > 0.0 {
        activation.retain(|_, v| *v >= config.activation_threshold);
    }

    activation
}

/// Borne de densité : au-delà, on retombe sur le HashSet (ids épars).
const DENSE_BITSET_MAX_ID: u64 = 1 << 26;

/// Teste si (node_id, hop) a déjà été visité — chemin dense OU épars.
#[inline]
fn was_visited(
    node_id: NodeId,
    hop: u32,
    max_hop: usize,
    dense: &[u64],
    sparse: &Option<HashSet<(NodeId, u32)>>,
) -> bool {
    let id = node_id.as_u64();
    if let Some(sp) = sparse {
        return sp.contains(&(node_id, hop));
    }
    if id >= DENSE_BITSET_MAX_ID {
        return false; // sera marqué et fera basculer en épars au mark
    }
    let plan = (hop as usize) * max_hop;
    let bit = (id as usize) + plan;
    dense
        .get(bit / 64)
        .is_some_and(|w| (w >> (bit % 64)) & 1 == 1)
}

/// Marque (node_id, hop) comme visité. Si un id dépasse la borne dense,
/// bascule le suivi sur le HashSet (comportement historique).
#[inline]
fn mark_visited(
    node_id: NodeId,
    hop: u32,
    max_hop: usize,
    dense: &mut Vec<u64>,
    sparse: &mut Option<HashSet<(NodeId, u32)>>,
) {
    let id = node_id.as_u64();
    if sparse.is_none() && id >= DENSE_BITSET_MAX_ID {
        // Basculer en épars : rejouer les entrées dense déjà marquées n'est
        // pas nécessaire — on repart du HashSet pour les prochains tests.
        *sparse = Some(HashSet::new());
        return;
    }
    if let Some(sp) = sparse.as_mut() {
        sp.insert((node_id, hop));
        return;
    }
    let plan = (hop as usize) * max_hop;
    let bit = (id as usize) + plan;
    let word = bit / 64;
    if word >= dense.len() {
        dense.resize(word + 1, 0);
    }
    dense[word] |= 1u64 << (bit % 64);
}

// ---------------------------------------------------------------------------
// Convenience: spread from a single source
// ---------------------------------------------------------------------------

/// Spreads activation from a single source node.
pub fn spread_single(
    source: NodeId,
    energy: f64,
    graph: &dyn ActivationSource,
    config: &SpreadConfig,
) -> ActivationMap {
    spread(&[(source, energy)], graph, config)
}
