//! Une écriture RDF commitée dans une transaction explicite survit à la
//! réouverture ; un rollback ne persiste rien (régression de durabilité RDF).
#![cfg(all(feature = "sparql", feature = "rdf"))]

use obrain_engine::{Config, GraphModel, ObrainDB};

#[test]
fn triplet_commite_en_transaction_explicite_survit_a_la_reouverture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rdf_tx.obrain");

    {
        let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
            .expect("open");
        let mut session = db.session();
        session.begin_transaction().unwrap();
        session
            .execute_sparql(r#"INSERT DATA { <http://ex.org/alix> <http://ex.org/name> "Alix" . }"#)
            .unwrap();
        session.commit().unwrap();
        db.close().expect("close");
    }

    let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
        .expect("reopen");
    let session = db.session();
    let result = session
        .execute_sparql("SELECT ?n WHERE { ?s <http://ex.org/name> ?n }")
        .unwrap();
    assert_eq!(
        result.rows.len(),
        1,
        "triplet commité perdu après réouverture"
    );
}

#[test]
fn triplet_en_autocommit_survit_a_la_reouverture_temoin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rdf_auto.obrain");
    {
        let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
            .expect("open");
        let session = db.session();
        session
            .execute_sparql(r#"INSERT DATA { <http://ex.org/alix> <http://ex.org/name> "Alix" . }"#)
            .unwrap();
        db.close().expect("close");
    }
    let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
        .expect("reopen");
    let session = db.session();
    let result = session
        .execute_sparql("SELECT ?n WHERE { ?s <http://ex.org/name> ?n }")
        .unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn graphe_nomme_commite_en_transaction_survit_a_la_reouverture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rdf_named_tx.obrain");
    {
        let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
            .expect("open");
        let mut session = db.session();
        session.begin_transaction().unwrap();
        session
            .execute_sparql(
                r#"INSERT DATA { GRAPH <http://ex.org/g> { <http://ex.org/alix> <http://ex.org/name> "Alix" . } }"#,
            )
            .unwrap();
        session.commit().unwrap();
        db.close().expect("close");
    }
    let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
        .expect("reopen");
    let session = db.session();
    let result = session
        .execute_sparql(
            "SELECT ?n WHERE { GRAPH <http://ex.org/g> { ?s <http://ex.org/name> ?n } }",
        )
        .unwrap();
    assert_eq!(
        result.rows.len(),
        1,
        "graphe nommé commité en transaction perdu"
    );
}

#[test]
fn rollback_ne_persiste_rien() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rdf_rollback.obrain");
    {
        let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
            .expect("open");
        let mut session = db.session();
        session.begin_transaction().unwrap();
        session
            .execute_sparql(r#"INSERT DATA { <http://ex.org/alix> <http://ex.org/name> "Alix" . }"#)
            .unwrap();
        session.rollback().unwrap();
        db.close().expect("close");
    }
    let db = ObrainDB::with_config(Config::persistent(&path).with_graph_model(GraphModel::Rdf))
        .expect("reopen");
    let session = db.session();
    let result = session
        .execute_sparql("SELECT ?n WHERE { ?s <http://ex.org/name> ?n }")
        .unwrap();
    assert_eq!(result.rows.len(), 0, "un rollback a persisté un triplet");
}
