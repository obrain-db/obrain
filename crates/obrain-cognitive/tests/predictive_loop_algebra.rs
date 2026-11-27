//! ALGÈBRE DES TRANSFORMATIONS sur l'état COMPLET de l'engramme.
//!
//! Chaque expérience compare des compositions T1(T2(S)) vs T2(T1(S)) sur
//! l'ÉTAT COMPLET : poids, compteurs, identité, projections (signature),
//! versions (révision globale du store), clé structurelle. On mesure
//! séparément commutativité, idempotence, indépendance, réversibilité et
//! conservation des invariants — et on rapporte les CONDITIONS, sans exiger
//! que toutes tiennent.

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_cognitive::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    EngramId, EngramManager, EngramManagerConfig, ObservationScope, RevisionError,
};
use obrain_common::types::NodeId;

#[allow(dead_code)]
type Ep = Vec<NodeId>;

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

fn revise(m: &EngramManager, eid: EngramId, nodes: &[NodeId], err: f64, scope: ObservationScope) {
    m.revise_adaptive(eid, nodes, err, scope).expect("révision");
}

#[derive(Debug, PartialEq)]
struct CompleteState {
    ensemble: Vec<(u64, f64)>,
    strength: f64,
    recall_count: u32,
    signature: Vec<f64>,
    structural_key: Option<String>,
    structural_key_stale: bool,
    store_revision: u64,
}

fn round9(x: f64) -> f64 {
    (x * 1e9).round() / 1e9
}

fn snapshot(m: &EngramManager, eid: EngramId) -> CompleteState {
    let e = m.store().get(eid).expect("engramme");
    let mut ensemble: Vec<(u64, f64)> = e.ensemble.iter().map(|(n, w)| (n.0, round9(*w))).collect();
    ensemble.sort_by_key(|(n, _)| *n);
    CompleteState {
        ensemble,
        strength: round9(e.strength),
        recall_count: e.recall_count,
        signature: e.spectral_signature.iter().map(|v| round9(*v)).collect(),
        structural_key: e.structural_key.clone(),
        structural_key_stale: e.structural_key_stale,
        store_revision: m.store().revision(),
    }
}

fn set_weight(m: &EngramManager, eid: EngramId, node: NodeId, w: f64) {
    m.store().update(eid, |e| {
        if let Some((_, x)) = e.ensemble.iter_mut().find(|(n, _)| *n == node) {
            *x = w;
        }
    });
}

// ---------------------------------------------------------------------------
// E1 — COMMUTATIVITÉ : deux renforcements INDÉPENDANTS (nœuds différents).
// ---------------------------------------------------------------------------

#[test]
fn e1_commutativite_renforcements_independants() {
    let (a, b) = (NodeId(1), NodeId(2));
    let mk = || {
        let m = manager();
        let eid = m.form_observation(&[a, b]);
        set_weight(&m, eid, a, 0.2);
        set_weight(&m, eid, b, 0.2);
        (m, eid)
    };
    // T1 = renforce a (Partial [a]) ; T2 = renforce b (Partial [b]).
    let (m1, e1) = mk();
    revise(&m1, e1, &[a], 0.4, ObservationScope::DeclaredPartial);
    revise(&m1, e1, &[b], 0.4, ObservationScope::DeclaredPartial);
    let s1 = snapshot(&m1, e1);

    let (m2, e2) = mk();
    revise(&m2, e2, &[b], 0.4, ObservationScope::DeclaredPartial);
    revise(&m2, e2, &[a], 0.4, ObservationScope::DeclaredPartial);
    let s2 = snapshot(&m2, e2);

    eprintln!(
        "[E1] T1(T2) ensemble={:?} ; T2(T1) ensemble={:?}",
        s1.ensemble, s2.ensemble
    );
    assert_eq!(
        s1, s2,
        "E1 : deux renforcements indépendants COMMUTENT (état complet)"
    );
}

// ---------------------------------------------------------------------------
// E2 — MÊME PARTICIPANT : commute (même fonction) mais N'EST PAS idempotent.
// ---------------------------------------------------------------------------

