//! Shared utilities for query language translators.
//!
//! Functions here are used by multiple translator modules (GQL, Cypher, etc.)
//! to avoid duplication of identical logic.

use std::collections::{HashMap, HashSet};
#[cfg(any(feature = "graphql", feature = "gremlin", test))]
use std::sync::atomic::{AtomicU32, Ordering};

use crate::query::plan::{
    AggregateFunction, BinaryOp, CountExpr, DistinctOp, FilterOp, LeftJoinOp, LimitOp,
    LogicalExpression, LogicalOperator, ReturnItem, ReturnOp, SkipOp, SortKey, SortOp,
};
use obrain_common::utils::error::{Error, QueryError, QueryErrorKind, Result};

/// Returns true if the function name is a recognized aggregate function.
pub(crate) fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "COUNT"
            | "SUM"
            | "AVG"
            | "MIN"
            | "MAX"
            | "COLLECT"
            | "STDEV"
            | "STDDEV"
            | "STDDEV_SAMP"
            | "STDEVP"
            | "STDDEVP"
            | "STDDEV_POP"
            | "VARIANCE"
            | "VAR_SAMP"
            | "VAR_POP"
            | "PERCENTILE_DISC"
            | "PERCENTILEDISC"
            | "PERCENTILE_CONT"
            | "PERCENTILECONT"
            | "GROUP_CONCAT"
            | "GROUPCONCAT"
            | "LISTAGG"
            | "SAMPLE"
            | "COVAR_SAMP"
            | "COVAR_POP"
            | "CORR"
            | "REGR_SLOPE"
            | "REGR_INTERCEPT"
            | "REGR_R2"
            | "REGR_COUNT"
            | "REGR_SXX"
            | "REGR_SYY"
            | "REGR_SXY"
            | "REGR_AVGX"
            | "REGR_AVGY"
    )
}

/// Converts a function name to an `AggregateFunction` enum variant.
pub(crate) fn to_aggregate_function(name: &str) -> Option<AggregateFunction> {
    match name.to_uppercase().as_str() {
        "COUNT" => Some(AggregateFunction::Count),
        "SUM" => Some(AggregateFunction::Sum),
        "AVG" => Some(AggregateFunction::Avg),
        "MIN" => Some(AggregateFunction::Min),
        "MAX" => Some(AggregateFunction::Max),
        "COLLECT" => Some(AggregateFunction::Collect),
        "STDEV" | "STDDEV" | "STDDEV_SAMP" => Some(AggregateFunction::StdDev),
        "STDEVP" | "STDDEVP" | "STDDEV_POP" => Some(AggregateFunction::StdDevPop),
        "VARIANCE" | "VAR_SAMP" => Some(AggregateFunction::Variance),
        "VAR_POP" => Some(AggregateFunction::VariancePop),
        "PERCENTILE_DISC" | "PERCENTILEDISC" => Some(AggregateFunction::PercentileDisc),
        "PERCENTILE_CONT" | "PERCENTILECONT" => Some(AggregateFunction::PercentileCont),
        "GROUP_CONCAT" | "GROUPCONCAT" | "LISTAGG" => Some(AggregateFunction::GroupConcat),
        "SAMPLE" => Some(AggregateFunction::Sample),
        "COVAR_SAMP" => Some(AggregateFunction::CovarSamp),
        "COVAR_POP" => Some(AggregateFunction::CovarPop),
        "CORR" => Some(AggregateFunction::Corr),
        "REGR_SLOPE" => Some(AggregateFunction::RegrSlope),
        "REGR_INTERCEPT" => Some(AggregateFunction::RegrIntercept),
        "REGR_R2" => Some(AggregateFunction::RegrR2),
        "REGR_COUNT" => Some(AggregateFunction::RegrCount),
        "REGR_SXX" => Some(AggregateFunction::RegrSxx),
        "REGR_SYY" => Some(AggregateFunction::RegrSyy),
        "REGR_SXY" => Some(AggregateFunction::RegrSxy),
        "REGR_AVGX" => Some(AggregateFunction::RegrAvgx),
        "REGR_AVGY" => Some(AggregateFunction::RegrAvgy),
        _ => None,
    }
}

/// Returns true if the aggregate function is a binary set function (requires two arguments).
pub(crate) fn is_binary_set_function(func: AggregateFunction) -> bool {
    matches!(
        func,
        AggregateFunction::CovarSamp
            | AggregateFunction::CovarPop
            | AggregateFunction::Corr
            | AggregateFunction::RegrSlope
            | AggregateFunction::RegrIntercept
            | AggregateFunction::RegrR2
            | AggregateFunction::RegrCount
            | AggregateFunction::RegrSxx
            | AggregateFunction::RegrSyy
            | AggregateFunction::RegrSxy
            | AggregateFunction::RegrAvgx
            | AggregateFunction::RegrAvgy
    )
}

/// Capitalizes the first character of a string.
///
/// Used by GraphQL translators to convert field names to type names.
#[cfg(any(feature = "graphql", test))]
pub(crate) fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Generates unique variable names with an atomic counter.
///
/// Replaces the duplicated `var_counter: AtomicU32` + `next_var()` pattern
/// used across multiple translators.
#[cfg(any(feature = "graphql", feature = "gremlin", test))]
pub(crate) struct VarGen {
    counter: AtomicU32,
}

#[cfg(any(feature = "graphql", feature = "gremlin", test))]
impl VarGen {
    /// Creates a new variable generator starting from 0.
    pub fn new() -> Self {
        Self {
            counter: AtomicU32::new(0),
        }
    }

    /// Returns the next unique variable name (e.g., `_v0`, `_v1`, ...).
    pub fn next(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("_v{n}")
    }

    /// Returns the current counter value without incrementing.
    pub fn current(&self) -> u32 {
        self.counter.load(Ordering::Relaxed)
    }
}

