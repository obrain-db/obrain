//! OPÉRATEUR NATIF RÉUTILISABLE (`RevisionOp`) : une séquence ordonnée de pas
//! de reconsolidation avec préconditions (validation des paramètres à chaque
//! pas) et effets explicites. On distingue :
//!   - ÉQUIVALENCE DU RÉSULTAT MATHÉMATIQUE (même état canonique final) ;
//!   - ÉQUIVALENCE DES HISTORIQUES (recall_count, révision globale).
//!
//! On n'exige pas que toutes les propriétés tiennent : on identifie les
//! conditions.

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::{
    AdaptiveRevisionParams, EngramManager, EngramManagerConfig, ObservationScope, RevisionOp,
    RevisionStep,
};
use obrain_cognitive::InMemoryVectorIndex;

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

fn step(nodes: &[NodeId], err: f64, scope: ObservationScope) -> RevisionStep {
    RevisionStep {
        nodes: nodes.to_vec(),
        prediction_error: err,
        scope,
        params: AdaptiveRevisionParams::default(),
    }
}

fn set_weight(m: &EngramManager, eid: obrain_cognitive::engram::EngramId, node: NodeId, w: f64) {
    m.store().update(eid, |e| {
        if let Some((_, x)) = e.ensemble.iter_mut().find(|(n, _)| *n == node) {
            *x = w;
        }
    });
}

/// État complet réduit : ensemble (trié), recall_count, révision globale.
fn snapshot(m: &EngramManager, eid: obrain_cognitive::engram::EngramId) -> (Vec<(u64, f64)>, u32, u64) {
    let e = m.store().get(eid).expect("engramme");
    let mut ens: Vec<(u64, f64)> = e.ensemble.iter().map(|(n, w)| (n.0, *w)).collect();
    ens.sort_by_key(|(n, _)| *n);
    (ens, e.recall_count, m.store().revision())
}

// E1 — séquentiel == matérialisé (résultat ET historique).
#[test]
fn operateur_sequentiel_equivaut_materialise() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let op1 = RevisionOp::new().push(step(&[a, b], 0.4, ObservationScope::DeclaredPartial));
    let op2 = RevisionOp::new().push(step(&[a, b, c], 0.4, ObservationScope::DeclaredPartial));

    let m1 = manager();
    let e1 = m1.form_observation(&[a, b]);
    set_weight(&m1, e1, a, 0.2);
    let r1 = op1.apply(&m1, e1).expect("op1");
    let r2 = op2.apply(&m1, e1).expect("op2");
    let s_seq = snapshot(&m1, e1);

    let m2 = manager();
    let e2 = m2.form_observation(&[a, b]);
    set_weight(&m2, e2, a, 0.2);
    let comp = RevisionOp::compose(&op1, &op2);
    let r_comp = comp.apply(&m2, e2).expect("compose");
    let s_comp = snapshot(&m2, e2);

    eprintln!("[op E1] séquentiel={:?} matérialisé={:?}", s_seq, s_comp);
    assert_eq!(s_seq, s_comp, "séquentiel == matérialisé (résultat ET historique)");
    assert_eq!(r1.len() + r2.len(), r_comp.len(), "même nombre de pas");
}

// E2 — regroupement différent (associativité de la composition matérialisée).
#[test]
fn operateur_regroupement_associatif() {
    let (a, b) = (NodeId(1), NodeId(2));
    let o1 = RevisionOp::new().push(step(&[a, b], 0.4, ObservationScope::DeclaredPartial));
    let o2 = RevisionOp::new().push(step(&[b], 0.4, ObservationScope::DeclaredComplete)); // décroît a
    let o3 = RevisionOp::new().push(step(&[a], 0.4, ObservationScope::DeclaredPartial));

    let run = |op: &RevisionOp| {
        let m = manager();
        let e = m.form_observation(&[a, b]);
        set_weight(&m, e, a, 0.2);
        op.apply(&m, e).expect("apply");
        snapshot(&m, e)
    };
    let left = run(&RevisionOp::compose(&RevisionOp::compose(&o1, &o2), &o3));
    let right = run(&RevisionOp::compose(&o1, &RevisionOp::compose(&o2, &o3)));

    eprintln!("[op E2] (o1∘o2)∘o3={:?} ; o1∘(o2∘o3)={:?}", left, right);
    assert_eq!(left, right, "regroupement différent -> même état (associativité matérialisée)");
}

