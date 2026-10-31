//! T5 batch-WAL — SyncMode::Group regression gate + honest re-diagnosis.
//!
//! Original hypothesis (gotcha 4af45f0a): `TransactionCommit` fsync per
//! `execute_cypher` call dominates the "200 relationships = 2.1s" cost.
//! DISPROVEN on the real PO base (gotcha 0f852da1, 2026-07-07): Group mode
//! gave only ~1.1× there (8.56s vs 9.43s), not 10×. The actual dominant
//! cost is Cypher parse+plan overhead — each loop iteration embeds a
//! different literal in the query string, so the query cache never hits
//! and every call pays a full parse+plan cycle (cost scales with PO's
//! large catalog/dict). Isolated proof: the same 200 edges via the direct
//! `GraphStoreMut` API (bypassing the parser entirely) took 272ms on real
//! PO — a 35× win over the Cypher loop, dwarfing anything SyncMode can
//! give. Actionable lesson for T6 (wardens) / T8 (neo4j2obrain import):
//! use the direct API for bulk writes, not interpolated Cypher strings.
//!
//! This test keeps SyncMode::Group as what it actually is: valid, tested
//! infrastructure with a real-but-modest (~10%) benefit for callers that
//! must stay on the Cypher/transaction-commit path. Uses a fresh tempdir
//! substrate (never prod PO — gotcha 5906c9b3).
#![cfg(feature = "cypher")]

use web_time::Instant;

use obrain_engine::ObrainDB;
use obrain_substrate::wal_io::SyncMode;

#[test]
fn batch_wal_group_mode_no_regression() {
    // --- Baseline: default EveryCommit (unchanged user-facing behaviour) ---
    let td_before = tempfile::tempdir().unwrap();
    let db_before = ObrainDB::open_substrate(td_before.path().join("kb")).unwrap();
    let session_before = db_before.session();
    for i in 0..200 {
        let _ = session_before.execute_cypher(&format!("CREATE (:_BenchSrc {{id: {i}}})"));
        let _ = session_before.execute_cypher(&format!("CREATE (:_BenchDst {{id: {i}}})"));
    }
    let t0 = Instant::now();
    for i in 0..200 {
        session_before
            .execute_cypher(&format!(
                "MATCH (a:_BenchSrc {{id: {i}}}), (b:_BenchDst {{id: {i}}}) \
                 CREATE (a)-[:_BENCH_REL]->(b)"
            ))
            .unwrap();
    }
    let every_commit_elapsed = t0.elapsed();

    // --- Group mode, single flush_wal() at end-of-batch ---
    let td_after = tempfile::tempdir().unwrap();
    let db_after =
        ObrainDB::open_substrate_with_mode(td_after.path().join("kb"), SyncMode::Group).unwrap();
    let session_after = db_after.session();
    for i in 0..200 {
        let _ = session_after.execute_cypher(&format!("CREATE (:_BenchSrc {{id: {i}}})"));
        let _ = session_after.execute_cypher(&format!("CREATE (:_BenchDst {{id: {i}}})"));
    }
    let t0 = Instant::now();
    for i in 0..200 {
        session_after
            .execute_cypher(&format!(
                "MATCH (a:_BenchSrc {{id: {i}}}), (b:_BenchDst {{id: {i}}}) \
                 CREATE (a)-[:_BENCH_REL]->(b)"
            ))
            .unwrap();
    }
    db_after.flush_wal().expect("flush_wal");
    let group_elapsed = t0.elapsed();

    println!("  EveryCommit (default) : {every_commit_elapsed:?} for 200 relationships");
    println!("  Group + flush_wal()   : {group_elapsed:?} for 200 relationships");

    // Regression gate only: Group must not be SLOWER than EveryCommit (a
    // generous 1.5× ceiling absorbs machine noise on a workload where the
    // real dominant cost — Cypher parse+plan — is identical on both sides).
    // Do NOT assert a speedup multiplier here: measured on real PO the gain
    // is ~1.1×, not the originally hypothesized 10× (see module doc).
    assert!(
        group_elapsed <= every_commit_elapsed.mul_f64(1.5),
        "Group mode got SLOWER than EveryCommit ({group_elapsed:?} vs {every_commit_elapsed:?}) \
         — regression in the WAL fsync path"
    );
}
