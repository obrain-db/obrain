//! Prédicat de motif en position d'expression (obrain-db/obrain#195, famille 5).
//!
//! `WHERE NOT (n)-[:SYNAPSE]->()` était refusé en Cypher (« Expected expression »),
//! alors que GQL l'acceptait. Un motif avec au moins une relation est un test
//! d'existence : il est rendu comme `EXISTS { motif }`. Les expressions
//! parenthésées ordinaires doivent rester inchangées (témoins ci-dessous).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test motif_predicat_195
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// a -SYNAPSE-> b ; c isolé (aucune relation sortante).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:Note {id: 'a', w: 2})-[:SYNAPSE]->(b:Note {id: 'b', w: 5}), \
             (c:Note {id: 'c', w: 3})",
        )
        .unwrap();
    db
}

fn ids(db: &ObrainDB, langage: &str, requete: &str) -> Vec<String> {
    db.session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"))
        .rows
        .into_iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("texte attendu, reçu {autre:?}"),
        })
        .collect()
}

#[test]
fn not_motif_garde_les_noeuds_sans_relation() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            ids(
                &db,
                langage,
                "MATCH (n:Note) WHERE NOT (n)-[:SYNAPSE]->() RETURN n.id AS id ORDER BY id"
            ),
            vec!["b".to_string(), "c".to_string()],
            "[{langage}]"
        );
    }
}

#[test]
fn motif_positif_et_combine() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            ids(
                &db,
                langage,
                "MATCH (n:Note) WHERE (n)-[:SYNAPSE]->() RETURN n.id AS id ORDER BY id"
            ),
            vec!["a".to_string()],
            "[{langage}] positif"
        );
        assert_eq!(
            ids(
                &db,
                langage,
                "MATCH (n:Note) WHERE n.w > 2 AND NOT (n)-[:SYNAPSE]->() RETURN n.id AS id ORDER BY id"
            ),
            vec!["b".to_string(), "c".to_string()],
            "[{langage}] combiné"
        );
    }
}

/// Témoins : une expression parenthésée reste une expression.
#[test]
fn expressions_parenthesees_inchangees() {
    let db = base();
    for langage in LANGAGES {
        let r = db
            .session()
            .execute_language("RETURN (1 + 2) * 3 AS v", langage, None)
            .unwrap();
        assert_eq!(
            r.rows,
            vec![vec![Value::Int64(9)]],
            "[{langage}] arithmétique"
        );
        assert_eq!(
            ids(
                &db,
                langage,
                "MATCH (n:Note) WHERE (n.w + 1) * 2 > 9 RETURN n.id AS id ORDER BY id"
            ),
            vec!["b".to_string()],
            "[{langage}] parenthèses dans un prédicat"
        );
        assert_eq!(
            ids(
                &db,
                langage,
                "MATCH (n:Note) WHERE (n.w) > 4 RETURN n.id AS id ORDER BY id"
            ),
            vec!["b".to_string()],
            "[{langage}] parenthèses simples"
        );
    }
}
