//! `EXISTS { }`, `COUNT { }` et `exists((motif))` au-delà du motif nu à un saut.
//!
//! - `exists((n)-[:R]->(:P {id: 'p1'}))` (forme fonction) mourait en
//!   « Unsupported EXISTS subquery pattern » ;
//! - en projection, `COUNT { }` n'acceptait que `(n)-[:T]->()` et `EXISTS { }`
//!   rien du tout (« Unsupported RETURN expression »).
//!
//! Le traducteur ramène la forme fonction à `EXISTS { }`, et relève les
//! sous-requêtes de projection en Apply + count(*) (le chemin rapide du motif
//! nu reste en place).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test sous_requete_exists_count
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// n1 -> p1, p2 (:P), q1 (:Q) ; n2 -> p3 (:P) ; n3 isolé.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.execute_cypher(
        "CREATE (n1:N {id: 'n1'})-[:R]->(:P {id: 'p1'}), (n1)-[:R]->(:Q {id: 'q1'}), \
                (n1)-[:R]->(:P {id: 'p2'}), (:N {id: 'n2'})-[:R]->(:P {id: 'p3'}), \
                (:N {id: 'n3'})",
    )
    .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    let mut rows = db
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows;
    rows.sort_by_key(|l| format!("{l:?}"));
    rows
}

fn paire(id: &str, v: Value) -> Vec<Value> {
    vec![Value::from(id), v]
}

#[test]
fn count_en_projection_avec_label_et_filtre() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:N) RETURN n.id, COUNT { (n)-[:R]->(:P) } AS c"
        ),
        vec![
            paire("n1", Value::Int64(2)),
            paire("n2", Value::Int64(1)),
            paire("n3", Value::Int64(0)),
        ]
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:N) RETURN n.id, COUNT { MATCH (n)-[:R]->(x) WHERE x.id STARTS WITH 'p' } AS c"
        ),
        vec![
            paire("n1", Value::Int64(2)),
            paire("n2", Value::Int64(1)),
            paire("n3", Value::Int64(0)),
        ]
    );
    // Le motif nu garde son chemin rapide, même résultat.
    assert_eq!(
        lignes(&db, "MATCH (n:N) RETURN n.id, COUNT { (n)-[:R]->() } AS c"),
        vec![
            paire("n1", Value::Int64(3)),
            paire("n2", Value::Int64(1)),
            paire("n3", Value::Int64(0)),
        ]
    );
}

#[test]
fn exists_en_projection() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:N) RETURN n.id, EXISTS { (n)-[:R]->(:Q) } AS e"
        ),
        vec![
            paire("n1", Value::Bool(true)),
            paire("n2", Value::Bool(false)),
            paire("n3", Value::Bool(false)),
        ]
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:N) WITH n, EXISTS { (n)-[:R]->() } AS e WHERE e RETURN n.id"
        ),
        vec![vec![Value::from("n1")], vec![Value::from("n2")]]
    );
}

#[test]
fn exists_forme_fonction() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:N) WHERE exists((n)-[:R]->(:P {id: 'p3'})) RETURN n.id"
        ),
        vec![vec![Value::from("n2")]]
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:N) WHERE NOT exists((n)-[:R]->(:P)) RETURN n.id"
        ),
        vec![vec![Value::from("n3")]]
    );
}
