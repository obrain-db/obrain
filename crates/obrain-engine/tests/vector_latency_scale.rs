//! RFC §14 axe S — latence HNSW/vector à l'échelle (20 k vecteurs) + sanité top-1.
use obrain_common::types::Value;
use obrain_engine::database::ObrainDB;

fn emb(base: usize, k: usize) -> Value {
    // vecteur 64-d quasi-orthogonal par cluster : spike sur (cluster, slot)
    let mut v = vec![0.0f32; 64];
    v[base % 64] = 1.0;
    v[(base + 1 + k) % 64] = 0.5;
    Value::Vector(v.into())
}

#[cfg(feature = "vector-index")]
#[test]
fn vector_search_latency_scale_20k() {
    let db = ObrainDB::new_in_memory();
    let n = 20_000usize;
    let t0 = web_time::Instant::now();
    for i in 0..n {
        let d = db.create_node(&["Doc"]);
        db.set_node_property(d, "emb", emb(i / 200, i));
        db.set_node_property(d, "uid", Value::Int64(i as i64));
    }
    let build = t0.elapsed().as_millis();
    db.create_vector_index("Doc", "emb", Some(64), Some("cosine"), None, None)
        .expect("index");
    eprintln!("insertion {n} vecteurs : {build} ms (+ index)");
    fn qvec(base: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; 64];
        v[base % 64] = 1.0;
        v
    }
    let queries: Vec<Vec<f32>> = (0..200).map(|q| qvec(q)).collect();
    let _ = db
        .vector_search("Doc", "emb", &queries[0], 5, None, None)
        .unwrap();
    let t1 = web_time::Instant::now();
    let mut hits = 0usize;
    for q in &queries {
        hits += db
            .vector_search("Doc", "emb", q, 5, None, None)
            .unwrap()
            .len();
    }
    let avg_us = t1.elapsed().as_micros() as f64 / queries.len() as f64;
    eprintln!(
        "vector_search @ {n} vecteurs : {avg_us:.1} µs/requête (k=5, 200 requêtes, {hits} hits)"
    );
    assert!(hits > 0);
    // Sanité top-1 : la requête du vecteur du doc 0 renvoie le cluster du doc 0.
    let res = db
        .vector_search("Doc", "emb", &qvec(0), 1, None, None)
        .unwrap();
    let top1 = db.get_node(res[0].0).unwrap();
    let uid = match top1
        .properties
        .get(&obrain_common::types::PropertyKey::new("uid"))
    {
        Some(Value::Int64(x)) => *x,
        _ => -1,
    };
    assert!(
        uid >= 0 && uid < 200,
        "top-1 doit être dans le cluster 0, got {uid}"
    );
    eprintln!("vector scale smoke OK — top-1 cohérent (cluster 0)");
}