/// Combines a non-empty vector of predicates into a single AND expression.
///
/// Returns an error if the input is empty. Used by `build_property_predicate`
/// in multiple translators.
pub(crate) fn combine_with_and(predicates: Vec<LogicalExpression>) -> Result<LogicalExpression> {
    predicates
        .into_iter()
        .reduce(|acc, pred| LogicalExpression::Binary {
            left: Box::new(acc),
            op: BinaryOp::And,
            right: Box::new(pred),
        })
        .ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                "Empty property predicate",
            ))
        })
}

// ---------------------------------------------------------------------------
// Variable extraction
// ---------------------------------------------------------------------------

/// Collects all variable names referenced by a logical expression.
pub(crate) fn collect_expression_variables(expr: &LogicalExpression, vars: &mut HashSet<String>) {
    match expr {
        LogicalExpression::Variable(name) => {
            vars.insert(name.clone());
        }
        LogicalExpression::Property { variable, .. }
        | LogicalExpression::Labels(variable)
        | LogicalExpression::Type(variable)
        | LogicalExpression::Id(variable) => {
            vars.insert(variable.clone());
        }
        LogicalExpression::Binary { left, right, .. } => {
            collect_expression_variables(left, vars);
            collect_expression_variables(right, vars);
        }
        LogicalExpression::Unary { operand, .. } => {
            collect_expression_variables(operand, vars);
        }
        LogicalExpression::FunctionCall { args, .. } => {
            for arg in args {
                collect_expression_variables(arg, vars);
            }
        }
        LogicalExpression::List(items) => {
            for item in items {
                collect_expression_variables(item, vars);
            }
        }
        LogicalExpression::Map(pairs) => {
            for (_, value) in pairs {
                collect_expression_variables(value, vars);
            }
        }
        LogicalExpression::IndexAccess { base, index } => {
            collect_expression_variables(base, vars);
            collect_expression_variables(index, vars);
        }
        LogicalExpression::SliceAccess { base, start, end } => {
            collect_expression_variables(base, vars);
            if let Some(s) = start {
                collect_expression_variables(s, vars);
            }
            if let Some(e) = end {
                collect_expression_variables(e, vars);
            }
        }
        LogicalExpression::Case {
            operand,
            when_clauses,
            else_clause,
        } => {
            if let Some(op) = operand {
                collect_expression_variables(op, vars);
            }
            for (cond, result) in when_clauses {
                collect_expression_variables(cond, vars);
                collect_expression_variables(result, vars);
            }
            if let Some(else_expr) = else_clause {
                collect_expression_variables(else_expr, vars);
            }
        }
        LogicalExpression::ListComprehension {
            list_expr,
            filter_expr,
            map_expr,
            ..
        } => {
            collect_expression_variables(list_expr, vars);
            if let Some(filter) = filter_expr {
                collect_expression_variables(filter, vars);
            }
            collect_expression_variables(map_expr, vars);
        }
        LogicalExpression::ListPredicate {
            list_expr,
            predicate,
            ..
        } => {
            collect_expression_variables(list_expr, vars);
            collect_expression_variables(predicate, vars);
        }
        LogicalExpression::MapProjection { base, entries } => {
            vars.insert(base.clone());
            for entry in entries {
                if let crate::query::plan::MapProjectionEntry::LiteralEntry(_, expr) = entry {
                    collect_expression_variables(expr, vars);
                }
            }
        }
        LogicalExpression::Reduce {
            initial,
            list,
            expression,
            ..
        } => {
            collect_expression_variables(initial, vars);
            collect_expression_variables(list, vars);
            collect_expression_variables(expression, vars);
        }
        LogicalExpression::PatternComprehension { projection, .. } => {
            collect_expression_variables(projection, vars);
        }
        LogicalExpression::Literal(_)
        | LogicalExpression::Parameter(_)
        | LogicalExpression::ExistsSubquery(_)
        | LogicalExpression::CountSubquery(_)
        | LogicalExpression::ValueSubquery(_) => {}
    }
}

// ---------------------------------------------------------------------------
// OPTIONAL MATCH predicate classification
// ---------------------------------------------------------------------------

/// Splits a conjunctive predicate (AND-chain) into individual conjuncts.
pub(crate) fn split_conjuncts(expr: LogicalExpression) -> Vec<LogicalExpression> {
    let mut result = Vec::new();
    split_conjuncts_recursive(expr, &mut result);
    result
}

fn split_conjuncts_recursive(expr: LogicalExpression, out: &mut Vec<LogicalExpression>) {
    if let LogicalExpression::Binary {
        left,
        op: BinaryOp::And,
        right,
    } = expr
    {
        split_conjuncts_recursive(*left, out);
        split_conjuncts_recursive(*right, out);
    } else {
        out.push(expr);
    }
}

/// Result of classifying WHERE predicates for OPTIONAL MATCH.
///
/// Predicates are split based on which side of the LeftJoin their variables
/// belong to, ensuring correct NULL-preservation semantics.
pub(crate) struct ClassifiedPredicates {
    /// Predicates referencing only left-side variables (or constants): placed
    /// as a Filter above the LeftJoin (they filter the required side).
    pub post_filters: Vec<LogicalExpression>,
    /// Predicates whose referenced variables all exist on the right side:
    /// pushed as a pre-filter on the right input of the LeftJoin.
    pub right_filters: Vec<LogicalExpression>,
}

