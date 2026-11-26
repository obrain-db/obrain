//! CONTRAT DE PROJECTION (INJECTABLE) : la signature est recalculée par le
//! store depuis l'ensemble (POIDS COMPRIS), pas par une comparaison de vecteurs.
//!
//! Contre-épreuves :
//!  (a) un changement de POIDS (ou de nœuds) sans recalcul -> la signature est
//!      RECALCULÉE (jamais périmée) ;
//!  (b) une signature arbitrairement DIFFÉRENTE -> ÉCRASÉE par le recalcul ;
//!  (c) une opération légitime sans changement -> aucun faux rejet.
//! La projection est INJECTABLE (aucun encodeur n'est imposé au store).

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

/// Contre-épreuves (a)(b)(c) : le contrat de projection recalculE, n'écrase que
/// ce qui doit l'être, et ne rejette pas à tort.
#[test]
fn projection_recalculee_par_le_contrat() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let m = manager();
    let eid = m.form_observation(&[a, b]);

    // (a) mutateur SANS recalcul + changement d'ensemble -> signature RECALCULÉE.
    m.store().update(eid, |e| reconsolidate(e, &[c], 0.5));
    let eng = m.store().get(eid).unwrap();
    assert!(
        eng.node_ids().contains(&c),
        "le mutateur est accepté (c ajouté) — plus de rejet par différence de vecteurs"
    );
    assert_eq!(
        eng.spectral_signature,
        m.encode_weighted(&eng.ensemble),
        "(a) signature RECALCULÉE sur les POIDS réels"
    );

    // (b) signature arbitrairement différente -> ÉCRASÉE par le recalcul.
    let dim = eng.spectral_signature.len();
    m.store().update(eid, |e| {
        e.spectral_signature = vec![0.0; dim];
    });
    let eng2 = m.store().get(eid).unwrap();
    assert_eq!(
        eng2.spectral_signature,
        m.encode_weighted(&eng2.ensemble),
        "(b) signature arbitraire ÉCRASÉE (le contrat ne compare pas deux vecteurs)"
    );

    // (c) opération sans changement -> aucun faux rejet, signature inchangée.
    let before = m.store().get(eid).unwrap().spectral_signature.clone();
    m.store().update(eid, |_e| {});
    let eng3 = m.store().get(eid).unwrap();
    assert_eq!(
        eng3.spectral_signature, before,
        "(c) pas de faux rejet : opération légitime inchangée"
    );
    eprintln!("[projection] (a) recalculé (b) écrasé (c) pas de faux rejet");
}

/// AUDIT de `record_activation` : la formation produit une signature cohérente.
#[test]
fn record_activation_publie_une_signature_coherente() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let m = manager();
    for _ in 0..3 {
        m.record_activation(&[a, b, c], 0.5);
    }
    let formed = m
        .store()
        .list()
        .into_iter()
        .find(|e| {
            let mut s: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
            s.sort_unstable();
            s == vec![a.0, b.0, c.0]
        })
        .expect("engramme {a,b,c} formé");
    assert_eq!(
        formed.spectral_signature,
        m.encode_weighted(&formed.ensemble),
        "record_activation : signature cohérente (encode des poids réels)"
    );
    eprintln!("[projection] record_activation : signature cohérente");
}

/// TROIS CATÉGORIES d'invariants : canonique, projection (désormais ENFORCÉE
/// par recalcul), historique.
#[test]
fn invariants_trois_categories() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let m = manager();
    let eid = m.form_observation(&[a, b]);

    // (1) ÉTAT CANONIQUE : id, unicité, poids finis.
    let e = m.store().get(eid).unwrap();
    assert_eq!(e.id, eid);
    let mut seen = std::collections::HashSet::new();
    for (n, w) in &e.ensemble {
        assert!(w.is_finite(), "poids fini");
        assert!(seen.insert(*n), "membre unique");
    }

    // (2) PROJECTION : cohérente à la formation ET après mutation (recalcul).
    assert_eq!(e.spectral_signature, m.encode_weighted(&e.ensemble));

    // (3) HISTORIQUE : recall_count croît.
    let r0 = e.recall_count;
    let _ = m.revise(eid, &[c], 0.5).expect("révision");
    let e2 = m.store().get(eid).unwrap();
    assert!(e2.recall_count > r0, "recall_count croît");
    assert_eq!(
        e2.spectral_signature,
        m.encode_weighted(&e2.ensemble),
        "projection recalculée après revise"
    );

    // Le chemin direct store.update(reconsolidate) est maintenant ASSAINI par
    // le recalcul : plus de publication périmée possible.
    let m2 = manager();
    let eid2 = m2.form_observation(&[a, b]);
    m2.store().update(eid2, |e| reconsolidate(e, &[c], 0.5));
    let e3 = m2.store().get(eid2).unwrap();
    assert!(e3.node_ids().contains(&c), "c ajouté (mutateur accepté)");
    assert_eq!(
        e3.spectral_signature,
        m2.encode_weighted(&e3.ensemble),
        "projection ENFORCÉE par recalcul (plus de signature périmée)"
    );
    eprintln!("[invariants] canonique=OK, projection=ENFORCÉE (recalcul), historique=monotone");
}
