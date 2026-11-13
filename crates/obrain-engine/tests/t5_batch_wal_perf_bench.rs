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
    let fsyncs_avant_ec = db_before.wal_fsyncs().expect("compteur fsync");
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
    let fsyncs_every_commit = db_before.wal_fsyncs().expect("compteur fsync") - fsyncs_avant_ec;

    // --- Group mode, single flush_wal() at end-of-batch ---
    let td_after = tempfile::tempdir().unwrap();
    let db_after =
        ObrainDB::open_substrate_with_mode(td_after.path().join("kb"), SyncMode::Group).unwrap();
    let session_after = db_after.session();
    for i in 0..200 {
        let _ = session_after.execute_cypher(&format!("CREATE (:_BenchSrc {{id: {i}}})"));
        let _ = session_after.execute_cypher(&format!("CREATE (:_BenchDst {{id: {i}}})"));
    }
    let fsyncs_avant_g = db_after.wal_fsyncs().expect("compteur fsync");
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
    let fsyncs_group = db_after.wal_fsyncs().expect("compteur fsync") - fsyncs_avant_g;

    println!(
        "  EveryCommit (default) : {every_commit_elapsed:?} for 200 relationships, {fsyncs_every_commit} fsync"
    );
    println!(
        "  Group + flush_wal()   : {group_elapsed:?} for 200 relationships, {fsyncs_group} fsync"
    );

    // Regression gate on the MECHANISM, not on wall-clock time. Until
    // 2026-11-13 this asserted `group_elapsed <= 1.5 × every_commit_elapsed`,
    // two durations measured one after the other: a load shift between the
    // two phases flipped the verdict (main CI red on 1e98a87e, 11.1 s vs
    // 1.08 s; 1 failure in 3 locally under load 33-49). What Group must keep
    // is what makes it cheaper: commits do NOT fsync, only `flush_wal()`
    // does. Counted, that is deterministic.
    assert!(
        fsyncs_every_commit >= 200,
        "EveryCommit must fsync every commit: {fsyncs_every_commit} fsync for 200 commits"
    );
    assert!(
        fsyncs_group >= 1,
        "flush_wal() must fsync: Group batch left non-durable (0 fsync)"
    );
    assert!(
        fsyncs_group * 10 <= fsyncs_every_commit,
        "Group mode fsyncs almost as often as EveryCommit ({fsyncs_group} vs \
         {fsyncs_every_commit}) — regression in the WAL fsync path"
    );
}
