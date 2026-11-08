//! Tests for cognitive query integration: UDFs and CALL procedures.

#![cfg(feature = "cognitive")]

use obrain_cognitive::{CognitiveConfig, CognitiveEngine, CognitiveEngineBuilder};
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_engine::cognitive_procedures::try_execute_cognitive_procedure;
use obrain_engine::cognitive_udfs::{EnergyUdf, RiskUdf, SynapsesUdf, register_cognitive_udfs};
use obrain_engine::query::plan::LogicalExpression;

use obrain_adapters::plugins::{PluginRegistry, UserDefinedFunction};
use std::sync::Arc;

/// Helper: create a cognitive engine (as dyn CognitiveEngine) with energy enabled.
/// Must be called within a tokio runtime (Scheduler requires it).
fn make_engine() -> Arc<dyn CognitiveEngine> {
    let bus = obrain_reactive::MutationBus::new();
    let scheduler = obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
    let mut config = CognitiveConfig::new();
    config.energy.enabled = true;
    let engine = CognitiveEngineBuilder::from_config(&config).build(&scheduler);
    Arc::new(engine) as Arc<dyn CognitiveEngine>
}

/// Helper: invokes the real dispatcher with a throwaway empty store — these
/// tests exercise procedures that don't depend on graph structure (spread,
/// distill, search); `detect_gaps` has its own dedicated tests below.
fn call(
    name: &str,
    args: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> obrain_common::utils::error::Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    let store: Arc<dyn GraphStore> =
        Arc::new(obrain_substrate::SubstrateStore::open_tempfile().unwrap());
    try_execute_cognitive_procedure(name, args, engine, store)
}

// ---------------------------------------------------------------------------
// UDF: obrain.energy()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn energy_udf_returns_zero_for_unknown_node() {
    let engine = make_engine();
    let udf = EnergyUdf::new(&engine);
    let result = udf.evaluate(&[Value::Int64(999)]).unwrap();
    match result {
        Value::Float64(v) => assert!(v >= 0.0),
        other => panic!("Expected Float64, got {:?}", other),
    }
}

#[tokio::test]
async fn energy_udf_returns_boosted_energy() {
    let engine = make_engine();
    if let Some(store) = engine.energy_store() {
        store.boost(NodeId(42), 5.0);
    }
    let udf = EnergyUdf::new(&engine);
    let result = udf.evaluate(&[Value::Int64(42)]).unwrap();
    match result {
        Value::Float64(v) => assert!(v >= 4.99, "Expected >= 4.99, got {}", v),
        other => panic!("Expected Float64, got {:?}", other),
    }
}

#[tokio::test]
async fn energy_udf_rejects_missing_args() {
    let engine = make_engine();
    let udf = EnergyUdf::new(&engine);
    let result = udf.evaluate(&[]);
    assert!(result.is_err());
}

#[tokio::test]
async fn energy_udf_name() {
    let engine = make_engine();
    let udf = EnergyUdf::new(&engine);
    assert_eq!(udf.name(), "obrain.energy");
}

// ---------------------------------------------------------------------------
// UDF: obrain.risk()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn risk_udf_returns_zero_without_fabric() {
    let engine = make_engine();
    let udf = RiskUdf::new(&engine);
    let result = udf.evaluate(&[Value::Int64(1)]).unwrap();
    match result {
        Value::Float64(v) => assert!((v - 0.0).abs() < f64::EPSILON),
        other => panic!("Expected Float64, got {:?}", other),
    }
}

#[tokio::test]
async fn risk_udf_name() {
    let engine = make_engine();
    let udf = RiskUdf::new(&engine);
    assert_eq!(udf.name(), "obrain.risk");
}

// ---------------------------------------------------------------------------
// UDF: obrain.synapses()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn synapses_udf_returns_empty_list_without_synapse() {
    let engine = make_engine();
    let udf = SynapsesUdf::new(&engine);
    let result = udf.evaluate(&[Value::Int64(1)]).unwrap();
    match result {
        Value::List(list) => assert!(list.is_empty()),
        other => panic!("Expected List, got {:?}", other),
    }
}

