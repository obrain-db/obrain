//! Projection, RETURN, sort, limit, and skip planning.

use super::{
    Arc, Error, GraphStore, HashMap, LimitOp, LogicalExpression, LogicalOperator, LogicalType,
    NullOrder, Operator, PhysicalSortKey, ProjectExpr, ProjectOperator, Result, ReturnOp, SkipOp,
    SortDirection, SortOp, SortOperator, SortOrder, common, expression_to_string,
    value_to_logical_type,
};

impl super::Planner {
    /// Plans a RETURN clause.
    pub(super) fn plan_return(&self, ret: &ReturnOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Handle Empty input (standalone RETURN like: RETURN 2 * 3 AS product)
        let (input_op, input_columns): (Box<dyn Operator>, Vec<String>) =
            if matches!(ret.input.as_ref(), LogicalOperator::Empty) {
                let single_row_op: Box<dyn Operator> = Box::new(
                    obrain_core::execution::operators::single_row::SingleRowOperator::new(),
                );
                (single_row_op, Vec::new())
            } else {
                self.plan_operator(&ret.input)?
            };

        self.plan_return_with_input(ret, input_op, input_columns)
    }

    /// Plans a RETURN operator with an already-planned input operator.
    /// This is used by `plan_sort` when ORDER BY needs pre-Return property projections.
    pub(super) fn plan_return_with_input(
        &self,
        ret: &ReturnOp,
        input_op: Box<dyn Operator>,
        input_columns: Vec<String>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (operator, columns) = self.plan_return_projection(ret, input_op, input_columns)?;

        // Apply DISTINCT if requested
        if ret.distinct {
            let schema = vec![LogicalType::Any; columns.len()];
            Ok(common::build_distinct(operator, columns, None, schema))
        } else {
            Ok((operator, columns))
        }
    }

