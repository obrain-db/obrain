//! Paramètre lié (`$p`) dans une sous-requête de filtre (obrain-db/obrain#227).
//!
//! `substitute_in_expression` ne descendait pas dans les sous-plans portés par
//! une expression : `EXISTS { … WHERE x.id = $p }`, le prédicat de motif
//! `(n)-[:T]->(:L {id: $p})`, sa négation `NOT (…)` et la compréhension de
//! motif. Le `$p` arrivait tel quel au planificateur, qui refusait la requête
//! (« Parameters not yet supported in filters », OBRAIN-X001).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test parametres_filtres_227
//! ```
#![cfg(feature = "cypher")]

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// n1 -[:INSIDE]-> p1, n2 -[:INSIDE]-> p2, n3 isolé.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (:N {id: 'n1'})-[:INSIDE]->(:P {id: 'p1'}), \
                    (:N {id: 'n2'})-[:INSIDE]->(:P {id: 'p2'}), \
                    (:N {id: 'n3'})",
        )
        .unwrap();
    db
}

fn ids(db: &ObrainDB, requete: &str, p: &str) -> Vec<String> {
    let mut params = HashMap::new();
    params.insert("p".to_string(), Value::from(p));
    let mut out: Vec<String> = db
        .execute_cypher_with_params(requete, params)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("texte attendu, reçu {autre:?}"),
        })
        .collect();
    out.sort();
    out
}

#[test]
fn exists_bloc_avec_parametre() {
    let db = base();
    let q = "MATCH (n:N) WHERE EXISTS { MATCH (n)-[:INSIDE]->(x:P) WHERE x.id = $p } RETURN n.id";
    assert_eq!(ids(&db, q, "p1"), ["n1"]);
    assert_eq!(ids(&db, q, "p2"), ["n2"]);
    assert_eq!(ids(&db, q, "absent"), Vec::<String>::new());
}

#[test]
fn predicat_de_motif_avec_parametre() {
    let db = base();
    let q = "MATCH (n:N) WHERE (n)-[:INSIDE]->(:P {id: $p}) RETURN n.id";
    assert_eq!(ids(&db, q, "p2"), ["n2"]);
}

#[test]
fn predicat_de_motif_nie_avec_parametre() {
    let db = base();
    let q = "MATCH (n:N) WHERE NOT (n)-[:INSIDE]->(:P {id: $p}) RETURN n.id";
    assert_eq!(ids(&db, q, "p1"), ["n2", "n3"]);
}

#[test]
fn comprehension_de_motif_avec_parametre() {
    let db = base();
    let q = "MATCH (n:N) WITH n, [(n)-[:INSIDE]->(x:P) WHERE x.id = $p | x.id] AS xs \
             WHERE size(xs) > 0 RETURN n.id";
    assert_eq!(ids(&db, q, "p1"), ["n1"]);
}

/// Un paramètre manquant dans la sous-requête est signalé par son nom, comme
/// au niveau principal, au lieu de l'erreur interne du planificateur.
#[test]
fn parametre_manquant_dans_sous_requete_nomme() {
    let db = base();
    let err = db
        .execute_cypher_with_params(
            "MATCH (n:N) WHERE EXISTS { MATCH (n)-[:INSIDE]->(x:P) WHERE x.id = $autre } RETURN n.id",
            HashMap::from([("p".to_string(), Value::from("p1"))]),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("$autre"), "{err}");
}
