//! Aggregate and factorized aggregate planning.

use super::{
    AggregateOp, Arc, Direction, Error, ExpandDirection, ExpandStep, ExpressionPredicate,
    FactorizedAggregate, FactorizedAggregateOperator, FilterExpression, FilterOperator, GraphStore,
    HashAggregateOperator, HashMap, LazyFactorizedChainOperator, LogicalAggregateFunction,
    LogicalExpression, LogicalOperator, LogicalType, Operator, PhysicalAggregateExpr, ProjectExpr,
    ProjectOperator, Result, ScalarResultOperator, SimpleAggregateOperator, Value,
    convert_aggregate_function, expression_to_string,
};
use obrain_core::execution::DataChunk;
use obrain_core::execution::operators::OperatorResult;

impl super::Planner {
    /// Plans an AGGREGATE operator.
    pub(super) fn plan_aggregate(
        &self,
        agg: &AggregateOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // === Fast-path: COUNT without scan ===
        // MATCH (n) RETURN count(n) → store.node_count() in O(1)
        // MATCH ()-[r]->() RETURN count(r) → store.edge_count() in O(1)
        if let Some(result) = self.try_plan_count_shortcut(agg) {
            return Ok(result);
        }

        // (2026-09-14) RACCOURCI DES COMPTES GROUPÉS PAR LABEL / TYPE D'ARÊTE.
        // Voir `try_plan_group_count_shortcut` : forme exacte reconnue, gardes,
        // et pourquoi la source est un BALAYAGE et non un compteur.
        if let Some(result) = self.try_plan_group_count_shortcut(agg) {
            return Ok(result);
        }

        // === Fast-path (NATIF-J6): min/max via index ORDONNE PAR VALEUR ===
        // MATCH (n) RETURN min(n.<prop>), max(n.<prop>) → extremites de l index
        // ordonne par valeur, en O(1), au lieu de balayer tous les noeuds.
        if let Some(result) = self.try_plan_minmax_shortcut(agg) {
            return Ok(result);
        }

        // (2026-09-08) PUSHDOWN CHAÎNES HOMOGÈNES → noyau `reachable_count` :
        // `MATCH (a:L)-[:E]->…->(d:L) RETURN count(DISTINCT d)` (k ∈ 1..=10,
        // un seul label, un seul type d'arête, sens sortant) est exécuté par
        // la fermeture par frontières distinctes au lieu de l'énumération des
        // chemins. Toute autre forme rend `None` ici → plan historique
        // (factorisé puis générique) inchangé. Voir le module
        // `reachable_pushdown` pour la liste exacte formes réécrites /
        // refusées.
        if let Some(result) =
            super::reachable_pushdown::try_plan_reachable_count_pushdown(self, agg)
        {
            return result;
        }

        // Check if we can use factorized aggregation for speedup
        // Conditions:
        // 1. Factorized execution is enabled
        // 2. Input is an expand chain (multi-hop)
        // 3. No GROUP BY
        // 4. All aggregates are simple (COUNT, SUM, AVG, MIN, MAX)
        if self.factorized_execution
            // Aretes inter-bases (V2) : le chemin factorise ne connait pas
            // le bras externe de l'expansion — avec un registre attache,
            // on reste sur la voie generale pour ne perdre aucune arete.
            && self.session_context.base_registry.is_none()
            && agg.group_by.is_empty()
            && Self::count_expand_chain(&agg.input).0 >= 2
            && self.is_simple_aggregate(agg)
            && let Ok((op, cols)) = self.plan_factorized_aggregate(agg)
        {
            return Ok((op, cols));
        }
        // Fall through to regular aggregate if factorized planning fails

        let (mut input_op, input_columns) = self.plan_operator(&agg.input)?;

        // Build variable to column index mapping
        let mut variable_columns: HashMap<String, usize> = input_columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();

        // Collect all property expressions that need to be projected before aggregation
        let mut property_projections: Vec<(String, String, String)> = Vec::new(); // (variable, property, new_column_name)
        let mut expression_projections: Vec<(FilterExpression, String)> = Vec::new();
        let mut next_col_idx = input_columns.len();

        // Check group-by expressions for properties and complex expressions (literals, CASE, etc.)
        for expr in &agg.group_by {
            match expr {
                LogicalExpression::Property { variable, property } => {
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
                LogicalExpression::Variable(_) => {
                    // Already in variable_columns, nothing to project
                }
                _ => {
                    // Complex expression (Literal, CASE, Binary, etc.): project as computed column
                    let col_name = format!("__expr_{:?}", expr);
                    if !variable_columns.contains_key(&col_name) {
                        let filter_expr = self.convert_expression(expr)?;
                        expression_projections.push((filter_expr, col_name.clone()));
                        variable_columns.insert(col_name, next_col_idx);
                        next_col_idx += 1;
                    }
                }
            }
        }

        // Check aggregate expressions for properties and complex expressions
        // (both first and second arguments)
        for agg_expr in &agg.aggregates {
            for expr_opt in [&agg_expr.expression, &agg_expr.expression2] {
                let Some(expr) = expr_opt else { continue };
                match expr {
                    LogicalExpression::Property { variable, property } => {
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
                    LogicalExpression::Variable(_) => {
                        // Already in variable_columns, nothing to project
                    }
                    _ => {
                        // Complex expression (CASE, Binary, etc.): project as computed column
                        let col_name = format!("__expr_{:?}", expr);
                        if !variable_columns.contains_key(&col_name) {
                            let filter_expr = self.convert_expression(expr)?;
                            expression_projections.push((filter_expr, col_name.clone()));
                            variable_columns.insert(col_name, next_col_idx);
                            next_col_idx += 1;
                        }
                    }
                }
            }
        }

        // If we have property or expression projections, add a projection to materialize them
        if !property_projections.is_empty() || !expression_projections.is_empty() {
            let mut projections = Vec::new();
            let mut output_types = Vec::new();

            // First, pass through all existing columns (use Node type to preserve node IDs
            // for subsequent property access - nodes need VectorData::NodeId for get_node_id())
            for (i, _) in input_columns.iter().enumerate() {
                projections.push(ProjectExpr::Column(i));
                output_types.push(LogicalType::Node);
            }

            // Then add property access projections
            for (variable, property, _col_name) in &property_projections {
                let source_col = *variable_columns.get(variable).ok_or_else(|| {
                    Error::Internal(format!(
                        "Variable '{}' not found for property projection",
                        variable
                    ))
                })?;
                projections.push(ProjectExpr::PropertyAccess {
                    column: source_col,
                    property: property.clone(),
                });
                output_types.push(LogicalType::Any);
            }

            // Then add complex expression projections (CASE, etc.)
            for (filter_expr, _col_name) in &expression_projections {
                projections.push(ProjectExpr::Expression {
                    expr: filter_expr.clone(),
                    variable_columns: variable_columns.clone(),
                });
                output_types.push(LogicalType::Any);
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

        // Convert group-by expressions to column indices
        let group_columns: Vec<usize> = agg
            .group_by
            .iter()
            .map(|expr| self.resolve_expression_to_column_with_properties(expr, &variable_columns))
            .collect::<Result<Vec<_>>>()?;

        // Convert aggregate expressions to physical form
        let physical_aggregates: Vec<PhysicalAggregateExpr> = agg
            .aggregates
            .iter()
            .map(|agg_expr| {
                let column = agg_expr
                    .expression
                    .as_ref()
                    .map(|e| {
                        self.resolve_expression_to_column_with_properties(e, &variable_columns)
                    })
                    .transpose()?;

                let column2 = agg_expr
                    .expression2
                    .as_ref()
                    .map(|e| {
                        self.resolve_expression_to_column_with_properties(e, &variable_columns)
                    })
                    .transpose()?;

                Ok(PhysicalAggregateExpr {
                    function: convert_aggregate_function(agg_expr.function),
                    column,
                    column2,
                    distinct: agg_expr.distinct,
                    alias: agg_expr.alias.clone(),
                    percentile: agg_expr.percentile,
                    separator: agg_expr.separator.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        // Build output schema and column names
        let mut output_schema = Vec::new();
        let mut output_columns = Vec::new();

        // Add group-by columns
        for expr in &agg.group_by {
            output_schema.push(LogicalType::Any); // Group-by values can be any type
            output_columns.push(expression_to_string(expr));
        }

        // Add aggregate result columns
        for agg_expr in &agg.aggregates {
            let result_type = match agg_expr.function {
                LogicalAggregateFunction::Count | LogicalAggregateFunction::CountNonNull => {
                    LogicalType::Int64
                }
                LogicalAggregateFunction::Sum => LogicalType::Any,
                LogicalAggregateFunction::Avg => LogicalType::Float64,
                LogicalAggregateFunction::Min | LogicalAggregateFunction::Max => {
                    // MIN/MAX rendent une valeur PRÉSENTE EN ENTRÉE : leur type
                    // déclaré est celui de l'entrée, comme Sum. Le câblage
                    // `Int64` faisait créer un vecteur de sortie Int64 et
                    // vector.rs y poussait 0 en silence pour toute valeur d'un
                    // autre type — min() d'une colonne chaîne rendait Int64(0).
                    LogicalType::Any
                }
                LogicalAggregateFunction::Collect => LogicalType::Any, // List type (using Any since List is a complex type)
                LogicalAggregateFunction::GroupConcat => LogicalType::String,
                LogicalAggregateFunction::Sample => LogicalType::Any,
                // Statistical functions return Float64
                LogicalAggregateFunction::StdDev
                | LogicalAggregateFunction::StdDevPop
                | LogicalAggregateFunction::Variance
                | LogicalAggregateFunction::VariancePop
                | LogicalAggregateFunction::PercentileDisc
                | LogicalAggregateFunction::PercentileCont
                | LogicalAggregateFunction::CovarSamp
                | LogicalAggregateFunction::CovarPop
                | LogicalAggregateFunction::Corr
                | LogicalAggregateFunction::RegrSlope
                | LogicalAggregateFunction::RegrIntercept
                | LogicalAggregateFunction::RegrR2
                | LogicalAggregateFunction::RegrSxx
                | LogicalAggregateFunction::RegrSyy
                | LogicalAggregateFunction::RegrSxy
                | LogicalAggregateFunction::RegrAvgx
                | LogicalAggregateFunction::RegrAvgy => LogicalType::Float64,
                // REGR_COUNT returns Int64
                LogicalAggregateFunction::RegrCount => LogicalType::Int64,
            };
            output_schema.push(result_type);
            output_columns.push(
                agg_expr
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}(...)", agg_expr.function).to_lowercase()),
            );
        }

        // Register all aggregate output columns as scalar (group-by values and
        // aggregate results are materialized scalar values, not entity references)
        for col in &output_columns {
            self.scalar_columns.borrow_mut().insert(col.clone());
        }

        // Choose operator based on whether there are group-by columns
        let mut operator: Box<dyn Operator> = if group_columns.is_empty() {
            Box::new(SimpleAggregateOperator::new(
                input_op,
                physical_aggregates,
                output_schema,
            ))
        } else {
            Box::new(HashAggregateOperator::new(
                input_op,
                group_columns,
                physical_aggregates,
                output_schema,
            ))
        };

        // Apply HAVING clause filter if present
        if let Some(having_expr) = &agg.having {
            // Build variable to column mapping for the aggregate output
            let having_var_columns: HashMap<String, usize> = output_columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();

            let filter_expr = self.convert_expression(having_expr)?;
            let predicate = ExpressionPredicate::new(
                filter_expr,
                having_var_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            operator = Box::new(FilterOperator::new(operator, Box::new(predicate)));
        }

        Ok((operator, output_columns))
    }

    /// Checks if an aggregate is simple enough for factorized execution.
    ///
    /// Simple aggregates:
    /// - COUNT(*) or COUNT(variable)
    /// - SUM, AVG, MIN, MAX on variables (not properties for now)
    pub(super) fn is_simple_aggregate(&self, agg: &AggregateOp) -> bool {
        agg.aggregates.iter().all(|agg_expr| {
            match agg_expr.function {
                LogicalAggregateFunction::Count | LogicalAggregateFunction::CountNonNull => {
                    // COUNT(*) is always OK, COUNT(var) is OK
                    agg_expr.expression.is_none()
                        || matches!(&agg_expr.expression, Some(LogicalExpression::Variable(_)))
                }
                LogicalAggregateFunction::Sum
                | LogicalAggregateFunction::Avg
                | LogicalAggregateFunction::Min
                | LogicalAggregateFunction::Max => {
                    // For now, only support when expression is a variable
                    // (property access would require flattening first)
                    matches!(&agg_expr.expression, Some(LogicalExpression::Variable(_)))
                }
                // Other aggregates (Collect, StdDev, Percentile) not supported in factorized form
                _ => false,
            }
        })
    }

    /// Plans a factorized aggregate that operates directly on factorized data.
    ///
    /// This avoids the O(n²) cost of flattening before aggregation.
    pub(super) fn plan_factorized_aggregate(
        &self,
        agg: &AggregateOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Build the expand chain - this returns a LazyFactorizedChainOperator
        let expands = Self::collect_expand_chain(&agg.input);
        if expands.is_empty() {
            return Err(Error::Internal(
                "Expected expand chain for factorized aggregate".to_string(),
            ));
        }

        // Get the base operator (before first expand)
        let first_expand = expands[0];
        let (base_op, base_columns) = self.plan_operator(&first_expand.input)?;

        let mut columns = base_columns.clone();
        let mut steps = Vec::new();
        let mut is_first = true;

        for expand in &expands {
            // Find source column for this expand
            let source_column = if is_first {
                base_columns
                    .iter()
                    .position(|c| c == &expand.from_variable)
                    .ok_or_else(|| {
                        Error::Internal(format!(
                            "Source variable '{}' not found in base columns",
                            expand.from_variable
                        ))
                    })?
            } else {
                1 // Target from previous level
            };

            let direction = match expand.direction {
                ExpandDirection::Outgoing => Direction::Outgoing,
                ExpandDirection::Incoming => Direction::Incoming,
                ExpandDirection::Both => Direction::Both,
            };

            steps.push(ExpandStep {
                source_column,
                direction,
                edge_types: expand.edge_types.clone(),
            });

            let edge_col_name = expand.edge_variable.clone().unwrap_or_else(|| {
                let count = self.anon_edge_counter.get();
                self.anon_edge_counter.set(count + 1);
                format!("_anon_edge_{}", count)
            });
            columns.push(edge_col_name);
            columns.push(expand.to_variable.clone());

            is_first = false;
        }

        // Create the lazy factorized chain operator
        let mut lazy_op = LazyFactorizedChainOperator::new(
            Arc::clone(&self.store) as Arc<dyn GraphStore>,
            base_op,
            steps,
        );

        if let Some(transaction_id) = self.transaction_id {
            lazy_op = lazy_op.with_transaction_context(self.viewing_epoch, Some(transaction_id));
        } else {
            lazy_op = lazy_op.with_transaction_context(self.viewing_epoch, None);
        }

        // Convert logical aggregates to factorized aggregates
        let factorized_aggs: Vec<FactorizedAggregate> = agg
            .aggregates
            .iter()
            .map(|agg_expr| {
                match agg_expr.function {
                    LogicalAggregateFunction::Count | LogicalAggregateFunction::CountNonNull => {
                        // COUNT(*) uses simple count, COUNT(col) uses column count
                        if agg_expr.expression.is_none() {
                            FactorizedAggregate::count()
                        } else {
                            // For COUNT(variable), we use the deepest level's target column
                            // which is the last column added to the schema
                            FactorizedAggregate::count_column(1) // Target is at index 1 in deepest level
                        }
                    }
                    LogicalAggregateFunction::Sum => {
                        // SUM on deepest level target
                        FactorizedAggregate::sum(1)
                    }
                    LogicalAggregateFunction::Avg => FactorizedAggregate::avg(1),
                    LogicalAggregateFunction::Min => FactorizedAggregate::min(1),
                    LogicalAggregateFunction::Max => FactorizedAggregate::max(1),
                    _ => {
                        // Shouldn't reach here due to is_simple_aggregate check
                        FactorizedAggregate::count()
                    }
                }
            })
            .collect();

        // Build output column names
        let output_columns: Vec<String> = agg
            .aggregates
            .iter()
            .map(|agg_expr| {
                agg_expr
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}(...)", agg_expr.function).to_lowercase())
            })
            .collect();

        // Register output columns as scalar (aggregate results are materialized
        // scalar values, not entity references). Without this, a post-Return
        // projection would treat them as node IDs and attempt NodeResolve, which
        // corrupts the result on 3+ hop queries.
        for col in &output_columns {
            self.scalar_columns.borrow_mut().insert(col.clone());
        }

        // Create the factorized aggregate operator
        let factorized_agg_op = FactorizedAggregateOperator::new(lazy_op, factorized_aggs);

        Ok((Box::new(factorized_agg_op), output_columns))
    }

    /// Resolves a logical expression to a column index, using projected property columns.
    ///
    /// This is used for aggregations where properties have been projected into their own columns.
    pub(super) fn resolve_expression_to_column_with_properties(
        &self,
        expr: &LogicalExpression,
        variable_columns: &HashMap<String, usize>,
    ) -> Result<usize> {
        match expr {
            LogicalExpression::Variable(name) => variable_columns
                .get(name)
                .copied()
                .ok_or_else(|| Error::Internal(format!("Variable '{}' not found", name))),
            LogicalExpression::Property { variable, property } => {
                // Look up the projected property column (e.g., "p_price" for p.price)
                let col_name = format!("{}_{}", variable, property);
                variable_columns.get(&col_name).copied().ok_or_else(|| {
                    Error::Internal(format!(
                        "Property column '{}' not found (from {}.{})",
                        col_name, variable, property
                    ))
                })
            }
            _ => {
                // Complex expression (CASE, Binary, etc.): look up synthetic column
                let col_name = format!("__expr_{:?}", expr);
                variable_columns.get(&col_name).copied().ok_or_else(|| {
                    Error::Internal(format!("Cannot resolve expression to column: {:?}", expr))
                })
            }
        }
    }

    /// Tries to optimize simple COUNT aggregates to avoid scanning.
    ///
    /// Patterns optimized:
    /// - `MATCH (n) RETURN count(n)` → `store.node_count()`
    /// - `MATCH (n:Label) RETURN count(n)` → `store.nodes_by_label("Label").len()`
    /// - `MATCH (n) RETURN count(*)` → `store.node_count()`
    ///
    /// Returns `Some((operator, columns))` if optimized, `None` otherwise.
    fn try_plan_count_shortcut(
        &self,
        agg: &AggregateOp,
    ) -> Option<(Box<dyn Operator>, Vec<String>)> {
        // Only optimize: no GROUP BY, no HAVING, single aggregate
        if !agg.group_by.is_empty() || agg.having.is_some() || agg.aggregates.len() != 1 {
            return None;
        }

        // (2026-08-30, vague 4a — isolation MVCC) DES ÉCRITURES NON COMMITÉES
        // SONT EN ATTENTE : les compteurs globaux (`node_count`,
        // `node_count_by_label`, `edge_count`) les incluent TOUTES, alors que
        // ce lecteur ne doit voir que les siennes. Le raccourci répondrait
        // deux quand le balayage — qui filtre par `filtrer_par_epoque_et_tx`
        // — ne rend qu'une ligne : le graphe se décrirait autrement qu'il ne
        // se laisse lire (gql_13, le_compte_n_inclut_pas_l_ecriture_non_
        // commitee_d_une_autre_session). On renonce au raccourci et le plan
        // repasse par le balayage filtré : toujours juste, brièvement plus
        // lent — l'état « écritures en attente » est transitoire par nature.
        if self.store.has_pending_writes() {
            return None;
        }

        let agg_expr = &agg.aggregates[0];

        // Must be COUNT or CountNonNull
        let is_count = matches!(
            agg_expr.function,
            LogicalAggregateFunction::Count | LogicalAggregateFunction::CountNonNull
        );
        if !is_count {
            return None;
        }

        // The expression must be None (COUNT(*)) or a Variable matching the scan variable
        let count_var = match &agg_expr.expression {
            None => None, // COUNT(*)
            Some(LogicalExpression::Variable(var)) => Some(var.as_str()),
            _ => return None, // COUNT(n.prop) etc. — can't shortcut
        };

        // (2026-08-21) LE RACCOURCI D'ARÊTES — ÉCRIT, RÉVOQUÉ, PUIS RENDU.
        //
        // `plan_aggregate` promettait plus haut que
        // `MATCH ()-[r]->() RETURN count(r)` passerait par `edge_count()` en
        // O(1). C'était vrai de la fonction et faux du plan.
        //
        // La première tentative rendait un NOMBRE FAUX, mesuré sur une copie de
        // la base `agent` : `edge_count()` annonçait 39 784 pour 37 476 arêtes
        // réelles, et `IN_MISSION` 4 660 pour 2 347 (×1,99). Six types sur neuf
        // divergeaient. Les deux plans — par nœud et par arête — s'accordaient
        // pourtant entre eux : ce n'était pas le parcours qui mentait, c'était
        // le compteur.
        //
        // ── CE QUI A CHANGÉ, ET POURQUOI LE RACCOURCI REVIENT ──
        //
        // La dérive était FIGÉE dans l'instantané, pas produite en continu :
        // vérifié expérimentalement — après réparation persistée, elle ne
        // revient pas. Depuis les bases v5 les compteurs sont restaurés en O(1)
        // et plus rien ne les confrontait au disque.
        //
        // Le substrat les VÉRIFIE désormais à l'ouverture, après la fusion du
        // WAL (le seul point où « recompter » veut dire quelque chose : plus
        // tôt, le filigrane porte encore la valeur de l'instantané et le
        // balayage s'arrête au même endroit). Coût mesuré : sous le plancher de
        // mesure sur 507 366 arêtes, ~1,2 s extrapolé sur 119 millions — parce
        // que la partie chère, les histogrammes de labels d'extrémités, en a
        // été séparée.
        //
        // Deux fuites de comptage ont aussi été bouchées : `add_label` et
        // `remove_label` écrivaient le bitset sans incrémenter, ce qui faisait
        // rendre 0 à `MATCH (n:B) RETURN count(n)` après un `SET n:B` sur 40
        // nœuds, pendant que le balayage en rendait 40.
        //
        // ── LA GARDE ──
        //
        // Un raccourci de comptage trop large ne rend pas un résultat lent : il
        // rend un nombre faux. Chaque condition exclut une forme où le compteur
        // ne répondrait pas à la question posée :
        //
        //  * `edge_variable` présent et égal à la variable comptée ;
        //  * un seul saut — une longueur variable compte des CHEMINS ;
        //  * pas d'alias de chemin, mode `Walk` — les autres filtrent les
        //    répétitions ;
        //  * direction `Outgoing` — `Both` traverse chaque arête deux fois ;
        //  * l'entrée est un `NodeScan` NU : ni label (qui filtrerait par la
        //    source), ni entrée propre.
        //
        // Et une garde de plus que la première version, celle qui manquait :
        //
        //  * AUCUNE ARÊTE INTER-BASE. `ExpandOperator::charger_aretes_externes`
        //    traverse aussi les arêtes externes pour toute direction non
        //    `Incoming`, alors que `create_external_edge` n'incrémente aucun
        //    compteur — elles vivent dans un sidecar jamais persisté. Le chemin
        //    générique les voit, le compteur ne les a jamais vues : sur une
        //    session multi-base, le raccourci SOUS-COMPTERAIT. `external_edge_count()`
        //    est un `len()` de DashMap, donc le contrôle est gratuit.
        //
        // Le test de parité `compteurs_et_balayage_s_accordent_apres_reouverture`
        // (obrain-substrate) garde la porte : il rougira si les compteurs
        // redeviennent indignes de confiance.
        if limit_ok_pour_raccourci_arete(self.store.as_ref())
            && let LogicalOperator::Expand(exp) = agg.input.as_ref()
            && let Some(var_arete) = exp.edge_variable.as_deref()
            && count_var.is_none_or(|v| v == var_arete)
            && exp.min_hops == 1
            && exp.max_hops == Some(1)
            && exp.path_alias.is_none()
            && exp.path_mode == crate::query::plan::PathMode::Walk
            && exp.direction == crate::query::plan::ExpandDirection::Outgoing
            && let LogicalOperator::NodeScan(scan) = exp.input.as_ref()
            && scan.label.is_none()
            && scan.input.is_none()
        {
            let compte: Option<i64> = if exp.edge_types.is_empty() {
                Some(self.store.edge_count() as i64)
            } else {
                // Les `edge_type` sont DISJOINTS — un `u16` par enregistrement —
                // donc la somme est exacte, sans double compte.
                //
                // `None` signifie « ce magasin ne sait pas compter par type » et
                // NON « zéro » : on renonce alors au raccourci plutôt que
                // d'inventer. C'est le contrat explicite d'`edge_counts_by_type`.
                self.store.edge_counts_by_type().map(|par_type| {
                    exp.edge_types
                        .iter()
                        .map(|t| par_type.get(t).copied().unwrap_or(0) as i64)
                        .sum()
                })
            };

            if let Some(n) = compte {
                let alias = agg_expr
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}(...)", agg_expr.function).to_lowercase());
                self.scalar_columns.borrow_mut().insert(alias.clone());
                let op: Box<dyn Operator> = Box::new(ScalarResultOperator::new(
                    vec![Value::Int64(n)],
                    vec![LogicalType::Int64],
                ));
                return Some((op, vec![alias]));
            }
        }

        // Input must be a simple NodeScan (no filter, no input)
        match agg.input.as_ref() {
            LogicalOperator::NodeScan(scan) if scan.input.is_none() => {
                // If COUNT(var), verify var matches the scan variable
                if let Some(var) = count_var
                    && var != scan.variable
                {
                    return None;
                }

                let count = if let Some(label) = &scan.label {
                    // MATCH (n:Label) RETURN count(n) → O(1) label count
                    self.store.node_count_by_label(label) as i64
                } else {
                    // MATCH (n) RETURN count(n) → node_count()
                    self.store.node_count() as i64
                };

                let alias = agg_expr
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}(...)", agg_expr.function).to_lowercase());

                // Register as scalar column
                self.scalar_columns.borrow_mut().insert(alias.clone());

                let op: Box<dyn Operator> = Box::new(ScalarResultOperator::new(
                    vec![Value::Int64(count)],
                    vec![LogicalType::Int64],
                ));

                Some((op, vec![alias]))
            }
            _ => None,
        }
    }

    /// (NATIF-J6) Fast-path `min(n.<prop>)` / `max(n.<prop>)` via l index
    /// ORDONNE PAR VALEUR : les extremites de la serie triee par
    /// `comparer_total` (le comparateur EXACT du SORT), donc EXACTES et en
    /// O(1), au lieu de balayer tous les noeuds. `None` — l appelant retombe
    /// alors sur l agregat balayant, jamais faux — des qu une condition manque
    /// (GROUP BY, HAVING, ecritures en attente, entree non-NodeScan nu ou avec
    /// label, expression non-Property, propriete non indexee par valeur, serie
    /// vide). Les NULL sont exclus de l index, exactement comme l agregat.
    ///
    /// Plusieurs agregats min/max sur la MEME variable scannee sont servis
    /// ensemble (ex. `RETURN min(n.p), max(n.p)`), chacun en O(1).
    fn try_plan_minmax_shortcut(
        &self,
        agg: &AggregateOp,
    ) -> Option<(Box<dyn Operator>, Vec<String>)> {
        if !agg.group_by.is_empty() || agg.having.is_some() || agg.aggregates.is_empty() {
            return None;
        }
        // (NATIF-J8) Garde consciente de l'epoque de l'index, comme la route
        // ORDER BY. `has_pending_writes` est aveugle aux ecritures auto-commit
        // (SYSTEM), qui commitent sans l'armer mais avancent le WAL et laissent
        // l'index en retard. On exige donc AUSSI que l'index couvre l'etat
        // committe courant, et que la vue soit au present (un index bati au
        // present ne sert pas une vue time-travel). Sinon repli sur l'agregat
        // balayant, jamais faux.
        if self.store.has_pending_writes()
            || self.viewing_epoch != self.store.current_epoch()
            || !self.store.index_ord_valeur_couvre_courant()
        {
            return None;
        }
        // L entree doit etre un NodeScan NU : pas de filtre (pas d input propre)
        // et pas de label — l index ordonne par valeur est GLOBAL. Un label
        // restreint fait renoncer (repli balayage), jamais un resultat faux.
        let LogicalOperator::NodeScan(scan) = agg.input.as_ref() else {
            return None;
        };
        if scan.input.is_some() || scan.label.is_some() {
            return None;
        }

        let mut valeurs: Vec<Value> = Vec::with_capacity(agg.aggregates.len());
        let mut schema: Vec<LogicalType> = Vec::with_capacity(agg.aggregates.len());
        let mut colonnes: Vec<String> = Vec::with_capacity(agg.aggregates.len());
        for agg_expr in &agg.aggregates {
            if agg_expr.expression2.is_some() {
                return None;
            }
            let est_max = if matches!(agg_expr.function, LogicalAggregateFunction::Max) {
                true
            } else if matches!(agg_expr.function, LogicalAggregateFunction::Min) {
                false
            } else {
                return None;
            };
            let Some(LogicalExpression::Property { variable, property }) = &agg_expr.expression
            else {
                return None;
            };
            if *variable != scan.variable {
                return None;
            }
            // `None` => index absent / non fiable / serie vide : on renonce au
            // raccourci pour CETTE requete (repli balayage, jamais faux).
            let v = if est_max {
                self.store.max_par_index(property)?
            } else {
                self.store.min_par_index(property)?
            };
            valeurs.push(v);
            // Type Any, jamais Int64 : sinon 0 pousse en silence sur une colonne
            // chaine (meme raison qu au chemin generique des agregats).
            schema.push(LogicalType::Any);
            let alias = agg_expr
                .alias
                .clone()
                .unwrap_or_else(|| format!("{:?}(...)", agg_expr.function).to_lowercase());
            self.scalar_columns.borrow_mut().insert(alias.clone());
            colonnes.push(alias);
        }

        let op: Box<dyn Operator> = Box::new(ScalarResultOperator::new(valeurs, schema));
        Some((op, colonnes))
    }

    /// (2026-09-14) COMPTES GROUPÉS PAR LABEL / TYPE D'ARÊTE, EN UN BALAYAGE.
    ///
    /// ── LE PROBLÈME, MESURÉ ─────────────────────────────────────────────
    ///
    /// Deux agrégats sont payés à CHAQUE exécution par les écrans d'état et les
    /// vues agrégées, et traversent des routes chaudes :
    ///
    ///   · `MATCH ()-[r]->() RETURN type(r), count(r) ORDER BY c DESC LIMIT 20`
    ///     — 919 à 933 ms sur `runtime-dev` figé (1,29 M arêtes), 231 ms sur
    ///     `runtime-full`, 3,9 ms sur `iam` ;
    ///   · `MATCH (n) RETURN labels(n)[0], count(n) ORDER BY c DESC LIMIT k`
    ///     — 149 à 171 ms, 122 ms, 3,7 ms sur les mêmes.
    ///
    /// Or le substrat sait déjà répondre à ces deux questions par UN parcours
    /// de zone : `edge_counts_by_type()` coûte **23 ms** sur le même store figé,
    /// contre 920 ms pour la requête — 40×. Ce n'est pas le travail qui coûte,
    /// c'est sa FORME : le chemin générique matérialise 1,29 M de lignes, les
    /// traverse d'un `ProjectOperator`, puis les hache. `edge_counts_by_type`,
    /// lui, agrège sur l'enregistrement pendant la lecture mmap séquentielle,
    /// sous UN SEUL verrou de zone.
    ///
    /// ── LA VOIE CHOISIE : LE BALAYAGE, PAS LE REGISTRE ──────────────────
    ///
    /// Trois voies étaient ouvertes. La moins invasive qui donne le gain est
    /// retenue, et la plus rapide est REFUSÉE — c'est un choix, pas un oubli.
    ///
    /// 1. REGISTRE MAINTENU À L'ÉCRITURE (`label → count`, `type → count`).
    ///    Le plus rapide (µs) et le plus RISQUÉ. Le dépôt l'a DÉJÀ essayé et
    ///    l'a RETIRÉ : `edge_type_live_counts` a annoncé « IN_MISSION 4 660 »
    ///    pour 2 347 arêtes réelles, six types sur neuf divergents, dans les
    ///    deux sens (voir le pavé d'`edge_counts_by_type`,
    ///    `crates/obrain-substrate/src/store.rs`). « Le balayage, lui, n'a
    ///    jamais menti. »
    ///
    ///    Pire : pour les LABELS, cette voie ne peut pas être réparée.
    ///    `node_count_by_label` compte les nœuds qui PORTENT le label ; la
    ///    requête compte chaque nœud sous son PREMIER label. Les 15 nœuds
    ///    `[Task, Node]` / `[Step, Task]` mesurés sur `runtime-dev` le
    ///    2026-09-14 tomberaient dans DEUX groupes, et la somme des groupes
    ///    (422 831) dépasserait `count(n)` (422 816). Aucune invalidation ne
    ///    corrige cela : la donnée dont la requête a besoin n'est pas dans le
    ///    registre — ce n'est pas un problème de fraîcheur mais de PROJECTION.
    ///
    /// 2. CACHE MÉMOIRE PAR (store, époque). Simple, mais il ne vit pas entre
    ///    processus — donc rien pour le CLI ni pour un lecteur RO fraîchement
    ///    lancé — et surtout il ne sert plus à grand-chose une fois le balayage
    ///    groupé ramené à 23 ms. L'invalider aurait demandé, soit de faire
    ///    confiance à `sync_epoch` (que le MOTEUR appelle, pas le magasin : une
    ///    écriture qui l'oublie sert un compte périmé), soit de tenir un
    ///    compteur de mutations — c'est-à-dire de rebâtir le registre qu'on
    ///    vient de refuser. NON LIVRÉ, délibérément : voir « FRAÎCHEUR » plus
    ///    bas.
    ///
    /// 3. BALAYAGE GROUPÉ, RECALCULÉ À CHAQUE EXÉCUTION. **C'est ce qui est
    ///    livré.** Aucun état, aucune invalidation, aucune migration, aucun
    ///    risque de péremption.
    ///
    /// ── FRAÎCHEUR : LA GARANTIE, ÉCRITE NOIR SUR BLANC ──────────────────
    ///
    /// Le raccourci ne consulte AUCUN compteur et ne conserve AUCUN état. Il
    /// parcourt les enregistrements au moment de la requête. Il ne peut donc
    /// pas être périmé :
    ///
    ///   · après un flush WAL, une reprise, un `INGEST`, une suppression de
    ///     nœud ou d'arête : la réponse suit l'état des zones, comme le
    ///     parcours générique — il n'y a rien à rafraîchir ;
    ///   · store en LECTURE SEULE : aucune écriture disque, aucun sidecar,
    ///     juste une lecture mmap — un lecteur RO obtient exactement ce qu'un
    ///     écrivain obtiendrait sur le même état ;
    ///   · stores existants : aucune colonne, aucun sidecar, aucune version de
    ///     format n'est requise — donc aucune migration, et rien à réparer ;
    ///   · mémoire : bornée par le nombre de groupes (≤ 64 labels, quelques
    ///     dizaines de types d'arêtes), pas par le nombre d'entités. Le
    ///     groupement intermédiaire se fait sur le BITSET de labels, dont le
    ///     nombre de valeurs distinctes est celui des combinaisons de labels.
    ///
    /// LA LIMITE, ET ELLE EST EXACTE : le parcours lit les zones, pas les
    /// chaînes de versions. Il n'est donc employé que lorsque le plan regarde
    /// l'époque COURANTE, hors transaction, et sans écriture en attente — les
    /// trois gardes correspondantes sont ci-dessous. Toute autre situation
    /// repart sur le plan historique, qui filtre.
    ///
    /// Le coût restant est celui du parcours lui-même (23 ms pour 1,29 M
    /// arêtes, ~4 ms pour 423 k nœuds) : c'est le plancher d'un comptage exact
    /// sans index persistant. Descendre en dessous demanderait un index — et
    /// c'est précisément la voie 1, refusée.
    ///
    /// ── LA GARANTIE D'IDENTITÉ ──────────────────────────────────────────
    ///
    /// La réponse est identique à celle d'aujourd'hui PAR CONSTRUCTION : les
    /// deux sources (`edge_counts_by_type`, `node_counts_by_first_label`)
    /// parcourent les MÊMES enregistrements avec les MÊMES filtres que le
    /// chemin générique — tombstoned exclu, extrémités nulles exclues pour les
    /// arêtes, premier bit allumé DU REGISTRE pour les labels — et ne font que
    /// remplacer « matérialiser puis hacher » par « compter en passant ».
    ///
    /// ── LES GARDES, ET CE QUE CHACUNE ÉVITE ─────────────────────────────
    ///
    /// Un raccourci d'agrégat trop large ne rend pas un résultat lent : il rend
    /// un nombre FAUX. Chaque condition ferme une forme où le parcours brut ne
    /// répondrait pas à la question posée :
    ///
    ///  · un seul agrégat, pas de `HAVING` ;
    ///  · l'agrégat est `COUNT` / `COUNT_NON_NULL` SANS `DISTINCT` — un
    ///    `count(DISTINCT x)` compte autre chose que des lignes ;
    ///  · l'agrégat porte sur la variable GROUPÉE (`count(r)` avec `type(r)`,
    ///    `count(n)` avec `labels(n)[0]`) : `count(m)` groupé par `type(r)`
    ///    demanderait une jointure, que le parcours ne fait pas ;
    ///  · les clés de groupe SECONDAIRES doivent être des CONSTANTES. Une
    ///    constante ne coupe aucun groupe (chaque ligne porte la même), donc
    ///    elle ne change pas les comptes — mais elle doit être REPRODUITE dans
    ///    les colonnes de sortie, ce que l'opérateur fait. C'est aussi ce qui
    ///    rend le protocole de mesure à nonce valide : `'<nonce>' AS k` est
    ///    ajouté au `GROUP BY` par le traducteur, et le nonce ne doit pas
    ///    fermer le raccourci ;
    ///  · AUCUNE écriture non commitée (`has_pending_writes`) : le parcours de
    ///    zone ignore `filtrer_par_epoque_et_tx` et verrait les écritures en
    ///    attente de TOUTES les transactions — même garde que
    ///    `try_plan_count_shortcut`, pour la même raison (gql_13) ;
    ///  · le plan regarde l'époque COURANTE et n'est pas dans une transaction :
    ///    un lecteur épinglé sur une époque antérieure verrait un sous-ensemble
    ///    que le parcours brut ne sait pas reproduire ;
    ///  · arêtes : motif `()-[r]->()` NU — un saut, `Walk`, sans alias de
    ///    chemin, sortant, sans filtre de type, sur un `NodeScan` sans label ni
    ///    entrée — PLUS la garde d'arêtes inter-bases
    ///    (`limit_ok_pour_raccourci_arete`) : elles ne sont dans aucun
    ///    parcours de zone ni dans aucun compteur, et le chemin générique, lui,
    ///    les traverse ;
    ///  · nœuds : `MATCH (n)` NU, sans label — `MATCH (f:File)` groupé par
    ///    `labels(f)[0]` ne verrait que le sous-ensemble, et le parcours, lui,
    ///    voit tout le store.
    ///
    /// Toute forme non reconnue rend `None` et repart sur le plan historique,
    /// INCHANGÉ. Le raccourci ne peut donc que accélérer : il ne peut pas
    /// changer une réponse qu'il n'a pas produite.
    fn try_plan_group_count_shortcut(
        &self,
        agg: &AggregateOp,
    ) -> Option<(Box<dyn Operator>, Vec<String>)> {
        // ── ÉCHAPPATOIRE, ET ELLE EST DOCUMENTÉE ────────────────────────────
        //
        // `OBRAIN_AGREGAT_GROUPE=0` rend le comportement d'AVANT : le parcours
        // générique, plan par plan, tel quel — et c'est aussi l'instrument qui
        // permet de mesurer les deux régimes avec UN SEUL binaire.
        //
        // Elle existe pour un cas précis, MESURÉ, et il faut le dire plutôt que
        // de l'enfouir. Sur un store dont les CHAÎNES D'ADJACENCE sont abîmées
        // — c'est le cas de `runtime-dev`, que le substrat documente lui-même
        // (5 nœuds à chaîne cyclique signalés par `walk_outgoing_chain`, une
        // corruption « pré-splice_lock », et une réparation
        // `repair_outgoing_chain` qui TRONQUE la chaîne en perdant les maillons
        // suivants) — le parcours ne rend PAS le contenu de la zone :
        //
        //     MATCH ()-[r:EVOLVES]->() RETURN type(r), count(r)   →  2
        //     MATCH ()-[r]->()      RETURN type(r), count(r)      → 13
        //
        // DEUX RÉPONSES À LA MÊME QUESTION, dans le même binaire d'AVANT. Le
        // balayage rend 2 — comme `count(r)` (1 158 516) et comme le chemin
        // typé. Sur `runtime-prod` (7,3 Go), `runtime-full`, `runtime-full-v2`,
        // `iam`, `atlas` et `self-demo`, les deux chemins s'accordent
        // EXACTEMENT : 7 stores sur 9 identiques, les 2 divergents étant les
        // deux incarnations de `runtime-dev`, toutes deux issues de la même
        // corruption. Le défaut est donc AMONT — dans les chaînes, pas dans
        // l'agrégat — et cette échappatoire permet de retrouver l'ancien chiffre
        // le temps de le réparer, sans recompiler.
        if std::env::var("OBRAIN_AGREGAT_GROUPE").is_ok_and(|v| v == "0") {
            return None;
        }
        let Some(cle) = agg.group_by.first() else {
            return None;
        };
        if agg.aggregates.len() != 1 || agg.having.is_some() {
            return None;
        }
        if self.store.has_pending_writes() {
            return None;
        }
        if self.transaction_id.is_some() || self.viewing_epoch != self.store.current_epoch() {
            return None;
        }

        // Les clés SECONDAIRES : des constantes uniquement. Elles ne coupent
        // aucun groupe mais doivent figurer dans les colonnes de sortie.
        let mut constantes: Vec<Value> = Vec::with_capacity(agg.group_by.len() - 1);
        for expr in &agg.group_by[1..] {
            match expr {
                LogicalExpression::Literal(v) => constantes.push(v.clone()),
                _ => return None,
            }
        }

        let agg_expr = &agg.aggregates[0];
        if agg_expr.distinct || agg_expr.expression2.is_some() || agg_expr.percentile.is_some() {
            return None;
        }
        if !matches!(
            agg_expr.function,
            LogicalAggregateFunction::Count | LogicalAggregateFunction::CountNonNull
        ) {
            return None;
        }
        // `COUNT(*)` (expression absente) compte les lignes ; chaque ligne porte
        // exactement une valeur de la clé groupée, c'est donc la même question.
        let count_var = match &agg_expr.expression {
            None => None,
            Some(LogicalExpression::Variable(v)) => Some(v.as_str()),
            _ => return None,
        };

        // Le noyau reconnu : le genre de clé, et la variable qu'elle décrit.
        // `type(r)` apparaît sous deux formes selon le chemin du traducteur
        // (`Type`, ou `FunctionCall { name: "type" }`) — les deux sont
        // acceptées, aucune n'est devinée ; le diagnostic plan par plan a
        // montré `FunctionCall` sur ce store.
        let (par_arete, variable) = match cle {
            LogicalExpression::Type(v) => (true, v.as_str()),
            LogicalExpression::FunctionCall {
                name,
                args,
                distinct: false,
            } if name.eq_ignore_ascii_case("type") && args.len() == 1 => match &args[0] {
                LogicalExpression::Variable(v) => (true, v.as_str()),
                _ => return None,
            },
            LogicalExpression::IndexAccess { base, index } => {
                // Seulement `[0]` : `labels(n)[1]` n'est pas la clé `null` des
                // nœuds sans label, c'est une AUTRE question.
                if !matches!(index.as_ref(), LogicalExpression::Literal(Value::Int64(0))) {
                    return None;
                }
                let v = match base.as_ref() {
                    LogicalExpression::Labels(v) => v.as_str(),
                    LogicalExpression::FunctionCall {
                        name,
                        args,
                        distinct: false,
                    } if name.eq_ignore_ascii_case("labels") && args.len() == 1 => match &args[0] {
                        LogicalExpression::Variable(v) => v.as_str(),
                        _ => return None,
                    },
                    _ => return None,
                };
                (false, v)
            }
            _ => return None,
        };

        if let Some(v) = count_var
            && v != variable
        {
            return None;
        }

        // Les lignes groupées, telles que le parcours les rend.
        let groupes: Vec<(Value, i64)> = if par_arete {
            if !limit_ok_pour_raccourci_arete(self.store.as_ref()) {
                return None;
            }
            // ── ÉCHAPPATOIRE, ET ELLE EST DOCUMENTÉE ────────────────────────
            //
            // Elle est posée en tête de `try_plan_group_count_shortcut`
            // (`OBRAIN_AGREGAT_GROUPE=0`) ; le pavé qui l'explique est là-bas.
            let LogicalOperator::Expand(exp) = agg.input.as_ref() else {
                return None;
            };
            if exp.edge_variable.as_deref() != Some(variable)
                || exp.min_hops != 1
                || exp.max_hops != Some(1)
                || exp.path_alias.is_some()
                || exp.path_mode != super::PathMode::Walk
                || exp.direction != ExpandDirection::Outgoing
                || !exp.edge_types.is_empty()
            {
                return None;
            }
            let LogicalOperator::NodeScan(scan) = exp.input.as_ref() else {
                return None;
            };
            if scan.label.is_some() || scan.input.is_some() {
                return None;
            }
            let mut par_type: Vec<(String, usize)> = self
                .store
                .walked_edge_counts_by_type()?
                .into_iter()
                .collect();
            // Ordre rendu DÉTERMINÉ (clé croissante). Le chemin générique, lui,
            // itère une `HashMap` : son ordre est arbitraire. Un `ORDER BY` vit
            // au-dessus dans toutes les requêtes de ce raccourci ; là où il n'y
            // en a pas, l'ordre d'un `GROUP BY` n'est pas spécifié — on choisit
            // donc le seul qui soit reproductible.
            par_type.sort_unstable();
            par_type
                .into_iter()
                .map(|(t, n)| (Value::String(t.into()), n as i64))
                .collect()
        } else {
            let LogicalOperator::NodeScan(scan) = agg.input.as_ref() else {
                return None;
            };
            if scan.label.is_some() || scan.input.is_some() || scan.variable != variable {
                return None;
            }
            let mut par_label: Vec<(Option<String>, usize)> =
                self.store.node_counts_by_first_label()?;
            par_label.sort_unstable();
            par_label
                .into_iter()
                .map(|(l, n)| {
                    let k = match l {
                        Some(nom) => Value::String(nom.into()),
                        // Le groupe des nœuds SANS label : `labels(n)[0]` vaut
                        // `null`, et c'est une LIGNE du résultat, pas une
                        // absence de ligne. Mesuré : 15 nœuds sur `runtime-dev`.
                        None => Value::Null,
                    };
                    (k, n as i64)
                })
                .collect()
        };

        // Chaque groupe précède ses constantes, puis le compte — l'ordre exact
        // du chemin générique (`plan_aggregate` : colonnes de groupe, puis
        // colonnes d'agrégat).
        let lignes: Vec<Vec<Value>> = groupes
            .into_iter()
            .map(|(k, n)| {
                let mut ligne = Vec::with_capacity(1 + constantes.len() + 1);
                ligne.push(k);
                ligne.extend(constantes.iter().cloned());
                ligne.push(Value::Int64(n));
                ligne
            })
            .collect();

        // Noms et types de sortie : les MÊMES expressions que le chemin
        // générique, pour que le parent — tri, projection, `HAVING` d'un plan
        // englobant — retrouve ses colonnes sous le même nom.
        let alias = agg_expr
            .alias
            .clone()
            .unwrap_or_else(|| format!("{:?}(...)", agg_expr.function).to_lowercase());
        let mut colonnes: Vec<String> = agg.group_by.iter().map(expression_to_string).collect();
        colonnes.push(alias);
        for c in &colonnes {
            self.scalar_columns.borrow_mut().insert(c.clone());
        }

        let op: Box<dyn Operator> =
            Box::new(GroupedCountsOperator::new(lignes, agg.group_by.len() + 1));
        Some((op, colonnes))
    }
}

/// Rend des comptes groupés DÉJÀ calculés par le magasin.
///
/// Il n'existe que pour [`super::Planner::try_plan_group_count_shortcut`] : les
/// lignes qu'il rend sont, à la valeur près, celles que `HashAggregateOperator`
/// aurait produites au-dessus du balayage générique — obtenues par UN parcours
/// de zone au lieu de N lignes matérialisées, projetées, puis hachées.
///
/// Le schéma est celui du chemin générique : `Any` pour les clés de groupe (un
/// `VectorData::Generic`, seul type qui porte aussi bien une chaîne qu'un
/// `Null`), `Int64` pour le compte.
struct GroupedCountsOperator {
    lignes: Vec<Vec<Value>>,
    nb_colonnes: usize,
    produit: bool,
}

impl GroupedCountsOperator {
    fn new(lignes: Vec<Vec<Value>>, nb_colonnes: usize) -> Self {
        Self {
            lignes,
            nb_colonnes,
            produit: false,
        }
    }
}

impl Operator for GroupedCountsOperator {
    fn next(&mut self) -> OperatorResult {
        if self.produit {
            return Ok(None);
        }
        self.produit = true;
        if self.lignes.is_empty() {
            // Aucun groupe = aucune ligne. C'est ce que rend un `GROUP BY` sur
            // un ensemble vide — et NON la ligne « count = 0 » d'un agrégat sans
            // group-by, qui est le cas de `try_plan_count_shortcut`.
            return Ok(None);
        }
        let mut schema = vec![LogicalType::Any; self.nb_colonnes - 1];
        schema.push(LogicalType::Int64);
        let mut chunk = DataChunk::with_capacity(&schema, self.lignes.len());
        for ligne in &self.lignes {
            for (i, v) in ligne.iter().enumerate() {
                chunk
                    .column_mut(i)
                    .expect("colonne présente : chunk créé avec le schéma complet")
                    .push_value(v.clone());
            }
        }
        chunk.set_count(self.lignes.len());
        Ok(Some(chunk))
    }

    fn reset(&mut self) {
        self.produit = false;
    }

    fn name(&self) -> &'static str {
        "GroupedCounts"
    }
}

/// Le raccourci de comptage d'arêtes est-il seulement envisageable sur ce
/// magasin ?
///
/// Une seule condition, et elle est structurelle : les arêtes INTER-BASES
/// échappent aux compteurs. `create_external_edge` n'appelle aucun
/// `incr_edge_type_count`, et elles vivent dans un sidecar `DashMap` jamais
/// persisté — reconstruit à l'ouverture. Or `ExpandOperator` les traverse pour
/// toute direction non `Incoming`.
///
/// Le chemin générique les voit donc, et le compteur ne les a jamais vues. Sur
/// une base qui en porte, le raccourci sous-compterait — silencieusement.
///
/// `external_edge_count()` est un `len()` de DashMap : le contrôle coûte moins
/// que la question qu'il évite de mal répondre.
fn limit_ok_pour_raccourci_arete(store: &dyn GraphStore) -> bool {
    store.external_edge_count() == 0
}
