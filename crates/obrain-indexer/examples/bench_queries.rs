//! Benchmark exhaustif : RAM (RSS interne via /proc) + indexation + toutes
//! les queries avec précision µs.
//!
//! Usage : cargo run --release -p obrain-indexer --example bench_queries -- <repo>
//! Pour le RSS max externe : /usr/bin/time -v <binaire> <repo>
use obrain_indexer::index::{IndexOptions, index_repository};
use obrain_indexer::schema::{EdgeKind, NodeKind};
use std::time::Instant;

fn rss_mb() -> u64 {
    // RSS courant du process (kB) depuis /proc/self/statm.
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = s
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pages * 4096 / 1024 / 1024
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let rss0 = rss_mb();

    let t0 = Instant::now();
    let out = index_repository(std::path::Path::new(&path), &IndexOptions::default());
    let idx_s = t0.elapsed().as_secs_f64();
    let g = &out.graph;
    let rss_idx = rss_mb();

    println!("=== BENCH COMPLET: {path} ===");
    println!(
        "indexation: {:.2} s | {} fichiers | {} nœuds | {} arêtes",
        idx_s,
        out.files_scanned,
        g.node_count(),
        g.edge_count()
    );
    println!(
        "RAM: {rss0} Mo (baseline) -> {rss_idx} Mo (après indexation) | Δ {} Mo",
        rss_idx.saturating_sub(rss0)
    );

    macro_rules! bench {
        ($name:expr, $n:expr, $code:expr) => {{
            let t = Instant::now();
            for _ in 0..$n {
                let _ = $code;
            }
            let us = t.elapsed().as_secs_f64() * 1e6 / $n as f64;
            println!("{:<32} {:>10.2} µs", $name, us);
        }};
    }

    // --- TOP-K (caches) ---
    bench!(
        "top-5 cognitive",
        2000,
        g.top_k_cached(NodeKind::Function, "cognitive", 5)
    );
    bench!(
        "top-5 complexity",
        2000,
        g.top_k_cached(NodeKind::Function, "complexity", 5)
    );
    bench!(
        "top-5 transitive_loop_depth",
        2000,
        g.top_k_cached(NodeKind::Function, "transitive_loop_depth", 5)
    );
    bench!(
        "top-5 loop_depth",
        2000,
        g.top_k_cached(NodeKind::Function, "loop_depth", 5)
    );
    bench!(
        "top-5 fichiers modifiés",
        2000,
        g.top_k_cached(NodeKind::File, "change_count", 5)
    );
    bench!(
        "top-100 cognitive",
        500,
        g.top_k_cached(NodeKind::Function, "cognitive", 100)
    );

    // --- Compteurs (bool_counts O(1)) ---
    bench!(
        "count récursives",
        200000,
        g.count_bool_metric(NodeKind::Function, "recursive")
    );
    bench!(
        "count is_test",
        200000,
        g.count_bool_metric(NodeKind::Function, "is_test")
    );
    bench!(
        "count has_docstring fns",
        200000,
        g.count_bool_metric(NodeKind::Function, "has_docstring")
    );

    // --- Compteurs edges (O(1)) ---
    bench!(
        "count edges TESTS",
        200000,
        g.edge_count_kind(EdgeKind::Tests)
    );
    bench!(
        "count edges CALLS",
        200000,
        g.edge_count_kind(EdgeKind::Calls)
    );
    bench!(
        "count edges DEFINES",
        200000,
        g.edge_count_kind(EdgeKind::Defines)
    );
    bench!(
        "count edges FILE_CHANGES_WITH",
        200000,
        g.edge_count_kind(EdgeKind::FileChangesWith)
    );

    // --- Lookups ---
    bench!(
        "lookup FQN (node_by_qn)",
        2000000,
        g.node_by_qn("linux-6.6.kernel.kthread.cleaner_kthread")
            .map(|n| n.id)
    );
    bench!(
        "positions_of_kind(Function)",
        2000000,
        g.positions_of_kind(NodeKind::Function).len()
    );

    // --- Requêtes complexes (edges FILE_CHANGES_WITH : tri top-5) ---
    let t = Instant::now();
    for _ in 0..200 {
        let _: Vec<_> = g
            .edge_positions(EdgeKind::FileChangesWith)
            .iter()
            .take(5)
            .collect();
    }
    println!(
        "{:<32} {:>10.2} µs",
        "top co-évolution (premiers)",
        t.elapsed().as_secs_f64() * 1e6 / 200.0
    );

    let rss_fin = rss_mb();
    println!("RAM finale: {rss_fin} Mo (queries n'allouent pas)");
}