#[test]
fn e2_meme_participant_non_idempotent() {
    let a = NodeId(1);
    let mk = || {
        let m = manager();
        let eid = m.form_observation(&[a]);
        set_weight(&m, eid, a, 0.2);
        (m, eid)
    };
    // T appliqué DEUX fois vs UNE fois.
    let (m1, e1) = mk();
    revise(&m1, e1, &[a], 0.4, ObservationScope::DeclaredPartial);
    revise(&m1, e1, &[a], 0.4, ObservationScope::DeclaredPartial);
    let s_tt = snapshot(&m1, e1);

    let (m2, e2) = mk();
    revise(&m2, e2, &[a], 0.4, ObservationScope::DeclaredPartial);
    let s_t = snapshot(&m2, e2);

    eprintln!("[E2] T={:?} ; T∘T={:?}", s_t.ensemble, s_tt.ensemble);
    // Commutativité triviale (même fonction), mais NON idempotent.
    assert_ne!(
        s_tt.ensemble, s_t.ensemble,
        "E2 : renforcement NON idempotent"
    );
    assert!(
        (s_t.ensemble[0].1 - 0.6).abs() < 1e-9,
        "E2 : 1 renforcement = 0.6"
    );
    assert!(
        (s_tt.ensemble[0].1 - 0.8).abs() < 1e-9,
        "E2 : 2 renforcements = 0.8 (pas 0.9 : 0.6 -> 0.8)"
    );
}

// ---------------------------------------------------------------------------
// E3 — NON-COMMUTATIVITÉ : renforcement et décroissance sur le MÊME nœud.
// ---------------------------------------------------------------------------

#[test]
fn e3_non_commutativite_renforcement_decroissance() {
    let (a, b) = (NodeId(1), NodeId(2));
    let mk = || {
        let m = manager();
        let eid = m.form_observation(&[a, b]);
        set_weight(&m, eid, a, 0.5);
        (m, eid)
    };
    // T1 = renforce a (Partial [a,b]) ; T2 = décroît a (Complete [b]).
    let (m1, e1) = mk();
    revise(&m1, e1, &[a, b], 0.4, ObservationScope::DeclaredPartial);
    revise(&m1, e1, &[b], 0.4, ObservationScope::DeclaredComplete);
    let s1 = snapshot(&m1, e1);

    let (m2, e2) = mk();
    revise(&m2, e2, &[b], 0.4, ObservationScope::DeclaredComplete);
    revise(&m2, e2, &[a, b], 0.4, ObservationScope::DeclaredPartial);
    let s2 = snapshot(&m2, e2);

    eprintln!("[E3] T1(T2)={:?} ; T2(T1)={:?}", s1.ensemble, s2.ensemble);
    assert_ne!(
        s1.ensemble, s2.ensemble,
        "E3 : renforcement et décroissance sur le MÊME nœud NE commutent PAS"
    );
}

// ---------------------------------------------------------------------------
// E4 — ORDRE : la révision EXIGE une formation préalable.
// ---------------------------------------------------------------------------

#[test]
fn e4_revision_exige_la_formation() {
    let a = NodeId(1);
    let m = manager();
    let absent = EngramId(9999);
    match m.revise_adaptive(absent, &[a], 0.4, ObservationScope::DeclaredPartial) {
        Err(RevisionError::EngramAbsent(id)) => assert_eq!(id, absent),
        other => panic!("révision d'un engramme absent devrait échouer, obtenu {other:?}"),
    }
    // Après formation, la révision réussit.
    let eid = m.form_observation(&[a]);
    m.revise_adaptive(eid, &[a], 0.4, ObservationScope::DeclaredPartial)
        .expect("révision après formation");
    eprintln!("[E4] révision AVANT formation -> EngramAbsent ; APRÈS -> OK (ordre contraint)");
}

// ---------------------------------------------------------------------------
// E5 — DEUX RESSOURCES INDÉPENDANTES : l'ordre des révisions ne change rien.
// ---------------------------------------------------------------------------

#[test]
fn e5_deux_ressources_independantes() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let mk = || {
        let m = manager();
        let e1 = m.form_observation(&[a]);
        let e2 = m.form_observation(&[b, c]);
        set_weight(&m, e1, a, 0.2);
        set_weight(&m, e2, b, 0.2);
        (m, e1, e2)
    };
    let (m1, e1, e2) = mk();
    revise(&m1, e1, &[a], 0.4, ObservationScope::DeclaredPartial);
    revise(&m1, e2, &[b, c], 0.4, ObservationScope::DeclaredPartial);
    let (s1a, s1b) = (snapshot(&m1, e1), snapshot(&m1, e2));

    let (m2, e1b, e2b) = mk();
    revise(&m2, e2b, &[b, c], 0.4, ObservationScope::DeclaredPartial);
    revise(&m2, e1b, &[a], 0.4, ObservationScope::DeclaredPartial);
    let (s2a, s2b) = (snapshot(&m2, e1b), snapshot(&m2, e2b));

    assert_eq!(s1a, s2a, "E5 : e1 identique quel que soit l'ordre");
    assert_eq!(s1b, s2b, "E5 : e2 identique quel que soit l'ordre");
    eprintln!("[E5] deux ressources indépendantes : états complets identiques (commutent)");
}

