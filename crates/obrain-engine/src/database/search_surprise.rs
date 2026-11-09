//! # search_surprise — « ce qui s'active contre les attentes » (surprise d'activité)
//!
//! Forme de recherche n°2 (objectif « formes de recherche pour l'IA », après
//! `search_delta`) : étant donné un instant de référence `since_ms` (timestamp
//! wall-clock Unix, ms), classe les nœuds dont l'ACTIVITÉ récente s'écarte le
//! plus de ce qu'une attente apprise prédit.
//!
//! ## Sémantique (v1, pivot documenté dans `docs/PROPOSAL-SEARCH-SURPRISE-2026-09-05.md`)
//!
//! - **activité réelle** `a(n)` = nombre d'événements CDC touchant le nœud dans
//!   `[since_ms, now]` (create/update du nœud ; chaque arête compte pour ses
//!   deux extrémités — résolues via `SubstrateStore::edge_endpoints_by_id`) ;
//! - **attente** `e(n)` = part d'activité proportionnelle au degré courant du
//!   nœud dans le graphe (modèle nul : l'activité suit la connectivité) —
//!   `e(n) = total_window × (deg(n)+1) / Σ_{actifs}(deg+1)` (lissage +1) ;
//! - **surprise signée** `s(n) = (a(n) − e(n)) / sqrt(e(n)+1)` (z approx. :
//!   positif = s'active PLUS que son degré ne le prédit, négatif = s'efface).
//!
//! `search_delta` liste TOUS les changements ; `search_surprise` en isole ceux
//! qui contredisent l'attente — complémentaire et déterministe. Lecture pure,
//! source : fenêtre CDC persistée (bornée à `CDC_RELOAD_CAP`), + degrés et
//! extrémités substrate. Base non-substrate ou CDC non armé → erreur explicite.
//!
//! # Surfaces
//!
//! - [`ObrainDB::search_surprise`](crate::ObrainDB::search_surprise) — méthode moteur.
//! - `CALL obrain.search_surprise(since_ms, limit, signed)`
//!   YIELD node_id, activity, degree, expected, surprise — procédure GQL
//!   classée LECTURE (exécutable en read-only). `limit` (défaut 20, cap
//!   10 000) ; `signed=true` → tri par surprise décroissante (les « activés
//!   plus que prédit » d'abord), défaut → tri par |surprise|.

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};
use obrain_substrate::SubstrateStore;

use crate::cdc::{CdcLog, ChangeEvent};
use crate::query::plan::LogicalExpression;

/// Cap de `limit`.
pub const SEARCH_SURPRISE_MAX_LIMIT: usize = 10_000;
/// Défaut de `limit`.
pub const SEARCH_SURPRISE_DEFAULT_LIMIT: usize = 20;

/// Une ligne de résultat `search_surprise`.
#[derive(Debug, Clone, PartialEq)]
pub struct SurpriseRow {
    /// Nœud (slot substrate).
    pub node_id: u64,
    /// Activité réelle dans la fenêtre (événements CDC le touchant).
    pub activity: u64,
    /// Degré sortant courant du nœud (attente proportionnelle).
    pub degree: usize,
    /// Attente du modèle nul.
    pub expected: f64,
    /// Surprise signée `(a − e)/sqrt(e+1)`.
    pub surprise: f64,
}

