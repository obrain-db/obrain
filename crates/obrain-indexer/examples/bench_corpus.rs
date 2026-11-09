//! Bench comparatif M6 — indexeur natif obrain vs indexeur C codebase-memory.
//!
//! Usage : cargo run --release -p obrain-indexer --example bench_corpus -- <repo>
//!
//! Mesure sur le MÊME corpus :
//! - temps d'indexation + richesse du graphe (nœuds par label, arêtes par type)
//! - la référence C est la base SQLite déjà produite par l'indexeur C :
//!   ~/.cache/codebase-memory-mcp/<projet>.db (stats extraites via le graphe
//!   indexé du brain — projet `obrain-engine`).
//!
//! Référence C sur obrain-engine (fichiers .rs, extrait 2026-09-06 via le
//! graphe) : 784 Files, 11534 Functions, 8461 Methods, 1401 Structs, 342
//! Enums, 66 Interfaces ; CALLS 67051, USAGE 78284, DEFINES 28229, TESTS
//! 16691, DEFINES_METHOD 8235, IMPORTS 2713, SIMILAR_TO 3779...

use obrain_indexer::index::{IndexOptions, index_repository};
use obrain_indexer::schema::{EdgeKind, NodeKind};
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = std::path::Path::new(&path);
    let t0 = Instant::now();
    let out = index_repository(root, &IndexOptions::default());
    let d = t0.elapsed();

    let g = &out.graph;
    println!("=== bench M6 (indexeur natif) ===");
    println!("depot   : {path}");
    println!("temps   : {:.3} s", d.as_secs_f64());
    println!("fichiers parses : {}", out.files_scanned);

    let mut nodes: HashMap<NodeKind, usize> = Default::default();
    for n in g.nodes() {
        *nodes.entry(n.kind).or_default() += 1;
    }
    let mut edges: HashMap<EdgeKind, usize> = Default::default();
    for e in g.edges() {
        *edges.entry(e.kind).or_default() += 1;
    }

    println!("noeuds  : {}", g.node_count());
    let mut nv: Vec<_> = nodes.into_iter().collect();
    nv.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (k, c) in nv.iter().take(10) {
        println!("  noeud   {:<12} {}", k.label(), c);
    }
    println!("aretes  : {}", g.edge_count());
    let mut ev: Vec<_> = edges.into_iter().collect();
    ev.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (k, c) in ev.iter().take(10) {
        println!("  arete   {:<12} {}", k.label(), c);
    }
    for w in &out.warnings {
        println!("  note: {w}");
    }
}
