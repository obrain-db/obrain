//! #357 — une compréhension de motif doit être réécrite OÙ QU'ELLE SOIT dans
//! une expression, pas seulement à la racine d'un item ou sous `size(…)`.
//!
//! Avant le correctif, ces formes atteignaient le planificateur telles quelles :
//! `OBRAIN-X001: Internal error: PatternComprehension reached the planner
//! without being rewritten; this is a bug in the Cypher translator`.
//!
//! Mesuré sur `main` `c4de4e72` : sur onze variantes, dix échouaient. La passe de
//! réécriture ne descendait que dans `Binary`, `Unary` (détecté, mais sans bras
//! de réécriture) et `FunctionCall` ; elle ignorait l'accès par indice
//! `[…][0]`, la tranche `[…][0..1]`, `CASE`, la liste et la map littérales.
//!
//! Hors périmètre de ce fichier : le `WHERE` d'un `OPTIONAL MATCH`, où la
//! réécriture est volontairement sautée (voir `translate_where`).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test comprehension_motif_357
//! ```

#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// `c1` a deux arêtes `D` et un parent (`SP`) ; `parent` n'a ni `D` ni `SP`.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (c:CS {id: 'c1'})-[:D]->(:X), (c)-[:D]->(:X), (c)-[:SP]->(:CS {id: 'parent'})",
        )
        .expect("jeu de départ");
    db
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn jouer(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

/// La requête de Project Orchestrator (`chat_23_get_chat_graph_data_sessions`),
/// réduite : compréhension INDEXÉE dans un RETURN qui suit un WITH agrégeant.
#[test]
fn forme_1_de_l_issue_indexee_apres_with_agregeant() {
    let lignes = jouer(
        &base(),
        "MATCH (c:CS)-[d:D]->() WITH c, count(d) AS n WHERE n > 0 \
         RETURN c.id AS id, [(c)-[:SP]->(p:CS) | p.id][0] AS parent ORDER BY c.id",
    );
    assert_eq!(lignes, vec![vec![s("c1"), s("parent")]]);
}

#[test]
fn indexee_sans_with() {
    let lignes = jouer(
        &base(),
        "MATCH (c:CS {id: 'c1'}) RETURN c.id AS id, [(c)-[:SP]->(p:CS) | p.id][0] AS parent",
    );
    assert_eq!(lignes, vec![vec![s("c1"), s("parent")]]);
}

#[test]
fn indice_hors_limites_rend_null() {
    // `parent` n'a aucun parent : la liste est vide, `[0]` est NULL, coalesce
    // rend la valeur de repli. Prouve la SÉMANTIQUE, pas seulement l'absence
    // d'erreur interne.
    let lignes = jouer(
        &base(),
        "MATCH (c:CS {id: 'parent'}) \
         RETURN coalesce([(c)-[:SP]->(p:CS) | p.id][0], 'aucun') AS a",
    );
    assert_eq!(lignes, vec![vec![s("aucun")]]);
}

#[test]
fn coalesce_d_une_comprehension_indexee() {
    let lignes = jouer(
        &base(),
        "MATCH (c:CS {id: 'c1'}) RETURN coalesce([(c)-[:SP]->(p:CS) | p.id][0], 'aucun') AS a",
    );
    assert_eq!(lignes, vec![vec![s("parent")]]);
}

#[test]
fn tranche() {
    let lignes = jouer(
        &base(),
        "MATCH (c:CS {id: 'c1'}) RETURN [(c)-[:SP]->(p:CS) | p.id][0..1] AS parents",
    );
    assert_eq!(lignes, vec![vec![Value::List(vec![s("parent")].into())]]);
}

#[test]
fn not_en_return() {
    // `c1` a un parent : size = 1, `size = 0` est faux, NOT rend vrai.
    let lignes = jouer(
        &base(),
        "MATCH (c:CS {id: 'c1'}) RETURN NOT (size([(c)-[:SP]->(p:CS) | p]) = 0) AS a",
    );
    assert_eq!(lignes, vec![vec![Value::Bool(true)]]);
}

#[test]
fn not_en_where() {
    // Seul `c1` a un parent.
    let lignes = jouer(
        &base(),
        "MATCH (c:CS) WHERE NOT (size([(c)-[:SP]->(p:CS) | p]) = 0) RETURN c.id",
    );
    assert_eq!(lignes, vec![vec![s("c1")]]);
}

#[test]
fn case_contenant_une_comprehension() {
    let lignes = jouer(
        &base(),
        "MATCH (c:CS) RETURN c.id AS id, \
         CASE WHEN size([(c)-[:SP]->(p:CS) | p]) > 0 THEN 'oui' ELSE 'non' END AS a \
         ORDER BY id",
    );
    // `c1` a un parent, `parent` non. L'ordre est celui de `id` : c1 < parent.
    assert_eq!(
        lignes,
        vec![vec![s("c1"), s("oui")], vec![s("parent"), s("non")]]
    );
}

#[test]
fn liste_litterale_contenant_une_comprehension() {
    let lignes = jouer(
        &base(),
        "MATCH (c:CS {id: 'c1'}) RETURN [size([(c)-[:SP]->(p:CS) | p]), 7] AS a",
    );
    assert_eq!(
        lignes,
        vec![vec![Value::List(
            vec![Value::Int64(1), Value::Int64(7)].into()
        )]]
    );
}

#[test]
fn la_forme_deja_couverte_ne_regresse_pas() {
    // `size([…])` à la racine d'un WHERE passait déjà (#195, famille 2).
    let lignes = jouer(
        &base(),
        "MATCH (c:CS) WHERE size([(c)-[:SP]->(p:CS) | p]) = 1 RETURN c.id",
    );
    assert_eq!(lignes, vec![vec![s("c1")]]);
}
