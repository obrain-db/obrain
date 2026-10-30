//! Structural gap detection — missing-link candidates, dead/orphan nodes,
//! and the structural `knowledge_gap` fabric metric.
//!
//! Complements [`crate::fabric`]'s `annotation_gap` (missing *notes/context*)
//! with a structural signal: nodes that are semantically/structurally close
//! (neighborhood overlap, structural-fingerprint twins) but not directly
//! linked, and nodes that have accumulated activation energy but no
//! outgoing edge toward an action/result (dead ends / orphan branches).
//!
//! # Design principle: read-only, generic, consumer-agnostic
//!
//! This module NEVER mutates the graph — every function takes `&dyn
//! GraphStore` (read-only), not `&dyn GraphStoreMut`, so it is *structurally
//! impossible* to create an edge/node from here. It only produces scored
//! candidates; acting on them (or not) is entirely up to the caller.
//!
//! The API is intentionally generic: plain seeds/scope/thresholds in,
//! scored candidates out. No output shape here is tailored to any specific
//! consumer (LLM injection, a particular UI, etc.) — see the "genericity"
//! guideline recorded on the parent plan.
//!
//! # Composition
//!
//! - Neighborhood similarity: [`obrain_adapters::plugins::algorithms::top_k_similar`]
//!   (Jaccard / Adamic-Adar / Cosine / Overlap / Resource-Allocation).
//! - Structural-twin detection: [`crate::fingerprint`] (`fingerprint()` +
//!   `detect_twins()`), computed over a bounded ego-network adjacency.
//! - Dead/orphan detection: [`crate::energy`] (`EnergyStore::get_energy`)
//!   crossed with [`obrain_core::graph::GraphStore`] out-degree/edge-type.
//! - Scan prioritization: [`crate::stagnation`] (`StagnationStore::get_stagnant_zones`)
//!   — see [`prioritize_communities`].

use std::collections::{HashMap, HashSet};

use obrain_adapters::plugins::algorithms::{SimilarityMetric, top_k_similar};
use obrain_common::types::NodeId;
use obrain_core::graph::{Direction, GraphStore};

#[cfg(test)]
use crate::energy::EnergyConfig;
use crate::energy::EnergyStore;
use crate::fingerprint::{self, StructuralFingerprint};
use crate::stagnation::{StagnationScore, StagnationStore};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tunables for gap detection. All thresholds are in `[0.0, 1.0]` unless
/// noted otherwise.
#[derive(Debug, Clone)]
pub struct GapDetectionConfig {
    /// Neighborhood-similarity metric used for the top-k candidate pass.
    pub similarity_metric: SimilarityMetric,
    /// How many candidate neighbors to consider per node.
    pub top_k_per_node: usize,
    /// Minimum combined score for a [`MissingLinkCandidate`] to be reported.
    pub min_link_score: f64,
    /// Minimum fingerprint-comparison score for two nodes to count as
    /// "structural twins" (see [`crate::fingerprint::compare`]).
    pub twin_threshold: f64,
    /// Depth of the ego-network BFS used both to build fingerprints and to
    /// decide whether two nodes are "already reachable" (and thus not a
    /// genuine gap).
    pub max_path_hops: usize,
    /// Relation types considered "causal" for dead-node detection. Empty =
    /// any outgoing edge counts as "not dead" (falls back to plain
    /// out-degree == 0).
    pub causal_edge_types: Vec<String>,
    /// Minimum energy for a node to be eligible as a dead-node candidate
    /// (low-energy leaves are just unused, not "dead ends worth wiring up").
    pub dead_node_min_energy: f64,
    /// Weight of the neighborhood-similarity signal in the combined score.
    pub w_neighborhood: f64,
    /// Weight of the structural-twin signal in the combined score.
    pub w_twin: f64,
}

impl Default for GapDetectionConfig {
    fn default() -> Self {
        Self {
            similarity_metric: SimilarityMetric::AdamicAdar,
            top_k_per_node: 5,
            min_link_score: 0.15,
            twin_threshold: 0.7,
            max_path_hops: 2,
            causal_edge_types: Vec::new(),
            dead_node_min_energy: 0.3,
            w_neighborhood: 0.6,
            w_twin: 0.4,
        }
    }
}

// ---------------------------------------------------------------------------
// MissingLinkCandidate
// ---------------------------------------------------------------------------

