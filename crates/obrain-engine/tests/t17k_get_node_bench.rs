//! T17k T3 step 4 — micro-bench get_node bulk-hydrate on prod bases.
#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
#[ignore = "bench on prod bases"]
fn bench_get_node_bulk_hydrate() {
    let home = std::env::var("HOME").unwrap_or_default();
    for (name, path, label) in [
        (
            "po",
            std::path::PathBuf::from(&home).join(".obrain/db/po"),
            "File",
        ),
        (
            "po",
            std::path::PathBuf::from(&home).join(".obrain/db/po"),
            "Decision",
        ),
        (
            "wikipedia",
            std::path::PathBuf::from(&home).join(".obrain/db/wikipedia"),
            "Article",
        ),
        (
            "megalaw",
            std::path::PathBuf::from(&home).join(".obrain/db/megalaw"),
            "Concept",
        ),
    ] {
        if !path.exists() {
            println!("⏭  {name}:{label} not present");
            continue;
        }
        let db = ObrainDB::open(&path).expect("open");
        let store = db.store();
        let nodes = store.nodes_by_label(label);
        if nodes.len() < 10 {
            continue;
        }
        let samples: Vec<_> = nodes.iter().take(1000).copied().collect();
        // Warm up caches
        for nid in samples.iter().take(50) {
            let _ = store.get_node(*nid);
        }
        let n_iters = 1000;
        let t0 = std::time::Instant::now();
        let mut total_props = 0usize;
        for _ in 0..=(n_iters / samples.len().max(1)) {
            for nid in &samples {
                if let Some(n) = store.get_node(*nid) {
                    total_props += n.properties.len();
                }
            }
        }
        let elapsed = t0.elapsed();
        let avg_per_call = elapsed / (n_iters as u32);
        let avg_props = total_props / n_iters.max(1);
        println!(
            "  {name}:{label:<10}  avg={:>6}μs/get_node  avg_props={avg_props:3}  (sample N={})",
            avg_per_call.as_micros(),
            samples.len(),
        );
    }
}
