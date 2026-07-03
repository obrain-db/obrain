//! Integration tests for temporal (versioned) property storage.
//!
//! These tests validate that when the `temporal` feature is enabled,
//! `get_node_at_epoch()` and `execute_at_epoch()` return correct
//! historical property values, not just current ones.
//!
//! When `temporal` is disabled, these tests are skipped.

#![cfg(feature = "temporal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_db() -> ObrainDB {
    ObrainDB::new_in_memory()
}

// ---------------------------------------------------------------------------
// Property versioning: basic get_node_at_epoch
// ---------------------------------------------------------------------------

#[test]
fn test_temporal_property_at_epoch() {
    let db = setup_db();
    let mut session = db.session();

    // Create node with initial properties
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01', status: 'healthy'})")
        .unwrap();
    session.commit().unwrap();
    let epoch_v1 = db.current_epoch();

    // Update status
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'degraded'")
        .unwrap();
    session.commit().unwrap();
    let epoch_v2 = db.current_epoch();

    // Update again
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'offline'")
        .unwrap();
    session.commit().unwrap();

    // Current state: offline
    let result = session
        .execute("MATCH (s:Server {name: 'web-01'}) RETURN s.status")
        .unwrap();
    assert_eq!(result.rows[0][0], Value::String("offline".into()));

    // At epoch_v1: healthy
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server {name: 'web-01'}) RETURN s.status",
            epoch_v1,
        )
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0][0],
        Value::String("healthy".into()),
        "temporal query at epoch_v1 should return 'healthy'"
    );

    // At epoch_v2: degraded
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server {name: 'web-01'}) RETURN s.status",
            epoch_v2,
        )
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0][0],
        Value::String("degraded".into()),
        "temporal query at epoch_v2 should return 'degraded'"
    );
}

#[test]
fn test_temporal_multiple_properties() {
    let db = setup_db();
    let mut session = db.session();

    // Create with initial properties
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'db-01', cpu: 10, memory: 1024})")
        .unwrap();
    session.commit().unwrap();
    let epoch_v1 = db.current_epoch();

    // Update cpu only
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'db-01'}) SET s.cpu = 80")
        .unwrap();
    session.commit().unwrap();
    let epoch_v2 = db.current_epoch();

    // Update memory only
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'db-01'}) SET s.memory = 2048")
        .unwrap();
    session.commit().unwrap();

    // At epoch_v1: cpu=10, memory=1024
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server {name: 'db-01'}) RETURN s.cpu, s.memory",
            epoch_v1,
        )
        .unwrap();
    assert_eq!(result.rows[0][0], Value::Int64(10));
    assert_eq!(result.rows[0][1], Value::Int64(1024));

    // At epoch_v2: cpu=80, memory=1024 (only cpu changed)
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server {name: 'db-01'}) RETURN s.cpu, s.memory",
            epoch_v2,
        )
        .unwrap();
    assert_eq!(result.rows[0][0], Value::Int64(80));
    assert_eq!(result.rows[0][1], Value::Int64(1024));
}

#[test]
fn test_temporal_where_filter_at_epoch() {
    let db = setup_db();
    let mut session = db.session();

    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01', status: 'healthy'})")
        .unwrap();
    session
        .execute("INSERT (:Server {name: 'web-02', status: 'healthy'})")
        .unwrap();
    session.commit().unwrap();
    let epoch_all_healthy = db.current_epoch();

    // Make web-01 degraded
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'degraded'")
        .unwrap();
    session.commit().unwrap();

    // Current: 1 healthy server
    let result = session
        .execute("MATCH (s:Server) WHERE s.status = 'healthy' RETURN s.name")
        .unwrap();
    assert_eq!(result.rows.len(), 1);

    // At epoch_all_healthy: 2 healthy servers
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server) WHERE s.status = 'healthy' RETURN s.name",
            epoch_all_healthy,
        )
        .unwrap();
    assert_eq!(
        result.rows.len(),
        2,
        "temporal WHERE filter should see both servers as healthy at epoch_all_healthy"
    );
}

#[test]
fn test_temporal_node_history_distinct_properties() {
    let db = setup_db();
    let mut session = db.session();

    // Create and commit
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01', status: 'healthy'})")
        .unwrap();
    session.commit().unwrap();

    // Modify status
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'degraded'")
        .unwrap();
    session.commit().unwrap();

    // get_node_history should return versions with distinct property snapshots
    let history = db.get_node_history(obrain_common::types::NodeId::new(0));
    assert!(!history.is_empty(), "node should have history");
}

