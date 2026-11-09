//! Features sur `wasm32-unknown-unknown` : AUCUNE PANIQUE.
//!
//! Sur cette cible les paniques sont `abort` : une seule tue toute l'instance du
//! navigateur. Ce fichier exerce donc deux choses :
//!
//! 1. des requêtes valides, avec leurs résultats (GQL, Cypher) ;
//! 2. des entrées hostiles (malformées, extrêmes) sur CHAQUE langage activé : le
//!    seul critère est « une erreur propre, jamais une panique ».
//!
//! Il est exécuté une fois par feature (et une fois avec toutes) :
//!
//! ```text
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test --target wasm32-unknown-unknown -p obrain --no-default-features \
//!   --features browser,<feature> --test navigateur_features
//! ```
#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;

use obrain::{ObrainDB, Value};
use wasm_bindgen_test::wasm_bindgen_test;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for (nom, age) in [("Alix", 30), ("Bao", 25), ("Chloé", 41)] {
        s.execute(&format!("INSERT (:Person {{name: '{nom}', age: {age}}})"))
            .unwrap();
    }
    s.execute("MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Bao'}) INSERT (a)-[:KNOWS]->(b)")
        .unwrap();
    db
}

fn entiers(rows: &[Vec<Value>]) -> Vec<i64> {
    let mut v: Vec<i64> = rows
        .iter()
        .filter_map(|l| match &l[0] {
            Value::Int64(n) => Some(*n),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

// ───────────────────────── GQL (profil browser) ─────────────────────────

#[wasm_bindgen_test]
fn gql_filtre_tri_limite() {
    let db = base();
    let s = db.session();
    let r = s
        .execute("MATCH (p:Person) WHERE p.age > 26 RETURN p.age ORDER BY p.age")
        .unwrap();
    assert_eq!(entiers(&r.rows), vec![30, 41]);
    let r = s
        .execute("MATCH (p:Person) RETURN p.age ORDER BY p.age DESC LIMIT 1")
        .unwrap();
    assert_eq!(entiers(&r.rows), vec![41]);
}

#[wasm_bindgen_test]
fn gql_agregats() {
    let db = base();
    let s = db.session();
    let r = s.execute("MATCH (p:Person) RETURN count(p)").unwrap();
    assert_eq!(entiers(&r.rows), vec![3]);
    let r = s.execute("MATCH (p:Person) RETURN sum(p.age)").unwrap();
    assert_eq!(entiers(&r.rows), vec![96]);
    let r = s.execute("MATCH (p:Person) RETURN max(p.age)").unwrap();
    assert_eq!(entiers(&r.rows), vec![41]);
}

#[wasm_bindgen_test]
fn gql_parametres() {
    let db = base();
    let s = db.session();
    let mut p = HashMap::new();
    p.insert("n".to_string(), Value::from("Bao"));
    let r = s
        .execute_with_params("MATCH (x:Person) WHERE x.name = $n RETURN x.age", p)
        .unwrap();
    assert_eq!(entiers(&r.rows), vec![25]);
}

#[wasm_bindgen_test]
fn gql_modifier_puis_supprimer() {
    let db = base();
    let s = db.session();
    s.execute("MATCH (p:Person {name: 'Bao'}) SET p.age = 26")
        .unwrap();
    let r = s
        .execute("MATCH (p:Person {name: 'Bao'}) RETURN p.age")
        .unwrap();
    assert_eq!(entiers(&r.rows), vec![26]);
    s.execute("MATCH (p:Person {name: 'Chloé'}) DETACH DELETE p")
        .unwrap();
    assert_eq!(db.node_count(), 2);
}

#[wasm_bindgen_test]
fn gql_transactions_commit_et_rollback() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.begin_transaction().unwrap();
    s.execute("INSERT (:T {v: 1})").unwrap();
    s.rollback().unwrap();
    assert_eq!(db.node_count(), 0, "le rollback annule");
    s.begin_transaction().unwrap();
    s.execute("INSERT (:T {v: 2})").unwrap();
    s.commit().unwrap();
    assert_eq!(db.node_count(), 1, "le commit garde");
}

#[wasm_bindgen_test]
fn gql_volume_et_traversee() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..1500 {
        s.execute(&format!("INSERT (:N {{i: {i}}})")).unwrap();
    }
    assert_eq!(db.node_count(), 1500);
    s.execute("MATCH (a:N {i: 1}), (b:N {i: 2}) INSERT (a)-[:R]->(b)")
        .unwrap();
    s.execute("MATCH (a:N {i: 2}), (b:N {i: 3}) INSERT (a)-[:R]->(b)")
        .unwrap();
    let r = s
        .execute("MATCH (a:N {i: 1})-[:R]->()-[:R]->(c) RETURN c.i")
        .unwrap();
    assert_eq!(entiers(&r.rows), vec![3]);
}

#[wasm_bindgen_test]
fn gql_unicode_et_chaines_longues() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:U {t: 'é ü 日本 😀 e\u{0301}'})")
        .unwrap();
    let r = s.execute("MATCH (u:U) RETURN u.t").unwrap();
    assert_eq!(r.rows.len(), 1);
    let long = "a".repeat(200_000);
    s.execute(&format!("INSERT (:U {{t: '{long}'}})")).unwrap();
    assert_eq!(db.node_count(), 2);
}

