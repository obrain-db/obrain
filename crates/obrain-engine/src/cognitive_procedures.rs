//! Cognitive procedure implementations for `CALL obrain.cognitive.*`.
//!
//! These procedures provide query-time access to the cognitive subsystems:
//! - `CALL obrain.cognitive.spread(node_or_seeds, {hops: 3, causal_edge_types: [...], causal_boost: 2.0}) YIELD activated, energy`
//! - `CALL obrain.cognitive.distill({min_weight: 0.1}) YIELD artifact`
//! - `CALL obrain.cognitive.detect_gaps([node1, node2, ...], {top_k: 20}) YIELD kind, source, target, score, energy, reason`
//! - `CALL obrain.cognitive.synthesize([node1, node2, ...], {budget_tokens: 750}) YIELD text`
//! - `CALL obrain.cognitive.reinforce(node_a, node_b, {amount: 0.3}) YIELD source, target, amount`
//!
//! They are resolved in the planner when the `cognitive` feature is enabled.
//!
//! # Genericity
//!
//! These procedures are generic, consumer-agnostic primitives — plain
//! seeds/scope/thresholds in, scored/neutral results out. No output shape
//! here should ever be tailored to a specific downstream consumer (an LLM
//! adapter, a particular UI, ...); adaptation to a consumer's needs belongs
//! in that consumer's own code, composed on top of this API.

use obrain_adapters::plugins::AlgorithmResult;
use obrain_cognitive::CognitiveEngine;
use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};
use obrain_core::graph::GraphStore;
use std::collections::HashSet;
use std::sync::Arc;

use crate::query::plan::LogicalExpression;

