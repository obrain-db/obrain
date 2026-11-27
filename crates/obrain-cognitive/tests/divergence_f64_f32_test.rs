//! Divergence ancre f64 / colonne f32 : une énergie f64 positive (2^-160)
//! arrondie à zéro en f32. Observe si la colonne (projection) gouverne la
//! dormance alors que l'ancre (autorité f64) reste strictement positive.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(64);

fn ancre_bytes(e_ref: f64, t_ref: f64, t_hi: f64, h: f64) -> Value {
    let mut b = [0u8; 32];
    for (i, x) in [e_ref, t_ref, t_hi, h].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(b.as_slice()))
}

#[test]
fn une_energie_f64_positive_arrondie_a_zero_en_f32() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    let t = (BASE as f64) / 1e6;
    let e64 = 2f64.powi(-160); // positif en f64, ~0 en f32
    sub.set_node_property(node, "_cog_energy_anchor", ancre_bytes(e64, t, t, 64.0));
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    // Lecture par l'ancre f64 : strictement positive.
    let par_ancre = decay_clock::energy_at(sub.as_ref(), node, &law, BASE);
    eprintln!("energy_at (ancre f64) = {par_ancre:?}");
    assert!(par_ancre.is_some() && par_ancre.unwrap() > 0.0, "l'ancre f64 est positive");

    // Balayage : la colonne f32 = 0 → le nœud est-il déclaré dormant ?
    let stats = decay_clock::advance_all(sub.as_ref(), &law, 0, BASE + 1_000_000).unwrap();
    eprintln!("stats: {stats:?}");
    // Après la migration (décodage AVANT dormance) : l'ancre f64 positive
    // n'est PLUS dormante. La matérialisation est différée (dt=1s → énergie
    // 2^-160 inchangée en f32), mais la dormance ne gouverne plus l'ancre.
    assert_eq!(stats.dormant, 0, "l'ancre positive n'est PAS dormante (autorité f64)");
    assert_eq!(stats.corrupted, 0, "l'ancre n'est PAS corrompue");
    assert_eq!(stats.advanced, 0, "matérialisation différée (pas de changement f32)");
}
