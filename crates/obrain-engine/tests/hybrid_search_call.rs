//! `CALL obrain.hybrid_search(...)` : la recherche hybride (BM25 + vecteur,
//! fusion RRF) exposée comme procédure GQL.
//!
//! Forme :
//!   CALL obrain.hybrid_search(label, text_property, vector_property, query_text
//!                             [, query_vector] [, {k, fusion, rrf_k, weights}])
//!   YIELD node_id, score
//!
//! Les scores attendus sont ceux de `fuse_results` (RRF, k = 60, rang à base 1) :
//! un nœud premier des deux sources vaut 2/61, deuxième des deux 2/62, présent
//! dans une seule source au rang 3 : 1/63.

// `algos` porte le dispatch `CALL` (plan_call_procedure) ; sans lui aucune
// procédure n'existe, et le rouge ne dirait rien de la recherche hybride.
#![cfg(all(feature = "hybrid-search", feature = "algos"))]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const EPS: f64 = 1e-9;

fn vec3(x: f32, y: f32, z: f32) -> Value {
    Value::Vector(vec![x, y, z].into())
}

/// Trois documents : n1 « Rust graph database » [1,0,0], n2 « Python machine
/// learning » [0,1,0], n3 « Rust systems programming » [0.9,0.1,0].
fn trois_docs() -> (ObrainDB, u64, u64, u64) {
    let db = ObrainDB::new_in_memory();
    let n1 = db.create_node(&["Doc"]);
    db.set_node_property(n1, "title", Value::String("Rust graph database".into()));
    db.set_node_property(n1, "emb", vec3(1.0, 0.0, 0.0));
    let n2 = db.create_node(&["Doc"]);
    db.set_node_property(
        n2,
        "title",
        Value::String("Python machine learning".into()),
    );
    db.set_node_property(n2, "emb", vec3(0.0, 1.0, 0.0));
    let n3 = db.create_node(&["Doc"]);
    db.set_node_property(
        n3,
        "title",
        Value::String("Rust systems programming".into()),
    );
    db.set_node_property(n3, "emb", vec3(0.9, 0.1, 0.0));
    (db, n1.0, n2.0, n3.0)
}

fn trois_docs_indexes() -> (ObrainDB, u64, u64, u64) {
    let (db, n1, n2, n3) = trois_docs();
    db.create_text_index("Doc", "title").expect("index texte");
    db.create_vector_index("Doc", "emb", Some(3), Some("cosine"), None, None)
        .expect("index vectoriel");
    (db, n1, n2, n3)
}

fn lignes_id_score(result: &obrain_engine::database::QueryResult) -> Vec<(u64, f64)> {
    result
        .rows
        .iter()
        .map(|row| {
            let id = match &row[0] {
                Value::Int64(i) => u64::try_from(*i).expect("node_id positif"),
                autre => panic!("node_id doit être Int64, reçu {autre:?}"),
            };
            let score = match &row[1] {
                Value::Float64(f) => *f,
                autre => panic!("score doit être Float64, reçu {autre:?}"),
            };
            (id, score)
        })
        .collect()
}

fn proche(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

// (a) texte + vecteur : ordre et scores RRF exacts.
#[test]
fn call_hybrid_search_texte_et_vecteur_ordre_et_scores() {
    let (db, n1, n2, n3) = trois_docs_indexes();
    let s = db.session();
    let r = s
        .execute(
            "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3}) \
             YIELD node_id, score",
        )
        .expect("CALL obrain.hybrid_search");
    assert_eq!(r.columns, vec!["node_id".to_string(), "score".to_string()]);
    let lignes = lignes_id_score(&r);
    let ids: Vec<u64> = lignes.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        ids,
        vec![n1, n3, n2],
        "n1 (Rust graph, [1,0,0]) > n3 (Rust, [0.9,0.1,0]) > n2 (vecteur seul) ; reçu {lignes:?}"
    );
    assert!(proche(lignes[0].1, 2.0 / 61.0), "n1 premier des deux sources : {lignes:?}");
    assert!(proche(lignes[1].1, 2.0 / 62.0), "n3 deuxième des deux sources : {lignes:?}");
    assert!(proche(lignes[2].1, 1.0 / 63.0), "n2 troisième, vecteur seul : {lignes:?}");
}

// (a') le vecteur peut venir d'un paramètre `$vec`.
#[test]
fn call_hybrid_search_vecteur_en_parametre() {
    let (db, n1, _n2, n3) = trois_docs_indexes();
    let s = db.session();
    let params = std::collections::HashMap::from([(
        "vec".to_string(),
        Value::List(vec![Value::Float64(1.0), Value::Float64(0.0), Value::Float64(0.0)].into()),
    )]);
    let r = s
        .execute_with_params(
            "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', $vec, {k: 2}) \
             YIELD node_id, score",
            params,
        )
        .expect("CALL avec $vec");
    let lignes = lignes_id_score(&r);
    let ids: Vec<u64> = lignes.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![n1, n3], "k = 2 : les deux premiers ; reçu {lignes:?}");
    assert!(proche(lignes[0].1, 2.0 / 61.0));
}

// (b) texte seul : sans vecteur de requête, seul BM25 contribue.
#[test]
fn call_hybrid_search_texte_seul() {
    let (db, n1, n2, n3) = trois_docs_indexes();
    let s = db.session();
    let r = s
        .execute("CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph') YIELD node_id, score")
        .expect("CALL texte seul");
    let lignes = lignes_id_score(&r);
    let ids: Vec<u64> = lignes.iter().map(|(id, _)| *id).collect();
    // BM25 est en OU : n1 porte les deux termes, n3 un seul, n2 aucun.
    assert_eq!(ids, vec![n1, n3], "ordre BM25 ; reçu {lignes:?}");
    assert!(!ids.contains(&n2));
    assert!(lignes[0].1 > lignes[1].1, "score BM25 décroissant : {lignes:?}");
    assert!(lignes[1].1 > 0.0);
}

