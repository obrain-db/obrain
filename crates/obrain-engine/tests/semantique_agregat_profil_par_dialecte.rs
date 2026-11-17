//! Le profil d'agrégat voyage de la requête à l'opérateur — obrain#196, D4.
//!
//! Chaque traducteur renseigne SON profil dans `AggregateOp::semantics` ; un
//! profil global unique reproduirait le défaut d'origine dans l'autre sens.
//! Ces tests lisent le plan logique émis, sans base : ils échouent si un
//! traducteur oublie de poser son profil (ou en pose un autre).

#[cfg(any(
    feature = "cypher",
    feature = "sparql",
    feature = "gremlin",
    feature = "sql-pgq"
))]
use obrain_core::execution::operators::AggregateSemantics;
use obrain_engine::query::plan::{AggregateOp, LogicalOperator};

fn premier_agregat(op: &LogicalOperator) -> Option<&AggregateOp> {
    match op {
        LogicalOperator::Aggregate(a) => Some(a),
        LogicalOperator::Return(r) => premier_agregat(&r.input),
        LogicalOperator::Project(p) => premier_agregat(&p.input),
        LogicalOperator::Filter(f) => premier_agregat(&f.input),
        _ => None,
    }
}

#[test]
fn gql_pose_le_profil_gql() {
    let plan = obrain_engine::query::translators::gql::translate("MATCH (t:T) RETURN sum(t.x)")
        .expect("GQL");
    let agg = premier_agregat(&plan.root).expect("Aggregate");
    assert_eq!(
        agg.semantics,
        obrain_core::execution::operators::AggregateSemantics::Gql
    );
}

#[cfg(feature = "cypher")]
#[test]
fn cypher_pose_le_profil_open_cypher() {
    let plan = obrain_engine::query::translators::cypher::translate("MATCH (t:T) RETURN sum(t.x)")
        .expect("Cypher");
    let agg = premier_agregat(&plan.root).expect("Aggregate");
    assert_eq!(agg.semantics, AggregateSemantics::OpenCypher);
}

#[cfg(feature = "sparql")]
#[test]
fn sparql_pose_le_profil_sparql() {
    let plan = obrain_engine::query::translators::sparql::translate(
        "SELECT (SUM(?x) AS ?s) WHERE { ?a ?b ?x }",
    )
    .expect("SPARQL");
    let agg = premier_agregat(&plan.root).expect("Aggregate");
    assert_eq!(agg.semantics, AggregateSemantics::Sparql);
}

#[cfg(feature = "gremlin")]
#[test]
fn gremlin_pose_le_profil_tinkerpop() {
    let plan = obrain_engine::query::translators::gremlin::translate("g.V().values('age').sum()")
        .expect("Gremlin");
    let agg = premier_agregat(&plan.root).expect("Aggregate");
    assert_eq!(agg.semantics, AggregateSemantics::TinkerPop);
}

#[cfg(feature = "sql-pgq")]
#[test]
fn sql_pgq_pose_le_profil_gql() {
    let plan = obrain_engine::query::translators::sql_pgq::translate(
        "SELECT SUM(g.score) AS total FROM GRAPH_TABLE (
            MATCH (n:Person) COLUMNS (n.score AS score)
        ) AS g",
    )
    .expect("SQL/PGQ");
    let agg = premier_agregat(&plan.root).expect("Aggregate");
    assert_eq!(agg.semantics, AggregateSemantics::Gql);
}