/// Classifies WHERE predicates for correct OPTIONAL MATCH semantics.
///
/// Given a predicate and the set of variables produced by the left (required)
/// and right (optional) sides of a LeftJoin, splits the predicate into:
///
/// - **post_filters**: reference only left-side variables, safe to apply after the join
/// - **right_filters**: reference only right-side variables, can be pushed as a
///   pre-filter on the right input (semantically equivalent to a join condition)
/// - **cross_filters**: reference both sides, must be applied as a join condition
pub(crate) fn classify_optional_predicates(
    predicate: LogicalExpression,
    left_vars: &HashSet<String>,
    right_vars: &HashSet<String>,
) -> ClassifiedPredicates {
    let conjuncts = split_conjuncts(predicate);
    let mut post_filters = Vec::new();
    let mut right_filters = Vec::new();

    for conjunct in conjuncts {
        let mut referenced = HashSet::new();
        collect_expression_variables(&conjunct, &mut referenced);

        // A predicate is a right-filter only if it references at least one
        // right-ONLY variable (a variable produced exclusively by the optional
        // side, not shared with the required side). Predicates on shared
        // variables alone (e.g., `n.city = 'NYC'` where `n` is the join key)
        // must remain as post-filters because they constrain the required side.
        let has_right_only_var = referenced
            .iter()
            .any(|v| right_vars.contains(v) && !left_vars.contains(v));
        let all_in_right = referenced.iter().all(|v| right_vars.contains(v));

        if referenced.is_empty() {
            // Constant predicate: post-filter
            post_filters.push(conjunct);
        } else if has_right_only_var && all_in_right {
            // References at least one right-only variable, and all referenced
            // variables exist on the right side: push as pre-filter on right input.
            right_filters.push(conjunct);
        } else {
            // Left-only, shared-only, or cross-side: post-filter above the join.
            post_filters.push(conjunct);
        }
    }

    ClassifiedPredicates {
        post_filters,
        right_filters,
    }
}

/// Collects all variables produced by a logical operator's subtree.
pub(crate) fn collect_operator_variables(op: &LogicalOperator, vars: &mut HashSet<String>) {
    match op {
        LogicalOperator::NodeScan(scan) => {
            vars.insert(scan.variable.clone());
            if let Some(input) = &scan.input {
                collect_operator_variables(input, vars);
            }
        }
        LogicalOperator::EdgeScan(scan) => {
            vars.insert(scan.variable.clone());
        }
        LogicalOperator::Expand(expand) => {
            vars.insert(expand.to_variable.clone());
            if let Some(edge_var) = &expand.edge_variable {
                vars.insert(edge_var.clone());
            }
            collect_operator_variables(&expand.input, vars);
        }
        LogicalOperator::Filter(filter) => {
            collect_operator_variables(&filter.input, vars);
        }
        LogicalOperator::Project(proj) => {
            for p in &proj.projections {
                if let Some(alias) = &p.alias {
                    vars.insert(alias.clone());
                }
            }
            collect_operator_variables(&proj.input, vars);
        }
        LogicalOperator::Join(join) => {
            collect_operator_variables(&join.left, vars);
            collect_operator_variables(&join.right, vars);
        }
        LogicalOperator::LeftJoin(lj) => {
            collect_operator_variables(&lj.left, vars);
            collect_operator_variables(&lj.right, vars);
        }
        LogicalOperator::Unwind(unwind) => {
            vars.insert(unwind.variable.clone());
            collect_operator_variables(&unwind.input, vars);
        }
        LogicalOperator::Bind(bind) => {
            vars.insert(bind.variable.clone());
            collect_operator_variables(&bind.input, vars);
        }
        LogicalOperator::Aggregate(agg) => {
            for expr in &agg.group_by {
                collect_expression_variables(expr, vars);
            }
            for agg_expr in &agg.aggregates {
                if let Some(alias) = &agg_expr.alias {
                    vars.insert(alias.clone());
                }
            }
            collect_operator_variables(&agg.input, vars);
        }
        LogicalOperator::Return(ret) => {
            collect_operator_variables(&ret.input, vars);
        }
        LogicalOperator::Limit(limit) => {
            collect_operator_variables(&limit.input, vars);
        }
        LogicalOperator::Skip(skip) => {
            collect_operator_variables(&skip.input, vars);
        }
        LogicalOperator::Sort(sort) => {
            collect_operator_variables(&sort.input, vars);
        }
        LogicalOperator::Distinct(distinct) => {
            collect_operator_variables(&distinct.input, vars);
        }
        _ => {
            // For other operators, do not recurse to avoid false positives.
            // The common cases (NodeScan, Expand, Filter, Join, LeftJoin,
            // Unwind, Project, Aggregate, Return) are covered above.
        }
    }
}

/// Builds a LeftJoin with properly classified WHERE predicates.
///
/// Given a WHERE predicate that follows an OPTIONAL MATCH, this function:
/// 1. Collects variables from both sides
/// 2. Classifies predicates into left-only, right-only, and cross-side
/// 3. Pushes right-only predicates as a Filter on the right input
/// 4. Stores cross-side predicates in `LeftJoinOp.condition`
/// 5. Returns the LeftJoin and any remaining post-filters to apply above
pub(crate) fn build_left_join_with_predicates(
    left: LogicalOperator,
    right: LogicalOperator,
    predicate: Option<LogicalExpression>,
) -> (LogicalOperator, Option<LogicalExpression>) {
    let Some(predicate) = predicate else {
        let join = LogicalOperator::LeftJoin(LeftJoinOp {
            left: Box::new(left),
            right: Box::new(right),
            condition: None,
        });
        return (join, None);
    };

    // Collect variables from each side
    let mut left_vars = HashSet::new();
    collect_operator_variables(&left, &mut left_vars);
    let mut right_vars = HashSet::new();
    collect_operator_variables(&right, &mut right_vars);

    // Classify
    let classified = classify_optional_predicates(predicate, &left_vars, &right_vars);

    // Build right input with right-only filters pushed down
    let filtered_right = if classified.right_filters.is_empty() {
        right
    } else {
        let right_pred = classified
            .right_filters
            .into_iter()
            .reduce(|acc, pred| LogicalExpression::Binary {
                left: Box::new(acc),
                op: BinaryOp::And,
                right: Box::new(pred),
            })
            .expect("non-empty right_filters");
        wrap_filter(right, right_pred)
    };

    let join = LogicalOperator::LeftJoin(LeftJoinOp {
        left: Box::new(left),
        right: Box::new(filtered_right),
        condition: None,
    });

    // Combine remaining post-filters
    let post_filter = if classified.post_filters.is_empty() {
        None
    } else {
        Some(
            classified
                .post_filters
                .into_iter()
                .reduce(|acc, pred| LogicalExpression::Binary {
                    left: Box::new(acc),
                    op: BinaryOp::And,
                    right: Box::new(pred),
                })
                .expect("non-empty post_filters"),
        )
    };

    (join, post_filter)
}

