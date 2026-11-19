//! `ORDER BY` sur une variable qu'une agrégation ne garde pas
//! (obrain-db/obrain#195, famille 7).
//!
//! ```cypher
//! MATCH (p:Project)-[r:HAS]->(n:Note) RETURN count(r) AS c ORDER BY r.since
//! ```
//!
//! Après agrégation, `r` n'est plus dans la portée : seule une erreur SÉMANTIQUE
//! est recevable. obrain rendait `OBRAIN-X001: Internal error: Variable 'r' not
//! found for ORDER BY property projection` — une erreur interne pour une requête
//! utilisateur, donc un message qui n'apprend rien et un code qu'aucun client ne
//! sait traiter autrement que comme « bug du moteur ».
//!
//! Les témoins fixent ce qui ne doit PAS changer : trier sur l'alias agrégé, et
//! trier sur une propriété de relation quand la variable est encore en portée.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test order_by_variable_hors_portee_195
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (p:Project {slug: 'p'}), (n1:Note {id: 'n1'}), (n2:Note {id: 'n2'}), \
             (p)-[:HAS {since: 2020}]->(n1), (p)-[:HAS {since: 2021}]->(n2)",
        )
        .unwrap();
    db
}

/// `true` si l'erreur est sémantique (requête fautive) et non interne (bug du moteur).
fn est_semantique(message: &str) -> bool {
    !message.contains("Internal error") && !message.contains("OBRAIN-X001")
}

#[test]
fn order_by_sur_variable_absorbe_par_l_agregat_est_une_erreur_semantique() {
    let db = base();
    let err = db
        .session()
        .execute_cypher("MATCH (:Project)-[r:HAS]->(:Note) RETURN count(r) AS c ORDER BY r.since")
        .expect_err("`r` n'est plus en portée après agrégation : la requête doit être refusée");
    let message = err.to_string();
    assert!(
        est_semantique(&message),
        "erreur interne pour une requête utilisateur : {message}"
    );
    assert!(
        message.contains("r"),
        "le message doit nommer la variable fautive : {message}"
    );
}

#[test]
fn temoin_tri_sur_l_alias_agrege_fonctionne() {
    let db = base();
    let r = db
        .session()
        .execute_cypher("MATCH (:Project)-[r:HAS]->(:Note) RETURN count(r) AS c ORDER BY c")
        .unwrap();
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert!(matches!(r.rows[0][0], Value::Int64(2)), "{:?}", r.rows);
}

#[test]
fn temoin_tri_sur_propriete_de_relation_en_portee_fonctionne() {
    let db = base();
    let r = db
        .session()
        .execute_cypher(
            "MATCH (:Project)-[r:HAS]->(n:Note) RETURN n.id AS id ORDER BY r.since DESC",
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2, "{:?}", r.rows);
    let ids: Vec<String> = r
        .rows
        .iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("texte attendu, reçu {autre:?}"),
        })
        .collect();
    assert_eq!(
        ids,
        vec!["n2".to_string(), "n1".to_string()],
        "since décroissant"
    );
}

#[test]
fn temoin_propriete_de_relation_projetee_puis_triee() {
    let db = base();
    let r = db
        .session()
        .execute_cypher(
            "MATCH (:Project)-[r:HAS]->(n:Note) RETURN n.id AS id, r.since AS s ORDER BY r.since",
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2, "{:?}", r.rows);
}