    /// Plans the projection part of a RETURN clause (without DISTINCT).
    fn plan_return_projection(
        &self,
        ret: &ReturnOp,
        input_op: Box<dyn Operator>,
        input_columns: Vec<String>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Expand RETURN * wildcard: replace with all user-visible input columns
        let expanded_items;
        let items = if ret.items.len() == 1
            && matches!(&ret.items[0].expression, LogicalExpression::Variable(n) if n == "*")
        {
            expanded_items = input_columns
                .iter()
                .filter(|col| !col.starts_with('_')) // Skip internal columns
                .map(|col| crate::query::plan::ReturnItem {
                    expression: LogicalExpression::Variable(col.clone()),
                    alias: None,
                })
                .collect::<Vec<_>>();
            &expanded_items
        } else {
            &ret.items
        };

        // Build variable to column index mapping
        let variable_columns: HashMap<String, usize> = input_columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();

        // Extract column names from return items
        let columns: Vec<String> = items
            .iter()
            .map(|item| {
                item.alias.clone().unwrap_or_else(|| {
                    // Generate a default name from the expression
                    expression_to_string(&item.expression)
                })
            })
            .collect();

        // Check if we need a project operator (for property access or expression evaluation)
        let needs_project = items
            .iter()
            .any(|item| !matches!(&item.expression, LogicalExpression::Variable(_)));

        if needs_project {
            // Build project expressions
            let mut projections = Vec::with_capacity(items.len());
            let mut output_types = Vec::with_capacity(items.len());

            for item in items {
                match &item.expression {
                    LogicalExpression::Variable(name) => {
                        let col_idx = *variable_columns.get(name).ok_or_else(|| {
                            Error::Internal(format!("Variable '{}' not found in input", name))
                        })?;
                        // Path detail variables and UNWIND/FOR scalar variables pass through as-is
                        if name.starts_with("_path_nodes_")
                            || name.starts_with("_path_edges_")
                            || name.starts_with("_path_length_")
                            || self.scalar_columns.borrow().contains(name)
                        {
                            projections.push(ProjectExpr::Column(col_idx));
                            output_types.push(LogicalType::Any);
                        } else if self.edge_columns.borrow().contains(name) {
                            projections.push(ProjectExpr::EdgeResolve { column: col_idx });
                            output_types.push(LogicalType::Any);
                        } else {
                            projections.push(ProjectExpr::NodeResolve { column: col_idx });
                            output_types.push(LogicalType::Any);
                        }
                    }
                    LogicalExpression::Property { variable, property } => {
                        let col_idx = *variable_columns.get(variable).ok_or_else(|| {
                            Error::Internal(format!("Variable '{}' not found in input", variable))
                        })?;
                        projections.push(ProjectExpr::PropertyAccess {
                            column: col_idx,
                            property: property.clone(),
                        });
                        // Property could be any type - use Any/Generic to preserve type
                        output_types.push(LogicalType::Any);
                    }
                    LogicalExpression::Literal(value) => {
                        projections.push(ProjectExpr::Constant(value.clone()));
                        output_types.push(value_to_logical_type(value));
                    }
                    LogicalExpression::FunctionCall { name, args, .. } => {
                        // Handle built-in functions
                        match name.to_lowercase().as_str() {
                            "type" => {
                                // type(r) returns the edge type string
                                if args.len() != 1 {
                                    return Err(Error::Internal(
                                        "type() requires exactly one argument".to_string(),
                                    ));
                                }
                                if let LogicalExpression::Variable(var_name) = &args[0] {
                                    let col_idx =
                                        *variable_columns.get(var_name).ok_or_else(|| {
                                            Error::Internal(format!(
                                                "Variable '{}' not found in input",
                                                var_name
                                            ))
                                        })?;
                                    projections.push(ProjectExpr::EdgeType { column: col_idx });
                                    output_types.push(LogicalType::String);
                                } else {
                                    return Err(Error::Internal(
                                        "type() argument must be a variable".to_string(),
                                    ));
                                }
                            }
                            "length" => {
                                // length(p) returns the path length for path variables,
                                // or delegates to the expression evaluator for other
                                // arguments (e.g. length(a.name) on strings/lists).
                                if args.len() != 1 {
                                    return Err(Error::Internal(
                                        "length() requires exactly one argument".to_string(),
                                    ));
                                }
                                if let LogicalExpression::Variable(var_name) = &args[0] {
                                    // Try direct column first, then path detail column
                                    let path_col = format!("_path_length_{var_name}");
                                    let col_idx = variable_columns
                                        .get(&path_col)
                                        .or_else(|| variable_columns.get(var_name))
                                        .ok_or_else(|| {
                                            Error::Internal(format!(
                                                "Variable '{}' not found in input",
                                                var_name
                                            ))
                                        })?;
                                    projections.push(ProjectExpr::Column(*col_idx));
                                    output_types.push(LogicalType::Int64);
                                } else {
                                    // Non-variable argument (e.g. property access):
                                    // fall through to expression evaluation
                                    let filter_expr = self.convert_expression(&item.expression)?;
                                    projections.push(ProjectExpr::Expression {
                                        expr: filter_expr,
                                        variable_columns: variable_columns.clone(),
                                    });
                                    output_types.push(LogicalType::Any);
                                }
                            }
                            "nodes" | "edges" | "relationships" => {
                                // nodes(p) / edges(p) / relationships(p) returns path components
                                let func_name = name.to_lowercase();
                                if args.len() != 1 {
                                    return Err(Error::Internal(format!(
                                        "{}() requires exactly one argument",
                                        name
                                    )));
                                }
                                if let LogicalExpression::Variable(var_name) = &args[0] {
                                    // Map to internal column name
                                    let suffix = if func_name == "nodes" {
                                        "nodes"
                                    } else {
                                        "edges"
                                    };
                                    let path_col = format!("_path_{suffix}_{var_name}");
                                    let col_idx = variable_columns
                                        .get(&path_col)
                                        .or_else(|| variable_columns.get(var_name))
                                        .ok_or_else(|| {
                                            Error::Internal(format!(
                                                "Variable '{var_name}' not found in input",
                                            ))
                                        })?;
                                    projections.push(ProjectExpr::Column(*col_idx));
                                    output_types.push(LogicalType::Any);
                                } else {
                                    return Err(Error::Internal(format!(
                                        "{}() argument must be a variable",
                                        name
                                    )));
                                }
                            }
                            // For other functions (head, tail, size, etc.), use expression evaluation
                            _ => {
                                let filter_expr = self.convert_expression(&item.expression)?;
                                projections.push(ProjectExpr::Expression {
                                    expr: filter_expr,
                                    variable_columns: variable_columns.clone(),
                                });
                                output_types.push(LogicalType::Any);
                            }
                        }
                    }
                    LogicalExpression::Case { .. } => {
                        // Convert CASE expression to FilterExpression for evaluation
                        let filter_expr = self.convert_expression(&item.expression)?;
                        projections.push(ProjectExpr::Expression {
                            expr: filter_expr,
                            variable_columns: variable_columns.clone(),
                        });
                        // CASE can return any type - use Any
                        output_types.push(LogicalType::Any);
                    }
                    LogicalExpression::Binary { .. }
                    | LogicalExpression::Unary { .. }
                    | LogicalExpression::List(_)
                    | LogicalExpression::Map(_)
                    | LogicalExpression::IndexAccess { .. }
                    | LogicalExpression::SliceAccess { .. }
                    | LogicalExpression::CountSubquery(_)
                    | LogicalExpression::ValueSubquery(_)
                    | LogicalExpression::MapProjection { .. }
                    | LogicalExpression::Reduce { .. }
                    | LogicalExpression::PatternComprehension { .. }
                    | LogicalExpression::ListComprehension { .. }
                    | LogicalExpression::ListPredicate { .. } => {
                        // Convert complex expressions to FilterExpression for evaluation
                        let filter_expr = self.convert_expression(&item.expression)?;
                        projections.push(ProjectExpr::Expression {
                            expr: filter_expr,
                            variable_columns: variable_columns.clone(),
                        });
                        output_types.push(LogicalType::Any);
                    }
                    _ => {
                        return Err(Error::Internal(format!(
                            "Unsupported RETURN expression: {:?}",
                            item.expression
                        )));
                    }
                }
            }

            let operator = Box::new(
                ProjectOperator::with_store(
                    input_op,
                    projections,
                    output_types,
                    Arc::clone(&self.store) as Arc<dyn GraphStore>,
                )
                .with_transaction_context(self.viewing_epoch, self.transaction_id)
                .with_session_context(self.session_context.clone()),
            );

            Ok((operator, columns))
        } else {
            // Simple case: all return items are bare variables
            // Emit resolve variants for entity variables
            let mut projections = Vec::with_capacity(items.len());
            let mut output_types = Vec::with_capacity(items.len());

            for item in items {
                if let LogicalExpression::Variable(name) = &item.expression {
                    let col_idx = *variable_columns.get(name).ok_or_else(|| {
                        Error::Internal(format!("Variable '{}' not found in input", name))
                    })?;
                    if self.scalar_columns.borrow().contains(name) {
                        projections.push(ProjectExpr::Column(col_idx));
                        output_types.push(LogicalType::Any);
                    } else if self.edge_columns.borrow().contains(name) {
                        projections.push(ProjectExpr::EdgeResolve { column: col_idx });
                        output_types.push(LogicalType::Any);
                    } else {
                        projections.push(ProjectExpr::NodeResolve { column: col_idx });
                        output_types.push(LogicalType::Any);
                    }
                }
            }

            // Skip ProjectOperator only when all projections are plain Column pass-throughs
            // (i.e., only scalar variables with no reordering). NodeResolve/EdgeResolve
            // always require a ProjectOperator with store access.
            if projections.len() == input_columns.len()
                && projections
                    .iter()
                    .enumerate()
                    .all(|(i, p)| matches!(p, ProjectExpr::Column(c) if *c == i))
            {
                // No reordering or resolution needed
                Ok((input_op, columns))
            } else {
                let operator = Box::new(
                    ProjectOperator::with_store(
                        input_op,
                        projections,
                        output_types,
                        Arc::clone(&self.store) as Arc<dyn GraphStore>,
                    )
                    .with_transaction_context(self.viewing_epoch, self.transaction_id)
                    .with_session_context(self.session_context.clone()),
                );
                Ok((operator, columns))
            }
        }
    }

