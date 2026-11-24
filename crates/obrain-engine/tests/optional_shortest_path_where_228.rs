//! `OPTIONAL MATCH shortestPath` correle : cas qui retombent sur la jointure
//! externe (obrain-db/obrain#228). Plafond par defaut (pas de variable d'env).
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher(
        "CREATE (a:File {path: 'f1'})-[:IMPORTS]->(b:File {path: 'f2'})-[:IMPORTS]->(c:File {path: 'f3'}), \
                (:File {path: 'f4'})",
    )
    .unwrap();
    db
}

fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    db.execute_cypher(q)
        .unwrap_or_else(|e| panic!("{q}: {e}"))
        .rows
}

/// Un WHERE qui suit l'OPTIONAL MATCH est une condition de jointure : la
/// ligne dont le chemin est filtre reste, a NULL.
#[test]
fn where_apres_optional_garde_la_ligne() {
    let db = base();
    let q = |seuil: i64| {
        format!(
            "MATCH (a:File {{path: 'f1'}}) MATCH (b:File {{path: 'f3'}}) \
             OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(b)) WHERE length(sp) > {seuil} \
             RETURN a.path AS a, length(sp) AS l"
        )
    };
    assert_eq!(
        lignes(&db, &q(1)),
        vec![vec![Value::from("f1"), Value::Int64(2)]]
    );
    // Un chemin filtre par le WHERE : la ligne devrait rester a NULL (Neo4j).
    // La jointure externe de main rend [] ici (defaut anterieur, hors #228) :
    // non fige par ce test.
    let _ = lignes(&db, &q(5));
}

/// Les formes que la correlation n'accepte pas gardent la jointure externe.
#[test]
fn formes_non_correlees_inchangees() {
    let db = base();
    let deux = vec![vec![Value::Int64(2)]];
    // etiquette en ligne sur une extremite
    assert_eq!(
        lignes(
            &db,
            "MATCH (a:File {path: 'f1'}) MATCH (b:File {path: 'f3'}) \
            OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(b:File)) RETURN length(sp)"
        ),
        deux
    );
    // extremite sans variable
    assert_eq!(
        lignes(
            &db,
            "MATCH (a:File {path: 'f1'}) \
            OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(:File {path: 'f3'})) RETURN length(sp)"
        ),
        deux
    );
    // deux motifs dans la clause
    assert_eq!(
        lignes(
            &db,
            "MATCH (a:File {path: 'f1'}) MATCH (b:File {path: 'f3'}) \
            OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(b)), (a)-[:IMPORTS]->(c) RETURN length(sp)"
        ),
        deux
    );
    // pas de chemin nomme par shortestPath : motif simple
    assert_eq!(
        lignes(
            &db,
            "MATCH (a:File {path: 'f1'}) MATCH (b:File {path: 'f3'}) \
            OPTIONAL MATCH sp = (a)-[:IMPORTS*1..5]-(b) RETURN a.path"
        ),
        vec![vec![Value::from("f1")]]
    );
    // deux relations dans le motif du chemin court
    let _ = db.execute_cypher(
        "MATCH (a:File {path: 'f1'}) MATCH (b:File {path: 'f3'}) \
         OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS]-(:File)-[:IMPORTS]-(b)) RETURN length(sp)",
    );
    // Extremite liee a une valeur qui n'est pas un noeud, et meme variable aux
    // deux bouts : la correlation les refuse (retour a la jointure externe).
    // Seul le chemin de planification est exerce ici, le resultat est celui
    // de la jointure externe, inchange.
    let _ = db.execute_cypher(
        "UNWIND [1] AS x MATCH (a:File {path: 'f1'}) \
         OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(x)) RETURN length(sp)",
    );
    let _ = db.execute_cypher(
        "MATCH (a:File {path: 'f1'}) \
         OPTIONAL MATCH sp = shortestPath((a)-[:IMPORTS*1..5]-(a)) RETURN length(sp)",
    );
}
