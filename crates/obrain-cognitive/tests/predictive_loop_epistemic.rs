//! Épistémique : absence / ignorance / contradiction, préservation sous
//! observation partielle, sensibilité des paramètres, identité structurelle,
//! distracteurs et hypothèses concurrentes.
//!
//! Corrige le défaut de la révision non monotone : décroître un nœud ABSENT à
//! chaque cycle détruisait des connaissances simplement NON OBSERVÉES. La
//! portée épistémique (`ObservationScope`) sépare désormais :
//!   Complete -> l'absence est une PREUVE (décroissance/retrait permis) ;
//!   Partial  -> l'absence est une NON-OBSERVATION (préservation).

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::{
    AdaptiveRevisionParams, EngramManager, EngramManagerConfig, ObservationScope, PredictivePolicy,
};
use obrain_cognitive::InMemoryVectorIndex;

type Ep = Vec<NodeId>;

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

fn members(m: &EngramManager, eid: obrain_cognitive::engram::EngramId) -> Vec<NodeId> {
    m.store().get(eid).expect("engram").node_ids()
}

// ---------------------------------------------------------------------------
// G — absence / ignorance / contradiction / retour, SÉPARÉS.
// ---------------------------------------------------------------------------

#[test]
fn g_absence_ignorance_contradiction_et_retour() {
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    // (C) INFORMATION INCONNUE : observation vide -> refusée, rien n'est détruit.
    let m = manager();
    let eid = m.form_observation(&[a, b]);
    let before = members(&m, eid);
    assert!(
        m.predictive_step(&[a], &[], 8).is_err(),
        "observation vide refusée (information indisponible)"
    );
    assert_eq!(members(&m, eid), before, "inconnu : engram PRÉSERVÉ");

    // (B) OBSERVATION PARTIELLE {a} : b non observé mais PRÉSERVÉ.
    let m = manager();
    let eid = m.form_observation(&[a, b]);
    for _ in 0..10 {
        let _ = m
            .revise_adaptive(eid, &[a], 0.9, ObservationScope::Partial)
            .expect("révision partielle");
    }
    let m_partial = members(&m, eid);
    assert!(
        m_partial.contains(&b),
        "PARTIEL : b non observé mais PRÉSERVÉ ({m_partial:?})"
    );
    eprintln!("[G] partiel : membres = {m_partial:?} (b préservé)");

    // (A) OBSERVATION COMPLÈTE {a} : b réellement absent -> décroît puis retiré.
    let m = manager();
    let eid = m.form_observation(&[a, b]);
    let mut steps_a = 0;
    for _ in 0..20 {
        let _ = m
            .revise_adaptive(eid, &[a], 0.9, ObservationScope::Complete)
            .expect("révision complète");
        steps_a += 1;
        if !members(&m, eid).contains(&b) {
            break;
        }
    }
    assert!(
        !members(&m, eid).contains(&b),
        "COMPLET : b retiré ({:?}) après {steps_a} pas",
        members(&m, eid)
    );

    // (E) RETOUR de l'ancien régime : b ré-ajouté.
    let _ = m
        .revise_adaptive(eid, &[a, b], 0.9, ObservationScope::Complete)
        .expect("révision retour");
    assert!(
        members(&m, eid).contains(&b),
        "RETOUR : b ré-ajouté ({:?})",
        members(&m, eid)
    );

    // (D) CONTRADICTION : {a,c} contredit b (b remplacé par c, a conservé).
    let m = manager();
    let eid = m.form_observation(&[a, b]);
    for _ in 0..20 {
        let _ = m
            .revise_adaptive(eid, &[a, c], 0.9, ObservationScope::Complete)
            .expect("révision contradiction");
        let mem = members(&m, eid);
        if !mem.contains(&b) && mem.contains(&c) {
            break;
        }
    }
    let m_contra = members(&m, eid);
    assert!(
        m_contra.contains(&a) && m_contra.contains(&c) && !m_contra.contains(&b),
        "CONTRADICTION : {{a,c}} remplace {{a,b}} ({m_contra:?})"
    );
    eprintln!("[G] contradiction : membres = {m_contra:?}");
}

// ---------------------------------------------------------------------------
// G' — duplicats et invariance à l'ordre.
// ---------------------------------------------------------------------------

#[test]
fn gprime_duplicats_et_invariance_ordre() {
    let (a, b) = (NodeId(1), NodeId(2));
    let m = manager();

    // Duplicats dans l'observation : pas de membre en double.
    let eid = m.form_observation(&[a, b, a, b, a]);
    assert_eq!(members(&m, eid).len(), 2, "duplicats -> membres uniques");

    // {a,b} et {b,a} : même engramme (l'ordre n'a pas de sens ici).
    let eid2 = m.form_observation(&[b, a]);
    assert_eq!(eid, eid2, "invariance à l'ordre : même engramme");

    // Observation dupliquée en révision : pas de croissance parasite.
    let _ = m
        .revise_adaptive(eid, &[a, b, a, b], 0.5, ObservationScope::Complete)
        .expect("révision dupliquée");
    assert_eq!(members(&m, eid).len(), 2, "observation dupliquée -> 2 membres");
    eprintln!("[G'] duplicats/ordre : {:?}", members(&m, eid));
}

// ---------------------------------------------------------------------------
// G'' — SENSIBILITÉ des paramètres de décroissance / retrait.
// ---------------------------------------------------------------------------