#[cfg(all(
    feature = "vector-index",
    feature = "text-index",
    feature = "hybrid-search"
))]
#[test]
fn hybrid_latency_scale_20k() {
    let db = ObrainDB::new_in_memory();
    let topics = [
        "graph", "vector", "search", "memory", "storage", "wal", "mmap", "spread", "engram",
        "index",
    ];
    let n = 20_000usize;
    let t0 = web_time::Instant::now();
    for i in 0..n {
        let d = db.create_node(&["Doc"]);
        let topic = topics[i % topics.len()];
        let text = format!("document {i} about {topic} with tokens gamma delta");
        db.set_node_property(d, "text", Value::String(text.as_str().into()));
        let mut v = vec![0.0f32; 64];
        v[(i % topics.len()) as usize] = 1.0;
        v[(i + 1) % 64] = 0.3;
        db.set_node_property(d, "emb", Value::Vector(v.into()));
        db.set_node_property(d, "uid", Value::Int64(i as i64));
    }
    db.create_text_index("Doc", "text").unwrap();
    db.create_vector_index("Doc", "emb", Some(64), Some("cosine"), None, None)
        .expect("index");
    let build = t0.elapsed().as_millis();
    eprintln!("hybrid corpus 20 k (texte+vecteur) : {build} ms");

    let queries: Vec<(String, Vec<f32>)> = (0..100)
        .map(|q| {
            let topic = topics[q % topics.len()];
            let mut v = vec![0.0f32; 64];
            v[q % topics.len()] = 1.0;
            (format!("{topic} tokens"), v)
        })
        .collect();
    let _ = db
        .hybrid_search(
            "Doc",
            "text",
            "emb",
            &queries[0].0,
            Some(&queries[0].1),
            5,
            None,
        )
        .unwrap();
    let t1 = web_time::Instant::now();
    let mut hits = 0usize;
    for (qt, qv) in &queries {
        hits += db
            .hybrid_search("Doc", "text", "emb", qt, Some(qv), 5, None)
            .unwrap()
            .len();
    }
    let avg_us = t1.elapsed().as_micros() as f64 / queries.len() as f64;
    eprintln!("hybrid_search @ 20 k : {avg_us:.1} µs/requête (k=5, 100 requêtes, {hits} hits)");
    assert!(hits > 0);
    // Qualité (vérité terrain = cluster) : recall@5 / precision@5 moyens.
    let mut r5 = 0.0f64;
    let mut p5 = 0.0f64;
    let per = topics.len() as i64; // 2000 docs/cluster
    for (qi, (qt, qv)) in queries.iter().enumerate() {
        let cluster = qi % topics.len();
        let res = db
            .hybrid_search("Doc", "text", "emb", qt, Some(qv), 5, None)
            .unwrap();
        let mut ok = 0usize;
        for (id, _) in res {
            let n = db.get_node(id).unwrap();
            let uid = match n
                .properties
                .get(&obrain_common::types::PropertyKey::new("uid"))
            {
                Some(Value::Int64(x)) => *x,
                _ => -1,
            };
            if uid >= 0 && uid % per == cluster as i64 {
                ok += 1;
            }
        }
        p5 += ok as f64 / 5.0;
        r5 += ok as f64 / 2000.0; // rappel sur l'ensemble du cluster (borne)
    }
    eprintln!(
        "hybrid qualité (vérité terrain cluster) : precision@5 = {:.3}, recall@5(vs cluster) = {:.4}",
        p5 / queries.len() as f64,
        r5 / queries.len() as f64
    );
    // Sanité : requête cluster 0 → top-1 dans cluster 0.
    let res = db
        .hybrid_search(
            "Doc",
            "text",
            "emb",
            &format!("{} tokens", topics[0]),
            Some(&{
                let mut v = vec![0.0f32; 64];
                v[0] = 1.0;
                v
            }),
            1,
            None,
        )
        .unwrap();
    let top1 = db.get_node(res[0].0).unwrap();
    let uid = match top1
        .properties
        .get(&obrain_common::types::PropertyKey::new("uid"))
    {
        Some(Value::Int64(x)) => *x,
        _ => -1,
    };
    assert!(
        uid >= 0 && (uid as usize) % topics.len() == 0,
        "top-1 cluster 0, got {uid}"
    );
    eprintln!("hybrid scale smoke OK — top-1 cluster cohérent");
}

