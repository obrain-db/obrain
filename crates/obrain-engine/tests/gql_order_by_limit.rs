//! T13 regression gate — GQL `ORDER BY … SKIP … LIMIT` must sort the FULL
//! result set before paginating.
//!
//! Bug history (gotcha 9ccf0d95): the GQL translator applied wrap_skip/
//! wrap_limit BEFORE wrap_sort, producing `Sort(Limit(…))` plans: the
//! limit truncated the input to K arbitrary rows which were then sorted.
//! `ORDER BY x ASC LIMIT 1` and `ORDER BY x DESC LIMIT 1` returned the
//! SAME row (the first scanned match) — proven on the real PO base
//! 2026-07-02. The Cypher translator was never affected.
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

#[test]
fn gql_order_by_limit_sorts_before_limiting() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Insert out of order so scan order ≠ sorted order.
    for i in [5, 3, 9, 1, 7] {
        s.execute(&format!("INSERT (:N {{v: {i}}})")).unwrap();
    }

    let asc = s
        .execute("MATCH (n:N) RETURN n.v ORDER BY n.v ASC LIMIT 1")
        .unwrap();
    assert_eq!(
        asc.rows[0][0],
        Value::Int64(1),
        "ASC LIMIT 1 must return the global minimum, not the first scanned row"
    );

    let desc = s
        .execute("MATCH (n:N) RETURN n.v ORDER BY n.v DESC LIMIT 1")
        .unwrap();
    assert_eq!(
        desc.rows[0][0],
        Value::Int64(9),
        "DESC LIMIT 1 must return the global maximum — if this equals the \
         ASC result, LIMIT is being applied before ORDER BY again"
    );

    // SKIP is part of the same pagination contract: sort → skip → limit.
    let second = s
        .execute("MATCH (n:N) RETURN n.v ORDER BY n.v ASC SKIP 1 LIMIT 1")
        .unwrap();
    assert_eq!(
        second.rows[0][0],
        Value::Int64(3),
        "ORDER BY ASC SKIP 1 LIMIT 1 must return the 2nd smallest value"
    );

    // Full ordered page for good measure.
    let page = s
        .execute("MATCH (n:N) RETURN n.v ORDER BY n.v ASC LIMIT 3")
        .unwrap();
    let vals: Vec<&Value> = page.rows.iter().map(|r| &r[0]).collect();
    assert_eq!(
        vals,
        [&Value::Int64(1), &Value::Int64(3), &Value::Int64(5)],
        "top-3 ascending page must be globally sorted"
    );
}
