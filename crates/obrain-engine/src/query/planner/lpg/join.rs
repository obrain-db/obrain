//! Join, union, and distinct planning.

use super::{
    ApplyOp, ApplyOperator, DistinctOp, Error, ExceptOp, HashJoinOperator, IntersectOp, JoinOp,
    JoinType, LeapfrogJoinOperator, LogicalExpression, LogicalOperator, MultiWayJoinOp, Operator,
    OtherwiseOp, PhysicalJoinType, Result, UnionOp, common,
};

impl super::Planner {
    /// ── CONTRÔLE DE L'EXPLOSION DES LIENS ────────────────────────────────
    ///
    /// Deux formes produisent un PRODUIT dont la taille est celle des deux côtés
    /// multipliés, et l'engin les exécute en boucle imbriquée :
    ///
    /// ```text
    /// NodeScan (c:*)          ← un scan complet POUR CHAQUE ligne d'en dessous
    ///   Expand (a)->(b)          (`plan_node_scan` en fait un
    ///     NodeScan (a:*)          NestedLoopJoin … PhysicalJoinType::Cross)
    ///
    /// Join (sans condition)   ← appariement sans clé
    /// ```
    ///
    /// Mesuré le 2026-09-12 sur `runtime-dev` : `MATCH (a)-[r]->(b), (c) WHERE
    /// id(b) IN [3] RETURN count(c)` = 1 251 348 arêtes × 406 335 nœuds ≈ 5·10¹¹
    /// paires — et le serveur **mourait**, sans message et sans panique (OOM).
    /// L'engin possède un budget mémoire (`config.memory_limit`, `spill_path`)
    /// mais AUCUN opérateur d'exécution ne le consulte : il ne pouvait donc rien
    /// refuser.
    ///
    /// Le contrôle est posé au passage unique de la planification (`plan_operator`),
    /// pour que l'erreur soit EXPLICABLE — elle dit quoi écrire à la place — au
    /// lieu d'être fatale. La taille est une BORNE INFÉRIEURE tirée des compteurs
    /// du store : on ne refuse que ce qui est prouvé énorme, jamais ce qui est
    /// modeste.
    pub(super) fn garde_explosion(&self, op: &LogicalOperator) -> Result<()> {
        let (gauche, droite, forme) = match op {
            LogicalOperator::NodeScan(scan) => {
                let Some(inner) = scan.input.as_ref() else {
                    return Ok(());
                };
                let lignes_du_scan = match &scan.label {
                    Some(label) => self.store.node_count_by_label(label),
                    None => self.store.node_count(),
                } as u128;
                (
                    self.borne_inferieure(inner),
                    lignes_du_scan.max(1),
                    "scan imbriqué (produit cartésien)",
                )
            }
            LogicalOperator::Join(join) if join.conditions.is_empty() => (
                self.borne_inferieure(&join.left),
                self.borne_inferieure(&join.right),
                "jointure sans condition",
            ),
            _ => return Ok(()),
        };
        let paires = gauche.saturating_mul(droite);
        let plafond = Self::plafond_produit();
        if paires <= plafond {
            return Ok(());
        }
        use obrain_common::utils::error::{QueryError, QueryErrorKind};
        Err(QueryError::new(
            QueryErrorKind::Semantic,
            format!(
                "{forme} refusé : au moins {gauche} × {droite} = {paires} paires (plafond {plafond}). \
                 Les parties du motif ne partagent aucune variable, donc rien ne peut les apparier. \
                 Mesuré le 2026-09-12 sur runtime-dev : 1 251 348 arêtes × 406 335 nœuds a fait \
                 tomber le serveur, sans message. Reliez les parties par une variable commune \
                 (le planificateur en fait alors une jointure amorcée par la graine), bornez par un \
                 LIMIT, ou comptez autrement. `OBRAIN_MAX_PRODUIT=0` lève ce garde."
            ),
        )
        .into())
    }