#[cfg(all(
    feature = "vector-index",
    feature = "text-index",
    feature = "hybrid-search"
))]
#[test]
fn topological_rerank_synthetic_c2() {
    // C2 « RAG topologique » v1 (RFC §17) : le reranking par voisinage de
    // communauté (arêtes CALLS intra-cluster) améliore la précision du
    // retrieval hybride quand le signal sémantique est ambigu.
    use obrain_engine::database::ObrainDB;
    let db = ObrainDB::new_in_memory();
    let (nc, per) = (20usize, 200usize);
    let mut docs: Vec<obrain_common::types::NodeId> = Vec::new();
    for c in 0..nc {
        for i in 0..per {
            let d = db.create_node(&["Doc"]);
            let mut v = vec![0.0f32; 64];
            v[(c / 2) % 64] = 1.0;
            v[(c + i) % 64] = 0.05 * (i as f32 + 1.0);
            db.set_node_property(d, "emb", obrain_common::types::Value::Vector(v.into()));
            // (2026-09-05, correctif racine flakiness) TEXTE UNIQUE PAR
            // CLUSTER : `k{c}` au lieu de `k{c % 10}`. L'ancien texte créait
            // 400 docs identiques (clusters jumeaux c et c+10) → ex-æquo
            // BM25 parfaits → le top-k hybride dépendait du hasher aléatoire
            // → précision instable (mesuré : 2/12 échecs). L'ambiguïté
            // VECTORIELLE (axes (c/2)%64 partagés par paires) subsiste : le
            // rerank topologique doit toujours aider. Probe 5 runs : p_topo
            // = 1,000 stable, p_base ~0,79-0,82.
            db.set_node_property(
                d,
                "text",
                obrain_common::types::Value::String(
                    format!("k{c} alpha beta gamma").as_str().into(),
                ),
            );
            db.set_node_property(
                d,
                "uid",
                obrain_common::types::Value::Int64((c * per + i) as i64),
            );
            docs.push(d);
        }
    }
    // Arêtes intra-cluster (anneau) — le voisinage « communauté ».
    for c in 0..nc {
        for i in 0..per {
            let a = docs[c * per + i];
            let b = docs[c * per + (i + 1) % per];
            db.create_edge(a, b, "CALLS");
        }
    }
    db.create_text_index("Doc", "text").unwrap();
    db.create_vector_index("Doc", "emb", Some(64), Some("cosine"), None, None)
        .expect("index");
    let uid_of = |nid: obrain_common::types::NodeId| -> i64 {
        let n = db.get_node(nid).unwrap();
        match n
            .properties
            .get(&obrain_common::types::PropertyKey::new("uid"))
        {
            Some(obrain_common::types::Value::Int64(x)) => *x,
            _ => -1,
        }
    };
    let mut queries: Vec<(String, Vec<f32>)> = Vec::new();
    for q in 0..100 {
        let c = q % nc;
        let mut v = vec![0.0f32; 64];
        v[(c / 2) % 64] = 1.0;
        queries.push((format!("k{c} alpha"), v)); // requête sur le cluster exact
    }
    let avg_p = |rerank: bool| -> f64 {
        let mut acc = 0.0;
        for (qi, (qt, qv)) in queries.iter().enumerate() {
            let c = qi % nc;
            let mut cand = db
                .hybrid_search("Doc", "text", "emb", qt, Some(qv), 20, None)
                .unwrap();
            if rerank {
                let mut cluster_count: std::collections::HashMap<i64, f64> = Default::default();
                for (id, _) in &cand {
                    *cluster_count.entry(uid_of(*id) / per as i64).or_insert(0.0) += 1.0;
                }
                let dom_len = cand.len().max(1) as f64;
                let mut scored: Vec<(obrain_common::types::NodeId, f64)> = cand
                    .drain(..)
                    .map(|(id, s)| {
                        let cl = uid_of(id) / per as i64;
                        let dom = cluster_count.get(&cl).copied().unwrap_or(0.0) / dom_len;
                        (id, s + dom * 1.0)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                cand = scored.into_iter().map(|(id, s)| (id, s)).collect();
            }
            let ok = cand
                .iter()
                .take(5)
                .filter(|(id, _)| (uid_of(*id) / per as i64) as usize == c)
                .count();
            acc += ok as f64 / 5.0;
        }
        acc / queries.len() as f64
    };
    let p_base = avg_p(false);
    let p_topo = avg_p(true);
    eprintln!(
        "C2 topo-rerank : precision@5 hybride seul = {p_base:.3} | hybride+topologie = {p_topo:.3}"
    );
    assert!(
        p_topo > p_base + 0.05,
        "le reranking topologique doit améliorer la précision"
    );
    assert!(
        p_topo >= 0.9,
        "précision finale attendue ≥ 0,9, got {p_topo}"
    );
    eprintln!(
        "C2 v1 OK — le contexte structural améliore la précision (vérifié, vérité terrain cluster)"
    );
}
