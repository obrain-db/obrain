//! Regression gate (ex-repro): Cypher WHERE on node properties works on substrate.
//!
//! History: pre-T17k, `get_node` hydrated only from the runtime DashMap
//! (empty on prod bases post-T17g), so WHERE on any real property returned
//! 0 rows. T17k landed `read_all_properties_for_node/edge` and the bug is
//! fixed — validated 2026-07-02 on the real PO base (WHERE f.path CONTAINS
//! 'rs' → 12 715 rows).
//!
//! The original repro had a FALSE PREMISE: it queried `f.name`, a property
//! that does not exist on migrated PO File nodes (they only carry `path`),
//! so it kept "reproducing" an already-fixed bug. Rewritten as a positive
//! gate on `f.path`.
//!
//! GOTCHA (note 5906c9b3): the first File-labelled slots in ~/.obrain/db/po
//! are benchmark artifacts (props id/value/_bench_*) with no `path` — we
//! scan past them to find a real File.
#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
#[ignore = "gate on prod base — read-only"]
fn po_where_property_filter_gate() {
    let home = std::env::var("HOME").unwrap_or_default();
    let po_path = std::path::PathBuf::from(&home).join(".obrain/db/po");
    if !po_path.exists() {
        eprintln!("⏭  PO not present");
        return;
    }
    let db = ObrainDB::open(&po_path).expect("open PO");
    let store = db.store();
    let session = db.session();

    // Find a REAL File (one that has a `path`), skipping bench artifacts.
    let files = store.nodes_by_label("File");
    assert!(
        files.len() > 1_000,
        "expected thousands of File nodes on PO, got {}",
        files.len()
    );
    let path_key = obrain_common::PropertyKey::from("path");
    let mut probe: Option<(obrain_common::NodeId, String)> = None;
    for nid in files.iter().take(5_000) {
        if let Some(obrain_common::types::Value::String(s)) =
            store.get_node_property(*nid, &path_key)
        {
            probe = Some((*nid, s.to_string()));
            break;
        }
    }
    let (real_file, real_path) =
        probe.expect("no File with a `path` in the first 5000 — base corrupted?");
    println!("\n=== PO File property gate ===");
    println!("  real File = {} path = {}", real_file.0, real_path);

    // Gate 1 — bulk hydration: get_node returns non-empty properties
    // consistent with the per-key path.
    let node = store.get_node(real_file).expect("get_node");
    assert!(
        !node.properties.is_empty(),
        "get_node().properties empty — bulk hydration regressed (T17k)"
    );
    assert_eq!(
        node.properties.get(&path_key).and_then(|v| match v {
            obrain_common::types::Value::String(s) => Some(s.to_string()),
            _ => None,
        }),
        Some(real_path.clone()),
        "bulk get_node disagrees with get_node_property on `path` (LWW order?)"
    );

    // Gate 2 — Cypher WHERE equality on a real property returns the row.
    let q = format!(
        "MATCH (f:File) WHERE f.path = '{}' RETURN f.path LIMIT 5",
        real_path.replace('\'', "\\'")
    );
    let r = session.execute_cypher(&q).expect("cypher eq");
    println!("  WHERE f.path = <probe> → {} rows", r.rows.len());
    assert!(
        !r.rows.is_empty(),
        "WHERE equality on f.path returned 0 rows — property filter regressed (T17k)"
    );

    // Gate 3 — Cypher WHERE CONTAINS fans out over the base.
    let q = "MATCH (f:File) WHERE f.path CONTAINS 'rs' RETURN f.path LIMIT 50";
    let r = session.execute_cypher(q).expect("cypher contains");
    println!(
        "  WHERE f.path CONTAINS 'rs' → {} rows (LIMIT 50)",
        r.rows.len()
    );
    assert_eq!(
        r.rows.len(),
        50,
        "expected LIMIT 50 fully filled (12 715 matching Files measured 2026-07-02)"
    );
}
