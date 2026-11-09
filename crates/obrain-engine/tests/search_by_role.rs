#![cfg(feature = "algos")]
//! `obrain.search_by_role` — « qui joue le même rôle structurel, ailleurs » :
//! (1) méthode `ObrainDB::search_by_role` ; (2) procédure CALL ; (3) filtre
//! `label` ; (4) lecture pure (read-only) ; (5) erreurs (nœud hors registre).
//!
//! Scénario : deux composants K5 ISOMORPHES mais de contenus différents
//! (labels L1 vs L2) + une étoile (centre + 4 feuilles) de forme différente.
//! Le hub d'un K5 doit retrouver les hubs de l'autre K5 à distance ≈ 0
//! (rôle identique, contenu différent), et être éloigné du centre de
//! l'étoile (rôle différent).

use obrain_engine::ObrainDB;

/// Crée une K5 (clique de 5) avec le label donné ; rend les 5 ids.
fn clique5(db: &ObrainDB, label: &str) -> Vec<u64> {
    let nodes: Vec<u64> = (0..5).map(|_| db.create_node(&[label]).0).collect();
    for i in 0..5 {
        for j in (i + 1)..5 {
            db.create_edge(
                obrain_common::types::NodeId(nodes[i]),
                obrain_common::types::NodeId(nodes[j]),
                "T",
            );
        }
    }
    nodes
}

/// Crée une étoile : centre + 4 feuilles ; rend (centre, feuilles).
fn star(db: &ObrainDB, label: &str) -> (u64, Vec<u64>) {
    let centre = db.create_node(&[label]).0;
    let leaves: Vec<u64> = (0..4).map(|_| db.create_node(&[label]).0).collect();
    for l in &leaves {
        db.create_edge(
            obrain_common::types::NodeId(centre),
            obrain_common::types::NodeId(*l),
            "T",
        );
    }
    (centre, leaves)
}

#[test]
fn search_by_role_retrouve_le_jumeau_structurel_cross_contenu() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-role");
    let (k1, k2, star_centre) = {
        let db = ObrainDB::open_substrate(&path).expect("open_substrate");
        let k1 = clique5(&db, "L1");
        let k2 = clique5(&db, "L2");
        let (sc, _leaves) = star(&db, "L3");
        let rows = db.search_by_role(k1[0], 100, None).expect("search_by_role");
        // Le hub k1[0] retrouve au moins un hub de l'autre K5 (rôle identique,
        // contenu différent : labels L1 vs L2).
        let twin = rows
            .iter()
            .find(|r| k2.contains(&r.node_id))
            .expect("jumeau K2 présent");
        // Bruit de l'estimateur Hutchinson (~0,1-0,2, doc geometric.rs) : pas
        // d'égalité exacte — on vérifie que le jumeau (même rôle, contenu
        // différent : L1 vs L2) est DISTINCTIVEMENT plus proche que la forme
        // différente (étoile).
        let sc_row = rows
            .iter()
            .find(|r| r.node_id == sc)
            .expect("étoile présente");
        assert!(
            twin.distance + 0.02 < sc_row.distance,
            "jumeau K5→K5 (dist {}) nettement plus proche que l'étoile (dist {})",
            twin.distance,
            sc_row.distance
        );
        assert!(
            twin.distance < 1.0,
            "jumeau à distance bornée, got {}",
            twin.distance
        );
        (k1, k2, sc)
    };
    let _ = (k1, k2, star_centre);
}

#[test]
fn search_by_role_label_filtre_les_candidats() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-role-label");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let k1 = clique5(&db, "L1");
    let k2 = clique5(&db, "L2");
    // Depuis un hub L1, candidats restreints à L2 → tous les retours sont L2.
    let rows = db.search_by_role(k1[0], 20, Some("L2")).expect("label L2");
    assert!(!rows.is_empty(), "des candidats L2");
    for r in &rows {
        assert!(
            k2.contains(&r.node_id),
            "candidat {} doit appartenir au composant L2",
            r.node_id
        );
    }
    // Le bruit de l'estimateur Hutchinson interdit l'égalité exacte : on
    // vérifie que le classement restreint L2 place un hub du K5 L2 en tête.
    assert!(
        rows[0].node_id == k2[0] || rows[0].node_id == k2[1],
        "un hub L2 en tête (dist {}), got node {}",
        rows[0].distance,
        rows[0].node_id
    );
}

#[test]
fn call_search_by_role_expose_le_classement() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-role-call");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let k1 = clique5(&db, "L1");
    let _k2 = clique5(&db, "L2");
    let s = db.session();
    let r = s
        .execute(&format!(
            "CALL obrain.search_by_role({}, 5) YIELD node_id, distance RETURN node_id, distance",
            k1[0]
        ))
        .expect("CALL search_by_role");
    assert_eq!(r.columns[0], "node_id", "colonne node_id");
    assert!(!r.rows.is_empty(), "CALL rend des lignes");
    for row in r.rows.iter().take(5) {
        assert!(matches!(row[0], obrain_common::types::Value::Int64(_)));
        assert!(matches!(row[1], obrain_common::types::Value::Float64(_)));
    }
}

#[test]
fn search_by_role_est_une_lecture_pure() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-role-ro");
    {
        let w = ObrainDB::open_substrate(&path).expect("open rw");
        let k1 = clique5(&w, "L1");
        let _k2 = clique5(&w, "L2");
        let rows = w.search_by_role(k1[0], 50, None).expect("rw ok");
        assert_ne!(rows.len(), 0);
    }
    let db_ro = ObrainDB::open_read_only(&path).expect("open ro");
    // On re-monte le graphe ? non : read-only, on lit l'existant. L'ancre k1[0]
    // est inconnue de ce process — retrouver son slot : c'est le premier nœud
    // créé = slot 1.
    let rows = db_ro.search_by_role(1, 50, None).expect("ro ok");
    assert!(!rows.is_empty(), "read-only rend le classement");
    let s = db_ro.session();
    let r = s
        .execute("CALL obrain.search_by_role(1, 5) YIELD node_id RETURN node_id")
        .expect("CALL read-only");
    assert!(!r.rows.is_empty(), "CALL read-only rend des lignes");
}

#[test]
fn search_by_role_noeud_invalide_erreur() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-role-err");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let _k1 = clique5(&db, "L1");
    // Slot 0 (sentinelle) et slot hors registre → erreur explicite.
    assert!(db.search_by_role(0, 10, None).is_err(), "slot 0 refusé");
    assert!(
        db.search_by_role(u64::MAX, 10, None).is_err(),
        "slot hors registre refusé"
    );
}

#[test]
fn search_by_role_respecte_la_limite() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-role-limit");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let k1 = clique5(&db, "L1");
    let rows = db.search_by_role(k1[0], 3, None).expect("limite");
    assert_eq!(rows.len(), 3, "limit appliquée");
    // Tri croissant par distance.
    let mut prev = -1.0_f32;
    for r in &rows {
        assert!(r.distance >= prev, "tri croissant par distance");
        prev = r.distance;
    }
}