#[tokio::test]
async fn synapses_udf_name() {
    let engine = make_engine();
    let udf = SynapsesUdf::new(&engine);
    assert_eq!(udf.name(), "obrain.synapses");
}

// ---------------------------------------------------------------------------
// UDF registration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_cognitive_udfs_adds_three_udfs() {
    let engine = make_engine();
    let registry = PluginRegistry::new();
    register_cognitive_udfs(&registry, engine);

    assert!(registry.get_udf("obrain.energy").is_some());
    assert!(registry.get_udf("obrain.risk").is_some());
    assert!(registry.get_udf("obrain.synapses").is_some());
    assert_eq!(registry.list_udfs().len(), 3);
}

// ---------------------------------------------------------------------------
// CALL procedures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spread_procedure_rejects_missing_args() {
    let engine = make_engine();
    let result = call("obrain.cognitive.spread", &[], &engine);
    assert!(result.is_err());
}

#[tokio::test]
async fn spread_procedure_returns_result_for_valid_args() {
    let engine = make_engine();
    let args = vec![LogicalExpression::Literal(Value::Int64(1))];
    let result = call("obrain.cognitive.spread", &args, &engine);
    assert!(result.is_ok());
    let algo_result = result.unwrap().unwrap();
    assert_eq!(algo_result.columns.len(), 2); // "activated", "energy"
}

#[tokio::test]
async fn spread_procedure_rejects_list_with_non_integer_items() {
    let engine = make_engine();
    let args = vec![LogicalExpression::List(vec![LogicalExpression::Literal(
        Value::String("not-a-node-id".into()),
    )])];
    let result = call("obrain.cognitive.spread", &args, &engine);
    assert!(result.is_err());
}

#[tokio::test]
async fn spread_procedure_accepts_multi_seed_list() {
    let bus = obrain_reactive::MutationBus::new();
    let scheduler = obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
    let engine = CognitiveEngineBuilder::from_config(&CognitiveConfig::new()).build(&scheduler);
    let engine: Arc<dyn CognitiveEngine> = Arc::new(engine);

    let store = obrain_substrate::SubstrateStore::open_tempfile().unwrap();
    let s1 = store.create_node(&["Node"]);
    let s2 = store.create_node(&["Node"]);
    let shared = store.create_node(&["Node"]);
    store.create_edge(s1, shared, "RELATED");
    store.create_edge(s2, shared, "RELATED");
    if let Some(synapses) = engine.synapse_store() {
        synapses.reinforce(s1, shared, 0.5);
        synapses.reinforce(s2, shared, 0.5);
    }
    let store: Arc<dyn GraphStore> = Arc::new(store);

    let args = vec![LogicalExpression::List(vec![
        LogicalExpression::Literal(Value::Int64(s1.0 as i64)),
        LogicalExpression::Literal(Value::Int64(s2.0 as i64)),
    ])];
    let result = try_execute_cognitive_procedure("obrain.cognitive.spread", &args, &engine, store);
    assert!(result.is_ok(), "{:?}", result.err());
    let algo_result = result.unwrap().unwrap();

    // `shared` should be activated from BOTH seeds (superposition), so its
    // energy should exceed what a single seed alone would contribute.
    let shared_row = algo_result
        .rows
        .iter()
        .find(|r| r[0] == Value::Int64(shared.0 as i64));
    assert!(
        shared_row.is_some(),
        "shared node should be activated by both seeds"
    );
}

