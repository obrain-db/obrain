//! Unicite MERGE (mission perf, feature 2.1) : un MERGE ne cree plus
//! JAMAIS un second porteur vivant du meme motif — y compris sous
//! concurrence (le chemin de secours createur des 46 doublons par id
//! logique mesures sur la base agent).
//!
//! CREATE explicite reste PERMISSIF (outil imperatif, nomme) : seul MERGE
//! porte la garantie.

#![cfg(feature = "gql")]

use obrain_core::graph::GraphStore;
use obrain_engine::ObrainDB;

/// 16 fils x 12 MERGE du meme id : UN seul porteur a la fin, zero echec.
#[test]
fn merge_concurrent_ne_cree_jamais_de_doublon() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    let mut hs = Vec::new();
    for t in 0..16 {
        let db = std::sync::Arc::clone(&db);
        hs.push(std::thread::spawn(move || {
            let s = db.session();
            for k in 0..12 {
                s.execute(&format!(
                    "MERGE (n:Cible {{id: \x27unique\x27}}) SET n.v = {t}{k} RETURN n.id"
                ))
                .expect("merge concurrent : zero echec attendu (re-tentative auto-commit)");
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let porteurs = db.graph_store().nodes_by_label("Cible");
    assert_eq!(
        porteurs.len(),
        1,
        "16 fils x 12 MERGE du meme id doivent laisser UN porteur, trouve {}",
        porteurs.len()
    );
}

/// Idem avec index de propriete (le chemin chaud de production).
#[test]
fn merge_concurrent_avec_index_reste_unique() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    db.create_property_index("id");
    let mut hs = Vec::new();
    for t in 0..16 {
        let db = std::sync::Arc::clone(&db);
        hs.push(std::thread::spawn(move || {
            let s = db.session();
            for k in 0..12 {
                s.execute(&format!(
                    "MERGE (n:CibleIdx {{id: \x27unique\x27}}) SET n.v = {t}{k} RETURN n.id"
                ))
                .expect("merge indexe");
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(db.graph_store().nodes_by_label("CibleIdx").len(), 1);
}

/// Sur des doublons DEJA presents (pollution historique), MERGE cible
/// toujours le MEME porteur (plus petit NodeId) — stable et convergent.
#[test]
fn merge_sur_doublons_existants_est_deterministe() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Trois doublons poses par CREATE explicite (permissif, nomme).
    for k in 0..3 {
        s.execute(&format!("CREATE (:Doublon {{id: \x27dup\x27, ne: {k}}})"))
            .expect("create doublon");
    }
    assert_eq!(db.graph_store().nodes_by_label("Doublon").len(), 3);
    for _ in 0..5 {
        s.execute("MERGE (n:Doublon {id: \x27dup\x27}) SET n.touche = true RETURN n.id")
            .expect("merge");
    }
    // Le porteur touche est UNIQUE et c est le plus petit NodeId.
    let r = s
        .execute("MATCH (n:Doublon) WHERE n.touche = true RETURN n.ne")
        .expect("verif");
    assert_eq!(r.rows.len(), 1, "un seul porteur doit recevoir les SET");
    assert_eq!(
        r.rows[0][0],
        obrain_common::types::Value::Int64(0),
        "regle min-NodeId : le premier-ne est la cible stable"
    );
}

/// CREATE explicite : deux CREATE du meme id posent DEUX noeuds —
/// tolerance nommee (outil imperatif), la garantie ne porte que sur MERGE.
#[test]
fn create_explicite_reste_permissif_et_nomme() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("CREATE (:Libre {id: \x27meme\x27})")
        .expect("create 1");
    s.execute("CREATE (:Libre {id: \x27meme\x27})")
        .expect("create 2");
    assert_eq!(db.graph_store().nodes_by_label("Libre").len(), 2);
}
