//! T17i regression gate — `LIMIT N` over a plain projection must not
//! materialise the whole label.
//!
//! Bug history : `plan_limit` carried a push-down fast-path guarded on
//! `Limit → NodeScan`. The GQL translator applies `wrap_return` BEFORE
//! `wrap_skip`/`wrap_limit`, so the shape it actually emits is
//! `Limit → Return → NodeScan` — the guard never matched and the
//! push-down was dead code for every user query. Measured on the live
//! `agent` base 2026-08-06 : `MATCH (n:SupervisorObjective) RETURN n.id
//! LIMIT 5` took 447 ms on 31 357 nodes, while `count(n)` on the same
//! label took 3 ms.
//!
//! These tests pin the SEMANTICS the push-down must preserve. The
//! push-down is only legal under a row-preserving projection, so the
//! DISTINCT case below is the one that breaks if the guard is loosened
//! carelessly.
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn as_i64(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        other => panic!("expected Int64, got {other:?}"),
    }
}

#[test]
fn limit_under_plain_projection_returns_exactly_n_rows() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..50 {
        s.execute(&format!("INSERT (:P {{v: {i}}})")).unwrap();
    }

    let r = s.execute("MATCH (n:P) RETURN n.v LIMIT 5").unwrap();
    assert_eq!(r.rows.len(), 5, "LIMIT 5 must yield exactly 5 rows");

    // Every returned row must be a real stored value, not a placeholder
    // produced by a truncated scan.
    for row in &r.rows {
        let v = as_i64(&row[0]);
        assert!((0..50).contains(&v), "unexpected value {v} in LIMIT result");
    }
}

#[test]
fn limit_larger_than_cardinality_returns_all_rows() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..7 {
        s.execute(&format!("INSERT (:Q {{v: {i}}})")).unwrap();
    }

    let r = s.execute("MATCH (n:Q) RETURN n.v LIMIT 100").unwrap();
    assert_eq!(
        r.rows.len(),
        7,
        "a LIMIT above the label cardinality must not lose rows"
    );
}

#[test]
fn limit_with_distinct_still_yields_n_distinct_values() {
    // The push-down MUST NOT fire here : DISTINCT collapses rows, so
    // capping the scan at N can starve the projection and return fewer
    // than N distinct values.
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // 40 nodes, only 4 distinct values, duplicates clustered at the front
    // of the scan so a naive cap would return a single value.
    for i in 0..40 {
        let v = i / 10;
        s.execute(&format!("INSERT (:R {{v: {v}}})")).unwrap();
    }

    let r = s
        .execute("MATCH (n:R) RETURN DISTINCT n.v LIMIT 4")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        4,
        "DISTINCT … LIMIT 4 must still find 4 distinct values, not 4 scanned rows"
    );
    let mut vals: Vec<i64> = r.rows.iter().map(|row| as_i64(&row[0])).collect();
    vals.sort_unstable();
    vals.dedup();
    assert_eq!(vals.len(), 4, "returned values must be pairwise distinct");
}

#[test]
fn limit_preserves_multi_item_projection() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..20 {
        s.execute(&format!("INSERT (:S {{v: {i}, w: {}}})", i * 2))
            .unwrap();
    }

    let r = s
        .execute("MATCH (n:S) RETURN n.v AS v, n.w AS w LIMIT 3")
        .unwrap();
    assert_eq!(r.rows.len(), 3);
    for row in &r.rows {
        assert_eq!(
            as_i64(&row[1]),
            as_i64(&row[0]) * 2,
            "columns must stay aligned on the same node after the push-down"
        );
    }
}

#[test]
fn limit_after_filter_is_not_pushed_past_the_filter() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Only the last 3 nodes match the predicate : a cap applied before
    // the filter would return nothing.
    for i in 0..30 {
        s.execute(&format!("INSERT (:T {{v: {i}}})")).unwrap();
    }

    let r = s
        .execute("MATCH (n:T) WHERE n.v > 26 RETURN n.v LIMIT 3")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        3,
        "the filter must run before the cap, otherwise late-matching rows vanish"
    );
    for row in &r.rows {
        assert!(as_i64(&row[0]) > 26);
    }
}

// ── T17i : sémantique de `IN` après suppression de la rematérialisation ──
//
// `eval_in_operator` construisait un `Value::List` neuf à chaque ligne du
// scan (40 chaînes clonées × 31 357 nœuds sur le chemin des conversations).
// Le court-circuit direct sur la liste doit préserver exactement la
// logique trivaluée.

