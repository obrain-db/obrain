//! Dynamic contextual synthesis — schema-less aggregation of heterogeneous
//! graph nodes into a single, budget-bounded text view.
//!
//! Chains [`crate::context_loader::load_context`] (community-bounded
//! spreading activation, already generic across node types) with a neutral
//! text formatter and a pluggable summarization step.
//!
//! # Design principle: no LLM call baked into obrain-db
//!
//! obrain-db has no HTTP client dependency, no LLM provider lock-in, and no
//! API keys anywhere in the codebase — introducing one is a real
//! product/cost/secrets decision that shouldn't be made unilaterally inside
//! the storage engine. [`Summarizer`] is therefore a pluggable trait (same
//! shape as [`crate::search::Reranker`]/`NoopReranker`): the built-in
//! [`NoopSummarizer`] returns the composed context text as-is
//! (budget-truncated), and any caller that wants LLM-backed summarization
//! (obrain-tmn, obrain-hub, obrain-chat, ...) supplies its own
//! `Box<dyn Summarizer>` from its own code. Composition, not coupling —
//! consistent with the genericity guideline recorded on this feature's
//! parent plan: obrain-db never special-cases a specific consumer's output
//! format.
//!
//! # Schema-less by construction
//!
//! [`ContextBundle`] mixes whatever
//! node types/labels the seeds happen to activate (missions, notes,
//! axioms, ...) — the formatter groups by label without assuming any fixed
//! schema, and opportunistically surfaces a text snippet only when a node
//! happens to carry one of a caller-configurable set of property names
//! (default: `content`, `text`, `description`, `summary`, `title`, `name`).
//! Nodes without any of those properties still appear, with structural
//! metadata only (id, score, depth, community) — the engine never assumes
//! *which* property holds "the content".

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use obrain_common::types::{NodeId, PropertyKey};
use obrain_core::graph::{Direction, GraphStore};

use crate::context_loader::{
    ActivatedNode, CommunityProvider, ContextBundle, NeighborProvider, TokenEstimator, load_context,
};
use crate::fabric::FabricStore;
use crate::kernel_params::CognitiveKernelConfig;
#[cfg(feature = "synapse")]
use crate::synapse::SynapseStore;

// ---------------------------------------------------------------------------
// GraphStore-backed providers
// ---------------------------------------------------------------------------

/// [`NeighborProvider`] backed directly by a [`GraphStore`]. Weighted by
/// synapse strength when a [`SynapseStore`] is attached via
/// [`Self::with_synapses`]; otherwise every neighbor gets a uniform weight
/// of `1.0` (pure structural spreading, no Hebbian bias).
pub struct GraphNeighborProvider {
    graph: Arc<dyn GraphStore>,
    #[cfg(feature = "synapse")]
    synapses: Option<Arc<SynapseStore>>,
}

impl GraphNeighborProvider {
    /// Creates a provider using plain graph adjacency (uniform weight 1.0).
    pub fn new(graph: Arc<dyn GraphStore>) -> Self {
        Self {
            graph,
            #[cfg(feature = "synapse")]
            synapses: None,
        }
    }

    /// Attaches a synapse store — neighbor weights become Hebbian synapse
    /// strength instead of uniform 1.0.
    #[cfg(feature = "synapse")]
    #[must_use]
    pub fn with_synapses(mut self, synapses: Arc<SynapseStore>) -> Self {
        self.synapses = Some(synapses);
        self
    }
}

impl NeighborProvider for GraphNeighborProvider {
    fn neighbors(&self, node_id: NodeId) -> Vec<(NodeId, f64)> {
        #[cfg(feature = "synapse")]
        if let Some(synapses) = &self.synapses {
            return synapses
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
                .collect();
        }
        self.graph
            .neighbors(node_id, Direction::Both)
            .into_iter()
            .map(|n| (n, 1.0))
            .collect()
    }
}

/// [`CommunityProvider`] backed by [`FabricStore`]'s GDS-refreshed
/// `community_id` (Louvain, see `crate::gds_refresh`).
///
/// Cohesion isn't independently tracked by `FabricStore`, so it defaults to
/// a neutral `1.0` (never blocks a community-boundary crossing in
/// `load_context`) — compose a different provider if per-community
/// cohesion gating matters for a given caller.
pub struct FabricCommunityProvider {
    fabric: Arc<FabricStore>,
}

impl FabricCommunityProvider {
    /// Creates a provider backed by the given fabric store.
    pub fn new(fabric: Arc<FabricStore>) -> Self {
        Self { fabric }
    }
}

