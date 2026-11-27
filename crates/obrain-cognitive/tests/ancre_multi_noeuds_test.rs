//! Contre-épreuve multi-nœuds : la corruption d'une ancre ne bloque pas les
//! autres connaissances, n'est pas réparée implicitement, et reste observable
//! après réouverture.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(64);
const JOUR: i64 = 24 * 3600 * 1_000_000;

fn horloge() -> (Arc<AtomicI64>, Arc<dyn Fn() -> i64 + Send + Sync>) {
    let t = Arc::new(AtomicI64::new(BASE));
    let f: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&t);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    (t, f)
}

#[test]
fn une_corruption_ne_bloque_pas_les_autres_et_n_est_pas_reparee() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (_c, clock_fn) = horloge();
    let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
    let store = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub.clone(),
    )
    .with_clock(clock_fn.clone());

    let valide: NodeId = sub.create_node(&["v"]);
    let corrompu: NodeId = sub.create_node(&["c"]);
    let non_ancre: NodeId = sub.create_node(&["u"]);
    sub.flush().unwrap();

    store.boost(valide, 1.0);
    store.boost(corrompu, 1.0);
    // Non ancré : colonne posée hors module, aucune ancre.
    sub.set_node_energy_f32(non_ancre, 1.0);
    // Corruption contrôlée.
    sub.set_node_property(corrompu, "_cog_energy_anchor", Value::Bytes(Arc::from(vec![1u8, 2, 3].as_slice())));
    sub.flush().unwrap();

    // Balayage : la corruption n'empêche pas le traitement des autres.
    let stats = decay_clock::advance_all(sub.as_ref(), &DecayLaw::new(H), 0, BASE + JOUR).unwrap();
    eprintln!("stats: {:?}", stats);
    assert_eq!(stats.advanced, 1, "le nœud valide est avancé");
    assert_eq!(stats.unanchored, 1, "le nœud non ancré est migré (légitime)");
    assert_eq!(stats.corrupted, 1, "le nœud corrompu est signalé");

    // Renforcement sur nœud corrompu : PAS de restauration implicite.
    let avant = sub.get_node_property(corrompu, &PropertyKey::from("_cog_energy_anchor"));
    let r = decay_clock::reinforce(sub.as_ref(), corrompu, 0.1, &DecayLaw::new(H), 1.0, BASE + 1);
    assert!(r.is_none(), "reinforce sur ancre corrompue -> None (pas de réparation)");
    let apres = sub.get_node_property(corrompu, &PropertyKey::from("_cog_energy_anchor"));
    assert_eq!(avant, apres, "octets corrompus inchangés");

    // Réouverture : la corruption reste observable.
    drop(store);
    let sub2 = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store2 = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub2.clone(),
    )
    .with_clock(clock_fn);
    let stats2 = decay_clock::advance_all(sub2.as_ref(), &DecayLaw::new(H), 0, BASE + 2).unwrap();
    assert_eq!(stats2.corrupted, 1, "corruption observable après réouverture");
    let _ = store2.get_energy(corrompu);
}