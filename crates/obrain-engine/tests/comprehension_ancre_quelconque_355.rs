//! Compréhension de motif corrélée par un nœud qui n'est pas le premier du
//! motif (obrain-db/obrain#355, PO `nbh_01` / `nbh_02`).
//!
//! `head([(op:P)-[:HP]->(m) | op.id])` avec `m` lié dehors était refusé : la
//! limite était d'implémentation (le sous-plan n'ancrait que sur le premier
//! nœud), pas de sémantique.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test comprehension_ancre_quelconque_355
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:P {id: 'proj'})-[:HP]->(f1:F {p: 'x'}), \
                    (b:P {id: 'autre'})-[:HP]->(f1), \
                    (a)-[:HP]->(:F {p: 'y'}), \
                    (f1)-[:NEXT]->(g:G {id: 'g1'})-[:NEXT]->(h:H {id: 'h1'}), \
                    (:F {p: 'seul'})",
        )
        .unwrap();
    db
}

fn col(db: &ObrainDB, q: &str) -> Vec<String> {
    let mut v: Vec<String> = db
        .execute_cypher(q)
        .unwrap_or_else(|e| panic!("{q}\n{e}"))
        .rows
        .iter()
        .map(|r| format!("{:?}", r[0]))
        .collect();
    v.sort();
    v
}

fn s(x: &str) -> String {
    format!("{:?}", Value::from(x))
}

/// Le cas de l'issue : ancre en dernière position.
#[test]
fn ancre_en_fin_de_motif() {
    let db = base();
    let r = col(
        &db,
        "MATCH (m:F {p: 'x'}) RETURN head([(op:P)-[:HP]->(m) | op.id]) AS owner",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0] == s("proj") || r[0] == s("autre"), "{r:?}");
}

/// Toutes les lignes sont rendues, chacune avec SA liste (pas le motif de
/// toutes les lignes).
#[test]
fn liste_par_ligne() {
    let db = base();
    let r = col(
        &db,
        "MATCH (m:F) RETURN size([(op:P)-[:HP]->(m) | op.id]) AS n",
    );
    assert_eq!(r.len(), 3);
    let tailles: Vec<&str> = r.iter().map(String::as_str).collect();
    let attendu = |n: i64| format!("{:?}", Value::from(n));
    let mut a = [attendu(0), attendu(1), attendu(2)];
    a.sort();
    assert_eq!(tailles, a.iter().map(String::as_str).collect::<Vec<_>>());
}

/// Direction inversée : `(m)<-[:HP]-(op)` est le premier-noeud, `(op)<-[:HP]-(m)` non.
#[test]
fn direction_inversee() {
    let db = base();
    let r = col(
        &db,
        "MATCH (m:P {id: 'proj'}) RETURN size([(f:F)<-[:HP]-(m) | f.p]) AS n",
    );
    assert_eq!(r, vec![format!("{:?}", Value::from(2_i64))]);
    let r = col(
        &db,
        "MATCH (f:F {p: 'x'}) RETURN [(f)<-[:HP]-(o:P) | o.id] AS owners",
    );
    assert_eq!(r.len(), 1);
}

/// Ancre au milieu d'un motif à trois nœuds, puis en fin.
#[test]
fn trois_noeuds_milieu_et_fin() {
    let db = base();
    let r = col(
        &db,
        "MATCH (g:G) RETURN [(f:F)-[:NEXT]->(g)-[:NEXT]->(h:H) | f.p + h.id] AS x",
    );
    assert_eq!(r.len(), 1);
    assert!(r[0].contains("xh1"), "{r:?}");
    let r = col(
        &db,
        "MATCH (h:H) RETURN [(f:F)-[:NEXT]->(g:G)-[:NEXT]->(h) | f.p + g.id] AS x",
    );
    assert!(r[0].contains("xg1"), "{r:?}");
}

/// Les contraintes (label) du nœud avant l'ancre sont respectées.
#[test]
fn label_avant_ancre_respecte() {
    let db = base();
    let r = col(
        &db,
        "MATCH (m:F {p: 'x'}) RETURN size([(op:G)-[:HP]->(m) | op.id]) AS n",
    );
    assert_eq!(r, vec![format!("{:?}", Value::from(0_i64))]);
}

/// Dans un WHERE.
#[test]
fn dans_un_where() {
    let db = base();
    let r = col(
        &db,
        "MATCH (m:F) WHERE 'proj' IN [(op:P)-[:HP]->(m) | op.id] RETURN m.p AS p",
    );
    assert_eq!(r, vec![s("x"), s("y")]);
}

/// Non-régression : le premier nœud lié marche toujours.
#[test]
fn premier_noeud_inchange() {
    let db = base();
    let r = col(
        &db,
        "MATCH (m:P {id: 'proj'}) RETURN size([(m)-[:HP]->(f:F) | f.p]) AS n",
    );
    assert_eq!(r, vec![format!("{:?}", Value::from(2_i64))]);
}
