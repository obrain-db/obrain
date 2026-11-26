//! CONTRE-ÉPREUVE de publication : `reconsolidate`/`reconsolidate_adaptive`
//! reçoivent `&mut Engram` et NE PUBLIENT RIEN eux-mêmes. C'est l'appelant qui
//! prend l'objet modifié et le publie. On identifie le chemin RÉEL qui publie
//! SANS recalculer la projection, et on audite `record_activation`.
//!
//! Invariants distingués : ÉTAT CANONIQUE (id, membres, poids), PROJECTION
//! (signature spectrale), HISTORIQUE (recall_count, recall_history).

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::{reconsolidate, EngramManager, EngramManagerConfig};
use obrain_cognitive::InMemoryVectorIndex;

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

/// CONTRE-ÉPREUVE EXÉCUTÉE : `store.update(eid, |e| reconsolidate(e, ...))`
/// publie un ensemble MODIFIÉ avec une signature NON RECALCULÉE (périmée).
/// `revise` (chemin canonique), lui, recalcule la projection.
#[test]
fn contre_preuve_publication_sans_recalcul_projection() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    // --- Chemin DIRECT (celui de predictive_loop.rs:131) ---
    let m = manager();
    let eid = m.form_observation(&[a, b]);
    let sig_before = m.store().get(eid).unwrap().spectral_signature.clone();
    assert_eq!(
        sig_before,
        m.encode_observation(&[a, b]),
        "formation : signature cohérente (encode uniforme)"
    );

    // reconsolidate reçoit &mut Engram, le MUTe, et ne publie rien ; c'est
    // store.update qui publie, SANS recalcul de projection.
    m.store().update(eid, |e| reconsolidate(e, &[c], 0.5));
    let after = m.store().get(eid).unwrap();
    assert!(
        after.node_ids().contains(&c),
        "l'ensemble a bien changé (c ajouté) : {:?}",
        after.node_ids()
    );
    assert_eq!(
        after.spectral_signature, sig_before,
        "CONTRE-ÉPREUVE : la signature n'a PAS été recalculée (périmée)"
    );
    eprintln!(
        "[publication] chemin direct : ensemble {:?} -> signature INCHANGÉE (périmée)",
        after.node_ids()
    );

    // --- Chemin CANONIQUE : revise recalcule la projection ---
    let m2 = manager();
    let eid2 = m2.form_observation(&[a, b]);
    let sig2_before = m2.store().get(eid2).unwrap().spectral_signature.clone();
    let _ = m2.revise(eid2, &[c], 0.5).expect("révision");
    let after2 = m2.store().get(eid2).unwrap();
    assert_ne!(
        after2.spectral_signature, sig2_before,
        "revise RECALCULE la signature (projection cohérente)"
    );
    assert!(after2.node_ids().contains(&c), "ensemble révisé");
    eprintln!("[publication] chemin canonique (revise) : signature RECALCULÉE");
}

/// AUDIT de `record_activation` : la formation recalcule bien la signature.
#[test]
fn record_activation_publie_une_signature_coherente() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let m = manager();
    for _ in 0..3 {
        m.record_activation(&[a, b, c], 0.5);
    }
    // L'engramme {a,b,c} formé par co-activation porte une signature RECALCULÉE.
    let formed = m
        .store()
        .list()
        .into_iter()
        .find(|e| {
            let mut s: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
            s.sort_unstable();
            s == vec![a.0, b.0, c.0]
        })
        .expect("engramme {{a,b,c}} formé");
    assert_eq!(
        formed.spectral_signature,
        m.encode_observation(&[a, b, c]),
        "record_activation recalcule la signature (cohérente, poids uniformes)"
    );
    eprintln!("[publication] record_activation : signature cohérente");
}

/// TROIS CATÉGORIES d'invariants distinguées, dont la PROJECTION peut être
/// violée par le chemin direct.
#[test]
fn invariants_trois_categories() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let m = manager();
    let eid = m.form_observation(&[a, b]);

    // (1) INVARIANT DE L'ÉTAT CANONIQUE : id, unicité des membres, poids finis.
    let e = m.store().get(eid).unwrap();
    assert_eq!(e.id, eid);
    let mut seen = std::collections::HashSet::new();
    for (n, w) in &e.ensemble {
        assert!(w.is_finite(), "poids fini");
        assert!(seen.insert(*n), "membre unique");
    }

    // (2) INVARIANT DE PROJECTION : signature cohérente à la formation.
    assert_eq!(e.spectral_signature, m.encode_observation(&e.node_ids()));

    // (3) INVARIANT D'HISTORIQUE : recall_count croît à chaque révision.
    let r0 = e.recall_count;
    let _ = m.revise(eid, &[c], 0.5).expect("révision");
    let e2 = m.store().get(eid).unwrap();
    assert!(e2.recall_count > r0, "recall_count croît");

    // VIOLATION de (2) par le chemin direct (documentée, pas corrigée ici) :
    let m2 = manager();
    let eid2 = m2.form_observation(&[a, b]);
    let sig = m2.store().get(eid2).unwrap().spectral_signature.clone();
    m2.store().update(eid2, |e| reconsolidate(e, &[c], 0.5));
    assert_eq!(
        m2.store().get(eid2).unwrap().spectral_signature, sig,
        "projection PÉRIMÉE : l'invariant de projection est VIOLABLE par store.update(reconsolidate)"
    );
    eprintln!("[invariants] canonique=OK, projection=VIOLABLE (chemin direct), historique=monotone");
}
