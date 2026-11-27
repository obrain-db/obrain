//! Renforcement sous horloge NON monotone : le high-water mark ne doit pas
//! produire une décroissance rétroactive (double comptabilisation).

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H_US: i64 = 64 * 1_000_000; // une demi-vie
const H: Duration = Duration::from_secs(64);

#[test]
fn renforcement_apres_retour_arriere_d_horloge() {
    let td = tempfile::tempdir().unwrap();
    let clock = Arc::new(AtomicI64::new(BASE));
    let clock_fn: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&clock);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub.clone(),
    )
    .with_clock(clock_fn);
    let law = DecayLaw::new(H);

    // 1. Énergie initiale 0,4.
    store.boost(node, 0.4);
    let e0 = store.get_energy(node);
    eprintln!("t0: e={e0:.6}");

    // 2. Une demi-vie → 0,2.
    clock.store(BASE + H_US, Ordering::Relaxed);
    decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + H_US).unwrap();
    let e1 = store.get_energy(node);
    eprintln!("après 1 demi-vie: e={e1:.6}");
    assert!((e1 - 0.2).abs() < 1e-3, "attendu ~0.2, obtenu {e1}");

    // 3-4. Retour arrière de l'horloge physique, puis renforcement +0,1.
    clock.store(BASE, Ordering::Relaxed);
    let r = store.boost(node, 0.1);
    eprintln!("reinforce -> {:?}", r);

    // 5. Lecture immédiate (temps effectif constant).
    let e2 = store.get_energy(node);
    eprintln!("immédiatement après renforcement: e={e2:.6}");

    // Référence : le renforcement intervient au temps LOGIQUE (high-water),
    // pas rétroactivement → énergie attendue ≈ 0,3, PAS 0,15.
    // Un résultat ~0,15 (0,3 décru d'une demi-vie) révèle la double comptabilisation.
    assert!(
        (e2 - 0.3).abs() < 1e-3,
        "le renforcement doit s'appliquer au temps logique courant : attendu ~0,3, obtenu {e2} (double comptabilisation ?)"
    );
    // À temps effectif constant, les lectures/consolidations NE doivent PAS
    // provoquer de décroissance supplémentaire.
    for _ in 0..3 {
        decay_clock::advance_all(sub.as_ref(), &law, 0, BASE).unwrap();
        let e = store.get_energy(node);
        assert!((e - 0.3).abs() < 1e-3, "temps constant → pas de décroissance : {e}");
    }
    // L'horloge reprend, dépasse le high-water mark → décroissance reprend.
    clock.store(BASE + 2 * H_US, Ordering::Relaxed);
    let e3 = store.get_energy(node);
    eprintln!("après reprise +2 demi-vies: e={e3:.6}");
    // Le renforcement a eu lieu au temps LOGIQUE (une demi-vie) : la reprise
    // de +2 demi-vies ne compte qu'UNE demi-vie depuis ce temps logique → 0,15.
    assert!((e3 - 0.15).abs() < 1e-3, "décroissance comptée depuis le temps logique : attendu ~0,15, obtenu {e3}");
}
