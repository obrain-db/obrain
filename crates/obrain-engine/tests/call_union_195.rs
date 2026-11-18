//! `UNION` dans `CALL { ... }` (obrain-db/obrain#195, famille 3).
//!
//! `CALL { WITH center MATCH ... RETURN n UNION WITH center RETURN center AS n }`
//! était refusé à l'analyse (« Expected RBrace »). La forme de Project
//! Orchestrator (`sampler_01`, `persona_07`, `persona_08`) est corrélée : chaque
//! branche importe `center` par un `WITH` en tête.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test call_union_195
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// c -> n1 -> n2 (CALLS).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (c:S {id: 'c'})-[:CALLS]->(n1:S {id: 'n1'})-[:CALLS]->(n2:S {id: 'n2'})",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

fn textes(db: &ObrainDB, requete: &str) -> Vec<String> {
    let mut out: Vec<String> = lignes(db, requete)
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("texte attendu, reçu {autre:?}"),
        })
        .collect();
    out.sort();
    out
}

fn entiers(db: &ObrainDB, requete: &str) -> Vec<i64> {
    let mut out: Vec<i64> = lignes(db, requete)
        .into_iter()
        .map(|l| match &l[0] {
            Value::Int64(i) => *i,
            autre => panic!("entier attendu, reçu {autre:?}"),
        })
        .collect();
    out.sort_unstable();
    out
}

/// Les branches sont corrélées à une ligne externe (`MATCH (c)` puis `WITH c`).
/// Sans entrée externe, `CALL { RETURN 1 AS x } RETURN x` rend NULL sur
/// l'ancien code aussi : ce défaut est hors de ce correctif.
#[test]
fn union_simple_deduplique() {
    let db = base();
    assert_eq!(
        entiers(
            &db,
            "MATCH (c:S {id: 'c'}) CALL { WITH c RETURN 1 AS x UNION WITH c RETURN 2 AS x } RETURN x"
        ),
        vec![1, 2]
    );
    assert_eq!(
        entiers(
            &db,
            "MATCH (c:S {id: 'c'}) CALL { WITH c RETURN 1 AS x UNION WITH c RETURN 1 AS x } RETURN x"
        ),
        vec![1]
    );
}

#[test]
fn union_all_garde_les_doublons() {
    let db = base();
    assert_eq!(
        entiers(
            &db,
            "MATCH (c:S {id: 'c'}) CALL { WITH c RETURN 1 AS x UNION ALL WITH c RETURN 1 AS x } RETURN x"
        ),
        vec![1, 1]
    );
}

/// UNION se déduplique PAR ligne externe, pas globalement : `n1` apparaît une
/// fois pour `c` (via `n1`) et une fois pour `n1` (via `n2`).
#[test]
fn union_se_deduplique_par_ligne_externe() {
    let db = base();
    assert_eq!(
        textes(
            &db,
            "MATCH (c:S) \
             CALL { WITH c MATCH (c)-[:CALLS]->(n) RETURN n.id AS id \
                    UNION WITH c RETURN c.id AS id } \
             RETURN id"
        ),
        vec![
            "c".to_string(),
            "n1".to_string(),
            "n1".to_string(),
            "n2".to_string(),
            "n2".to_string()
        ]
    );
}

/// Forme corrélée : chaque branche importe `c`.
#[test]
fn union_correle_par_with_en_tete_de_branche() {
    let db = base();
    assert_eq!(
        textes(
            &db,
            "MATCH (c:S {id: 'c'}) \
             CALL { WITH c MATCH (c)-[:CALLS]->(n) RETURN n.id AS id \
                    UNION WITH c RETURN c.id AS id } \
             RETURN id"
        ),
        vec!["c".to_string(), "n1".to_string()]
    );
}

/// La forme de PO (`sampler_01`) : `DISTINCT` et `LIMIT` dans la première branche,
/// variable-length undirected, `UNION` avec la centrale.
#[test]
fn union_forme_sampler_avec_limit_et_distinct() {
    let db = base();
    assert_eq!(
        entiers(
            &db,
            "MATCH (center:S) WHERE center.id = 'c' \
             WITH center \
             CALL { WITH center MATCH (center)-[:CALLS*1..3]-(neighbor) \
                    WITH DISTINCT neighbor RETURN neighbor.id AS id LIMIT 10 \
                    UNION WITH center RETURN center.id AS id } \
             RETURN count(id) AS k"
        ),
        vec![3],
        "n1, n2 (voisins) et c lui-même"
    );
}

#[test]
fn union_et_union_all_melanges_sont_refuses() {
    let db = base();
    let r = db.session().execute_cypher(
        "CALL { RETURN 1 AS x UNION RETURN 2 AS x UNION ALL RETURN 3 AS x } RETURN x",
    );
    assert!(
        r.is_err(),
        "UNION et UNION ALL mélangés doivent être refusés"
    );
}

/// Les branches importent des variables différentes : refus explicite, pas un
/// résultat faux (le moteur ne porte qu'un état de paramètres par `Apply`).
#[test]
fn branches_important_des_variables_differentes_sont_refusees() {
    let db = base();
    let r = db.session().execute_cypher(
        "MATCH (c:S {id: 'c'}) MATCH (d:S {id: 'n1'}) \
         CALL { WITH c RETURN c.id AS id UNION WITH d RETURN d.id AS id } \
         RETURN id",
    );
    assert!(r.is_err(), "imports différents : erreur explicite attendue");
}
