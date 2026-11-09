//! Procédure substrate-native `CALL obrain.window_2d(...)` — RFC
//! zorder-superposition, capacité C5 (fenêtres 2D en syntaxe GQL).
//!
//! Expose la fenêtre 2D par communauté sur le plan
//! `(centrality_cached × out-degree)` — jusqu'alors réservée à l'API
//! moteur/binding (`ObrainDB::window_query_2d[_sorted]`, NAPI
//! `windowQuery2d`) — comme procédure CALL du langage :
//!
//! ```text
//! CALL obrain.window_2d(community_id, c_lo, c_hi, d_lo, d_hi) YIELD node_id
//! ```
//!
//! Et sa variante cognitive **C3** (RFC §17, « fenêtre cognitive ») :
//!
//! ```text
//! CALL obrain.window_2d_energy(community_id, c_lo, c_hi, d_lo, d_hi,
//!                              energy_min_q15) YIELD node_id, energy
//! ```
//!
//! qui ne retient de la fenêtre que les nœuds dont l'énergie (colonne
//! inline `NodeRecord.energy`, Q1.15) est ≥ `energy_min_q15` — « le
//! sous-graphe vivant d'une communauté », sélection par bande d'énergie
//! pendant l'inférence (C3). La valeur `energy` rendue est l'énergie
//! réelle (décodée en [0, 1]).
//!
//! Sémantique strictement identique à [`ObrainDB::window_query_2d_sorted`](crate::ObrainDB::window_query_2d_sorted) :
//! voie accélérée zruns quand le layout est trié (`HILBERT_SORTED` + grille
//! `substrate.zgrid` présentes), **repli scan exact automatique** sinon (layout
//! non trié, grille absente, fenêtre large > ~⅓ de la communauté) — jamais de
//! faux positif, jamais de résultat divergent du scan.
//!
//! La résolution a lieu au plan-time (comme les procédures cognitives), ce qui
//! exige que le Planner détienne le handle substrate typé — câblage
//! `with_substrate_store` (ObrainDB → Session → QueryProcessor → Planner),
//! miroir exact de `with_cognitive_engine`.
//!
//! # Genericity
//!
//! Même règle que les procédures cognitives : primitives génériques, neutres —
//! les ids de nœuds sortent, l'adaptation à un consommateur vit chez lui.

use obrain_adapters::plugins::AlgorithmResult;
use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};

use crate::query::plan::LogicalExpression;

/// Exécute une procédure window si `resolved_name` est connue.
///
/// Retourne `Ok(None)` si le nom ne matche aucune procédure window (l'appelant
/// continue la résolution). Retourne `Ok(Some(result))` en cas de succès.
/// Erreur explicite si la procédure est reconnue mais que le store n'est pas
/// substrate (`substrate` = `None`).
pub fn try_execute_window_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    substrate: Option<&std::sync::Arc<obrain_substrate::SubstrateStore>>,
) -> Result<Option<AlgorithmResult>> {
    match resolved_name {
        "obrain.window_2d" | "window_2d" => {
            let result = execute_window_2d(arguments, substrate)?;
            Ok(Some(result))
        }
        "obrain.window_2d_energy" | "window_2d_energy" => {
            let result = execute_window_2d_energy(arguments, substrate)?;
            Ok(Some(result))
        }
        _ => Ok(None),
    }
}

/// Parse un argument entier non-négatif.
fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "obrain.window_2d(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

/// `CALL obrain.window_2d(community_id, c_lo, c_hi, d_lo, d_hi) YIELD node_id`
///
/// Renvoie les ids (`u64`) des nœuds VIVANTS de `community_id` dont la
/// centralité de PageRank en cache (`centrality_cached`, Q0.16) tombe dans
/// `[c_lo, c_hi]` ET dont le degré sortant tombe dans `[d_lo, d_hi]`.
///
/// - Bornes inclusives, insensibles à l'ordre (normalisées par le store).
/// - Centralité attendue en `u16` (0..=65535), degré en `u32`.
/// - Repli scan exact automatique (voir [`ObrainDB::window_query_2d_sorted`]).
fn execute_window_2d(
    arguments: &[LogicalExpression],
    substrate: Option<&std::sync::Arc<obrain_substrate::SubstrateStore>>,
) -> Result<AlgorithmResult> {
    let community_id = arg_u64(arguments, 0, "community_id")?;
    let c_lo = arg_u64(arguments, 1, "c_lo")?;
    let c_hi = arg_u64(arguments, 2, "c_hi")?;
    let d_lo = arg_u64(arguments, 3, "d_lo")?;
    let d_hi = arg_u64(arguments, 4, "d_hi")?;

    if c_lo > u16::MAX as u64 || c_hi > u16::MAX as u64 {
        return Err(Error::Internal(format!(
            "obrain.window_2d(): centrality bounds must fit in u16 (0..=65535), got [{c_lo}, {c_hi}]"
        )));
    }
    if d_lo > u32::MAX as u64 || d_hi > u32::MAX as u64 {
        return Err(Error::Internal(format!(
            "obrain.window_2d(): degree bounds must fit in u32, got [{d_lo}, {d_hi}]"
        )));
    }

    let Some(store) = substrate else {
        return Err(Error::Internal(
            "obrain.window_2d requires a substrate-backed database".to_string(),
        ));
    };

    // La voie accélérée retombe seule sur le scan exact quand une
    // précondition manque (layout non trié / grille absente / fenêtre large).
    let slots = store
        .window_query_2d_sorted(
            community_id as u32,
            c_lo as u16,
            c_hi as u16,
            d_lo as u32,
            d_hi as u32,
        )
        .map_err(|e| Error::Internal(format!("obrain.window_2d failed: {e}")))?;

    let mut result = AlgorithmResult::new(vec!["node_id".to_string()]);
    for slot in slots {
        result.add_row(vec![Value::Int64(u64::from(slot) as i64)]);
    }
    Ok(result)
}

