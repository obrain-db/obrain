//! T12 property pushdown — in-process bench isolating the RETURN
//! property-projection cost from CLI process/JSON overhead.
//!
//! Root cause (note 96ae814c / d4eb523f): `ProjectOperator::PropertyAccess`
//! gated its O(1) `get_node_property` fast path on `!has_mvcc`, but
//! sessions always set `viewing_epoch = Some(_)` — so every session query
//! took the `get_node_at_epoch` branch, which on SubstrateStore is a full
//! `get_node()` bulk hydrate (all props, all 4 zones) just to read one key.
//!
//! This bench measures `MATCH (f:File) RETURN f.path` (full scan, no LIMIT
//! pushdown interference) in-process via `session.execute()`, mirroring
//! the real query shape behind list_project_symbols/call_edges.
#![cfg(feature = "cypher")]

use obrain_bases_banc::{Base, base_lecture};
use web_time::Instant;

use obrain_engine::ObrainDB;

#[test]
#[ignore = "bench on prod base — read-only"]
fn bench_property_projection_full_scan() {
    let Some(po_path) = base_lecture(Base::Po) else {
        return;
    };
    let db = ObrainDB::open_read_only(&po_path).expect("open PO");
    let session = db.session();

    // Warm-up (page cache) — not timed.
    let warm = session
        .execute("MATCH (f:File) RETURN f.path")
        .expect("warmup query");
    let n = warm.rows.len();
    println!("  File rows = {n}");

    let runs = 3;
    let mut total = std::time::Duration::ZERO;
    for _ in 0..runs {
        let t0 = Instant::now();
        let r = session
            .execute("MATCH (f:File) RETURN f.path")
            .expect("query");
        total += t0.elapsed();
        assert_eq!(r.rows.len(), n, "row count should be stable across runs");
    }
    let avg = total / runs;
    let per_row_ns = avg.as_nanos() as f64 / n as f64;
    println!("  MATCH (f:File) RETURN f.path : {avg:?}/run over {n} rows ({per_row_ns:.0} ns/row)");

    // Regression gate: property-only projection over File should be well
    // under the ~247 µs/row full get_node() cost measured pre-T12 (t17k
    // bench). Generous ceiling — this is a floor-raising gate, not a tight
    // budget (exact number depends on machine/cache state).
    assert!(
        per_row_ns < 50_000.0,
        "property projection regressed: {per_row_ns:.0} ns/row (expected << 247 000 ns/row full hydrate)"
    );
}
