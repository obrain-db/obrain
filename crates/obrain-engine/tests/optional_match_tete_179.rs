//! `OPTIONAL MATCH` en PREMIÈRE clause (obrain-db/obrain#179).
//!
//! Neo4j accepte la clause en tête : elle a une ligne d'entrée implicite, et sans
//! correspondance elle rend une ligne à NULL. obrain refusait « OPTIONAL MATCH
//! requires input ». Forme dont dépend Project Orchestrator (lectures qui doivent
//! rendre une ligne même sans correspondance).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test optional_match_tete_179
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:Project {slug: 'p1', id: 'x1'})")
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, langage: &str, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"))
        .rows
}

#[test]
fn sans_correspondance_rend_une_ligne_a_null() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "OPTIONAL MATCH (p:Project {slug: 'inexistant'}) RETURN p.id AS id"
            ),
            vec![vec![Value::Null]],
            "[{langage}]"
        );
    }
}

#[test]
fn avec_correspondance_rend_la_ligne() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "OPTIONAL MATCH (p:Project {slug: 'p1'}) RETURN p.id AS id"
            ),
            vec![vec![Value::String("x1".into())]],
            "[{langage}]"
        );
    }
}

#[test]
fn count_sans_correspondance_vaut_zero() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "OPTIONAL MATCH (p:Project {slug: 'inexistant'}) RETURN count(p) AS n"
            ),
            vec![vec![Value::Int64(0)]],
            "[{langage}]"
        );
    }
}

/// Une deuxième clause OPTIONAL MATCH chaîne sur la ligne de la première.
#[test]
fn deux_optional_match_en_tete() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "OPTIONAL MATCH (p:Project {slug: 'p1'}) \
                 OPTIONAL MATCH (q:Project {slug: 'absent'}) RETURN p.id AS p, q.id AS q"
            ),
            vec![vec![Value::String("x1".into()), Value::Null]],
            "[{langage}]"
        );
    }
}