#[wasm_bindgen_test]
fn index_de_propriete_checkpoint_et_fermeture() {
    let db = base();
    db.create_property_index("name");
    let r = db
        .session()
        .execute("MATCH (p:Person {name: 'Alix'}) RETURN p.age")
        .unwrap();
    assert_eq!(entiers(&r.rows), vec![30]);
    // Ni l'un ni l'autre ne doit paniquer sur une base mémoire (Ok ou Err).
    let _ = db.checkpoint();
    let _ = db.close();
}

#[wasm_bindgen_test]
fn session_reutilisee_apres_erreur() {
    let db = base();
    let s = db.session();
    assert!(s.execute("MATCH (").is_err());
    let r = s.execute("MATCH (p:Person) RETURN count(p)").unwrap();
    assert_eq!(entiers(&r.rows), vec![3], "la session survit à une erreur");
}

// ─────────────────────── entrées hostiles, tous langages ───────────────────────

fn hostiles() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        " ",
        "\n\n",
        ";",
        "(",
        ")))",
        "{",
        "}}}",
        "[",
        "'",
        "\"",
        "`",
        "\0",
        "MATCH",
        "MATCH (",
        "MATCH (n",
        "MATCH (n)",
        "MATCH (n) RETURN",
        "MATCH (n) RETURN n.",
        "MATCH (n) RETURN n LIMIT -1",
        "MATCH (n) RETURN n SKIP 99999999999999999999",
        "MATCH (n) RETURN n.x.y.z",
        "MATCH (n) WHERE n.age > 'x' RETURN n",
        "MATCH (n)-[*]->(m) RETURN n",
        "MATCH (n)-[*1..100]->(m) RETURN count(n)",
        "MATCH (n) WHERE n.age / 0 > 1 RETURN n",
        "RETURN 1/0",
        "RETURN 1%0",
        "RETURN 9223372036854775807 + 1",
        "RETURN -9223372036854775808 * -1",
        "RETURN 1e999",
        "RETURN 0.0/0.0",
        "RETURN '\u{1F600}\u{0301}'",
        "INSERT",
        "INSERT (:",
        "INSERT (:A {",
        "INSERT (:A {x:})",
        "INSERT (:A {x: 1})-[:R]->",
        "MATCH (a) DELETE a DELETE a",
        "MATCH (a), (b) INSERT (a)-[:R]->(b)",
        "CALL",
        "CALL obrain.nope()",
        "g.V(",
        "g.V().has(",
        "{ person }",
        "{ person( }",
        "SELECT",
        "SELECT * FROM",
        "SELECT ?s WHERE { ?s ?p }",
        "INSERT DATA { <a> <b> }",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push(format!("RETURN '{}'", "a".repeat(100_000)));
    v.push(format!("MATCH {} RETURN 1", "(".repeat(5_000)));
    v.push(format!(
        "RETURN {}1{}",
        "(".repeat(2_000),
        ")".repeat(2_000)
    ));
    v
}

#[wasm_bindgen_test]
fn gql_hostile_sans_panique() {
    let db = base();
    let s = db.session();
    for q in hostiles() {
        let _ = s.execute(&q);
    }
    // et la base reste utilisable
    assert!(db.node_count() >= 3);
}

#[cfg(feature = "cypher")]
#[wasm_bindgen_test]
fn cypher_valide_et_hostile() {
    let db = ObrainDB::new_in_memory();
    db.execute_cypher("CREATE (:P {name: 'a', n: 1})").unwrap();
    let r = db.execute_cypher("MATCH (p:P) RETURN p.n").unwrap();
    assert_eq!(entiers(&r.rows), vec![1]);
    for q in hostiles() {
        let _ = db.execute_cypher(&q);
    }
}

#[cfg(feature = "sparql")]
#[wasm_bindgen_test]
fn sparql_sans_panique() {
    let db = ObrainDB::new_in_memory();
    let _ = db.execute_sparql("INSERT DATA { <http://a> <http://b> <http://c> }");
    let _ = db.execute_sparql("SELECT ?s ?p ?o WHERE { ?s ?p ?o }");
    for q in hostiles() {
        let _ = db.execute_sparql(&q);
    }
}

