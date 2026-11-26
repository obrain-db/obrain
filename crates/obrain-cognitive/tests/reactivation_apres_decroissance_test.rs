//! Réactivation cognitive après décroissance complète : une énergie nulle est
//! un état valide, et un renforcement la réactive (0,1), sans corruption.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock::{self, EnergyRead};
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const JOUR: i64 = 24 * 3600 * 1_000_000;
const H: Duration = Duration::from_secs(64);

#[test]
fn une_decroissance_complete_puis_un_renforcement_reactive() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let clock = Arc::new(AtomicI64::new(BASE));
    let clock_fn: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&clock);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub.clone(),
    ).with_clock(clock_fn.clone());
    let law = DecayLaw::new(H);

    store.boost(node, 1.0);
    clock.store(BASE + JOUR, Ordering::Relaxed);
    decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + JOUR).unwrap();
    let e = store.get_energy(node);
    eprintln!("après 1 jour: e={e}");
    assert!(e == 0.0, "décroissance complète -> 0, obtenu {e}");

    // Fermeture + réouverture.
    drop(store);
    let sub2 = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store2 = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub2.clone(),
    ).with_clock(clock_fn);
    let r1 = store2.read_qualified(node);
    eprintln!("après réouverture: {r1:?}");
    assert!(matches!(r1, EnergyRead::Effective(e) if e == 0.0), "EnergyRead = Effective(0.0), obtenu {r1:?}");

    // Réactivation : renforcement +0,1 -> ~0,1 exploitable.
    store2.boost(node, 0.1);
    let r2 = store2.read_qualified(node);
    eprintln!("après renforcement: {r2:?}");
    assert!(matches!(r2, EnergyRead::Effective(e) if (e - 0.1).abs() < 1e-3), "EnergyRead ≈ Effective(0.1), obtenu {r2:?}");
}
