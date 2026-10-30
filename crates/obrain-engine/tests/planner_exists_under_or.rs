//! Planner — `EXISTS {…}` : conjonction/AND et négation OK, disjonction (OR)
//! REFUSÉE explicitement (jamais de résultat faux silencieux).
//!
//! Mesuré sur runtime-dev (2026-09-07) : un `EXISTS` sous `OR` ne renvoyait
//! que les autres disjonctifs (racines seules) — le bras EXISTS était ignoré.
//! `NOT EXISTS` en conjonction est correct (anti-join). La garde transforme le
//! faux silencieux en erreur sémantique claire tant que la réécriture UNION
//! n'est pas implémentée.

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;

fn db_at(path: &std::path::Path) -> ObrainDB {
    ObrainDB::open_substrate(path).expect("open_substrate")
}

/// racine (sans origin), subagent AVEC message, subagent SANS message.
fn setup(db: &ObrainDB) -> (NodeId, NodeId) {
    let root = db.create_node(&["Session"]);
    let sub_avec = db.create_node(&["Session"]);
    db.set_node_property(
        sub_avec,
        "origin",
        obrain_common::types::Value::String("subagent".into()),
    );
    let sub_sans = db.create_node(&["Session"]);
    db.set_node_property(
        sub_sans,
        "origin",
        obrain_common::types::Value::String("subagent".into()),
    );
    for s in [root, sub_avec] {
        let m = db.create_node(&["Message"]);
        db.create_edge(m, s, "PART_OF");
    }
    (root, sub_avec)
}

fn count(db: &ObrainDB, q: &str) -> i64 {
    let r = db.session().execute(q).expect("query");
    match r.rows[0][0] {
        obrain_common::types::Value::Int64(v) => v,
        ref other => panic!("count inattendu: {other:?}"),
    }
}

#[test]
fn exists_sous_or_rend_le_bon_resultat() {
    let td = tempfile::tempdir().unwrap();
    let db = db_at(&td.path().join("or"));
    std::mem::forget(td);
    let _ = setup(&db);
    // OR-EXISTS (réécrit en UNION-DISTINCT des branches) : la racine matche
    // les DEUX premiers disjonctifs ET le EXISTS (doublon) ; le subagent sans
    // message ne matche rien → résultat attendu 2, sans doublon.
    let got = count(
        &db,
        "MATCH (s:Session) WHERE (s.origin IS NULL OR s.origin <> 'subagent' \
         OR EXISTS { MATCH (:Message)-[:PART_OF]->(s) }) RETURN count(*) AS c",
    );
    assert_eq!(got, 2, "OR-EXISTS = union dédupliquée (racine + subagent-avec-message)");
}

#[test]
fn not_exists_en_conjonction_est_evalue() {
    let td = tempfile::tempdir().unwrap();
    let db = db_at(&td.path().join("not"));
    std::mem::forget(td);
    let _ = setup(&db);
    // Subagent SANS message = 1 (anti-join supporté).
    let got = count(
        &db,
        "MATCH (s:Session) WHERE s.origin = 'subagent' AND \
         NOT EXISTS { MATCH (:Message)-[:PART_OF]->(s) } RETURN count(*) AS c",
    );
    assert_eq!(got, 1, "NOT EXISTS doit isoler le subagent sans message");
}
