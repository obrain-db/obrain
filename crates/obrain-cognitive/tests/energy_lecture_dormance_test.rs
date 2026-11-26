//! Énergie — chemins de LECTURE et de DORMANCE restés non couverts (#414, périmètre C).
//!
//! Deux familles de chemins :
//! - **lecture** : la relecture paresseuse depuis le graph store (mode legacy,
//!   sans colonne substrat) — `get_energy`, `load_from_graph`, `snapshot` ;
//! - **dormance** : le vieillissement des nœuds que personne ne lit —
//!   `decay_all`, `decay_tick`, `list_low_energy`.
//!
//! S'y ajoutent les accesseurs purs de `NodeEnergy`, les constructeurs de
//! `EnergyConfig`, `energy_score` / `effective_half_life` et la lecture
//! qualifiée substrat `read_qualified`.

#![cfg(feature = "energy")]

use obrain_common::types::{NodeId, Value};
use obrain_cognitive::{effective_half_life, energy_score, EnergyConfig, EnergyStore, NodeEnergy};
use obrain_core::graph::GraphStoreMut;
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

const H: Duration = Duration::from_secs(3600);

fn node(id: u64) -> NodeId {
    NodeId::new(id)
}

/// Une base substrat brute, utilisée ici comme simple `GraphStoreMut` pour
/// exercer le mode legacy (`with_graph_store`), pas la colonne d'énergie.
fn substrate() -> (Arc<obrain_substrate::SubstrateStore>, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(obrain_substrate::SubstrateStore::create(td.path().join("kb")).unwrap());
    (sub, td)
}

// ---------------------------------------------------------------------------
// NodeEnergy — accesseurs et matérialisation
// ---------------------------------------------------------------------------

#[test]
fn node_energy_accesseurs_et_clamp() {
    let t0 = Instant::now();
    let e = NodeEnergy::new_at(2.0, H, t0);
    assert!((e.raw_energy() - 2.0).abs() < 1e-12);
    assert_eq!(e.last_activated(), t0);
    assert_eq!(e.half_life(), H);

    let mut borné = NodeEnergy::new_at(5.0, H, t0);
    borné.clamp(0.0, 2.0);
    assert!((borné.raw_energy() - 2.0).abs() < 1e-12, "clamp haut");
    borné.clamp(3.0, 4.0);
    assert!((borné.raw_energy() - 3.0).abs() < 1e-12, "clamp bas");
}

#[test]
fn node_energy_boost_at_decroit_puis_ajoute() {
    let t0 = Instant::now();
    let mut e = NodeEnergy::new_at(1.0, H, t0);
    // Une demi-vie : 1.0 → 0.5, puis +0.5 = 1.0, ancre déplacée.
    let t1 = t0 + H;
    e.boost_at(0.5, t1);
    assert!((e.raw_energy() - 1.0).abs() < 1e-12);
    assert_eq!(e.last_activated(), t1);
}

#[test]
fn node_energy_advance_to_materialise_et_deplace_l_ancre() {
    let t0 = Instant::now();
    let mut e = NodeEnergy::new_at(1.0, H, t0);
    e.advance_to(t0 + H);
    assert!((e.raw_energy() - 0.5).abs() < 1e-12);
    assert_eq!(e.last_activated(), t0 + H);
}

// ---------------------------------------------------------------------------
// EnergyConfig — constructeurs et accès
// ---------------------------------------------------------------------------

#[test]
fn config_builders_et_accesseur() {
    let cfg = EnergyConfig::default()
        .with_half_life(Duration::from_secs(60))
        .with_boost(2.0);
    let store = EnergyStore::new(cfg);
    assert_eq!(store.config().default_half_life, Duration::from_secs(60));
    assert_eq!(store.config().boost_on_mutation, 2.0);
}

// ---------------------------------------------------------------------------
// energy_score / effective_half_life — bornes
// ---------------------------------------------------------------------------

#[test]
fn energy_score_bornes() {
    assert_eq!(energy_score(0.0, 1.0), 0.0);
    assert_eq!(energy_score(-1.0, 1.0), 0.0);
    assert_eq!(energy_score(f64::NAN, 1.0), 0.0);
    // Référence invalide (≤ 0) → repli sur 1.0 : même courbe que ref = 1.0.
    let repli = energy_score(1.0, 0.0);
    let direct = energy_score(1.0, 1.0);
    assert!((repli - direct).abs() < 1e-12);
    // Valeur exacte : 1 − e^−1.
    assert!((direct - (1.0 - std::f64::consts::E.recip())).abs() < 1e-12);
}

#[test]
fn effective_half_life_bornes() {
    let base = Duration::from_secs(3600);
    assert_eq!(effective_half_life(base, 10, 0.0), base, "α = 0 → base");
    assert_eq!(effective_half_life(base, 10, f64::NAN), base, "α NaN → base");
    assert_eq!(effective_half_life(base, 0, 1.0), base, "degré 0 → base");
    let renforcée = effective_half_life(base, 10, 1.0);
    assert!(renforcée > base, "degré 10 renforce : {renforcée:?}");
}

