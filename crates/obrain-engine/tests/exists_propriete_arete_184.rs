//! Propriété d'une RELATION après `WHERE EXISTS { … }` (obrain-db/obrain#184).
//!
//! Avant correctif, toute expression sur `r.c` (`r.c * 2`, `coalesce(r.c, 9)`)
//! rendait NULL dès qu'une clause portait un EXISTS, alors que `r.c` brut et
//! `count(r.c)` restaient justes. Cause : le semi-join copiait l'identifiant
//! d'arête comme un `Int64` dans une colonne générique, et la lecture de propriété
//! le prenait pour un nœud.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test exists_propriete_arete_184
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// a -R{c:0.85}-> b (b a une sortie S), a -R{c:0.5}-> c (c a une sortie S),
/// a -R{c:0.2}-> d (d n'a pas de sortie S).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:X {k: 'a'})-[:R {c: 0.85}]->(b:X {k: 'b'}), \
             (a)-[:R {c: 0.5}]->(c:X {k: 'c'}), (a)-[:R {c: 0.2}]->(d:X {k: 'd'}), \
             (b)-[:S]->(:Y), (c)-[:S]->(:Y)",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, langage: &str, requete: &str) -> Vec<Vec<Value>> {
    let mut r = db
        .session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"))
        .rows;
    r.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    r
}

fn f(x: f64) -> Vec<Value> {
    vec![Value::Float64(x)]
}

const SOUS_EXISTS: &str = "MATCH (a:X {k: 'a'})-[r:R]->(x:X) WHERE EXISTS { MATCH (x)-[:S]->(:Y) }";

#[test]
fn expression_sur_propriete_de_relation_apres_exists() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(&db, langage, &format!("{SOUS_EXISTS} RETURN r.c * 2 AS v")),
            vec![f(1.0), f(1.7)],
            "[{langage}] r.c * 2"
        );
        assert_eq!(
            lignes(
                &db,
                langage,
                &format!("{SOUS_EXISTS} RETURN coalesce(r.c, 9.0) AS v")
            ),
            vec![f(0.5), f(0.85)],
            "[{langage}] coalesce"
        );
        assert_eq!(
            lignes(
                &db,
                langage,
                &format!("{SOUS_EXISTS} RETURN r.c > 0.6 AS v")
            ),
            vec![vec![Value::Bool(false)], vec![Value::Bool(true)]],
            "[{langage}] comparaison"
        );
    }
}

#[test]
fn le_brut_et_l_agregat_restent_justes() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(&db, langage, &format!("{SOUS_EXISTS} RETURN r.c AS v")),
            vec![f(0.5), f(0.85)],
            "[{langage}] r.c brut"
        );
        assert_eq!(
            lignes(
                &db,
                langage,
                &format!("{SOUS_EXISTS} RETURN count(r.c) AS n")
            ),
            vec![vec![Value::Int64(2)]],
            "[{langage}] count"
        );
    }
}

/// Le négatif : seule l'arête vers d (sans S) doit sortir, avec sa propre valeur.
#[test]
fn not_exists_garde_la_valeur_de_l_arete() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "MATCH (a:X {k: 'a'})-[r:R]->(x:X) WHERE NOT EXISTS { MATCH (x)-[:S]->(:Y) } \
                 RETURN r.c * 2 AS v"
            ),
            vec![f(0.4)],
            "[{langage}] NOT EXISTS"
        );
    }
}

/// Jointure ordinaire à deux MATCH : la même propriété d'arête.
#[test]
fn jointure_de_deux_match_garde_la_propriete_d_arete() {
    let db = base();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "MATCH (a:X {k: 'a'})-[r:R]->(x:X) MATCH (x)-[:S]->(y:Y) RETURN r.c * 2 AS v"
            ),
            vec![f(1.0), f(1.7)],
            "[{langage}] jointure"
        );
    }
}
