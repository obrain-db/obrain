//! Micro-bench (perf(query) 399992e3): isolate the LIMIT-pushdown gain —
//! `nodes_by_label` (full slot scan) vs `nodes_by_label_capped` (break
//! after N matches) on the real PO base (1.35M slots, ChatEvent = 681k).
//!
//! Run: cargo test -p obrain-substrate --test label_scan_cap_bench -- \
//!        --include-ignored --nocapture

use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;
use std::time::Instant;

#[test]
#[ignore = "bench on prod base — read-only"]
fn label_scan_capped_vs_full() {
    let home = std::env::var("HOME").unwrap_or_default();
    let po = std::path::PathBuf::from(&home).join(".obrain/db/po");
    if !po.exists() {
        eprintln!("⏭  PO not present");
        return;
    }
    let store = SubstrateStore::open(&po).expect("open PO");

    // Warm-up pass so both measurements see the same page cache.
    let warm = store.nodes_by_label("ChatEvent");
    println!("  ChatEvent live count = {}", warm.len());

    let runs = 5;

    let t = Instant::now();
    for _ in 0..runs {
        let v = store.nodes_by_label("ChatEvent");
        assert!(v.len() > 100_000);
    }
    let full = t.elapsed() / runs;

    let t = Instant::now();
    for _ in 0..runs {
        let v = store.nodes_by_label_capped("ChatEvent", 5);
        assert_eq!(v.len(), 5);
    }
    let capped = t.elapsed() / runs;

    let speedup = full.as_nanos() as f64 / capped.as_nanos().max(1) as f64;
    println!("  full scan  (nodes_by_label)          : {full:?}/call");
    println!("  capped 5   (nodes_by_label_capped)   : {capped:?}/call");
    println!("  speedup    : {speedup:.0}×");

    // Regression gate: the capped path must stay well over an order of
    // magnitude cheaper than the full scan on a big label.
    // Measured 2026-07-02 (PO warm, 681k ChatEvents / 1.35M slots):
    // full = 9.6 ms/call, capped-5 = 136 µs/call → 71×. The capped cost
    // is dominated by scanning the slots BEFORE the first ChatEvent, so
    // it depends on label placement — hence a conservative 20× floor.
    assert!(
        speedup > 20.0,
        "LIMIT pushdown regressed: only {speedup:.0}× vs full scan"
    );
}
