//! # search_conflicts — « dissonance : les états qui se contredisent »
//!
//! Forme de recherche n°4 (objectif « formes de recherche pour l'IA », après
//! search_delta/search_surprise/search_by_role) : étant donné `since_ms`
//! (timestamp wall-clock Unix, ms), classe les `(nœud, propriété)` dont la
//! valeur OSCILLE dans la fenêtre CDC `[since_ms, now]` — le motif de
//! contradiction A→B→A→… (« le claim se dément lui-même »).
//!
//! ## Sémantique (v1, proposition `docs/PROPOSAL-SEARCH-CONFLICTS-2026-09-05.md`)
//!
//! Chaque `Update` de propriété (CDC, mono-clé before/after) est un
//! « claim » ; un **reversal** = retour à un état antérieur après un état
//! différent (`v[t] == v[t−2]` sur la séquence des valeurs distinctes
//! consécutives). Score par `(nœud, propriété)` = nombre de reversals.
//! Lecture pure de la fenêtre CDC — aucun scan store, aucune mutation.
//! Vide = aucune dissonance dans la fenêtre (documenté : les conflits sont
//! rares par nature).
//!
//! # Surfaces
//!
//! - [`ObrainDB::search_conflicts`] — méthode moteur.
//! - `CALL obrain.search_conflicts(since_ms, limit)`
//!   YIELD node_id, property, reversals — procédure GQL classée LECTURE.

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};

use crate::cdc::{CdcLog, ChangeEvent, ChangeKind};
use crate::query::plan::LogicalExpression;

/// Cap de `limit`.
pub const SEARCH_CONFLICTS_MAX_LIMIT: usize = 10_000;
/// Défaut de `limit`.
pub const SEARCH_CONFLICTS_DEFAULT_LIMIT: usize = 20;

/// Une ligne de résultat `search_conflicts`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConflictRow {
    /// Nœud dont l'état oscille.
    pub node_id: u64,
    /// Propriété concernée.
    pub property: String,
    /// Nombre de reversals (A→B→A…) dans la fenêtre.
    pub reversals: usize,
}

/// État de suivi par (nœud, propriété).
#[derive(Default)]
struct KeyTrace {
    last: Option<Value>,
    before_last: Option<Value>,
    reversals: usize,
}

/// Détecte les couples `(nœud, propriété)` dont la valeur oscille dans la
/// fenêtre CDC. O(window) + tri O(k log k), mémoire bornée par la fenêtre.
pub fn compute_search_conflicts(log: &CdcLog, since_ms: u64, limit: usize) -> Vec<ConflictRow> {
    let events: Vec<ChangeEvent> =
        log.changes_since_timestamp(since_ms, |e| matches!(e.kind, ChangeKind::Update));

    let mut traces: HashMap<(u64, String), KeyTrace> = HashMap::new();
    let mut changed: Vec<(u64, String, usize)> = Vec::new();
    for e in &events {
        let crate::cdc::EntityId::Node(nid) = e.entity_id else {
            continue; // seules les propriétés de nœuds sont suivies ici
        };
        let (Some(before), Some(after)) = (&e.before, &e.after) else {
            continue;
        };
        let (Some(key), Some(new_val)) = (after.keys().next(), after.values().next()) else {
            continue;
        };
        let old_val = before.get(key);
        // « Aucun changement » (valeur identique réécrite) → sans effet.
        if old_val == Some(new_val) {
            continue;
        }
        let trace = traces
            .entry((nid.0, key.clone()))
            .or_insert_with(KeyTrace::default);
        // Reversal : retour à l'état d'avant le changement (A→B→A).
        if trace.before_last.as_ref() == Some(new_val) {
            trace.reversals += 1;
        }
        trace.before_last = trace.last.take();
        trace.last = Some(new_val.clone());
    }
    for ((node_id, property), t) in traces {
        if t.reversals > 0 {
            changed.push((node_id, property, t.reversals));
        }
    }
    changed.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    changed
        .into_iter()
        .take(limit.min(SEARCH_CONFLICTS_MAX_LIMIT))
        .map(|(node_id, property, reversals)| ConflictRow {
            node_id,
            property,
            reversals,
        })
        .collect()
}

impl super::ObrainDB {
    /// (2026-09-05) Couples `(nœud, propriété)` dont la valeur oscille
    /// (A→B→A…) dans la fenêtre CDC `[since_ms, now]`. Nécessite le CDC armé.
    #[cfg(feature = "cdc")]
    pub fn search_conflicts(&self, since_ms: u64, limit: usize) -> Result<Vec<ConflictRow>> {
        // L'échéance de l'appelant est consultée avant le travail (voir
        // `search_surprise` pour la raison et la mesure).
        obrain_common::utils::cancel::check()?;
        Ok(compute_search_conflicts(&self.cdc_log, since_ms, limit))
    }
}

/// Exécute `CALL obrain.search_conflicts(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas une procédure search_conflicts. Erreur
/// explicite si le CDC n'est pas armé.
#[cfg(feature = "cdc")]
pub fn try_execute_search_conflicts_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    cdc: Option<&std::sync::Arc<CdcLog>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    if resolved_name != "obrain.search_conflicts" && resolved_name != "search_conflicts" {
        return Ok(None);
    }
    let result = execute_search_conflicts(arguments, cdc)?;
    Ok(Some(result))
}

#[cfg(feature = "cdc")]
fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "obrain.search_conflicts(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

#[cfg(feature = "cdc")]
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
            "obrain.search_conflicts(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

#[cfg(feature = "cdc")]
fn execute_search_conflicts(
    arguments: &[LogicalExpression],
    cdc: Option<&std::sync::Arc<CdcLog>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    let since_ms = arg_u64(arguments, 0, "since_ms")?;
    let limit = arg_usize(arguments, 1, "limit", SEARCH_CONFLICTS_DEFAULT_LIMIT)?;

    let Some(log) = cdc else {
        return Err(Error::Internal(
            "obrain.search_conflicts requires the CDC log (feature 'cdc' + persistor attaché)"
                .to_string(),
        ));
    };

    let rows = compute_search_conflicts(log, since_ms, limit);

    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "node_id".to_string(),
        "property".to_string(),
        "reversals".to_string(),
    ]);
    for r in rows {
        result.add_row(vec![
            Value::Int64(r.node_id as i64),
            Value::String(r.property.into()),
            Value::Int64(r.reversals as i64),
        ]);
    }
    Ok(result)
}
