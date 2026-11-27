//! Comparaison A/B : la monotonie est fournie par EnergyStore::now_eff
//! (session_hi), PAS par decay_clock direct.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H_US: i64 = 64 * 1_000_000;
const H: Duration = Duration::from_secs(64);

fn setup(
    chemin: &std::path::Path,
) -> (
    Arc<SubstrateStore>,
    NodeId,
    EnergyStore,
    Arc<AtomicI64>,
    Arc<dyn Fn() -> i64 + Send + Sync>,
) {
    let clock = Arc::new(AtomicI64::new(BASE));
    let clock_fn: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&clock);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    let sub = Arc::new(SubstrateStore::create(chemin).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(
        EnergyConfig {
            default_half_life: H,
            ..Default::default()
        },
        sub.clone(),
    )
    .with_clock(clock_fn.clone());
    (sub, node, store, clock, clock_fn)
}

#[test]
fn chemin_a_energystore_est_monotone() {
    let td = tempfile::tempdir().unwrap();
    let (sub, node, store, clock, _) = setup(&td.path().join("a"));
    let law = DecayLaw::new(H);
    store.boost(node, 0.4);
    clock.store(BASE + H_US, Ordering::Relaxed);
    decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + H_US).unwrap();
    // DÉFAUT : advance_all (direct) ne met PAS à jour session_hi. Sans une
    // observation EnergyStore au temps haut, le recul n'est PAS protégé.
    clock.store(BASE, Ordering::Relaxed);
    store.boost(node, 0.1);
    let e = store.get_energy(node);
    eprintln!("A sans observation haute (EnergyStore): e={e:.6}");
    assert!(
        (e - 0.3).abs() < 1e-3,
        "τ = max(now, t_hi, t_ref) rend le temps logique : attendu 0.3, obtenu {e}"
    );
}

#[test]
fn chemin_a_observation_haute_protege() {
    let td = tempfile::tempdir().unwrap();
    let (sub, node, store, clock, _) = setup(&td.path().join("a2"));
    let law = DecayLaw::new(H);
    store.boost(node, 0.4);
    clock.store(BASE + H_US, Ordering::Relaxed);
    decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + H_US).unwrap();
    let _e1 = store.get_energy(node); // OBSERVE au temps haut -> session_hi = BASE+H
    clock.store(BASE, Ordering::Relaxed);
    store.boost(node, 0.1);
    let e = store.get_energy(node);
    eprintln!("A' (observation haute puis boost): e={e:.6}");
    assert!(
        (e - 0.3).abs() < 1e-3,
        "session_hi mis à jour par l'observation : attendu 0.3, obtenu {e}"
    );
}

#[test]
fn chemin_b_decay_clock_direct_n_est_pas_protege() {
    let td = tempfile::tempdir().unwrap();
    let (sub, node, store, clock, _) = setup(&td.path().join("b"));
    let law = DecayLaw::new(H);
    store.boost(node, 0.4);
    clock.store(BASE + H_US, Ordering::Relaxed);
    decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + H_US).unwrap();
    // Appel DIRECT avec now inférieur au high-water mark persisté.
    let r = decay_clock::reinforce(sub.as_ref(), node, 0.1, &law, 1.0, BASE);
    eprintln!("B (decay_clock direct): reinforce={r:?}");
    let e = decay_clock::energy_at(sub.as_ref(), node, &law, BASE);
    eprintln!("B (decay_clock direct): e={e:?}");
    // decay_clock direct ne fournit PAS la monotonie : l'ancre écrite à t_ref=BASE
    // avec t_hi=BASE+H subit immédiatement une décroissance rétroactive.
    assert!(
        e.map_or(false, |v| (v - 0.3).abs() < 1e-3),
        "chemin direct protégé par τ persisté : attendu 0.3, obtenu {e:?}"
    );
}
