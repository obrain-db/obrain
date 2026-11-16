//! Alias d'une colonne nommé comme un mot-clé (obrain-db/obrain#195, famille 4).
//!
//! `RETURN s.id AS order` était refusé (« Expected identifier » en Cypher,
//! « Expected alias name » en GQL). Après `AS`, la position est sans ambiguïté :
//! tout mot est un alias, comme dans Neo4j. Forme de Project Orchestrator :
//! `RETURN t.path AS task_id, inc.order AS order` (dayplan_01).
//!
//! Non couvert ici, et volontairement : un mot-clé employé comme VARIABLE dans
//! une expression (`ORDER BY order`) reste refusé. Ce n'est pas la même position.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test alias_mot_cle_195
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:Step {id: 's1', order: 2}), (:Step {id: 's2', order: 1})")
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, langage: &str, requete: &str) -> (Vec<String>, Vec<Vec<Value>>) {
    let r = db
        .session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"));
    (r.columns, r.rows)
}

#[test]
fn alias_order_devient_une_colonne() {
    let db = base();
    for langage in LANGAGES {
        let (colonnes, rows) = lignes(&db, langage, "MATCH (s:Step) RETURN s.id AS order");
        assert_eq!(colonnes, vec!["order".to_string()], "[{langage}]");
        assert_eq!(rows.len(), 2, "[{langage}]");
    }
}

#[test]
fn alias_mot_cle_avec_une_seconde_colonne() {
    let db = base();
    for langage in LANGAGES {
        let (colonnes, rows) = lignes(
            &db,
            langage,
            "MATCH (s:Step) RETURN s.id AS id, s.order AS order LIMIT 2",
        );
        assert_eq!(
            colonnes,
            vec!["id".to_string(), "order".to_string()],
            "[{langage}]"
        );
        assert!(
            rows.contains(&vec![Value::String("s1".into()), Value::Int64(2)]),
            "[{langage}] {rows:?}"
        );
    }
}

#[test]
fn alias_limit_et_match() {
    let db = base();
    for langage in LANGAGES {
        let (colonnes, _) = lignes(
            &db,
            langage,
            "MATCH (s:Step) RETURN s.id AS limit, s.order AS match",
        );
        assert_eq!(
            colonnes,
            vec!["limit".to_string(), "match".to_string()],
            "[{langage}]"
        );
    }
}

/// Témoin : un alias ordinaire ne change pas.
#[test]
fn alias_ordinaire_inchange() {
    let db = base();
    for langage in LANGAGES {
        let (colonnes, _) = lignes(&db, langage, "MATCH (s:Step) RETURN s.id AS identifiant");
        assert_eq!(colonnes, vec!["identifiant".to_string()], "[{langage}]");
    }
}
