//! Chemin factorisé des agrégats : ensemble vide et profil de dialecte
//! (obrain-db/obrain#196, D4 — réserve 1 de la PR #219).
//!
//! Le planificateur prend `FactorizedAggregateOperator` pour un agrégat
//! simple (`count`, `sum`, `avg`, `min`, `max` d'une variable nue), sans
//! GROUP BY, au-dessus d'une chaîne d'au moins deux expansions. Deux défauts
//! sur `main` :
//! - une expansion sans arête était OMISE : la chaîne rendait les lignes du
//!   niveau précédent (`count` = 1 sur un graphe à un seul arc, `sum` = le
//!   nœud intermédiaire) ;
//! - sur un graphe sans aucun nœud, `sum` valait NULL quel que soit le profil,
//!   alors que le chemin général consulte `valeur_sur_ensemble_vide`.
//!
//! Les portes Cypher et GQL n'atteignent pas ce chemin (le filtre d'unicité
//! des relations s'intercale entre la chaîne et l'agrégat) ; la porte Gremlin
//! l'atteint (`g.V().out().out().sum()`, voir
//! `agregat_factorise_gremlin_196.rs`). Ici le plan logique est construit à la
//! main pour viser l'opérateur directement, quelle que soit la porte.
//!
//! ```bash
//! cargo test -p obrain-engine --test agregat_factorise_vide_196
//! ```

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_core::execution::operators::{AggregateSemantics, Operator};
use obrain_core::graph::GraphStoreMut;
use obrain_engine::query::plan::{
    AggregateExpr, AggregateFunction, AggregateOp, ExpandDirection, ExpandOp, LogicalExpression,
    LogicalOperator, LogicalPlan, NodeScanOp, PathMode,
};
use obrain_engine::query::planner::Planner;
use obrain_substrate::SubstrateStore;

fn agregat(function: AggregateFunction, variable: Option<&str>, alias: &str) -> AggregateExpr {
    AggregateExpr {
        function,
        expression: variable.map(|v| LogicalExpression::Variable(v.to_string())),
        expression2: None,
        distinct: false,
        alias: Some(alias.to_string()),
        percentile: None,
        separator: None,
    }
}

fn expansion(from: &str, to: &str, input: LogicalOperator) -> LogicalOperator {
    LogicalOperator::Expand(ExpandOp {
        from_variable: from.to_string(),
        to_variable: to.to_string(),
        edge_variable: None,
        direction: ExpandDirection::Outgoing,
        edge_types: vec![],
        min_hops: 1,
        max_hops: Some(1),
        input: Box::new(input),
        path_alias: None,
        path_mode: PathMode::Walk,
    })
}

/// `MATCH (a)-->(b)-->(c)` (ou `-->(d)` avec `sauts` = 3) puis
/// `RETURN count(*), sum(x), avg(x), min(x), max(x)` sur le dernier nœud.
fn plan(sauts: usize, semantics: AggregateSemantics) -> LogicalPlan {
    let noms = ["a", "b", "c", "d"];
    let mut op = LogicalOperator::NodeScan(NodeScanOp {
        variable: "a".to_string(),
        label: None,
        input: None,
        pushdown_hint: None,
    });
    for i in 0..sauts {
        op = expansion(noms[i], noms[i + 1], op);
    }
    let dernier = Some(noms[sauts]);
    LogicalPlan::new(LogicalOperator::Aggregate(AggregateOp {
        semantics,
        group_by: vec![],
        aggregates: vec![
            agregat(AggregateFunction::Count, None, "n"),
            agregat(AggregateFunction::Sum, dernier, "s"),
            agregat(AggregateFunction::Avg, dernier, "a"),
            agregat(AggregateFunction::Min, dernier, "mi"),
            agregat(AggregateFunction::Max, dernier, "ma"),
        ],
        input: Box::new(op),
        having: None,
    }))
}

