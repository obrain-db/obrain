//! In-plan cognitive-boost wiring.
//!
//! [`obrain_core::execution::operators::CognitiveBoostOperator`] re-ranks
//! rows using a pre-computed `ActivationMap`, but historically nothing in
//! the query engine actually *computed* that map at plan time — callers had
//! to run spreading activation externally and inject the result. This
//! module closes that gap: [`build_cognitive_boost_operator`] runs
//! [`obrain_cognitive::activation::spread`] right here, using the seeds and
//! config supplied by the plan, and wraps a child operator with the result
//! — all within a single planning call.
//!
//! Kept in `obrain-engine` (not `obrain-core`) because it needs
//! [`CognitiveEngine`] (from `obrain-cognitive`), and `obrain-core` cannot
//! depend on `obrain-cognitive` (see the layering note on
//! [`obrain_core::execution::operators::cognitive_boost::ActivationMap`]).
//!
//! Generic, consumer-agnostic by design: seeds/config/mode are plain
//! parameters, the output is the same neutral `_cognitive_score` column
//! `CognitiveBoostOperator` always produces — no shape here is specific to
//! any particular caller.

use std::collections::HashMap;
use std::sync::Arc;

use obrain_cognitive::CognitiveEngine;
use obrain_cognitive::activation::{
    CausalAwareActivationSource, SpreadConfig, SynapseActivationSource, spread,
};
use obrain_common::types::NodeId;
use obrain_core::execution::operators::Operator;
use obrain_core::execution::operators::cognitive_boost::{
    ActivationMap as CoreActivationMap, BoostMode, CognitiveBoostOperator,
};
use obrain_core::graph::GraphStore;

/// Parameters for in-plan spreading activation.
///
/// Mirrors `obrain_cognitive::activation::SpreadConfig` plus the
/// caller-supplied causal-edge-type list needed to build a
/// [`CausalAwareActivationSource`] (see that type's docs — obrain-db
/// imposes no fixed causal/associative ontology).
#[derive(Debug, Clone, Default)]
pub struct CognitiveBoostSpec {
    /// Seed nodes with initial energy (same shape as `spread()`'s `sources`).
    pub seeds: Vec<(NodeId, f64)>,
    /// Spreading-activation parameters (hops, decay, causal_boost, ...).
    pub spread_config: SpreadConfig,
    /// Relation types treated as causal. Empty = pure associative spread,
    /// identical to the historical synapse-only behavior (backward
    /// compatible default).
    pub causal_edge_types: Vec<String>,
}