    /// Plans a project operator (for WITH clause).
    pub(super) fn plan_project(
        &self,
        project: &crate::query::plan::ProjectOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Handle Empty input specially (standalone WITH like: WITH [1,2,3] AS nums)
        let (input_op, input_columns): (Box<dyn Operator>, Vec<String>) =
            if matches!(project.input.as_ref(), LogicalOperator::Empty) {
                // Create a single-row operator for projecting literals
                let single_row_op: Box<dyn Operator> = Box::new(
                    obrain_core::execution::operators::single_row::SingleRowOperator::new(),
                );
                (single_row_op, Vec::new())
            } else {
                self.plan_operator(&project.input)?
            };

        // Build variable to column index mapping
        let variable_columns: HashMap<String, usize> = input_columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();

        // Build projections and new column names
        let capacity = if project.pass_through_input {
            input_columns.len() + project.projections.len()
        } else {
            project.projections.len()
        };
        let mut projections = Vec::with_capacity(capacity);
        let mut output_types = Vec::with_capacity(capacity);
        let mut output_columns = Vec::with_capacity(capacity);

        // When pass_through_input is set (e.g. LET clause), first pass through
        // all existing input columns so they remain accessible to downstream
        // operators. The explicit projections are then appended as new columns.
        if project.pass_through_input {
            for (idx, col_name) in input_columns.iter().enumerate() {
                projections.push(ProjectExpr::Column(idx));
                output_types.push(LogicalType::Any);
                output_columns.push(col_name.clone());
            }
        }

        for projection in &project.projections {
            // Determine the output column name (alias or expression string)
            let col_name = projection
                .alias
                .clone()
                .unwrap_or_else(|| expression_to_string(&projection.expression));

            match &projection.expression {
                LogicalExpression::Variable(name) => {
                    let col_idx = *variable_columns.get(name).ok_or_else(|| {
                        Error::Internal(format!("Variable '{}' not found in input", name))
                    })?;
                    projections.push(ProjectExpr::Column(col_idx));
                    // ── LE TYPE D'UNE VARIABLE PROJETÉE N'EST PAS TOUJOURS UN NŒUD ──
                    // Défaut mesuré le 2026-09-17 : cette ligne disait
                    // `LogicalType::Node` pour TOUTE variable projetée. Un nœud est
                    // bien le cas le plus fréquent, mais une variable de projection
                    // porte aussi des SCALAIRES (résultat d'agrégat, propriété
                    // calculée, alias) — et le type déclaré décide du VECTEUR de
                    // sortie. Recopier une liste (`WITH collect(n.v) AS vals RETURN
                    // vals`) dans un vecteur de nœuds la perdait silencieusement :
                    // mesuré, `RETURN vals` rendait `Int64(0)` et `size(vals)` rendait
                    // null, alors que la même requête en Cypher rendait `[10, 20]`
                    // (Cypher passe par un autre chemin de planification). Le plan,
                    // lui, portait la bonne valeur : c'est la COPIE qui la perdait.
                    //
                    // L'information existe déjà dans le planificateur (les mêmes
                    // ensembles que le conventionnel « ni scalaire ni arête ⇒ nœud »),
                    // et le planificateur d'agrégats y inscrit ses colonnes de sortie.
                    // On s'en sert au lieu de la supposer.
                    let est_scalaire = self.scalar_columns.borrow().contains(name);
                    let est_arete = self.edge_columns.borrow().contains(name);
                    output_types.push(if est_scalaire {
                        // GÉNÉRIQUE : accepte nombres, chaînes ET listes. Un type
                        // scalaire précis (Int64) perdrait les autres.
                        LogicalType::Any
                    } else if est_arete {
                        LogicalType::Edge
                    } else {
                        LogicalType::Node
                    });
                    // Propagate scalar/edge status from input variable to output alias
                    if self.scalar_columns.borrow().contains(name) {
                        self.scalar_columns.borrow_mut().insert(col_name.clone());
                    } else if self.edge_columns.borrow().contains(name) {
                        self.edge_columns.borrow_mut().insert(col_name.clone());
                    }
                }
                LogicalExpression::Property { variable, property } => {
                    let col_idx = *variable_columns.get(variable).ok_or_else(|| {
                        Error::Internal(format!("Variable '{}' not found in input", variable))
                    })?;
                    projections.push(ProjectExpr::PropertyAccess {
                        column: col_idx,
                        property: property.clone(),
                    });
                    output_types.push(LogicalType::Any);
                    // Property access produces a scalar value
                    self.scalar_columns.borrow_mut().insert(col_name.clone());
                }
                LogicalExpression::Literal(value) => {
                    projections.push(ProjectExpr::Constant(value.clone()));
                    output_types.push(value_to_logical_type(value));
                    // Literals are scalar values
                    self.scalar_columns.borrow_mut().insert(col_name.clone());
                }
                _ => {
                    // For complex expressions, use full expression evaluation
                    let filter_expr = self.convert_expression(&projection.expression)?;
                    projections.push(ProjectExpr::Expression {
                        expr: filter_expr,
                        variable_columns: variable_columns.clone(),
                    });
                    output_types.push(LogicalType::Any);
                    // Expression results are scalar values
                    self.scalar_columns.borrow_mut().insert(col_name.clone());
                }
            }

            output_columns.push(col_name);
        }

        let operator = Box::new(
            ProjectOperator::with_store(
                input_op,
                projections,
                output_types,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone()),
        );

        Ok((operator, output_columns))
    }

