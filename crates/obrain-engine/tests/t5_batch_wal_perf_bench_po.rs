//! T5 batch-WAL perf gate — REAL PO base variant.
//!
//! The tempdir variant (t5_batch_wal_perf_bench.rs) showed 1.0× speedup:
//! on a small fresh substrate, 200 MATCH+CREATE cypher calls cost ~330ms
//! regardless of SyncMode, meaning fsync is NOT the dominant cost there.
//! The original 2.1s baseline (gotcha 4af45f0a) was measured on the real
//! PO base (1.35M nodes) — this test isolates whether SyncMode::Group
//! actually helps AT THAT SCALE, or whether the dominant cost is
//! elsewhere (Cypher parse/plan overhead, label-index scan cost on a
//! large base) as the gotcha's own note hinted ("Cypher ~10ms/edge vs
//! API directe ~0.5ms/edge").
//!
//! Self-cleaning: uses the same `_BenchSrc`/`_BenchDst`/`_BENCH_REL`
//! labels as `po_vs_neo4j_bench.rs` (established precedent) and deletes
//! them at the end. `#[ignore]` — real prod base, run manually.
#![cfg(feature = "cypher")]

use web_time::Instant;

use obrain_engine::ObrainDB;
use obrain_substrate::wal_io::SyncMode;

#[test]
#[ignore = "touches the real PO base — self-cleaning, run manually"]
fn batch_wal_group_mode_on_real_po() {
    let home = std::env::var("HOME").unwrap_or_default();
    let po_path = std::path::PathBuf::from(&home).join(".obrain/db/po");
    if !po_path.exists() {
        eprintln!("⏭  PO not present");
        return;
    }

    // --- EveryCommit (current default behaviour) ---
    {
        let db = ObrainDB::open_substrate(&po_path).expect("open PO (EveryCommit)");
        let session = db.session();
        for i in 0..200 {
            let _ = session.execute_cypher(&format!("CREATE (:_BenchSrc {{id: {i}}})"));
            let _ = session.execute_cypher(&format!("CREATE (:_BenchDst {{id: {i}}})"));
        }
        let t0 = Instant::now();
        for i in 0..200 {
            session
                .execute_cypher(&format!(
                    "MATCH (a:_BenchSrc {{id: {i}}}), (b:_BenchDst {{id: {i}}}) \
                     CREATE (a)-[:_BENCH_REL]->(b)"
                ))
                .unwrap();
        }
        let elapsed = t0.elapsed();
        println!("  [PO] EveryCommit : {elapsed:?} for 200 relationships");

        // Cleanup before the Group-mode pass reopens the same path.
        let _ = session.execute_cypher("MATCH (n:_BenchSrc) DETACH DELETE n");
        let _ = session.execute_cypher("MATCH (n:_BenchDst) DETACH DELETE n");
        db.flush_wal().ok();
    }

    // --- Group mode + single flush_wal() at end-of-batch ---
    {
        let db =
            ObrainDB::open_substrate_with_mode(&po_path, SyncMode::Group).expect("open PO (Group)");
        let session = db.session();
        for i in 0..200 {
            let _ = session.execute_cypher(&format!("CREATE (:_BenchSrc {{id: {i}}})"));
            let _ = session.execute_cypher(&format!("CREATE (:_BenchDst {{id: {i}}})"));
        }
        let t0 = Instant::now();
        for i in 0..200 {
            session
                .execute_cypher(&format!(
                    "MATCH (a:_BenchSrc {{id: {i}}}), (b:_BenchDst {{id: {i}}}) \
                     CREATE (a)-[:_BENCH_REL]->(b)"
                ))
                .unwrap();
        }
        db.flush_wal().expect("flush_wal");
        let elapsed = t0.elapsed();
        println!("  [PO] Group+flush : {elapsed:?} for 200 relationships");

        // Cleanup — restore PO to its pre-test state.
        let _ = session.execute_cypher("MATCH (n:_BenchSrc) DETACH DELETE n");
        let _ = session.execute_cypher("MATCH (n:_BenchDst) DETACH DELETE n");
        db.flush_wal().ok();
    }
}