impl CommunityProvider for FabricCommunityProvider {
    fn community_of(&self, node_id: NodeId) -> Option<u64> {
        self.fabric.get_fabric_score(node_id).community_id
    }

    fn community_cohesion(&self, _community_id: u64) -> f64 {
        1.0
    }
}

// ---------------------------------------------------------------------------
// Pluggable summarizer
// ---------------------------------------------------------------------------

/// Turns composed context text into the final synthesis output.
///
/// obrain-db ships exactly one implementation ([`NoopSummarizer`]) — see
/// the module-level design note on why no LLM-backed implementation lives
/// here.
pub trait Summarizer: Send + Sync {
    /// Summarizes (or otherwise transforms) `context_text`, targeting
    /// roughly `budget_tokens`.
    fn summarize(&self, context_text: &str, budget_tokens: u32) -> String;
}

/// Default [`Summarizer`]: returns the composed context unsummarized,
/// truncated to approximately `budget_tokens` (a coarse `chars / 4`
/// heuristic — good enough for a structural/metadata-heavy text, not meant
/// to replace a real tokenizer).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSummarizer;

impl Summarizer for NoopSummarizer {
    fn summarize(&self, context_text: &str, budget_tokens: u32) -> String {
        let max_chars = (budget_tokens as usize).saturating_mul(4);
        if context_text.len() <= max_chars || max_chars == 0 {
            return context_text.to_string();
        }
        // Truncate on a char boundary, not a byte boundary.
        let mut end = max_chars.min(context_text.len());
        while end > 0 && !context_text.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = context_text[..end].to_string();
        truncated.push_str("\n...[truncated]");
        truncated
    }
}

// ---------------------------------------------------------------------------
// Synthesis
// ---------------------------------------------------------------------------

/// Property names opportunistically checked, in order, for a human-readable
/// text snippet on an activated node. Caller-configurable via
/// [`SynthesisConfig::content_properties`] — obrain-db imposes no fixed
/// schema of "which property holds the content".
pub const DEFAULT_CONTENT_PROPERTIES: &[&str] =
    &["content", "text", "description", "summary", "title", "name"];

/// Configuration for [`synthesize`].
#[derive(Debug, Clone)]
pub struct SynthesisConfig {
    /// Spreading/community-boundary parameters forwarded to `load_context`.
    pub kernel_config: CognitiveKernelConfig,
    /// Target token budget for the final (post-summarizer) text.
    pub budget_tokens: u32,
    /// Property names tried, in order, to extract a text snippet per node.
    pub content_properties: Vec<String>,
}

impl Default for SynthesisConfig {
    fn default() -> Self {
        Self {
            kernel_config: CognitiveKernelConfig::default(),
            budget_tokens: 750,
            content_properties: DEFAULT_CONTENT_PROPERTIES
                .iter()
                .map(|&s| s.to_string())
                .collect(),
        }
    }
}

/// Result of a synthesis run.
#[derive(Debug, Clone)]
pub struct SynthesisResult {
    /// Final text, after the [`Summarizer`] step.
    pub text: String,
    /// The underlying activation bundle (nodes, scores, communities
    /// touched) — kept for callers that want the raw structure alongside
    /// the text.
    pub bundle: ContextBundle,
}

/// Runs the full dynamic-synthesis chain:
/// seeds → `load_context` (community-bounded spreading activation) →
/// neutral structured text (grouped by label) → [`Summarizer`].
///
/// Generic and schema-less by construction — see module docs.
pub fn synthesize(
    seeds: &[NodeId],
    store: &dyn GraphStore,
    neighbors: &dyn NeighborProvider,
    communities: &dyn CommunityProvider,
    tokens: &dyn TokenEstimator,
    config: &SynthesisConfig,
    summarizer: &dyn Summarizer,
) -> SynthesisResult {
    let bundle = load_context(seeds, &config.kernel_config, neighbors, communities, tokens);
    let text = format_bundle(&bundle, store, &config.content_properties);
    let text = summarizer.summarize(&text, config.budget_tokens);
    SynthesisResult { text, bundle }
}

