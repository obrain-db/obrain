//! T17i T4 evidence — force the v5 → v6 upgrade via explicit
//! store.flush(), then measure steady-state DB open. Answers the
//! user's question "11 s était un one-shot, n'est-ce pas ?".
//!
//! The first phase opens each base, performs a trivial mutation to
//! force the flush path to run end-to-end (the in-memory v6
//! histograms — now populated by the T17i T2 init-time rebuild —
//! get persisted), then closes. The second phase re-opens and
//! measures the open time ; with v6 histograms on disk, the
//! `restore_counters_from_snapshot` path takes over and no rebuild
//! scan is triggered.

#![cfg(feature = "cypher")]

use obrain_bases_banc::{Base, base_copie};
use obrain_engine::ObrainDB;
use web_time::Instant;

fn upgrade_and_measure(name: &str, base: Base) {
    let Some(path) = base_copie(base) else {
        return;
    };

    // === Phase 1 — first open triggers v5 → v6 rebuild (slow), then
    // a trivial mutation + close forces the dict flush to persist v6.
    println!("\n  === {name} : Phase 1 (upgrade v5 → v6) ===");
    let t0 = Instant::now();
    let db = ObrainDB::open(&path).expect("open");
    let upgrade_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("    first open : {:>8.1} ms (v5 rebuild scan)", upgrade_ms);

    // Trivial mutation : create_node + delete_node. The store's
    // flush path writes the dict with the populated v6 histograms.
    let store = db.store();
    let t0 = Instant::now();
    let nid = store.create_node(&["__T17i_Probe__"]);
    store.delete_node(nid);
    let mutate_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    db.close().expect("close");
    let close_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "    mutate+close : {:>6.1} ms  (mutate {:.1} ms, close {:.1} ms)",
        mutate_ms + close_ms,
        mutate_ms,
        close_ms
    );
    drop(db);

    // === Phase 2 — re-open reads the now-v6 dict ; no rebuild.
    println!("  === {name} : Phase 2 (steady-state v6 open) ===");
    let mut samples_ms: Vec<f64> = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = Instant::now();
        let db = ObrainDB::open(&path).expect("open");
        samples_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        drop(db);
    }
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples_ms[samples_ms.len() / 2];
    println!(
        "    steady-state : p50={:>6.1} ms  min={:>6.1} ms  max={:>6.1} ms",
        p50,
        samples_ms.first().unwrap(),
        samples_ms.last().unwrap()
    );
}

#[test]
fn upgrade_then_measure_three_corpora() {
    println!("\n═══ T17i T4 evidence : v5 → v6 upgrade + steady-state ═══");
    upgrade_and_measure("PO", Base::Po);
    upgrade_and_measure("Wikipedia", Base::Wikipedia);
    upgrade_and_measure("Megalaw", Base::Megalaw);
    println!("═══ END ═══");
}