/// `CALL obrain.window_2d_energy(cid, c_lo, c_hi, d_lo, d_hi, energy_min_q15)
/// YIELD node_id, energy` — fenêtre cognitive C3.
///
/// Même fenêtre 2D que [`execute_window_2d`], puis filtre sur l'énergie
/// inline (`NodeRecord.energy`, Q1.15) : seuls les nœuds dont
/// `energy_q15() ≥ energy_min_q15 / 32767` sont rendus. Deux colonnes :
/// `node_id` (u64) et `energy` (Float64, décodée en [0, 1]).
///
/// - `energy_min_q15` attendu en `u16` (0..=65535 ; convention Q15 réelle :
///   valeurs utiles 0..=32767, toute valeur > 32767 = seuil 1.0).
/// - Lecture par slot du record (une seule `read_node` par candidat) —
///   coût O(|fenêtre|), aucun état persistant.
fn execute_window_2d_energy(
    arguments: &[LogicalExpression],
    substrate: Option<&std::sync::Arc<obrain_substrate::SubstrateStore>>,
) -> Result<AlgorithmResult> {
    let community_id = arg_u64(arguments, 0, "community_id")?;
    let c_lo = arg_u64(arguments, 1, "c_lo")?;
    let c_hi = arg_u64(arguments, 2, "c_hi")?;
    let d_lo = arg_u64(arguments, 3, "d_lo")?;
    let d_hi = arg_u64(arguments, 4, "d_hi")?;
    let energy_min_q15 = arg_u64(arguments, 5, "energy_min_q15")?;

    if c_lo > u16::MAX as u64 || c_hi > u16::MAX as u64 {
        return Err(Error::Internal(format!(
            "obrain.window_2d_energy(): centrality bounds must fit in u16, got [{c_lo}, {c_hi}]"
        )));
    }
    if d_lo > u32::MAX as u64 || d_hi > u32::MAX as u64 {
        return Err(Error::Internal(format!(
            "obrain.window_2d_energy(): degree bounds must fit in u32, got [{d_lo}, {d_hi}]"
        )));
    }
    if energy_min_q15 > u16::MAX as u64 {
        return Err(Error::Internal(format!(
            "obrain.window_2d_energy(): energy_min_q15 must fit in u16 (0..=65535), got {energy_min_q15}"
        )));
    }

    let Some(store) = substrate else {
        return Err(Error::Internal(
            "obrain.window_2d_energy requires a substrate-backed database".to_string(),
        ));
    };

    let slots = store
        .window_query_2d_sorted(
            community_id as u32,
            c_lo as u16,
            c_hi as u16,
            d_lo as u32,
            d_hi as u32,
        )
        .map_err(|e| Error::Internal(format!("obrain.window_2d_energy failed: {e}")))?;

    let threshold = energy_min_q15.min(32767) as u16; // seuil Q1.15 saturé

    let mut result = AlgorithmResult::new(vec!["node_id".to_string(), "energy".to_string()]);
    for slot in slots {
        // Lecture du record du slot : énergie inline (Q1.15). Le slot est un
        // id de nœud u32 (contrat window_query_2d_sorted).
        let Some(rec) = store.writer().read_node(slot).ok().flatten() else {
            continue; // slot non lisible (tombstone/borne) — ignoré
        };
        let energy = rec.energy; // u16 Q1.15
        if energy >= threshold {
            let energy_f = f64::from(rec.energy_q15());
            result.add_row(vec![
                Value::Int64(u64::from(slot) as i64),
                Value::Float64(energy_f),
            ]);
        }
    }
    Ok(result)
}
