//! Autorité conditionnelle native sur les mutations énergétiques : le
//! renforcement passe par capturer → calculer (valeur capturée + estampille HLC)
//! → préparer (Strict) → publier. Un `ConflitPrecondition` provoque un RECALCUL
//! depuis la nouvelle ancre — jamais une re-publication aveugle du delta additif.

#![cfg(feature = "substrate")]

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{IssuePublication, ModePublication, Precondition, SubstrateStore};

const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(64);
const ANCRE: &str = "_cog_energy_anchor";

fn law() -> DecayLaw {
    DecayLaw::new(H)
}

fn ancre_bytes(e: f64, t_ref: f64, t_hi: f64, h: f64) -> Value {
    let mut b = [0u8; 32];
    for (i, x) in [e, t_ref, t_hi, h].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(b.as_slice()))
}

fn t_ref_f64(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

/// Un renforcement est UNE application du delta : deux renforcements successifs
/// de +0.1 depuis 0.2 donnent 0.4 (le second RECALCULE depuis 0.3), jamais 0.5.
#[test]
fn renforcements_successifs_sans_double_renforcement() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id: NodeId = sub.create_node(&["n"]);
    decay_clock::persist_anchor_explicit(&sub, id, 0.2, BASE, H);
    sub.set_node_energy_f32(id, 0.2).unwrap();

    let r1 = decay_clock::reinforce(&sub, id, 0.1, &law(), 1.0, BASE);
    let r2 = decay_clock::reinforce(&sub, id, 0.1, &law(), 1.0, BASE);
    assert!((r1.unwrap() - 0.3).abs() < 1e-6, "premier renforcement → 0.3");
    assert!((r2.unwrap() - 0.4).abs() < 1e-6, "second renforcement → 0.4 (recalcul, pas 0.5)");

    let e = decay_clock::energy_at(&sub, id, &law(), BASE).unwrap();
    assert!((e - 0.4).abs() < 1e-6, "énergie finale 0.4, lue {e}");
}

/// Une re-publication depuis une capture OBSOLÈTE est refusée par la précondition
/// (estampille HLC, pas la valeur) : le delta additif n'est jamais appliqué deux
/// fois. C'est le garde-fou du chemin boost.
#[test]
fn une_republication_obsolete_est_refusee_par_la_precondition() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id: NodeId = sub.create_node(&["n"]);
    decay_clock::persist_anchor_explicit(&sub, id, 0.2, BASE, H);
    sub.set_node_energy_f32(id, 0.2).unwrap();

    // Capture OBSOLÈTE : vue de l'ancre 0.2.
    let pre0: Precondition = sub.capturer_precondition(id, &[ANCRE]).unwrap();

    // A (+0.1) publie 0.3 via le chemin conditionnel.
    let _ = decay_clock::reinforce(&sub, id, 0.1, &law(), 1.0, BASE);

    // B naïf : depuis la capture obsolète (0.2), il recalcule 0.2+0.1+0.1 = 0.4
    // (double application) et tente de la publier. La précondition la refuse.
    let ancre_04 = ancre_bytes(0.4, t_ref_f64(BASE), t_ref_f64(BASE), 64.0);
    let prep = sub
        .preparer_transformation(id, &[(ANCRE, ancre_04)], &pre0, ModePublication::Strict)
        .unwrap();
    let verdict = sub.publier_si_precondition_intacte(prep).unwrap();
    assert_eq!(verdict, IssuePublication::ConflitPrecondition, "delta additif obsolète refusé");

    // L'ancre n'a pas bougé : toujours 0.3, le +0.1 de B n'a PAS été ré-appliqué.
    let e = decay_clock::energy_at(&sub, id, &law(), BASE).unwrap();
    assert!((e - 0.3).abs() < 1e-6, "pas de double renforcement : 0.3, lue {e}");
}

/// Huit renforcements concurrents de +0.05 depuis 0.2 : le total vaut exactement
/// 0.2 + 8×0.05 = 0.6. Tout conflit est résolu par RECALCUL depuis la nouvelle
/// ancre — ni perte, ni double comptage.
#[test]
fn renforcements_concurrents_sans_double_comptage() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id: NodeId = sub.create_node(&["n"]);
    decay_clock::persist_anchor_explicit(&sub, id, 0.2, BASE, H);
    sub.set_node_energy_f32(id, 0.2).unwrap();

    const N: usize = 8;
    const DELTA: f64 = 0.05;
    let barrier = Arc::new(Barrier::new(N));
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let sub = Arc::clone(&sub);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            decay_clock::reinforce(&sub, id, DELTA, &law(), 1.0, BASE)
        }));
    }
    for h in handles {
        let r = h.join().unwrap();
        assert!(r.is_some(), "chaque renforcement concurrent doit aboutir");
    }

    let e = decay_clock::energy_at(&sub, id, &law(), BASE).unwrap();
    let attendu = 0.2 + (N as f64) * DELTA;
    assert!(
        (e - attendu).abs() < 1e-3,
        "8×+0.05 depuis 0.2 = 0.6 (ni perte ni double), lu {e}"
    );
}