/// A candidate for a missing edge between two nodes that are structurally
/// or semantically close but not directly (or closely) connected.
#[derive(Debug, Clone, PartialEq)]
pub struct MissingLinkCandidate {
    /// One endpoint of the candidate link (unordered pair with `target`).
    pub source: NodeId,
    /// The other endpoint of the candidate link.
    pub target: NodeId,
    /// Combined plausibility score in `[0.0, 1.0]`. Higher = more likely
    /// this link "should" exist.
    pub score: f64,
    /// Per-signal breakdown, for explainability (never used to auto-act).
    pub evidence: LinkEvidence,
}

/// Explains why a [`MissingLinkCandidate`] was surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinkEvidence {
    /// Neighborhood-overlap similarity (Jaccard/Adamic-Adar/...).
    pub neighborhood_similarity: f64,
    /// Structural-fingerprint twin similarity (degree distribution,
    /// clustering coefficient, motif counts).
    pub structural_twin_similarity: f64,
    /// Shortest path length already connecting the two nodes, if any was
    /// found within `max_path_hops`. `None` means genuinely disconnected
    /// within the search horizon — the strongest gap signal.
    pub shortest_path_hops: Option<usize>,
}

// ---------------------------------------------------------------------------
// DeadNodeCandidate
// ---------------------------------------------------------------------------

/// A node with meaningful recent activation energy but no outgoing
/// edge continuing the graph — a dead end or orphan branch.
#[derive(Debug, Clone, PartialEq)]
pub struct DeadNodeCandidate {
    /// The flagged node.
    pub node: NodeId,
    /// The node's current activation energy at scan time.
    pub energy: f64,
    /// The node's total outgoing-edge count at scan time.
    pub out_degree: usize,
    /// Why this node was flagged.
    pub reason: DeadNodeReason,
}

/// Why a node was flagged as dead/orphaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadNodeReason {
    /// Zero outgoing edges of any type — a true leaf.
    NoOutgoingEdges,
    /// Has outgoing edges, but none match the configured causal relation
    /// types (only relevant when `causal_edge_types` is non-empty).
    NoOutgoingCausalEdge,
}

// ---------------------------------------------------------------------------
// Aggregate report
// ---------------------------------------------------------------------------

/// Combined output of a gap-detection scan. Purely informational — no
/// mutation is ever performed by this module.
#[derive(Debug, Clone, Default)]
pub struct GapReport {
    /// Missing-link candidates found in the scanned scope.
    pub missing_links: Vec<MissingLinkCandidate>,
    /// Dead/orphan node candidates found in the scanned scope.
    pub dead_nodes: Vec<DeadNodeCandidate>,
}

// ---------------------------------------------------------------------------
// Ego-network / reachability helpers
// ---------------------------------------------------------------------------

