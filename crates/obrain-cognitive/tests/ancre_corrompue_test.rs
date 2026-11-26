//! Témoin public : une ancre corrompue ne doit ni réinitialiser l'énergie
//! canonique ni déclencher une migration historique injustifiée.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const JOUR: i64 = 24 * 3600 * 1_000_000;
const H: Duration = Duration::from_secs(64);

fn horloge() -> (Arc<AtomicI64>, Arc<dyn Fn() -> i64 + Send + Sync>) {
    let t = Arc::new(AtomicI64::new(BASE));
    let f: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&t);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    (t, f)
}

/// Ancre valide → corruption contrôlée (longueur invalide) → lecture publique
/// → avancement → fermeture → réouverture. L'énergie canonique doit rester
/// intacte (colonne E_ref), et la corruption ne doit pas être confondue avec
/// une ancre absente (pas de migration historique silencieuse).
#[test]
fn une_ancre_corrompue_preserve_l_energie_canonique() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (clock, clock_fn) = horloge();
    let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();

    let store = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub.clone(),
    )
    .with_clock(clock_fn.clone());
    store.boost(node, 1.0);
    assert!((store.get_energy(node) - 1.0).abs() < 1e-9, "énergie initiale 1.0");

    // Corruption contrôlée : ancre de 3 octets au lieu de 32.
    sub.set_node_property(
        node,
        "_cog_energy_anchor",
        Value::Bytes(Arc::from(vec![1u8, 2, 3].as_slice())),
    );
    sub.flush().unwrap();

    // Lecture publique : pas de réinitialisation silencieuse (colonne préservée).
    let e = store.get_energy(node);
    assert!((e - 1.0).abs() < 1e-9, "énergie préservée après corruption : {e}");

    // Avancement : la corruption NE DOIT PAS être traitée comme une ancre
    // absente (migration historique). Le comportement actuel est observé ici.
    clock.store(BASE + JOUR, Ordering::Relaxed);
    let stats = decay_clock::advance_all(sub.as_ref(), &DecayLaw::new(H), 0, BASE + JOUR).unwrap();
    eprintln!("après corruption : scanned={} advanced={} unanchored={} corrupted={}", stats.scanned, stats.advanced, stats.unanchored, stats.corrupted);
    assert_eq!(stats.unanchored, 0, "une ancre corrompue ne doit PAS être migrée comme une ancre absente");
    assert_eq!(stats.corrupted, 1, "la corruption est signalée (verdict typé), pas migrée");

    // Fermeture + réouverture : l'énergie canonique survit.
    drop(store);
    let sub2 = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store2 = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub2,
    )
    .with_clock(clock_fn);
    let e2 = store2.get_energy(node);
    assert!((e2 - 1.0).abs() < 1e-9, "énergie canonique préservée après réouverture : {e2}");
}