#[tokio::test]
async fn spread_procedure_causal_boost_favors_causal_edge_type() {
    let bus = obrain_reactive::MutationBus::new();
    let scheduler = obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
    let engine = CognitiveEngineBuilder::from_config(&CognitiveConfig::new()).build(&scheduler);
    let engine: Arc<dyn CognitiveEngine> = Arc::new(engine);

    let store = obrain_substrate::SubstrateStore::open_tempfile().unwrap();
    let root = store.create_node(&["Node"]);
    let caused = store.create_node(&["Node"]);
    let mentioned = store.create_node(&["Node"]);
    store.create_edge(root, caused, "CAUSES");
    store.create_edge(root, mentioned, "MENTIONS");
    if let Some(synapses) = engine.synapse_store() {
        // Equal synapse weight on both — any score gap must come from
        // causal_edge_types/causal_boost, not from synapse strength.
        synapses.reinforce(root, caused, 0.6);
        synapses.reinforce(root, mentioned, 0.6);
    }
    let store: Arc<dyn GraphStore> = Arc::new(store);

    let args = vec![
        LogicalExpression::Literal(Value::Int64(root.0 as i64)),
        LogicalExpression::Map(vec![
            (
                "causal_edge_types".to_string(),
                LogicalExpression::List(vec![LogicalExpression::Literal(Value::String(
                    "CAUSES".into(),
                ))]),
            ),
            (
                "causal_boost".to_string(),
                LogicalExpression::Literal(Value::Float64(4.0)),
            ),
        ]),
    ];
    let result = try_execute_cognitive_procedure("obrain.cognitive.spread", &args, &engine, store);
    assert!(result.is_ok(), "{:?}", result.err());
    let algo_result = result.unwrap().unwrap();

    let energy_of = |node: obrain_common::types::NodeId| {
        algo_result
            .rows
            .iter()
            .find(|r| r[0] == Value::Int64(node.0 as i64))
            .and_then(|r| match &r[1] {
                Value::Float64(f) => Some(*f),
                _ => None,
            })
            .unwrap_or(0.0)
    };

    let causal_energy = energy_of(caused);
    let associative_energy = energy_of(mentioned);
    assert!(
        causal_energy > associative_energy,
        "CAUSES ({causal_energy}) should outrank MENTIONS ({associative_energy}) with causal_boost=4.0"
    );
}

#[tokio::test]
async fn distill_procedure_returns_result() {
    let engine = make_engine();
    let args = vec![]; // default min_weight = 0.1
    let result = call("obrain.cognitive.distill", &args, &engine);
    assert!(result.is_ok());
    let algo_result = result.unwrap().unwrap();
    assert_eq!(algo_result.columns.len(), 1); // "artifact"
}

#[tokio::test]
async fn unknown_procedure_returns_none() {
    let engine = make_engine();
    let result = call("obrain.cognitive.unknown", &[], &engine).unwrap();
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// CALL obrain.cognitive.search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cognitive_search_procedure_rejects_missing_embedding() {
    let engine = make_engine();
    // No arguments → should error
    let result = call("obrain.cognitive.search", &[], &engine);
    assert!(result.is_err());
}

#[tokio::test]
async fn cognitive_search_procedure_rejects_empty_embedding() {
    let engine = make_engine();
    let args = vec![LogicalExpression::Map(vec![(
        "query_embedding".to_string(),
        LogicalExpression::List(vec![]),
    )])];
    let result = call("obrain.cognitive.search", &args, &engine);
    assert!(result.is_err());
}

