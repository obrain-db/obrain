//! Aggregate extraction from RETURN items.

#[allow(clippy::wildcard_imports)]
use super::*;

impl GqlTranslator {
    /// Extracts aggregate and group-by expressions from RETURN items.
    ///
    /// Returns `(aggregates, group_by, post_return)` where `post_return` is
    /// `Some(...)` when any return item wraps an aggregate in a binary/unary
    /// expression (e.g. `count(n) > 0 AS exists`).
    pub(super) fn extract_aggregates_and_groups(
        &self,
        items: &[ast::ReturnItem],
    ) -> Result<(
        Vec<AggregateExpr>,
        Vec<LogicalExpression>,
        Option<Vec<ReturnItem>>,
    )> {
        let mut aggregates = Vec::new();
        let mut group_by = Vec::new();
        let mut needs_post_return = false;
        let mut post_return_items = Vec::new();
        let mut agg_counter: u32 = 0;

        for item in items {
            if let Some(agg_expr) = self.try_extract_aggregate(&item.expression, &item.alias)? {
                // Direct aggregate (e.g. `count(n) AS cnt`)
                aggregates.push(agg_expr);
                let agg_alias = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("_agg_{agg_counter}"));
                post_return_items.push(ReturnItem {
                    expression: LogicalExpression::Variable(agg_alias),
                    alias: item.alias.clone(),
                });
                agg_counter += 1;
            } else if contains_aggregate(&item.expression) {
                // Wrapped aggregate (e.g. `count(n) > 0 AS exists`)
                needs_post_return = true;
                let synthetic_alias = format!("_agg_{agg_counter}");
                agg_counter += 1;

                let mut item_aggs = Vec::new();
                let substitute = self.extract_wrapped_aggregates_into(
                    &item.expression,
                    &synthetic_alias,
                    &mut item_aggs,
                )?;
                aggregates.append(&mut item_aggs);
                post_return_items.push(ReturnItem {
                    expression: substitute,
                    alias: item.alias.clone(),
                });
            } else {
                // Non-aggregate expression: group-by key.
                // The Aggregate operator names its output columns using
                // expression_to_string, so the post-Return must reference
                // those column names (not the raw property expression).
                let expr = self.translate_expression(&item.expression)?;
                group_by.push(expr.clone());
                let col_name = crate::query::planner::common::expression_to_string(&expr);
                post_return_items.push(ReturnItem {
                    expression: LogicalExpression::Variable(col_name),
                    alias: item.alias.clone(),
                });
            }
        }

