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
    assert!(
        aretes.iter().all(|a| noeuds.contains(a)),
        "{noeuds:?} {aretes:?}"
    );
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
    assert_eq!(
        rows,
        vec![vec![s("e1"), s("e2"), s("root"), s("root"), s("c1")]]
    );
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

// ---------------------------------------------------------------------------
// Réouverture de #229 : la liste garde son genre au-delà de la forme de
// `chat_11`. Bases neuves, ids 1 et 2 communs à un nœud et à une arête.
// ---------------------------------------------------------------------------

fn requete(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    let mut rows = db
        .execute_cypher(q)
        .unwrap_or_else(|e| panic!("{q}\n{e}"))
        .rows;
    rows.sort_by_key(|l| format!("{l:?}"));
    rows
}

/// `ana_09_get_circular_dependencies` : `nodes(path)` passé par deux `WITH`
/// puis lu par une compréhension.
#[test]
fn ana_09_cycle_par_comprehension() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (a:F {p: 'a'})-[:I]->(b:F {p: 'b'})-[:I]->(a)")
        .unwrap();
    let rows = requete(
        &db,
        "MATCH path = (f:F)-[:I*2..3]->(f) WITH nodes(path) AS ns \
         WITH [n IN ns | n.p] AS cycle RETURN DISTINCT cycle ORDER BY size(cycle) LIMIT 10",
    );
    assert!(!rows.is_empty(), "{rows:?}");
    for r in &rows {
        let Value::List(l) = &r[0] else {
            panic!("{r:?}")
        };
        assert!(l.iter().all(|v| matches!(v, Value::String(_))), "{r:?}");
    }
}

/// `ws_14_get_workspace_milestone_progress` : `collect(DISTINCT t)` filtré par
/// `IS NOT NULL`, puis prédicat dans une compréhension.
#[test]
fn ws_14_collect_distinct_filtre_puis_comprehension() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (w:W {id: 'w'})-[:IT]->(:T {status: 'C'}), (w)-[:IT]->(:T {status: 'P'})",
        )
        .unwrap();
    let rows = requete(
        &db,
        "MATCH (w:W {id: 'w'}) OPTIONAL MATCH (w)-[:IT]->(t1:T) \
         WITH [x IN collect(DISTINCT t1) WHERE x IS NOT NULL] AS tasks \
         RETURN size(tasks) AS total, size([t IN tasks WHERE t.status = 'C']) AS c",
    );
    assert_eq!(rows, vec![vec![Value::Int64(2), Value::Int64(1)]]);
}

/// Voisins : la même forme avec des arêtes doit continuer à marcher.
#[test]
fn comprehension_sur_liste_d_aretes() {
    let db = base();
    let rows = lignes(
        &db,
        "MATCH path = (root:ChatSession {id: $id})<-[:SPAWNED_BY*2]-(c) \
         WITH relationships(path) AS rs WITH [r IN rs | r.k] AS ks RETURN ks",
    );
    assert_eq!(rows, vec![vec![Value::List(vec![s("e1"), s("e2")].into())]]);
}

/// Prédicat de liste (`any` / `all`) sur une liste prouvée nœuds, relue après
/// un `WITH` : même cause, autre opérateur.
#[test]
fn predicat_de_liste_apres_with() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (a:F {p: 'a'})-[:I {p: 'x'}]->(b:F {p: 'b'})-[:I {p: 'y'}]->(a)")
        .unwrap();
    let rows = requete(
        &db,
        "MATCH path = (f:F {p: 'a'})-[:I*2]->(f) WITH nodes(path) AS ns, relationships(path) AS rs \
         RETURN any(n IN ns WHERE n.p = 'b'), all(n IN ns WHERE n.p = 'a'), \
                any(r IN rs WHERE r.p = 'y')",
    );
    assert_eq!(
        rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true)
        ]]
    );
}