/// Builds a bounded-depth BFS adjacency map around `node`, used as input to
/// [`crate::fingerprint::fingerprint`] (which characterizes a node by the
/// shape of its local neighborhood, not the whole graph).
fn ego_adjacency(store: &dyn GraphStore, node: NodeId, depth: usize) -> HashMap<u64, Vec<u64>> {
    let mut adjacency: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut visited: HashSet<NodeId> = HashSet::from([node]);
    let mut frontier = vec![node];

    for _ in 0..depth.max(1) {
        let mut next = Vec::new();
        for n in frontier {
            let neighbors = store.neighbors(n, Direction::Both);
            adjacency.insert(n.0, neighbors.iter().map(|x| x.0).collect());
            for nb in neighbors {
                if visited.insert(nb) {
                    next.push(nb);
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    adjacency
}

/// Bounded BFS: returns the shortest path length from `from` to `to` if one
/// exists within `max_hops`, `None` otherwise (treated as "genuinely
/// disconnected" for gap-detection purposes — the graph may still have a
/// longer path, which is exactly why it's a candidate worth surfacing).
fn reachable_within(
    store: &dyn GraphStore,
    from: NodeId,
    to: NodeId,
    max_hops: usize,
) -> Option<usize> {
    if from == to {
        return Some(0);
    }
    let mut visited: HashSet<NodeId> = HashSet::from([from]);
    let mut frontier = vec![from];

    for hop in 1..=max_hops {
        let mut next = Vec::new();
        for n in frontier {
            for nb in store.neighbors(n, Direction::Both) {
                if nb == to {
                    return Some(hop);
                }
                if visited.insert(nb) {
                    next.push(nb);
                }
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Missing-link enumeration
// ---------------------------------------------------------------------------

/// Enumerates missing-link candidates among `candidate_nodes`, combining
/// neighborhood similarity with structural-fingerprint twin detection.
///
/// Bound `candidate_nodes` to a tractable scope (e.g. one community from
/// [`prioritize_communities`]) — this is O(n·k) in neighborhood similarity
/// plus O(n²) in the twin-detection pass, not meant to run over an entire
/// large graph in one call.
pub fn detect_missing_links(
    store: &dyn GraphStore,
    candidate_nodes: &[NodeId],
    config: &GapDetectionConfig,
) -> Vec<MissingLinkCandidate> {
    if candidate_nodes.len() < 2 {
        return Vec::new();
    }

    // Structural-twin pass: fingerprint each node's bounded ego-network,
    // then compare pairwise via detect_twins().
    let fingerprints: Vec<(u64, StructuralFingerprint)> = candidate_nodes
        .iter()
        .map(|&n| {
            let adjacency = ego_adjacency(store, n, config.max_path_hops);
            (n.0, fingerprint::fingerprint(&adjacency))
        })
        .collect();
    let twin_scores: HashMap<(u64, u64), f64> =
        fingerprint::detect_twins(&fingerprints, config.twin_threshold)
            .into_iter()
            .map(|(a, b, score)| ((a.min(b), a.max(b)), score))
            .collect();

    // Neighborhood-similarity pass: top-k per node, deduplicated by
    // unordered pair, keeping the best-evidenced score per pair.
    let mut candidates: HashMap<(u64, u64), MissingLinkCandidate> = HashMap::new();
    for &node in candidate_nodes {
        let direct_neighbors: HashSet<NodeId> =
            store.neighbors(node, Direction::Both).into_iter().collect();

        for similar in top_k_similar(store, node, config.top_k_per_node, config.similarity_metric) {
            if similar.node == node || direct_neighbors.contains(&similar.node) {
                continue; // already linked — not a gap
            }

            let key = (node.0.min(similar.node.0), node.0.max(similar.node.0));
            let twin_similarity = twin_scores.get(&key).copied().unwrap_or(0.0);
            let combined = similar.score * config.w_neighborhood + twin_similarity * config.w_twin;
            if combined < config.min_link_score {
                continue;
            }

            let hops = reachable_within(store, node, similar.node, config.max_path_hops);
            let (source, target) = (NodeId(key.0), NodeId(key.1));

            candidates
                .entry(key)
                .and_modify(|existing| {
                    if combined > existing.score {
                        existing.score = combined;
                        existing.evidence = LinkEvidence {
                            neighborhood_similarity: similar.score,
                            structural_twin_similarity: twin_similarity,
                            shortest_path_hops: hops,
                        };
                    }
                })
                .or_insert(MissingLinkCandidate {
                    source,
                    target,
                    score: combined,
                    evidence: LinkEvidence {
                        neighborhood_similarity: similar.score,
                        structural_twin_similarity: twin_similarity,
                        shortest_path_hops: hops,
                    },
                });
        }
    }

    let mut result: Vec<MissingLinkCandidate> = candidates.into_values().collect();
    result.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

// ---------------------------------------------------------------------------
// Dead / orphan node detection
// ---------------------------------------------------------------------------

/// Flags nodes with meaningful activation energy but no (causal) outgoing
/// edge — dead ends the graph accumulated attention on without ever
/// "continuing" toward an action or result.
pub fn detect_dead_nodes(
    store: &dyn GraphStore,
    energy_store: &EnergyStore,
    candidate_nodes: &[NodeId],
    config: &GapDetectionConfig,
) -> Vec<DeadNodeCandidate> {
    let mut result = Vec::new();

    for &node in candidate_nodes {
        let energy = energy_store.get_energy(node);
        if energy < config.dead_node_min_energy {
            continue;
        }

        let out_degree = store.out_degree(node);
        if out_degree == 0 {
            result.push(DeadNodeCandidate {
                node,
                energy,
                out_degree,
                reason: DeadNodeReason::NoOutgoingEdges,
            });
            continue;
        }

        if config.causal_edge_types.is_empty() {
            continue; // any outgoing edge counts as "not dead"
        }

        let has_causal_edge =
            store
                .edges_from(node, Direction::Outgoing)
                .into_iter()
                .any(|(_, edge_id)| {
                    store.edge_type(edge_id).is_some_and(|t| {
                        config
                            .causal_edge_types
                            .iter()
                            .any(|c| c.as_str() == t.as_str())
                    })
                });

        if !has_causal_edge {
            result.push(DeadNodeCandidate {
                node,
                energy,
                out_degree,
                reason: DeadNodeReason::NoOutgoingCausalEdge,
            });
        }
    }

    result.sort_by(|a, b| {
        b.energy
            .partial_cmp(&a.energy)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

// ---------------------------------------------------------------------------
// Combined scan
// ---------------------------------------------------------------------------

/// Runs both detectors over `scope` and returns a combined [`GapReport`].
/// `energy_store` is optional — when absent, dead-node detection is skipped
/// (missing-link detection alone doesn't require the energy subsystem).
pub fn detect_gaps(
    store: &dyn GraphStore,
    energy_store: Option<&EnergyStore>,
    scope: &[NodeId],
    config: &GapDetectionConfig,
) -> GapReport {
    let missing_links = detect_missing_links(store, scope, config);
    let dead_nodes = energy_store
        .map(|es| detect_dead_nodes(store, es, scope, config))
        .unwrap_or_default();
    GapReport {
        missing_links,
        dead_nodes,
    }
}

// ---------------------------------------------------------------------------
// Prioritization
// ---------------------------------------------------------------------------

/// Returns communities ordered by scan priority (most stagnant first), so a
/// caller with a limited scan budget spends it on the least healthy parts
/// of the graph before well-maintained ones. Thin wrapper over
/// [`StagnationStore::get_stagnant_zones`] kept here so gap-detection
/// callers don't need to know about the stagnation subsystem directly.
pub fn prioritize_communities(
    stagnation: &StagnationStore,
    threshold: f64,
) -> Vec<StagnationScore> {
    stagnation.get_stagnant_zones(threshold)
}

// ---------------------------------------------------------------------------
// knowledge_gap fabric metric
// ---------------------------------------------------------------------------

/// Derives a per-node `knowledge_gap` score in `[0.0, 1.0]` from a
/// [`GapReport`], suitable for [`crate::fabric::FabricStore::set_knowledge_gap`].
///
/// A node's gap exposure is the sum of missing-link scores touching it
/// (both as source and target), plus a fixed penalty for being a confirmed
/// dead end — clamped to `[0.0, 1.0]`.
pub fn compute_knowledge_gap_scores(report: &GapReport) -> HashMap<NodeId, f64> {
    let mut raw: HashMap<NodeId, f64> = HashMap::new();

    for candidate in &report.missing_links {
        *raw.entry(candidate.source).or_insert(0.0) += candidate.score;
        *raw.entry(candidate.target).or_insert(0.0) += candidate.score;
    }
    for dead in &report.dead_nodes {
        *raw.entry(dead.node).or_insert(0.0) += 1.0;
    }

    raw.into_iter()
        .map(|(k, v)| (k, v.clamp(0.0, 1.0)))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_core::graph::GraphStoreMut;
    use obrain_substrate::SubstrateStore;

    /// Two "missions" (M1, M2) that each depend on a shared set of
    /// "prereq" nodes, but are never linked to each other directly nor
    /// via any short path — the case from the plan's acceptance criteria:
    /// "deux missions isolées partageant des prérequis communs non liées".
    ///
    /// ```text
    ///   M1 --- P1        M2 --- P1
    ///   M1 --- P2        M2 --- P2
    ///   M1 --- P3        M2 --- P3
    /// ```
    /// M1 and M2 share all 3 prereqs but have no direct edge and no path
    /// within 2 hops (P nodes aren't connected to each other either), so
    /// they should surface as a high-confidence missing-link candidate.
    fn shared_prereq_graph() -> (SubstrateStore, NodeId, NodeId, Vec<NodeId>) {
        let store = SubstrateStore::open_tempfile().unwrap();

        let m1 = store.create_node(&["Mission"]);
        let m2 = store.create_node(&["Mission"]);
        let prereqs: Vec<NodeId> = (0..3).map(|_| store.create_node(&["Prereq"])).collect();

        for &p in &prereqs {
            store.create_edge(m1, p, "REQUIRES");
            store.create_edge(p, m1, "REQUIRED_BY");
            store.create_edge(m2, p, "REQUIRES");
            store.create_edge(p, m2, "REQUIRED_BY");
        }

        (store, m1, m2, prereqs)
    }

    #[test]
    fn detects_missing_link_between_nodes_with_shared_prereqs() {
        let (store, m1, m2, prereqs) = shared_prereq_graph();
        let mut scope = vec![m1, m2];
        scope.extend(prereqs);

        let config = GapDetectionConfig {
            max_path_hops: 1, // M1 -> P -> M2 is 2 hops; force "not directly reachable"
            min_link_score: 0.05,
            ..GapDetectionConfig::default()
        };

        let candidates = detect_missing_links(&store, &scope, &config);
        let m1_m2 = candidates
            .iter()
            .find(|c| (c.source == m1 && c.target == m2) || (c.source == m2 && c.target == m1))
            .expect("M1/M2 should be surfaced as a missing-link candidate");

        assert!(m1_m2.score > 0.0);
        assert!(
            m1_m2.evidence.neighborhood_similarity > 0.0,
            "M1 and M2 share all 3 neighbors — Adamic-Adar similarity must be > 0"
        );
    }

    /// A node with meaningful energy but zero outgoing edges — "a concept
    /// with no link toward action/result" from the plan's acceptance
    /// criteria.
    #[test]
    fn detects_dead_node_with_no_outgoing_edges() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let concept = store.create_node(&["Concept"]);
        let unrelated = store.create_node(&["Concept"]);
        // Someone points AT `concept` (it has incoming edges / in-degree),
        // but it never leads anywhere.
        store.create_edge(unrelated, concept, "MENTIONS");

        let energy_store = EnergyStore::new(EnergyConfig::default());
        energy_store.boost(concept, 1.0);
        energy_store.boost(unrelated, 0.01); // stays under threshold

        let config = GapDetectionConfig::default();
        let dead = detect_dead_nodes(&store, &energy_store, &[concept, unrelated], &config);

        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].node, concept);
        assert_eq!(dead[0].reason, DeadNodeReason::NoOutgoingEdges);
    }

    /// Safety guard: detection never mutates the store. Enforced
    /// structurally (all detectors take `&dyn GraphStore`, not
    /// `&dyn GraphStoreMut`), verified here by checking node/edge counts
    /// are unchanged after a full scan.
    #[test]
    fn detect_gaps_never_mutates_the_graph() {
        let (store, m1, m2, prereqs) = shared_prereq_graph();
        let mut scope = vec![m1, m2];
        scope.extend(prereqs.clone());

        let nodes_before = store.node_ids().len();
        let edges_before: usize = scope
            .iter()
            .map(|&n| store.out_degree(n) as usize + store.in_degree(n) as usize)
            .sum();

        let energy_store = EnergyStore::new(EnergyConfig::default());
        let _report = detect_gaps(
            &store,
            Some(&energy_store),
            &scope,
            &GapDetectionConfig::default(),
        );

        let nodes_after = store.node_ids().len();
        let edges_after: usize = scope
            .iter()
            .map(|&n| store.out_degree(n) as usize + store.in_degree(n) as usize)
            .sum();

        assert_eq!(
            nodes_before, nodes_after,
            "gap detection must not create/delete nodes"
        );
        assert_eq!(
            edges_before, edges_after,
            "gap detection must not create/delete edges"
        );
    }

    #[test]
    fn compute_knowledge_gap_scores_clamps_to_unit_range() {
        let a = NodeId(1);
        let b = NodeId(2);
        let report = GapReport {
            missing_links: vec![
                MissingLinkCandidate {
                    source: a,
                    target: b,
                    score: 0.9,
                    evidence: LinkEvidence::default(),
                },
                MissingLinkCandidate {
                    source: a,
                    target: NodeId(3),
                    score: 0.8,
                    evidence: LinkEvidence::default(),
                },
            ],
            dead_nodes: vec![],
        };

        let scores = compute_knowledge_gap_scores(&report);
        // a accumulates 0.9 + 0.8 = 1.7, clamped to 1.0
        assert_eq!(scores.get(&a).copied(), Some(1.0));
        assert_eq!(scores.get(&b).copied(), Some(0.9));
    }

    #[test]
    fn prioritize_communities_scans_most_stagnant_first() {
        use crate::stagnation::{StagnationConfig, StagnationScore, StagnationStore, Trend};
        use std::time::Duration;

        let store = StagnationStore::new(StagnationConfig::default());
        // Community 1: low energy, old, quiet — stagnant.
        store.update(
            1,
            StagnationScore {
                community_id: 1,
                avg_energy: 0.1,
                last_mutation_age: Duration::from_secs(60 * 24 * 3600),
                synapse_activity: 0.05,
                stagnation_score: 0.9,
                trend: Trend::Stable,
            },
        );
        // Community 2: healthy — not stagnant.
        store.update(
            2,
            StagnationScore {
                community_id: 2,
                avg_energy: 0.8,
                last_mutation_age: Duration::from_secs(3600),
                synapse_activity: 0.9,
                stagnation_score: 0.1,
                trend: Trend::Stable,
            },
        );

        let zones = prioritize_communities(&store, 0.5);
        assert_eq!(
            zones.len(),
            1,
            "only community 1 should clear the 0.5 stagnation threshold"
        );
        assert_eq!(zones[0].community_id, 1);
    }
}