#[test]
fn in_list_matches_and_excludes() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..30 {
        s.execute(&format!("INSERT (:U {{k: \"k{i}\"}})")).unwrap();
    }

    let r = s
        .execute("MATCH (n:U) WHERE n.k IN [\"k3\", \"k17\", \"k29\"] RETURN n.k AS k")
        .unwrap();
    let mut got: Vec<String> = r
        .rows
        .iter()
        .map(|row| match &row[0] {
            Value::String(s) => s.to_string(),
            other => panic!("expected String, got {other:?}"),
        })
        .collect();
    got.sort();
    assert_eq!(got, vec!["k17".to_string(), "k29".into(), "k3".into()]);
}

#[test]
fn in_empty_list_matches_nothing() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..5 {
        s.execute(&format!("INSERT (:V {{k: {i}}})")).unwrap();
    }
    let r = s
        .execute("MATCH (n:V) WHERE n.k IN [] RETURN n.k AS k")
        .unwrap();
    assert_eq!(r.rows.len(), 0, "IN [] must exclude every row");
}

#[test]
fn in_list_with_null_does_not_produce_false_positives() {
    // Three-valued logic: a non-matching value against a list containing
    // NULL is UNKNOWN, not TRUE — the row must not be returned.
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:W {k: 1})").unwrap();
    s.execute("INSERT (:W {k: 2})").unwrap();

    let r = s
        .execute("MATCH (n:W) WHERE n.k IN [1, null] RETURN n.k AS k")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "only the exact match may pass; UNKNOWN must not be treated as TRUE"
    );
    assert_eq!(as_i64(&r.rows[0][0]), 1);
}

#[test]
fn in_list_absent_value_returns_no_row() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..10 {
        s.execute(&format!("INSERT (:X {{k: {i}}})")).unwrap();
    }
    let r = s
        .execute("MATCH (n:X) WHERE n.k IN [99, 100] RETURN n.k AS k")
        .unwrap();
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn in_list_does_not_leak_rolled_back_nodes() {
    // ⚠ CE TEST ÉCHOUE AUJOURD'HUI — et il échouait DÉJÀ avant ce travail.
    //
    // Vérifié le 2026-08-06 en désactivant `try_plan_filter_with_in_list` :
    // la fuite est identique sur le chemin générique. Ce n'est donc pas une
    // régression du routage label-first, c'est le même défaut MVCC que
    // `rollback_hides_nodes_from_equality_pushdown` et
    // `rollback_hides_nodes_from_range_pushdown` dans filter_pushdown.rs,
    // rouges eux aussi.
    //
    // Il est laissé ROUGE volontairement : un nœud annulé qui reste lisible
    // est un défaut de correction, pas une préférence. `#[ignore]` le
    // rendrait invisible. À lever avec le correctif de visibilité.
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    s.execute("INSERT (:Y {k: \"kept\"})").unwrap();

    s.begin_transaction().unwrap();
    s.execute("INSERT (:Y {k: \"ghost\"})").unwrap();
    s.rollback().unwrap();

    let r = s
        .execute("MATCH (n:Y) WHERE n.k IN [\"ghost\"] RETURN n.k AS k")
        .unwrap();
    assert!(
        r.rows.is_empty(),
        "rolled-back node leaked through the IN path"
    );

    let r = s
        .execute("MATCH (n:Y) WHERE n.k IN [\"kept\"] RETURN n.k AS k")
        .unwrap();
    assert_eq!(r.rows.len(), 1, "committed node must stay visible");
}

#[test]
fn in_list_in_compound_predicate_stays_correct() {
    // Compound predicate → the label-first IN path bails out. The generic
    // evaluator must still give the right answer.
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..12 {
        s.execute(&format!("INSERT (:Z {{k: {i}, grp: {}}})", i % 3))
            .unwrap();
    }
    let r = s
        .execute("MATCH (n:Z) WHERE n.k IN [1, 4, 7, 8] AND n.grp = 1 RETURN n.k AS k")
        .unwrap();
    let mut got: Vec<i64> = r.rows.iter().map(|row| as_i64(&row[0])).collect();
    got.sort_unstable();
    assert_eq!(got, vec![1, 4, 7], "8 has grp=2 and must be excluded");
}

#[test]
fn in_list_coerces_int_and_float_like_equality() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:AA {k: 3})").unwrap();
    s.execute("INSERT (:AA {k: 5})").unwrap();

    let r = s
        .execute("MATCH (n:AA) WHERE n.k IN [3.0] RETURN n.k AS k")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "IN must coerce Int64/Float64 the same way equality does"
    );
}

#[test]
fn in_list_with_limit_returns_capped_matches() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..40 {
        s.execute(&format!("INSERT (:AB {{k: {i}}})")).unwrap();
    }
    let r = s
        .execute("MATCH (n:AB) WHERE n.k IN [1, 2, 3, 4, 5] RETURN n.k AS k LIMIT 3")
        .unwrap();
    assert_eq!(r.rows.len(), 3);
    for row in &r.rows {
        assert!((1..=5).contains(&as_i64(&row[0])));
    }
}