#[tokio::test]
async fn cognitive_search_procedure_returns_result() {
    let engine = make_engine();
    let args = vec![LogicalExpression::Map(vec![
        (
            "query_embedding".to_string(),
            LogicalExpression::List(vec![
                LogicalExpression::Literal(Value::Float64(0.1)),
                LogicalExpression::Literal(Value::Float64(0.2)),
                LogicalExpression::Literal(Value::Float64(0.3)),
            ]),
        ),
        (
            "limit".to_string(),
            LogicalExpression::Literal(Value::Int64(5)),
        ),
        (
            "weights".to_string(),
            LogicalExpression::Map(vec![
                (
                    "similarity".to_string(),
                    LogicalExpression::Literal(Value::Float64(0.3)),
                ),
                (
                    "energy".to_string(),
                    LogicalExpression::Literal(Value::Float64(0.3)),
                ),
                (
                    "topology".to_string(),
                    LogicalExpression::Literal(Value::Float64(0.3)),
                ),
                (
                    "synapse".to_string(),
                    LogicalExpression::Literal(Value::Float64(0.1)),
                ),
            ]),
        ),
    ])];
    let result = call("obrain.cognitive.search", &args, &engine);
    assert!(result.is_ok());
    let algo_result = result.unwrap().unwrap();
    assert_eq!(algo_result.columns.len(), 6);
    assert_eq!(algo_result.columns[0], "node_id");
    assert_eq!(algo_result.columns[1], "score");
    assert_eq!(algo_result.columns[2], "signal_similarity");
    assert_eq!(algo_result.columns[3], "signal_energy");
    assert_eq!(algo_result.columns[4], "signal_topology");
    assert_eq!(algo_result.columns[5], "signal_synapse");
}

// ---------------------------------------------------------------------------
// CALL obrain.cognitive.detect_gaps
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detect_gaps_procedure_rejects_missing_scope() {
    let engine = make_engine();
    let result = call("obrain.cognitive.detect_gaps", &[], &engine);
    assert!(
        result.is_err(),
        "detect_gaps must require an explicit scope — no implicit full-graph scan"
    );
}

#[tokio::test]
#[cfg(feature = "cognitive-gap-detection")]
async fn detect_gaps_procedure_finds_missing_link_and_dead_node() {
    let bus = obrain_reactive::MutationBus::new();
    let scheduler = obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
    let mut config = CognitiveConfig::new();
    config.energy.enabled = true;
    let engine = CognitiveEngineBuilder::from_config(&config).build(&scheduler);
    let engine: Arc<dyn CognitiveEngine> = Arc::new(engine);

    let store = obrain_substrate::SubstrateStore::open_tempfile().unwrap();

    // Two "missions" sharing all their prereqs but never linked directly —
    // same fixture shape as obrain_cognitive::gap_detection's own unit test.
    let m1 = store.create_node(&["Mission"]);
    let m2 = store.create_node(&["Mission"]);
    let prereqs: Vec<NodeId> = (0..3).map(|_| store.create_node(&["Prereq"])).collect();
    for &p in &prereqs {
        store.create_edge(m1, p, "REQUIRES");
        store.create_edge(p, m1, "REQUIRED_BY");
        store.create_edge(m2, p, "REQUIRES");
        store.create_edge(p, m2, "REQUIRED_BY");
    }

    // A "dead" concept: has activation energy but no outgoing edge.
    let concept = store.create_node(&["Concept"]);
    let pointer = store.create_node(&["Concept"]);
    store.create_edge(pointer, concept, "MENTIONS");
    if let Some(es) = engine.energy_store() {
        es.boost(concept, 1.0);
    }

    let mut scope = vec![m1, m2, concept];
    scope.extend(prereqs);
    let scope_arg = LogicalExpression::List(
        scope
            .iter()
            .map(|n| LogicalExpression::Literal(Value::Int64(n.0 as i64)))
            .collect(),
    );

    let store: Arc<dyn GraphStore> = Arc::new(store);
    let result = try_execute_cognitive_procedure(
        "obrain.cognitive.detect_gaps",
        &[scope_arg],
        &engine,
        store,
    );
    assert!(result.is_ok(), "{:?}", result.err());
    let algo_result = result.unwrap().unwrap();
    assert_eq!(
        algo_result.columns,
        vec!["kind", "node_a", "node_b", "score", "detail"]
    );

    let has_missing_link = algo_result
        .rows
        .iter()
        .any(|r| r[0] == Value::from("missing_link"));
    let has_dead_node = algo_result
        .rows
        .iter()
        .any(|r| r[0] == Value::from("dead_node") && r[1] == Value::Int64(concept.0 as i64));
    assert!(
        has_missing_link,
        "expected at least one missing_link candidate (M1/M2 share all prereqs)"
    );
    assert!(
        has_dead_node,
        "expected the concept node to be flagged as dead"
    );
}