    /// Plafond du nombre de paires qu'une jointure SANS condition peut produire.
    ///
    /// Un milliard de paires ne tient dans aucune mémoire de cette machine : le
    /// garde vise l'explosion, pas les jointures modestes. `OBRAIN_MAX_PRODUIT`
    /// permet de le régler (`0` = garde levé), sans recompilation.
    pub(super) fn plafond_produit() -> u128 {
        match std::env::var("OBRAIN_MAX_PRODUIT")
            .ok()
            .and_then(|v| v.trim().parse::<u128>().ok())
        {
            Some(0) => u128::MAX,
            Some(v) => v,
            None => 1_000_000_000,
        }
    }

    /// Borne INFÉRIEURE du nombre de lignes que produira un opérateur, d'après
    /// les seuls compteurs du store — O(1), aucun parcours.
    ///
    /// On ne devine jamais : chaque chiffre vient du store. Ce qui n'est pas
    /// prouvable vaut **1**, et une borne basse ne fait que rendre le garde plus
    /// permissif — elle ne peut pas faire refuser une requête modeste.
    pub(super) fn borne_inferieure(&self, op: &LogicalOperator) -> u128 {
        match op {
            LogicalOperator::NodeScan(scan) => {
                if let Some(inner) = &scan.input {
                    return self.borne_inferieure(inner);
                }
                let n = match &scan.label {
                    Some(label) => self.store.node_count_by_label(label),
                    None => self.store.node_count(),
                };
                (n as u128).max(1)
            }
            // Un parcours depuis un scan NON borné traverse toutes les arêtes du
            // store : le compteur d'arêtes est donc une borne inférieure EXACTE
            // des paires produites. À deux sauts et plus, le second saut peut
            // être vide et la borne ne tient plus : on n'affirme rien.
            LogicalOperator::Expand(expand) => match expand.input.as_ref() {
                LogicalOperator::NodeScan(scan) if scan.label.is_none() && scan.input.is_none() => {
                    (self.store.edge_count() as u128).max(1)
                }
                _ => 1,
            },
            _ => 1,
        }
    }

    /// Plans a JOIN operator.
    ///
    /// When join conditions reference shared variables, deduplicates the output
    /// columns by projecting out the right-side copies (whose values are equal
    /// to the left-side copies due to the join condition).
    pub(super) fn plan_join(&self, join: &JoinOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (left_op, left_columns) = self.plan_operator(&join.left)?;
        let (right_op, right_columns) = self.plan_operator(&join.right)?;

        // Full column list before deduplication (HashJoin produces all columns)
        let mut all_columns = left_columns.clone();
        all_columns.extend(right_columns.clone());

        // Convert join type
        let physical_join_type = match join.join_type {
            JoinType::Inner => PhysicalJoinType::Inner,
            JoinType::Left => PhysicalJoinType::Left,
            JoinType::Right => PhysicalJoinType::Right,
            JoinType::Full => PhysicalJoinType::Full,
            JoinType::Cross => PhysicalJoinType::Cross,
            JoinType::Semi => PhysicalJoinType::Semi,
            JoinType::Anti => PhysicalJoinType::Anti,
        };

        // Build key columns from join conditions
        let (probe_keys, build_keys): (Vec<usize>, Vec<usize>) = if join.conditions.is_empty() {
            // Cross join - no keys
            (vec![], vec![])
        } else {
            join.conditions
                .iter()
                .filter_map(|cond| {
                    // Try to extract column indices from expressions
                    let left_idx = self.expression_to_column(&cond.left, &left_columns).ok()?;
                    let right_idx = self
                        .expression_to_column(&cond.right, &right_columns)
                        .ok()?;
                    Some((left_idx, right_idx))
                })
                .unzip()
        };

        let output_schema = self.derive_schema_from_columns(&all_columns);

        let operator: Box<dyn Operator> = Box::new(HashJoinOperator::new(
            left_op,
            right_op,
            probe_keys,
            build_keys,
            physical_join_type,
            output_schema,
        ));

        Ok((operator, all_columns))
    }

