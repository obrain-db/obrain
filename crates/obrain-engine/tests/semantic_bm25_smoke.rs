#![cfg(feature = "text-index")]
//! Premier repère SÉMANTIQUE (RFC §14, axe S) : BM25 engine sur un corpus
//! texte réel (extraits des docs obrain). Qualité qualitative top-k + latence.
//! Étape suivante (formalisation recall@k) : labels BEIR sur corpus plus grand.

use obrain_engine::database::ObrainDB;

fn docs() -> Vec<&'static str> {
    vec![
        "Obrain is an embeddable high-performance graph database with zero external dependencies",
        "Hilbert space-filling curve orders nodes for physical locality and spreading-activation prefetch",
        "Engrams and Hebbian synapses implement cognitive memory; queries trigger spreading activation",
        "BM25 full-text search is exposed through text indexes on node properties",
        "HNSW vector search stores embeddings for semantic similarity over properties",
        "Hybrid retrieval fuses BM25 scores and vector similarity for better ranking",
        "The substrate storage is mmap-native with per-zone files and a write-ahead log",
        "Spreading activation propagates energy across the topology with decay and thresholds",
        "RAG pipelines orchestrate retrievers and ranking for grounded generation during inference",
        "Morton Z-order keys provide a hierarchical prefix index on the centrality by degree plane",
    ]
}

#[test]
fn bm25_semantic_smoke_topk_and_latency() {
    let db = ObrainDB::new_in_memory();
    let mut ids = Vec::new();
    for (i, d) in docs().iter().enumerate() {
        let n = db.create_node(&["Doc"]);
        db.set_node_property(n, "id", obrain_common::types::Value::Int64(i as i64));
        db.set_node_property(n, "text", obrain_common::types::Value::String((*d).into()));
        ids.push(n);
    }
    db.create_text_index("Doc", "text").unwrap();

    let queries = [
        "vector search embeddings",
        "graph database memory locality",
        "spreading activation cognitive",
        "bm25 full text",
    ];
    for q in &queries {
        let t0 = web_time::Instant::now();
        let res = db.text_search("Doc", "text", q, 3).unwrap();
        let us = t0.elapsed().as_micros();
        let mut top = Vec::new();
        for (id, _) in res.iter() {
            let node = db.get_node(*id).expect("node");
            let i = match node
                .properties
                .get(&obrain_common::types::PropertyKey::new("id"))
            {
                Some(obrain_common::types::Value::Int64(x)) => *x as usize,
                _ => 0,
            };
            let words: Vec<&str> = docs()[i].split(' ').take(4).collect();
            top.push(words.join(" "));
        }
        eprintln!("Q «{q}» : top3 = {top:?} ({us} µs)");
        assert!(!res.is_empty(), "BM25 doit répondre");
    }
    // Sanité : la requête « embedding vectoriel » ramène bien le doc HNSW/vector.
    let res = db
        .text_search("Doc", "text", "embeddings semantic similarity", 3)
        .unwrap();
    let top1 = db.get_node(res[0].0).unwrap();
    let t = match top1
        .properties
        .get(&obrain_common::types::PropertyKey::new("text"))
    {
        Some(obrain_common::types::Value::String(s)) => s.to_string(),
        _ => String::new(),
    };
    assert!(
        t.contains("vector"),
        "top-1 attendu : doc vector, got : {t}"
    );
    eprintln!("semantic smoke OK — pipeline BM25 engine fonctionnel (top-1 vérifié)");
}

#[test]
fn bm25_latency_scale_20k_docs() {
    use obrain_engine::database::ObrainDB;
    let db = ObrainDB::new_in_memory();
    let topics = [
        "graph", "vector", "search", "memory", "storage", "wal", "mmap", "spread", "engram",
        "index",
    ];
    let n = 20_000usize;
    let t0 = web_time::Instant::now();
    for i in 0..n {
        let d = db.create_node(&["Doc"]);
        let text = format!(
            "document number {i} about {} with tokens alpha beta gamma delta {}",
            topics[i % topics.len()],
            topics[(i / topics.len()) % topics.len()]
        );
        db.set_node_property(
            d,
            "text",
            obrain_common::types::Value::String(text.as_str().into()),
        );
        db.set_node_property(d, "uid", obrain_common::types::Value::Int64(i as i64));
    }
    db.create_text_index("Doc", "text").unwrap();
    let build_ms = t0.elapsed().as_millis();
    eprintln!("indexation BM25 de {n} docs : {build_ms} ms");

    let queries: Vec<String> = (0..200)
        .map(|q| {
            let a = topics[q % topics.len()];
            let b = topics[(q / topics.len()) % topics.len()];
            format!("{a} {b} tokens gamma")
        })
        .collect();
    // warm-up
    let _ = db.text_search("Doc", "text", &queries[0], 5).unwrap();
    let t1 = web_time::Instant::now();
    let mut hits = 0usize;
    for q in &queries {
        hits += db.text_search("Doc", "text", q, 5).unwrap().len();
    }
    let avg_us = t1.elapsed().as_micros() as f64 / queries.len() as f64;
    eprintln!("BM25 @ {n} docs : {avg_us:.1} µs/requête (k=5, 200 requêtes, {hits} hits)");
    assert!(hits > 0);
    // Sanité top-1 : requête ciblant un seul doc (uid exact).
    let res = db
        .text_search("Doc", "text", "document number 42 about index", 1)
        .unwrap();
    let top1 = db.get_node(res[0].0).unwrap();
    let uid = match top1
        .properties
        .get(&obrain_common::types::PropertyKey::new("uid"))
    {
        Some(obrain_common::types::Value::Int64(x)) => *x,
        _ => -1,
    };
    assert_eq!(uid, 42, "top-1 doit être le doc 42, got {uid}");
    eprintln!("scale smoke OK — top-1 exact vérifié sur 20 k docs");
}
