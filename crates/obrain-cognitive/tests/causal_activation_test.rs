//! Integration tests for causal-aware spreading activation
//! (`EdgeClass`, `SpreadConfig::causal_boost`, `CausalAwareActivationSource`).

#![cfg(feature = "synapse")]

use obrain_cognitive::activation::{
    ActivationSource, CausalAwareActivationSource, EdgeClass, SpreadConfig, spread_single,
};
use obrain_cognitive::{SynapseConfig, SynapseStore};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

fn n(id: u64) -> NodeId {
    NodeId::new(id)
}

fn make_synapse_store() -> Arc<SynapseStore> {
    Arc::new(SynapseStore::new(SynapseConfig {
        initial_weight: 0.0,
        default_half_life: Duration::from_secs(3600 * 24 * 365),
        ..SynapseConfig::default()
    }))
}

// ---------------------------------------------------------------------------
// Default behavior is unchanged (backward compatibility)
// ---------------------------------------------------------------------------

#[test]
fn default_edge_class_is_associative_and_causal_boost_is_noop() {
    let default_config = SpreadConfig::default();
    assert_eq!(default_config.causal_boost, 1.0);

    struct DummySource;
    impl ActivationSource for DummySource {
        fn neighbors(&self, _node_id: NodeId) -> Vec<(NodeId, f64)> {
            vec![]
        }
    }
    // Trait default: always Associative, regardless of the pair.
    assert_eq!(DummySource.edge_class(n(1), n(2)), EdgeClass::Associative);
}

#[test]
fn spread_with_default_causal_boost_matches_plain_synapse_source() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "MENTIONS");

    let synapses = make_synapse_store();
    synapses.reinforce(a, b, 0.8);

    let causal_source =
        CausalAwareActivationSource::new(Arc::clone(&synapses), Arc::new(store), vec![]);
    let plain_source =
        obrain_cognitive::activation::SynapseActivationSource::new(Arc::clone(&synapses));

    let config = SpreadConfig {
        max_hops: 1,
        decay_factor: 0.5,
        min_propagated_energy: 0.001,
        ..Default::default()
    };

    let causal_map = spread_single(a, 1.0, &causal_source, &config);
    let plain_map = spread_single(a, 1.0, &plain_source, &config);

    // Not a strict assert_eq!: each call re-reads the live (exponentially
    // decaying) synapse weight, so consecutive reads differ by a
    // sub-nanosecond-decay epsilon — not by anything causal_boost-related.
    assert_eq!(causal_map.len(), plain_map.len());
    for (node, energy) in &causal_map {
        let other = plain_map[node];
        assert!(
            (energy - other).abs() < 1e-6,
            "node {node:?}: causal={energy}, plain={other}"
        );
    }
}

// ---------------------------------------------------------------------------
// Causal boost actually prioritizes classified edges
// ---------------------------------------------------------------------------

#[test]
fn causal_boost_amplifies_propagation_over_causal_edges() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let n1 = store.create_node(&["Node"]);
    let n2 = store.create_node(&["Node"]);
    let n3 = store.create_node(&["Node"]);
    store.create_edge(n1, n2, "CAUSES");
    store.create_edge(n1, n3, "MENTIONS");

    let synapses = make_synapse_store();
    // Equal synapse weight on both branches — any score difference below
    // must come purely from edge classification, not from synapse strength.
    synapses.reinforce(n1, n2, 0.6); // classified CAUSES
    synapses.reinforce(n1, n3, 0.6); // classified MENTIONS

    let source =
        CausalAwareActivationSource::new(synapses, Arc::new(store), vec!["CAUSES".to_string()]);

    assert_eq!(source.edge_class(n1, n2), EdgeClass::Causal);
    assert_eq!(source.edge_class(n1, n3), EdgeClass::Associative);

    let config = SpreadConfig {
        max_hops: 1,
        decay_factor: 0.5,
        min_propagated_energy: 0.0,
        causal_boost: 3.0,
        ..Default::default()
    };

    let map = spread_single(n1, 1.0, &source, &config);
    let energy_via_causes = map[&n2];
    let energy_via_mentions = map[&n3];

    assert!(
        energy_via_causes > energy_via_mentions,
        "CAUSES ({energy_via_causes}) should outrank MENTIONS ({energy_via_mentions}) with causal_boost=3.0"
    );
    // Exactly 3x: same synapse weight, same decay, only the class factor differs.
    assert!(
        (energy_via_causes / energy_via_mentions - 3.0).abs() < 1e-9,
        "ratio should be exactly causal_boost (3.0), got {}",
        energy_via_causes / energy_via_mentions
    );
}

#[test]
fn empty_causal_edge_types_means_everything_is_associative() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "CAUSES"); // would match if causal_edge_types weren't empty

    let synapses = make_synapse_store();
    synapses.reinforce(a, b, 0.5);

    let source = CausalAwareActivationSource::new(synapses, Arc::new(store), vec![]);
    assert_eq!(source.edge_class(a, b), EdgeClass::Associative);
}