    /// Plans a LIMIT operator.
    pub(super) fn plan_limit(&self, limit: &LimitOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Un `CountExpr::Parameter` encore présent ici signifie que la
        // requête portait `LIMIT $k` mais qu'aucun paramètre n'a été fourni
        // (chemin `execute()` sans substitution). Erreur propre plutôt que
        // le panic de `count.value()` plus bas.
        limit.count.try_value().map_err(|msg| {
            Error::Query(obrain_common::utils::error::QueryError::new(
                obrain_common::utils::error::QueryErrorKind::Semantic,
                msg,
            ))
        })?;
        // T17h T9c fast-path : try the typed-degree top-K rewrite. On
        // a match, the entire `Limit → Sort → Return → Aggregate →
        // LeftJoin(s) → Expand` subtree is substituted with a single
        // TypedDegreeTopKOperator wrapped in a Project. Silent
        // fallback : return None → fall through to the generic plan.
        if let Some(result) = super::typed_degree_rewrite::try_plan_typed_degree_topk(self, limit) {
            return Ok(result);
        }

        // T17h+ fast-path : `Limit(N) → NodeScan(label)` (no Sort, no Filter
        // in between). We can push the LIMIT down into the scan via
        // `nodes_by_label_capped`. Without this, the scan iterates every
        // slot of the store (1.35M for `po`) before the LimitOperator on
        // top trims to N — observed 3-9s on big labels.
        //
        // SAFETY constraints (any of these → fall through to generic plan) :
        //  - Input is NOT a direct NodeScan (could be Filter, Sort, Aggregate…)
        //  - NodeScan has its own child input (cross-join pattern)
        //  - count == 0 (degenerate)
        // T17i : the fast-path below only ever matched `Limit → NodeScan`, a
        // shape the GQL translator never produces — it applies `wrap_return`
        // BEFORE `wrap_skip`/`wrap_limit`, so a real query is always
        // `Limit → Return → NodeScan`. The push-down was therefore dead code
        // for every user query: `MATCH (n:SupervisorObjective) RETURN n.id
        // LIMIT 5` measured 447 ms on 31 357 nodes because the scan
        // materialised the whole label before the LimitOperator trimmed to 5.
        //
        // We push the LIMIT *under* the projection when the projection is
        // row-preserving, then let the recursion hit the NodeScan fast-path.
        // Row-preserving means, structurally :
        //  - not DISTINCT (DISTINCT can collapse rows → N inputs may yield <N)
        //  - every item is a plain variable or property read. Any other shape
        //    (function call, subquery…) may not be 1:1, and the aggregate path
        //    builds an Aggregate node under the Return anyway, so requiring a
        //    direct NodeScan already excludes it.
        if limit.count.value() > 0
            && let crate::query::plan::LogicalOperator::Return(ret) = limit.input.as_ref()
            && !ret.distinct
            && ret.items.iter().all(|item| {
                matches!(
                    &item.expression,
                    LogicalExpression::Variable(_) | LogicalExpression::Property { .. }
                )
            })
            && let crate::query::plan::LogicalOperator::NodeScan(scan) = ret.input.as_ref()
            && scan.input.is_none()
        {
            // (2026-09-11) L'EXIGENCE D'ETIQUETTE EST LEVEE. Elle valait
            // `scan.label.is_some()` et rendait ce chemin inapplicable a
            // `MATCH (n) RETURN ... LIMIT k` — la forme exacte du `LIMIT 30`
            // de production, qui payait donc 3,5 millions de slots pour 30
            // lignes.
            //
            // CE QUI TIENT LA SURETE, ce n'est pas l'etiquette : c'est la
            // FORME STRUCTURELLE exigee juste au-dessus. `ret.input` doit
            // etre un `NodeScan` DIRECT. Un `WHERE` construit un
            // `LogicalOperator::Filter` — un variant distinct, qui ne peut
            // pas s'unifier avec `NodeScan` et fait donc tomber la requete
            // dans le plan generique. Verifie aussi cote structure :
            // `NodeScanOp` ne porte AUCUN predicat (variable, label, input,
            // pushdown_hint) — il ne peut pas cacher un filtre. Idem pour
            // Sort, Join, Aggregate, Distinct, tous des variants distincts,
            // et `ret.distinct` est deja exclu.
            //
            // Autrement dit le plafond ne descend que si RIEN entre le scan
            // et le LIMIT ne reduit le nombre de lignes.
            let pushed = LogicalOperator::Return(ReturnOp {
                items: ret.items.clone(),
                distinct: ret.distinct,
                input: Box::new(LogicalOperator::Limit(LimitOp {
                    count: limit.count.clone(),
                    input: ret.input.clone(),
                })),
            });
            let (input_op, columns) = self.plan_operator(&pushed)?;
            let schema = self.derive_schema_from_columns(&columns);
            return Ok(crate::query::planner::common::build_limit(
                input_op,
                columns,
                limit.count.value(),
                schema,
            ));
        }

        // (NATIF-J6) ROUTE ORDER BY <prop> LIMIT k VIA L INDEX ORDONNE PAR
        // VALEUR : lit ~k slots au lieu de N. Prend le pas sur la descente de
        // limite ci-dessous ; si non eligible, elle rend None et le heap borne
        // (identique a l'ancien comportement) reprend la main.
        if let Some(result) = self.try_plan_order_by_index(limit) {
            return Ok(result);
        }

        // (2026-08-21) DESCENTE DE LA LIMITE DANS LE TRI.
        //
        // `SortOperator` matérialisait TOUT et triait TOUT, quelle que soit la
        // limite au-dessus. Mesuré sur la forme de production
        // `ORDER BY … LIMIT k` : `LIMIT 8` coûtait 126,2 ms et `LIMIT 20000`
        // 129,4 ms — un rapport de 0,98. Rendre huit lignes coûtait le prix de
        // seize mille.
        //
        // POURQUOI C'EST SÛR. `bounded_top_k` porte sa preuve : l'ordre utilisé
        // est (clés de tri, ordinal d'arrivée). Les ordinaux étant distincts,
        // cet ordre est TOTAL — aucune paire n'est égale, donc les k plus
        // petits forment un ensemble unique dans une séquence unique. Or un tri
        // stable produit exactement cette séquence. Le borné rend donc le
        // préfixe exact du complet, ex æquo compris
        // (`test_limit_matches_full_sort_prefix_*`).
        //
        // POURQUOI SEULEMENT UN `Sort` DIRECT. Un `Skip(s)` intercalé ferait de
        // la limite utile `s + k`, pas `k` ; un `Distinct` au-dessus du tri
        // pourrait réduire le nombre de lignes et en exiger plus que k. Ces
        // formes tombent dans le chemin générique, où rien ne change.
        //
        // L'enveloppe `build_limit` est CONSERVÉE — même prudence que les
        // chemins rapides voisins : la borne du tri est une optimisation, pas
        // le contrat de sortie.
        if limit.count.value() > 0
            && let crate::query::plan::LogicalOperator::Sort(sort) = limit.input.as_ref()
        {
            let (input_op, columns) = self.plan_sort(sort, Some(limit.count.value()))?;
            let schema = self.derive_schema_from_columns(&columns);
            return Ok(crate::query::planner::common::build_limit(
                input_op,
                columns,
                limit.count.value(),
                schema,
            ));
        }

        // Meme leve d'exigence que le chemin A, meme justification : c'est le
        // `NodeScan` DIRECT sous le `Limit` qui porte la surete, pas
        // l'etiquette. Le cas sans etiquette emprunte
        // `ScanOperator::new` + `with_max_rows`, que `load_batch` route vers
        // `node_ids_capped` sous ses trois gardes.
        if let crate::query::plan::LogicalOperator::NodeScan(scan) = limit.input.as_ref()
            && scan.input.is_none()
            && limit.count.value() > 0
        {
            let store = std::sync::Arc::clone(&self.store)
                as std::sync::Arc<dyn crate::query::planner::lpg::GraphStore>;
            let base = match &scan.label {
                Some(label) => crate::query::planner::lpg::ScanOperator::with_label(store, label),
                None => crate::query::planner::lpg::ScanOperator::new(store),
            };
            let scan_op = base
                .with_max_rows(limit.count.value())
                .with_transaction_context(self.viewing_epoch, self.transaction_id);
            let scan_box: Box<dyn Operator> = Box::new(scan_op);
            // Still wrap in a LIMIT operator for safety (the cap is a
            // store-level hint, but visibility filtering or
            // chunk-boundary effects can produce slightly more rows).
            let columns = vec![scan.variable.clone()];
            let schema = self.derive_schema_from_columns(&columns);
            return Ok(crate::query::planner::common::build_limit(
                scan_box,
                columns,
                limit.count.value(),
                schema,
            ));
        }

        let (input_op, columns) = self.plan_operator(&limit.input)?;
        let schema = self.derive_schema_from_columns(&columns);
        Ok(crate::query::planner::common::build_limit(
            input_op,
            columns,
            limit.count.value(),
            schema,
        ))
    }