/// Runs spreading activation now (using `engine`'s synapse store and
/// `store`'s graph structure for edge classification) and wraps `child`
/// with a [`CognitiveBoostOperator`] using the result.
///
/// Returns `child` unmodified, un-boosted, if the synapse subsystem isn't
/// available on `engine` — there's nothing to spread with, so this is a
/// graceful no-op rather than an error (mirrors the fallback pattern used
/// throughout `cognitive_procedures.rs`).
pub fn build_cognitive_boost_operator(
    spec: &CognitiveBoostSpec,
    engine: &Arc<dyn CognitiveEngine>,
    store: &Arc<dyn GraphStore>,
    child: Box<dyn Operator>,
    node_id_column: usize,
    score_column: Option<usize>,
    mode: BoostMode,
) -> Box<dyn Operator> {
    let Some(synapse_store) = engine.synapse_store() else {
        return child;
    };

    let activation_map: HashMap<NodeId, f64> = if spec.causal_edge_types.is_empty() {
        let source = SynapseActivationSource::new(Arc::clone(synapse_store));
        spread(&spec.seeds, &source, &spec.spread_config)
    } else {
        let source: CausalAwareActivationSource = CausalAwareActivationSource::new(
            Arc::clone(synapse_store),
            Arc::clone(store),
            spec.causal_edge_types.clone(),
        );
        spread(&spec.seeds, &source, &spec.spread_config)
    };

    let core_map: CoreActivationMap = activation_map
        .into_iter()
        .map(|(node_id, energy)| (node_id.0, energy))
        .collect();

    Box::new(CognitiveBoostOperator::new(
        child,
        core_map,
        node_id_column,
        score_column,
        mode,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_cognitive::{CognitiveConfig, CognitiveEngineBuilder};
    use obrain_common::types::LogicalType;
    use obrain_core::execution::DataChunk;
    use obrain_core::execution::operators::OperatorResult;
    use obrain_core::execution::vector::ValueVector;
    use obrain_core::graph::GraphStoreMut;
    use obrain_substrate::SubstrateStore;

    struct SingleChunkOperator {
        chunk: Option<DataChunk>,
    }
    impl Operator for SingleChunkOperator {
        fn next(&mut self) -> OperatorResult {
            Ok(self.chunk.take())
        }
        fn reset(&mut self) {}
        fn name(&self) -> &'static str {
            "SingleChunk"
        }
    }

    fn make_engine() -> Arc<dyn CognitiveEngine> {
        let bus = obrain_reactive::MutationBus::new();
        let scheduler =
            obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
        let mut config = CognitiveConfig::new();
        config.synapse.enabled = true;
        let engine = CognitiveEngineBuilder::from_config(&config).build(&scheduler);
        Arc::new(engine) as Arc<dyn CognitiveEngine>
    }

    #[tokio::test]
    async fn boosts_rows_using_activation_computed_in_plan() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let a = store.create_node(&["Node"]);
        let b = store.create_node(&["Node"]);
        let c = store.create_node(&["Node"]); // unreached by spread
        store.create_edge(a, b, "RELATED");
        let store: Arc<dyn GraphStore> = Arc::new(store);

        let engine = make_engine();
        if let Some(synapses) = engine.synapse_store() {
            synapses.reinforce(a, b, 0.9);
        }

        let mut id_col = ValueVector::with_type(LogicalType::Int64);
        id_col.push_int64(a.0 as i64);
        id_col.push_int64(b.0 as i64);
        id_col.push_int64(c.0 as i64);
        let child = Box::new(SingleChunkOperator {
            chunk: Some(DataChunk::new(vec![id_col])),
        });

        let spec = CognitiveBoostSpec {
            seeds: vec![(a, 1.0)],
            spread_config: SpreadConfig {
                max_hops: 1,
                decay_factor: 0.5,
                min_propagated_energy: 0.0,
                ..Default::default()
            },
            causal_edge_types: vec![],
        };

        let mut op = build_cognitive_boost_operator(
            &spec,
            &engine,
            &store,
            child,
            0,
            None,
            BoostMode::Replace,
        );

        let chunk = op.next().unwrap().unwrap();
        let score_col = chunk.column(1).unwrap();
        assert_eq!(score_col.get_float64(0), Some(1.0)); // seed itself
        assert!(score_col.get_float64(1).unwrap() > 0.0); // b, one hop away
        assert_eq!(score_col.get_float64(2), Some(0.0)); // c, never reached
    }

    #[tokio::test]
    async fn falls_back_to_child_unmodified_without_synapse_subsystem() {
        // Engine built with the synapse subsystem explicitly OFF
        // (CognitiveConfig::new() enables it by default).
        let bus = obrain_reactive::MutationBus::new();
        let scheduler =
            obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
        let mut config = CognitiveConfig::new();
        config.synapse.enabled = false;
        let engine: Arc<dyn CognitiveEngine> =
            Arc::new(CognitiveEngineBuilder::from_config(&config).build(&scheduler));

        let store: Arc<dyn GraphStore> = Arc::new(SubstrateStore::open_tempfile().unwrap());

        let mut id_col = ValueVector::with_type(LogicalType::Int64);
        id_col.push_int64(1);
        let child = Box::new(SingleChunkOperator {
            chunk: Some(DataChunk::new(vec![id_col])),
        });

        let spec = CognitiveBoostSpec {
            seeds: vec![(NodeId(1), 1.0)],
            spread_config: SpreadConfig::default(),
            causal_edge_types: vec![],
        };

        let mut op = build_cognitive_boost_operator(
            &spec,
            &engine,
            &store,
            child,
            0,
            None,
            BoostMode::Replace,
        );

        let chunk = op.next().unwrap().unwrap();
        // Unmodified child: only the original 1 column, no _cognitive_score
        // appended, since there was nothing to boost with.
        assert_eq!(chunk.column_count(), 1);
    }
}
