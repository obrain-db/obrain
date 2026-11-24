//! La décroissance d'énergie doit survivre à la fermeture de la base.
//!
//! Défaut mesuré (2026-11-24, base b8ee6f87) : en mode substrat, l'énergie est
//! stockée dans la colonne `NodeRecord.energy` (Q1.15) mais **aucun instant de
//! référence n'est persisté**. Au redémarrage, la valeur de la colonne est
//! relue telle quelle : elle est figée. Le champ `last_activated` est un
//! `web_time::Instant`, monotone et non sérialisable — il meurt avec le
//! processus.
//!
//! Ces tests simulent un redémarrage réel : `SubstrateStore::close()` puis
//! `SubstrateStore::open()` sur le même répertoire, avec une demi-vie d'une
//! seconde pour que l'écoulement du temps soit observable.

#![cfg(feature = "substrate")]

use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const HALF_LIFE: Duration = Duration::from_secs(1);

fn config() -> EnergyConfig {
    EnergyConfig {
        default_half_life: HALF_LIFE,
        ..Default::default()
    }
}

/// Ouvre une base, booste un nœud à 1.0, ferme proprement, attend deux
/// demi-vies, rouvre. La valeur lue doit valoir `E_ref · 2^(-Δt/H) = 2^-2`.
#[test]
fn l_energie_decroit_pendant_que_la_base_est_fermee() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let node_id;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        node_id = sub.create_node(&["n"]);
        sub.flush().unwrap();
        {
            let store = EnergyStore::with_substrate(config(), Arc::clone(&sub));
            store.boost(node_id, 1.0);
            let e = store.get_energy(node_id);
            assert!(
                (e - 1.0).abs() < 1e-3,
                "juste après le boost, énergie = {e} (attendu 1.0)"
            );
        }
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // La base est fermée : plus aucun code ne tourne. Deux demi-vies passent.
    std::thread::sleep(2 * HALF_LIFE);

    {
        let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
        let store = EnergyStore::with_substrate(config(), Arc::clone(&sub));
        let e = store.get_energy(node_id);
        assert!(
            (e - 0.25).abs() < 0.03,
            "après 2 demi-vies hors ligne, énergie = {e}, attendu ≈ 0.25 (2^-2 = 0.25)"
        );
    }
}

/// Deux sessions successives : chacune doit repartir de l'ancre physique
/// laissée par la précédente, sans cumuler ni perdre de temps.
#[test]
fn deux_sessions_ne_comptent_qu_une_fois_le_temps_ecoule() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let node_id;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        node_id = sub.create_node(&["n"]);
        sub.flush().unwrap();
        let store = EnergyStore::with_substrate(config(), Arc::clone(&sub));
        store.boost(node_id, 1.0);
        drop(store);
        sub.flush().unwrap();
        sub.close().unwrap();
    }
    std::thread::sleep(HALF_LIFE);

    let apres_premiere;
    {
        let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
        let store = EnergyStore::with_substrate(config(), Arc::clone(&sub));
        apres_premiere = store.get_energy(node_id);
        // La lecture ne réécrit pas l'ancre : on ne matérialise pas ici.
        drop(store);
        sub.flush().unwrap();
        sub.close().unwrap();
    }
    assert!(
        (apres_premiere - 0.5).abs() < 0.03,
        "après 1 demi-vie, énergie = {apres_premiere}, attendu ≈ 0.5"
    );

    // Une seconde session, immédiatement après, ne doit PAS avoir re-décru.
    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store = EnergyStore::with_substrate(config(), Arc::clone(&sub));
    let apres_seconde = store.get_energy(node_id);
    assert!(
        (apres_seconde - apres_premiere).abs() < 0.02,
        "relire dans une nouvelle session ne doit pas re-décroître : {apres_premiere} puis {apres_seconde}"
    );
}