    /// (NATIF-J6) Route `ORDER BY n.<prop> [ASC|DESC] LIMIT k` via l'index
    /// ORDONNE PAR VALEUR : au lieu de balayer les N noeuds puis d'en garder k
    /// (heap borne), on demande a l'index les k NodeId extremes et on les
    /// injecte comme feuille (`NodeListOperator`) SOUS la meme projection et le
    /// meme `SortOperator.with_limit(k)`. Le resultat est donc IDENTIQUE au tri
    /// complet (memes lignes, meme ordre, ex-aequo compris — le tri final
    /// depart par la valeur puis l'ordre d'arrivee = slot, comme le heap), mais
    /// ne lit que ~k slots.
    ///
    /// `None` (repli sur le heap borne, jamais faux) des qu'une condition
    /// manque : transaction active ou ecritures en attente (le jeu visible
    /// pourrait differer du jeu committe), cle de tri multiple ou non-Property,
    /// entree qui n'est pas un NodeScan NU de la meme variable (WHERE, label,
    /// entree propre, DISTINCT), propriete non indexee par valeur, ou NULL
    /// possibles en TETE du resultat (placement NULLS FIRST sur store mixte) —
    /// voir la garde NULL NATIF-J6B ci-dessous : en NULLS LAST la route reste
    /// engagee malgre les NULLs des que l'index rend k ids.
    fn try_plan_order_by_index(&self, limit: &LimitOp) -> Option<(Box<dyn Operator>, Vec<String>)> {
        let k = limit.count.try_value().ok()?;
        if k == 0 {
            return None;
        }
        // Sans ecriture en attente et EN VUE DE L'EPOQUE COURANTE, le jeu
        // visible == le jeu vivant committe == le contenu de l'index : aucun
        // filtrage de visibilite n'est requis (donc pas d'over-fetch). Une
        // transaction de LECTURE seule est admise (comme les fast-paths count /
        // min-max, qui ne gardent que has_pending_writes) ; une vue time-travel
        // (epoque != courante) ou des ecritures en attente font renoncer.
        // (NATIF-J8) Garde consciente de l'EPOQUE DE L'INDEX. En plus des deux
        // conditions historiques (aucune ecriture en attente ; vue au present),
        // on exige que l'index ORDONNE PAR VALEUR couvre TOUT l'etat committe
        // courant. `has_pending_writes` seul etait aveugle aux ecritures
        // auto-commit (SYSTEM), qui commitent sans l'armer mais font avancer le
        // WAL : l'index (bati sur disque) restait en retard sur la requete et
        // pouvait rendre une ligne perimee/disparue. `couvre_courant`
        // reconfronte FRAICHEMENT les ancres de l'index a l'etat present ; des
        // qu'une ecriture a suivi la derniere fusion, il rend faux -> balayage.
        if self.store.has_pending_writes()
            || self.viewing_epoch != self.store.current_epoch()
            || !self.store.index_ord_valeur_couvre_courant()
        {
            return None;
        }
        let LogicalOperator::Sort(sort) = limit.input.as_ref() else {
            return None;
        };
        if sort.keys.len() != 1 {
            return None;
        }
        let LogicalExpression::Property { variable, property } = &sort.keys[0].expression else {
            return None;
        };
        // L'entree du tri doit descendre DIRECTEMENT sur un NodeScan NU de la
        // meme variable : `Sort -> Return(non distinct) -> NodeScan` ou
        // `Sort -> NodeScan`. Un Filter (WHERE), un label, une entree propre ou
        // un DISTINCT font renoncer (variants distincts, ne s'unifient pas).
        let scan_ok = match sort.input.as_ref() {
            LogicalOperator::Return(ret) if !ret.distinct => matches!(
                ret.input.as_ref(),
                LogicalOperator::NodeScan(s)
                    if s.input.is_none() && s.label.is_none() && &s.variable == variable
            ),
            LogicalOperator::NodeScan(s) => {
                s.input.is_none() && s.label.is_none() && &s.variable == variable
            }
            _ => false,
        };
        if !scan_ok {
            return None;
        }
        // GARDE NULL (NATIF-J6B) : l'index ne porte que les valeurs non-null.
        // Placement effectif des NULL = clause explicite, sinon defaut par
        // direction (convention PostgreSQL/Neo4j : ASC->LAST, DESC->FIRST).
        //
        // * NULLS LAST : les NULL sortent en QUEUE du tri ; les k premieres
        //   lignes sont les k valeurs non-null extremes. La route est EXACTE
        //   des que l'index rend k ids — MEME en presence de NULLs (store
        //   mixte, la forme de production). S'il en rend moins, des NULL
        //   completeraient la queue du resultat => repli.
        // * NULLS FIRST : les NULL sortent en TETE, l'index ne peut pas les
        //   servir => route seulement si AUCUN NULL (nb == node_count) — ce
        //   qui ne tient pas sur un store mixte => repli, jamais faux.
        let ascending = matches!(sort.keys[0].order, SortOrder::Ascending);
        let nulls_en_queue = match sort.keys[0].nulls {
            Some(crate::query::plan::NullsOrdering::Last) => true,
            Some(crate::query::plan::NullsOrdering::First) => false,
            None => ascending,
        };
        if !nulls_en_queue
            && self.store.nb_valeurs_ordonnees(property) != Some(self.store.node_count())
        {
            return None;
        }
        let ids = if ascending {
            self.store.premiers_k_ordonnes(property, k)?
        } else {
            self.store.derniers_k_ordonnes(property, k)?
        };
        // NULLS LAST : il faut k ids pleins — ou tout le store si
        // node_count < k, cas ou s'il existe des NULLs on a nb < node_count et
        // cette meme borne fait replier. NULLS FIRST : sans NULL (garde
        // ci-dessus), l'index rend min(k, node_count).
        if ids.len() < k.min(self.store.node_count()) {
            return None;
        }
        // Injecte la feuille et laisse le plan de tri INCHANGE l'envelopper —
        // c'est ce qui garantit l'identite des resultats.
        *self.restriction_scan.borrow_mut() = Some((variable.clone(), ids));
        let planned = self.plan_sort(sort, Some(k));
        *self.restriction_scan.borrow_mut() = None;
        let (input_op, columns) = planned.ok()?;
        let schema = self.derive_schema_from_columns(&columns);
        Some(crate::query::planner::common::build_limit(
            input_op, columns, k, schema,
        ))
    }