/// Planifie, vérifie que c'est bien l'opérateur factorisé qui répond, et rend
/// l'unique ligne.
fn executer(store: Arc<SubstrateStore>, sauts: usize, semantics: AggregateSemantics) -> Vec<Value> {
    let planner = Planner::new(store as Arc<dyn GraphStoreMut>);
    let physique = planner.plan(&plan(sauts, semantics)).unwrap();
    assert_eq!(
        physique.columns(),
        &["n", "s", "a", "mi", "ma"],
        "colonnes de sortie"
    );
    let mut op: Box<dyn Operator> = physique.operator;
    assert_eq!(
        op.name(),
        "FactorizedAggregate",
        "le test doit viser le chemin factorisé, pas le chemin général"
    );
    let mut lignes = Vec::new();
    while let Some(chunk) = op.next().unwrap() {
        for ligne in 0..chunk.row_count() {
            lignes.push(
                (0..chunk.column_count())
                    .map(|c| {
                        chunk
                            .column(c)
                            .unwrap()
                            .get_value(ligne)
                            .unwrap_or(Value::Null)
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }
    assert_eq!(lignes.len(), 1, "un agrégat sans GROUP BY rend une ligne");
    lignes.remove(0)
}

/// Un seul arc `a -> b` : aucun chemin à deux sauts.
fn un_seul_arc() -> Arc<SubstrateStore> {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let a = store.create_node(&["N"]);
    let b = store.create_node(&["N"]);
    store.create_edge(a, b, "R");
    store
}

/// Deux arcs `a -> b -> c` : un chemin à deux sauts, aucun à trois.
fn deux_arcs() -> Arc<SubstrateStore> {
    let store = Arc::new(SubstrateStore::open_tempfile().unwrap());
    let a = store.create_node(&["N"]);
    let b = store.create_node(&["N"]);
    let c = store.create_node(&["N"]);
    store.create_edge(a, b, "R");
    store.create_edge(b, c, "R");
    store
}

const NULLS: [Value; 3] = [Value::Null, Value::Null, Value::Null];

#[test]
fn chaine_sans_chemin_complet_rend_zero_ligne_open_cypher() {
    let ligne = executer(un_seul_arc(), 2, AggregateSemantics::OpenCypher);
    assert_eq!(
        ligne[0],
        Value::Int64(0),
        "count(*) : aucun chemin à deux sauts"
    );
    assert_eq!(
        ligne[1],
        Value::Int64(0),
        "sum sur ensemble vide : 0 en openCypher"
    );
    assert_eq!(&ligne[2..], &NULLS, "avg, min, max : NULL");
}

#[test]
fn chaine_sans_chemin_complet_rend_zero_ligne_gql() {
    let ligne = executer(un_seul_arc(), 2, AggregateSemantics::Gql);
    assert_eq!(ligne[0], Value::Int64(0));
    assert_eq!(
        ligne[1],
        Value::Null,
        "SUM sur ensemble vide : NULL en SQL / ISO GQL"
    );
    assert_eq!(&ligne[2..], &NULLS);
}

#[test]
fn trois_sauts_sur_deux_arcs_rend_zero_ligne() {
    let ligne = executer(deux_arcs(), 3, AggregateSemantics::OpenCypher);
    assert_eq!(
        ligne[0],
        Value::Int64(0),
        "count(*) : aucun chemin à trois sauts"
    );
    assert_eq!(ligne[1], Value::Int64(0));
    assert_eq!(&ligne[2..], &NULLS);
}

#[test]
fn graphe_sans_noeud_suit_le_profil() {
    let vide = || Arc::new(SubstrateStore::open_tempfile().unwrap());
    let ligne = executer(vide(), 2, AggregateSemantics::OpenCypher);
    assert_eq!(ligne[0], Value::Int64(0));
    assert_eq!(
        ligne[1],
        Value::Int64(0),
        "sum : 0 en openCypher, même sans aucun nœud"
    );
    assert_eq!(&ligne[2..], &NULLS);

    let ligne = executer(vide(), 2, AggregateSemantics::TinkerPop);
    assert_eq!(ligne[1], Value::Int64(0), "sum : 0 en TinkerPop");

    // Témoin : GQL rendait déjà NULL ici.
    let ligne = executer(vide(), 2, AggregateSemantics::Gql);
    assert_eq!(ligne[0], Value::Int64(0));
    assert_eq!(ligne[1], Value::Null);
}

/// Témoin : une chaîne complète n'est pas touchée (un chemin `a -> b -> c`).
#[test]
fn chaine_complete_inchangee() {
    let ligne = executer(deux_arcs(), 2, AggregateSemantics::OpenCypher);
    assert_eq!(ligne[0], Value::Int64(1), "un chemin à deux sauts");
    assert_ne!(ligne[1], Value::Null, "sum porte une valeur");
    assert_ne!(ligne[2], Value::Null, "avg porte une valeur");
}
