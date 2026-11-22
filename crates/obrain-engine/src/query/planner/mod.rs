//! Converts logical plans into physical execution trees.
//!
//! The optimizer produces a logical plan (what data you want), but the planner
//! converts it to a physical plan (how to actually get it). This means choosing
//! hash joins vs nested loops, picking index scans vs full scans, etc.
//!
//! This module contains shared infrastructure used by both the LPG and RDF planners:
//! - [`PhysicalPlan`] - the output of planning
//! - Expression and operator conversion functions
//! - Reusable operator builders (in the common submodule)
//!
//! Model-specific planning lives in [`lpg`] and `rdf` (feature-gated).

pub(crate) mod common;
pub mod lpg;

#[cfg(feature = "rdf")]
pub mod rdf;

// Re-export the LPG planner as the default `Planner` for backwards compatibility.
pub use lpg::Planner;

use crate::query::plan::{
    AggregateFunction as LogicalAggregateFunction, BinaryOp, LogicalExpression, UnaryOp,
};
use obrain_common::types::LogicalType;
use obrain_common::utils::error::{Error, Result};
use obrain_core::execution::AdaptiveContext;
use obrain_core::execution::operators::{
    AggregateFunction as PhysicalAggregateFunction, BinaryFilterOp, FilterExpression, Operator,
    UnaryFilterOp,
};

/// A physical plan ready for execution.
pub struct PhysicalPlan {
    /// The root physical operator.
    pub operator: Box<dyn Operator>,
    /// Column names for the result.
    pub columns: Vec<String>,
    /// Adaptive execution context with cardinality estimates.
    ///
    /// When adaptive execution is enabled, this context contains estimated
    /// cardinalities at various checkpoints in the plan. During execution,
    /// actual row counts are recorded and compared against estimates.
    pub adaptive_context: Option<AdaptiveContext>,
}

impl PhysicalPlan {
    /// Returns the column names.
    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Consumes the plan and returns the operator.
    pub fn into_operator(self) -> Box<dyn Operator> {
        self.operator
    }

    /// Returns the adaptive context, if adaptive execution is enabled.
    #[must_use]
    pub fn adaptive_context(&self) -> Option<&AdaptiveContext> {
        self.adaptive_context.as_ref()
    }

    /// Takes ownership of the adaptive context.
    pub fn take_adaptive_context(&mut self) -> Option<AdaptiveContext> {
        self.adaptive_context.take()
    }
}

// ---------------------------------------------------------------------------
// Shared conversion functions (used by both LPG and RDF planners)
// ---------------------------------------------------------------------------

/// Converts a logical binary operator to a filter binary operator.
pub fn convert_binary_op(op: BinaryOp) -> Result<BinaryFilterOp> {
    match op {
        BinaryOp::Eq => Ok(BinaryFilterOp::Eq),
        BinaryOp::Ne => Ok(BinaryFilterOp::Ne),
        BinaryOp::Lt => Ok(BinaryFilterOp::Lt),
        BinaryOp::Le => Ok(BinaryFilterOp::Le),
        BinaryOp::Gt => Ok(BinaryFilterOp::Gt),
        BinaryOp::Ge => Ok(BinaryFilterOp::Ge),
        BinaryOp::And => Ok(BinaryFilterOp::And),
        BinaryOp::Or => Ok(BinaryFilterOp::Or),
        BinaryOp::Xor => Ok(BinaryFilterOp::Xor),
        BinaryOp::Add => Ok(BinaryFilterOp::Add),
        BinaryOp::Sub => Ok(BinaryFilterOp::Sub),
        BinaryOp::Mul => Ok(BinaryFilterOp::Mul),
        BinaryOp::Div => Ok(BinaryFilterOp::Div),
        BinaryOp::Mod => Ok(BinaryFilterOp::Mod),
        BinaryOp::StartsWith => Ok(BinaryFilterOp::StartsWith),
        BinaryOp::EndsWith => Ok(BinaryFilterOp::EndsWith),
        BinaryOp::Contains => Ok(BinaryFilterOp::Contains),
        BinaryOp::In => Ok(BinaryFilterOp::In),
        BinaryOp::Regex => Ok(BinaryFilterOp::Regex),
        BinaryOp::Pow => Ok(BinaryFilterOp::Pow),
        BinaryOp::Concat => Ok(BinaryFilterOp::Concat),
        BinaryOp::Like => Ok(BinaryFilterOp::Like),
    }
}

