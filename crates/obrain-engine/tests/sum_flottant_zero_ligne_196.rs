//! `sum()` de FLOTTANTS sur zéro ligne rend l'entier 0, comme Neo4j
//! (obrain-db/obrain#196, D4 — réserve 2 de la PR #219).
//!
//! Neo4j initialise l'accumulateur de `sum` à `Values.ZERO_INT`
//! (`NumericOrDurationAggregationExpression.scala`, branche 4.4 :
//! `protected var sumNumber: NumberValue = Values.ZERO_INT`) et `result`
//! le rend tel quel quand rien n'a été ajouté : le type du résultat ne
//! dépend que des valeurs vues, jamais du type déclaré de la propriété. La
//! base de connaissances Neo4j (« Understanding aggregations on zero rows »)
//! dit « `sum()` produces an output of 0 ». `valeur_sur_ensemble_vide` rend
//! donc `Int64(0)` à juste titre, flottants compris.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test sum_flottant_zero_ligne_196
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:N {w: 1.5}), (:N {w: 2.5})")
        .unwrap();
    db
}

fn cypher(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

#[test]
fn sum_de_flottants_sur_zero_ligne_vaut_l_entier_zero() {
    let db = base();
    assert_eq!(
        cypher(&db, "MATCH (n:N) WHERE n.w > 100 RETURN sum(n.w) AS s"),
        vec![vec![Value::Int64(0)]]
    );
    assert_eq!(
        cypher(
            &db,
            "MATCH (n:N) WHERE n.w > 100 RETURN sum(toFloat(n.w)) AS s"
        ),
        vec![vec![Value::Int64(0)]]
    );
    // avg reste NULL (Neo4j aussi).
    assert_eq!(
        cypher(&db, "MATCH (n:N) WHERE n.w > 100 RETURN avg(n.w) AS a"),
        vec![vec![Value::Null]]
    );
}

/// Témoin : dès qu'un flottant est vu, la somme est flottante.
#[test]
fn sum_de_flottants_non_vide_reste_flottante() {
    let db = base();
    assert_eq!(
        cypher(&db, "MATCH (n:N) RETURN sum(n.w) AS s"),
        vec![vec![Value::Float64(4.0)]]
    );
}
