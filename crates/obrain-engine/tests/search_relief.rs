#![cfg(all(feature = "cdc", feature = "algos"))]
//! `obrain.relief` — projection contextuelle R_q sur les signaux existants :
//! (1) méthode `ObrainDB::search_relief` ; (2) procédure CALL ;
//! (3) pénalité des contradictions ; (4) fenêtre vide ; (5) bornes des dims.
//!
//! Scénario : dans une fenêtre propre, un « nouveau hub » H reçoit une rafale
//! (nouveauté + fraîcheur élevées) ; un claim C subit un flip A→B→A
//! (contradiction → pénalité). H doit dominer le relief ; C doit porter une
//! dimension `conflict > 0` et un relief inférieur.

use std::time::Duration;
use web_time::SystemTime;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("horloge")
        .as_millis() as u64
}

fn edge(db: &ObrainDB, a: u64, b: u64, t: &str) {
    db.create_edge(
        obrain_common::types::NodeId(a),
        obrain_common::types::NodeId(b),
        t,
    );
}

#[test]
fn relief_domine_le_nouveau_hub_et_penalise_la_contradiction() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-relief");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");

    // Pré-graphe (AVANT la fenêtre) : feuilles à degré initial 1.
    let mut leaves: Vec<u64> = Vec::new();
    let mut dummies: Vec<u64> = Vec::new();
    for _ in 0..8 {
        leaves.push(db.create_node(&["Doc"]).0);
        dummies.push(db.create_node(&["Doc"]).0);
    }
    for leaf in &leaves {
        edge(&db, *leaf, dummies[0], "T");
    }
    let claim = db.create_node(&["Claim"]).0;
    db.set_node_property(
        obrain_common::types::NodeId(claim),
        "epistemic",
        Value::String("OBSERVED".into()),
    );
    // Pré-valeur 0 posée AVANT la fenêtre (motif du test search_conflicts :
    // les reversals sont détectés sur les changements 1,2,1,2,1).
    db.set_node_property(
        obrain_common::types::NodeId(claim),
        "state",
        Value::Int64(0),
    );

    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();

    // Fenêtre : (a) contradiction A→B→A… sur le claim ; (b) rafale sur le hub.
    for v in [1i64, 2, 1, 2, 1] {
        db.set_node_property(
            obrain_common::types::NodeId(claim),
            "state",
            Value::Int64(v),
        );
    }
    // Séparation temporelle : la rafale du hub arrive APRÈS les flips du claim
    // (sinon span ≈ 0 → la dimension fraîcheur est dégénérée).
    std::thread::sleep(Duration::from_millis(3));
    let hub = db.create_node(&["Doc"]).0;
    for leaf in &leaves {
        edge(&db, *leaf, hub, "T");
    }

    let rows = db.search_relief(since, 50).expect("relief");
    assert!(!rows.is_empty(), "fenêtre non vide");
    // Dims bornées [0,1].
    for r in &rows {
        for d in [r.novelty, r.recency, r.centrality, r.conflict] {
            assert!((0.0..=1.0).contains(&d), "dim hors bornes {d}");
        }
    }
    // Le nouveau hub domine (nouveauté + fraîcheur).
    assert_eq!(rows[0].node_id, hub, "hub en tête du relief");
    let h = &rows[0];
    assert!(h.novelty > 0.0, "hub: nouveauté > 0 ({})", h.novelty);
    assert!(
        h.recency >= 1.0 - 1e-9,
        "hub: fraîcheur max ({})",
        h.recency
    );
    // DIAGNOSTIC : la même fenêtre vue par search_conflicts (méthode moteur).
    let conf = db.search_conflicts(since, 100).expect("conflicts(méthode)");
    assert!(
        conf.iter().any(|r| r.node_id == claim),
        "search_conflicts({since}) doit voir le claim: {conf:?}"
    );
    // Le claim contradictoire est présent avec conflict > 0 et relief moindre.
    let c = rows
        .iter()
        .find(|r| r.node_id == claim)
        .expect("claim actif");
    assert!(
        c.conflict > 0.0,
        "claim: contradiction détectée ({})",
        c.conflict
    );
    assert!(
        h.relief > c.relief,
        "relief(hub) {} > relief(claim contradictoire) {}",
        h.relief,
        c.relief
    );
}

#[test]
fn call_relief_expose_les_dimensions() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-relief-call");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let a = db.create_node(&["Doc"]).0;
    let b = db.create_node(&["Doc"]).0;
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    edge(&db, a, b, "T");
    let s = db.session();
    let r = s
        .execute(&format!(
            "CALL obrain.relief({since}, 10) YIELD node_id, relief RETURN node_id, relief"
        ))
        .expect("CALL relief");
    assert!(!r.rows.is_empty(), "CALL rend des lignes");
    assert_eq!(r.columns[0], "node_id");
}

#[test]
fn relief_fenetre_vide_rend_vide() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-relief-vide");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let _a = db.create_node(&["Doc"]);
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    let rows = db.search_relief(since, 50).expect("vide");
    assert!(rows.is_empty(), "aucune activité dans la fenêtre → vide");
}
