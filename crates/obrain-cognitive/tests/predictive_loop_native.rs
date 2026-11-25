//! Boucle prédictive NATIVE via `EngramManager::predict` / `EngramManager::revise`.
//!
//! Contrat scientifique (mission #4, branche feat/predictive-loop) :
//! 1. Prédiction AVANT observation — `EngramManager::predict(cues, k)`.
//! 2. Observation INDÉPENDANTE — encodée par un `SpectralEncoder` dédié
//!    (même spécification que l'interne, jamais lue depuis le manager).
//! 3. Erreur RÉELLE — distance L2 entre embedding prédit et embedding observé
//!    (pas une erreur arbitraire choisie pour faire « apprendre »).
//! 4. Révision — `EngramManager::revise(engram_id, noeuds_observes, erreur)`.
//! 5. Comparaison aux baselines classiques (union cumulative, dernière
//!    observation, fréquentiel, témoin) dans le MÊME espace d'embedding.
//!
//! Honnêteté : la reconsolidation additive (`revise`) ne retire jamais de
//! nœud ; sur une séquence non monotone l'engramme converge vers l'union
//! cumulative. Ce fichier mesure ce que la boucle démontre (apprentissage
//! stationnaire, discrimination contextuelle) et ce qu'elle ne démontre pas.

#![cfg(feature = "engram")]

use std::collections::HashMap;
use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::{Engram, EngramManager, EngramManagerConfig, SpectralEncoder};
use obrain_cognitive::InMemoryVectorIndex;

type Ep = Vec<NodeId>;

fn make_manager() -> (EngramManager, SpectralEncoder) {
    let config = EngramManagerConfig::default();
    let index = Arc::new(InMemoryVectorIndex::new());
    let manager = EngramManager::new(config, index);
    // Encodeur dédié à l'observation — même spécification (64 dims, hachage
    // par défaut) que celui interne au manager, mais instance indépendante.
    let spectral = SpectralEncoder::new();
    (manager, spectral)
}

/// Distance L2 (les signatures sont L2-normalisées, donc distance ∈ [0, 2]).
fn l2(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}

/// Encode un ensemble observé (poids uniformes) dans l'espace spectral.
fn encode_set(spectral: &SpectralEncoder, nodes: &[NodeId]) -> Vec<f64> {
    let weighted: Vec<(NodeId, f64)> = nodes.iter().map(|&n| (n, 1.0)).collect();
    spectral.encode(&weighted)
}

// ---------------------------------------------------------------------------
// Baselines classiques — prédisent un ENSEMBLE depuis l'historique, encodé
// puis comparé à l'observation dans le même espace (distance L2).
// ---------------------------------------------------------------------------

fn baseline_union(history: &[Ep]) -> Ep {
    let mut acc: Ep = Vec::new();
    for e in history {
        for &n in e {
            if !acc.contains(&n) {
                acc.push(n);
            }
        }
    }
    acc
}

fn baseline_last(history: &[Ep]) -> Ep {
    history.last().cloned().unwrap_or_default()
}

fn baseline_freq_topk(history: &[Ep], k: usize) -> Ep {
    let mut counts: HashMap<NodeId, usize> = HashMap::new();
    for e in history {
        for &n in e {
            *counts.entry(n).or_insert(0) += 1;
        }
    }
    let mut v: Vec<(NodeId, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0 .0.cmp(&b.0 .0)));
    v.into_iter().take(k).map(|(n, _)| n).collect()
}

fn baseline_control(_history: &[Ep]) -> Ep {
    Vec::new()
}

/// Erreur d'une baseline : elle prédit un ensemble, qu'on encode puis compare.
fn baseline_error(
    spectral: &SpectralEncoder,
    predicted_set: &Ep,
    observed: &[NodeId],
) -> f64 {
    let p = encode_set(spectral, predicted_set);
    let o = encode_set(spectral, observed);
    l2(&p, &o)
}

// ---------------------------------------------------------------------------
// Pas adaptatif NATIF — prédiction avant observation, erreur réelle, révision.
// ---------------------------------------------------------------------------

