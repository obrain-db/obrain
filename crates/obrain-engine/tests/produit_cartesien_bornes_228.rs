//! `shortestPath` entre deux variables DEJA liees n'est pas un produit
//! cartesien (obrain-db/obrain#228, reouverte : `code_44_get_bridge_proximity`).
//!
//! Le garde d'explosion est une estimation de cardinalite : on l'abaisse par
//! `OBRAIN_MAX_PRODUIT` pour le declencher sur un petit graphe.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test produit_cartesien_bornes_228
//! ```
#![cfg(feature = "cypher")]

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const N: i64 = 60;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher(&format!(
        "CREATE (:Project {{id: 'p'}}) WITH 1 AS x \
         UNWIND range(1, {N}) AS i CREATE (:File {{path: 'f' + toString(i)}})"
    ))
    .unwrap();
    s.execute_cypher("MATCH (p:Project), (f:File) CREATE (p)-[:CONTAINS]->(f)")
        .unwrap();
    s.execute_cypher(
        "MATCH (a:File {path: 'f1'}), (b:File {path: 'f2'}), (c:File {path: 'f3'}) \
         CREATE (a)-[:CO_CHANGED {count: 5}]->(b), (a)-[:CO_CHANGED {count: 3}]->(c), \
                (a)-[:IMPORTS]->(b), (b)-[:IMPORTS]->(c)",
    )
    .unwrap();
    db
}

fn params() -> HashMap<String, Value> {
    HashMap::from([
        ("path".to_string(), Value::from("f1")),
        ("project_id".to_string(), Value::from("p")),
    ])
}

/// La requete exacte de l'issue, sous un plafond inferieur a (N+1)^2.
#[test]
#[allow(unsafe_code)]
fn code_44_extremites_liees() {
    // SAFETY du test : lu a la planification, seul test de ce binaire a le lire.
    unsafe { std::env::set_var("OBRAIN_MAX_PRODUIT", "3000") };
    let db = base();
    let q = "MATCH (f:File {path: $path})<-[:CONTAINS]-(p:Project {id: $project_id}) \
             MATCH (f)-[r:CO_CHANGED]-(co:File) WHERE EXISTS { MATCH (co)<-[:CONTAINS]-(p) } \
             WITH f, co ORDER BY r.count DESC LIMIT 10 \
             OPTIONAL MATCH sp = shortestPath((f)-[:IMPORTS|CALLS*1..5]-(co)) \
             WITH co.path AS co_path, CASE WHEN sp IS NOT NULL THEN 1.0 / length(sp) ELSE 0.0 END AS proximity \
             RETURN co_path, proximity ORDER BY proximity DESC";
    let rows = db
        .execute_cypher_with_params(q, params())
        .unwrap_or_else(|e| panic!("{e}"))
        .rows;
    assert_eq!(
        rows,
        vec![
            vec![Value::from("f2"), Value::Float64(1.0)],
            vec![Value::from("f3"), Value::Float64(0.5)],
        ]
    );
}

/// Un vrai produit non borne reste refuse sous le meme plafond.
#[test]
#[allow(unsafe_code)]
fn vrai_produit_non_borne_reste_refuse() {
    // SAFETY : voir ci-dessus.
    unsafe { std::env::set_var("OBRAIN_MAX_PRODUIT", "3000") };
    let db = base();
    let err = db
        .execute_cypher_with_params("MATCH (a), (b) RETURN count(*)", params())
        .unwrap_err()
        .to_string();
    assert!(err.contains("refus"), "{err}");
}

fn cypher(db: &ObrainDB, q: &str) -> Result<Vec<Vec<Value>>, String> {
    db.execute_cypher_with_params(q, params())
        .map(|r| r.rows)
        .map_err(|e| e.to_string())
}

#[allow(unsafe_code)]
fn plafond_bas() -> ObrainDB {
    // SAFETY : meme valeur dans tous les tests de ce binaire.
    unsafe { std::env::set_var("OBRAIN_MAX_PRODUIT", "3000") };
    base()
}

