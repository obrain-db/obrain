//! Propriété d'un élément de liste dont l'identifiant est commun à un nœud et
//! à une arête (obrain-db/obrain#229, et #195 famille 6 : `chat_11`).
//!
//! Nœuds et arêtes voyagent dans une liste comme leur identifiant entier, et
//! les deux séries partent de 1 : `startNode(rels[i]).id` ou `rels[0].k`
//! tombaient sur un identifiant qui désigne À LA FOIS un nœud et une arête, et
//! l'évaluateur refusait de choisir. Le traducteur Cypher sait pourtant, par
//! la forme de l'expression, ce que porte la liste : il le dit maintenant à
//! l'évaluateur.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test identifiant_noeud_arete_229
//! ```
#![cfg(feature = "cypher")]

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// root <-[e1]- c1 <-[e2]- c2 : les nœuds 1..3 et les arêtes 1..2 se
/// recoupent.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (:ChatSession {id: 'root'})<-[:SPAWNED_BY {k: 'e1'}]-\
                    (:ChatSession {id: 'c1'})<-[:SPAWNED_BY {k: 'e2'}]-\
                    (:ChatSession {id: 'c2'})",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    let mut rows = db
        .execute_cypher_with_params(
            requete,
            HashMap::from([("id".to_string(), Value::from("root"))]),
        )
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows;
    rows.sort_by_key(|l| format!("{l:?}"));
    rows
}

fn s(v: &str) -> Value {
    Value::from(v)
}

/// Les identifiants se recoupent bien : sans cela le test ne prouve rien.
#[test]
fn les_identifiants_se_recoupent() {
    let db = base();
    let noeuds = lignes(&db, "MATCH (n) RETURN id(n)");
    let aretes = lignes(&db, "MATCH ()-[e]->() RETURN id(e)");
    assert!(aretes.iter().all(|a| noeuds.contains(a)), "{noeuds:?} {aretes:?}");
}

/// La requête de Project Orchestrator (`chat_11_get_session_tree`).
#[test]
fn chat_11_start_node_du_dernier_element() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH path = (root:ChatSession {id: $id})<-[:SPAWNED_BY*0..10]-(child:ChatSession) \
         WITH child, relationships(path) AS rels \
         RETURN child.id, \
                CASE WHEN size(rels) = 0 THEN null \
                     ELSE startNode(rels[size(rels)-1]).id END AS self_id",
    );
    assert_eq!(
        rows,
        vec![
            vec![s("c1"), s("c1")],
            vec![s("c2"), s("c2")],
            vec![s("root"), Value::Null],
        ]
    );
}

#[test]
fn element_de_liste_d_aretes_et_de_noeuds() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH path = (root:ChatSession {id: $id})<-[:SPAWNED_BY*2]-(child:ChatSession) \
         WITH path, relationships(path) AS rels \
         RETURN rels[0].k, last(rels).k, endNode(rels[0]).id, \
                head(nodes(path)).id, nodes(path)[1].id",
    );
    assert_eq!(rows, vec![vec![s("e1"), s("e2"), s("root"), s("root"), s("c1")]]);
}

/// `collect` d'une arête puis d'un nœud, élément relu après un `WITH`.
#[test]
fn collect_puis_element() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH (a:ChatSession)-[r:SPAWNED_BY]->(b:ChatSession {id: 'root'}) \
         WITH collect(r) AS rs, collect(a) AS ns \
         RETURN head(rs).k, ns[0].id",
    );
    assert_eq!(rows, vec![vec![s("e1"), s("c1")]]);
}


/// `*0..n` rend la correspondance de longueur nulle (la source elle-même),
/// et seulement si elle satisfait le label de la cible.
#[test]
fn chemin_de_longueur_nulle() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH p = (r:ChatSession {id: 'root'})<-[:SPAWNED_BY*0..1]-(c) \
         RETURN c.id, length(p), size(relationships(p))",
    );
    assert_eq!(
        rows,
        vec![
            vec![s("c1"), Value::Int64(1), Value::Int64(1)],
            vec![s("root"), Value::Int64(0), Value::Int64(0)],
        ]
    );
    let rows = lignes(
        &db,
        "MATCH (r:ChatSession {id: 'root'})<-[:SPAWNED_BY*0..1]-(c:Autre) RETURN c",
    );
    assert!(rows.is_empty(), "{rows:?}");
    let rows = lignes(
        &db,
        "MATCH (r:ChatSession {id: 'c1'})-[:SPAWNED_BY*0..]->(c) RETURN c.id",
    );
    assert_eq!(rows, vec![vec![s("c1")], vec![s("root")]]);
}
