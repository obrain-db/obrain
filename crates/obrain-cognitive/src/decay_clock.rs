//! # Matérialisation de la décroissance sur la colonne d'énergie du substrat
//!
//! En mode substrat, l'énergie vit dans `NodeRecord.energy` (Q1.15) et l'instant
//! de référence dans la propriété `_cog_energy_last_activated_epoch` (secondes
//! epoch `f64`). Ce module est la seule porte entre les deux : il lit l'ancre,
//! applique la loi, et — quand on le lui demande explicitement — réécrit la
//! colonne ET l'ancre ensemble.
//!
//! ## Pourquoi la lecture est paresseuse et la tique est facultative
//!
//! La valeur rendue par [`current_energy`] est calculée depuis l'ancre, donc
//! elle est **exacte sans qu'aucune minuterie ne tourne**. La tique périodique
//! ([`advance_all`]) ne sert pas à rendre la lecture juste : elle sert à faire
//! vieillir les nœuds que PERSONNE NE LIT, pour que `list_low_energy` et
//! l'archivage les voient. Comme chaque nœud porte sa propre ancre, une tique
//! manquée — ou un redémarrage, ou un arrêt de plusieurs jours — ne perd rien :
//! la tique suivante rattrape le temps avec `2^(-Δt/H)` sur un `Δt` plus grand.
//!
//! ## Coût
//!
//! Une avance coûte deux écritures journalisées par nœud (colonne + ancre).
//! C'est le prix de l'exactitude par nœud : appliquer un seul facteur à toute
//! la colonne (`SubstrateStore::decay_all_energy`, un seul enregistrement WAL)
//! n'est juste que si toutes les ancres sont identiques, et il faut alors
//! déplacer N ancres de toute façon. `max_nodes` borne une tique : les nœuds
//! non traités gardent leur ancre et sont rattrapés à la suivante.

use obrain_common::types::{NodeId, PropertyKey};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

use crate::decay_law::{DecayLaw, elapsed_since, micros_to_epoch_secs, unix_micros};
use crate::store_trait::{PROP_ENERGY_LAST_ACTIVATED_EPOCH, load_node_f64, persist_node_f64};

/// Clé de propriété précalculée pour les lectures en lot.
fn cle() -> PropertyKey {
    PropertyKey::from(PROP_ENERGY_LAST_ACTIVATED_EPOCH)
}

/// Lit l'ancre physique d'un nœud (micros Unix). `None` = jamais ancré.
pub fn anchor_of(store: &dyn GraphStoreMut, node_id: NodeId) -> Option<i64> {
    load_node_f64(store, node_id, PROP_ENERGY_LAST_ACTIVATED_EPOCH)
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(crate::decay_law::epoch_secs_to_micros)
}

/// Écrit l'ancre physique d'un nœud (micros Unix).
pub fn persist_anchor(store: &dyn GraphStoreMut, node_id: NodeId, micros: i64) {
    persist_node_f64(
        store,
        node_id,
        PROP_ENERGY_LAST_ACTIVATED_EPOCH,
        micros_to_epoch_secs(micros),
    );
}

/// Énergie d'un nœud à `now`, sans rien écrire.
///
/// `None` si le nœud n'est pas vivant (slot non alloué ou enterré) — le même
/// contrat que `SubstrateStore::boost_node_energy_f32`.
pub fn energy_at(
    sub: &SubstrateStore,
    node_id: NodeId,
    law: &DecayLaw,
    now_micros: i64,
) -> Option<f64> {
    let e_ref = sub.get_node_energy_f32(node_id).ok().flatten()? as f64;
    if e_ref <= 0.0 {
        return Some(0.0);
    }
    match anchor_of(sub, node_id) {
        // Pas d'ancre : on ne connaît pas le temps écoulé, on ne l'invente pas.
        None => Some(e_ref),
        Some(t_ref) => Some(law.energy_at(e_ref, elapsed_since(t_ref, now_micros))),
    }
}