/// Un pas de la boucle : prédit AVANT d'observer, mesure l'erreur réelle,
/// puis révise l'engramme dominant (ou nourrit la formation native au
/// démarrage à froid, quand aucun engramme n'est encore rappelable).
/// Retourne l'erreur réelle de ce pas.
fn adaptive_step(
    manager: &EngramManager,
    spectral: &SpectralEncoder,
    cue: &[NodeId],
    observed: &[NodeId],
) -> f64 {
    let dim = spectral.dimensions();

    // 1. PRÉDICTION (avant observation).
    let prediction = manager.predict(cue, 8);
    let predicted: Vec<f64> = prediction
        .as_ref()
        .map(|p| p.predicted.clone())
        .unwrap_or_else(|| vec![0.0; dim]);
    let dominant = prediction
        .as_ref()
        .and_then(|p| p.attention.first().map(|(id, _)| *id));

    // 2. OBSERVATION indépendante.
    let observed_emb = encode_set(spectral, observed);

    // 3. ERREUR RÉELLE (L2, pas arbitraire).
    let error = l2(&predicted, &observed_emb);

    // 4. RÉVISION native, ou formation au démarrage à froid.
    // Revise un engramme UNIQUEMENT si l observation le recoupe (co-activation
    // partagee). Sinon motif distinct -> formation. Sans ce garde, une
    // reconsolidation purement additive fusionnerait des contextes disjoints.
    let revise_target = dominant.filter(|eid| {
        manager
            .store()
            .get(*eid)
            .map(|e| e.node_ids().iter().any(|n| observed.contains(n)))
            .unwrap_or(false)
    });
    match revise_target {
        Some(eid) => {
            let _ = manager.revise(eid, observed, error);
        }
        None => {
            manager.record_activation(observed, error);
        }
    }

    error
}

// ---------------------------------------------------------------------------
// Test 1 — Contrôle positif stationnaire : l'erreur décroît puis s'annule.
// ---------------------------------------------------------------------------

#[test]
fn boucle_predictive_native_erreur_reelle_decroit() {
    let (manager, spectral) = make_manager();
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let observed: Ep = vec![a, b, c];
    let cue = vec![a];

    let mut errors = Vec::new();
    for _ in 0..6 {
        let e = adaptive_step(&manager, &spectral, &cue, &observed);
        errors.push(e);
    }

    eprintln!(
        "[stationnaire] erreurs L2 (6 pas) = {:.6?}",
        errors
    );

    // Démarrage à froid : aucune prédiction possible → erreur = norme de
    // l'observation (L2-normalisée) = 1.0.
    assert!(
        errors[0] > 0.99,
        "démarrage à froid : erreur ~1.0, obtenu {}",
        errors[0]
    );
    // Après formation (min_episodes=3), la prédiction épouse l'observation.
    assert!(
        errors[5] < 0.05,
        "après apprentissage, erreur ~0, obtenu {}",
        errors[5]
    );
    assert!(
        errors[5] < errors[0],
        "l'erreur décroît réellement ({} -> {})",
        errors[0],
        errors[5]
    );

    // La prédiction suivante rappelle bien l'engramme formé {a,b,c}.
    let prediction = manager
        .predict(&cue, 8)
        .expect("prédiction calculée après formation");
    assert!(
        !prediction.attention.is_empty(),
        "l'attention contient l'engramme rappelé"
    );
    let eid = prediction.attention[0].0;
    let members = manager
        .store()
        .get(eid)
        .expect("engramme présent")
        .node_ids();
    assert_eq!(members.len(), 3, "engramme {{a,b,c}} complet");
}

// ---------------------------------------------------------------------------
// Test 2 — Discrimination contextuelle : l'adaptatif conditionné au contexte
// bat les baselines AVEUGLES (union sur-prédit).
// ---------------------------------------------------------------------------