#[test]
fn gpp_sensibilite_des_parametres() {
    let (a, b) = (NodeId(1), NodeId(2));
    let configs = [
        (0.1, 0.05),
        (0.3, 0.05),
        (0.6, 0.05),
        (0.3, 0.20),
        (0.3, 0.01),
    ];
    let mut all_steps = Vec::new();
    for (lr_base, floor) in configs {
        let m = manager();
        let eid = m.form_observation(&[a, b]);
        let params = AdaptiveRevisionParams {
            lr_base,
            lr_error_gain: 0.5,
            lr_max: 0.8,
            floor,
        };
        let mut steps = 0;
        for _ in 0..60 {
            let _ = m
                .revise_adaptive_with(eid, &[a], 0.9, ObservationScope::Complete, params)
                .expect("révision paramétrée");
            steps += 1;
            if !members(&m, eid).contains(&b) {
                break;
            }
        }
        let removed = !members(&m, eid).contains(&b);
        eprintln!("[G''] lr_base={lr_base} floor={floor} -> retrait={removed} en {steps} pas");
        assert!(removed, "avec lr_base={lr_base} floor={floor}, b finit retiré");
        all_steps.push(steps);
    }
    assert!(
        all_steps.iter().min() != all_steps.iter().max(),
        "les constantes CHANGENT réellement la vitesse de retrait : {all_steps:?}"
    );
}

// ---------------------------------------------------------------------------
// H — identité STRUCTURELLE vs ensemble de participants.
// ---------------------------------------------------------------------------

#[test]
fn h_identite_structurelle_vs_participants() {
    let (a, b) = (NodeId(1), NodeId(2));
    let m = manager();

    // Mêmes participants {a,b}, clés structurelles DIFFÉRENTES (directions opposées).
    let e_ab = m.form_observation_with(&[a, b], Some("cause:a->b"));
    let e_ba = m.form_observation_with(&[a, b], Some("cause:b->a"));
    let e_ab2 = m.form_observation_with(&[a, b], Some("cause:a->b"));

    assert_eq!(e_ab, e_ab2, "même clé structurelle -> même engramme");
    assert_ne!(
        e_ab, e_ba,
        "mêmes participants mais clés différentes -> engrams DISTINCTS"
    );
    assert_eq!(m.store().count(), 2, "deux engrams pour {{a,b}}");
    let keys: Vec<Option<String>> = m.store().list().iter().map(|e| e.structural_key.clone()).collect();
    eprintln!("[H] engrams={{a,b}} : {keys:?}");
    assert!(keys.iter().any(|k| k.as_deref() == Some("cause:a->b")));
    assert!(keys.iter().any(|k| k.as_deref() == Some("cause:b->a")));
}

// ---------------------------------------------------------------------------
// I — distracteurs et hypothèses concurrentes.
// ---------------------------------------------------------------------------

#[test]
fn i_distracteurs_et_hypotheses_concurrentes() {
    let (ctx, a, b, c, d) = (NodeId(10), NodeId(1), NodeId(2), NodeId(3), NodeId(4));

    // DISTRACTEURS : le motif {ctx,a,b} est observé avec un nœud de bruit VARIABLE.
    let m = manager();
    for i in 0..8 {
        let noise = NodeId(200 + i);
        let obs: Ep = vec![ctx, a, b, noise];
        let _ = m.predictive_step(&[ctx], &obs, 8).expect("pas distrait");
    }
    let truth: Ep = vec![ctx, a, b];
    let p = m.predict(&[ctx], 8).expect("prédiction");
    let err = m
        .prediction_error(&p.predicted, &m.encode_observation(&truth))
        .expect("dim");
    eprintln!(
        "[I] distracteurs variables : erreur vs motif propre = {:.4} ; engrams = {}",
        err,
        m.store().count()
    );
    assert!(m.store().count() >= 1, "un engramme s'est formé malgré le bruit");

    // HYPOTHÈSES CONCURRENTES : même indice, deux issues distinctes. En
    // formation épisodique elles restent SÉPARÉES (pas de fusion).
    let m2 = manager();
    let o1: Ep = vec![ctx, a, b];
    let o2: Ep = vec![ctx, c, d];
    for _ in 0..3 {
        let _ = m2
            .predictive_step_with_policy(&[ctx], &o1, 8, PredictivePolicy::Episodic)
            .expect("pas o1");
        let _ = m2
            .predictive_step_with_policy(&[ctx], &o2, 8, PredictivePolicy::Episodic)
            .expect("pas o2");
    }
    let recalled = m2.recall(&[ctx], 8);
    let e1 = m2
        .prediction_error(&m2.predict(&[ctx], 8).expect("pred").predicted, &m2.encode_observation(&o1))
        .expect("dim");
    let e2 = m2
        .prediction_error(&m2.predict(&[ctx], 8).expect("pred").predicted, &m2.encode_observation(&o2))
        .expect("dim");
    let n = m2.store().count();
    eprintln!(
        "[I] hypothèses concurrentes : engrams={n} rappelés={} ; erreur vs o1={:.4} vs o2={:.4}",
        recalled.len(),
        e1,
        e2
    );
    assert_eq!(n, 2, "deux hypothèses concurrentes = deux engrams distincts");
    assert_eq!(recalled.len(), 2, "le rappel ramène les deux hypothèses");
    // La prédiction est un mélange : elle ne peut pas satisfaire les deux.
    assert!(e1 > 0.05 && e2 > 0.05, "hypothèses contradictoires : résidu des deux côtés");
}
