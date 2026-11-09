//! Bench M6 — indexe un dépôt Rust réel et mesure le temps + la richesse.
//! Usage : cargo run --release -p obrain-indexer --example bench_index -- <path>
use obrain_indexer::index::{IndexOptions, index_repository};
use obrain_indexer::schema::NodeKind;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = std::path::Path::new(&path);
    let t0 = Instant::now();
    let out = index_repository(root, &IndexOptions::default());
    let d = t0.elapsed();

    let g = &out.graph;
    println!("=== bench indexation M6 (Rust seul) ===");
    println!("depot   : {path}");
    println!("temps   : {:.3} s", d.as_secs_f64());
    println!("fichiers parses : {}", out.files_scanned);
    println!("noeuds  : {}  aretes : {}", g.node_count(), g.edge_count());
    let mut counts: std::collections::HashMap<NodeKind, usize> = Default::default();
    for n in g.nodes() {
        *counts.entry(n.kind).or_default() += 1;
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (k, c) in v.iter().take(8) {
        println!("  {:<10} {}", k.label(), c);
    }
    for w in &out.warnings {
        println!("  note: {w}");
    }
}
