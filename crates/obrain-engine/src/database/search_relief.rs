//! # search_relief — « le relief contextuel d'un nœud » (projection R_q)
//!
//! Feature nouvelle (2026-09-05), inspirée du tissage EdgeBasis^* (§XXIII —
//! relief vectoriel) et construite EXCLUSIVEMENT sur les signaux obrain déjà
//! existants — pas de sémantique inventée :
//!
//! | dim | signal source (existant) | définition normalisée ∈ [0,1] |
//! |---|---|---|
//! | N (nouveauté) | `search_surprise` (z signé ≥ 0) | `max(0, z)/max_z` sur la fenêtre |
//! | T (fraîcheur) | dernière activité CDC du nœud dans la fenêtre | `(t_last − t_min)/(t_max − t_min)` |
//! | B (centralité de passage) | degré (out+in) courant | `deg/max_deg` (nœuds actifs) |
//! | X (contradiction, PÉNALITÉ) | `search_conflicts` (reversals A→B→A) | `reversals/max_reversals` |
//!
//! `relief = θ_N·N + θ_T·T + θ_B·B − θ_X·X` avec les θ par défaut
//! `(0,35 ; 0,15 ; 0,25 ; 0,25)` (dépendent de la question — version à θ
//! paramétrables en phase 2).
//!
//! Dimensions du tissage SANS signal honnête aujourd'hui (E preuve, C levier
//! causal, S rôle, G but, A actionnabilité, U incertitude, D dépendance) :
//! NON calculées, listées dans la doc — jamais une valeur inventée (NULL
//! explicite, anti-déformation).
//!
//! Lecture pure : fenêtre CDC (bornée `CDC_RELOAD_CAP`) + degrés substrate.
//! Surfaces : [`ObrainDB::search_relief`] et
//! `CALL obrain.relief(since_ms, limit) YIELD node_id, relief, novelty,
//! recency, centrality, conflict` — classée LECTURE.

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};
use obrain_substrate::SubstrateStore;

use crate::cdc::CdcLog;
use crate::query::plan::LogicalExpression;

use super::search_conflicts::compute_search_conflicts;
use super::search_surprise::compute_search_surprise;

/// Poids par défaut : `[θ_N, θ_T, θ_B, θ_X]` (X = pénalité).
pub const RELIEF_DEFAULT_THETA: [f64; 4] = [0.35, 0.15, 0.25, 0.25];
/// Cap de `limit`.
pub const RELIEF_MAX_LIMIT: usize = 10_000;
/// Défaut de `limit`.
pub const RELIEF_DEFAULT_LIMIT: usize = 20;

/// Une ligne de résultat `relief` (dimensions normalisées ∈ [0,1]).
#[derive(Debug, Clone, PartialEq)]
pub struct ReliefRow {
    /// Nœud (slot substrate).
    pub node_id: u64,
    /// `θ_N·N + θ_T·T + θ_B·B − θ_X·X`.
    pub relief: f64,
    /// Nouveauté (surprise ≥ 0 normalisée).
    pub novelty: f64,
    /// Fraîcheur (dernière activité dans la fenêtre, normalisée).
    pub recency: f64,
    /// Centralité de passage (degré normalisé).
    pub centrality: f64,
    /// Contradiction (reversals normalisés — pénalité).
    pub conflict: f64,
}

