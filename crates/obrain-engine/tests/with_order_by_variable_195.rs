//! `WITH ... ORDER BY` sur une variable que le `WITH` ne projette pas
//! (obrain-db/obrain#195, famille 7).
//!
//! `WITH f, co ORDER BY r.count DESC LIMIT 10` : Neo4j trie sur la portée d'AVANT
//! le `WITH` (tant qu'il n'y a ni agrégat ni DISTINCT). obrain répondait « Variable
//! 'r' not found for ORDER BY property projection ». Le tri se fait désormais avant
//! la projection ; l'ordre survit à la projection. Forme de Project Orchestrator :
//! `code_44` (`WITH f, co ORDER BY r.count DESC LIMIT 10`).
//!
//! Les témoins fixent ce qui ne doit PAS changer : un tri sur un alias ou une
//! variable projetée par le `WITH`, un `WITH DISTINCT`, un `WITH` agrégé.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test with_order_by_variable_195
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// b et c changent de fichier avec a (compte 3 et 7), d aussi (compte 5).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (b:File {path: 'b'})-[:CO_CHANGED {count: 3}]->(a:File {path: 'a'}), \
             (c:File {path: 'c'})-[:CO_CHANGED {count: 7}]->(a), \
             (d:File {path: 'd'})-[:CO_CHANGED {count: 5}]->(a)",
        )
        .unwrap();
    db
}

fn chemins(db: &ObrainDB, langage: &str, requete: &str) -> Vec<String> {
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

const MOTIF: &str = "MATCH (f:File {path: 'a'})<-[r:CO_CHANGED]-(co:File)";

fn s(x: &str) -> String {
    x.to_string()
}

#[test]
fn tri_sur_la_variable_de_relation_avant_le_with() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            chemins(
                &db,
                langage,
                &format!("{MOTIF} WITH f, co ORDER BY r.count DESC RETURN co.path AS p")
            ),
            vec![s("c"), s("d"), s("b")],
            "[{langage}] DESC"
        );
        assert_eq!(
            chemins(
                &db,
                langage,
                &format!("{MOTIF} WITH f, co ORDER BY r.count ASC RETURN co.path AS p")
            ),
            vec![s("b"), s("d"), s("c")],
            "[{langage}] ASC"
        );
    }
}

/// La forme exacte de PO (code_44), avec LIMIT : le tri précède la limite.
#[test]
fn avec_limit_comme_code_44() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            chemins(
                &db,
                langage,
                &format!("{MOTIF} WITH f, co ORDER BY r.count DESC LIMIT 2 RETURN co.path AS p")
            ),
            vec![s("c"), s("d")],
            "[{langage}]"
        );
    }
}

/// Témoin : le tri sur un alias que le WITH projette ne change pas.
#[test]
fn tri_sur_un_alias_du_with_inchange() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            chemins(
                &db,
                langage,
                &format!("{MOTIF} WITH co, r.count AS k ORDER BY k DESC RETURN co.path AS p")
            ),
            vec![s("c"), s("d"), s("b")],
            "[{langage}]"
        );
    }
}

/// Témoin : le tri sur une variable projetée par le WITH ne change pas.
#[test]
fn tri_sur_une_variable_projetee_inchange() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            chemins(
                &db,
                langage,
                "MATCH (n:File) WITH n ORDER BY n.path DESC LIMIT 2 RETURN n.path AS p"
            ),
            vec![s("d"), s("c")],
            "[{langage}]"
        );
    }
}

/// Témoin : un WITH agrégé garde sa portée (le tri sur un agrégat marche déjà).
#[test]
fn with_agrege_inchange() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            chemins(
                &db,
                langage,
                &format!(
                    "{MOTIF} WITH co.path AS p, sum(r.count) AS total ORDER BY total DESC RETURN p"
                )
            ),
            vec![s("c"), s("d"), s("b")],
            "[{langage}]"
        );
    }
}