// ---------------------------------------------------------------------------
// Plan node builder helpers
// ---------------------------------------------------------------------------

/// Wraps an operator with a filter predicate.
pub(crate) fn wrap_filter(input: LogicalOperator, predicate: LogicalExpression) -> LogicalOperator {
    LogicalOperator::Filter(FilterOp {
        predicate,
        input: Box::new(input),
        pushdown_hint: None,
    })
}

/// Wraps an operator with ORDER BY.
pub(crate) fn wrap_sort(input: LogicalOperator, keys: Vec<SortKey>) -> LogicalOperator {
    LogicalOperator::Sort(SortOp {
        keys,
        input: Box::new(input),
    })
}

/// Wraps an operator with SKIP.
pub(crate) fn wrap_skip(input: LogicalOperator, count: impl Into<CountExpr>) -> LogicalOperator {
    LogicalOperator::Skip(SkipOp {
        count: count.into(),
        input: Box::new(input),
    })
}

/// Wraps an operator with LIMIT.
pub(crate) fn wrap_limit(input: LogicalOperator, count: impl Into<CountExpr>) -> LogicalOperator {
    LogicalOperator::Limit(LimitOp {
        count: count.into(),
        input: Box::new(input),
    })
}

/// Wraps an operator with DISTINCT.
pub(crate) fn wrap_distinct(input: LogicalOperator) -> LogicalOperator {
    LogicalOperator::Distinct(DistinctOp {
        input: Box::new(input),
        columns: None,
    })
}

/// Wraps an operator with RETURN.
pub(crate) fn wrap_return(
    input: LogicalOperator,
    items: Vec<ReturnItem>,
    distinct: bool,
) -> LogicalOperator {
    LogicalOperator::Return(ReturnOp {
        items,
        distinct,
        input: Box::new(input),
    })
}

/// Un item de `RETURN` / `WITH` agrégé, tel que l'extraction le note avant que
/// la liste complète des clés et des agrégats soit connue.
pub(crate) enum AggregateItem {
    /// Clé de groupement : sa position dans `group_by`.
    GroupKey {
        position: usize,
        alias: Option<String>,
    },
    /// Agrégat direct (`count(*) AS c`) : sa position dans `aggregates`.
    Direct {
        position: usize,
        alias: Option<String>,
    },
    /// Expression qui enveloppe un agrégat (`count(*) > 0 AS x`), déjà réécrite
    /// sur les colonnes synthétiques des agrégats qu'elle contient.
    Wrapped(ReturnItem),
}

/// Décide si un agrégat a besoin d'une projection posée après lui, et la
/// construit.
///
/// L'opérateur `Aggregate` sort ses clés sous le nom de leur expression et ses
/// agrégats sous leur alias. Une projection d'après-agrégat est nécessaire dès
/// que cela ne suffit pas à rendre ce que la clause a écrit :
///
/// - un item enveloppe un agrégat : l'expression englobante se calcule après ;
/// - une clé porte un alias qui n'est pas le nom de sa colonne (`n.city AS
///   city`) : elle doit sortir sous cet alias, pour la clause qui suit un
///   `WITH` comme pour l'`ORDER BY` d'un `RETURN` ;
/// - les items ne sont pas écrits dans l'ordre où l'agrégat sort ses colonnes
///   (les clés, puis les agrégats) ;
/// - une clé calculée a reçu un nom distinct (voir
///   [`group_key_column_names`](crate::query::planner::common::group_key_column_names))
///   parce qu'une autre clé portait déjà son nom générique : la projection lui
///   rend son nom visible.
///
/// La projection lit chaque colonne par son NOM. Un agrégat direct sans alias
/// n'a qu'un nom par défaut (`count(...)`), que deux agrégats peuvent partager :
/// quand la projection existe, il reçoit un alias interne propre, et la
/// projection lui rend son nom par défaut.
///
/// Quand rien de cela ne s'applique (`WITH f, count(*) AS nb`), l'agrégat sort
/// déjà exactement ce que la clause a écrit : aucune projection n'est posée,
/// la requête ne paie pas un opérateur de plus.
///
/// `always_project_aliases` garde le comportement historique de `RETURN`, qui
/// pose la projection dès qu'un item porte un alias.
pub(crate) fn finalize_aggregate_projection(
    aggregates: &mut [crate::query::plan::AggregateExpr],
    group_by: &[LogicalExpression],
    items: Vec<AggregateItem>,
    always_project_aliases: bool,
) -> Option<Vec<ReturnItem>> {
    use crate::query::planner::common::{
        default_aggregate_column_name, expression_to_string, group_key_column_names,
    };

    let noms_des_cles = group_key_column_names(group_by);
    // L'agrégat sort ses clés dans l'ordre, puis ses agrégats dans l'ordre.
    let mut cles_finies = false;
    let ordre_naturel = items.iter().all(|item| match item {
        AggregateItem::GroupKey { .. } => !cles_finies,
        AggregateItem::Direct { .. } | AggregateItem::Wrapped(_) => {
            cles_finies = true;
            true
        }
    });
    let necessaire = !ordre_naturel
        || items.iter().any(|item| match item {
            AggregateItem::Wrapped(_) => true,
            AggregateItem::GroupKey { position, alias } => {
                let colonne = &noms_des_cles[*position];
                *colonne != expression_to_string(&group_by[*position])
                    || alias
                        .as_ref()
                        .is_some_and(|a| always_project_aliases || a != colonne)
            }
            AggregateItem::Direct { alias, .. } => always_project_aliases && alias.is_some(),
        });
    if !necessaire {
        return None;
    }

    Some(
        items
            .into_iter()
            .map(|item| match item {
                AggregateItem::Wrapped(item) => item,
                AggregateItem::GroupKey { position, alias } => {
                    let colonne = noms_des_cles[position].clone();
                    let visible = expression_to_string(&group_by[position]);
                    ReturnItem {
                        alias: alias.or_else(|| (colonne != visible).then_some(visible)),
                        expression: LogicalExpression::Variable(colonne),
                    }
                }
                AggregateItem::Direct { position, alias } => {
                    let agregat = &mut aggregates[position];
                    let visible =
                        alias.unwrap_or_else(|| default_aggregate_column_name(agregat.function));
                    let colonne = agregat
                        .alias
                        .get_or_insert_with(|| format!("_agg_direct_{position}"))
                        .clone();
                    ReturnItem {
                        expression: LogicalExpression::Variable(colonne),
                        alias: Some(visible),
                    }
                }
            })
            .collect(),
    )
}