        // Always produce a post-Return when any item has an alias, so that
        // output column names reflect the aliases and are visible to ORDER BY.
        let has_aliases = items.iter().any(|item| item.alias.is_some());
        if needs_post_return || has_aliases {
            Ok((aggregates, group_by, Some(post_return_items)))
        } else {
            Ok((aggregates, group_by, None))
        }
    }

    /// Extracts EVERY aggregate contained in a wrapping expression.
    ///
    /// Chaque agrégat rencontré est poussé dans `aggs` et substitué, dans
    /// l'expression post-agrégat rendue, par la variable de sa colonne de
    /// sortie. Le premier agrégat reçoit `synthetic_alias` tel quel
    /// (comportement historique, aucune sortie existante ne change), les
    /// suivants `{synthetic_alias}_{n}`.
    ///
    /// C'est ce qui permet à `sum(x) / count(x)` de plannifier ses DEUX
    /// agrégats : l'ancienne version n'extrayait que le premier et
    /// traduisait l'autre côté en simple FunctionCall, que le post-Return
    /// évaluait en NULL.
    pub(super) fn extract_wrapped_aggregates_into(
        &self,
        expr: &ast::Expression,
        synthetic_alias: &str,
        aggs: &mut Vec<AggregateExpr>,
    ) -> Result<LogicalExpression> {
        match expr {
            ast::Expression::FunctionCall {
                name,
                args,
                distinct,
            } => {
                // First, check if this function itself is an aggregate
                let alias = if aggs.is_empty() {
                    synthetic_alias.to_string()
                } else {
                    format!("{synthetic_alias}_{}", aggs.len())
                };
                if let Some(agg) = self.try_extract_aggregate(expr, &Some(alias.clone()))? {
                    aggs.push(agg);
                    return Ok(LogicalExpression::Variable(alias));
                }
                // Not an aggregate (e.g. COALESCE, toString, etc.) — recurse
                // into every argument containing a nested aggregate.
                if !args.iter().any(contains_aggregate) {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!(
                            "contains_aggregate was true but no aggregate found in arguments of {name}"
                        ),
                    )));
                }
                let mut translated_args = Vec::with_capacity(args.len());
                for arg in args {
                    if contains_aggregate(arg) {
                        translated_args.push(self.extract_wrapped_aggregates_into(
                            arg,
                            synthetic_alias,
                            aggs,
                        )?);
                    } else {
                        translated_args.push(self.translate_expression(arg)?);
                    }
                }
                Ok(LogicalExpression::FunctionCall {
                    name: name.clone(),
                    args: translated_args,
                    distinct: *distinct,
                })
            }
            ast::Expression::Binary { left, op, right } => {
                let binary_op = self.translate_binary_op(*op);
                // Chaque côté est extrait s'il contient un agrégat — y
                // compris les DEUX à la fois (`sum(x) / count(x)`).
                let left_sub = if contains_aggregate(left) {
                    self.extract_wrapped_aggregates_into(left, synthetic_alias, aggs)?
                } else {
                    self.translate_expression(left)?
                };
                let right_sub = if contains_aggregate(right) {
                    self.extract_wrapped_aggregates_into(right, synthetic_alias, aggs)?
                } else {
                    self.translate_expression(right)?
                };
                Ok(LogicalExpression::Binary {
                    left: Box::new(left_sub),
                    op: binary_op,
                    right: Box::new(right_sub),
                })
            }
            ast::Expression::Unary { op, operand } => {
                let sub = self.extract_wrapped_aggregates_into(operand, synthetic_alias, aggs)?;
                // Unary positive is identity: just return the operand
                if *op == ast::UnaryOp::Pos {
                    return Ok(sub);
                }
                let unary_op = self.translate_unary_op(*op);
                Ok(LogicalExpression::Unary {
                    op: unary_op,
                    operand: Box::new(sub),
                })
            }
            ast::Expression::Case {
                input,
                whens,
                else_clause,
            } => {
                // Find the first aggregate inside the CASE branches and extract it.
                // (Comportement historique conservé : les agrégats du CASE sont
                // enregistrés mais l'expression rendue est le CASE traduit tel
                // quel.)
                for (cond, then) in whens {
                    if contains_aggregate(cond) {
                        let _ = self.extract_wrapped_aggregates_into(cond, synthetic_alias, aggs)?;
                        return self.translate_expression(expr);
                    }
                    if contains_aggregate(then) {
                        let _ = self.extract_wrapped_aggregates_into(then, synthetic_alias, aggs)?;
                        return self.translate_expression(expr);
                    }
                }
                if let Some(el) = else_clause
                    && contains_aggregate(el)
                {
                    let _ = self.extract_wrapped_aggregates_into(el, synthetic_alias, aggs)?;
                    return self.translate_expression(expr);
                }
                if let Some(inp) = input
                    && contains_aggregate(inp)
                {
                    let _ = self.extract_wrapped_aggregates_into(inp, synthetic_alias, aggs)?;
                    return self.translate_expression(expr);
                }
                Err(Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    "Unsupported expression wrapping an aggregate",
                )))
            }
            _ => Err(Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                "Unsupported expression wrapping an aggregate",
            ))),
        }
    }

    /// Tries to extract an aggregate expression from an AST expression.
    pub(super) fn try_extract_aggregate(
        &self,
        expr: &ast::Expression,
        alias: &Option<String>,
    ) -> Result<Option<AggregateExpr>> {
        match expr {
            ast::Expression::FunctionCall {
                name,
                args,
                distinct,
            } => {
                if let Some(func) = to_aggregate_function(name) {
                    let agg_expr = if args.is_empty() {
                        // COUNT(*) case
                        AggregateExpr {
                            function: func,
                            expression: None,
                            expression2: None,
                            distinct: *distinct,
                            alias: alias.clone(),
                            percentile: None,
                            separator: None,
                        }
                    } else {
                        // COUNT(x), SUM(x), etc.
                        // For COUNT with an expression, use CountNonNull to ensure we fetch values
                        let actual_func = if func == AggregateFunction::Count {
                            AggregateFunction::CountNonNull
                        } else {
                            func
                        };
                        // Extract percentile parameter for percentile functions
                        let percentile = if matches!(
                            actual_func,
                            AggregateFunction::PercentileDisc | AggregateFunction::PercentileCont
                        ) && args.len() >= 2
                        {
                            // Second argument is the percentile value
                            if let ast::Expression::Literal(ast::Literal::Float(p)) = &args[1] {
                                Some((*p).clamp(0.0, 1.0))
                            } else if let ast::Expression::Literal(ast::Literal::Integer(p)) =
                                &args[1]
                            {
                                Some((*p as f64).clamp(0.0, 1.0))
                            } else {
                                Some(0.5) // Default to median
                            }
                        } else {
                            None
                        };
                        // Extract second argument for binary set functions
                        let expression2 = if is_binary_set_function(actual_func) && args.len() >= 2
                        {
                            Some(self.translate_expression(&args[1])?)
                        } else {
                            None
                        };
                        // Extract separator for LISTAGG / GROUP_CONCAT
                        let upper_name = name.to_uppercase();
                        let separator = if actual_func == AggregateFunction::GroupConcat {
                            if args.len() >= 2 {
                                // Second argument is the separator string
                                if let ast::Expression::Literal(ast::Literal::String(s)) = &args[1]
                                {
                                    Some(s.clone())
                                } else if upper_name == "LISTAGG" {
                                    Some(",".to_string())
                                } else {
                                    None // GROUP_CONCAT default (space) handled in AggregateState
                                }
                            } else if upper_name == "LISTAGG" {
                                Some(",".to_string()) // ISO GQL default for LISTAGG
                            } else {
                                None // GROUP_CONCAT default (space) handled in AggregateState
                            }
                        } else {
                            None
                        };
                        AggregateExpr {
                            function: actual_func,
                            expression: Some(self.translate_expression(&args[0])?),
                            expression2,
                            distinct: *distinct,
                            alias: alias.clone(),
                            percentile,
                            separator,
                        }
                    };
                    Ok(Some(agg_expr))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }
}

/// Checks if an AST expression contains an aggregate function call.
pub(super) fn contains_aggregate(expr: &ast::Expression) -> bool {
    match expr {
        ast::Expression::FunctionCall { name, args, .. } => {
            is_aggregate_function(name) || args.iter().any(contains_aggregate)
        }
        ast::Expression::Binary { left, right, .. } => {
            contains_aggregate(left) || contains_aggregate(right)
        }
        ast::Expression::Unary { operand, .. } => contains_aggregate(operand),
        ast::Expression::Case {
            input,
            whens,
            else_clause,
        } => {
            input.as_deref().is_some_and(contains_aggregate)
                || whens
                    .iter()
                    .any(|(w, t)| contains_aggregate(w) || contains_aggregate(t))
                || else_clause.as_deref().is_some_and(contains_aggregate)
        }
        ast::Expression::List(items) => items.iter().any(contains_aggregate),
        ast::Expression::ListComprehension {
            filter_expr,
            map_expr,
            ..
        } => filter_expr.as_deref().is_some_and(contains_aggregate) || contains_aggregate(map_expr),
        _ => false,
    }
}
