//! #368 — une compréhension de motif liée dehors par DEUX variables ne doit
//! jamais rendre de ligne FAUSSE.
//!
//! Avant le correctif, la seconde variable liée dehors était traitée comme un
//! nouveau nœud libre : `MATCH (a:Note), (b:Note) WHERE size([(a)-[:LINKS]->(t:Tag)<-[:LINKS]-(b) | t.name]) > 0`
//! rendait `(n5, n1)`, `(n5, n2)`… alors que `n1` n'a aucun lien. Mesuré
//! IDENTIQUE sur `main` et avec le correctif de #355 : défaut préexistant.
//!
//! Contrat : un refus net (erreur de traduction nommée), jamais de lignes
//! fausses.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test comprehension_variables_liees_368
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Seule `n5` est reliée à un `Tag`.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (:Note {id: 'n1'}), (:Note {id: 'n2'}), (:Note {id: 'n3'}), (:Note {id: 'n4'}), \
                    (n5:Note {id: 'n5'})-[:LINKS]->(:Tag {name: 'x'})",
        )
        .unwrap();
    db
}

fn s(v: &str) -> Value {
    Value::from(v)
}

/// La requête de l'issue : a ET b sont liés dehors. Contrat choisi : REFUS NET
/// par une erreur de traduction qui nomme les deux variables (« jamais un
/// résultat faux »). Corréler sur toutes les variables liées reste possible
/// plus tard ; le refus est strictement meilleur que cinq lignes fausses.
#[test]
fn deux_variables_liees_dehors_sont_refusees_nettement() {
    let db = base();
    let err = db
        .execute_cypher(
            "MATCH (a:Note), (b:Note) \
             WHERE size([(a)-[:LINKS]->(t:Tag)<-[:LINKS]-(b) | t.name]) > 0 \
             RETURN a.id AS a, b.id AS b",
        )
        .expect_err("deux nœuds liés dehors doivent être refusés, pas rendre des lignes fausses")
        .to_string();
    assert!(
        err.contains("plusieurs nœuds") && err.contains('a') && err.contains('b'),
        "l'erreur doit nommer le cas et les deux variables : {err}"
    );
}

/// Le refus ne dépend pas de la place des nœuds liés dans le motif : premier
/// nœud lié ET dernier nœud lié, ou deux nœuds au milieu et à la fin.
#[test]
fn le_refus_vaut_quelle_que_soit_la_place_des_noeuds_lies() {
    let db = base();
    for requete in [
        "MATCH (a:Note), (b:Note) WHERE size([(b)<-[:LINKS]-(a)-[:LINKS]->(t:Tag) | t.name]) > 0 RETURN a.id",
        "MATCH (a:Note), (t:Tag) WHERE size([(a)-[:LINKS]->(t)<-[:LINKS]-(b:Note) | b.id]) > 0 RETURN a.id",
    ] {
        let err = db.execute_cypher(requete).expect_err(requete).to_string();
        assert!(err.contains("plusieurs nœuds"), "{requete}\n{err}");
    }
}

/// Témoin : `b` NON lié dehors est un nœud libre du motif, la forme correcte
/// d'aujourd'hui (#355 ne doit pas la casser).
#[test]
fn b_non_lie_dehors_reste_correct() {
    let db = base();
    let r = db
        .execute_cypher(
            "MATCH (a:Note) \
             WHERE size([(a)-[:LINKS]->(t:Tag)<-[:LINKS]-(b:Note) | t.name]) > 0 \
             RETURN a.id AS a",
        )
        .unwrap();
    assert_eq!(r.rows, vec![vec![s("n5")]]);
}

/// Témoin : une seule variable liée dehors (cas #355 ancrage non premier).
#[test]
fn une_variable_liee_dehors_reste_correcte() {
    let db = base();
    let r = db
        .execute_cypher(
            "MATCH (b:Note) \
             WHERE size([(a:Note)-[:LINKS]->(t:Tag)<-[:LINKS]-(b) | t.name]) > 0 \
             RETURN b.id AS b",
        )
        .unwrap();
    // `a` libre : n5 (a = n5, b = n5) est le seul candidat possible.
    for ligne in &r.rows {
        assert_eq!(ligne[0], s("n5"), "{:?}", r.rows);
    }
}