#[test]
fn discrimination_contextuelle_bat_baselines_aveugles() {
    let (manager, spectral) = make_manager();

    // Deux contextes DISJOINTS (aucun noeud partage).
    let (ctx1, ctx2) = (NodeId(101), NodeId(102));
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let obs1: Ep = vec![ctx1, a, b];
    let obs2: Ep = vec![ctx2, c, d];

    // Bootstrap deterministe : deux engrams disjoints {ctx1,a,b} et {ctx2,c,d},
    // indexes par leur signature spectrale (meme voie que le test unitaire du
    // manager). On contourne ainsi la formation non-idempotente
    // (should_form_engram reforme le meme motif tant que les compteurs >= 3),
    // limitation documentee separement dans la note de synthese.
    for nodes in [&obs1, &obs2] {
        let id = manager.store().next_id();
        let weighted: Vec<(NodeId, f64)> = nodes.iter().map(|&n| (n, 1.0)).collect();
        let mut engram = Engram::new(id, weighted);
        engram.spectral_signature = spectral.encode(&engram.ensemble.clone());
        manager.store().insert(engram);
    }

    // Evaluation : predire depuis le contexte, mesurer l erreur reelle (L2)
    // face a l observation, comparer aux baselines aveugles.
    let eval: Vec<(NodeId, Ep)> = vec![
        (ctx1, obs1.clone()),
        (ctx2, obs2.clone()),
        (ctx1, obs1.clone()),
        (ctx2, obs2.clone()),
    ];

    let mut history: Vec<Ep> = vec![obs1.clone(), obs2.clone(), obs1.clone(), obs2.clone()];
    let mut ad_errors = Vec::new();
    let mut union_errors = Vec::new();
    let mut last_errors = Vec::new();
    let mut freq_errors = Vec::new();
    let mut ctrl_errors = Vec::new();

    for (ctx, ep) in &eval {
        // Adaptatif : prediction AVANT observation, conditionnee au contexte.
        let p = manager.predict(&[*ctx], 8).expect("prediction");
        let e = l2(&p.predicted, &encode_set(&spectral, ep));
        ad_errors.push(e);

        // Baselines aveugles (historique seul, sans le contexte).
        let u = baseline_union(&history);
        union_errors.push(baseline_error(&spectral, &u, ep));
        let l = baseline_last(&history);
        last_errors.push(baseline_error(&spectral, &l, ep));
        let f = baseline_freq_topk(&history, 3);
        freq_errors.push(baseline_error(&spectral, &f, ep));
        ctrl_errors.push(baseline_error(&spectral, &baseline_control(&history), ep));

        history.push(ep.clone());
    }

    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    eprintln!("[contextuel] erreurs L2 adaptatif = {:.6?}", ad_errors);
    eprintln!(
        "[contextuel] erreurs L2 union={:.6?} last={:.6?} freq={:.6?} temoin={:.6?}",
        union_errors, last_errors, freq_errors, ctrl_errors
    );
    eprintln!(
        "[contextuel] moyennes adaptatif={:.4} union={:.4} last={:.4} freq={:.4} temoin={:.4}",
        mean(&ad_errors),
        mean(&union_errors),
        mean(&last_errors),
        mean(&freq_errors),
        mean(&ctrl_errors),
    );

    // 1. Discrimination de l ATTENTION : le top-1 de predict([ctx]) est bien
    //    l engram du contexte correspondant (pas l autre).
    for (ctx, ep) in &eval {
        let p = manager.predict(&[*ctx], 8).expect("prediction");
        let top = manager.store().get(p.attention[0].0).expect("engram");
        let top_set: std::collections::HashSet<u64> =
            top.node_ids().iter().map(|n| n.0).collect();
        let obs_set: std::collections::HashSet<u64> = ep.iter().map(|n| n.0).collect();
        assert_eq!(top_set, obs_set, "top-1 de predict([ctx]) = engram du contexte");
    }

    // 2+3. La prediction adaptative est plus proche du BON contexte que du
    //      MAUVAIS, et bat l union aveugle (qui sur-predit symetriquement).
    for (idx, (_ctx, ep)) in eval.iter().enumerate() {
        let ae = ad_errors[idx];
        let ue = union_errors[idx];
        let other = if ep[0] == ctx1 { &obs2 } else { &obs1 };
        let p = manager.predict(&[ep[0]], 8).expect("prediction");
        let e_other = l2(&p.predicted, &encode_set(&spectral, other));
        assert!(
            ae < e_other,
            "prediction plus proche du bon contexte ({} < {})",
            ae,
            e_other
        );
        assert!(ae < ue, "adaptatif ({}) < union aveugle ({})", ae, ue);
    }
}

// Test 3 — L'erreur est RÉELLE : elle pilote la reconsolidation via le seuil
// natif (0.1), pas via une valeur arbitraire.
// ---------------------------------------------------------------------------

#[test]
fn revision_pilotee_par_l_erreur_reelle() {
    let (manager, spectral) = make_manager();
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    // Forme {a,b} par co-activations répétées (chemin de formation natif).
    let pair: Ep = vec![a, b];
    for _ in 0..3 {
        adaptive_step(&manager, &spectral, &[a], &pair);
    }
    let eid = manager
        .predict(&[a], 8)
        .expect("prédiction après formation")
        .attention[0]
        .0;
    let size = |m: &EngramManager| m.store().get(eid).unwrap().node_ids().len();
    assert_eq!(size(&manager), 2, "engramme {{a,b}} formé");

    // (a) Observation identique → erreur réelle ~0 → PAS de croissance.
    let pred_emb = manager
        .predict(&[a], 8)
        .expect("prédiction")
        .predicted;
    let obs_same = encode_set(&spectral, &pair);
    let err_same = l2(&pred_emb, &obs_same);
    let _ = manager.revise(eid, &pair, err_same);
    assert_eq!(size(&manager), 2, "erreur réelle ~0 → pas de nœud ajouté");
    assert!(err_same < 0.05, "erreur réelle faible, obtenu {err_same}");

    // (b) Observation étendue {a,b,c} → erreur réelle > seuil → croissance.
    let extended: Ep = vec![a, b, c];
    let obs_ext = encode_set(&spectral, &extended);
    let err_ext = l2(&pred_emb, &obs_ext);
    let _ = manager.revise(eid, &extended, err_ext);
    assert!(err_ext > 0.1, "erreur réelle forte, obtenu {err_ext}");
    assert_eq!(
        size(&manager),
        3,
        "erreur réelle > seuil → reconsolidation ajoute le nœud manquant"
    );

    eprintln!(
        "[révision] erreurs réelles — identique={:.4} (pas d'ajout), étendue={:.4} (ajout de c)",
        err_same, err_ext
    );
}

