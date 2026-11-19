//! Éléments d'une liste de nœuds / d'arêtes : ils résolvent leurs propriétés
//! (obrain-db/obrain#189, requêtes réelles `get_propagated_notes` de PO).
//!
//! `nodes(path)` / `relationships(path)` rendent des IDENTIFIANTS, pas des
//! entités : le modèle de valeurs n'a pas de variante nœud / arête. Une
//! compréhension sur cette liste perdait donc tout accès à une propriété
//! (`[n IN nodes(p) | n.id]` rendait `[]`, `coalesce(node.name, …)` rendait
//! `[null, null]`), et `reduce(… | s + n.pagerank)` sommait des zéros.
//!
//! Les requêtes de PO qui en dépendent, mot pour mot :
//!
//! ```text
//! [node IN nodes(path) | coalesce(node.name, node.path, node.id)] AS path_names
//! [r IN relationships(path) | type(r)] AS rel_types
//! reduce(s = 0.0, node IN nodes(path) | s + coalesce(node.pagerank, 0.05))
//! ```
#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

/// Rend les valeurs d'une requête sous forme lisible (`[[A, B]]`).
fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<String>> {
    let session = db.session();
    let r = session.execute(q).unwrap_or_else(|e| panic!("{q}\n{e}"));
    r.rows
        .iter()
        .map(|ligne| ligne.iter().map(rendu).collect())
        .collect()
}

fn rendu(v: &obrain_common::types::Value) -> String {
    match v {
        obrain_common::types::Value::List(items) => {
            let dedans: Vec<String> = items.iter().map(rendu).collect();
            format!("[{}]", dedans.join(", "))
        }
        obrain_common::types::Value::String(s) => s.to_string(),
        obrain_common::types::Value::Null => "null".to_string(),
        autre => format!("{autre:?}"),
    }
}

/// `a -[T]-> b -[T]-> c`, avec des propriétés sur les trois nœuds.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute(
            "CREATE (a:N {id:'a', name:'A', pagerank: 0.5})\
             -[:T {w: 1.0}]->(b:N {id:'b', name:'B', pagerank: 0.2})\
             -[:T {w: 2.0}]->(c:N {id:'c', name:'C', pagerank: 0.3})",
        )
        .unwrap_or_else(|e| panic!("création : {e}"));
    db
}

#[test]
fn propriete_des_noeuds_d_un_chemin() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH p = (a:N {id:'a'})-[:T]->(b) RETURN [n IN nodes(p) | n.id]"
        ),
        vec![vec!["[a, b]".to_string()]]
    );
}

#[test]
fn coalesce_sur_les_noeuds_d_un_chemin() {
    let db = base();
    // Exprimé comme PO : le nom s'il existe, sinon l'identifiant.
    assert_eq!(
        lignes(
            &db,
            "MATCH p = (a:N {id:'a'})-[:T]->(b) \
             RETURN [node IN nodes(p) | coalesce(node.name, node.path, node.id)]"
        ),
        vec![vec!["[A, B]".to_string()]]
    );
}

#[test]
fn type_des_aretes_d_un_chemin() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH p = (a:N {id:'a'})-[:T]->(b) RETURN [r IN relationships(p) | type(r)]"
        ),
        vec![vec!["[T]".to_string()]]
    );
}

#[test]
fn reduce_sur_une_propriete_des_noeuds() {
    let db = base();
    // 0.5 + 0.2 = 0.7 ; sans résolution des propriétés la somme valait 0.1.
    assert_eq!(
        lignes(
            &db,
            "MATCH p = (a:N {id:'a'})-[:T]->(b) \
             RETURN reduce(s = 0.0, node IN nodes(p) | s + coalesce(node.pagerank, 0.0))"
        ),
        vec![vec!["Float64(0.7)".to_string()]]
    );
}

#[test]
fn propriete_des_aretes_et_poids() {
    let db = base();
    // Un seul segment : le chemin nommé sur PLUSIEURS segments est traité
    // par la tranche dédiée (composition du chemin), pas ici.
    assert_eq!(
        lignes(
            &db,
            "MATCH p = (a:N {id:'a'})-[:T]->(b) \
             RETURN [r IN relationships(p) | type(r)], \
                    reduce(w = 1.0, r IN relationships(p) | w * coalesce(r.w, 0.5))"
        ),
        vec![vec!["[T]".to_string(), "Float64(1)".to_string()]]
    );
}

#[test]
fn une_liste_d_entiers_reste_une_liste_d_entiers() {
    let db = base();
    // Régression : la résolution par identifiant ne doit pas s'appliquer aux
    // valeurs qui ne sont ni un nœud ni une arête (ici des entiers nus).
    assert_eq!(
        lignes(&db, "RETURN [x IN [1, 2, 3] | x * 2]"),
        vec![vec!["[Int64(2), Int64(4), Int64(6)]".to_string()]]
    );
}

#[test]
fn une_propriete_absente_reste_null() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH p = (a:N {id:'a'})-[:T]->(b) RETURN [n IN nodes(p) | n.absente]"
        ),
        vec![vec!["[null, null]".to_string()]]
    );
}