/// Formats a [`ContextBundle`] as structured text, grouped by node label
/// (or `"Unlabeled"`), each group sorted by activation score descending.
fn format_bundle(
    bundle: &ContextBundle,
    store: &dyn GraphStore,
    content_properties: &[String],
) -> String {
    let mut by_label: HashMap<String, Vec<&ActivatedNode>> = HashMap::new();
    for node in &bundle.nodes {
        let labels = store.get_node_labels(node.node_id).unwrap_or_default();
        let key = if labels.is_empty() {
            "Unlabeled".to_string()
        } else {
            labels
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        by_label.entry(key).or_default().push(node);
    }

    let mut groups: Vec<(String, Vec<&ActivatedNode>)> = by_label.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();
    for (label, mut nodes) in groups {
        nodes.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let _ = writeln!(out, "## {label}");
        for n in nodes {
            let snippet = extract_text_snippet(store, n.node_id, content_properties);
            match snippet {
                Some(s) => {
                    let _ = writeln!(
                        out,
                        "- (id={}, score={:.3}, depth={}) {}",
                        n.node_id.0, n.score, n.depth, s
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "- (id={}, score={:.3}, depth={})",
                        n.node_id.0, n.score, n.depth
                    );
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Returns the first matching property (in `content_properties` order)
/// that resolves to a string value on the given node.
fn extract_text_snippet(
    store: &dyn GraphStore,
    node_id: NodeId,
    content_properties: &[String],
) -> Option<String> {
    let node = store.get_node(node_id)?;
    for key in content_properties {
        if let Some(s) = node
            .properties
            .get(&PropertyKey::from(key.as_str()))
            .and_then(|v| v.as_str())
        {
            return Some(s.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_loader::FixedTokenEstimator;
    use obrain_core::graph::GraphStoreMut;
    use obrain_substrate::SubstrateStore;

    #[test]
    fn noop_summarizer_passes_through_short_text() {
        let s = NoopSummarizer;
        assert_eq!(s.summarize("hello", 100), "hello");
    }

    #[test]
    fn noop_summarizer_truncates_long_text() {
        let s = NoopSummarizer;
        let long = "a".repeat(1000);
        let result = s.summarize(&long, 10); // budget=10 tokens -> ~40 chars
        assert!(result.len() < long.len());
        assert!(result.ends_with("...[truncated]"));
    }

    #[test]
    fn synthesize_groups_by_label_and_includes_content_property() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let mission = store.create_node(&["Mission"]);
        let axiom = store.create_node(&["Axiom"]);
        store.create_edge(mission, axiom, "RELIES_ON");
        store.set_node_property(
            mission,
            "title",
            obrain_common::types::Value::from("Reach orbit"),
        );
        store.set_node_property(
            axiom,
            "content",
            obrain_common::types::Value::from("Energy is conserved"),
        );

        let graph: Arc<dyn GraphStore> = Arc::new(store);
        let neighbors = GraphNeighborProvider::new(Arc::clone(&graph));

        struct NoCommunities;
        impl CommunityProvider for NoCommunities {
            fn community_of(&self, _node_id: NodeId) -> Option<u64> {
                None
            }
            fn community_cohesion(&self, _community_id: u64) -> f64 {
                1.0
            }
        }

        let config = SynthesisConfig::default();
        let result = synthesize(
            &[mission],
            graph.as_ref(),
            &neighbors,
            &NoCommunities,
            &FixedTokenEstimator(50),
            &config,
            &NoopSummarizer,
        );

        assert!(result.text.contains("## Axiom"));
        assert!(result.text.contains("## Mission"));
        assert!(result.text.contains("Reach orbit"));
        assert!(result.text.contains("Energy is conserved"));
        assert!(!result.bundle.nodes.is_empty(), "résultat attendu non vide");
    }

    #[test]
    fn synthesize_handles_nodes_without_any_content_property() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let bare = store.create_node(&["Mystery"]);

        let graph: Arc<dyn GraphStore> = Arc::new(store);
        let neighbors = GraphNeighborProvider::new(Arc::clone(&graph));

        struct NoCommunities;
        impl CommunityProvider for NoCommunities {
            fn community_of(&self, _node_id: NodeId) -> Option<u64> {
                None
            }
            fn community_cohesion(&self, _community_id: u64) -> f64 {
                1.0
            }
        }

        let config = SynthesisConfig::default();
        let result = synthesize(
            &[bare],
            graph.as_ref(),
            &neighbors,
            &NoCommunities,
            &FixedTokenEstimator(50),
            &config,
            &NoopSummarizer,
        );

        // Node still appears (structural metadata only), engine never
        // panics/errors just because no content property exists.
        assert!(result.text.contains("## Mystery"));
        assert!(result.text.contains(&format!("id={}", bare.0)));
    }

    #[test]
    fn fabric_community_provider_reads_from_fabric_store() {
        let fabric = Arc::new(FabricStore::new());
        let node = NodeId(1);
        fabric.set_gds_metrics(node, 0.5, 0.2, Some(7));

        let provider = FabricCommunityProvider::new(fabric);
        assert_eq!(provider.community_of(node), Some(7));
        assert_eq!(provider.community_of(NodeId(999)), None);
        assert_eq!(provider.community_cohesion(7), 1.0);
    }
}
