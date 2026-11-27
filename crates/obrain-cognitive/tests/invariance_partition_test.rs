//! Invariance temporelle : l'énergie effective après T=64 s (H=64 s, E0=1)
//! vaut 0,5 quelle que soit la fréquence de matérialisation. Chaque scénario
//! est isolé (substrat + horloge + nœud frais) pour éviter toute fuite du
//! high-water mark entre partitions.

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
const H: Duration = Duration::from_secs(64);
const T_US: i64 = 64 * 1_000_000;
const REF: f64 = 0.5;

fn scenario(chemin: &std::path::Path, ends: &[i64]) -> (f64, u64) {
    let clock = Arc::new(AtomicI64::new(BASE));
    let clock_fn: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&clock);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    let sub = Arc::new(SubstrateStore::create(chemin).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();
    let store = EnergyStore::with_substrate(
        EnergyConfig { default_half_life: H, ..Default::default() },
        sub.clone(),
    )
    .with_clock(clock_fn);
    store.boost(node, 1.0);

    let law = DecayLaw::new(H);
    let mut ecritures = 0u64;
    for &fin in ends {
        clock.store(BASE + fin, Ordering::Relaxed);
        let s = decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + fin).unwrap();
        ecritures += s.advanced;
    }
    let e = decay_clock::energy_at(sub.as_ref(), node, &law, BASE + *ends.last().unwrap()).unwrap_or(0.0);
    (e, ecritures)
}

#[test]
fn l_energie_effective_est_independante_de_la_partition() {
    let td = tempfile::tempdir().unwrap();
    let (e_a, w_a) = scenario(&td.path().join("a"), &[T_US]);
    let fin_b: Vec<i64> = (1..=64).map(|i| i * 1_000_000).collect();
    let (e_b, w_b) = scenario(&td.path().join("b"), &fin_b);
    let fin_c: Vec<i64> = (1..=1024).map(|i| i * 62_500).collect();
    let (e_c, w_c) = scenario(&td.path().join("c"), &fin_c);
    // Irrégulier : 10, 20, 30, 4 s (total 64 s).
    let (e_d, w_d) = scenario(&td.path().join("d"), &[10_000_000, 30_000_000, 60_000_000, 64_000_000]);

    eprintln!("A(unique): e={e_a:.12} écritures={w_a}");
    eprintln!("B(64×1s): e={e_b:.12} écritures={w_b}");
    eprintln!("C(1024×62.5ms): e={e_c:.12} écritures={w_c}");
    eprintln!("D(irrégulier): e={e_d:.12} écritures={w_d}");

    for (e, n) in [(e_a, "A"), (e_b, "B"), (e_c, "C"), (e_d, "D")] {
        let rel = (e - REF).abs() / REF;
        assert!(rel < 1e-6, "{n}: énergie {e} trop loin de {REF} (rel={rel:e})");
    }
}
