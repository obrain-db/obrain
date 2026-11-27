//! Divergence ancre/miroir après panne (ou écriture désynchronisée) :
//! l'ancre (autorité) et le miroir Q1.15 (projection) peuvent diverger.
//! L'autorité doit rester l'ancre ; la divergence doit être DÉTECTABLE.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock::{self, EnergyRead};
use obrain_cognitive::decay_law::DecayLaw;
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(64);

fn ancre_bytes(e: f64, t_ref: f64, t_hi: f64, h: f64) -> Value {
    let mut b = [0u8; 32];
    for (i, x) in [e, t_ref, t_hi, h].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(b.as_slice()))
}

#[test]
fn la_divergence_ancre_miroir_est_detectable_et_l_ancre_fait_autorite() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);
    let t = (BASE as f64) / 1e6;
    // Miroir (projection) : 0,5. Ancre (autorité) : 0,75 — divergence simulée.
    sub.set_node_energy_f32(n, 0.5).unwrap();
    sub.set_node_property(n, "_cog_energy_anchor", ancre_bytes(0.75, t, t, 64.0));
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    let miroir = sub.get_node_energy_f32(n).unwrap().unwrap();
    let ancre = decay_clock::read_anchor(&sub, n).unwrap();
    eprintln!("miroir (Q1.15) = {miroir}, ancre e_ref = {}", ancre.e_ref);
    assert!((ancre.e_ref - 0.75).abs() < 1e-9, "ancre = 0,75 (autorité)");
    assert!(
        (miroir - 0.5).abs() < 1e-4,
        "miroir = 0,5 (projection obsolète)"
    );

    // La lecture qualifiée rend l'ancre (Effective 0,75), pas le miroir obsolète.
    let r = decay_clock::read_energy_qualified(&sub, n, &law, BASE);
    eprintln!("read_energy_qualified = {r:?}");
    assert!(
        matches!(r, EnergyRead::Effective(e) if (e - 0.75).abs() < 1e-3),
        "l'ancre fait autorité, pas le miroir"
    );

    // La divergence est détectable : ancre ≠ miroir.
    assert!(
        (ancre.e_ref as f32 - miroir).abs() > 0.1,
        "divergence ancre/miroir observable"
    );
}
