//! Branches résiduelles du chemin d'écriture de la décroissance : ancre non
//! figée (H = 0) dans `reinforce`, ancre HÉRITÉE (f64 séparé) dans
//! `advance_all`, et tique à `dt = 0` (ancre posée à l'instant même du
//! balayage) — les trois chemins que les bancs de cadence/migration n'exercent
//! pas parce qu'ils avancent toujours d'au moins une tique.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::store_trait::{PROP_ENERGY_LAST_ACTIVATED_EPOCH, persist_node_f64};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

/// Époque déterministe en micros, loin de l'heure murale (aller-retour exact).
const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(24 * 3600);
const JOUR: i64 = 24 * 3600 * 1_000_000;

/// `reinforce` sur une ancre NON figée (H = 0, posée par `persist_anchor`)
/// fige la demi-vie DE L'APPELANT : la loi ne vient pas de l'ancre mais du
/// paramètre `law`.
#[test]
fn reinforce_avec_ancre_non_figee_utilise_la_loi_de_l_appelant() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id = sub.create_node(&["n"]);
    sub.set_node_energy_f32(id, 0.5).unwrap();
    // Ancre non figée (H = 0).
    decay_clock::persist_anchor(&sub, id, BASE);
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    let nouvelle = decay_clock::reinforce(&sub, id, 0.25, &law, 1.0, BASE).unwrap();
    assert!(
        (nouvelle - 0.75).abs() < 0.01,
        "0,5 + 0,25 à t=0 (aucune décroissance) = 0,75, mesuré {nouvelle}"
    );
}

/// `advance_all` migre une ancre HÉRITÉE (colonne + propriété f64 séparée,
/// sans ancre composite) : il applique la loi depuis `t_ref` hérité puis pose
/// l'ancre composite, sans remettre l'énergie à zéro.
#[test]
fn advance_all_migre_une_ancre_heritee() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id = sub.create_node(&["n"]);
    sub.set_node_energy_f32(id, 1.0).unwrap();
    persist_node_f64(
        sub.as_ref(),
        id,
        PROP_ENERGY_LAST_ACTIVATED_EPOCH,
        (BASE as f64) / 1_000_000.0,
    );
    assert!(decay_clock::read_anchor(&sub, id).is_none());
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    let stats = decay_clock::advance_all(&sub, &law, 0, BASE + JOUR).unwrap();
    assert_eq!(stats.advanced, 1, "le nœud hérité est avancé");

    let e = decay_clock::energy_at(&sub, id, &law, BASE + JOUR).unwrap();
    assert!(
        (e - 0.5).abs() < 0.02,
        "une demi-vie depuis l'ancre héritée → 0,5, mesuré {e}"
    );
    assert!(
        decay_clock::read_anchor(&sub, id).is_some(),
        "ancre composite posée"
    );
}

/// Une tique dont l'ancre est posée À l'instant même du balayage (`dt = 0`)
/// n'écrit rien : le nœud est sauté, pas re-matérialisé.
#[test]
fn advance_all_skippe_un_noeud_ancree_a_l_instant_du_balayage() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id = sub.create_node(&["n"]);
    sub.set_node_energy_f32(id, 0.5).unwrap();
    decay_clock::persist_anchor_explicit(&sub, id, 0.5, BASE, H);
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    let stats = decay_clock::advance_all(&sub, &law, 0, BASE).unwrap();
    assert_eq!(stats.scanned, 1, "un slot vivant examiné");
    assert_eq!(stats.advanced, 0, "dt = 0 → aucun nœud avancé");
}