/// Checks if a procedure name matches a cognitive procedure and executes it.
///
/// Returns `Some(AlgorithmResult)` if the name matched, `None` otherwise.
///
/// `store` gives read-only access to the graph for procedures that need
/// structural information (neighbors, degree, edge types) beyond what the
/// cognitive subsystem stores track — e.g. `detect_gaps`, or `spread` in
/// causal mode. It is never used to mutate the graph. Taken as an owned
/// `Arc` (cheap to clone) because some procedures build an
/// `ActivationSource` that must outlive the call (see
/// `obrain_cognitive::activation::CausalAwareActivationSource`).
pub fn try_execute_cognitive_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
    store: Arc<dyn GraphStore>,
) -> Result<Option<AlgorithmResult>> {
    match resolved_name {
        "obrain.cognitive.spread" => {
            let result = execute_spread(arguments, engine, Arc::clone(&store))?;
            Ok(Some(result))
        }
        "obrain.cognitive.distill" => {
            let result = execute_distill(arguments, engine)?;
            Ok(Some(result))
        }
        "obrain.cognitive.search" => {
            let result = execute_search(arguments, engine)?;
            Ok(Some(result))
        }
        "obrain.cognitive.detect_gaps" => {
            let result = execute_detect_gaps(arguments, engine, store.as_ref())?;
            Ok(Some(result))
        }
        "obrain.cognitive.synthesize" => {
            let result = execute_synthesize(arguments, engine, store)?;
            Ok(Some(result))
        }
        "obrain.cognitive.reinforce" => {
            let result = execute_reinforce(arguments, engine)?;
            Ok(Some(result))
        }
        // (2026-08-31) BOOTSTRAP DES SYNAPSES DEPUIS LE GRAPHE — le layer
        // cognitif « développé ». `spread()` ne propage que via les
        // synapses existantes (`SynapseActivationSource`/`CausalAware` ne
        // découvrent JAMAIS de voisins depuis les arêtes du graphe — elles
        // re-pondèrent seulement les synapses déjà créées). Sur une base
        // importée (données structurelles, zéro historique de co-activation),
        // `spread` ne sort donc jamais des seeds. Cette procédure crée une
        // synapse pour chaque arête du graphe : la base née « cognitive »,
        // la propagation devient réelle, et le coût est mesurable (le bench
        // cognitif l'utilise). Poids initial = 1.0 (purement structurel) ;
        // l'apprentissage Hebbien (co-activation réelle) le fera évoluer.
        "obrain.cognitive.bootstrap" => {
            let result = execute_bootstrap(arguments, engine, store)?;
            Ok(Some(result))
        }
        // DEC-P1 — couche de décision. `obrain.decide` est une LECTURE prouvée
        // (jamais de mutation) ; `obrain.settle` est une ÉCRITURE différée
        // (version minimale : directe, DEC-C5/SegQueue viendra en DEC-P4).
        #[cfg(feature = "decision")]
        "obrain.decide" | "decide" => {
            let result = execute_decide(arguments, engine, Arc::clone(&store))?;
            Ok(Some(result))
        }
        #[cfg(feature = "decision")]
        "obrain.settle" | "settle" => {
            let result = execute_settle(arguments, engine)?;
            Ok(Some(result))
        }
        // Level 2 — Engram introspection procedures
        "obrain.engrams.list" | "engrams.list" => {
            let result = execute_engrams_list(engine)?;
            Ok(Some(result))
        }
        "obrain.engrams.inspect" | "engrams.inspect" => {
            let result = execute_engrams_inspect(arguments, engine)?;
            Ok(Some(result))
        }
        "obrain.engrams.forget" | "engrams.forget" => {
            let result = execute_engrams_forget(arguments, engine)?;
            Ok(Some(result))
        }
        "obrain.cognitive.metrics" | "cognitive.metrics" => {
            let result = execute_cognitive_metrics(engine)?;
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

/// `CALL obrain.cognitive.spread(node_or_seeds, {hops: N, causal_edge_types: [...], causal_boost: B}) YIELD activated, energy`
///
/// Performs spreading activation from one or more source nodes and returns
/// all activated nodes with their energy values.
///
/// The first argument accepts either a single node ID (backward-compatible
/// single-seed form, each starting at energy 1.0) or a list of node IDs
/// (multi-seed, each also starting at energy 1.0 — superposed by `spread()`
/// when reachable from more than one seed).
///
/// `causal_edge_types` (optional, default empty) lists relation-type
/// strings to treat as causal — see
/// `obrain_cognitive::activation::CausalAwareActivationSource`. When empty,
/// propagation is purely associative (identical to the historical
/// synapse-only behavior). `causal_boost` (optional, default `1.0` = no-op)
/// multiplies propagation over edges classified causal.
fn execute_spread(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
    store: Arc<dyn GraphStore>,
) -> Result<AlgorithmResult> {
    use obrain_common::types::NodeId;

    // Parse arguments: first is a node ID or a list of node IDs, second is
    // an optional config map.
    let seeds: Vec<NodeId> = match arguments.first() {
        Some(LogicalExpression::Literal(Value::Int64(id))) => vec![NodeId(*id as u64)],
        Some(LogicalExpression::List(_)) => {
            extract_node_id_list(arguments.first()).ok_or_else(|| {
                Error::Internal(
                "obrain.cognitive.spread(): list argument must contain only node IDs (integers)"
                    .to_string(),
            )
            })?
        }
        Some(_) => {
            return Err(Error::Internal(
                "obrain.cognitive.spread(): first argument must be a node ID (integer) or a list of node IDs".to_string(),
            ));
        }
        None => {
            return Err(Error::Internal(
                "obrain.cognitive.spread(): requires at least 1 argument (node ID or list of node IDs)".to_string(),
            ));
        }
    };
    if seeds.is_empty() {
        return Err(Error::Internal(
            "obrain.cognitive.spread(): seed list must not be empty".to_string(),
        ));
    }

    let max_hops = extract_map_int(arguments.get(1), "hops").unwrap_or(3) as u32;
    let causal_boost = extract_map_float(arguments.get(1), "causal_boost").unwrap_or(1.0);
    let causal_edge_types =
        extract_map_string_list(arguments.get(1), "causal_edge_types").unwrap_or_default();

    let mut result = AlgorithmResult::new(vec!["activated".to_string(), "energy".to_string()]);

    // Perform spreading activation if synapse subsystem is available
    #[cfg(feature = "cognitive")]
    {
        use obrain_cognitive::activation::{
            CausalAwareActivationSource, SpreadConfig, SynapseActivationSource, spread,
        };

        if let Some(synapse_store) = engine.synapse_store() {
            let config = SpreadConfig::default()
                .with_max_hops(max_hops)
                .with_causal_boost(causal_boost);
            let sources: Vec<(NodeId, f64)> = seeds.iter().map(|&id| (id, 1.0)).collect();

            let activation_map = if causal_edge_types.is_empty() {
                let source = SynapseActivationSource::new(Arc::clone(synapse_store));
                spread(&sources, &source, &config)
            } else {
                let source = CausalAwareActivationSource::new(
                    Arc::clone(synapse_store),
                    Arc::clone(&store),
                    causal_edge_types.clone(),
                );
                spread(&sources, &source, &config)
            };

            for (node_id, energy) in activation_map {
                result.add_row(vec![Value::Int64(node_id.0 as i64), Value::Float64(energy)]);
            }
        }
    }

    let _ = (
        &seeds,
        max_hops,
        causal_boost,
        &causal_edge_types,
        engine,
        &store,
    );

    Ok(result)
}

/// `CALL obrain.cognitive.reinforce(node_a, node_b, {amount: 0.3}) YIELD source, target, amount`
///
/// Directly reinforces (creates if absent, strengthens if present) a
/// bidirectional SYNAPSE connection between two nodes, at the given
/// amount (default `0.3`). This is the write-side counterpart to
/// `spread()`: `spread()` only ever traverses existing synapse weights (see
/// `SynapseActivationSource`/`CausalAwareActivationSource`, both backed by
/// `SynapseStore::list_synapses`) — it never discovers new neighbors from
/// generic graph edges (HAS_NOTE, AFFECTS, ...) no matter what
/// `causal_edge_types` is set to (that parameter only re-weights synapses
/// that already exist as causal vs associative). Exposed for databases
/// whose synapse fabric hasn't organically accumulated yet (fresh
/// substrate stores, migrated data with structural edges but no synapse
/// history) so a caller can bootstrap a synapse before relying on `spread`
/// to surface it. Mirrors the co-activation reinforcement the cognitive
/// engine already performs internally during normal recall — this
/// procedure just exposes the same primitive (`SynapseStore::reinforce`)
/// at query time instead of only as an implicit side effect.
fn execute_reinforce(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> Result<AlgorithmResult> {
    use obrain_common::types::NodeId;

    let node_a = match arguments.first() {
        Some(LogicalExpression::Literal(Value::Int64(id))) => NodeId(*id as u64),
        Some(_) => {
            return Err(Error::Internal(
                "obrain.cognitive.reinforce(): first argument must be a node ID (integer)"
                    .to_string(),
            ));
        }
        None => {
            return Err(Error::Internal(
                "obrain.cognitive.reinforce(): requires 2 node ID arguments (source, target)"
                    .to_string(),
            ));
        }
    };
    let node_b = match arguments.get(1) {
        Some(LogicalExpression::Literal(Value::Int64(id))) => NodeId(*id as u64),
        Some(_) => {
            return Err(Error::Internal(
                "obrain.cognitive.reinforce(): second argument must be a node ID (integer)"
                    .to_string(),
            ));
        }
        None => {
            return Err(Error::Internal(
                "obrain.cognitive.reinforce(): requires 2 node ID arguments (source, target)"
                    .to_string(),
            ));
        }
    };
    let amount = extract_map_float(arguments.get(2), "amount").unwrap_or(0.3);

    let mut result = AlgorithmResult::new(vec![
        "source".to_string(),
        "target".to_string(),
        "amount".to_string(),
    ]);

    #[cfg(feature = "cognitive")]
    {
        if let Some(synapse_store) = engine.synapse_store() {
            synapse_store.reinforce(node_a, node_b, amount);
            result.add_row(vec![
                Value::Int64(node_a.0 as i64),
                Value::Int64(node_b.0 as i64),
                Value::Float64(amount),
            ]);
        }
    }

    let _ = (node_a, node_b, amount, engine);

    Ok(result)
}

/// `CALL obrain.decide({question: '...', options: ['a','b'], type: 'choice'}) YIELD node_id, decision, confidence`
///
/// **LECTURE PROUVÉE (DEC-C2 / DEC-C6 / DEC-P2).** Cette procédure n'appelle
/// AUCUNE mutation : elle lit les comptes `(n, k)` via
/// [`DecisionStore::distribution`](obrain_cognitive::DecisionStore) (lecture
/// du cache chaud DashMap), recalcule le postérieur Beta
/// (`posterior_beta(k, n)`), et émet une ligne neutre par option. Elle
/// n'appelle jamais `register`, `enregistrer`, ni aucune méthode de
/// `GraphStoreMut` — le paramètre `store` n'est même pas lu ; il n'est
/// présent que par symétrie de signature avec les autres procédures
/// cognitives. C'est ce qui autorise son inscription dans
/// `PROCEDURES_DE_LECTURE` (contrairement à `settle`, absent de cette liste).
///
/// DEC-P2 — l'estimateur : `distribution` rend déjà `posterior_beta(k, n)`
/// pour chaque option, avec `0.5` (prior Beta(1,1)) pour toute option sans
/// preuve — c'est l'équivalent « lire la décision, ou la créer avec
/// `n = k = 0` » sans jamais toucher un compteur. Si le registre n'est pas
/// câblé, on rend le même prior uniforme `0.5` (une ligne par option).
#[cfg(feature = "decision")]
fn execute_decide(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
    store: Arc<dyn GraphStore>,
) -> Result<AlgorithmResult> {
    use obrain_common::types::NodeId;

    let question = extract_map_string(arguments.first(), "question").ok_or_else(|| {
        Error::Internal("obrain.decide(): requires a 'question' string".to_string())
    })?;
    let options = extract_map_string_list(arguments.first(), "options").ok_or_else(|| {
        Error::Internal("obrain.decide(): requires an 'options' list of strings".to_string())
    })?;
    if options.is_empty() {
        return Err(Error::Internal(
            "obrain.decide(): 'options' must not be empty".to_string(),
        ));
    }

    let mut result = AlgorithmResult::new(vec![
        "node_id".to_string(),
        "decision".to_string(),
        "confidence".to_string(),
    ]);

    let Some(decision_store) = engine.decision_store() else {
        // Registre non câblé → prior uniforme Beta(1,1) = 0.5 pour chaque
        // option ; aucune lecture de compteur n'est possible.
        for option in &options {
            result.add_row(vec![
                Value::Int64(0),
                Value::from(option.as_str()),
                Value::Float64(0.5),
            ]);
        }
        // Preuve de lecture seule : `store` n'est transmis à aucun appel mutant.
        let _ = &store;
        return Ok(result);
    };

    // DEC-P2 — LECTURE PROUVÉE. On ne crée pas de compteur : `distribution`
    // lit (n, k) du cache chaud et rend Beta(1,1)=0.5 pour toute option sans
    // preuve (équivalent « créer avec n=k=0 »). Aucun appel à register /
    // enregistrer / GraphStoreMut — seulement des lectures.
    let dist = decision_store.distribution(&question, &options);
    let decision_node_id = decision_store
        .decision_node_id(&question)
        .unwrap_or(NodeId(0));
    for (i, option) in options.iter().enumerate() {
        let confidence = dist.get(i).copied().unwrap_or(0.5);
        let node_id = decision_store
            .option_node_id(&question, option)
            .unwrap_or(decision_node_id);
        result.add_row(vec![
            Value::Int64(node_id.0 as i64),
            Value::from(option.as_str()),
            Value::Float64(confidence),
        ]);
    }

    // Preuve de lecture seule : `store` n'est transmis à aucun appel mutant.
    let _ = &store;
    Ok(result)
}

/// `CALL obrain.settle({question: '...', option: '...', success: true}) YIELD question, option, success`
///
/// **ÉCRITURE (différée en DEC-P4).** Enregistre l'issue (`n += 1`, `k += 1`
/// si `success`) via [`DecisionStore::enregistrer`](obrain_cognitive::DecisionStore).
/// La version minimale écrit directement — le `DecisionStore` n'a pas encore
/// de `SegQueue` (DEC-C5) ; le drain par le consolidateur et le compteur de
/// pertes exposé (DEC-C7) arrivent en DEC-P4. Cette procédure est volontairement
/// **absente** de `PROCEDURES_DE_LECTURE` : c'est ce qui la fait refuser en
/// session `open_read_only` (DEC-C2).
#[cfg(feature = "decision")]
fn execute_settle(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> Result<AlgorithmResult> {
    let question = extract_map_string(arguments.first(), "question").ok_or_else(|| {
        Error::Internal("obrain.settle(): requires a 'question' string".to_string())
    })?;
    let option = extract_map_string(arguments.first(), "option").ok_or_else(|| {
        Error::Internal("obrain.settle(): requires an 'option' string".to_string())
    })?;
    let success = extract_map_bool(arguments.first(), "success").unwrap_or(true);
    let decision_type =
        extract_map_string(arguments.first(), "type").unwrap_or_else(|| "choice".to_string());
    let options = extract_map_string_list(arguments.first(), "options").unwrap_or_default();

    let mut result = AlgorithmResult::new(vec![
        "question".to_string(),
        "option".to_string(),
        "success".to_string(),
    ]);

    if let Some(decision_store) = engine.decision_store() {
        if !options.is_empty() {
            decision_store.register(&question, &decision_type, &options);
        }
        decision_store.enregistrer(&question, &option, success);
        result.add_row(vec![
            Value::from(question.as_str()),
            Value::from(option.as_str()),
            Value::Bool(success),
        ]);
    }

    Ok(result)
}

/// `CALL obrain.cognitive.bootstrap({edge_types: [...], weight: 1.0, direction: 'out'}) YIELD source, target, weight`
///
/// (2026-08-31) Crée une synapse par arête du graphe — le geste qui rend la
/// base « cognitive » dès l'import. `spread()` ne propage que via les
/// synapses ; sans bootstrap, un graphe importé (arêtes structurelles, zéro
/// historique de co-activation) reste muet à la propagation. Poids initial
/// `weight` (défaut 1.0) : purement structurel ; l'apprentissage Hebbien
/// (co-activation réelle via `reinforce`/recall) fera évoluer les poids.
///
/// Options :
/// - `edge_types` : liste de types d'arêtes à inclure (défaut : toutes).
/// - `direction` : `'out'` (défaut, chaque arête source→target), `'both'`
///   (ajoute aussi target→source).
///
/// Retourne le nombre de synapses créées par `count`.
fn execute_bootstrap(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
    store: Arc<dyn GraphStore>,
) -> Result<AlgorithmResult> {
    use obrain_common::types::NodeId;
    use obrain_core::graph::Direction;

    let weight = extract_map_float(arguments.first(), "weight").unwrap_or(1.0);
    let edge_types: HashSet<String> = extract_map_string_list(arguments.first(), "edge_types")
        .unwrap_or_default()
        .into_iter()
        .collect();

    let result = AlgorithmResult::new(vec![
        "source".to_string(),
        "target".to_string(),
        "weight".to_string(),
    ]);

    #[cfg(feature = "cognitive")]
    {
        if let Some(synapse_store) = engine.synapse_store() {
            // (2026-08-31) BOOTSTRAP BATCH : collecte les arêtes puis crée
            // les synapses en UNE passe (`bootstrap_batch`) au lieu de N
            // reinforces unitaires (normalize ×2 + queue par itération —
            // mesuré 407-749 ms pour 29 994 synapses). Au bootstrap les
            // poids sont uniformes : la mécanique Hebbienne est inutile.
            let mut edges: Vec<(NodeId, NodeId, f64)> = Vec::new();
            let mut seen: std::collections::HashSet<(NodeId, NodeId)> =
                std::collections::HashSet::new();
            for node in store.node_ids() {
                let targets: Vec<(NodeId, NodeId)> = store
                    .edges_from(node, Direction::Outgoing)
                    .into_iter()
                    .filter_map(|(other, edge_id)| {
                        if edge_types.is_empty()
                            || store
                                .edge_type(edge_id)
                                .is_some_and(|t| edge_types.contains(t.as_str()))
                        {
                            Some((node, other))
                        } else {
                            None
                        }
                    })
                    .collect();
                for (src, dst) in targets {
                    if seen.insert((src, dst)) {
                        edges.push((src, dst, weight));
                    }
                }
            }
            let count = synapse_store.bootstrap_batch(&edges);
            let mut counted = AlgorithmResult::new(vec!["count".to_string()]);
            counted.add_row(vec![Value::Int64(count as i64)]);
            return Ok(counted);
        }
    }

    let _ = (weight, edge_types, engine, store);
    Ok(result)
}

/// `CALL obrain.cognitive.distill({min_weight: 0.1}) YIELD artifact`
///
/// Distills the knowledge graph by extracting high-weight synaptic connections
/// as knowledge artifacts. Each artifact is a map with source, target, weight.
fn execute_distill(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> Result<AlgorithmResult> {
    let min_weight = extract_map_float(arguments.first(), "min_weight").unwrap_or(0.1);

    let mut result = AlgorithmResult::new(vec!["artifact".to_string()]);

    #[cfg(feature = "cognitive")]
    {
        use obrain_common::types::PropertyKey;
        use std::collections::BTreeMap;

        if let Some(synapse_store) = engine.synapse_store() {
            let all = synapse_store.snapshot();
            for synapse in all {
                if synapse.current_weight() >= min_weight {
                    let mut map = BTreeMap::new();
                    map.insert(
                        PropertyKey::from("source"),
                        Value::Int64(synapse.source.0 as i64),
                    );
                    map.insert(
                        PropertyKey::from("target"),
                        Value::Int64(synapse.target.0 as i64),
                    );
                    map.insert(
                        PropertyKey::from("weight"),
                        Value::Float64(synapse.current_weight()),
                    );
                    result.add_row(vec![Value::Map(Arc::new(map))]);
                }
            }
        }
    }

    let _ = (min_weight, engine);

    Ok(result)
}

/// `CALL obrain.cognitive.search({query_embedding: [...], weights: {energy: 0.3, ...}, limit: 10})`
///
/// Performs a multi-signal search combining vector similarity, energy, topology,
/// and synapse traversal. Returns scored nodes with per-signal breakdown.
fn execute_search(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> Result<AlgorithmResult> {
    use obrain_cognitive::search::{SearchConfig, SearchPipeline, SearchWeights};
    use obrain_common::types::NodeId;

    // Parse the config map (first argument)
    let config_arg = arguments.first();

    // Extract query_embedding (required)
    let query_embedding =
        extract_map_float_list(config_arg, "query_embedding").ok_or_else(|| {
            Error::Internal(
                "obrain.cognitive.search(): requires query_embedding (list of floats)".to_string(),
            )
        })?;

    if query_embedding.is_empty() {
        return Err(Error::Internal(
            "obrain.cognitive.search(): query_embedding must not be empty".to_string(),
        ));
    }

    // Extract optional parameters
    let limit = extract_map_int(config_arg, "limit").unwrap_or(10) as usize;

    // Extract weights from nested map
    let w_similarity = extract_nested_map_float(config_arg, "weights", "similarity").unwrap_or(0.3);
    let w_energy = extract_nested_map_float(config_arg, "weights", "energy").unwrap_or(0.3);
    let w_topology = extract_nested_map_float(config_arg, "weights", "topology").unwrap_or(0.3);
    let w_synapse = extract_nested_map_float(config_arg, "weights", "synapse").unwrap_or(0.1);

    let search_config = SearchConfig {
        weights: SearchWeights::new(w_similarity, w_energy, w_topology, w_synapse),
        limit,
        ..Default::default()
    };

    // For now, simulate vector candidates from query_embedding
    // In a full integration, this would delegate to the HNSW index on ObrainDB.
    // We create placeholder candidates — the actual vector search integration
    // happens at the ObrainDB level when wiring the procedure.
    let vector_candidates: Vec<(NodeId, f64)> = Vec::new();

    // Build the pipeline from cognitive engine stores
    #[allow(unused_mut)]
    let mut pipeline = SearchPipeline::new();

    #[cfg(feature = "cognitive")]
    {
        if let Some(energy_store) = engine.energy_store() {
            pipeline = pipeline.with_energy_store(Arc::clone(energy_store));
        }
        if let Some(synapse_store) = engine.synapse_store() {
            pipeline = pipeline.with_synapse_store(Arc::clone(synapse_store));
        }
    }

    // Fabric store access requires the fabric feature on obrain-cognitive
    // which is enabled via cognitive-fabric feature on obrain-engine.
    // We use cfg to conditionally access it.
    #[cfg(feature = "cognitive-fabric")]
    {
        if let Some(fabric_store) = engine.fabric_store() {
            pipeline = pipeline.with_fabric_store(Arc::clone(fabric_store));
        }
    }

    let _ = (&query_embedding, engine);

    let results = pipeline.search(&vector_candidates, &search_config);

    let mut algo_result = AlgorithmResult::new(vec![
        "node_id".to_string(),
        "score".to_string(),
        "signal_similarity".to_string(),
        "signal_energy".to_string(),
        "signal_topology".to_string(),
        "signal_synapse".to_string(),
    ]);

    for r in results {
        algo_result.add_row(vec![
            Value::Int64(r.node_id.0 as i64),
            Value::Float64(r.score),
            Value::Float64(r.signal_similarity),
            Value::Float64(r.signal_energy),
            Value::Float64(r.signal_topology),
            Value::Float64(r.signal_synapse),
        ]);
    }

    Ok(algo_result)
}

/// `CALL obrain.cognitive.detect_gaps([node1, node2, ...], {top_k: 20}) YIELD kind, node_a, node_b, score, detail`
///
/// Structural gap detection: scans the given `scope` (required — this is a
/// bounded, opt-in scan, never an implicit full-graph walk) for missing-link
/// candidates and dead/orphan nodes. Read-only: never creates or deletes
/// anything in the graph. See `obrain_cognitive::gap_detection` for the
/// underlying algorithm.
///
/// Rows:
/// - `kind = "missing_link"`: `node_a`/`node_b` are the candidate pair,
///   `score` is the combined plausibility score, `detail` breaks down the
///   evidence (neighborhood similarity, structural-twin similarity,
///   shortest known path).
/// - `kind = "dead_node"`: `node_a` is the flagged node, `node_b` is null,
///   `score` is the node's current energy, `detail` carries out_degree and
///   the reason it was flagged.
///
/// This is a generic, consumer-agnostic primitive: it returns scored
/// candidates only, never mutates the graph, and imposes no output shape
/// specific to any particular caller.
fn execute_detect_gaps(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
    store: &dyn GraphStore,
) -> Result<AlgorithmResult> {
    let scope = extract_node_id_list(arguments.first()).ok_or_else(|| {
        Error::Internal(
            "obrain.cognitive.detect_gaps(): requires an explicit scope as the first argument \
             (a list of node IDs, e.g. a community from get_stagnant_zones) — full-graph scans \
             are intentionally not supported"
                .to_string(),
        )
    })?;

    let top_k = extract_map_int(arguments.get(1), "top_k")
        .unwrap_or(20)
        .max(0) as usize;

    #[allow(unused_mut)]
    let mut result = AlgorithmResult::new(vec![
        "kind".to_string(),
        "node_a".to_string(),
        "node_b".to_string(),
        "score".to_string(),
        "detail".to_string(),
    ]);

    #[cfg(feature = "cognitive-gap-detection")]
    {
        use obrain_cognitive::gap_detection::{DeadNodeReason, GapDetectionConfig, detect_gaps};
        use obrain_common::types::PropertyKey;
        use std::collections::BTreeMap;

        let config = GapDetectionConfig::default();
        let report = detect_gaps(
            store,
            engine.energy_store().map(Arc::as_ref),
            &scope,
            &config,
        );

        for candidate in report.missing_links.iter().take(top_k) {
            let mut detail = BTreeMap::new();
            detail.insert(
                PropertyKey::from("neighborhood_similarity"),
                Value::Float64(candidate.evidence.neighborhood_similarity),
            );
            detail.insert(
                PropertyKey::from("structural_twin_similarity"),
                Value::Float64(candidate.evidence.structural_twin_similarity),
            );
            detail.insert(
                PropertyKey::from("shortest_path_hops"),
                candidate
                    .evidence
                    .shortest_path_hops
                    .map_or(Value::Null, |h| Value::Int64(h as i64)),
            );
            result.add_row(vec![
                Value::from("missing_link"),
                Value::Int64(candidate.source.0 as i64),
                Value::Int64(candidate.target.0 as i64),
                Value::Float64(candidate.score),
                Value::Map(Arc::new(detail)),
            ]);
        }

        for dead in report.dead_nodes.iter().take(top_k) {
            let mut detail = BTreeMap::new();
            detail.insert(
                PropertyKey::from("out_degree"),
                Value::Int64(dead.out_degree as i64),
            );
            detail.insert(
                PropertyKey::from("reason"),
                Value::from(match dead.reason {
                    DeadNodeReason::NoOutgoingEdges => "no_outgoing_edges",
                    DeadNodeReason::NoOutgoingCausalEdge => "no_outgoing_causal_edge",
                }),
            );
            result.add_row(vec![
                Value::from("dead_node"),
                Value::Int64(dead.node.0 as i64),
                Value::Null,
                Value::Float64(dead.energy),
                Value::Map(Arc::new(detail)),
            ]);
        }
    }

    let _ = (&scope, top_k, engine, store);

    Ok(result)
}

/// `CALL obrain.cognitive.synthesize([node1, node2, ...], {budget_tokens: 750}) YIELD text`
///
/// Dynamic contextual synthesis: chains community-bounded spreading
/// activation (`obrain_cognitive::context_loader::load_context`) with a
/// neutral, schema-less structured-text formatter (groups activated nodes
/// by label; opportunistically surfaces a `content`/`text`/`description`/...
/// property when present). Read-only — never mutates the graph.
///
/// Uses the synapse store for neighbor weighting and the fabric store for
/// community-boundary awareness when those subsystems are available on
/// `engine`; degrades gracefully (unweighted / community-agnostic
/// spreading) otherwise, rather than erroring.
///
/// No LLM-backed summarization is performed here — see
/// `obrain_cognitive::synthesis` module docs for why (no HTTP client / API
/// key / provider lock-in belongs inside the storage engine) and how to
/// plug in a custom `Summarizer` at the Rust API level. This CALL always
/// uses the built-in `NoopSummarizer` (structured text, budget-truncated).
fn execute_synthesize(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
    store: Arc<dyn GraphStore>,
) -> Result<AlgorithmResult> {
    let seeds = extract_node_id_list(arguments.first()).ok_or_else(|| {
        Error::Internal(
            "obrain.cognitive.synthesize(): requires an explicit list of seed node IDs as the first argument"
                .to_string(),
        )
    })?;
    if seeds.is_empty() {
        return Err(Error::Internal(
            "obrain.cognitive.synthesize(): seed list must not be empty".to_string(),
        ));
    }

    let budget_tokens = extract_map_int(arguments.get(1), "budget_tokens").unwrap_or(750) as u32;

    let mut result = AlgorithmResult::new(vec!["text".to_string()]);

    #[cfg(feature = "cognitive-synthesis")]
    {
        use obrain_cognitive::CognitiveKernelConfig;
        use obrain_cognitive::context_loader::{CommunityProvider, FixedTokenEstimator};
        use obrain_cognitive::synthesis::{
            FabricCommunityProvider, GraphNeighborProvider, NoopSummarizer, SynthesisConfig,
            synthesize,
        };

        let mut neighbor_provider = GraphNeighborProvider::new(Arc::clone(&store));
        if let Some(synapses) = engine.synapse_store() {
            neighbor_provider = neighbor_provider.with_synapses(Arc::clone(synapses));
        }

        /// Neutral fallback when the fabric subsystem isn't active —
        /// synthesis still works, just without community-boundary awareness.
        struct NoCommunities;
        impl CommunityProvider for NoCommunities {
            fn community_of(&self, _node_id: obrain_common::types::NodeId) -> Option<u64> {
                None
            }
            fn community_cohesion(&self, _community_id: u64) -> f64 {
                1.0
            }
        }

        let config = SynthesisConfig {
            kernel_config: CognitiveKernelConfig {
                context_budget_tokens: budget_tokens,
                ..CognitiveKernelConfig::default()
            },
            budget_tokens,
            ..SynthesisConfig::default()
        };

        let synthesis_result = if let Some(fabric) = engine.fabric_store() {
            let community_provider = FabricCommunityProvider::new(Arc::clone(fabric));
            synthesize(
                &seeds,
                store.as_ref(),
                &neighbor_provider,
                &community_provider,
                &FixedTokenEstimator(50),
                &config,
                &NoopSummarizer,
            )
        } else {
            synthesize(
                &seeds,
                store.as_ref(),
                &neighbor_provider,
                &NoCommunities,
                &FixedTokenEstimator(50),
                &config,
                &NoopSummarizer,
            )
        };

        result.add_row(vec![Value::from(synthesis_result.text.as_str())]);
    }

    let _ = (&seeds, budget_tokens, engine, &store);

    Ok(result)
}

// ============================================================================
// Level 2 — Engram introspection procedures
// ============================================================================

/// `CALL obrain.engrams.list() YIELD id, strength, valence, precision, recall_count, horizon, ensemble_size`
///
/// Lists all engrams in the store with their key metrics.
fn execute_engrams_list(engine: &Arc<dyn CognitiveEngine>) -> Result<AlgorithmResult> {
    #[cfg(feature = "cognitive-engram")]
    {
        if let Some(store) = engine.engram_store() {
            let proc_result = obrain_cognitive::procedures::engrams_list(store);
            return Ok(procedure_result_to_algorithm_result(proc_result));
        }
    }
    let _ = engine;

    // Engram subsystem not available — return empty result with correct columns
    Ok(AlgorithmResult::new(vec![
        "id".into(),
        "strength".into(),
        "valence".into(),
        "precision".into(),
        "recall_count".into(),
        "horizon".into(),
        "ensemble_size".into(),
    ]))
}

/// `CALL obrain.engrams.inspect(id) YIELD ...`
///
/// Returns full detail for a single engram including recall history.
fn execute_engrams_inspect(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> Result<AlgorithmResult> {
    let engram_id = match arguments.first() {
        Some(LogicalExpression::Literal(Value::Int64(id))) => *id as u64,
        Some(_) => {
            return Err(Error::Internal(
                "obrain.engrams.inspect(): argument must be an engram ID (integer)".to_string(),
            ));
        }
        None => {
            return Err(Error::Internal(
                "obrain.engrams.inspect(): requires 1 argument (engram ID)".to_string(),
            ));
        }
    };

    #[cfg(feature = "cognitive-engram")]
    {
        if let Some(store) = engine.engram_store() {
            let proc_result = obrain_cognitive::procedures::engrams_inspect(store, engram_id);
            return Ok(procedure_result_to_algorithm_result(proc_result));
        }
    }
    let _ = (engram_id, engine);

    Ok(AlgorithmResult::new(vec![
        "id".into(),
        "strength".into(),
        "valence".into(),
        "precision".into(),
        "recall_count".into(),
        "horizon".into(),
        "ensemble_size".into(),
        "ensemble".into(),
        "recall_history_count".into(),
        "source_episodes".into(),
        "spectral_dim".into(),
        "fsrs_stability".into(),
        "fsrs_difficulty".into(),
    ]))
}

/// `CALL obrain.engrams.forget(id) YIELD id, status, relations_removed`
///
/// RGPD right to erasure: completely removes an engram and all associated data.
fn execute_engrams_forget(
    arguments: &[LogicalExpression],
    engine: &Arc<dyn CognitiveEngine>,
) -> Result<AlgorithmResult> {
    let engram_id = match arguments.first() {
        Some(LogicalExpression::Literal(Value::Int64(id))) => *id as u64,
        Some(_) => {
            return Err(Error::Internal(
                "obrain.engrams.forget(): argument must be an engram ID (integer)".to_string(),
            ));
        }
        None => {
            return Err(Error::Internal(
                "obrain.engrams.forget(): requires 1 argument (engram ID)".to_string(),
            ));
        }
    };

    #[cfg(feature = "cognitive-engram")]
    {
        if let (Some(store), Some(vi)) = (engine.engram_store(), engine.vector_index()) {
            let proc_result =
                obrain_cognitive::procedures::engrams_forget(store, vi.as_ref(), engram_id);
            return Ok(procedure_result_to_algorithm_result(proc_result));
        }
    }
    let _ = (engram_id, engine);

    let mut result = AlgorithmResult::new(vec![
        "id".into(),
        "status".into(),
        "relations_removed".into(),
    ]);
    result.add_row(vec![
        Value::Int64(engram_id as i64),
        Value::from("not_available"),
        Value::Int64(0),
    ]);
    Ok(result)
}

/// `CALL obrain.cognitive.metrics() YIELD engrams_active, engrams_formed, ...`
///
/// Returns full cognitive metrics snapshot.
fn execute_cognitive_metrics(engine: &Arc<dyn CognitiveEngine>) -> Result<AlgorithmResult> {
    #[cfg(feature = "cognitive-engram")]
    {
        if let Some(metrics) = engine.engram_metrics() {
            let proc_result = obrain_cognitive::procedures::cognitive_metrics(metrics);
            return Ok(procedure_result_to_algorithm_result(proc_result));
        }
    }
    let _ = engine;

    Ok(AlgorithmResult::new(vec![
        "engrams_active".into(),
        "engrams_formed".into(),
        "engrams_decayed".into(),
        "engrams_recalled".into(),
        "engrams_crystallized".into(),
        "formations_attempted".into(),
        "recalls_attempted".into(),
        "recalls_successful".into(),
        "recalls_rejected".into(),
        "homeostasis_sweeps".into(),
        "prediction_errors_total".into(),
        "mean_strength".into(),
        "pheromone_entropy".into(),
        "max_pheromone_ratio".into(),
        "immune_fp_rate".into(),
        "immune_detector_count".into(),
        "avg_precision_beta".into(),
        "marks_evaluated".into(),
        "marks_applied".into(),
        "marks_suppressed".into(),
    ]))
}

/// Converts a `obrain_cognitive::procedures::ProcedureResult` to an `AlgorithmResult`.
#[cfg(feature = "cognitive-engram")]
fn procedure_result_to_algorithm_result(
    proc_result: obrain_cognitive::procedures::ProcedureResult,
) -> AlgorithmResult {
    let mut result = AlgorithmResult::new(proc_result.columns);
    for row in proc_result.rows {
        result.add_row(row);
    }
    result
}

/// Extracts an integer from a map expression argument: `{key: value}`.
fn extract_map_int(arg: Option<&LogicalExpression>, key: &str) -> Option<i64> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == key
                && let LogicalExpression::Literal(Value::Int64(n)) = v
            {
                return Some(*n);
            }
        }
    }
    None
}

/// Extracts a float from a map expression argument: `{key: value}`.
fn extract_map_float(arg: Option<&LogicalExpression>, key: &str) -> Option<f64> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == key {
                if let LogicalExpression::Literal(Value::Float64(f)) = v {
                    return Some(*f);
                }
                if let LogicalExpression::Literal(Value::Int64(n)) = v {
                    return Some(*n as f64);
                }
            }
        }
    }
    None
}

/// Extracts a string from a map expression argument: `{key: 'value'}`.
fn extract_map_string(arg: Option<&LogicalExpression>, key: &str) -> Option<String> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == key
                && let LogicalExpression::Literal(Value::String(s)) = v
            {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Extracts a boolean from a map expression argument: `{key: true}`.
fn extract_map_bool(arg: Option<&LogicalExpression>, key: &str) -> Option<bool> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == key
                && let LogicalExpression::Literal(Value::Bool(b)) = v
            {
                return Some(*b);
            }
        }
    }
    None
}