#[cfg(feature = "gremlin")]
#[wasm_bindgen_test]
fn gremlin_sans_panique() {
    let db = ObrainDB::new_in_memory();
    let _ = db.execute_gremlin("g.addV('p').property('name','a')");
    let _ = db.execute_gremlin("g.V().count()");
    for q in hostiles() {
        let _ = db.execute_gremlin(&q);
    }
}

#[cfg(feature = "graphql")]
#[wasm_bindgen_test]
fn graphql_sans_panique() {
    let db = ObrainDB::new_in_memory();
    let _ = db.execute_graphql("{ person { name } }");
    for q in hostiles() {
        let _ = db.execute_graphql(&q);
    }
}

#[cfg(feature = "sql-pgq")]
#[wasm_bindgen_test]
fn sql_pgq_sans_panique() {
    let db = ObrainDB::new_in_memory();
    let _ = db.execute_sql("SELECT 1");
    for q in hostiles() {
        let _ = db.execute_sql(&q);
    }
}

/// L'imbrication profonde épuisait la pile (1 Mio sur wasm32) : l'instance entière
/// tombait, sans message. Elle doit maintenant être refusée par une ERREUR, quelle
/// que soit la profondeur, et une imbrication raisonnable doit rester acceptée.
#[wasm_bindgen_test]
fn imbrication_profonde_refusee_sans_tomber() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for n in [65usize, 100, 400, 5_000, 50_000] {
        let ret = format!("RETURN {}1{}", "(".repeat(n), ")".repeat(n));
        let e = s.execute(&ret).expect_err("trop imbriquée");
        assert!(e.to_string().contains("trop imbriqu"), "n={n} : {e}");
        let pat = format!("MATCH {} RETURN 1", "(".repeat(n));
        assert!(s.execute(&pat).is_err(), "n={n}");
    }
    // contournement du comptage par un commentaire à apostrophe
    for n in [100usize, 400, 5_000] {
        let q = format!(
            "-- l'utilisateur\nRETURN {}1{}",
            "(".repeat(n),
            ")".repeat(n)
        );
        assert!(s.execute(&q).is_err(), "n={n}");
        let q = format!("-- l'utilisateur\nMATCH {} RETURN 1", "(".repeat(n));
        assert!(s.execute(&q).is_err(), "n={n}");
    }
}

#[wasm_bindgen_test]
fn imbrication_raisonnable_acceptee() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for n in [1usize, 10, 30, 60] {
        let q = format!("RETURN {}1{}", "(".repeat(n), ")".repeat(n));
        let r = s
            .execute(&q)
            .unwrap_or_else(|e| panic!("n={n} doit passer : {e}"));
        assert_eq!(entiers(&r.rows), vec![1], "n={n}");
    }
}

// ─────────────────────────── recherche et algorithmes ───────────────────────────

#[cfg(feature = "algos")]
#[wasm_bindgen_test]
fn algorithmes_sans_panique() {
    let db = base();
    let s = db.session();
    let r = s.execute("CALL obrain.pagerank()");
    assert!(r.is_ok(), "pagerank sur une base mémoire : {:?}", r.err());
    for q in [
        "CALL obrain.pagerank({damping: 0.85, max_iterations: 10})",
        "CALL obrain.connected_components()",
        "CALL obrain.nope()",
        "CALL obrain.pagerank({damping: -1})",
        "CALL obrain.pagerank({max_iterations: 0})",
    ] {
        let _ = s.execute(q);
    }
    // base vide
    let vide = ObrainDB::new_in_memory();
    let _ = vide.session().execute("CALL obrain.pagerank()");
}

#[cfg(feature = "text-index")]
#[wasm_bindgen_test]
fn index_texte_trouve_le_bon_document() {
    let db = ObrainDB::new_in_memory();
    let renard = db.create_node(&["Doc"]);
    db.set_node_property(renard, "body", Value::from("le renard brun saute"));
    let chien = db.create_node(&["Doc"]);
    db.set_node_property(chien, "body", Value::from("un chien dort"));
    db.create_text_index("Doc", "body").expect("index texte");
    let r = db
        .text_search("Doc", "body", "renard", 5)
        .expect("recherche");
    assert_eq!(r.len(), 1, "un seul document contient « renard » : {r:?}");
    assert_eq!(r[0].0, renard);
    assert!(r[0].1 > 0.0, "score positif");
    // entrées dégénérées : une erreur ou un résultat, jamais une panique
    let _ = db.text_search("Doc", "body", "", 5);
    let _ = db.text_search("Doc", "body", "x", 0);
    let _ = db.text_search("Nope", "nope", "x", 5);
}

