//! Persiste le graphe indexé M6 dans un store substrate obrain (couplage
//! obrain ↔ llama-obrain, sans CBM).
//! Usage : cargo run --release -p obrain-indexer --example persist -- <repo> <out.obrain>
use obrain_indexer::index::{IndexOptions, index_repository};
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let repo = a.next().unwrap_or_else(|| ".".to_string());
    let out = a.next().expect("usage: persist <repo> <out.obrain>");
    let root = std::path::Path::new(&repo);

    let t0 = Instant::now();
    let idx = index_repository(root, &IndexOptions::default());
    let g = &idx.graph;
    let t_idx = t0.elapsed();
    let db = obrain_engine::ObrainDB::open_substrate(&out).expect("open store");
    let mut map: HashMap<u64, u64> = HashMap::new();
    let mut bykind: HashMap<&str, u64> = HashMap::new();
    for n in g.nodes() {
        let label = n.kind.label();
        *bykind.entry(label).or_default() += 1;
        let nid = db.create_node(&[label]);
        map.insert(n.id.0, nid.0);
        db.set_node_property(nid, "qualified_name", obrain_common::types::Value::String(n.qualified_name.clone().into()));
        if let Some(name) = &n.props.name {
            db.set_node_property(nid, "name", obrain_common::types::Value::String(name.clone().into()));
        }
        if let Some(fp) = &n.props.file_path {
            db.set_node_property(nid, "file_path", obrain_common::types::Value::String(fp.clone().into()));
        }
        if let (Some(a1), Some(a2)) = (n.props.start_line, n.props.end_line) {
            db.set_node_property(nid, "start_line", obrain_common::types::Value::Int64(a1 as i64));
            db.set_node_property(nid, "end_line", obrain_common::types::Value::Int64(a2 as i64));
        }
        // Texte pour l'extraction d'état KV (facts) : docstring sinon signature.
        let txt = ["docstring", "signature"]
            .iter()
            .find_map(|k| n.props.extra.get(*k).and_then(|v| v.as_str()));
        if let Some(t) = txt {
            db.set_node_property(nid, "text", obrain_common::types::Value::String(t.to_string().into()));
        }
        for (k, v) in &n.props.extra {
            let val = match v {
                serde_json::Value::String(s) => obrain_common::types::Value::String(s.clone().into()),
                serde_json::Value::Bool(b) => obrain_common::types::Value::Bool(*b),
                serde_json::Value::Number(x) => x.as_i64()
                    .map(obrain_common::types::Value::Int64)
                    .unwrap_or(obrain_common::types::Value::Float64(x.as_f64().unwrap_or(0.0))),
                _ => continue,
            };
            db.set_node_property(nid, k, val);
        }
    }
    let t_nodes = t0.elapsed();
    let mut edges = 0u64;
    for e in g.edges() {
        let (Some(&s), Some(&t)) = (map.get(&e.source.0), map.get(&e.target.0)) else { continue };
        let eid = db.create_edge(obrain_common::types::NodeId(s), obrain_common::types::NodeId(t), e.kind.label());
        edges += 1;
        // Provenance des arêtes (chantier « résolution sémantique », étape 1) :
        // les CALLS portent strategy/confidence/line/caller_scope — traçables
        // depuis le store (prérequis « apprentissage » / re-ranker).
        for (k, v) in &e.props.extra {
            let val = match v {
                serde_json::Value::String(s) => obrain_common::types::Value::String(s.clone().into()),
                serde_json::Value::Bool(b) => obrain_common::types::Value::Bool(*b),
                serde_json::Value::Number(x) => x.as_i64()
                    .map(obrain_common::types::Value::Int64)
                    .unwrap_or(obrain_common::types::Value::Float64(x.as_f64().unwrap_or(0.0))),
                _ => continue,
            };
            db.set_edge_property(eid, k, val);
        }
    }
    let _ = db.checkpoint();
    // Index d'égalité natif substrate sur qualified_name (lookups O(1)/µs) —
    // politique « µs par défaut » pour tout store produit par persist.
    db.create_property_index("qualified_name");
    let t_end = t0.elapsed();
    println!("=== persist M6 → substrate ===");
    println!("repo    : {repo}");
    println!("out     : {out}");
    println!("index   : {:.3} s ({} fichiers)", t_idx.as_secs_f64(), idx.files_scanned);
    println!("noeuds  : {}  aretes : {}", g.node_count(), edges);
    let mut v: Vec<_> = bykind.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, c) in v.iter().take(10) {
        println!("  {:<10} {}", k, c);
    }
    println!("persist : {:.3} s | total {:.3} s", (t_end - t_nodes).as_secs_f64(), t_end.as_secs_f64());
}
