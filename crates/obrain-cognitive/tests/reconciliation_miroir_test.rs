//! Réconciliation à temps constant : un miroir Q1.15 obsolète est réparé sans
//! matérialiser l'ancre, même si aucune décroissance supplémentaire n'a lieu.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(64);

fn ancre(e: f64) -> Value {
    let mut b = [0u8; 32];
    let t = (BASE as f64) / 1e6;
    for (i, x) in [e, t, t, 64.0].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(b.as_slice()))
}

#[test]
fn un_miroir_obsolete_est_reconcilie_a_temps_constant() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);
    // Ancre valide 0,75 ; miroir Q1.15 = 0 (obsolète).
    sub.set_node_property(n, "_cog_energy_anchor", ancre(0.75));
    sub.set_node_energy_f32(n, 0.0).unwrap();
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    // Temps constant (BASE, t_ref=BASE) : dt=0, pas de décroissance.
    let stats = decay_clock::advance_all(sub.as_ref(), &law, 0, BASE).unwrap();
    eprintln!("stats: {:?}", stats);
    assert_eq!(stats.advanced, 0, "pas de matérialisation canonique (dt=0)");
    assert_eq!(stats.reconciled, 1, "le miroir obsolète est réconcilié");

    // Le miroir est désormais Q1.15(0,75) ; l'ancre est inchangée.
    let miroir = sub.get_node_energy_f32(n).unwrap().unwrap();
    eprintln!("miroir après réconciliation = {miroir}");
    assert!((miroir - 0.75).abs() < 1e-4, "miroir réparé à ~0,75 : {miroir}");
    let a = decay_clock::read_anchor(&sub, n).unwrap();
    assert!((a.e_ref - 0.75).abs() < 1e-9, "ancre inchangée (0,75)");

    // Une seconde consolidation ne produit AUCUNE écriture superflue.
    let stats2 = decay_clock::advance_all(sub.as_ref(), &law, 0, BASE).unwrap();
    assert_eq!(stats2.advanced + stats2.reconciled, 0, "miroir déjà conforme : aucune écriture");
}
