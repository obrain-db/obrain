//! Expérience de PRÉDICTION COMPOSITIONNELLE native, via `Composition` (#416).
//!
//! Obrain apprend deux connaissances de base K1={a,b} et K2={c,d}, puis une
//! connaissance composite K3 dont les ENFANTS référencent K1 et K2.
//! `resolve(K3)` aplatit transitivement en {a,b,c,d}. On PRÉDIT ce composite
//! depuis un indice qui active les parties, SANS avoir jamais observé le
//! composite entier. On rapporte l'erreur L2 mesurée : le bénéfice
//! compositionnel n'est PAS supposé acquis.

#![cfg(feature = "cognitive-engram")]

use obrain_common::types::NodeId;
use obrain_engine::{Composition, KnowledgeSpec, ObrainDB};

#[test]
fn prediction_compositionnelle_native() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate_observed(dir.path().join("base")).unwrap();

    // Participants (membres des connaissances de base).
    let a: NodeId = db.create_node(&["Membre"]);
    let b: NodeId = db.create_node(&["Membre"]);
    let c: NodeId = db.create_node(&["Membre"]);
    let d: NodeId = db.create_node(&["Membre"]);

    let comp = Composition::new(&db);

    // K1 = {a,b}, K2 = {c,d}.
    let k1 = comp.construct(&KnowledgeSpec {
        members: [a, b].into_iter().collect(),
        children: Default::default(),
        relations: Default::default(),
    });
    let k2 = comp.construct(&KnowledgeSpec {
        members: [c, d].into_iter().collect(),
        children: Default::default(),
        relations: Default::default(),
    });

    // K3 = composite dont les ENFANTS référencent K1 et K2.
    let k3 = comp.construct(&KnowledgeSpec {
        members: Default::default(),
        children: [k1, k2].into_iter().collect(),
        relations: Default::default(),
    });

    // resolve(K3) aplatit transitivement -> {a,b,c,d}.
    let resolved = comp.resolve(k3).expect("resolve composite");
    assert_eq!(
        resolved.len(),
        4,
        "resolve(K3) = {{a,b,c,d}} (obtenu {} membres)",
        resolved.len()
    );
    assert!(
        [a, b, c, d].iter().all(|n| resolved.contains(n)),
        "tous les participants des parties sont présents"
    );
    let composite: Vec<NodeId> = resolved.into_iter().collect();

    // --- Apprentissage des PARTIES par la primitive native ---
    let mgr = db
        .cognitive_engine()
        .expect("moteur cognitif")
        .engram_manager()
        .expect("engram manager");
    let part1: Vec<NodeId> = vec![a, b];
    let part2: Vec<NodeId> = vec![c, d];
    for _ in 0..3 {
        let _ = mgr.predictive_step(&[a], &part1, 8).expect("pas K1");
        let _ = mgr.predictive_step(&[c], &part2, 8).expect("pas K2");
    }

    // Le composite n'a JAMAIS été observé : on vérifie qu'aucun engramme ne
    // contient les quatre participants simultanément.
    let composite_observed = mgr.store().list().iter().any(|e| {
        let s: std::collections::HashSet<u64> = e.node_ids().iter().map(|n| n.0).collect();
        let target: std::collections::HashSet<u64> =
            [a, b, c, d].iter().map(|n| n.0).collect();
        s == target
    });
    assert!(
        !composite_observed,
        "le composite entier n'a pas été observé (seules les parties le sont)"
    );

    // --- PRÉDICTION du composite depuis un indice qui active les deux parties ---
    // Cue = [a, c] : un membre de chaque partie.
    let pred = mgr.predict(&[a, c], 8).expect("prédiction composite");
    let predicted_emb = pred.predicted.clone();
    let composite_emb = mgr.encode_observation(&composite);
    // Dimension vérifiée par la primitive (refus d'une L2 hétérogène).
    let err = mgr
        .prediction_error(&predicted_emb, &composite_emb)
        .expect("dimensions égales");

    // Références : erreur d'un témoin « vide » (= norme de l'observation) et
    // d'une prédiction d'UNE SEULE partie (contrôle négatif).
    let empty = vec![0.0f64; composite_emb.len()];
    let err_empty = mgr.prediction_error(&empty, &composite_emb).expect("dim");
    let pred1 = mgr.predict(&[a], 8).expect("prédiction K1 seule");
    let err_part = mgr
        .prediction_error(&pred1.predicted, &composite_emb)
        .expect("dim");

    eprintln!(
        "[composition] erreur L2 composite prédit = {:.4} (témoin vide = {:.4}, une partie seule = {:.4})",
        err, err_empty, err_part
    );
    eprintln!(
        "[composition] attention = {:?}",
        pred.attention
            .iter()
            .map(|(id, w)| (id.0, *w))
            .collect::<Vec<_>>()
    );

    // Le composite prédit depuis les parties doit être plus proche du vrai
    // composite qu'un vecteur vide (sinon la prédiction n'apporte rien).
    assert!(
        err < err_empty,
        "la prédiction composite ({err}) fait mieux que le vide ({err_empty})"
    );
    // Elle doit aussi faire au moins aussi bien qu'une seule partie (contrôle
    // négatif : activer une partie ne suffit pas à prédire le tout).
    assert!(
        err < err_part,
        "activer les DEUX parties ({err}) bat une seule partie ({err_part})"
    );

    // Ce n'est PAS une exactitude : on rapporte le résidu, on ne le cache pas.
    assert!(
        err > 0.0,
        "le composite prédit n'est pas exact (résidu {err}) — pas de bénéfice magique"
    );
}
