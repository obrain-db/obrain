//! Sens d'une arête créée avec une flèche GAUCHE (défaut découvert en
//! investiguant #195 famille 7 : `CREATE (x)<-[:R]-(y)` stockait x -> y).
//!
//! La flèche d'une création décide du sens : `(x)<-[:R]-(y)` crée y -> x.
//! Une flèche droite reste inchangée (témoin). Vérifié en Cypher (CREATE)
//! et en GQL (INSERT), puis relu par un MATCH dans le sens écrit.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test sens_arete_creation
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn paires(db: &ObrainDB, langage: &str, requete: &str) -> Vec<(String, String)> {
    let r = db
        .session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"));
    r.rows
        .into_iter()
        .map(|l| match (&l[0], &l[1]) {
            (Value::String(a), Value::String(b)) => (a.to_string(), b.to_string()),
            autre => panic!("texte attendu, reçu {autre:?}"),
        })
        .collect()
}

#[test]
fn cypher_fleche_gauche_cree_dans_le_bon_sens() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (x:N {k: 'x'})<-[:R]-(y:N {k: 'y'})")
        .unwrap();
    // La flèche gauche : y -> x.
    assert_eq!(
        paires(
            &db,
            "cypher",
            "MATCH (a:N)-[:R]->(b:N) RETURN a.k AS a, b.k AS b"
        ),
        vec![("y".to_string(), "x".to_string())]
    );
}

#[test]
fn gql_fleche_gauche_insert_dans_le_bon_sens() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_language("INSERT (x:N {k: 'x'})<-[:R]-(y:N {k: 'y'})", "gql", None)
        .unwrap();
    assert_eq!(
        paires(
            &db,
            "gql",
            "MATCH (a:N)-[:R]->(b:N) RETURN a.k AS a, b.k AS b"
        ),
        vec![("y".to_string(), "x".to_string())]
    );
}

/// Témoin : la flèche droite est inchangée.
#[test]
fn fleche_droite_inchangee() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (x:N {k: 'x'})-[:R]->(y:N {k: 'y'})")
        .unwrap();
    assert_eq!(
        paires(
            &db,
            "cypher",
            "MATCH (a:N)-[:R]->(b:N) RETURN a.k AS a, b.k AS b"
        ),
        vec![("x".to_string(), "y".to_string())]
    );
}
