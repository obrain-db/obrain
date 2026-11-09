//! # search_causes / search_effects — spread directionnel causal
//!
//! Forme de recherche n°5 (dernière, objectif « formes de recherche pour
//! l'IA ») : propagation d'influence AMORTIE sur le graphe dirigé, en lecture
//! pure — les CONDUCTEURS AMONT (`search_causes`, BFS inverse sur les arêtes
//! entrantes) et les CONSÉQUENCES AVAL (`search_effects`, BFS avant sur les
//! arêtes sortantes) d'un nœud.
//!
//! ## Sémantique (v1, proposition `docs/PROPOSAL-SEARCH-CAUSES-EFFECTS-2026-09-05.md`)
//!
//! Score par niveau : `score(v) = Σ_{parents u au niveau précédent, u→v}
//! score(u) × γ`, γ = 0,5 ; l'ancre vaut 1.0 ; `level` = niveau de première
//! découverte (BFS), le score total accumule les contributions multi-parents.
//! Causalité STRUCTURELLE dirigée amortie (pas d'inférence temporelle formelle
//! — extension v1.1 documentée). Frontière par niveau → terminaison garantie
//! même sur graphes cycliques ; l'ancre est exclue du résultat.
//!
//! # Surfaces
//!
//! - [`ObrainDB::search_causes`](crate::ObrainDB::search_causes) / [`ObrainDB::search_effects`](crate::ObrainDB::search_effects).
//! - `CALL obrain.search_causes(node_id, limit, depth)` /
//!   `CALL obrain.search_effects(node_id, limit, depth)`
//!   YIELD node_id, level, score — procédures GQL classées LECTURE.

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};
use obrain_core::graph::{Direction, GraphStore};
use obrain_substrate::SubstrateStore;

use crate::query::plan::LogicalExpression;

/// Cap de `limit`.
pub const SEARCH_CAUSALITY_MAX_LIMIT: usize = 10_000;
/// Défaut de `limit`.
pub const SEARCH_CAUSALITY_DEFAULT_LIMIT: usize = 20;
/// Défaut de `depth`.
pub const SEARCH_CAUSALITY_DEFAULT_DEPTH: usize = 3;
/// Plafond de `depth`.
pub const SEARCH_CAUSALITY_MAX_DEPTH: usize = 6;
/// Amortissement par niveau.
pub const SPREAD_DECAY: f64 = 0.5;

/// Une ligne de résultat `search_causes` / `search_effects`.
#[derive(Debug, Clone, PartialEq)]
pub struct CauseRow {
    /// Nœud atteint.
    pub node_id: u64,
    /// Niveau de première découverte (BFS).
    pub level: usize,
    /// Score d'influence amortie (ancre = 1.0).
    pub score: f64,
}