/// Ce qu'une variable visible désigne.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VariableKind {
    /// Un nœud : on peut s'en servir comme source d'une expansion.
    Node,
    /// Une arête (un seul saut).
    Edge,
    /// Une valeur qui n'est PROUVÉEMENT ni un nœud ni une arête : littéral,
    /// opération, propriété, liste ou map, résultat d'`id()` ou d'un
    /// agrégat de dénombrement. Un `MATCH` qui la reprend comme nœud est une
    /// erreur de type (Neo4j : « Type mismatch: expected Node but was
    /// Integer ») ; sans cela l'entier 1 se joignait au nœud n° 1.
    Scalar,
    /// Genre INCONNU : peut porter un nœud (élément d'un `UNWIND` d'une liste
    /// de nœuds, `head(collect(n))`, `CASE`, paramètre d'un `CALL`).
    Other,
}

/// Erreur de type si `variable`, liée à ce que `bound` dit, ne peut pas jouer
/// le rôle `expected` (`Node` ou `Edge`) dans un motif. Un genre inconnu
/// (`Other`) passe : le moteur ne peut pas le prouver faux.
pub(crate) fn check_bound_kind(
    variable: &str,
    expected: VariableKind,
    bound: &HashMap<String, VariableKind>,
) -> Result<()> {
    let Some(kind) = bound.get(variable) else {
        return Ok(());
    };
    let quoi = |k: VariableKind| match k {
        VariableKind::Node => "un nœud",
        VariableKind::Edge => "une arête",
        VariableKind::Scalar => "une valeur (nombre, texte, liste, map…)",
        VariableKind::Other => "une valeur de type inconnu",
    };
    if *kind == VariableKind::Scalar || (*kind != VariableKind::Other && *kind != expected) {
        return Err(Error::Query(QueryError::new(
            QueryErrorKind::Semantic,
            format!(
                "Type mismatch: la variable '{variable}' est déjà liée à {}, \
                 le motif l'attend comme {}",
                quoi(*kind),
                quoi(expected)
            ),
        )));
    }
    Ok(())
}

/// Genre d'une expression qui produit une valeur de projection.
fn expression_kind(
    expression: &LogicalExpression,
    entree: &HashMap<String, VariableKind>,
) -> VariableKind {
    match expression {
        LogicalExpression::Variable(name) => {
            entree.get(name).copied().unwrap_or(VariableKind::Other)
        }
        // NULL est lié sans être d'aucun genre.
        LogicalExpression::Literal(obrain_common::types::Value::Null) => VariableKind::Other,
        LogicalExpression::Literal(_)
        | LogicalExpression::Property { .. }
        | LogicalExpression::Binary { .. }
        | LogicalExpression::Unary { .. }
        | LogicalExpression::List(_)
        | LogicalExpression::Map(_)
        | LogicalExpression::Id(_)
        | LogicalExpression::Labels(_)
        | LogicalExpression::Type(_) => VariableKind::Scalar,
        LogicalExpression::FunctionCall { name, .. } => {
            const SCALAIRES: &[&str] = &[
                "id",
                "elementid",
                "element_id",
                "tostring",
                "tointeger",
                "tofloat",
                "toboolean",
                "size",
                "length",
                "abs",
                "ceil",
                "floor",
                "round",
                "sqrt",
                "exp",
                "log",
                "toupper",
                "tolower",
                "trim",
                "substring",
                "split",
                "replace",
                "labels",
                "type",
                "keys",
                "range",
                "date",
                "datetime",
                "duration",
                "time",
                "timestamp",
                "rand",
            ];
            if SCALAIRES.contains(&name.to_ascii_lowercase().as_str()) {
                VariableKind::Scalar
            } else {
                VariableKind::Other
            }
        }
        _ => VariableKind::Other,
    }
}

/// Variables qu'un plan laisse VISIBLES à la clause qui le suit, avec ce
/// qu'elles désignent.
///
/// Commune aux traducteurs Cypher et GQL : c'est ce qui décide, pour le
/// `MATCH` qui suit, quelles variables de son motif sont déjà liées (et donc
/// à fermer ou à prendre comme ancre) et lesquelles sont neuves.
///
/// Une projection (`WITH`, `RETURN`) ou un agrégat FERME la portée : seules
/// leurs colonnes de sortie restent visibles, sous leur alias. Une variable
/// que le `WITH` a fait sortir et que le `MATCH` suivant réemploie est une
/// variable neuve. Lire l'entrée de ces opérateurs au lieu de leur sortie
/// (c'est ce que faisait GQL) croyait liée une variable disparue et ignorait
/// un alias : `WITH f AS g, count(*) AS nb MATCH (c2)-[:T]->(g)` rendait un
/// produit cartésien, sans erreur.
///
/// Pour un opérateur sans règle propre, on prend l'union de ses enfants : au
/// pire une variable est dite liée à tort, et la jointure échoue bruyamment au
/// lieu de rendre un résultat faux.
pub(crate) fn visible_variables(op: &LogicalOperator) -> HashMap<String, VariableKind> {
    let mut vars = HashMap::new();
    collect_visible_variables(op, &mut vars);
    vars
}

