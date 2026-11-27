//! Cause de `advanced=0` à +1 µs, DÉMONTRÉE : seuil de matérialisation f32.
//! La dynamique f64 de l'ancre est préservée ; la colonne f32 diffère l'écriture.

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
const US: i64 = 1;
const JOUR: i64 = 24 * 3600 * 1_000_000;
const H: Duration = Duration::from_secs(64);

#[test]
fn la_materialisation_f32_differe_sans_perdre_la_dynamique() {
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
    store.boost(node, 1.0);

    // +1 µs : la décroissance mathématique ≈ 1-1,08e-8 s'arrondit À 1 en f32
    // → la colonne ne change pas au bit près → aucune écriture (advanced=0).
    let s1 = decay_clock::advance_all(sub.as_ref(), &DecayLaw::new(H), 0, BASE + US).unwrap();
    assert_eq!(s1.advanced, 0, "à +1 µs la colonne f32 ne change pas → écriture différée");
    assert_eq!(s1.scanned, 1, "le nœud est bien scanné (pas dormant)");

    // +1 jour : dt calculé depuis t_ref=BASE (inchangé) → l'énergie cumule
    // TOUT le temps écoulé, la dynamique f64 est exacte.
    let s2 = decay_clock::advance_all(sub.as_ref(), &DecayLaw::new(H), 0, BASE + JOUR).unwrap();
    assert_eq!(s2.advanced, 1, "à +1 jour la matérialisation a lieu");
    let e = store.get_energy(node);
    let attendu = 1.0 * 2f64.powf(-(JOUR as f64) / H.as_secs_f64());
    assert!((e - attendu).abs() < 1e-6, "énergie = décroissance exacte sur le temps total : {e} vs {attendu}");
}