/// Converts a logical unary operator to a filter unary operator.
pub fn convert_unary_op(op: UnaryOp) -> Result<UnaryFilterOp> {
    match op {
        UnaryOp::Not => Ok(UnaryFilterOp::Not),
        UnaryOp::IsNull => Ok(UnaryFilterOp::IsNull),
        UnaryOp::IsNotNull => Ok(UnaryFilterOp::IsNotNull),
        UnaryOp::Neg => Ok(UnaryFilterOp::Neg),
    }
}

/// Converts a logical aggregate function to a physical aggregate function.
pub fn convert_aggregate_function(func: LogicalAggregateFunction) -> PhysicalAggregateFunction {
    match func {
        LogicalAggregateFunction::Count => PhysicalAggregateFunction::Count,
        LogicalAggregateFunction::CountNonNull => PhysicalAggregateFunction::CountNonNull,
        LogicalAggregateFunction::Sum => PhysicalAggregateFunction::Sum,
        LogicalAggregateFunction::Avg => PhysicalAggregateFunction::Avg,
        LogicalAggregateFunction::Min => PhysicalAggregateFunction::Min,
        LogicalAggregateFunction::Max => PhysicalAggregateFunction::Max,
        LogicalAggregateFunction::Collect => PhysicalAggregateFunction::Collect,
        LogicalAggregateFunction::StdDev => PhysicalAggregateFunction::StdDev,
        LogicalAggregateFunction::StdDevPop => PhysicalAggregateFunction::StdDevPop,
        LogicalAggregateFunction::Variance => PhysicalAggregateFunction::Variance,
        LogicalAggregateFunction::VariancePop => PhysicalAggregateFunction::VariancePop,
        LogicalAggregateFunction::PercentileDisc => PhysicalAggregateFunction::PercentileDisc,
        LogicalAggregateFunction::PercentileCont => PhysicalAggregateFunction::PercentileCont,
        LogicalAggregateFunction::GroupConcat => PhysicalAggregateFunction::GroupConcat,
        LogicalAggregateFunction::Sample => PhysicalAggregateFunction::Sample,
        LogicalAggregateFunction::CovarSamp => PhysicalAggregateFunction::CovarSamp,
        LogicalAggregateFunction::CovarPop => PhysicalAggregateFunction::CovarPop,
        LogicalAggregateFunction::Corr => PhysicalAggregateFunction::Corr,
        LogicalAggregateFunction::RegrSlope => PhysicalAggregateFunction::RegrSlope,
        LogicalAggregateFunction::RegrIntercept => PhysicalAggregateFunction::RegrIntercept,
        LogicalAggregateFunction::RegrR2 => PhysicalAggregateFunction::RegrR2,
        LogicalAggregateFunction::RegrCount => PhysicalAggregateFunction::RegrCount,
        LogicalAggregateFunction::RegrSxx => PhysicalAggregateFunction::RegrSxx,
        LogicalAggregateFunction::RegrSyy => PhysicalAggregateFunction::RegrSyy,
        LogicalAggregateFunction::RegrSxy => PhysicalAggregateFunction::RegrSxy,
        LogicalAggregateFunction::RegrAvgx => PhysicalAggregateFunction::RegrAvgx,
        LogicalAggregateFunction::RegrAvgy => PhysicalAggregateFunction::RegrAvgy,
    }
}