/// Extracts a top-level list of node IDs: `[1, 2, 3]` (not nested in a map —
/// used for positional scope/seed arguments like `detect_gaps`'s first arg).
fn extract_node_id_list(
    arg: Option<&LogicalExpression>,
) -> Option<Vec<obrain_common::types::NodeId>> {
    if let Some(LogicalExpression::List(items)) = arg {
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            match item {
                LogicalExpression::Literal(Value::Int64(id)) => {
                    ids.push(obrain_common::types::NodeId(*id as u64));
                }
                _ => return None,
            }
        }
        return Some(ids);
    }
    None
}

/// Extracts a list of strings from a map expression argument: `{key: ["a", "b"]}`.
fn extract_map_string_list(arg: Option<&LogicalExpression>, key: &str) -> Option<Vec<String>> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == key
                && let LogicalExpression::List(items) = v
            {
                let mut strings = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        LogicalExpression::Literal(Value::String(s)) => {
                            strings.push(s.to_string());
                        }
                        _ => return None,
                    }
                }
                return Some(strings);
            }
        }
    }
    None
}

/// Extracts a list of floats from a map expression argument: `{key: [1.0, 2.0]}`.
fn extract_map_float_list(arg: Option<&LogicalExpression>, key: &str) -> Option<Vec<f64>> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == key
                && let LogicalExpression::List(items) = v
            {
                let mut floats = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        LogicalExpression::Literal(Value::Float64(f)) => floats.push(*f),
                        LogicalExpression::Literal(Value::Int64(n)) => {
                            floats.push(*n as f64);
                        }
                        _ => return None,
                    }
                }
                return Some(floats);
            }
        }
    }
    None
}

/// Extracts a float from a nested map: `{outer_key: {inner_key: value}}`.
fn extract_nested_map_float(
    arg: Option<&LogicalExpression>,
    outer_key: &str,
    inner_key: &str,
) -> Option<f64> {
    if let Some(LogicalExpression::Map(entries)) = arg {
        for (k, v) in entries {
            if k == outer_key
                && let LogicalExpression::Map(inner_entries) = v
            {
                for (ik, iv) in inner_entries {
                    if ik == inner_key {
                        if let LogicalExpression::Literal(Value::Float64(f)) = iv {
                            return Some(*f);
                        }
                        if let LogicalExpression::Literal(Value::Int64(n)) = iv {
                            return Some(*n as f64);
                        }
                    }
                }
            }
        }
    }
    None
}