// E3 — dépendance réelle : un pas dépend de l'effet du précédent.
#[test]
fn operateur_dependance_reelle() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let op = RevisionOp::new()
        .push(step(&[a, b, c], 0.4, ObservationScope::DeclaredPartial)) // ajoute c
        .push(step(&[c], 0.4, ObservationScope::DeclaredPartial));       // renforce c
    let m = manager();
    let e = m.form_observation(&[a, b]);
    op.apply(&m, e).expect("apply");
    let ens = m.store().get(e).unwrap().ensemble.clone();
    assert!(
        ens.iter().any(|(n, _)| *n == c),
        "le second pas dépend de l'ajout de c par le premier (c présent)"
    );
    eprintln!("[op E3] dépendance réelle : ensemble = {:?}", ens);
}

// E4 — erreur intermédiaire : application PARTIELLE (pas atomique).
#[test]
fn operateur_erreur_intermediaire_partielle() {
    let (a, b) = (NodeId(1), NodeId(2));
    let bad = RevisionStep {
        nodes: vec![a],
        prediction_error: 0.4,
        scope: ObservationScope::DeclaredPartial,
        params: AdaptiveRevisionParams {
            lr_base: f64::NAN,
            lr_error_gain: 0.5,
            lr_max: 0.8,
            floor: 0.05,
        },
    };
    let op = RevisionOp::new()
        .push(step(&[a, b], 0.4, ObservationScope::DeclaredPartial)) // OK
        .push(bad)                                                   // échoue (paramètres invalides)
        .push(step(&[a, b], 0.4, ObservationScope::DeclaredPartial)); // jamais atteint
    let m = manager();
    let e = m.form_observation(&[a]);
    set_weight(&m, e, a, 0.2);
    let r = op.apply(&m, e);
    assert!(r.is_err(), "l'opérateur s'arrête à la première erreur");
    let eng = m.store().get(e).unwrap();
    assert_eq!(eng.recall_count, 1, "application PARTIELLE : seul le premier pas est publié");
    assert!(eng.node_ids().contains(&b), "le premier pas a bien ajouté b");
    eprintln!("[op E4] erreur intermédiaire : recall_count={} (partiel, pas atomique)", eng.recall_count);
}

// E5 — conservation des invariants (identité, unicité, projection cohérente).
#[test]
fn operateur_conserve_les_invariants() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let op = RevisionOp::new()
        .push(step(&[a, b, c], 0.4, ObservationScope::DeclaredPartial))
        .push(step(&[a, b], 0.4, ObservationScope::DeclaredComplete)); // décroît c
    let m = manager();
    let e = m.form_observation(&[a, b]);
    op.apply(&m, e).expect("apply");
    let eng = m.store().get(e).unwrap();
    let mut seen = std::collections::HashSet::new();
    for (n, w) in &eng.ensemble {
        assert!(w.is_finite(), "poids fini");
        assert!(seen.insert(*n), "membre unique");
    }
    // Projection COHÉRENTE : signature publiée == encode des POIDS RÉELS.
    assert_eq!(
        eng.spectral_signature,
        m.encode_weighted(&eng.ensemble),
        "projection cohérente après chaque pas"
    );
    eprintln!("[op E5] invariants conservés (identité, unicité, projection)");
}

// E6 — résultat mathématique vs historique transactionnel.
#[test]
fn operateur_resultat_vs_historique() {
    let a = NodeId(1);
    // Un seul pas vs trois pas de même nature : le renforcement n'étant PAS
    // idempotent, le résultat mathématique DIFFÈRE, et l'historique aussi.
    let one = RevisionOp::new().push(step(&[a], 0.4, ObservationScope::DeclaredPartial));
    let three = RevisionOp::new()
        .push(step(&[a], 0.4, ObservationScope::DeclaredPartial))
        .push(step(&[a], 0.4, ObservationScope::DeclaredPartial))
        .push(step(&[a], 0.4, ObservationScope::DeclaredPartial));

    let m1 = manager();
    let e1 = m1.form_observation(&[a]);
    set_weight(&m1, e1, a, 0.2);
    one.apply(&m1, e1).expect("un pas");

    let m2 = manager();
    let e2 = m2.form_observation(&[a]);
    set_weight(&m2, e2, a, 0.2);
    three.apply(&m2, e2).expect("trois pas");

    let (ens1, rc1, _) = snapshot(&m1, e1);
    let (ens2, rc2, _) = snapshot(&m2, e2);
    eprintln!("[op E6] un pas w={:?} (recall {rc1}) ; trois pas w={:?} (recall {rc2})", ens1[0].1, ens2[0].1);
    assert_ne!(ens1[0].1, ens2[0].1, "résultat mathématique DIFFÉRENT (non idempotent)");
    assert_eq!(rc1, 1);
    assert_eq!(rc2, 3);
    // Équivalence de RÉSULTAT et équivalence d'HISTORIQUE sont deux notions
    // DISTINCTES : ici ni l'une ni l'autre ne tient.
}