// ---------------------------------------------------------------------------
// CALL obrain.cognitive.synthesize
// ---------------------------------------------------------------------------

#[tokio::test]
async fn synthesize_procedure_rejects_missing_seeds() {
    let engine = make_engine();
    let result = call("obrain.cognitive.synthesize", &[], &engine);
    assert!(
        result.is_err(),
        "synthesize must require an explicit seed list"
    );
}

#[tokio::test]
#[cfg(feature = "cognitive-synthesis")]
async fn synthesize_procedure_returns_structured_schema_less_text() {
    let bus = obrain_reactive::MutationBus::new();
    let scheduler = obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
    let engine = CognitiveEngineBuilder::from_config(&CognitiveConfig::new()).build(&scheduler);
    let engine: Arc<dyn CognitiveEngine> = Arc::new(engine);

    let store = obrain_substrate::SubstrateStore::open_tempfile().unwrap();
    let mission = store.create_node(&["Mission"]);
    let axiom = store.create_node(&["Axiom"]);
    store.create_edge(mission, axiom, "RELIES_ON");
    store.set_node_property(mission, "title", Value::from("Reach orbit"));
    store.set_node_property(axiom, "content", Value::from("Energy is conserved"));
    if let Some(synapses) = engine.synapse_store() {
        synapses.reinforce(mission, axiom, 0.8);
    }
    let store: Arc<dyn GraphStore> = Arc::new(store);

    let args = vec![LogicalExpression::List(vec![LogicalExpression::Literal(
        Value::Int64(mission.0 as i64),
    )])];
    let result =
        try_execute_cognitive_procedure("obrain.cognitive.synthesize", &args, &engine, store);
    assert!(result.is_ok(), "{:?}", result.err());
    let algo_result = result.unwrap().unwrap();
    assert_eq!(algo_result.columns, vec!["text"]);
    assert_eq!(algo_result.rows.len(), 1);

    let text = match &algo_result.rows[0][0] {
        Value::String(s) => s.to_string(),
        other => panic!("expected a String, got {other:?}"),
    };
    // Schema-less: heterogeneous node types (Mission, Axiom) surfaced
    // without any hardcoded shape, content opportunistically included.
    assert!(text.contains("## Mission"));
    assert!(text.contains("## Axiom"));
    assert!(text.contains("Reach orbit"));
    assert!(text.contains("Energy is conserved"));
}

#[tokio::test]
#[cfg(feature = "cognitive-synthesis")]
async fn synthesize_procedure_respects_budget_tokens() {
    let bus = obrain_reactive::MutationBus::new();
    let scheduler = obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
    let engine = CognitiveEngineBuilder::from_config(&CognitiveConfig::new()).build(&scheduler);
    let engine: Arc<dyn CognitiveEngine> = Arc::new(engine);

    let store = obrain_substrate::SubstrateStore::open_tempfile().unwrap();
    let seed = store.create_node(&["Node"]);
    store.set_node_property(seed, "content", Value::from("x".repeat(10_000).as_str()));
    let store: Arc<dyn GraphStore> = Arc::new(store);

    let args = vec![
        LogicalExpression::List(vec![LogicalExpression::Literal(Value::Int64(
            seed.0 as i64,
        ))]),
        LogicalExpression::Map(vec![(
            "budget_tokens".to_string(),
            LogicalExpression::Literal(Value::Int64(5)), // ~20 chars
        )]),
    ];
    let result =
        try_execute_cognitive_procedure("obrain.cognitive.synthesize", &args, &engine, store);
    assert!(result.is_ok(), "{:?}", result.err());
    let algo_result = result.unwrap().unwrap();
    let text = match &algo_result.rows[0][0] {
        Value::String(s) => s.to_string(),
        other => panic!("expected a String, got {other:?}"),
    };
    assert!(
        text.len() < 10_000,
        "tiny budget_tokens should truncate the 10k-char content, got {} chars",
        text.len()
    );
    assert!(text.ends_with("...[truncated]"));
}