/// Calcule le classement de surprise sur la fenêtre CDC `[since_ms, now]`.
///
/// O(window + k log k) : parcours des événements de la fenêtre (≤
/// `CDC_RELOAD_CAP`), agrégation par nœud, puis tri. Aucune mutation.
pub fn compute_search_surprise(
    log: &CdcLog,
    substrate: &SubstrateStore,
    since_ms: u64,
    limit: usize,
    signed: bool,
) -> Vec<SurpriseRow> {
    let events: Vec<ChangeEvent> = log.changes_since_timestamp(since_ms, |_| true);

    // Activité par nœud (les arêtes comptent pour leurs deux extrémités).
    let counts: Vec<(u64, u64)> = {
        use std::collections::HashMap;
        let mut map: HashMap<u64, u64> = HashMap::new();
        for e in &events {
            match e.entity_id {
                crate::cdc::EntityId::Node(nid) => {
                    *map.entry(nid.0).or_insert(0) += 1;
                }
                crate::cdc::EntityId::Edge(eid) => {
                    if let Some((src, dst)) = substrate.edge_endpoints_by_id(eid) {
                        *map.entry(src.0).or_insert(0) += 1;
                        *map.entry(dst.0).or_insert(0) += 1;
                    }
                }
            }
        }
        map.into_iter().collect()
    };
    if counts.is_empty() {
        return Vec::new();
    }

    // Degré de DÉBUT de fenêtre par nœud actif : le degré courant inclut déjà
    // les arêtes de la fenêtre — l'utiliser rendrait l'attente
    // auto-réalisatrice (une rafale d'arêtes gonflerait le degré donc
    // l'attente). On retranche les arêtes de la fenêtre dont le nœud était
    // l'extrémité.
    let mut deg_start_of: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    {
        use std::collections::HashMap as Hm;
        let mut out_window: Hm<u64, u64> = Hm::new();
        let mut in_window: Hm<u64, u64> = Hm::new();
        for e in &events {
            if let crate::cdc::EntityId::Edge(eid) = e.entity_id
                && let Some((src, dst)) = substrate.edge_endpoints_by_id(eid)
            {
                *out_window.entry(src.0).or_insert(0) += 1;
                *in_window.entry(dst.0).or_insert(0) += 1;
            }
        }
        for (n, _) in &counts {
            let nid = obrain_common::types::NodeId(*n);
            let deg_now =
                u64::from(substrate.out_degree(nid)) + u64::from(substrate.in_degree(nid));
            let out = out_window.get(n).copied().unwrap_or(0);
            let inn = in_window.get(n).copied().unwrap_or(0);
            deg_start_of.insert(*n, deg_now.saturating_sub(out + inn));
        }
    }

    let total_window: f64 = counts.iter().map(|(_, c)| *c as f64).sum();
    let denom: f64 = counts
        .iter()
        .map(|(n, _)| deg_start_of.get(n).copied().unwrap_or(0) as f64 + 1.0)
        .sum();

    let mut rows: Vec<SurpriseRow> = counts
        .into_iter()
        .map(|(n, activity)| {
            let degree_start = deg_start_of.get(&n).copied().unwrap_or(0) as usize;
            let degree_now = (u64::from(substrate.out_degree(obrain_common::types::NodeId(n)))
                + u64::from(substrate.in_degree(obrain_common::types::NodeId(n))))
                as usize;
            let expected = if denom > 0.0 {
                total_window * (degree_start as f64 + 1.0) / denom
            } else {
                0.0
            };
            let surprise = (activity as f64 - expected) / (expected + 1.0).sqrt();
            SurpriseRow {
                node_id: n,
                activity,
                degree: degree_now,
                expected,
                surprise,
            }
        })
        .collect();

    if signed {
        rows.sort_by(|a, b| {
            b.surprise
                .partial_cmp(&a.surprise)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    } else {
        rows.sort_by(|a, b| {
            b.surprise
                .abs()
                .partial_cmp(&a.surprise.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    rows.truncate(limit.min(SEARCH_SURPRISE_MAX_LIMIT));
    rows
}

impl super::ObrainDB {
    /// (2026-09-05) Nœuds dont l'activité récente (fenêtre CDC `[since_ms,
    /// now]`) s'écarte le plus de l'attente proportionnelle au degré.
    ///
    /// Nécessite un store substrate (degrés + extrémités des arêtes) et le CDC
    /// armé — erreur explicite sinon.
    #[cfg(feature = "cdc")]
    pub fn search_surprise(
        &self,
        since_ms: u64,
        limit: usize,
        signed: bool,
    ) -> Result<Vec<SurpriseRow>> {
        // ── L'ÉCHÉANCE DE L'APPELANT EST CONSULTÉE AVANT LE TRAVAIL ────────
        // Défaut mesuré : ces lectures (fenêtre CDC + agrégation) tournaient
        // hors de toute portée d'annulation — l'échéance du pont n'était armée
        // que pour les requêtes GQL. Une consolidation partie ne pouvait plus
        // être arrêtée. Le contrôle coûte une lecture de thread-local ; il est
        // posé AVANT le travail lourd (une échéance déjà dépassée ne doit pas
        // payer le travail qu'elle interdit) et APRÈS (pour que l'appelant
        // suivant ne reprenne pas un budget déjà consommé).
        obrain_common::utils::cancel::check()?;

        let substrate = self.substrate_store.as_ref().ok_or_else(|| {
            Error::Internal("search_surprise requires a substrate-backed database".to_string())
        })?;
        Ok(compute_search_surprise(
            &self.cdc_log,
            substrate,
            since_ms,
            limit,
            signed,
        ))
    }
}

/// Exécute `CALL obrain.search_surprise(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas une procédure search_surprise. Erreur
/// explicite si le CDC n'est pas armé ou la base non substrate.
#[cfg(feature = "cdc")]
pub fn try_execute_search_surprise_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
    cdc: Option<&Arc<CdcLog>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    if resolved_name != "obrain.search_surprise" && resolved_name != "search_surprise" {
        return Ok(None);
    }
    let result = execute_search_surprise(arguments, substrate, cdc)?;
    Ok(Some(result))
}

#[cfg(feature = "cdc")]
fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "obrain.search_surprise(): argument {i} ({name}) must be a non-negative integer"
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
            "obrain.search_surprise(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

#[cfg(feature = "cdc")]
fn arg_bool(arguments: &[LogicalExpression], i: usize, name: &str, default: bool) -> Result<bool> {
    match arguments.get(i) {
        None => Ok(default),
        Some(LogicalExpression::Literal(Value::Bool(b))) => Ok(*b),
        _ => Err(Error::Internal(format!(
            "obrain.search_surprise(): argument {i} ({name}) must be a boolean"
        ))),
    }
}

#[cfg(feature = "cdc")]
fn execute_search_surprise(
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
    cdc: Option<&Arc<CdcLog>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    let since_ms = arg_u64(arguments, 0, "since_ms")?;
    let limit = arg_usize(arguments, 1, "limit", SEARCH_SURPRISE_DEFAULT_LIMIT)?;
    let signed = arg_bool(arguments, 2, "signed", false)?;

    let Some(log) = cdc else {
        return Err(Error::Internal(
            "obrain.search_surprise requires the CDC log (feature 'cdc' + persistor attaché)"
                .to_string(),
        ));
    };
    let Some(sub) = substrate else {
        return Err(Error::Internal(
            "obrain.search_surprise requires a substrate-backed database".to_string(),
        ));
    };

    let rows = compute_search_surprise(log, sub, since_ms, limit, signed);

    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "node_id".to_string(),
        "activity".to_string(),
        "degree".to_string(),
        "expected".to_string(),
        "surprise".to_string(),
    ]);
    for r in rows {
        result.add_row(vec![
            Value::Int64(r.node_id as i64),
            Value::Int64(r.activity as i64),
            Value::Int64(r.degree as i64),
            Value::Float64(r.expected),
            Value::Float64(r.surprise),
        ]);
    }
    Ok(result)
}