// ---------------------------------------------------------------------------
// Lecture paresseuse (mode legacy) et écriture transitée
// ---------------------------------------------------------------------------

#[test]
fn boost_transite_et_la_lecture_paresseuse_relit_le_graph_store() {
    let (sub, _td) = substrate();
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let node = sub.create_node(&["n"]);

    // Écriture transitée : `boost` persiste PROP_ENERGY (+ epoch) au graph store.
    let store = EnergyStore::with_graph_store(EnergyConfig::default(), gs.clone());
    store.boost(node, 1.0);
    assert!((store.get_energy(node) - 1.0).abs() < 1e-6);

    // Un store NEUF (cache vide) relit la valeur depuis le graph store.
    let frais = EnergyStore::with_graph_store(EnergyConfig::default(), gs.clone());
    let e = frais.get_energy(node);
    assert!((e - 1.0).abs() < 1e-6, "lecture paresseuse : {e}");
}

#[test]
fn lecture_paresseuse_depuis_une_propriete_heritee() {
    let (sub, _td) = substrate();
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let node = sub.create_node(&["n"]);
    // Propriété posée HORS du module (base héritée) : PROP_ENERGY seul.
    sub.set_node_property(node, "_cog_energy", Value::Float64(2.0));

    let store = EnergyStore::with_graph_store(
        EnergyConfig {
            default_half_life: H,
            ..Default::default()
        },
        gs.clone(),
    );
    assert!((store.get_energy(node) - 2.0).abs() < 1e-6);
    // Nœud jamais suivi → 0.0, pas un panique ni un NaN.
    let manquant = sub.create_node(&["n"]);
    assert_eq!(store.get_energy(manquant), 0.0);
}

#[test]
fn load_from_graph_recharge_le_cache_legacy() {
    let (sub, _td) = substrate();
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let a = sub.create_node(&["n"]);
    let b = sub.create_node(&["n"]);
    sub.set_node_property(a, "_cog_energy", Value::Float64(0.5));
    sub.set_node_property(b, "_cog_energy", Value::Float64(0.25));

    let store = EnergyStore::with_graph_store(EnergyConfig::default(), gs.clone());
    assert_eq!(store.load_from_graph(), 2, "deux nœuds porteurs");
    assert_eq!(store.len(), 2);
    assert!(!store.is_empty());
    // Un second chargement ne duplique pas.
    assert_eq!(store.load_from_graph(), 0);
}

// ---------------------------------------------------------------------------
// Dormance (mode legacy) — vieillissement et seuil
// ---------------------------------------------------------------------------

#[test]
fn dormance_legacy_decay_all_et_list_low_energy() {
    let store = EnergyStore::new(EnergyConfig::default());
    store.boost(node(1), 1.0);
    store.boost(node(2), 0.5);
    store.boost(node(3), 0.25);

    let snap = store.snapshot();
    assert_eq!(snap.len(), 3);

    // decay_all divise chaque énergie par deux.
    store.decay_all(0.5);
    let bas = store.list_low_energy(0.3);
    assert_eq!(bas.len(), 2, "0,25 et 0,125 sous le seuil, 0,5 au-dessus");
}

#[test]
fn dormance_legacy_decay_tick_avance_et_bornes_par_max_nodes() {
    let store = EnergyStore::new(EnergyConfig::default());
    store.boost(node(1), 1.0);
    store.boost(node(2), 0.5);
    store.boost(node(3), 0.25);
    // Sans borne : les trois nœuds sont avancés en une tique.
    assert_eq!(store.decay_tick(0), 3);
    // Borne à 1 : un seul nœud avancé, le `break` interrompt la boucle.
    assert_eq!(store.decay_tick(1), 1);
}

#[test]
fn eviction_lru_borne_le_cache() {
    let store = EnergyStore::new(EnergyConfig::default()).with_max_cache_entries(2);
    store.boost(node(1), 1.0);
    store.boost(node(2), 1.0);
    store.boost(node(3), 1.0);
    assert_eq!(store.len(), 2, "le 3e boost évince l'entrée la moins récente");
}

// ---------------------------------------------------------------------------
// Lecture qualifiée (substrat)
// ---------------------------------------------------------------------------

#[cfg(feature = "substrate")]
#[test]
fn read_qualified_rend_un_etat_effectif_apres_boost() {
    use obrain_cognitive::decay_clock::EnergyRead;
    let (sub, _td) = substrate();
    let node = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(EnergyConfig::default(), sub.clone());
    store.boost(node, 0.5);
    match store.read_qualified(node) {
        EnergyRead::Effective(e) => assert!((e - 0.5).abs() < 1e-3, "e = {e}"),
        autre => panic!("attendu Effective, obtenu {autre:?}"),
    }
}