/// Énergie d'un nœud à l'instant courant.
pub fn current_energy(sub: &SubstrateStore, node_id: NodeId, law: &DecayLaw) -> Option<f64> {
    energy_at(sub, node_id, law, unix_micros())
}

/// Matérialise un nœud : applique la décroissance écoulée, ajoute `delta`,
/// écrit la colonne et réécrit l'ancre à `now`. Rend la valeur écrite.
///
/// `max_energy` est borné par ce que la colonne Q1.15 sait porter (`1.0`).
pub fn reinforce(
    sub: &SubstrateStore,
    node_id: NodeId,
    delta: f64,
    law: &DecayLaw,
    max_energy: f64,
    now_micros: i64,
) -> Option<f64> {
    let e_courante = energy_at(sub, node_id, law, now_micros)?;
    let plafond = max_energy.min(1.0);
    let nouvelle = (e_courante + delta).clamp(0.0, plafond);
    sub.set_node_energy_f32(node_id, nouvelle as f32).ok()?;
    persist_anchor(sub, node_id, now_micros);
    Some(nouvelle)
}

/// Comptes d'une avance de tique.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecayAdvance {
    /// Slots vivants examinés.
    pub scanned: u64,
    /// Nœuds effectivement avancés (colonne + ancre réécrites).
    pub advanced: u64,
    /// Nœuds d'énergie nulle : rien à faire, aucune ancre écrite.
    pub dormant: u64,
    /// Nœuds non traités faute de budget — rattrapés à la tique suivante.
    pub deferred: u64,
    /// Nœuds dont l'ancre manquait (colonne posée hors de ce module).
    pub unanchored: u64,
}

/// Avance jusqu'à `max_nodes` nœuds le long de LEUR propre loi, depuis LEUR
/// propre ancre, jusqu'à `now`. `max_nodes == 0` = pas de borne.
///
/// Chaque nœud est traité exactement comme `E ← E · 2^(-(now - t_i)/H)`, donc le
/// résultat ne dépend pas du découpage en tiques (propriété de semi-groupe de
/// l'exponentielle). Sauter une tique, ou 10 000, donne le même résultat.
pub fn advance_all(
    sub: &SubstrateStore,
    law: &DecayLaw,
    max_nodes: usize,
    now_micros: i64,
) -> obrain_substrate::SubstrateResult<DecayAdvance> {
    let mut stats = DecayAdvance::default();
    let slots = sub.iter_live_node_energies()?;
    stats.scanned = slots.len() as u64;

    // Ancres lues en un seul verrou de zone : N lectures groupées, pas N
    // acquisitions.
    let ids: Vec<NodeId> = slots.iter().map(|(id, _)| *id).collect();
    let ancres = sub.get_node_property_batch(&ids, &cle());

    for ((node_id, e_ref), ancre) in slots.into_iter().zip(ancres) {
        let e_ref = e_ref as f64;
        if e_ref <= 0.0 {
            stats.dormant += 1;
            continue;
        }
        if max_nodes != 0 && stats.advanced as usize >= max_nodes {
            stats.deferred += 1;
            continue;
        }
        let Some(t_ref) = ancre
            .and_then(|v| v.as_float64())
            .filter(|s| s.is_finite() && *s > 0.0)
            .map(crate::decay_law::epoch_secs_to_micros)
        else {
            stats.unanchored += 1;
            continue;
        };
        let dt = elapsed_since(t_ref, now_micros);
        if dt.is_zero() {
            continue;
        }
        let nouvelle = law.energy_at(e_ref, dt);
        // Une valeur qui ne change pas au bit près ne mérite pas deux écritures.
        if (nouvelle as f32) == (e_ref as f32) {
            continue;
        }
        if sub.set_node_energy_f32(node_id, nouvelle as f32).is_ok() {
            persist_anchor(sub, node_id, now_micros);
            stats.advanced += 1;
        }
    }
    Ok(stats)
}