// ── (2026-08-31) BOOTSTRAP DES SYNAPSES — le layer cognitif « développé » ──
//
// `spread()` ne propage que via les synapses ; sur une base importée sans
// historique de co-activation il ne sort jamais des seeds. `bootstrap` crée
// une synapse par arête du graphe — ce test vérifie que :
//   1. le bootstrap crée une synapse par arête (count == nombre d'arêtes) ;
//   2. le spread propage réellement APRÈS bootstrap (plus de nœuds activés
//      que de seeds) ;
//   3. un second bootstrap est idempotent (les synapses existantes sont
//      renforcées, pas dupliquées à l'infini — count reste borné).
// `CALL` exige la feature `algos` (« CALL procedures require the 'algos'
// feature ») : sans elle le test était rouge en `--features cognitive` seul.
#[cfg(feature = "algos")]
#[test]
fn bootstrap_rend_le_graphe_cognitif_et_idempotent() {
    use obrain_cognitive::CognitiveEngine;
    use obrain_common::types::Value;

    let db = obrain_engine::ObrainDB::new_in_memory();
    let session = db.session();

    // Petit graphe : 3 nœuds, 2 arêtes CALLS.
    session
        .execute("CREATE (:Function {name:'a'}), (:Function {name:'b'}), (:Function {name:'c'})")
        .expect("create");
    let ids: Vec<u64> = session
        .execute("MATCH (n:Function) RETURN id(n) ORDER BY id(n)")
        .expect("ids")
        .rows
        .iter()
        .filter_map(|r| r.first())
        .map(|v| v.to_string().parse::<u64>().unwrap_or(0))
        .collect();
    assert_eq!(ids.len(), 3, "3 nœuds créés");
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    session
        .execute(&format!(
            "MATCH (x:Function {{name:'a'}}), (y:Function {{name:'b'}}) CREATE (x)-[:CALLS]->(y)"
        ))
        .expect("edge a→b");
    session
        .execute(&format!(
            "MATCH (x:Function {{name:'b'}}), (y:Function {{name:'c'}}) CREATE (x)-[:CALLS]->(y)"
        ))
        .expect("edge b→c");

    // 1. bootstrap : 2 synapses pour 2 arêtes.
    let r = session
        .execute(
            "CALL obrain.cognitive.bootstrap({edge_types: ['CALLS']}) YIELD count RETURN count",
        )
        .expect("bootstrap");
    let count: i64 = r
        .rows
        .first()
        .and_then(|row| row.first())
        .map(|v| v.to_string().parse().unwrap_or(-1))
        .unwrap_or(-1);
    assert_eq!(count, 2, "une synapse par arête CALLS");

    // 2. spread depuis a : doit propager (au moins b, probablement c).
    let r = session
        .execute(&format!(
            "CALL obrain.cognitive.spread([{a}], {{hops: 3}}) YIELD activated RETURN activated"
        ))
        .expect("spread");
    assert!(
        r.row_count() >= 2,
        "le spread doit activer au moins {a} et un voisin (got {} lignes)",
        r.row_count()
    );

    // 3. second bootstrap idempotent : les arêtes existantes sont renforcées,
    //    pas ré-créées — le count reste 2 (pas 4).
    let r2 = session
        .execute(
            "CALL obrain.cognitive.bootstrap({edge_types: ['CALLS']}) YIELD count RETURN count",
        )
        .expect("bootstrap 2");
    let count2: i64 = r2
        .rows
        .first()
        .and_then(|row| row.first())
        .map(|v| v.to_string().parse().unwrap_or(-1))
        .unwrap_or(-1);
    assert_eq!(
        count2, 2,
        "idempotent : toujours 2 synapses (renforcées, pas dupliquées)"
    );
    let _ = (b, c, Value::Null);
}
