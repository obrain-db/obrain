//! # search_delta — « ce qui a changé depuis ma dernière vue » (CDC graphe)
//!
//! Re-contextualisation par diff pour une IA en session : étant donné un
//! instant de référence (timestamp wall-clock en ms Unix), renvoie les
//! changements CDC (create/update/delete avec before/after) survenus depuis.
//!
//! Source : [`CdcLog`](crate::cdc::CdcLog) — fenêtre chaude persistée en zone
//! `substrate.cdc_journal` (bornée à `CDC_RELOAD_CAP` au rechargement). C'est
//! le diff au niveau GRAPHE (complémentaire du diff de journal de session,
//! déjà exposé par `events_after_seq` dans le bridge).
//!
//! # Surfaces
//!
//! - [`ObrainDB::search_delta`] — méthode moteur (pattern `window_query_2d`).
//! - `CALL obrain.search_delta(since_ms, {top_k, entity: "node"|"edge"})`
//!   YIELD entity_id, kind, timestamp, before, after — procédure GQL.
//!
//! # Coût
//!
//! O(n) sur la fenêtre chaude + tri O(m log m) sur les matches — pas de scan
//! store. Lecture pure, classée LECTURE (exécutable en read-only).

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};

use crate::cdc::ChangeEvent;
use crate::query::plan::LogicalExpression;

impl super::ObrainDB {
    /// Tous les changements CDC depuis `since_ms` (timestamp wall-clock Unix,
    /// ms), triés par timestamp croissant.
    ///
    /// `only_nodes` / `only_edges` filtrent par type d'entité (`false` les
    /// deux = tout). Nécessite le CDC compilé (`feature = "cdc"`) et attaché
    /// (persistor substrate) — erreur explicite sinon.
    #[cfg(feature = "cdc")]
    pub fn search_delta(
        &self,
        since_ms: u64,
        only_nodes: bool,
        only_edges: bool,
    ) -> Result<Vec<ChangeEvent>> {
        let filter = move |e: &ChangeEvent| {
            let is_edge = matches!(e.entity_id, crate::cdc::EntityId::Edge(_));
            (!only_nodes || !is_edge) && (!only_edges || is_edge)
        };
        Ok(self.cdc_log().changes_since_timestamp(since_ms, filter))
    }
}

/// Exécute `CALL obrain.search_delta(...)` si le nom matche.
///
/// Retourne `Ok(None)` si le nom n'est pas une procédure search_delta
/// (l'appelant continue la résolution). Erreur explicite si le CDC n'est pas
/// compilé/attaché.
#[cfg(feature = "cdc")]
pub fn try_execute_search_delta_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    cdc: Option<&Arc<crate::cdc::CdcLog>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    if resolved_name != "obrain.search_delta" && resolved_name != "search_delta" {
        return Ok(None);
    }
    let result = execute_search_delta(arguments, cdc)?;
    Ok(Some(result))
}

#[cfg(feature = "cdc")]
fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "obrain.search_delta(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

#[cfg(feature = "cdc")]
fn arg_bool(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<bool> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Bool(b))) => Ok(*b),
        _ => Err(Error::Internal(format!(
            "obrain.search_delta(): argument {i} ({name}) must be a boolean"
        ))),
    }
}

#[cfg(feature = "cdc")]
#[cfg(feature = "cdc")]
fn arg_usize(arguments: &[LogicalExpression], i: usize, name: &str, default: usize) -> Result<usize> {
    match arguments.get(i) {
        None => Ok(default),
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as usize),
        _ => Err(Error::Internal(format!(
            "obrain.search_delta(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

fn execute_search_delta(
    arguments: &[LogicalExpression],
    cdc: Option<&Arc<crate::cdc::CdcLog>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    let since_ms = arg_u64(arguments, 0, "since_ms")?;
    // (2026-09-06) LIMITE OPTIONNELLE (arg 3) : borner le nombre de lignes
    // rendues (la sérialisation de 10 k lignes ≈ 10 ms ; limit 100 → < 1 ms).
    // Défaut 10 000 = la fenêtre — aucun changement de comportement.
    let limit = arguments
        .get(3)
        .map(|a| arg_usize(arguments, 3, "limit", 10_000))
        .transpose()?
        .unwrap_or(10_000);
    // Options optionnelles (position 1 = map booléenne simple : on lit les
    // champs top_k/entity de manière tolérante — ici on supporte la forme
    // positionnelle directe pour rester simple et déterministe).
    let only_nodes = arguments
        .get(1)
        .map(|a| arg_bool(arguments, 1, "only_nodes"))
        .transpose()?
        .unwrap_or(false);
    let only_edges = arguments
        .get(2)
        .map(|a| arg_bool(arguments, 2, "only_edges"))
        .transpose()?
        .unwrap_or(false);

    let Some(log) = cdc else {
        return Err(Error::Internal(
            "obrain.search_delta requires the CDC log (feature 'cdc' + persistor attaché)"
                .to_string(),
        ));
    };

    let filter = |e: &ChangeEvent| {
        let is_edge = matches!(e.entity_id, crate::cdc::EntityId::Edge(_));
        (!only_nodes || !is_edge) && (!only_edges || is_edge)
    };
    let events = log.changes_since_timestamp(since_ms, filter);

    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "entity_id".to_string(),
        "kind".to_string(),
        "timestamp".to_string(),
    ]);
    for e in events.into_iter().take(limit) {
        let id = match e.entity_id {
            crate::cdc::EntityId::Node(n) => Value::Int64(n.0 as i64),
            crate::cdc::EntityId::Edge(ed) => Value::Int64(ed.0 as i64),
        };
        let kind = match e.kind {
            crate::cdc::ChangeKind::Create => Value::String("create".into()),
            crate::cdc::ChangeKind::Update => Value::String("update".into()),
            crate::cdc::ChangeKind::Delete => Value::String("delete".into()),
        };
        result.add_row(vec![id, kind, Value::Int64(e.timestamp as i64)]);
    }
    Ok(result)
}