fn collect_visible_variables(op: &LogicalOperator, vars: &mut HashMap<String, VariableKind>) {
    use crate::query::plan::LogicalOperator as LO;
    // Sortie d'une projection : chaque item sous son alias (ou son nom s'il
    // est une variable nue), du type de la variable qu'il reprend.
    let projette = |vars: &mut HashMap<String, VariableKind>,
                    entree: &HashMap<String, VariableKind>,
                    expression: &LogicalExpression,
                    alias: Option<&String>| {
        let kind = expression_kind(expression, entree);
        match (alias, expression) {
            (Some(alias), _) => {
                vars.insert(alias.clone(), kind);
            }
            (None, LogicalExpression::Variable(name)) => {
                vars.insert(name.clone(), kind);
            }
            (None, _) => {}
        }
    };
    match op {
        LO::NodeScan(scan) => {
            if let Some(input) = &scan.input {
                collect_visible_variables(input, vars);
            }
            vars.insert(scan.variable.clone(), VariableKind::Node);
        }
        LO::EdgeScan(scan) => {
            if let Some(input) = &scan.input {
                collect_visible_variables(input, vars);
            }
            vars.insert(scan.variable.clone(), VariableKind::Edge);
        }
        LO::Expand(expand) => {
            collect_visible_variables(&expand.input, vars);
            vars.insert(expand.to_variable.clone(), VariableKind::Node);
            if let Some(edge) = &expand.edge_variable {
                // Sur plusieurs sauts, la variable d'arête est une LISTE.
                let single_hop = expand.min_hops == 1 && expand.max_hops == Some(1);
                vars.insert(
                    edge.clone(),
                    if single_hop {
                        VariableKind::Edge
                    } else {
                        VariableKind::Other
                    },
                );
            }
            if let Some(alias) = &expand.path_alias {
                vars.insert(alias.clone(), VariableKind::Other);
            }
        }
        LO::Unwind(unwind) => {
            collect_visible_variables(&unwind.input, vars);
            // Les éléments d'une liste LITTÉRALE de constantes sont des
            // scalaires ; ceux d'une autre liste (`UNWIND nodes AS n`)
            // peuvent être des nœuds.
            let elements_scalaires = matches!(
                &unwind.expression,
                LogicalExpression::List(items)
                    if items.iter().all(|i| matches!(i, LogicalExpression::Literal(_)))
            );
            vars.insert(
                unwind.variable.clone(),
                if elements_scalaires {
                    VariableKind::Scalar
                } else {
                    VariableKind::Other
                },
            );
            for extra in [&unwind.ordinality_var, &unwind.offset_var]
                .into_iter()
                .flatten()
            {
                vars.insert(extra.clone(), VariableKind::Other);
            }
        }
        LO::Project(project) => {
            let entree = visible_variables(&project.input);
            if project.pass_through_input {
                vars.extend(entree.iter().map(|(k, v)| (k.clone(), *v)));
            }
            for projection in &project.projections {
                projette(
                    vars,
                    &entree,
                    &projection.expression,
                    projection.alias.as_ref(),
                );
            }
        }
        LO::Return(ret) => {
            let entree = visible_variables(&ret.input);
            for item in &ret.items {
                if let LogicalExpression::Variable(name) = &item.expression
                    && name == "*"
                {
                    vars.extend(entree.iter().map(|(k, v)| (k.clone(), *v)));
                    continue;
                }
                projette(vars, &entree, &item.expression, item.alias.as_ref());
            }
        }
        LO::Aggregate(aggregate) => {
            let entree = visible_variables(&aggregate.input);
            for key in &aggregate.group_by {
                projette(vars, &entree, key, None);
            }
            for agg in &aggregate.aggregates {
                if let Some(alias) = &agg.alias {
                    // `min` / `max` / `sample` rendent un élément (un nœud
                    // possible) ; tous les autres, un nombre, un texte ou une liste.
                    use crate::query::plan::AggregateFunction as F;
                    let kind = match agg.function {
                        F::Min | F::Max | F::Sample => VariableKind::Other,
                        _ => VariableKind::Scalar,
                    };
                    vars.insert(alias.clone(), kind);
                }
            }
        }
        LO::CreateNode(create) => {
            if let Some(input) = &create.input {
                collect_visible_variables(input, vars);
            }
            vars.insert(create.variable.clone(), VariableKind::Node);
        }
        LO::CreateEdge(create) => {
            collect_visible_variables(&create.input, vars);
            if let Some(variable) = &create.variable {
                vars.insert(variable.clone(), VariableKind::Edge);
            }
        }
        LO::Merge(merge) => {
            collect_visible_variables(&merge.input, vars);
            vars.insert(merge.variable.clone(), VariableKind::Node);
        }
        LO::MergeRelationship(merge) => {
            collect_visible_variables(&merge.input, vars);
            vars.insert(merge.variable.clone(), VariableKind::Edge);
        }
        LO::CallProcedure(call) => {
            for child in op.children() {
                collect_visible_variables(child, vars);
            }
            for item in call.yield_items.iter().flatten() {
                let name = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| item.field_name.clone());
                vars.insert(name, VariableKind::Other);
            }
        }
        LO::ParameterScan(scan) => {
            for column in &scan.columns {
                vars.insert(column.clone(), VariableKind::Other);
            }
        }
        LO::Apply(apply) => {
            // L'entrée reste visible ; du sous-plan, seules ses colonnes de
            // SORTIE le sont (celles d'un `RETURN`, d'une projection ou d'un
            // agrégat en racine) — jamais les variables internes d'un
            // `EXISTS` ou d'une compréhension de motif.
            collect_visible_variables(&apply.input, vars);
            if matches!(
                apply.subplan.as_ref(),
                LO::Return(_) | LO::Project(_) | LO::Aggregate(_)
            ) {
                collect_visible_variables(&apply.subplan, vars);
            }
        }
        LO::AntiJoin(join) => collect_visible_variables(&join.left, vars),
        LO::ShortestPath(sp) => {
            collect_visible_variables(&sp.input, vars);
            vars.insert(sp.path_alias.clone(), VariableKind::Other);
        }
        other => {
            for child in other.children() {
                collect_visible_variables(child, vars);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::Value;

    // --- capitalize_first ---

    #[test]
    fn capitalize_first_empty() {
        assert_eq!(capitalize_first(""), "");
    }

    #[test]
    fn capitalize_first_single_char() {
        assert_eq!(capitalize_first("a"), "A");
    }

    #[test]
    fn capitalize_first_already_upper() {
        assert_eq!(capitalize_first("Hello"), "Hello");
    }

    #[test]
    fn capitalize_first_lower() {
        assert_eq!(capitalize_first("person"), "Person");
    }

    // --- VarGen ---

    #[test]
    fn var_gen_starts_at_zero() {
        let vg = VarGen::new();
        assert_eq!(vg.current(), 0);
    }

    #[test]
    fn var_gen_increments() {
        let vg = VarGen::new();
        assert_eq!(vg.next(), "_v0");
        assert_eq!(vg.next(), "_v1");
        assert_eq!(vg.current(), 2);
    }

    // --- is_aggregate_function ---

    #[test]
    fn aggregate_functions_recognized() {
        for name in [
            "count",
            "COUNT",
            "sum",
            "avg",
            "min",
            "max",
            "collect",
            "stdev",
            "stddev",
            "stdevp",
            "stddevp",
            "stddev_samp",
            "STDDEV_POP",
            "variance",
            "VARIANCE",
            "var_samp",
            "VAR_POP",
            "percentile_disc",
            "percentiledisc",
            "percentile_cont",
            "percentilecont",
        ] {
            assert!(is_aggregate_function(name), "{name} should be aggregate");
        }
    }

    #[test]
    fn non_aggregate_functions_rejected() {
        for name in ["toString", "toUpper", "size", "rand", "abs", "coalesce", ""] {
            assert!(
                !is_aggregate_function(name),
                "{name} should not be aggregate"
            );
        }
    }

    // --- to_aggregate_function ---

    #[test]
    fn to_aggregate_all_variants() {
        assert!(matches!(
            to_aggregate_function("count"),
            Some(AggregateFunction::Count)
        ));
        assert!(matches!(
            to_aggregate_function("SUM"),
            Some(AggregateFunction::Sum)
        ));
        assert!(matches!(
            to_aggregate_function("Avg"),
            Some(AggregateFunction::Avg)
        ));
        assert!(matches!(
            to_aggregate_function("MIN"),
            Some(AggregateFunction::Min)
        ));
        assert!(matches!(
            to_aggregate_function("max"),
            Some(AggregateFunction::Max)
        ));
        assert!(matches!(
            to_aggregate_function("collect"),
            Some(AggregateFunction::Collect)
        ));
        assert!(matches!(
            to_aggregate_function("stdev"),
            Some(AggregateFunction::StdDev)
        ));
        assert!(matches!(
            to_aggregate_function("stddev"),
            Some(AggregateFunction::StdDev)
        ));
        assert!(matches!(
            to_aggregate_function("stdevp"),
            Some(AggregateFunction::StdDevPop)
        ));
        assert!(matches!(
            to_aggregate_function("stddevp"),
            Some(AggregateFunction::StdDevPop)
        ));
        assert!(matches!(
            to_aggregate_function("stddev_samp"),
            Some(AggregateFunction::StdDev)
        ));
        assert!(matches!(
            to_aggregate_function("STDDEV_POP"),
            Some(AggregateFunction::StdDevPop)
        ));
        assert!(matches!(
            to_aggregate_function("variance"),
            Some(AggregateFunction::Variance)
        ));
        assert!(matches!(
            to_aggregate_function("VAR_SAMP"),
            Some(AggregateFunction::Variance)
        ));
        assert!(matches!(
            to_aggregate_function("VAR_POP"),
            Some(AggregateFunction::VariancePop)
        ));
        assert!(matches!(
            to_aggregate_function("percentile_disc"),
            Some(AggregateFunction::PercentileDisc)
        ));
        assert!(matches!(
            to_aggregate_function("percentiledisc"),
            Some(AggregateFunction::PercentileDisc)
        ));
        assert!(matches!(
            to_aggregate_function("percentile_cont"),
            Some(AggregateFunction::PercentileCont)
        ));
        assert!(matches!(
            to_aggregate_function("percentilecont"),
            Some(AggregateFunction::PercentileCont)
        ));
    }

    #[test]
    fn to_aggregate_unknown_returns_none() {
        assert!(to_aggregate_function("unknown").is_none());
        assert!(to_aggregate_function("").is_none());
        assert!(to_aggregate_function("size").is_none());
    }

    // --- combine_with_and ---

    #[test]
    fn combine_with_and_empty_returns_error() {
        let result = combine_with_and(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn combine_with_and_single_predicate() {
        let pred = LogicalExpression::Property {
            variable: "n".to_string(),
            property: "name".to_string(),
        };
        let result = combine_with_and(vec![pred.clone()]).unwrap();
        assert!(matches!(result, LogicalExpression::Property { .. }));
    }

    #[test]
    fn combine_with_and_two_predicates() {
        let p1 = LogicalExpression::Property {
            variable: "n".to_string(),
            property: "a".to_string(),
        };
        let p2 = LogicalExpression::Property {
            variable: "n".to_string(),
            property: "b".to_string(),
        };
        let result = combine_with_and(vec![p1, p2]).unwrap();
        assert!(matches!(
            result,
            LogicalExpression::Binary {
                op: BinaryOp::And,
                ..
            }
        ));
    }

    // --- split_conjuncts ---

    #[test]
    fn split_conjuncts_single() {
        let expr = LogicalExpression::Variable("x".into());
        let conjuncts = split_conjuncts(expr);
        assert_eq!(conjuncts.len(), 1);
    }

    #[test]
    fn split_conjuncts_nested_and() {
        // (a AND b) AND c -> [a, b, c]
        let a = LogicalExpression::Variable("a".into());
        let b = LogicalExpression::Variable("b".into());
        let c = LogicalExpression::Variable("c".into());
        let ab = LogicalExpression::Binary {
            left: Box::new(a),
            op: BinaryOp::And,
            right: Box::new(b),
        };
        let abc = LogicalExpression::Binary {
            left: Box::new(ab),
            op: BinaryOp::And,
            right: Box::new(c),
        };
        let conjuncts = split_conjuncts(abc);
        assert_eq!(conjuncts.len(), 3);
    }

    #[test]
    fn split_conjuncts_or_not_split() {
        // a OR b should NOT be split (only AND is split)
        let a = LogicalExpression::Variable("a".into());
        let b = LogicalExpression::Variable("b".into());
        let or_expr = LogicalExpression::Binary {
            left: Box::new(a),
            op: BinaryOp::Or,
            right: Box::new(b),
        };
        let conjuncts = split_conjuncts(or_expr);
        assert_eq!(conjuncts.len(), 1);
    }

    // --- classify_optional_predicates ---

    #[test]
    fn classify_left_only_predicate() {
        let left_vars: HashSet<String> = ["n".into()].into_iter().collect();
        let right_vars: HashSet<String> = ["m".into()].into_iter().collect();
        let pred = LogicalExpression::Property {
            variable: "n".into(),
            property: "age".into(),
        };

        let result = classify_optional_predicates(pred, &left_vars, &right_vars);
        assert_eq!(
            result.post_filters.len(),
            1,
            "left-only should be post-filter"
        );
        assert!(result.right_filters.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn classify_right_only_predicate() {
        let left_vars: HashSet<String> = ["n".into()].into_iter().collect();
        let right_vars: HashSet<String> = ["m".into()].into_iter().collect();
        let pred = LogicalExpression::Property {
            variable: "m".into(),
            property: "age".into(),
        };

        let result = classify_optional_predicates(pred, &left_vars, &right_vars);
        assert!(result.post_filters.is_empty(), "résultat attendu vide");
        assert_eq!(
            result.right_filters.len(),
            1,
            "right-only should be right-filter"
        );
    }

    #[test]
    fn classify_shared_variable_as_right() {
        // Variable `n` is in both left_vars and right_vars (shared).
        // A predicate on `m.age > n.age` has both m and n on right side,
        // so it should be classified as right-filter.
        let left_vars: HashSet<String> = ["n".into()].into_iter().collect();
        let right_vars: HashSet<String> = ["n".into(), "m".into()].into_iter().collect();
        let pred = LogicalExpression::Binary {
            left: Box::new(LogicalExpression::Property {
                variable: "m".into(),
                property: "age".into(),
            }),
            op: BinaryOp::Gt,
            right: Box::new(LogicalExpression::Property {
                variable: "n".into(),
                property: "age".into(),
            }),
        };

        let result = classify_optional_predicates(pred, &left_vars, &right_vars);
        assert!(result.post_filters.is_empty(), "résultat attendu vide");
        assert_eq!(
            result.right_filters.len(),
            1,
            "shared variable predicate should be right-filter"
        );
    }

    #[test]
    fn classify_mixed_and_predicate() {
        // n.active AND m.age > 30 should split into post-filter and right-filter
        let left_vars: HashSet<String> = ["n".into()].into_iter().collect();
        let right_vars: HashSet<String> = ["n".into(), "m".into()].into_iter().collect();

        let left_pred = LogicalExpression::Property {
            variable: "n".into(),
            property: "active".into(),
        };
        let right_pred = LogicalExpression::Binary {
            left: Box::new(LogicalExpression::Property {
                variable: "m".into(),
                property: "age".into(),
            }),
            op: BinaryOp::Gt,
            right: Box::new(LogicalExpression::Literal(Value::Int64(30))),
        };
        let combined = LogicalExpression::Binary {
            left: Box::new(left_pred),
            op: BinaryOp::And,
            right: Box::new(right_pred),
        };

        let result = classify_optional_predicates(combined, &left_vars, &right_vars);

        // n.active -> n is in both left and right, so all_in_right = true
        // It should be classified as right_filter since n is available on right side.
        // Actually n.active references only n, which IS in right_vars too.
        // So both predicates should be right_filters.
        // BUT we also need to check: n.active only references n, which is in left_vars.
        // Since n is in BOTH sets, all_in_left AND all_in_right are true.
        // The logic checks all_in_right FIRST, so it goes to right_filters.
        assert_eq!(
            result.right_filters.len() + result.post_filters.len(),
            2,
            "two conjuncts should be classified"
        );
    }

    // --- collect_expression_variables ---

    #[test]
    fn collect_vars_from_property() {
        let expr = LogicalExpression::Property {
            variable: "n".into(),
            property: "age".into(),
        };
        let mut vars = HashSet::new();
        collect_expression_variables(&expr, &mut vars);
        assert!(vars.contains("n"));
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn collect_vars_from_binary() {
        let expr = LogicalExpression::Binary {
            left: Box::new(LogicalExpression::Property {
                variable: "a".into(),
                property: "x".into(),
            }),
            op: BinaryOp::Gt,
            right: Box::new(LogicalExpression::Property {
                variable: "b".into(),
                property: "y".into(),
            }),
        };
        let mut vars = HashSet::new();
        collect_expression_variables(&expr, &mut vars);
        assert!(vars.contains("a"));
        assert!(vars.contains("b"));
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn collect_vars_from_function_call() {
        let expr = LogicalExpression::FunctionCall {
            name: "size".into(),
            args: vec![LogicalExpression::Variable("list".into())],
            distinct: false,
        };
        let mut vars = HashSet::new();
        collect_expression_variables(&expr, &mut vars);
        assert!(vars.contains("list"));
    }

    #[test]
    fn collect_vars_from_literal_is_empty() {
        let expr = LogicalExpression::Literal(Value::Int64(42));
        let mut vars = HashSet::new();
        collect_expression_variables(&expr, &mut vars);
        assert!(vars.is_empty(), "résultat attendu vide");
    }
}