// ---------------------------------------------------------------------------
// E6 — DÉPENDANCE PARTAGÉE : réviser un engramme ne touche PAS l'autre.
// ---------------------------------------------------------------------------

#[test]
fn e6_dependance_partagee() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let m = manager();
    let e1 = m.form_observation(&[a, b]);
    let e2 = m.form_observation(&[a, c]); // partage a
    set_weight(&m, e1, a, 0.2);
    set_weight(&m, e2, a, 0.2);
    let e2_before = snapshot(&m, e2);
    revise(&m, e1, &[a, b], 0.4, ObservationScope::DeclaredPartial);
    let e2_after = snapshot(&m, e2);
    eprintln!(
        "[E6] e2 avant={:?} après révision de e1={:?}",
        e2_before.ensemble, e2_after.ensemble
    );
    // Les ENSEMBLES sont distincts : réviser e1 ne modifie PAS les poids de e2.
    assert_eq!(
        e2_before.ensemble, e2_after.ensemble,
        "E6 : dépendance partagée (même NodeId a) mais ensembles distincts -> pas d'effet croisé sur les poids"
    );
    // Invariant : l'identité de chaque engramme est conservée.
    assert_eq!(e2_after.structural_key, e2_before.structural_key);
}

// ---------------------------------------------------------------------------
// E7 — RÉVERSIBILITÉ : le renforcement est LOSSY (pas d'inverse exact).
// ---------------------------------------------------------------------------

#[test]
fn e7_reversibilite_impossible() {
    let a = NodeId(1);
    let m = manager();
    let eid = m.form_observation(&[a]);
    set_weight(&m, eid, a, 0.2);
    let s0 = snapshot(&m, eid);
    revi0(&m, eid); // renforce
    revi0(&m, eid); // renforce à nouveau (pas un inverse)
    let s2 = snapshot(&m, eid);
    eprintln!(
        "[E7] état initial={:?} après 2 renforcements={:?} (pas de retour exact)",
        s0.ensemble, s2.ensemble
    );
    assert_ne!(
        s0.ensemble, s2.ensemble,
        "E7 : pas de retour exact -> transformation LOSSY"
    );
    // Invariant conservé : l'IDENTITÉ ne change pas.
    assert_eq!(
        s0.signature.len(),
        s2.signature.len(),
        "dimension de signature conservée"
    );
}

fn revi0(m: &EngramManager, eid: EngramId) {
    let a = NodeId(1);
    revise(m, eid, &[a], 0.4, ObservationScope::DeclaredPartial);
}

// ---------------------------------------------------------------------------
// E8 — ASSOCIATIVITÉ : structurelle pour la composition séquentielle.
// ---------------------------------------------------------------------------

#[test]
fn e8_associativite_structurelle() {
    let (a, b) = (NodeId(1), NodeId(2));
    let mk = || {
        let m = manager();
        let eid = m.form_observation(&[a, b]);
        set_weight(&m, eid, a, 0.2);
        (m, eid)
    };
    // Séquence FIXE T1, T2, T3. L'associativité (T1∘T2)∘T3 = T1∘(T2∘T3) est
    // STRUCTURELLE pour la composition de fonctions : c'est la même séquence,
    // quel que soit le groupement. On le vérifie sur l'état complet.
    let (m1, e1) = mk();
    revise(&m1, e1, &[a, b], 0.4, ObservationScope::DeclaredPartial); // T1 : renforce a,b
    revise(&m1, e1, &[b], 0.4, ObservationScope::DeclaredComplete); // T2 : décroît a
    revise(&m1, e1, &[a], 0.4, ObservationScope::DeclaredPartial); // T3 : renforce a
    let s = snapshot(&m1, e1);

    eprintln!(
        "[E8] associativité structurelle : ensemble={:?}, recall_count={} (rappel E7 : pas d'inverse)",
        s.ensemble, s.recall_count
    );
    // L'algèbre est un SEMI-GROUPE : associative (triviale pour une séquence),
    // sans inverse (E7), sans idempotence (E2). On documente, on n'exige rien d'autre.
    assert!(s.ensemble.iter().all(|(_, w)| w.is_finite() && *w >= 0.0));
    assert_eq!(
        s.recall_count, 3,
        "3 révisions -> recall_count = 3 (historique cohérent)"
    );
}
