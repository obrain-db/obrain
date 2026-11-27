//! Limites du balayage d'avancement : dt nul (saut), ancre sans demi-vie
//! (repli sur la loi), et décroissance effective (écriture réussie).

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
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

/// `advance_all` au MÊME instant que l'ancre : dt nul → le nœud est sauté,
/// aucune écriture, énergie intacte. (Couvre le `if dt.is_zero() { continue; }`.)
#[test]
fn un_avancement_a_dt_nul_ne_touche_pas_le_noeud() {
    let td = tempfile::tempdir().unwrap();
    let (_clock, clock_fn) = horloge();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(
        EnergyConfig {
            default_half_life: H,
            ..Default::default()
        },
        sub.clone(),
    )
    .with_clock(clock_fn);
    store.boost(node, 1.0);

    let stats = decay_clock::advance_all(sub.as_ref(), &DecayLaw::new(H), 0, BASE).unwrap();
    assert!(stats.scanned >= 1, "nœud scanné");
    assert_eq!(stats.advanced, 0, "dt nul → aucun avancement (saut)");
    let e = store.get_energy(node);
    assert!((e - 1.0).abs() < 1e-9, "énergie intacte à dt nul : {e}");
}

/// `advance_all` APRÈS une demi-vie : décroissance effective (0.5) et écriture
/// comptée. (Couvre le chemin `set_node_energy_f32(...).is_ok()` d'écriture.)
#[test]
fn un_avancement_apres_une_demie_vie_ecrit_et_compte() {
    let td = tempfile::tempdir().unwrap();
    let (clock, clock_fn) = horloge();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(
        EnergyConfig {
            default_half_life: H,
            ..Default::default()
        },
        sub.clone(),
    )
    .with_clock(clock_fn);
    store.boost(node, 1.0);

    clock.store(BASE + JOUR, Ordering::Relaxed);
    let stats = decay_clock::advance_all(sub.as_ref(), &DecayLaw::new(H), 0, BASE + JOUR).unwrap();
    assert_eq!(stats.advanced, 1, "un avancement compté");
    let e = store.get_energy(node);
    // 24 h = 1350 demi-vies de 64 s → énergie ≈ 0, à la précision f32 près.
    assert!(e < 0.01, "énergie écoulée : {e}");
}