/// Plusieurs lignes de gauche : une recherche par ligne, valeurs exactes ;
/// une ligne sans chemin reste, a NULL.
#[test]
fn plusieurs_lignes_dont_une_sans_chemin() {
    let db = plafond_bas();
    let rows = cypher(
        &db,
        "UNWIND ['f1', 'f2', 'f4'] AS pa MATCH (a:File {path: pa}) MATCH (b:File {path: 'f3'}) \
         OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(b)) \
         RETURN a.path AS p, length(sp) AS l ORDER BY p",
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Value::from("f1"), Value::Int64(2)],
            vec![Value::from("f2"), Value::Int64(1)],
            vec![Value::from("f4"), Value::Null],
        ]
    );
}

/// Sens et types respectes ; `allShortestPaths` aussi.
#[test]
fn sens_types_et_all_shortest_paths() {
    let db = plafond_bas();
    let longueur = |motif: &str| {
        let q = format!(
            "MATCH (a:File {{path: 'f1'}}) MATCH (b:File {{path: 'f3'}}) \
             OPTIONAL MATCH sp = {motif} RETURN length(sp) AS l"
        );
        cypher(&db, &q).unwrap()
    };
    let deux = vec![vec![Value::Int64(2)]];
    let nul = vec![vec![Value::Null]];
    assert_eq!(longueur("shortestPath((a)-[:IMPORTS*1..5]->(b))"), deux);
    assert_eq!(longueur("shortestPath((a)<-[:IMPORTS*1..5]-(b))"), nul);
    assert_eq!(
        longueur("shortestPath((a)-[:CO_CHANGED*1..5]->(b))"),
        vec![vec![Value::Int64(1)]]
    );
    assert_eq!(longueur("shortestPath((a)-[:AUCUN*1..5]-(b))"), nul);
    assert_eq!(longueur("allShortestPaths((a)-[:IMPORTS*1..5]-(b))"), deux);
}

/// Un OPTIONAL MATCH ordinaire est inchange.
#[test]
fn optional_match_ordinaire_inchange() {
    let db = plafond_bas();
    let rows = cypher(
        &db,
        "MATCH (a:File {path: 'f1'}) OPTIONAL MATCH (a)-[:IMPORTS]->(b) \
         RETURN a.path AS a, b.path AS b",
    )
    .unwrap();
    assert_eq!(rows, vec![vec![Value::from("f1"), Value::from("f2")]]);
    let rows = cypher(
        &db,
        "MATCH (a:File {path: 'f3'}) OPTIONAL MATCH (a)-[:IMPORTS]->(b) \
         RETURN a.path AS a, b.path AS b",
    )
    .unwrap();
    assert_eq!(rows, vec![vec![Value::from("f3"), Value::Null]]);
}

/// Extremite NON liee : vrai produit, reste refuse, et le message n'affirme
/// plus que les parties ne partagent aucune variable.
#[test]
fn extremite_non_liee_reste_refusee() {
    let db = plafond_bas();
    let err = cypher(
        &db,
        "MATCH (f:File {path: 'f1'}) \
         OPTIONAL MATCH sp = shortestPath((f)-[:IMPORTS*1..5]-(x:File)) RETURN length(sp)",
    )
    .unwrap_err();
    assert!(err.contains("refus"), "{err}");
    assert!(!err.contains("ne partagent aucune variable"), "{err}");
}

/// `sp IS NULL` / `sp IS NOT NULL` suivent l'existence du chemin.
#[test]
fn sp_is_null_suit_l_existence_du_chemin() {
    let db = plafond_bas();
    let rows = cypher(
        &db,
        "UNWIND ['f1', 'f4'] AS pa MATCH (a:File {path: pa}) MATCH (b:File {path: 'f3'}) \
         OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(b)) \
         RETURN a.path AS p, sp IS NULL AS nul, sp IS NOT NULL AS pas_nul ORDER BY p",
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Value::from("f1"), Value::Bool(false), Value::Bool(true)],
            vec![Value::from("f4"), Value::Bool(true), Value::Bool(false)],
        ]
    );
}
