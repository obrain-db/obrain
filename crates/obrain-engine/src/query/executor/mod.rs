//! Query executor.
//!
//! Executes physical plans and produces results.

#[cfg(feature = "algos")]
pub mod procedure_call;
#[cfg(feature = "algos")]
pub mod user_procedure;

use web_time::Instant;

use crate::config::AdaptiveConfig;
use crate::database::QueryResult;
use obrain_common::types::{LogicalType, Value};
use obrain_common::utils::error::{Error, QueryError, Result};
use obrain_core::execution::operators::{Operator, OperatorError};
use obrain_core::execution::{
    AdaptiveContext, AdaptiveSummary, CardinalityTrackingWrapper, DataChunk, SharedAdaptiveContext,
};

/// Executes a physical operator tree and collects results.
pub struct Executor {
    /// Column names for the result.
    columns: Vec<String>,
    /// Column types for the result.
    column_types: Vec<LogicalType>,
    /// Wall-clock deadline after which execution is aborted.
    deadline: Option<Instant>,
}

impl Executor {
    /// Creates a new executor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            column_types: Vec::new(),
            deadline: None,
        }
    }

    /// Creates an executor with specified column names.
    #[must_use]
    pub fn with_columns(columns: Vec<String>) -> Self {
        let len = columns.len();
        Self {
            columns,
            column_types: vec![LogicalType::Any; len],
            deadline: None,
        }
    }

    /// Creates an executor with specified column names and types.
    #[must_use]
    pub fn with_columns_and_types(columns: Vec<String>, column_types: Vec<LogicalType>) -> Self {
        Self {
            columns,
            column_types,
            deadline: None,
        }
    }

    /// Sets a wall-clock deadline for query execution.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Option<Instant>) -> Self {
        self.deadline = deadline;
        self
    }

    /// Vérifie l'ANNULATION coopérative puis le délai.
    ///
    /// Les deux sont distincts : le délai est une constatation du moteur,
    /// l'annulation une décision de l'appelant. `Error::is_cancelled` les
    /// sépare pour que l'un ne soit pas compté comme l'autre.
    ///
    /// ── CE CONTRÔLE NE SUFFIT PAS, ET C'EST LE POINT ──────────────────
    ///
    /// Il ne s'exécute qu'ENTRE deux `operator.next()`. Les opérateurs
    /// bloquants (Sort, HashAggregate, build de HashJoin, ScanOperator sur
    /// un label entier) consomment tout leur enfant DANS un seul `next()` :
    /// pour eux, ce contrôle-ci ne passe qu'une fois le travail déjà fait.
    /// C'est précisément le trou que le délai existant laissait ouvert. Les
    /// vrais points de contrôle sont dans les boucles des opérateurs
    /// (`obrain_core::execution::operators::verifier_annulation`) et dans
    /// les balayages du substrat. Celui-ci est le filet du dernier ressort.
    #[inline]
    fn check_cancel(&self) -> Result<()> {
        use obrain_common::utils::cancel::{Raison, raison};
        if obrain_common::utils::cancel::check().is_err() {
            return Err(match raison() {
                Some(Raison::Echeance) => Error::Query(QueryError::timeout()),
                _ => Error::Query(QueryError::cancelled()),
            });
        }
        Ok(())
    }

    /// Checks whether the deadline has been exceeded.
    fn check_deadline(&self) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(Error::Query(QueryError::timeout()));
        }
        Ok(())
    }

    /// Executes a physical operator and collects all results.
    ///
    /// # Errors
    ///
    /// Returns an error if operator execution fails or the query timeout is exceeded.
    pub fn execute(&self, operator: &mut dyn Operator) -> Result<QueryResult> {
        let _span = tracing::debug_span!("obrain::query::execute").entered();
        let mut result = QueryResult::with_types(self.columns.clone(), self.column_types.clone());
        let mut types_captured = !result.column_types.iter().all(|t| *t == LogicalType::Any);

        loop {
            self.check_cancel()?;
            self.check_deadline()?;

            match operator.next() {
                Ok(Some(chunk)) => {
                    // Capture column types from first non-empty chunk
                    if !types_captured && chunk.column_count() > 0 {
                        self.capture_column_types(&chunk, &mut result);
                        types_captured = true;
                    }
                    self.collect_chunk(&chunk, &mut result)?;
                }
                Ok(None) => break,
                Err(err) => return Err(convert_operator_error(err)),
            }
        }

        // ── CONTROLE DE SORTIE : LA FERMETURE DU MECANISME ───────────
        //
        // Le controle en tete de boucle ne suffit pas. La boucle sort sur
        // `Ok(None)` — sans repasser par lui — et une douzaine d operateurs
        // appellent `store.nodes_by_label()` sans post-controle propre
        // (`scan_vector`, `typed_degree_topk`, `vector_join`, le balayage de
        // label du planificateur de filtres...). Or un balayage du substrat
        // annule rend un `Vec` PARTIEL : sa signature rend un `Vec` nu, il n a
        // aucun canal pour dire « je n ai pas fini ». Sans ce controle-ci, un
        // resultat TRONQUE sortirait en `Ok` — moins de lignes, aucune erreur,
        // et un appelant qui n a aucun moyen de s en apercevoir. C est
        // strictement pire que ne pas annuler du tout.
        //
        // Il est exhaustif parce que le jeton est MONOTONE : si une troncature
        // a eu lieu, le jeton etait arme pendant, il l est donc encore ici.
        self.check_cancel()?;

        Self::affiner_types_any(&mut result);
        Ok(result)
    }

    /// Executes and returns at most `limit` rows.
    ///
    /// # Errors
    ///
    /// Returns an error if operator execution fails or the query timeout is exceeded.
    pub fn execute_with_limit(
        &self,
        operator: &mut dyn Operator,
        limit: usize,
    ) -> Result<QueryResult> {
        let mut result = QueryResult::with_types(self.columns.clone(), self.column_types.clone());
        let mut collected = 0;
        let mut types_captured = !result.column_types.iter().all(|t| *t == LogicalType::Any);

        loop {
            if collected >= limit {
                break;
            }

            self.check_cancel()?;
            self.check_deadline()?;

            match operator.next() {
                Ok(Some(chunk)) => {
                    // Capture column types from first non-empty chunk
                    if !types_captured && chunk.column_count() > 0 {
                        self.capture_column_types(&chunk, &mut result);
                        types_captured = true;
                    }
                    let remaining = limit - collected;
                    collected += self.collect_chunk_limited(&chunk, &mut result, remaining)?;
                }
                Ok(None) => break,
                Err(err) => return Err(convert_operator_error(err)),
            }
        }

        // Meme raisonnement que dans `execute` : la sortie de boucle ne
        // repasse pas par le controle, et une sortie par `collected >= limit`
        // non plus.
        self.check_cancel()?;

        Self::affiner_types_any(&mut result);
        Ok(result)
    }

    /// AFFINE les colonnes déclarées `Any` d'après le résultat COMPLET
    /// (correctif E) : si toutes les valeurs non NULL d'une colonne portent
    /// le même type scalaire, ce type est déclaré ; sinon `Any` reste —
    /// l'aveu honnête. Nécessaire depuis que MIN/MAX déclarent `Any` comme
    /// Sum (le câblage `Int64` écrasait la valeur en 0) : le contrat
    /// `column_types` doit dire STRING pour `min()` d'une colonne de chaînes.
    /// Balayage O(lignes) par discriminant, payé une fois le résultat déjà
    /// matérialisé.
    fn affiner_types_any(result: &mut QueryResult) {
        /// Type scalaire d'une valeur, `None` pour NULL (neutre) et pour les
        /// conteneurs/types complexes — pour lesquels `Any` reste l'aveu
        /// honnête.
        #[allow(clippy::option_option)] // `Some(None)` = NULL (neutre), `None` = complexe : deux sens distincts
        fn type_scalaire(v: &Value) -> Option<Option<LogicalType>> {
            match v {
                Value::Null => Some(None),
                Value::Bool(_) => Some(Some(LogicalType::Bool)),
                Value::Int64(_) => Some(Some(LogicalType::Int64)),
                Value::Float64(_) => Some(Some(LogicalType::Float64)),
                Value::String(_) => Some(Some(LogicalType::String)),
                Value::Bytes(_) => Some(Some(LogicalType::Bytes)),
                Value::Date(_) => Some(Some(LogicalType::Date)),
                Value::Time(_) => Some(Some(LogicalType::Time)),
                Value::Timestamp(_) => Some(Some(LogicalType::Timestamp)),
                Value::Duration(_) => Some(Some(LogicalType::Duration)),
                Value::ZonedDatetime(_) => Some(Some(LogicalType::ZonedDatetime)),
                _ => None, // conteneur : colonne non affinable
            }
        }

        for (col, t) in result.column_types.iter_mut().enumerate() {
            if *t != LogicalType::Any {
                continue;
            }
            let mut vu: Option<LogicalType> = None;
            let mut affinable = true;
            for ligne in &result.rows {
                let Some(v) = ligne.get(col) else { continue };
                match type_scalaire(v) {
                    Some(None) => {} // NULL : neutre
                    Some(Some(lt)) => match &vu {
                        None => vu = Some(lt),
                        Some(prev) if *prev == lt => {}
                        Some(_) => {
                            affinable = false;
                            break;
                        }
                    },
                    None => {
                        affinable = false;
                        break;
                    }
                }
            }
            if affinable && let Some(lt) = vu {
                *t = lt;
            }
        }
    }

    /// Captures column types from a DataChunk.
    fn capture_column_types(&self, chunk: &DataChunk, result: &mut QueryResult) {
        let col_count = chunk.column_count();
        result.column_types = Vec::with_capacity(col_count);
        for col_idx in 0..col_count {
            let col_type = chunk
                .column(col_idx)
                .map_or(LogicalType::Any, |col| col.data_type().clone());
            result.column_types.push(col_type);
        }
    }

    /// Collects all rows from a DataChunk into the result.
    ///
    /// Uses `selected_indices()` to correctly handle chunks with selection vectors
    /// (e.g., after filtering operations).
    fn collect_chunk(&self, chunk: &DataChunk, result: &mut QueryResult) -> Result<usize> {
        let col_count = chunk.column_count();
        let mut collected = 0;

        for row_idx in chunk.selected_indices() {
            let mut row = Vec::with_capacity(col_count);
            for col_idx in 0..col_count {
                let value = chunk
                    .column(col_idx)
                    .and_then(|col| col.get_value(row_idx))
                    .unwrap_or(Value::Null);
                row.push(value);
            }
            result.rows.push(row);
            collected += 1;
        }

        Ok(collected)
    }

    /// Collects up to `limit` rows from a DataChunk.
    ///
    /// Uses `selected_indices()` to correctly handle chunks with selection vectors
    /// (e.g., after filtering operations).
    fn collect_chunk_limited(
        &self,
        chunk: &DataChunk,
        result: &mut QueryResult,
        limit: usize,
    ) -> Result<usize> {
        let col_count = chunk.column_count();
        let mut collected = 0;

        for row_idx in chunk.selected_indices() {
            if collected >= limit {
                break;
            }
            let mut row = Vec::with_capacity(col_count);
            for col_idx in 0..col_count {
                let value = chunk
                    .column(col_idx)
                    .and_then(|col| col.get_value(row_idx))
                    .unwrap_or(Value::Null);
                row.push(value);
            }
            result.rows.push(row);
            collected += 1;
        }

        Ok(collected)
    }

    /// Executes a physical operator with adaptive cardinality tracking.
    ///
    /// This wraps the operator in a cardinality tracking layer and monitors
    /// deviation from estimates during execution. The adaptive summary is
    /// returned alongside the query result.
    ///
    /// # Arguments
    ///
    /// * `operator` - The root physical operator to execute
    /// * `adaptive_context` - Context with cardinality estimates from planning
    /// * `config` - Adaptive execution configuration
    ///
    /// # Errors
    ///
    /// Returns an error if operator execution fails.
    pub fn execute_adaptive(
        &self,
        operator: Box<dyn Operator>,
        adaptive_context: Option<AdaptiveContext>,
        config: &AdaptiveConfig,
    ) -> Result<(QueryResult, Option<AdaptiveSummary>)> {
        // If adaptive is disabled or no context, fall back to normal execution
        if !config.enabled {
            let mut op = operator;
            let result = self.execute(op.as_mut())?;
            return Ok((result, None));
        }

        let Some(ctx) = adaptive_context else {
            let mut op = operator;
            let result = self.execute(op.as_mut())?;
            return Ok((result, None));
        };

        // Create shared context for tracking
        let shared_ctx = SharedAdaptiveContext::from_context(AdaptiveContext::with_thresholds(
            config.threshold,
            config.min_rows,
        ));

        // Copy estimates from the planning context to the shared tracking context
        for (op_id, checkpoint) in ctx.all_checkpoints() {
            if let Some(mut inner) = shared_ctx.snapshot() {
                inner.set_estimate(op_id, checkpoint.estimated);
            }
        }

        // Wrap operator with tracking
        let mut wrapped = CardinalityTrackingWrapper::new(operator, "root", shared_ctx.clone());

        // Execute with tracking
        let mut result = QueryResult::with_types(self.columns.clone(), self.column_types.clone());
        let mut types_captured = !result.column_types.iter().all(|t| *t == LogicalType::Any);
        let mut total_rows: u64 = 0;
        let check_interval = config.min_rows;

        loop {
            self.check_cancel()?;
            self.check_deadline()?;

            match wrapped.next() {
                Ok(Some(chunk)) => {
                    let chunk_rows = chunk.row_count();
                    total_rows += chunk_rows as u64;

                    // Capture column types from first non-empty chunk
                    if !types_captured && chunk.column_count() > 0 {
                        self.capture_column_types(&chunk, &mut result);
                        types_captured = true;
                    }
                    self.collect_chunk(&chunk, &mut result)?;

                    // Periodically check for significant deviation
                    if total_rows >= check_interval
                        && total_rows.is_multiple_of(check_interval)
                        && shared_ctx.should_reoptimize()
                    {
                        // For now, just log/note that re-optimization would trigger
                        // Full re-optimization would require plan regeneration
                        // which is a more invasive change
                    }
                }
                Ok(None) => break,
                Err(err) => return Err(convert_operator_error(err)),
            }
        }

        // Meme raisonnement que dans `execute`.
        self.check_cancel()?;

        // Get final summary
        let summary = shared_ctx.snapshot().map(|ctx| ctx.summary());

        Ok((result, summary))
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts an operator error to a common error.
fn convert_operator_error(err: OperatorError) -> Error {
    match err {
        OperatorError::TypeMismatch { expected, found } => Error::TypeMismatch { expected, found },
        OperatorError::ColumnNotFound(name) => {
            Error::InvalidValue(format!("Column not found: {name}"))
        }
        OperatorError::Execution(msg) => Error::Internal(msg),
        OperatorError::ConstraintViolation(msg) => {
            Error::InvalidValue(format!("Constraint violation: {msg}"))
        }
        OperatorError::WriteConflict(msg) => {
            Error::Transaction(obrain_common::utils::error::TransactionError::WriteConflict(msg))
        }
        // Une annulation traverse la frontiere sans changer de nature : elle
        // NE DOIT PAS devenir `Error::Internal`, qui est la categorie
        // « bug du moteur ». Voir `Error::is_cancelled`.
        // La boucle chaude qui a leve ce signal ignore POURQUOI elle s est
        // arretee — c est voulu : `Cancelled` traverse trois crates sans
        // alourdir aucune signature. La raison se relit ici, une fois, a la
        // frontiere du moteur, la ou elle sert a choisir l erreur rendue.
        OperatorError::Cancelled => {
            use obrain_common::utils::cancel::{Raison, raison};
            match raison() {
                Some(Raison::Echeance) => {
                    Error::Query(obrain_common::utils::error::QueryError::timeout())
                }
                _ => Error::Query(obrain_common::utils::error::QueryError::cancelled()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::LogicalType;
    use obrain_core::execution::DataChunk;

    /// A mock operator that generates chunks with integer data on demand.
    struct MockIntOperator {
        values: Vec<i64>,
        position: usize,
        chunk_size: usize,
    }

    impl MockIntOperator {
        fn new(values: Vec<i64>, chunk_size: usize) -> Self {
            Self {
                values,
                position: 0,
                chunk_size,
            }
        }
    }

    impl Operator for MockIntOperator {
        fn next(&mut self) -> obrain_core::execution::operators::OperatorResult {
            if self.position >= self.values.len() {
                return Ok(None);
            }

            let end = (self.position + self.chunk_size).min(self.values.len());
            let mut chunk = DataChunk::with_capacity(&[LogicalType::Int64], self.chunk_size);

            {
                let col = chunk.column_mut(0).unwrap();
                for i in self.position..end {
                    col.push_int64(self.values[i]);
                }
            }
            chunk.set_count(end - self.position);
            self.position = end;

            Ok(Some(chunk))
        }

        fn reset(&mut self) {
            self.position = 0;
        }

        fn name(&self) -> &'static str {
            "MockInt"
        }
    }

    /// Empty mock operator for testing empty results.
    struct EmptyOperator;

    impl Operator for EmptyOperator {
        fn next(&mut self) -> obrain_core::execution::operators::OperatorResult {
            Ok(None)
        }

        fn reset(&mut self) {}

        fn name(&self) -> &'static str {
            "Empty"
        }
    }

    #[test]
    fn test_executor_empty() {
        let executor = Executor::with_columns(vec!["a".to_string()]);
        let mut op = EmptyOperator;

        let result = executor.execute(&mut op).unwrap();
        assert_eq!(result.len(), 0);
        assert_eq!(result.column_count(), 1);
    }

    #[test]
    fn test_executor_single_chunk() {
        let executor = Executor::with_columns(vec!["value".to_string()]);
        let mut op = MockIntOperator::new(vec![1, 2, 3], 10);

        let result = executor.execute(&mut op).unwrap();
        assert_eq!(result.row_count(), 3);
        assert_eq!(result.rows[0][0], Value::Int64(1));
        assert_eq!(result.rows[1][0], Value::Int64(2));
        assert_eq!(result.rows[2][0], Value::Int64(3));
    }

    #[test]
    fn test_executor_with_limit() {
        let executor = Executor::with_columns(vec!["value".to_string()]);
        let mut op = MockIntOperator::new((0..10).collect(), 100);

        let result = executor.execute_with_limit(&mut op, 5).unwrap();
        assert_eq!(result.row_count(), 5);
    }

    #[test]
    fn test_executor_timeout_expired() {
        use std::time::Duration;
        use web_time::Instant;

        // Set a deadline that has already passed
        let executor = Executor::with_columns(vec!["value".to_string()]).with_deadline(Some(
            Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
        ));
        let mut op = MockIntOperator::new(vec![1, 2, 3], 10);

        let result = executor.execute(&mut op);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Query exceeded timeout"),
            "Expected timeout error, got: {err}"
        );
    }

    /// Operateur qui rend un chunk, puis rend `Ok(None)` EN ARMANT le jeton
    /// au meme instant — le motif exact d un balayage du substrat tronque :
    /// moins de lignes, aucune erreur.
    struct SourceQuiTronque {
        rendu: bool,
        jeton: obrain_common::utils::cancel::CancelToken,
    }

    impl Operator for SourceQuiTronque {
        fn next(&mut self) -> obrain_core::execution::operators::OperatorResult {
            if self.rendu {
                // La troncature se decide ICI, sur le dernier appel : la
                // boucle sort par `Ok(None)` sans repasser par le controle
                // en tete.
                self.jeton.cancel();
                return Ok(None);
            }
            self.rendu = true;
            let mut chunk = DataChunk::with_capacity(&[LogicalType::Int64], 1);
            chunk.column_mut(0).unwrap().push_int64(1);
            chunk.set_count(1);
            Ok(Some(chunk))
        }

        fn reset(&mut self) {
            self.rendu = false;
        }

        fn name(&self) -> &'static str {
            "SourceQuiTronque"
        }
    }

    #[test]
    fn un_resultat_tronque_ne_sort_jamais_en_ok() {
        // LE TROU QUE CE TEST FERME. Une douzaine d operateurs appellent
        // `store.nodes_by_label()` sans post-controle propre. Un balayage
        // annule leur rend un `Vec` PARTIEL — sa signature rend un `Vec` nu,
        // il n a aucun canal pour dire « je n ai pas fini ». La boucle de
        // l executeur sort alors par `Ok(None)`, SANS repasser par le
        // controle en tete, et un resultat tronque sortirait en `Ok` : moins
        // de lignes, aucune erreur, aucun moyen pour l appelant de s en
        // apercevoir. C est strictement pire que ne pas annuler du tout.
        let jeton = obrain_common::utils::cancel::CancelToken::new();
        let mut op = SourceQuiTronque {
            rendu: false,
            jeton: jeton.clone(),
        };
        let executor = Executor::with_columns(vec!["value".to_string()]);
        let r =
            obrain_common::utils::cancel::scope_en_vigueur(&jeton, || executor.execute(&mut op));
        let err = r.expect_err("un resultat tronque doit remonter en Err");
        assert!(
            err.is_cancelled(),
            "et l erreur doit etre une ANNULATION identifiable : {err}"
        );
    }

    #[test]
    fn un_resultat_tronque_ne_sort_jamais_en_ok_avec_limite() {
        // Meme trou sur `execute_with_limit`, qui sort AUSSI par
        // `collected >= limit` sans repasser par le controle.
        let jeton = obrain_common::utils::cancel::CancelToken::new();
        let mut op = SourceQuiTronque {
            rendu: false,
            jeton: jeton.clone(),
        };
        let executor = Executor::with_columns(vec!["value".to_string()]);
        let r = obrain_common::utils::cancel::scope_en_vigueur(&jeton, || {
            executor.execute_with_limit(&mut op, 100)
        });
        assert!(
            r.is_err_and(|e| e.is_cancelled()),
            "un resultat tronque doit remonter en Err meme sous LIMIT"
        );
    }

    #[test]
    fn sans_annulation_la_sortie_reste_un_ok() {
        // CONTRE-EPREUVE du controle de sortie : sans jeton, il ne doit rien
        // changer au comportement. Un controle de sortie trop zele
        // transformerait des requetes normales en erreurs.
        let executor = Executor::with_columns(vec!["value".to_string()]);
        let mut op = MockIntOperator::new(vec![1, 2, 3], 10);
        assert_eq!(executor.execute(&mut op).unwrap().row_count(), 3);
    }

    #[test]
    fn test_executor_no_timeout() {
        // No deadline set - should execute normally
        let executor = Executor::with_columns(vec!["value".to_string()]).with_deadline(None);
        let mut op = MockIntOperator::new(vec![1, 2, 3], 10);

        let result = executor.execute(&mut op).unwrap();
        assert_eq!(result.row_count(), 3);
    }
}