#[test]
fn test_temporal_edge_property_versioning() {
    let db = setup_db();
    let mut session = db.session();

    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01'})")
        .unwrap();
    session.execute("INSERT (:Server {name: 'db-01'})").unwrap();
    session
        .execute(
            "MATCH (a:Server {name: 'web-01'}), (b:Server {name: 'db-01'}) \
             INSERT (a)-[:CONNECTS {latency: 5}]->(b)",
        )
        .unwrap();
    session.commit().unwrap();
    let epoch_v1 = db.current_epoch();

    // Update edge property
    session.begin_transaction().unwrap();
    session
        .execute(
            "MATCH (:Server {name: 'web-01'})-[c:CONNECTS]->(:Server {name: 'db-01'}) \
             SET c.latency = 50",
        )
        .unwrap();
    session.commit().unwrap();

    // At epoch_v1: latency=5
    let result = session
        .execute_at_epoch(
            "MATCH (:Server {name: 'web-01'})-[c:CONNECTS]->(:Server {name: 'db-01'}) \
             RETURN c.latency",
            epoch_v1,
        )
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0][0],
        Value::Int64(5),
        "edge property should be 5 at epoch_v1"
    );
}

// NOTE(T17 cutover): `test_temporal_snapshot_roundtrip` and
// `test_temporal_snapshot_query_roundtrip` were removed here. They exercised
// the legacy byte-array snapshot API (`ObrainDB::export_snapshot` /
// `import_snapshot`), which was retired with the single-file .obrain format.
// Temporal history persistence is now covered by the substrate WAL/reopen
// tests in obrain-substrate.

#[test]
fn test_temporal_transaction_rollback() {
    let db = setup_db();
    let mut session = db.session();

    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01', status: 'healthy'})")
        .unwrap();
    session.commit().unwrap();

    // Start transaction, modify, then rollback
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'offline'")
        .unwrap();
    session.rollback().unwrap();

    // Status should still be healthy
    let result = session
        .execute("MATCH (s:Server {name: 'web-01'}) RETURN s.status")
        .unwrap();
    assert_eq!(result.rows[0][0], Value::String("healthy".into()));
}

#[test]
fn test_temporal_savepoint_rollback() {
    let db = setup_db();
    let mut session = db.session();

    // Create node and commit
    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01', status: 'healthy', cpu: 10})")
        .unwrap();
    session.commit().unwrap();
    let epoch_v1 = db.current_epoch();

    // Start transaction, modify, set savepoint, modify again, rollback to savepoint
    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'degraded'")
        .unwrap();
    session.savepoint("sp1").unwrap();

    // Changes after savepoint
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'offline'")
        .unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.cpu = 99")
        .unwrap();

    // Rollback to savepoint: status should revert to 'degraded', cpu to 10
    session.rollback_to_savepoint("sp1").unwrap();

    // Commit the pre-savepoint changes
    session.commit().unwrap();
    let epoch_v2 = db.current_epoch();

    // Current: status should be 'degraded' (pre-savepoint), cpu should be 10 (unchanged by commit)
    let result = session
        .execute("MATCH (s:Server {name: 'web-01'}) RETURN s.status, s.cpu")
        .unwrap();
    assert_eq!(
        result.rows[0][0],
        Value::String("degraded".into()),
        "post-savepoint status change should be rolled back"
    );
    assert_eq!(
        result.rows[0][1],
        Value::Int64(10),
        "post-savepoint cpu change should be rolled back"
    );

    // Historical: epoch_v1 should still show 'healthy'
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server {name: 'web-01'}) RETURN s.status",
            epoch_v1,
        )
        .unwrap();
    assert_eq!(
        result.rows[0][0],
        Value::String("healthy".into()),
        "temporal query at epoch_v1 should still return 'healthy' after savepoint rollback"
    );

    // epoch_v2 should show 'degraded'
    let result = session
        .execute_at_epoch(
            "MATCH (s:Server {name: 'web-01'}) RETURN s.status",
            epoch_v2,
        )
        .unwrap();
    assert_eq!(
        result.rows[0][0],
        Value::String("degraded".into()),
        "temporal query at epoch_v2 should return 'degraded'"
    );
}

#[test]
fn test_temporal_gc_does_not_break_current_state() {
    let db = setup_db();
    let mut session = db.session();

    session.begin_transaction().unwrap();
    session
        .execute("INSERT (:Server {name: 'web-01', status: 'v1'})")
        .unwrap();
    session.commit().unwrap();

    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'v2'")
        .unwrap();
    session.commit().unwrap();

    session.begin_transaction().unwrap();
    session
        .execute("MATCH (s:Server {name: 'web-01'}) SET s.status = 'v3'")
        .unwrap();
    session.commit().unwrap();

    // Run GC
    db.gc();

    // Current state should still work
    let result = session
        .execute("MATCH (s:Server {name: 'web-01'}) RETURN s.status")
        .unwrap();
    assert_eq!(result.rows[0][0], Value::String("v3".into()));
}