    /// Plans a multi-way leapfrog join (WCOJ) operator.
    ///
    /// Materializes each input into a sorted trie and uses `LeapfrogJoinOperator`
    /// for worst-case optimal intersection.
    pub(super) fn plan_multi_way_join(
        &self,
        mwj: &MultiWayJoinOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Plan each input, collecting operators and their column lists
        let mut input_ops: Vec<Box<dyn Operator>> = Vec::with_capacity(mwj.inputs.len());
        let mut input_columns: Vec<Vec<String>> = Vec::with_capacity(mwj.inputs.len());

        for input in &mwj.inputs {
            let (op, cols) = self.plan_operator(input)?;
            input_ops.push(op);
            input_columns.push(cols);
        }

        // For each input, find the column indices of the shared variables (join keys)
        let mut join_key_indices: Vec<Vec<usize>> = Vec::with_capacity(mwj.inputs.len());
        for cols in &input_columns {
            let mut key_indices = Vec::new();
            for shared_var in &mwj.shared_variables {
                if let Some(idx) = cols.iter().position(|c| c == shared_var) {
                    key_indices.push(idx);
                }
            }
            join_key_indices.push(key_indices);
        }

        // Build combined output columns: shared variables first (deduplicated),
        // then remaining columns from each input
        let mut output_columns: Vec<String> = mwj.shared_variables.clone();
        let mut output_column_mapping: Vec<(usize, usize)> = Vec::new();

        // Map shared variables from the first input that has them
        for shared_var in &mwj.shared_variables {
            let mut found = false;
            for (input_idx, cols) in input_columns.iter().enumerate() {
                if let Some(col_idx) = cols.iter().position(|c| c == shared_var) {
                    output_column_mapping.push((input_idx, col_idx));
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(Error::Internal(format!(
                    "Shared variable '{}' not found in any input",
                    shared_var
                )));
            }
        }

        // Add non-shared columns from each input
        for (input_idx, cols) in input_columns.iter().enumerate() {
            for (col_idx, col_name) in cols.iter().enumerate() {
                if !mwj.shared_variables.contains(col_name) {
                    output_columns.push(col_name.clone());
                    output_column_mapping.push((input_idx, col_idx));
                }
            }
        }

        let output_schema = self.derive_schema_from_columns(&output_columns);

        let operator: Box<dyn Operator> = Box::new(LeapfrogJoinOperator::new(
            input_ops,
            join_key_indices,
            output_schema,
            output_column_mapping,
        ));

        Ok((operator, output_columns))
    }

    /// Extracts a column index from an expression.
    pub(super) fn expression_to_column(
        &self,
        expr: &LogicalExpression,
        columns: &[String],
    ) -> Result<usize> {
        match expr {
            LogicalExpression::Variable(name) => columns
                .iter()
                .position(|c| c == name)
                .ok_or_else(|| Error::Internal(format!("Variable '{}' not found", name))),
            _ => Err(Error::Internal(
                "Only variables supported in join conditions".to_string(),
            )),
        }
    }

    /// Plans a UNION operator.
    ///
    /// Validates that all branches produce the same number of columns with
    /// compatible names. The output schema is derived from the first branch.
    pub(super) fn plan_union(&self, union: &UnionOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let mut inputs = Vec::with_capacity(union.inputs.len());
        let mut columns = Vec::new();

        for (i, input) in union.inputs.iter().enumerate() {
            let (op, cols) = self.plan_operator(input)?;
            if i == 0 {
                columns = cols;
            } else if cols.len() != columns.len() {
                return Err(Error::Query(obrain_common::utils::error::QueryError::new(
                    obrain_common::utils::error::QueryErrorKind::Semantic,
                    format!(
                        "UNION branches must return the same number of columns (left: {}, right: {})",
                        columns.len(),
                        cols.len()
                    ),
                )));
            }
            inputs.push(op);
        }

        let schema = self.derive_schema_from_columns(&columns);
        common::build_union(inputs, columns, schema)
    }

    /// Plans a DISTINCT operator.
    pub(super) fn plan_distinct(
        &self,
        distinct: &DistinctOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (input_op, columns) = self.plan_operator(&distinct.input)?;
        let schema = self.derive_schema_from_columns(&columns);
        Ok(common::build_distinct(
            input_op,
            columns,
            distinct.columns.as_deref(),
            schema,
        ))
    }

    /// Plans an EXCEPT operator.
    pub(super) fn plan_except(
        &self,
        except: &ExceptOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (left_op, columns) = self.plan_operator(&except.left)?;
        let (right_op, _) = self.plan_operator(&except.right)?;
        let schema = self.derive_schema_from_columns(&columns);
        Ok(common::build_except(
            left_op, right_op, columns, except.all, schema,
        ))
    }

    /// Plans an INTERSECT operator.
    pub(super) fn plan_intersect(
        &self,
        intersect: &IntersectOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (left_op, columns) = self.plan_operator(&intersect.left)?;
        let (right_op, _) = self.plan_operator(&intersect.right)?;
        let schema = self.derive_schema_from_columns(&columns);
        Ok(common::build_intersect(
            left_op,
            right_op,
            columns,
            intersect.all,
            schema,
        ))
    }

    /// Plans an OTHERWISE operator.
    pub(super) fn plan_otherwise(
        &self,
        otherwise: &OtherwiseOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (left_op, columns) = self.plan_operator(&otherwise.left)?;
        let (right_op, _) = self.plan_operator(&otherwise.right)?;
        Ok(common::build_otherwise(left_op, right_op, columns))
    }

    /// Plans an APPLY (lateral join) operator.
    ///
    /// When `shared_variables` is non-empty, creates a correlated Apply that
    /// injects outer row values into the inner plan via [`ParameterState`].
    pub(super) fn plan_apply(&self, apply: &ApplyOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (outer_op, outer_columns) = self.plan_operator(&apply.input)?;

        if apply.shared_variables.is_empty() {
            // Uncorrelated Apply
            let (inner_op, inner_columns) = self.plan_operator(&apply.subplan)?;
            // Inner subquery RETURN materializes values (PropertyAccess, NodeResolve,
            // aggregates, etc.), so all its output columns are scalar.
            for col in &inner_columns {
                self.scalar_columns.borrow_mut().insert(col.clone());
            }
            let inner_col_count = inner_columns.len();
            let mut columns = outer_columns;
            columns.extend(inner_columns);
            let mut op = ApplyOperator::new(outer_op, inner_op);
            if apply.optional {
                op = op.with_optional(inner_col_count);
            }
            return Ok((Box::new(op), columns));
        }

        // Expand wildcard: WITH * imports all outer-scope variables
        let shared_vars = if apply.shared_variables.len() == 1 && apply.shared_variables[0] == "*" {
            outer_columns.clone()
        } else {
            apply.shared_variables.clone()
        };

        // Correlated Apply: create shared ParameterState
        let param_state = std::sync::Arc::new(
            obrain_core::execution::operators::ParameterState::new(shared_vars.clone()),
        );

        // Find column indices for the shared variables in outer columns
        let param_col_indices: Vec<usize> = shared_vars
            .iter()
            .map(|var| outer_columns.iter().position(|c| c == var).unwrap_or(0))
            .collect();

        // Set the parameter state so the inner plan's ParameterScan can find it
        *self.correlated_param_state.borrow_mut() = Some(std::sync::Arc::clone(&param_state));

        let (inner_op, inner_columns) = self.plan_operator(&apply.subplan)?;

        // Clear the parameter state after planning the inner operator
        *self.correlated_param_state.borrow_mut() = None;

        // Inner subquery RETURN materializes values, so register as scalar
        // to prevent the outer RETURN from misinterpreting them as node IDs.
        for col in &inner_columns {
            self.scalar_columns.borrow_mut().insert(col.clone());
        }

        // Build correlated Apply
        let mut columns = outer_columns;
        let inner_col_count = inner_columns.len();
        columns.extend(inner_columns);
        let mut op =
            ApplyOperator::new_correlated(outer_op, inner_op, param_state, param_col_indices);
        if apply.optional {
            op = op.with_optional(inner_col_count);
        }
        Ok((Box::new(op), columns))
    }
}
