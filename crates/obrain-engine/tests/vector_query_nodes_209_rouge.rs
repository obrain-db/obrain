//! #209 — LE ROUGE SUR MAIN : `CALL db.index.vector.queryNodes` doit EXISTER.
//!
//! Ce fichier n'utilise que des API présentes avant #209 (`create_vector_index`,
//! `execute_cypher_with_params`, `obrain.procedures()`), pour que le rouge sur
//! `origin/main` soit le comportement mesuré — `Unknown procedure:
//! 'db.index.vector.queryNodes'` — et non une erreur de compilation. Les
//! comportements qui demandent un index NOMMÉ sont dans
//! `vector_query_nodes_209.rs`.

#![cfg(all(feature = "cypher", feature = "vector-index", feature = "algos"))]

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn liste(v: &[f64]) -> Value {
    Value::List(
        v.iter()
            .map(|x| Value::Float64(*x))
            .collect::<Vec<_>>()
            .into(),
    )
}

/// La requête exacte de PO (note.rs), sur un index qui n'a pas de nom : la
/// procédure doit exister, et l'erreur doit NOMMER l'index inconnu — ni
/// « Unknown procedure », ni un résultat vide silencieux.
#[test]
fn query_nodes_existe_et_nomme_l_index_inconnu() {
    let db = ObrainDB::new_in_memory();
    for (i, v) in [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.7, 0.7, 0.0]]
        .iter()
        .enumerate()
    {
        let n = db.create_node(&["Note"]);
        db.set_node_property(n, "embedding", Value::Vector(v.to_vec().into()));
        db.set_node_property(n, "status", Value::String("active".into()));
        db.set_node_property(n, "project_id", Value::String("p".into()));
        db.set_node_property(n, "rang", Value::Int64(i as i64));
    }
    // Un index vectoriel existe sur :Note(embedding), mais sans nom.
    db.create_vector_index("Note", "embedding", Some(3), Some("cosine"), None, None)
        .expect("index vectoriel");

    let params = HashMap::from([
        ("query_limit".to_string(), Value::Int64(3)),
        ("embedding".to_string(), liste(&[1.0, 0.0, 0.0])),
        ("project_id".to_string(), Value::String("p".into())),
        ("limit".to_string(), Value::Int64(2)),
    ]);
    let r = db.execute_cypher_with_params(
        "CALL db.index.vector.queryNodes('note_embeddings', $query_limit, $embedding) \
         YIELD node AS n, score \
         WHERE n.status IN ['active', 'needs_review'] AND n.project_id = $project_id \
         RETURN n, score ORDER BY score DESC LIMIT $limit",
        params,
    );
    let erreur = match r {
        Ok(res) => panic!(
            "un index inconnu doit lever une erreur, pas rendre {} ligne(s) : {:?}",
            res.rows.len(),
            res.rows
        ),
        Err(e) => e.to_string(),
    };
    assert!(
        !erreur.contains("Unknown procedure"),
        "la procédure doit exister : {erreur}"
    );
    assert!(
        erreur.contains("'note_embeddings'"),
        "l'erreur doit nommer l'index inconnu : {erreur}"
    );
}

/// `obrain.procedures()` dit la vérité : la procédure y figure.
#[test]
fn query_nodes_figure_dans_obrain_procedures() {
    let db = ObrainDB::new_in_memory();
    let r = db
        .session()
        .execute("CALL obrain.procedures()")
        .expect("obrain.procedures()");
    let ligne = r
        .rows
        .iter()
        .find(
            |row| matches!(&row[0], Value::String(s) if s.as_str() == "db.index.vector.queryNodes"),
        )
        .unwrap_or_else(|| panic!("db.index.vector.queryNodes absente : {:?}", r.rows));
    assert_eq!(ligne[3], Value::from("node, score"));
}
