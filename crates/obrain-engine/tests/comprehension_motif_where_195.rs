//! Compréhension de motif dans un `WHERE` (obrain-db/obrain#195, famille 2).
//!
//! `WHERE n.project_id IN [(w:Workspace {slug: $s})<-[:BELONGS_TO_WORKSPACE]-(p) | p.id]`
//! (PO `note_09`, `note_10`, `dec_13`, `chat_23`, `proto_05`) mourait en
//! « PatternComprehension reached the planner without being rewritten » : seuls
//! `RETURN` et `WITH` réécrivaient la compréhension. Et la réécriture corrélait
//! toujours sur le premier nœud, même quand il n'est pas lié dehors (`w`).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test comprehension_motif_where_195
//! ```
#![cfg(feature = "cypher")]

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Workspace w1 <- p1, p2 ; w2 <- p3. Notes : n1 (p1), n2 (p3), n3 (p2), n4 (sans projet).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (w1:Workspace {slug: 'w1'}), (w2:Workspace {slug: 'w2'}), \
                    (p1:Project {id: 'p1'})-[:BELONGS_TO_WORKSPACE]->(w1), \
                    (p2:Project {id: 'p2'})-[:BELONGS_TO_WORKSPACE]->(w1), \
                    (p3:Project {id: 'p3'})-[:BELONGS_TO_WORKSPACE]->(w2), \
                    (:Note {id: 'n1', project_id: 'p1'}), (:Note {id: 'n2', project_id: 'p3'}), \
                    (:Note {id: 'n3', project_id: 'p2'}), (:Note {id: 'n4'}), \
                    (n5:Note {id: 'n5'})-[:LINKS]->(:Tag {name: 'x'})",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    let mut rows = db
        .execute_cypher_with_params(
            requete,
            HashMap::from([("s".to_string(), Value::from("w1"))]),
        )
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows;
    rows.sort_by_key(|l| format!("{l:?}"));
    rows
}

fn v(s: &str) -> Value {
    Value::from(s)
}

/// La forme de `note_09` : compréhension NON corrélée, dans un `WHERE`.
#[test]
fn note_09_non_correlee() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH (n:Note) \
         WHERE n.project_id IN [(w:Workspace {slug: $s})<-[:BELONGS_TO_WORKSPACE]-(proj:Project) | proj.id] \
         RETURN count(n) AS total",
    );
    assert_eq!(rows, vec![vec![Value::Int64(2)]]);
}

/// Compréhension corrélée sur la variable du `MATCH`.
#[test]
fn correlee_dans_where() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH (n:Note) WHERE size([(n)-[:LINKS]->(t:Tag) | t.name]) > 0 RETURN n.id",
    );
    assert_eq!(rows, vec![vec![v("n5")]]);
    let rows = lignes(
        &db,
        "MATCH (n:Note) WHERE 'x' IN [(n)-[:LINKS]->(t:Tag) | t.name] OR n.id = 'n1' RETURN n.id",
    );
    assert_eq!(rows, vec![vec![v("n1")], vec![v("n5")]]);
}

#[test]
fn dans_le_where_d_un_with() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH (n:Note) WITH n \
         WHERE n.project_id IN [(w:Workspace {slug: $s})<-[:BELONGS_TO_WORKSPACE]-(p:Project) | p.id] \
         RETURN n.id",
    );
    assert_eq!(rows, vec![vec![v("n1")], vec![v("n3")]]);
}

/// Non corrélée dans un `RETURN` : la même liste pour chaque ligne.
#[test]
fn non_correlee_dans_return() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH (n:Note {id: 'n1'}) \
         RETURN size([(w:Workspace {slug: $s})<-[:BELONGS_TO_WORKSPACE]-(p:Project) | p.id]) AS k",
    );
    assert_eq!(rows, vec![vec![Value::Int64(2)]]);
}

/// Lié dehors ailleurs qu'au premier nœud : erreur nette, pas un résultat faux.
#[test]
fn correlee_ailleurs_qu_au_premier_noeud_refusee() {
    let db = base();
    let err = db
        .execute_cypher(
            "MATCH (n:Note) WHERE size([(t:Tag)<-[:LINKS]-(n) | t.name]) > 0 RETURN n.id",
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("'n'"), "{err}");
}

/// Non corrélée : calculée UNE fois (produit avec une ligne), pas rejouée par
/// un Apply à chaque ligne externe. Sur le graphe de code d'obrain (33 k
/// nœuds), la forme Apply ne finissait pas.
#[test]
fn non_correlee_calculee_une_fois() {
    let db = base();
    let plan = match &db
        .execute_cypher_with_params(
            "EXPLAIN MATCH (n:Note) \
             WHERE n.project_id IN [(w:Workspace {slug: $s})<-[:BELONGS_TO_WORKSPACE]-(p:Project) | p.id] \
             RETURN n.id",
            HashMap::from([("s".to_string(), Value::from("w1"))]),
        )
        .unwrap()
        .rows[0][0]
    {
        Value::String(s) => s.to_string(),
        autre => panic!("{autre:?}"),
    };
    assert!(!plan.contains("Apply"), "{plan}");
}
