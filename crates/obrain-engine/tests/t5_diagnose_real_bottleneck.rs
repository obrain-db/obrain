//! T5 re-diagnosis: SyncMode::Group only gave ~1.1× on real PO (8.6s vs
//! 9.4s for 200 relationships) — NOT the 10× the gotcha 4af45f0a
//! hypothesis (fsync-per-TransactionCommit) predicted. This isolates the
//! actual dominant cost: each of the 200 `execute_cypher` calls embeds a
//! different literal (`{i}`) in the query STRING, so the query cache
//! never hits — every call pays a full parse+plan cycle. Compares:
//! (a) the Cypher MATCH+CREATE loop (baseline, ~9.4s on PO)
//! (b) the same 200 edges via the direct GraphStoreMut API (no parser,
//!     no planner) to isolate parse+plan overhead from everything else.
#![cfg(feature = "cypher")]

use web_time::Instant;

use obrain_engine::ObrainDB;

#[test]
#[ignore = "touches the real PO base — self-cleaning, run manually"]
fn diagnose_cypher_parse_plan_vs_direct_api() {
    let home = std::env::var("HOME").unwrap_or_default();
    let po_path = std::path::PathBuf::from(&home).join(".obrain/db/po");
    if !po_path.exists() {
        eprintln!("⏭  PO not present");
        return;
    }

    let db = ObrainDB::open_substrate(&po_path).expect("open PO");
    let store = db.graph_store();

    // Direct API: create 200 nodes + 200 edges bypassing Cypher entirely.
    let mut srcs = Vec::with_capacity(200);
    let mut dsts = Vec::with_capacity(200);
    let t_setup = Instant::now();
    for i in 0..200 {
        let s = store.create_node(&["_BenchSrc"]);
        store.set_node_property(s, "id", obrain_common::types::Value::Int64(i));
        srcs.push(s);
        let d = store.create_node(&["_BenchDst"]);
        store.set_node_property(d, "id", obrain_common::types::Value::Int64(i));
        dsts.push(d);
    }
    println!(
        "  [PO] direct-API node setup (400 nodes) : {:?}",
        t_setup.elapsed()
    );

    let t0 = Instant::now();
    for i in 0..200usize {
        store.create_edge(srcs[i], dsts[i], "_BENCH_REL");
    }
    let direct_elapsed = t0.elapsed();
    println!("  [PO] direct API create_edge×200        : {direct_elapsed:?}");

    // Cleanup via direct API too.
    for &n in srcs.iter().chain(dsts.iter()) {
        let _ = store.delete_node(n);
    }
    db.flush_wal().ok();

    println!(
        "  ratio vs Cypher MATCH+CREATE baseline (9.43s measured earlier): {:.0}×",
        9.43 / direct_elapsed.as_secs_f64().max(1e-9)
    );
}