// (c) sans aucun index : erreur qui nomme les index attendus.
#[test]
fn call_hybrid_search_sans_index_nomme_les_index_attendus() {
    let (db, _, _, _) = trois_docs();
    let s = db.session();
    let err = s
        .execute("CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph') YIELD node_id, score")
        .expect_err("sans index, la procédure doit refuser");
    let msg = err.to_string();
    assert!(
        !msg.contains("Unknown procedure"),
        "la procédure doit exister : {msg}"
    );
    assert!(
        msg.contains(":Doc(title)") && msg.contains(":Doc(emb)"),
        "le message doit nommer l'index texte :Doc(title) et l'index vectoriel :Doc(emb) : {msg}"
    );
    assert!(
        msg.contains("create_text_index") && msg.contains("create_vector_index"),
        "le message doit dire comment créer les index : {msg}"
    );
}

// (d) enchaînement : CALL … YIELD node_id MATCH (n) WHERE id(n) = node_id RETURN n.title
#[test]
fn call_hybrid_search_enchaine_avec_match() {
    let (db, _n1, _n2, _n3) = trois_docs_indexes();
    let s = db.session();
    let r = s
        .execute(
            "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 2}) \
             YIELD node_id \
             MATCH (n) WHERE id(n) = node_id \
             RETURN n.title",
        )
        .expect("CALL … YIELD … MATCH … RETURN");
    let titres: Vec<String> = r
        .rows
        .iter()
        .map(|row| match &row[0] {
            Value::String(t) => t.to_string(),
            autre => panic!("n.title doit être une chaîne : {autre:?}"),
        })
        .collect();
    assert_eq!(
        titres,
        vec![
            "Rust graph database".to_string(),
            "Rust systems programming".to_string()
        ],
        "les deux premiers du classement hybride, dans l'ordre"
    );
}

// (d') l'enchaînement garde le score : CALL … YIELD node_id, score MATCH … RETURN n.title, score
#[test]
fn call_hybrid_search_enchaine_avec_match_garde_le_score() {
    let (db, _, _, _) = trois_docs_indexes();
    let s = db.session();
    let r = s
        .execute(
            "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 1}) \
             YIELD node_id, score \
             MATCH (n:Doc) WHERE id(n) = node_id \
             RETURN n.title AS title, score",
        )
        .expect("CALL … YIELD node_id, score MATCH … RETURN");
    assert_eq!(r.columns, vec!["title".to_string(), "score".to_string()]);
    assert_eq!(r.row_count(), 1);
    assert_eq!(r.rows[0][0], Value::String("Rust graph database".into()));
    match &r.rows[0][1] {
        Value::Float64(f) => assert!(proche(*f, 2.0 / 61.0), "score transporté : {f}"),
        autre => panic!("score doit être Float64 : {autre:?}"),
    }
}

// (e) obrain.procedures() liste hybrid_search avec ses colonnes.
#[test]
fn obrain_procedures_liste_hybrid_search() {
    let (db, _, _, _) = trois_docs_indexes();
    let s = db.session();
    let r = s.execute("CALL obrain.procedures()").expect("procedures()");
    let ligne = r
        .rows
        .iter()
        .find(|row| row[0] == Value::String("obrain.hybrid_search".into()))
        .unwrap_or_else(|| panic!("obrain.hybrid_search absente de obrain.procedures() : {:?}", r.rows));
    assert_eq!(
        ligne[3],
        Value::String("node_id, score".into()),
        "colonnes de sortie"
    );
    match &ligne[2] {
        Value::String(p) => {
            for attendu in ["label", "text_property", "vector_property", "query_text", "query_vector"] {
                assert!(p.contains(attendu), "paramètre {attendu} absent de : {p}");
            }
        }
        autre => panic!("parameters doit être une chaîne : {autre:?}"),
    }
}

// Fusion pondérée : {fusion: 'weighted', weights: [0.5, 0.5]} est acceptée.
#[test]
fn call_hybrid_search_fusion_ponderee() {
    let (db, n1, _n2, _n3) = trois_docs_indexes();
    let s = db.session();
    let r = s
        .execute(
            "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], \
             {k: 3, fusion: 'weighted', weights: [0.5, 0.5]}) YIELD node_id, score",
        )
        .expect("fusion pondérée");
    let lignes = lignes_id_score(&r);
    assert_eq!(lignes[0].0, n1, "n1 premier des deux sources : {lignes:?}");
    assert!(proche(lignes[0].1, 1.0), "min-max normalisé : 0.5·1 + 0.5·1 = 1 ; reçu {lignes:?}");
}

// Argument mal typé : erreur explicite, pas une procédure inconnue ni un panic.
#[test]
fn call_hybrid_search_argument_invalide() {
    let (db, _, _, _) = trois_docs_indexes();
    let s = db.session();
    let err = s
        .execute("CALL obrain.hybrid_search('Doc', 'title', 'emb', 42) YIELD node_id, score")
        .expect_err("query_text doit être une chaîne");
    let msg = err.to_string();
    assert!(msg.contains("query_text"), "l'erreur nomme l'argument fautif : {msg}");
    let err = s
        .execute("CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust', [1.0, 0.0, 0.0], {fusion: 'inconnue'}) YIELD node_id, score")
        .expect_err("fusion inconnue");
    let msg = err.to_string();
    assert!(msg.contains("fusion"), "l'erreur nomme l'option fautive : {msg}");
}