#[cfg(feature = "vector-index")]
#[wasm_bindgen_test]
fn index_vectoriel_trouve_le_plus_proche() {
    let db = ObrainDB::new_in_memory();
    let vecteurs: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut ids = Vec::new();
    for v in vecteurs {
        let n = db.create_node(&["V"]);
        db.set_node_property(n, "emb", Value::Vector(v.to_vec().into()));
        ids.push(n);
    }
    db.create_vector_index("V", "emb", Some(3), Some("cosine"), None, None)
        .expect("index vectoriel");
    let r = db
        .vector_search("V", "emb", &[0.0, 0.9, 0.1], 1, None, None)
        .expect("recherche");
    assert_eq!(r.len(), 1);
    assert_eq!(
        r[0].0, ids[1],
        "le plus proche de (0, 0.9, 0.1) est (0, 1, 0)"
    );
    // entrées dégénérées
    let _ = db.create_vector_index("V", "emb", Some(0), Some("nope"), None, None);
    let _ = db.vector_search("V", "emb", &[], 5, None, None);
    let _ = db.vector_search("Nope", "nope", &[1.0], 0, None, None);
}

#[cfg(feature = "hybrid-search")]
#[wasm_bindgen_test]
fn recherche_hybride_trouve_le_bon_document() {
    let db = ObrainDB::new_in_memory();
    let renard = db.create_node(&["D"]);
    db.set_node_property(renard, "body", Value::from("le renard brun saute"));
    db.set_node_property(renard, "emb", Value::Vector(vec![1.0, 0.0, 0.0].into()));
    let chien = db.create_node(&["D"]);
    db.set_node_property(chien, "body", Value::from("un chien dort"));
    db.set_node_property(chien, "emb", Value::Vector(vec![0.0, 1.0, 0.0].into()));
    db.create_text_index("D", "body").expect("index texte");
    db.create_vector_index("D", "emb", Some(3), Some("cosine"), None, None)
        .expect("index vectoriel");
    let r = db
        .hybrid_search(
            "D",
            "body",
            "emb",
            "renard",
            Some(&[0.9, 0.1, 0.0]),
            2,
            None,
        )
        .expect("recherche hybride");
    assert!(!r.is_empty());
    assert_eq!(
        r[0].0, renard,
        "texte et vecteur désignent le même document"
    );
    let _ = db.hybrid_search("D", "body", "emb", "", None, 0, None);
}

// ───────────────────────── métriques, CDC, couche cognitive ─────────────────────────

#[cfg(feature = "metrics")]
#[wasm_bindgen_test]
fn metriques_comptent_les_requetes() {
    let db = base();
    let s = db.session();
    for _ in 0..5 {
        s.execute("MATCH (p:Person) RETURN count(p)").unwrap();
    }
    let _ = s.execute("MATCH (");
    let m = db.metrics();
    assert!(
        m.query_count >= 5,
        "au moins 5 requêtes comptées : {}",
        m.query_count
    );
    assert!(!db.metrics_prometheus().is_empty() || m.query_count > 0);
    db.reset_metrics();
}

#[cfg(feature = "cdc")]
#[wasm_bindgen_test]
fn cdc_historique_sans_panique() {
    let db = ObrainDB::new_in_memory();
    let n = db.create_node(&["C"]);
    db.set_node_property(n, "v", Value::Int64(1));
    db.set_node_property(n, "v", Value::Int64(2));
    // Ok (vide ou non selon le mode) : jamais une panique.
    let h = db.history(n);
    assert!(h.is_ok(), "{:?}", h.err());
    let _ = db.history_since(n, obrain_common::types::EpochId(0));
    let _ = db.changes_between(
        obrain_common::types::EpochId(0),
        obrain_common::types::EpochId(u64::MAX),
    );
}

#[cfg(feature = "cognitive")]
#[wasm_bindgen_test]
fn procedures_cognitives_sans_panique() {
    let db = base();
    let s = db.session();
    let r = s.execute("CALL obrain.cognitive.metrics()");
    assert!(r.is_ok(), "metrics cognitives : {:?}", r.err());
    let _ = s.execute(
        "CALL obrain.cognitive.bootstrap({edge_types: ['KNOWS']}) YIELD count RETURN count",
    );
    let _ = s.execute("CALL obrain.cognitive.reinforce(1, 2)");
    let _ =
        s.execute("CALL obrain.cognitive.spread([1], {hops: 2}) YIELD activated RETURN activated");
    // arguments dégénérés
    for q in [
        "CALL obrain.cognitive.reinforce(0, 0)",
        "CALL obrain.cognitive.reinforce(999999999, -1)",
        "CALL obrain.cognitive.spread([], {hops: 0})",
        "CALL obrain.cognitive.spread([1], {hops: 99999})",
        "CALL obrain.cognitive.nope()",
        "CALL obrain.cognitive.bootstrap({})",
    ] {
        let _ = s.execute(q);
    }
    // et la base reste utilisable
    assert_eq!(db.node_count(), 3);
}
