//! `WITH *, expr AS alias` (obrain-db/obrain#356, requete PO ana_11).
//!
//! Le parseur refusait la virgule apres `WITH *` (« Expected end of query »).
//! Semantique Neo4j : toutes les variables en portee sont conservees ET les
//! items supplementaires sont ajoutes.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test with_etoile_expression_356
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (:P {name: 'a', v: 1}), (:P {name: 'b', v: 2}), (:P {name: 'c', v: 3})",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> (Vec<String>, Vec<Vec<Value>>) {
    let r = db
        .session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"));
    (r.columns, r.rows)
}

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, obtenu {autre:?}"),
    }
}

#[test]
fn etoile_plus_une_expression() {
    let db = base();
    let (cols, rows) = lignes(
        &db,
        "MATCH (n:P) WITH *, n.v * 10 AS x RETURN n.name AS nom, x ORDER BY nom",
    );
    assert_eq!(cols, vec!["nom".to_string(), "x".to_string()]);
    let vus: Vec<(String, i64)> = rows
        .iter()
        .map(|r| (r[0].to_string(), entier(&r[1])))
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(entier(&rows[0][1]), 10);
    assert_eq!(entier(&rows[1][1]), 20);
    assert_eq!(entier(&rows[2][1]), 30);
    assert!(vus[0].0.contains('a'));
}

#[test]
fn etoile_plus_deux_expressions() {
    let db = base();
    let (_, rows) = lignes(
        &db,
        "MATCH (n:P) WITH *, n.v AS a, n.v + 100 AS b RETURN a, b ORDER BY a",
    );
    let v: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| (entier(&r[0]), entier(&r[1])))
        .collect();
    assert_eq!(v, vec![(1, 101), (2, 102), (3, 103)]);
}

#[test]
fn etoile_garde_les_variables_et_ajoute_l_alias() {
    let db = base();
    let (_, rows) = lignes(
        &db,
        "MATCH (n:P) WITH *, n.v AS x WHERE x >= 2 RETURN n.v AS v, x ORDER BY v",
    );
    let v: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| (entier(&r[0]), entier(&r[1])))
        .collect();
    assert_eq!(v, vec![(2, 2), (3, 3)]);
}

#[test]
fn etoile_plus_agregat() {
    let db = base();
    let (_, rows) = lignes(
        &db,
        "MATCH (n:P) WITH *, count(n) AS c RETURN n.v AS v, c ORDER BY v",
    );
    let v: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| (entier(&r[0]), entier(&r[1])))
        .collect();
    assert_eq!(v, vec![(1, 1), (2, 1), (3, 1)]);
}

// --- non-regression ---

#[test]
fn etoile_seule() {
    let db = base();
    let (_, rows) = lignes(&db, "MATCH (n:P) WITH * RETURN n.v AS v ORDER BY v");
    let v: Vec<i64> = rows.iter().map(|r| entier(&r[0])).collect();
    assert_eq!(v, vec![1, 2, 3]);
}

#[test]
fn etoile_avec_where() {
    let db = base();
    let (_, rows) = lignes(
        &db,
        "MATCH (n:P) WITH * WHERE n.v > 1 RETURN n.v AS v ORDER BY v",
    );
    let v: Vec<i64> = rows.iter().map(|r| entier(&r[0])).collect();
    assert_eq!(v, vec![2, 3]);
}

#[test]
fn with_alias_simple() {
    let db = base();
    let (_, rows) = lignes(&db, "MATCH (n:P) WITH n.v AS y RETURN y ORDER BY y");
    let v: Vec<i64> = rows.iter().map(|r| entier(&r[0])).collect();
    assert_eq!(v, vec![1, 2, 3]);
}