/// Calcule le relief contextuel des nœuds actifs dans `[since_ms, now]`.
///
/// Coût : une fenêtre CDC (≤ `CDC_RELOAD_CAP`) + O(k log k) — réutilise les
/// calculs de `search_surprise` et `search_conflicts`. Aucune mutation.
pub fn compute_search_relief(
    log: &CdcLog,
    substrate: &SubstrateStore,
    since_ms: u64,
    limit: usize,
) -> Vec<ReliefRow> {
    let theta = RELIEF_DEFAULT_THETA;

    // ── N : nouveauté — surprise ≥ 0 normalisée (sur la fenêtre complète).
    let surprise = compute_search_surprise(log, substrate, since_ms, usize::MAX, false);
    let zmax = surprise
        .iter()
        .map(|r| r.surprise.max(0.0))
        .fold(0.0_f64, f64::max);

    // ── X : contradiction — reversals cumulés par nœud.
    let conflicts = compute_search_conflicts(log, since_ms, usize::MAX);
    let mut x_rev: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for c in conflicts {
        if c.reversals > 0 {
            *x_rev.entry(c.node_id).or_insert(0) += c.reversals as u64;
        }
    }
    let xmax = x_rev.values().copied().max().unwrap_or(0);

    // ── T : fraîcheur — dernière activité (événements CDC, arêtes via leurs
    // extrémités) et B : degré courant.
    let mut last_ts: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    let mut t_min: u64 = u64::MAX;
    let mut t_max: u64 = 0;
    let mut active: Vec<u64> = Vec::new();
    {
        use std::collections::HashSet;
        let mut seen: HashSet<u64> = HashSet::new();
        let mut note = |n: u64, ts: u64, last: &mut std::collections::HashMap<u64, u64>,
                        seen: &mut HashSet<u64>, act: &mut Vec<u64>| {
            let e = last.entry(n).or_insert(0);
            if ts > *e {
                *e = ts;
            }
            if seen.insert(n) {
                act.push(n);
            }
        };
        for e in log.changes_since_timestamp(since_ms, |_| true) {
            let ts = e.timestamp;
            if ts < t_min {
                t_min = ts;
            }
            if ts > t_max {
                t_max = ts;
            }
            match e.entity_id {
                crate::cdc::EntityId::Node(nid) => {
                    note(nid.0, ts, &mut last_ts, &mut seen, &mut active);
                }
                crate::cdc::EntityId::Edge(eid) => {
                    if let Some((src, dst)) = substrate.edge_endpoints_by_id(eid) {
                        note(src.0, ts, &mut last_ts, &mut seen, &mut active);
                        note(dst.0, ts, &mut last_ts, &mut seen, &mut active);
                    }
                }
            }
        }
    }
    if active.is_empty() {
        return Vec::new();
    }
    let span = t_max.saturating_sub(t_min);

    // ── B : degré courant (out+in) — normalisé sur les actifs.
    let mut deg_max: u64 = 0;
    let mut degs: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for &n in &active {
        let d = u64::from(substrate.out_degree(obrain_common::types::NodeId(n)))
            + u64::from(substrate.in_degree(obrain_common::types::NodeId(n)));
        degs.insert(n, d);
        deg_max = deg_max.max(d);
    }

    let z_of: std::collections::HashMap<u64, f64> =
        surprise.iter().map(|r| (r.node_id, r.surprise)).collect();

    let mut rows: Vec<ReliefRow> = Vec::with_capacity(active.len());
    for &n in &active {
        let zn = z_of.get(&n).copied().unwrap_or(0.0);
        let novelty = if zmax > 0.0 {
            (zn.max(0.0) / zmax).min(1.0)
        } else {
            0.0
        };
        let t_last = last_ts.get(&n).copied().unwrap_or(0);
        let recency = if span > 0 {
            ((t_last.saturating_sub(t_min)) as f64 / span as f64).min(1.0)
        } else {
            0.0
        };
        let centrality = if deg_max > 0 {
            degs.get(&n).copied().unwrap_or(0) as f64 / deg_max as f64
        } else {
            0.0
        };
        let conflict = if xmax > 0 {
            (x_rev.get(&n).copied().unwrap_or(0) as f64 / xmax as f64).min(1.0)
        } else {
            0.0
        };
        let relief = theta[0] * novelty + theta[1] * recency + theta[2] * centrality
            - theta[3] * conflict;
        rows.push(ReliefRow {
            node_id: n,
            relief,
            novelty,
            recency,
            centrality,
            conflict,
        });
    }
    rows.sort_by(|a, b| {
        b.relief
            .partial_cmp(&a.relief)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    rows.truncate(limit.min(RELIEF_MAX_LIMIT));
    rows
}

impl super::ObrainDB {
    /// (2026-09-05) Relief contextuel des nœuds actifs dans `[since_ms, now]`
    /// — projection `θ_N·N + θ_T·T + θ_B·B − θ_X·X` sur les signaux existants.
    /// Nécessite un store substrate + CDC armé.
    #[cfg(feature = "cdc")]
    pub fn search_relief(&self, since_ms: u64, limit: usize) -> Result<Vec<ReliefRow>> {
        // L'échéance de l'appelant est consultée avant le travail (voir
        // `search_surprise` pour la raison et la mesure).
        obrain_common::utils::cancel::check()?;
        let substrate = self.substrate_store.as_ref().ok_or_else(|| {
            Error::Internal("search_relief requires a substrate-backed database".to_string())
        })?;
        Ok(compute_search_relief(&self.cdc_log, substrate, since_ms, limit))
    }
}

/// Exécute `CALL obrain.relief(...)` si le nom matche.
#[cfg(feature = "cdc")]
pub fn try_execute_relief_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
    cdc: Option<&Arc<CdcLog>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    if resolved_name != "obrain.relief" && resolved_name != "relief" {
        return Ok(None);
    }
    let result = execute_relief(arguments, substrate, cdc)?;
    Ok(Some(result))
}

#[cfg(feature = "cdc")]
fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "obrain.relief(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

#[cfg(feature = "cdc")]
fn arg_usize(arguments: &[LogicalExpression], i: usize, name: &str, default: usize) -> Result<usize> {
    match arguments.get(i) {
        None => Ok(default),
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as usize),
        _ => Err(Error::Internal(format!(
            "obrain.relief(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

#[cfg(feature = "cdc")]
fn execute_relief(
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
    cdc: Option<&Arc<CdcLog>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    let since_ms = arg_u64(arguments, 0, "since_ms")?;
    let limit = arg_usize(arguments, 1, "limit", RELIEF_DEFAULT_LIMIT)?;
    let Some(log) = cdc else {
        return Err(Error::Internal(
            "obrain.relief requires the CDC log (feature 'cdc' + persistor attaché)".to_string(),
        ));
    };
    let Some(sub) = substrate else {
        return Err(Error::Internal(
            "obrain.relief requires a substrate-backed database".to_string(),
        ));
    };
    let rows = compute_search_relief(log, sub, since_ms, limit);
    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "node_id".to_string(),
        "relief".to_string(),
        "novelty".to_string(),
        "recency".to_string(),
        "centrality".to_string(),
        "conflict".to_string(),
    ]);
    for r in rows {
        result.add_row(vec![
            Value::Int64(r.node_id as i64),
            Value::Float64(r.relief),
            Value::Float64(r.novelty),
            Value::Float64(r.recency),
            Value::Float64(r.centrality),
            Value::Float64(r.conflict),
        ]);
    }
    Ok(result)
}
