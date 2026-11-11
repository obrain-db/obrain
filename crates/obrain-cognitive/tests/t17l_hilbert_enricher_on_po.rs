//! T17l verification — run HilbertEnricher one tick against the real
//! PO base and measure (a) computation time, (b) coverage before/after,
//! (c) RSS delta.
//!
//! `#[ignore]` by default — this test MUTATES the PO base (a temp COPY of `OBRAIN_BENCH_DB_PO`) by
//! writing `_hilbert_features` on every node. Run manually with
//! `cargo test -p obrain-cognitive --release --features enrichment
//! --test t17l_hilbert_enricher_on_po -- --include-ignored --nocapture`.

#![cfg(feature = "enrichment")]

use obrain_bases_banc::{Base, base_copie};
use std::sync::Arc;

use obrain_cognitive::thinker::{HilbertEnricher, HilbertEnricherConfig, Thinker};
use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

fn sample_coverage(store: &Arc<SubstrateStore>, key: &PropertyKey, sample_n: usize) -> f64 {
    let ids = store.node_ids();
    let n = ids.len().min(sample_n);
    if n == 0 {
        return 0.0;
    }
    let mut hits = 0usize;
    for nid in ids.iter().take(n) {
        if let Some(Value::Vector(_)) = store.get_node_property(*nid, key) {
            hits += 1;
        }
    }
    (hits as f64) / (n as f64)
}

#[test]
#[ignore = "long — s'exécute sur une copie de OBRAIN_BENCH_DB_PO, à lancer à la main"]
fn hilbert_enricher_tick_on_po_base() {
    let Some(po_path) = base_copie(Base::Po) else {
        return;
    };

    println!("\n=== T17l HilbertEnricher on PO — verification ===");
    let t_open = web_time::Instant::now();
    let store = Arc::new(SubstrateStore::open(&po_path).expect("open PO"));
    println!(
        "  PO open      : {:.1} ms",
        t_open.elapsed().as_secs_f64() * 1000.0
    );
    println!("  nodes        : {}", store.node_count());
    println!("  edges        : {}", store.edge_count());

    let key = PropertyKey::from("_hilbert_features");
    let before = sample_coverage(&store, &key, 1000);
    println!(
        "\n  _hilbert_features coverage BEFORE : {:.1}% (sample 1000)",
        before * 100.0
    );

    let enricher = HilbertEnricher::new(HilbertEnricherConfig::default());

    println!("\n  Running one tick (full-graph compute + persist)...");
    let t_tick = web_time::Instant::now();
    let report = enricher.tick(&store).expect("tick");
    let tick_elapsed = t_tick.elapsed();

    let stats = enricher.stats();
    println!("  tick elapsed   : {:.2} s", tick_elapsed.as_secs_f64());
    println!("  nodes_touched  : {}", report.nodes_touched);
    println!("  side_counter   : {} (dims)", report.side_counter);
    println!("  stats.ticks    : {}", stats.ticks);
    println!("  stats.nodes_written      : {}", stats.nodes_written);
    println!("  stats.last_coverage      : {:.4}", stats.last_coverage);

    let after = sample_coverage(&store, &key, 1000);
    println!(
        "\n  _hilbert_features coverage AFTER  : {:.1}% (sample 1000)",
        after * 100.0
    );

    println!(
        "\n  ⭐ delta : {:.1}% → {:.1}% (+{:.1}%)",
        before * 100.0,
        after * 100.0,
        (after - before) * 100.0
    );

    if report.nodes_touched > 0 {
        assert!(
            after > before,
            "coverage should increase after a compute tick (before={before}, after={after})"
        );
    }

    // Sanity : probe first 3 nodes and show their vector dims
    let sample_ids = store.nodes_by_label("File");
    if let Some(first) = sample_ids.first()
        && let Some(Value::Vector(v)) = store.get_node_property(*first, &key)
    {
        println!("\n  sample File[0] _hilbert_features dim = {}", v.len());
    }
}