/// Propagation amortie depuis `anchor`. `forward=true` = effets (BFS avant),
/// `false` = causes (BFS inverse). Déterministe ; frontière bornée par niveau.
pub fn compute_causal_spread(
    substrate: &SubstrateStore,
    anchor: obrain_common::types::NodeId,
    forward: bool,
    depth: usize,
    limit: usize,
) -> Result<Vec<CauseRow>> {
    if anchor.0 == 0 {
        return Err(Error::Internal(
            "search_causes/search_effects: slot 0 (sentinelle) invalide".to_string(),
        ));
    }
    let depth = depth.clamp(1, SEARCH_CAUSALITY_MAX_DEPTH);
    let direction = if forward {
        Direction::Outgoing
    } else {
        Direction::Incoming
    };

    // (nœud → (niveau de première découverte, score total)).
    let mut level: HashMap<u64, usize> = HashMap::new();
    let mut score: HashMap<u64, f64> = HashMap::new();
    let mut frontier: Vec<u64> = Vec::new();
    score.insert(anchor.0, 1.0);
    level.insert(anchor.0, 0);
    frontier.push(anchor.0);

    let mut out: Vec<CauseRow> = Vec::new();
    for lvl in 1..=depth {
        // Contributions au niveau `lvl` depuis les nœuds du front précédent.
        let mut contrib: HashMap<u64, f64> = HashMap::new();
        for u in &frontier {
            let su = score.get(u).copied().unwrap_or(0.0);
            if su <= 0.0 {
                continue;
            }
            for (v, _eid) in substrate.edges_from(obrain_common::types::NodeId(*u), direction) {
                if v.0 == anchor.0 {
                    continue;
                }
                *contrib.entry(v.0).or_insert(0.0) += su * SPREAD_DECAY;
            }
        }
        if contrib.is_empty() {
            break;
        }
        // Niveau de première découverte + accumulation du score total.
        let mut next_frontier: Vec<u64> = Vec::new();
        for (v, c) in &contrib {
            if !level.contains_key(v) {
                level.insert(*v, lvl);
                next_frontier.push(*v);
            }
            let total = score.entry(*v).or_insert(0.0);
            *total += c;
        }
        for v in &next_frontier {
            out.push(CauseRow {
                node_id: *v,
                level: lvl,
                score: score[v],
            });
        }
        frontier = next_frontier;
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    out.truncate(limit.min(SEARCH_CAUSALITY_MAX_LIMIT));
    Ok(out)
}

impl super::ObrainDB {
    /// (2026-09-05) Conducteurs amont de `node_id` : BFS inverse amorti le
    /// long des arêtes entrantes. Nécessite un store substrate.
    pub fn search_causes(&self, node_id: u64, limit: usize, depth: usize) -> Result<Vec<CauseRow>> {
        let substrate = self.substrate_store.as_ref().ok_or_else(|| {
            Error::Internal("search_causes requires a substrate-backed database".to_string())
        })?;
        compute_causal_spread(
            substrate,
            obrain_common::types::NodeId(node_id),
            false,
            depth,
            limit,
        )
    }

    /// (2026-09-05) Conséquences aval de `node_id` : BFS avant amorti le long
    /// des arêtes sortantes. Nécessite un store substrate.
    pub fn search_effects(
        &self,
        node_id: u64,
        limit: usize,
        depth: usize,
    ) -> Result<Vec<CauseRow>> {
        let substrate = self.substrate_store.as_ref().ok_or_else(|| {
            Error::Internal("search_effects requires a substrate-backed database".to_string())
        })?;
        compute_causal_spread(
            substrate,
            obrain_common::types::NodeId(node_id),
            true,
            depth,
            limit,
        )
    }
}

/// Exécute `CALL obrain.search_causes(...)` / `CALL obrain.search_effects(...)`
/// si le nom matche. `Ok(None)` sinon.
pub fn try_execute_search_causality_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    substrate: Option<&std::sync::Arc<SubstrateStore>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    let (name, forward) = match resolved_name {
        "obrain.search_causes" | "search_causes" => ("search_causes", false),
        "obrain.search_effects" | "search_effects" => ("search_effects", true),
        _ => return Ok(None),
    };
    let result = execute_search_causality(name, forward, arguments, substrate)?;
    Ok(Some(result))
}

fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "{name}: argument {i} (node_id) must be a non-negative integer"
        ))),
    }
}

fn arg_usize(
    arguments: &[LogicalExpression],
    i: usize,
    name: &str,
    default: usize,
) -> Result<usize> {
    match arguments.get(i) {
        None => Ok(default),
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as usize),
        _ => Err(Error::Internal(format!(
            "{name}: argument {i} must be a non-negative integer"
        ))),
    }
}

fn execute_search_causality(
    name: &str,
    forward: bool,
    arguments: &[LogicalExpression],
    substrate: Option<&std::sync::Arc<SubstrateStore>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    let node_id = arg_u64(arguments, 0, name)?;
    let limit = arg_usize(arguments, 1, name, SEARCH_CAUSALITY_DEFAULT_LIMIT)?;
    let depth = arg_usize(arguments, 2, name, SEARCH_CAUSALITY_DEFAULT_DEPTH)?;

    let Some(sub) = substrate else {
        return Err(Error::Internal(format!(
            "obrain.{name} requires a substrate-backed database"
        )));
    };

    let rows = compute_causal_spread(
        sub,
        obrain_common::types::NodeId(node_id),
        forward,
        depth,
        limit,
    )?;

    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "node_id".to_string(),
        "level".to_string(),
        "score".to_string(),
    ]);
    for r in rows {
        result.add_row(vec![
            Value::Int64(r.node_id as i64),
            Value::Int64(r.level as i64),
            Value::Float64(r.score),
        ]);
    }
    Ok(result)
}