    /// Plans a SKIP operator.
    pub(super) fn plan_skip(&self, skip: &SkipOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Même garde que `plan_limit` : `SKIP $k` sans paramètre fourni doit
        // échouer proprement, pas paniquer dans `count.value()`.
        skip.count.try_value().map_err(|msg| {
            Error::Query(obrain_common::utils::error::QueryError::new(
                obrain_common::utils::error::QueryErrorKind::Semantic,
                msg,
            ))
        })?;
        let (input_op, columns) = self.plan_operator(&skip.input)?;
        let schema = self.derive_schema_from_columns(&columns);
        Ok(crate::query::planner::common::build_skip(
            input_op,
            columns,
            skip.count.value(),
            schema,
        ))
    }

    /// Plans a SORT (ORDER BY) operator.
    ///
    /// When Sort wraps a Return (e.g. `RETURN p.name ORDER BY p.age`), ORDER BY
    /// may reference entity variables that Return has already projected away. In
    /// that case we inject a property projection BEFORE the Return so the sort
    /// key is available in the output columns.
    /// `limite` : quand l'appelant SAIT qu'au plus k lignes seront consommées
    /// — c'est `plan_limit` qui le sait, en voyant `Limit(k) → Sort` — le tri
    /// devient borné : `O(n log k)` au lieu de `O(n log n)`, et
    /// `SortOperator` n'émet que k lignes. `None` conserve strictement le
    /// comportement antérieur.
    ///
    /// La limite est un plafond de lignes ÉMISES. Un `Skip` intercalé la
    /// changerait en `skip + limit` ; `plan_limit` ne descend donc la limite
    /// que sur un `Sort` DIRECT, et laisse toute autre forme au chemin
    /// générique.
    pub(super) fn plan_sort(
        &self,
        sort: &SortOp,
        limite: Option<usize>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Check if we need pre-Return property projections. This is necessary
        // when ORDER BY references a property on an entity variable (e.g. p.age)
        // that is not included in the RETURN clause (e.g. RETURN p.name).
        let needs_pre_return = if let LogicalOperator::Return(ret) = sort.input.as_ref() {
            sort.keys.iter().any(|key| {
                if let LogicalExpression::Property { variable, .. } = &key.expression {
                    // Check if the Return items produce a column matching this variable
                    !ret.items.iter().any(|item| {
                        item.alias.as_deref() == Some(variable)
                            || matches!(
                                &item.expression,
                                LogicalExpression::Variable(v) if v == variable
                            )
                    })
                } else {
                    // Clé de tri EXPRESSION (`ORDER BY 0 - n.v`) : elle aussi
                    // passe par le Return augmenté (voir plus bas).
                    cle_de_tri_expression(&key.expression)
                }
            })
        } else {
            false
        };

        // Number of extra sort-key columns appended after Return items
        let mut sort_extra_count: usize = 0;

        // Clés de tri expression injectées dans le Return augmenté :
        // représentation Debug de l'expression -> nom de colonne synthétique.
        // (`LogicalExpression` ne dérive pas PartialEq ; la représentation
        // Debug est injective sur la structure, ce qui suffit pour retrouver
        // LA colonne projetée pour LA clé qui l'a demandée.)
        let mut complex_sort_cols: HashMap<String, String> = HashMap::new();

        let (mut input_op, input_columns) = if needs_pre_return {
            // Plan the Return's input, then build a combined projection that
            // outputs both RETURN items and ORDER BY sort-key properties.
            let LogicalOperator::Return(ret) = sort.input.as_ref() else {
                unreachable!()
            };
            let (inner_op, inner_columns) = self.plan_operator(&ret.input)?;
            let inner_vars: HashMap<String, usize> = inner_columns
                .iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), i))
                .collect();

            // Collect ORDER BY property keys that need extra projection
            let mut extra_items: Vec<(String, String, String)> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for key in &sort.keys {
                if let LogicalExpression::Property { variable, property } = &key.expression
                    && inner_vars.contains_key(variable)
                {
                    let col_name = format!("{}_{}", variable, property);
                    if seen.insert(col_name.clone()) {
                        extra_items.push((variable.clone(), property.clone(), col_name));
                    }
                }
            }

            // Build an augmented ReturnOp that includes the extra sort-key items
            let mut augmented_items = ret.items.clone();
            let mut extra_columns = Vec::new();
            for (variable, property, col_name) in &extra_items {
                augmented_items.push(crate::query::plan::ReturnItem {
                    expression: LogicalExpression::Property {
                        variable: variable.clone(),
                        property: property.clone(),
                    },
                    alias: Some(col_name.clone()),
                });
                extra_columns.push(col_name.clone());
            }

            // Clés de tri EXPRESSION (`ORDER BY 0 - n.v`, `ORDER BY n.v + 1`) :
            // le Return sait déjà évaluer une expression arbitraire, donc la
            // clé est projetée comme item supplémentaire du Return augmenté
            // — pas d'évaluateur à écrire — puis retirée après le tri par le
            // strip `sort_extra_count` (correctif borné du lot 09). Un NULL
            // produit par l'expression suit la clause NULLS comme n'importe
            // quelle colonne.
            for key in &sort.keys {
                if cle_de_tri_expression(&key.expression) {
                    let repr = format!("{:?}", key.expression);
                    if !complex_sort_cols.contains_key(&repr) {
                        let col_name = format!("__sort_expr_{}", complex_sort_cols.len());
                        augmented_items.push(crate::query::plan::ReturnItem {
                            expression: key.expression.clone(),
                            alias: Some(col_name.clone()),
                        });
                        extra_columns.push(col_name.clone());
                        complex_sort_cols.insert(repr, col_name);
                    }
                }
            }

            let augmented_ret = crate::query::plan::ReturnOp {
                items: augmented_items,
                distinct: ret.distinct,
                input: ret.input.clone(),
            };

            // Plan the augmented Return with the original inner operator
            let (op, columns) =
                self.plan_return_with_input(&augmented_ret, inner_op, inner_columns)?;
            sort_extra_count = extra_columns.len();
            (op, columns)
        } else {
            self.plan_operator(&sort.input)?
        };

        // Build variable to column index mapping
        let mut variable_columns: HashMap<String, usize> = input_columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();

        // Collect property expressions that need to be projected before sorting
        let mut property_projections: Vec<(String, String, String)> = Vec::new();
        let mut next_col_idx = input_columns.len();

        // Collect cognitive UDF expressions that need projection
        #[cfg(feature = "cognitive")]
        let mut cognitive_projections: Vec<(String, LogicalExpression)> = Vec::new();

        for key in &sort.keys {
            if let LogicalExpression::Property { variable, property } = &key.expression {
                let col_name = format!("{}_{}", variable, property);
                if !variable_columns.contains_key(&col_name) {
                    property_projections.push((
                        variable.clone(),
                        property.clone(),
                        col_name.clone(),
                    ));
                    variable_columns.insert(col_name, next_col_idx);
                    next_col_idx += 1;
                }
            }

            // Detect cognitive UDF calls (e.g., obrain.energy(n)) and project them
            #[cfg(feature = "cognitive")]
            if let LogicalExpression::FunctionCall { name, .. } = &key.expression {
                let col_name = name.replace('.', "_");
                if !variable_columns.contains_key(&col_name) {
                    cognitive_projections.push((col_name.clone(), key.expression.clone()));
                    variable_columns.insert(col_name, next_col_idx);
                    next_col_idx += 1;
                }
            }
        }

        // Track output columns
        let mut output_columns = input_columns.clone();

        // If we have property expressions, add a projection to materialize them
        if !property_projections.is_empty() {
            let mut projections = Vec::new();
            let mut output_types = Vec::new();

            // First, pass through all existing columns (use Node type to preserve node IDs
            // for subsequent property access - nodes need VectorData::NodeId for get_node_id())
            for (i, _) in input_columns.iter().enumerate() {
                projections.push(ProjectExpr::Column(i));
                output_types.push(LogicalType::Node);
            }

            // Then add property access projections
            for (variable, property, col_name) in &property_projections {
                let source_col = *variable_columns.get(variable).ok_or_else(|| {
                    Error::Internal(format!(
                        "Variable '{}' not found for ORDER BY property projection",
                        variable
                    ))
                })?;
                projections.push(ProjectExpr::PropertyAccess {
                    column: source_col,
                    property: property.clone(),
                });
                output_types.push(LogicalType::Any);
                output_columns.push(col_name.clone());
            }

            input_op = Box::new(
                ProjectOperator::with_store(
                    input_op,
                    projections,
                    output_types,
                    Arc::clone(&self.store) as Arc<dyn GraphStore>,
                )
                .with_transaction_context(self.viewing_epoch, self.transaction_id)
                .with_session_context(self.session_context.clone()),
            );
        }

        // Project cognitive UDF expressions (e.g., obrain.energy(n)) as extra columns
        #[cfg(feature = "cognitive")]
        if !cognitive_projections.is_empty() {
            let mut projections = Vec::new();
            let mut proj_types = Vec::new();

            // Pass through all existing columns
            for i in 0..output_columns.len() {
                projections.push(ProjectExpr::Column(i));
                proj_types.push(LogicalType::Any);
            }

            // Add cognitive expression columns
            for (col_name, expr) in &cognitive_projections {
                let filter_expr = self.convert_expression(expr)?;
                projections.push(ProjectExpr::Expression {
                    expr: filter_expr,
                    variable_columns: variable_columns.clone(),
                });
                proj_types.push(LogicalType::Float64);
                output_columns.push(col_name.clone());
            }

            input_op = Box::new(
                ProjectOperator::with_store(
                    input_op,
                    projections,
                    proj_types,
                    Arc::clone(&self.store) as Arc<dyn GraphStore>,
                )
                .with_transaction_context(self.viewing_epoch, self.transaction_id)
                .with_session_context(self.session_context.clone()),
            );
        }

        // Convert logical sort keys to physical sort keys
        let physical_keys: Vec<PhysicalSortKey> = sort
            .keys
            .iter()
            .map(|key| {
                // Clé expression injectée dans le Return augmenté : résolue
                // vers sa colonne synthétique, sinon résolution classique.
                let col_idx = match complex_sort_cols.get(&format!("{:?}", key.expression)) {
                    Some(col_name) => *variable_columns.get(col_name).ok_or_else(|| {
                        Error::Internal(format!(
                            "Injected sort-key column '{col_name}' not found for ORDER BY"
                        ))
                    })?,
                    None => self.resolve_sort_expression_with_properties(
                        &key.expression,
                        &variable_columns,
                    )?,
                };
                Ok(PhysicalSortKey {
                    column: col_idx,
                    direction: match key.order {
                        SortOrder::Ascending => SortDirection::Ascending,
                        SortOrder::Descending => SortDirection::Descending,
                    },
                    null_order: match key.nulls {
                        Some(crate::query::plan::NullsOrdering::First) => NullOrder::NullsFirst,
                        Some(crate::query::plan::NullsOrdering::Last) => NullOrder::NullsLast,
                        // Défaut CALCULÉ PAR DIRECTION (convention
                        // PostgreSQL/Neo4j : NULL est la plus grande valeur) :
                        // ASC → queue, DESC → tête. Indissociable du placement
                        // ABSOLU des NULL dans les comparateurs : avant, un
                        // `NullsLast` unique était retourné en NullsFirst sous
                        // DESC par le `.reverse()` fautif du comparateur — le
                        // bon défaut obtenu par un chemin faux. Le comparateur
                        // corrigé, ce défaut-ci préserve le comportement des
                        // requêtes sans clause NULLS.
                        None => match key.order {
                            SortOrder::Ascending => NullOrder::NullsLast,
                            SortOrder::Descending => NullOrder::NullsFirst,
                        },
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let output_schema = self.derive_schema_from_columns(&output_columns);
        let tri = SortOperator::new(input_op, physical_keys, output_schema);
        // Tri borné : voir la preuve de stabilité sur `bounded_top_k`. Le
        // résultat est le PRÉFIXE EXACT du tri complet — mêmes lignes, même
        // ordre, ex æquo compris — donc poser la limite ici ne change aucune
        // sortie, seulement le travail fourni pour l'obtenir.
        let tri = match limite {
            Some(k) => tri.with_limit(k),
            None => tri,
        };
        let mut operator: Box<dyn Operator> = Box::new(tri);

        // Strip extra sort-key columns that were injected for ORDER BY resolution
        if sort_extra_count > 0 {
            let keep_count = output_columns.len() - sort_extra_count;
            let strip_projections: Vec<ProjectExpr> =
                (0..keep_count).map(ProjectExpr::Column).collect();
            let strip_types: Vec<LogicalType> = (0..keep_count).map(|_| LogicalType::Any).collect();
            operator = Box::new(ProjectOperator::new(
                operator,
                strip_projections,
                strip_types,
            ));
            output_columns.truncate(keep_count);
        }

        Ok((operator, output_columns))
    }

    /// Resolves a sort expression to a column index, using projected property columns.
    pub(super) fn resolve_sort_expression_with_properties(
        &self,
        expr: &LogicalExpression,
        variable_columns: &HashMap<String, usize>,
    ) -> Result<usize> {
        match expr {
            LogicalExpression::Variable(name) => {
                variable_columns.get(name).copied().ok_or_else(|| {
                    Error::Internal(format!("Variable '{}' not found for ORDER BY", name))
                })
            }
            LogicalExpression::Property { variable, property } => {
                // Look up the projected property column (e.g., "p_age" for p.age)
                let col_name = format!("{}_{}", variable, property);
                variable_columns.get(&col_name).copied().ok_or_else(|| {
                    Error::Internal(format!(
                        "Property column '{}' not found for ORDER BY (from {}.{})",
                        col_name, variable, property
                    ))
                })
            }
            LogicalExpression::FunctionCall { name, .. } => {
                // Look for a projected cognitive/function column
                let col_name = name.replace('.', "_");
                variable_columns.get(&col_name).copied().ok_or_else(|| {
                    Error::Internal(format!(
                        "Function column '{}' not found for ORDER BY (from {}()). \
                         Cognitive UDF columns are auto-projected when the cognitive feature is enabled.",
                        col_name, name
                    ))
                })
            }
            _ => Err(Error::Internal(format!(
                "Unsupported ORDER BY expression: {:?}",
                expr
            ))),
        }
    }

    /// Derives a schema from column names (uses Any type to handle all value types).
    pub(super) fn derive_schema_from_columns(&self, columns: &[String]) -> Vec<LogicalType> {
        columns.iter().map(|_| LogicalType::Any).collect()
    }
}

/// Une clé de tri qui est une EXPRESSION à évaluer (`ORDER BY 0 - n.v`,
/// `ORDER BY n.v + 1`, CASE…) — par opposition aux formes résolues par nom
/// (`Variable`, `Property`, `FunctionCall` cognitif). Ces clés sont projetées
/// en colonne supplémentaire via le Return augmenté de `plan_sort`.
fn cle_de_tri_expression(expr: &LogicalExpression) -> bool {
    matches!(
        expr,
        LogicalExpression::Binary { .. }
            | LogicalExpression::Unary { .. }
            | LogicalExpression::Case { .. }
    )
}