/// Converts a logical expression to a filter expression.
///
/// This is a standalone function used by both LPG and RDF planners.
pub fn convert_filter_expression(expr: &LogicalExpression) -> Result<FilterExpression> {
    match expr {
        LogicalExpression::Literal(v) => Ok(FilterExpression::Literal(v.clone())),
        LogicalExpression::Variable(name) => Ok(FilterExpression::Variable(name.clone())),
        LogicalExpression::Property { variable, property } => Ok(FilterExpression::Property {
            variable: variable.clone(),
            property: property.clone(),
        }),
        LogicalExpression::Binary { left, op, right } => {
            let left_expr = convert_filter_expression(left)?;
            let right_expr = convert_filter_expression(right)?;
            let filter_op = convert_binary_op(*op)?;
            Ok(FilterExpression::Binary {
                left: Box::new(left_expr),
                op: filter_op,
                right: Box::new(right_expr),
            })
        }
        LogicalExpression::Unary { op, operand } => {
            let operand_expr = convert_filter_expression(operand)?;
            let filter_op = convert_unary_op(*op)?;
            Ok(FilterExpression::Unary {
                op: filter_op,
                operand: Box::new(operand_expr),
            })
        }
        LogicalExpression::FunctionCall { name, args, .. } => {
            let filter_args: Vec<FilterExpression> = args
                .iter()
                .map(convert_filter_expression)
                .collect::<Result<Vec<_>>>()?;
            Ok(FilterExpression::FunctionCall {
                name: name.clone(),
                args: filter_args,
            })
        }
        LogicalExpression::Case {
            operand,
            when_clauses,
            else_clause,
        } => {
            let filter_operand = operand
                .as_ref()
                .map(|e| convert_filter_expression(e))
                .transpose()?
                .map(Box::new);
            let filter_when_clauses: Vec<(FilterExpression, FilterExpression)> = when_clauses
                .iter()
                .map(|(cond, result)| {
                    Ok((
                        convert_filter_expression(cond)?,
                        convert_filter_expression(result)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let filter_else = else_clause
                .as_ref()
                .map(|e| convert_filter_expression(e))
                .transpose()?
                .map(Box::new);
            Ok(FilterExpression::Case {
                operand: filter_operand,
                when_clauses: filter_when_clauses,
                else_clause: filter_else,
            })
        }
        LogicalExpression::List(items) => {
            let filter_items: Vec<FilterExpression> = items
                .iter()
                .map(convert_filter_expression)
                .collect::<Result<Vec<_>>>()?;
            Ok(FilterExpression::List(filter_items))
        }
        LogicalExpression::Map(pairs) => {
            let filter_pairs: Vec<(String, FilterExpression)> = pairs
                .iter()
                .map(|(k, v)| Ok((k.clone(), convert_filter_expression(v)?)))
                .collect::<Result<Vec<_>>>()?;
            Ok(FilterExpression::Map(filter_pairs))
        }
        LogicalExpression::IndexAccess { base, index } => {
            let base_expr = convert_filter_expression(base)?;
            let index_expr = convert_filter_expression(index)?;
            Ok(FilterExpression::IndexAccess {
                base: Box::new(base_expr),
                index: Box::new(index_expr),
            })
        }
        LogicalExpression::SliceAccess { base, start, end } => {
            let base_expr = convert_filter_expression(base)?;
            let start_expr = start
                .as_ref()
                .map(|s| convert_filter_expression(s))
                .transpose()?
                .map(Box::new);
            let end_expr = end
                .as_ref()
                .map(|e| convert_filter_expression(e))
                .transpose()?
                .map(Box::new);
            Ok(FilterExpression::SliceAccess {
                base: Box::new(base_expr),
                start: start_expr,
                end: end_expr,
            })
        }
        LogicalExpression::Parameter(_) => Err(Error::Internal(
            "Parameters not yet supported in filters".to_string(),
        )),
        LogicalExpression::Labels(var) => Ok(FilterExpression::Labels(var.clone())),
        LogicalExpression::Type(var) => Ok(FilterExpression::Type(var.clone())),
        LogicalExpression::Id(var) => Ok(FilterExpression::Id(var.clone())),
        LogicalExpression::ListComprehension {
            variable,
            list_expr,
            filter_expr,
            map_expr,
        } => {
            let list = convert_filter_expression(list_expr)?;
            let filter = filter_expr
                .as_ref()
                .map(|f| convert_filter_expression(f))
                .transpose()?
                .map(Box::new);
            let map = convert_filter_expression(map_expr)?;
            Ok(FilterExpression::ListComprehension {
                variable: variable.clone(),
                list_expr: Box::new(list),
                filter_expr: filter,
                map_expr: Box::new(map),
            })
        }
        LogicalExpression::ListPredicate {
            kind,
            variable,
            list_expr,
            predicate,
        } => {
            use crate::query::plan::ListPredicateKind as LPK;
            let filter_kind = match kind {
                LPK::All => obrain_core::execution::operators::ListPredicateKind::All,
                LPK::Any => obrain_core::execution::operators::ListPredicateKind::Any,
                LPK::None => obrain_core::execution::operators::ListPredicateKind::None,
                LPK::Single => obrain_core::execution::operators::ListPredicateKind::Single,
            };
            let list = convert_filter_expression(list_expr)?;
            let pred = convert_filter_expression(predicate)?;
            Ok(FilterExpression::ListPredicate {
                kind: filter_kind,
                variable: variable.clone(),
                list_expr: Box::new(list),
                predicate: Box::new(pred),
            })
        }
        LogicalExpression::ExistsSubquery(_)
        | LogicalExpression::CountSubquery(_)
        | LogicalExpression::ValueSubquery(_) => {
            // Complex subqueries are handled at the plan_filter level via semi-join
            // or Apply rewrites. If we reach here, the subquery is in a position that
            // cannot be rewritten (e.g., nested inside a CASE expression). Return a
            // literal false/zero as a safe fallback.
            Err(Error::Internal(
                "Subquery expressions in this position require the semi-join or Apply rewrite; \
                 move the EXISTS/COUNT/VALUE subquery to a top-level WHERE predicate or RETURN"
                    .to_string(),
            ))
        }
        LogicalExpression::MapProjection { base, entries } => {
            let physical_entries: Vec<(String, FilterExpression)> = entries
                .iter()
                .map(|entry| match entry {
                    crate::query::plan::MapProjectionEntry::PropertySelector(name) => Ok((
                        name.clone(),
                        FilterExpression::Property {
                            variable: base.clone(),
                            property: name.clone(),
                        },
                    )),
                    crate::query::plan::MapProjectionEntry::LiteralEntry(key, expr) => {
                        Ok((key.clone(), convert_filter_expression(expr)?))
                    }
                    crate::query::plan::MapProjectionEntry::AllProperties => Ok((
                        "*".to_string(),
                        FilterExpression::FunctionCall {
                            name: "properties".to_string(),
                            args: vec![FilterExpression::Variable(base.clone())],
                        },
                    )),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(FilterExpression::Map(physical_entries))
        }
        LogicalExpression::Reduce {
            accumulator,
            initial,
            variable,
            list,
            expression,
        } => Ok(FilterExpression::Reduce {
            accumulator: accumulator.clone(),
            initial: Box::new(convert_filter_expression(initial)?),
            variable: variable.clone(),
            list: Box::new(convert_filter_expression(list)?),
            expression: Box::new(convert_filter_expression(expression)?),
        }),
        LogicalExpression::PatternComprehension { projection, .. } => {
            let proj = convert_filter_expression(projection)?;
            Ok(FilterExpression::FunctionCall {
                name: "collect".to_string(),
                args: vec![proj],
            })
        }
    }
}

/// Infers the logical type from a value.
pub(crate) fn value_to_logical_type(value: &obrain_common::types::Value) -> LogicalType {
    use obrain_common::types::Value;
    match value {
        Value::Null => LogicalType::String,
        Value::Bool(_) => LogicalType::Bool,
        Value::Int64(_) => LogicalType::Int64,
        Value::Float64(_) => LogicalType::Float64,
        Value::String(_) => LogicalType::String,
        Value::Bytes(_) => LogicalType::String,
        Value::Timestamp(_) => LogicalType::Timestamp,
        Value::Date(_) => LogicalType::Date,
        Value::Time(_) => LogicalType::Time,
        Value::Duration(_) => LogicalType::Duration,
        Value::ZonedDatetime(_) => LogicalType::ZonedDatetime,
        Value::List(_) => LogicalType::String,
        Value::Map(_) => LogicalType::String,
        Value::Vector(v) => LogicalType::Vector(v.len()),
        Value::Path { .. } => LogicalType::Any,
    }
}

/// Evaluates a constant logical expression to a Value.
///
/// Only handles literals, unary minus on numeric literals, and simple expressions.
/// Returns an error for runtime-dependent expressions (variables, property accesses, etc.).
#[cfg(feature = "algos")]
pub(crate) fn eval_constant_expression(
    expr: &crate::query::plan::LogicalExpression,
) -> Result<obrain_common::types::Value> {
    use crate::query::plan::LogicalExpression;
    use obrain_common::types::Value;

    match expr {
        LogicalExpression::Literal(val) => Ok(val.clone()),
        LogicalExpression::Unary {
            op: crate::query::plan::UnaryOp::Neg,
            operand,
        } => {
            let val = eval_constant_expression(operand)?;
            match val {
                Value::Int64(n) => Ok(Value::Int64(-n)),
                Value::Float64(f) => Ok(Value::Float64(-f)),
                _ => Err(Error::Internal("Cannot negate non-numeric value".into())),
            }
        }
        _ => Err(Error::Internal(
            "Procedure argument must be a constant value".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::query::plan::{ListPredicateKind, LogicalOperator, MapProjectionEntry, NodeScanOp};
    use obrain_common::types::{Date, Duration, Time, Timestamp, Value, ZonedDatetime};
    use obrain_core::execution::operators::EmptyOperator;
    use obrain_core::execution::operators::ListPredicateKind as PhysicalListPredicateKind;

    fn var(name: &str) -> LogicalExpression {
        LogicalExpression::Variable(name.into())
    }

    fn int(n: i64) -> LogicalExpression {
        LogicalExpression::Literal(Value::Int64(n))
    }

    fn scan(variable: &str) -> Box<LogicalOperator> {
        Box::new(LogicalOperator::NodeScan(NodeScanOp {
            variable: variable.into(),
            label: None,
            input: None,
            pushdown_hint: None,
        }))
    }

    fn convertit(expr: &LogicalExpression) -> FilterExpression {
        convert_filter_expression(expr).expect("conversion attendue sans erreur")
    }

    // --- PhysicalPlan ---

    #[test]
    fn physical_plan_expose_colonnes_et_contexte_adaptatif() {
        let mut plan = PhysicalPlan {
            operator: Box::new(EmptyOperator::new(vec![])),
            columns: vec!["a".into(), "b".into()],
            adaptive_context: Some(AdaptiveContext::new()),
        };
        assert_eq!(plan.columns(), ["a".to_string(), "b".to_string()]);
        assert!(plan.adaptive_context().is_some());
        assert!(plan.take_adaptive_context().is_some());
        assert!(
            plan.adaptive_context().is_none(),
            "take_adaptive_context doit consommer le contexte"
        );
        assert!(plan.take_adaptive_context().is_none());
        let operateur = plan.into_operator();
        assert_ne!(operateur.name(), "");
    }

    #[test]
    fn physical_plan_sans_contexte_adaptatif() {
        let plan = PhysicalPlan {
            operator: Box::new(EmptyOperator::new(vec![])),
            columns: vec![],
            adaptive_context: None,
        };
        assert_eq!(plan.columns().len(), 0);
        assert!(plan.adaptive_context().is_none());
    }

    // --- convert_binary_op / convert_unary_op ---

    #[test]
    fn convert_binary_op_couvre_toutes_les_variantes() {
        let cas = [
            (BinaryOp::Eq, BinaryFilterOp::Eq),
            (BinaryOp::Ne, BinaryFilterOp::Ne),
            (BinaryOp::Lt, BinaryFilterOp::Lt),
            (BinaryOp::Le, BinaryFilterOp::Le),
            (BinaryOp::Gt, BinaryFilterOp::Gt),
            (BinaryOp::Ge, BinaryFilterOp::Ge),
            (BinaryOp::And, BinaryFilterOp::And),
            (BinaryOp::Or, BinaryFilterOp::Or),
            (BinaryOp::Xor, BinaryFilterOp::Xor),
            (BinaryOp::Add, BinaryFilterOp::Add),
            (BinaryOp::Sub, BinaryFilterOp::Sub),
            (BinaryOp::Mul, BinaryFilterOp::Mul),
            (BinaryOp::Div, BinaryFilterOp::Div),
            (BinaryOp::Mod, BinaryFilterOp::Mod),
            (BinaryOp::StartsWith, BinaryFilterOp::StartsWith),
            (BinaryOp::EndsWith, BinaryFilterOp::EndsWith),
            (BinaryOp::Contains, BinaryFilterOp::Contains),
            (BinaryOp::In, BinaryFilterOp::In),
            (BinaryOp::Regex, BinaryFilterOp::Regex),
            (BinaryOp::Pow, BinaryFilterOp::Pow),
            (BinaryOp::Concat, BinaryFilterOp::Concat),
            (BinaryOp::Like, BinaryFilterOp::Like),
        ];
        for (logique, physique) in cas {
            assert_eq!(
                convert_binary_op(logique).expect("conversion"),
                physique,
                "{logique:?}"
            );
        }
    }

    #[test]
    fn convert_unary_op_couvre_toutes_les_variantes() {
        let cas = [
            (UnaryOp::Not, UnaryFilterOp::Not),
            (UnaryOp::IsNull, UnaryFilterOp::IsNull),
            (UnaryOp::IsNotNull, UnaryFilterOp::IsNotNull),
            (UnaryOp::Neg, UnaryFilterOp::Neg),
        ];
        for (logique, physique) in cas {
            assert_eq!(
                convert_unary_op(logique).expect("conversion"),
                physique,
                "{logique:?}"
            );
        }
    }

    // --- convert_aggregate_function ---

    #[test]
    fn convert_aggregate_function_couvre_toutes_les_variantes() {
        use LogicalAggregateFunction as L;
        use PhysicalAggregateFunction as P;
        let cas = [
            (L::Count, P::Count),
            (L::CountNonNull, P::CountNonNull),
            (L::Sum, P::Sum),
            (L::Avg, P::Avg),
            (L::Min, P::Min),
            (L::Max, P::Max),
            (L::Collect, P::Collect),
            (L::StdDev, P::StdDev),
            (L::StdDevPop, P::StdDevPop),
            (L::Variance, P::Variance),
            (L::VariancePop, P::VariancePop),
            (L::PercentileDisc, P::PercentileDisc),
            (L::PercentileCont, P::PercentileCont),
            (L::GroupConcat, P::GroupConcat),
            (L::Sample, P::Sample),
            (L::CovarSamp, P::CovarSamp),
            (L::CovarPop, P::CovarPop),
            (L::Corr, P::Corr),
            (L::RegrSlope, P::RegrSlope),
            (L::RegrIntercept, P::RegrIntercept),
            (L::RegrR2, P::RegrR2),
            (L::RegrCount, P::RegrCount),
            (L::RegrSxx, P::RegrSxx),
            (L::RegrSyy, P::RegrSyy),
            (L::RegrSxy, P::RegrSxy),
            (L::RegrAvgx, P::RegrAvgx),
            (L::RegrAvgy, P::RegrAvgy),
        ];
        for (logique, physique) in cas {
            assert_eq!(convert_aggregate_function(logique), physique, "{logique:?}");
        }
    }

    // --- convert_filter_expression : feuilles ---

    #[test]
    fn filter_feuilles_litteral_variable_propriete_et_accesseurs() {
        assert!(matches!(
            convertit(&int(7)),
            FilterExpression::Literal(Value::Int64(7))
        ));
        assert!(matches!(
            convertit(&var("n")),
            FilterExpression::Variable(v) if v == "n"
        ));
        assert!(matches!(
            convertit(&LogicalExpression::Property {
                variable: "n".into(),
                property: "age".into(),
            }),
            FilterExpression::Property { variable, property }
                if variable == "n" && property == "age"
        ));
        assert!(matches!(
            convertit(&LogicalExpression::Labels("n".into())),
            FilterExpression::Labels(v) if v == "n"
        ));
        assert!(matches!(
            convertit(&LogicalExpression::Type("r".into())),
            FilterExpression::Type(v) if v == "r"
        ));
        assert!(matches!(
            convertit(&LogicalExpression::Id("n".into())),
            FilterExpression::Id(v) if v == "n"
        ));
    }

    #[test]
    fn filter_binaire_et_unaire_convertissent_les_operateurs() {
        let binaire = LogicalExpression::Binary {
            left: Box::new(var("a")),
            op: BinaryOp::Gt,
            right: Box::new(int(3)),
        };
        assert!(matches!(
            convertit(&binaire),
            FilterExpression::Binary { op: BinaryFilterOp::Gt, left, right }
                if matches!(*left, FilterExpression::Variable(_))
                    && matches!(*right, FilterExpression::Literal(_))
        ));

        let unaire = LogicalExpression::Unary {
            op: UnaryOp::IsNull,
            operand: Box::new(var("a")),
        };
        assert!(matches!(
            convertit(&unaire),
            FilterExpression::Unary {
                op: UnaryFilterOp::IsNull,
                ..
            }
        ));
    }

    #[test]
    fn filter_appel_de_fonction_liste_et_map() {
        let appel = LogicalExpression::FunctionCall {
            name: "size".into(),
            args: vec![var("l"), int(1)],
            distinct: true,
        };
        assert!(matches!(
            convertit(&appel),
            FilterExpression::FunctionCall { name, args } if name == "size" && args.len() == 2
        ));

        let liste = LogicalExpression::List(vec![int(1), int(2), int(3)]);
        assert!(matches!(
            convertit(&liste),
            FilterExpression::List(items) if items.len() == 3
        ));

        let map = LogicalExpression::Map(vec![("k".into(), int(1)), ("j".into(), var("x"))]);
        assert!(matches!(
            convertit(&map),
            FilterExpression::Map(paires)
                if paires.len() == 2 && paires[0].0 == "k" && paires[1].0 == "j"
        ));
    }

    #[test]
    fn filter_acces_par_index_et_tranche() {
        let index = LogicalExpression::IndexAccess {
            base: Box::new(var("l")),
            index: Box::new(int(0)),
        };
        assert!(matches!(
            convertit(&index),
            FilterExpression::IndexAccess { .. }
        ));

        let tranche_complete = LogicalExpression::SliceAccess {
            base: Box::new(var("l")),
            start: Some(Box::new(int(1))),
            end: Some(Box::new(int(2))),
        };
        assert!(matches!(
            convertit(&tranche_complete),
            FilterExpression::SliceAccess {
                start: Some(_),
                end: Some(_),
                ..
            }
        ));

        let tranche_ouverte = LogicalExpression::SliceAccess {
            base: Box::new(var("l")),
            start: None,
            end: None,
        };
        assert!(matches!(
            convertit(&tranche_ouverte),
            FilterExpression::SliceAccess {
                start: None,
                end: None,
                ..
            }
        ));
    }

    #[test]
    fn filter_case_avec_et_sans_operande() {
        let simple = LogicalExpression::Case {
            operand: Some(Box::new(var("x"))),
            when_clauses: vec![(int(1), var("un")), (int(2), var("deux"))],
            else_clause: Some(Box::new(var("autre"))),
        };
        assert!(matches!(
            convertit(&simple),
            FilterExpression::Case { operand: Some(_), when_clauses, else_clause: Some(_) }
                if when_clauses.len() == 2
        ));

        let recherche = LogicalExpression::Case {
            operand: None,
            when_clauses: vec![(var("c"), int(1))],
            else_clause: None,
        };
        assert!(matches!(
            convertit(&recherche),
            FilterExpression::Case { operand: None, when_clauses, else_clause: None }
                if when_clauses.len() == 1
        ));
    }

    #[test]
    fn filter_comprehension_et_predicat_de_liste() {
        let comprehension = LogicalExpression::ListComprehension {
            variable: "x".into(),
            list_expr: Box::new(var("l")),
            filter_expr: Some(Box::new(var("x"))),
            map_expr: Box::new(var("x")),
        };
        assert!(matches!(
            convertit(&comprehension),
            FilterExpression::ListComprehension { variable, filter_expr: Some(_), .. }
                if variable == "x"
        ));

        let sans_filtre = LogicalExpression::ListComprehension {
            variable: "x".into(),
            list_expr: Box::new(var("l")),
            filter_expr: None,
            map_expr: Box::new(var("x")),
        };
        assert!(matches!(
            convertit(&sans_filtre),
            FilterExpression::ListComprehension {
                filter_expr: None,
                ..
            }
        ));

        for (logique, physique) in [
            (ListPredicateKind::All, PhysicalListPredicateKind::All),
            (ListPredicateKind::Any, PhysicalListPredicateKind::Any),
            (ListPredicateKind::None, PhysicalListPredicateKind::None),
            (ListPredicateKind::Single, PhysicalListPredicateKind::Single),
        ] {
            let predicat = LogicalExpression::ListPredicate {
                kind: logique.clone(),
                variable: "x".into(),
                list_expr: Box::new(var("l")),
                predicate: Box::new(var("x")),
            };
            match convertit(&predicat) {
                FilterExpression::ListPredicate { kind, variable, .. } => {
                    assert_eq!(kind, physique, "{logique:?}");
                    assert_eq!(variable, "x");
                }
                autre => panic!("ListPredicate attendu, obtenu {autre:?}"),
            }
        }
    }

    #[test]
    fn filter_map_projection_rend_une_map_de_trois_formes() {
        let projection = LogicalExpression::MapProjection {
            base: "n".into(),
            entries: vec![
                MapProjectionEntry::PropertySelector("name".into()),
                MapProjectionEntry::LiteralEntry("k".into(), int(1)),
                MapProjectionEntry::AllProperties,
            ],
        };
        let FilterExpression::Map(entrees) = convertit(&projection) else {
            panic!("Map attendue");
        };
        assert_eq!(entrees.len(), 3);
        assert_eq!(entrees[0].0, "name");
        assert!(matches!(
            &entrees[0].1,
            FilterExpression::Property { variable, property }
                if variable == "n" && property == "name"
        ));
        assert_eq!(entrees[1].0, "k");
        assert!(matches!(&entrees[1].1, FilterExpression::Literal(_)));
        assert_eq!(entrees[2].0, "*");
        assert!(matches!(
            &entrees[2].1,
            FilterExpression::FunctionCall { name, args }
                if name == "properties" && args.len() == 1
        ));
    }

    #[test]
    fn filter_reduce_et_pattern_comprehension() {
        let reduce = LogicalExpression::Reduce {
            accumulator: "acc".into(),
            initial: Box::new(int(0)),
            variable: "x".into(),
            list: Box::new(var("l")),
            expression: Box::new(var("acc")),
        };
        assert!(matches!(
            convertit(&reduce),
            FilterExpression::Reduce { accumulator, variable, .. }
                if accumulator == "acc" && variable == "x"
        ));

        let motif = LogicalExpression::PatternComprehension {
            subplan: scan("m"),
            projection: Box::new(var("m")),
        };
        assert!(matches!(
            convertit(&motif),
            FilterExpression::FunctionCall { name, args } if name == "collect" && args.len() == 1
        ));
    }

    #[test]
    fn filter_refuse_parametre_et_sous_requetes() {
        let parametre = LogicalExpression::Parameter("p".into());
        let err = convert_filter_expression(&parametre).expect_err("paramètre refusé");
        assert!(format!("{err:?}").contains("Parameters"), "{err:?}");

        for sous_requete in [
            LogicalExpression::ExistsSubquery(scan("m")),
            LogicalExpression::CountSubquery(scan("m")),
            LogicalExpression::ValueSubquery(scan("m")),
        ] {
            let err = convert_filter_expression(&sous_requete).expect_err("sous-requête refusée");
            assert!(format!("{err:?}").contains("semi-join"), "{err:?}");
        }
    }

    #[test]
    fn filter_propage_l_erreur_d_un_enfant() {
        let parametre_enfoui = LogicalExpression::Binary {
            left: Box::new(var("a")),
            op: BinaryOp::Eq,
            right: Box::new(LogicalExpression::Parameter("p".into())),
        };
        assert!(convert_filter_expression(&parametre_enfoui).is_err());

        let dans_une_liste =
            LogicalExpression::List(vec![LogicalExpression::Parameter("p".into())]);
        assert!(convert_filter_expression(&dans_une_liste).is_err());
    }

    // --- value_to_logical_type ---

    #[test]
    fn value_to_logical_type_couvre_toutes_les_variantes() {
        let cas: Vec<(Value, LogicalType)> = vec![
            (Value::Null, LogicalType::String),
            (Value::Bool(true), LogicalType::Bool),
            (Value::Int64(1), LogicalType::Int64),
            (Value::Float64(1.5), LogicalType::Float64),
            (Value::String("s".into()), LogicalType::String),
            (Value::Bytes(Arc::from(vec![1u8, 2])), LogicalType::String),
            (
                Value::Timestamp(Timestamp::from_secs(0)),
                LogicalType::Timestamp,
            ),
            (Value::Date(Date::from_days(0)), LogicalType::Date),
            (
                Value::Time(Time::from_hms(1, 2, 3).expect("heure valide")),
                LogicalType::Time,
            ),
            (
                Value::Duration(Duration::from_days(1)),
                LogicalType::Duration,
            ),
            (
                Value::ZonedDatetime(ZonedDatetime::from_timestamp_offset(
                    Timestamp::from_secs(0),
                    3600,
                )),
                LogicalType::ZonedDatetime,
            ),
            (
                Value::List(Arc::from(vec![Value::Int64(1)])),
                LogicalType::String,
            ),
            (Value::Map(Arc::new(BTreeMap::new())), LogicalType::String),
            (
                Value::Vector(Arc::from(vec![1.0f32, 2.0, 3.0])),
                LogicalType::Vector(3),
            ),
            (
                Value::Path {
                    nodes: Arc::from(Vec::<Value>::new()),
                    edges: Arc::from(Vec::<Value>::new()),
                },
                LogicalType::Any,
            ),
        ];
        for (valeur, attendu) in cas {
            assert_eq!(value_to_logical_type(&valeur), attendu, "{valeur:?}");
        }
    }

    // --- eval_constant_expression (feature algos) ---

    #[cfg(feature = "algos")]
    #[test]
    fn eval_constant_litteral_et_negation_numerique() {
        assert_eq!(
            eval_constant_expression(&int(5)).expect("littéral"),
            Value::Int64(5)
        );
        let moins_cinq = LogicalExpression::Unary {
            op: UnaryOp::Neg,
            operand: Box::new(int(5)),
        };
        assert_eq!(
            eval_constant_expression(&moins_cinq).expect("négation"),
            Value::Int64(-5)
        );
        let moins_flottant = LogicalExpression::Unary {
            op: UnaryOp::Neg,
            operand: Box::new(LogicalExpression::Literal(Value::Float64(2.5))),
        };
        assert_eq!(
            eval_constant_expression(&moins_flottant).expect("négation"),
            Value::Float64(-2.5)
        );
    }

    #[cfg(feature = "algos")]
    #[test]
    fn eval_constant_refuse_non_numerique_et_non_constant() {
        let moins_texte = LogicalExpression::Unary {
            op: UnaryOp::Neg,
            operand: Box::new(LogicalExpression::Literal(Value::String("x".into()))),
        };
        let err = eval_constant_expression(&moins_texte).expect_err("négation d'un texte");
        assert!(format!("{err:?}").contains("non-numeric"), "{err:?}");

        let err = eval_constant_expression(&var("n")).expect_err("variable non constante");
        assert!(format!("{err:?}").contains("constant"), "{err:?}");

        let non_negation = LogicalExpression::Unary {
            op: UnaryOp::Not,
            operand: Box::new(int(1)),
        };
        assert!(eval_constant_expression(&non_negation).is_err());
    }
}
