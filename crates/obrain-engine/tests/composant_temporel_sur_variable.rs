//! Composant d'une valeur temporelle liée à une variable (obrain-db/obrain#213).
//!
//! `t.year` rend la bonne valeur quand la base est un littéral ou une
//! propriété de nœud, mais rendait NULL dès que la base était une variable
//! (`WITH … AS t`, `UNWIND … AS t`) ; un filtre `WHERE x.year = 2020` perdait
//! alors toutes les lignes sans erreur.
//!
//! Chaque cas est joué en Cypher et en GQL.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test composant_temporel_sur_variable
//! ```

#![cfg(feature = "cypher")]

use obrain_common::types::{Timestamp, Value, ZonedDatetime};
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// 2020-01-02T03:04:05Z.
const INSTANT_NS: i64 = 1_577_934_245_000_000_000;

fn valeur(db: &ObrainDB, langage: &str, requete: &str) -> Value {
    let r = db
        .session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"));
    match r.rows.as_slice() {
        [ligne] => ligne[0].clone(),
        autre => panic!("[{langage}] {requete} : une ligne attendue, reçu {autre:?}"),
    }
}

fn base_zonee() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let id = db.create_node(&["Z"]);
    db.set_node_property(
        id,
        "t",
        Value::ZonedDatetime(ZonedDatetime::from_timestamp_offset(
            Timestamp::from_nanos(INSTANT_NS),
            0,
        )),
    );
    db
}

#[test]
fn composant_sur_litteral_et_sur_variable() {
    let db = base_zonee();
    // GQL n'a ni `WITH` ni `UNWIND` : `LET` (après un `MATCH`) lie une variable, `FOR` itère.
    let cas: [(&str, &str); 10] = [
        (
            "cypher",
            "RETURN datetime('2020-01-02T03:04:05Z').year AS v",
        ),
        (
            "cypher",
            "WITH datetime('2020-01-02T03:04:05Z') AS t RETURN t.year AS v",
        ),
        (
            "cypher",
            "UNWIND [datetime('2020-01-02T03:04:05Z')] AS t RETURN t.year AS v",
        ),
        ("cypher", "MATCH (z:Z) RETURN z.t.year AS v"),
        ("cypher", "MATCH (z:Z) WITH z.t AS x RETURN x.year AS v"),
        ("gql", "RETURN datetime('2020-01-02T03:04:05Z').year AS v"),
        (
            "gql",
            "UNWIND [datetime('2020-01-02T03:04:05Z')] AS t RETURN t.year AS v",
        ),
        (
            "gql",
            "FOR t IN [datetime('2020-01-02T03:04:05Z')] RETURN t.year AS v",
        ),
        ("gql", "MATCH (z:Z) RETURN z.t.year AS v"),
        ("gql", "MATCH (z:Z) LET x = z.t RETURN x.year AS v"),
    ];
    for (langage, requete) in cas {
        assert_eq!(
            valeur(&db, langage, requete),
            Value::Int64(2020),
            "[{langage}] {requete}"
        );
    }
}

#[test]
fn filtre_sur_composant_de_variable() {
    let db = base_zonee();
    for (langage, requete) in [
        (
            "cypher",
            "MATCH (z:Z) WITH z.t AS x WHERE x.year = 2020 RETURN count(*) AS n",
        ),
        (
            "gql",
            "MATCH (z:Z) LET x = z.t FILTER x.year = 2020 RETURN count(*) AS n",
        ),
    ] {
        assert_eq!(
            valeur(&db, langage, requete),
            Value::Int64(1),
            "[{langage}] {requete}"
        );
    }
}

#[test]
fn propriete_de_noeud_via_variable_inchangee() {
    let db = ObrainDB::new_in_memory();
    let id = db.create_node(&["P"]);
    db.set_node_property(id, "nom", Value::from("a"));
    for langage in LANGAGES {
        assert_eq!(
            valeur(&db, langage, "MATCH (p:P) WITH p AS q RETURN q.nom AS v"),
            Value::from("a"),
            "[{langage}]"
        );
    }
}
