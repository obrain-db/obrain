//! Apply operator (lateral join / correlated subquery).
//!
//! For each row from the outer input, the inner subplan is reset and executed.
//! Results are the concatenation of outer columns with inner columns.
//!
//! This operator is the backend for:
//! - Cypher: `CALL { subquery }`
//! - GQL: `VALUE { subquery }`
//! - Pattern comprehensions (with a Collect aggregate wrapper)

use std::sync::Arc;

use obrain_common::types::{LogicalType, Value};

use super::parameter_scan::ParameterState;
use super::{DataChunk, Operator, OperatorResult};
use crate::execution::vector::ValueVector;

/// Apply (lateral join) operator.
///
/// Evaluates `inner` once for each row of `outer`. The result schema is
/// `outer_columns ++ inner_columns`. If the inner plan produces zero rows
/// for a given outer row, that outer row is omitted (inner join semantics).
///
/// When `param_state` is set, outer row values for the specified column indices
/// are injected into the shared [`ParameterState`] before each inner execution,
/// allowing the inner plan's [`ParameterScanOperator`](super::ParameterScanOperator) to read them.
pub struct ApplyOperator {
    outer: Box<dyn Operator>,
    inner: Box<dyn Operator>,
    /// Shared parameter state for correlated subqueries.
    param_state: Option<Arc<ParameterState>>,
    /// Indices of outer columns to inject into the inner plan.
    param_col_indices: Vec<usize>,
    /// When true, outer rows with no inner results emit NULLs (left-join).
    optional: bool,
    /// Number of columns the inner plan produces (needed for NULL-padding).
    inner_column_count: usize,
    /// EXISTS mode: Some(true) = semi-join (keep if inner has rows),
    /// Some(false) = anti-join (keep if inner has NO rows).
    /// Inner columns are NOT appended in EXISTS mode.
    exists_mode: Option<bool>,
    /// (F1) Garder le type des colonnes d'identifiants (nœud, arête) de la
    /// sortie, relevé sur les blocs reçus. Sans lui, un identifiant d'arête
    /// sort comme un `Int64` générique, qu'une lecture de propriété prend
    /// pour un nœud (même défaut que obrain#184 pour la jointure par hachage).
    typed_passthrough: bool,
    /// Types relevés sur le premier bloc externe, puis interne.
    outer_types: Option<Vec<LogicalType>>,
    inner_types: Option<Vec<LogicalType>>,
    /// Buffered outer rows waiting to be combined with inner results.
    state: ApplyState,
}

enum ApplyState {
    /// Pull next outer chunk, process row-by-row.
    Init,
    /// Processing a chunk of outer rows. `outer_chunk` is the current batch,
    /// `outer_row` is the next row index to process.
    Processing {
        outer_chunk: DataChunk,
        outer_row: usize,
        /// Accumulated output rows (combined outer + inner).
        output: Vec<Vec<Value>>,
    },
    /// All outer input exhausted.
    Done,
}

impl ApplyOperator {
    /// Creates a new Apply operator (uncorrelated: no parameter injection).
    pub fn new(outer: Box<dyn Operator>, inner: Box<dyn Operator>) -> Self {
        Self {
            outer,
            inner,
            param_state: None,
            param_col_indices: Vec::new(),
            optional: false,
            inner_column_count: 0,
            exists_mode: None,
            typed_passthrough: false,
            outer_types: None,
            inner_types: None,
            state: ApplyState::Init,
        }
    }

    /// Creates a correlated Apply operator that injects outer row values.
    ///
    /// `param_state` is shared with a [`ParameterScanOperator`](super::ParameterScanOperator) in the inner plan.
    /// `param_col_indices` specifies which outer columns to inject (by index).
    pub fn new_correlated(
        outer: Box<dyn Operator>,
        inner: Box<dyn Operator>,
        param_state: Arc<ParameterState>,
        param_col_indices: Vec<usize>,
    ) -> Self {
        Self {
            outer,
            inner,
            param_state: Some(param_state),
            param_col_indices,
            optional: false,
            inner_column_count: 0,
            exists_mode: None,
            typed_passthrough: false,
            outer_types: None,
            inner_types: None,
            state: ApplyState::Init,
        }
    }

    /// Enables optional (left-join) semantics with the given inner column count.
    ///
    /// When enabled, outer rows that produce no inner results will be emitted
    /// with NULL values for the inner columns instead of being dropped.
    pub fn with_optional(mut self, inner_column_count: usize) -> Self {
        self.optional = true;
        self.inner_column_count = inner_column_count;
        self
    }

    /// Enables EXISTS mode: semi-join (`keep_matches=true`) or anti-join
    /// (`keep_matches=false`). Inner columns are NOT appended to the output.
    pub fn with_exists_mode(mut self, keep_matches: bool) -> Self {
        self.exists_mode = Some(keep_matches);
        self
    }

    /// (F1) Sortie typée : une colonne qui arrive en identifiants de nœud
    /// ou d'arête sort en identifiants du même genre (voir
    /// `typed_passthrough`).
    pub fn with_typed_passthrough(mut self) -> Self {
        self.typed_passthrough = true;
        self
    }

    /// Type de sortie d'une colonne reçue : celui d'un vecteur
    /// d'identifiants, `Any` pour tout le reste.
    fn column_types(chunk: &DataChunk) -> Vec<LogicalType> {
        (0..chunk.num_columns())
            .map(|i| match chunk.column(i).map(ValueVector::data_type) {
                Some(LogicalType::Node) => LogicalType::Node,
                Some(LogicalType::Edge) => LogicalType::Edge,
                _ => LogicalType::Any,
            })
            .collect()
    }

    /// Types des colonnes de sortie, `None` hors sortie typée.
    fn output_types(
        typed_passthrough: bool,
        exists_mode: Option<bool>,
        outer_types: Option<&Vec<LogicalType>>,
        inner_types: Option<&Vec<LogicalType>>,
        inner_column_count: usize,
    ) -> Option<Vec<LogicalType>> {
        if !typed_passthrough {
            return None;
        }
        let mut types = outer_types.cloned().unwrap_or_default();
        if exists_mode.is_none() {
            match inner_types {
                Some(inner) => types.extend(inner.iter().cloned()),
                None => types.extend(std::iter::repeat_n(LogicalType::Any, inner_column_count)),
            }
        }
        Some(types)
    }

    /// Extracts all values from a single row of a DataChunk.
    fn extract_row(chunk: &DataChunk, row: usize) -> Vec<Value> {
        let mut values = Vec::with_capacity(chunk.num_columns());
        for col_idx in 0..chunk.num_columns() {
            let val = chunk
                .column(col_idx)
                .and_then(|col| col.get_value(row))
                .unwrap_or(Value::Null);
            values.push(val);
        }
        values
    }

    /// Builds a DataChunk from accumulated rows.
    fn build_chunk(rows: &[Vec<Value>], types: Option<&[LogicalType]>) -> DataChunk {
        if rows.is_empty() {
            return DataChunk::empty();
        }
        let num_cols = rows[0].len();
        let mut columns: Vec<ValueVector> = (0..num_cols)
            .map(|i| {
                let ty = types
                    .and_then(|t| t.get(i))
                    .cloned()
                    .unwrap_or(LogicalType::Any);
                ValueVector::with_capacity(ty, rows.len())
            })
            .collect();

        for row in rows {
            for (col_idx, val) in row.iter().enumerate() {
                if col_idx < columns.len() {
                    columns[col_idx].push_value(val.clone());
                }
            }
        }
        DataChunk::new(columns)
    }
}

impl Operator for ApplyOperator {
    fn next(&mut self) -> OperatorResult {
        loop {
            super::verifier_annulation()?;
            match &mut self.state {
                ApplyState::Init => match self.outer.next()? {
                    Some(chunk) => {
                        if self.typed_passthrough && self.outer_types.is_none() {
                            self.outer_types = Some(Self::column_types(&chunk));
                        }
                        self.state = ApplyState::Processing {
                            outer_chunk: chunk,
                            outer_row: 0,
                            output: Vec::new(),
                        };
                    }
                    None => {
                        self.state = ApplyState::Done;
                        return Ok(None);
                    }
                },
                ApplyState::Processing {
                    outer_chunk,
                    outer_row,
                    output,
                } => {
                    let selected: Vec<usize> = outer_chunk.selected_indices().collect();
                    while *outer_row < selected.len() {
                        let row = selected[*outer_row];
                        let outer_values = Self::extract_row(outer_chunk, row);

                        // Inject outer values into the inner plan's parameter state
                        if let Some(ref param_state) = self.param_state {
                            let injected: Vec<Value> = self
                                .param_col_indices
                                .iter()
                                .map(|&idx| outer_values.get(idx).cloned().unwrap_or(Value::Null))
                                .collect();
                            param_state.set_values(injected);
                        }

                        // Reset and run inner plan for this outer row
                        self.inner.reset();

                        // EXISTS mode: check for row existence without appending inner columns
                        if let Some(keep_matches) = self.exists_mode {
                            // Un bloc VIDE n'est pas une correspondance : lire
                            // jusqu'à la première ligne ou la fin.
                            let mut has_results = false;
                            while let Some(inner_chunk) = self.inner.next()? {
                                if inner_chunk.selected_indices().next().is_some() {
                                    has_results = true;
                                    break;
                                }
                            }
                            if has_results == keep_matches {
                                output.push(outer_values);
                            }
                        } else {
                            let pre_len = output.len();
                            while let Some(inner_chunk) = self.inner.next()? {
                                if self.typed_passthrough && self.inner_types.is_none() {
                                    self.inner_types = Some(Self::column_types(&inner_chunk));
                                }
                                for inner_row in inner_chunk.selected_indices() {
                                    let inner_values = Self::extract_row(&inner_chunk, inner_row);
                                    let mut combined = outer_values.clone();
                                    combined.extend(inner_values);
                                    output.push(combined);
                                }
                            }

                            // OPTIONAL: emit outer row with NULLs when inner produced nothing
                            if self.optional && output.len() == pre_len {
                                let mut combined = outer_values;
                                combined.extend(std::iter::repeat_n(
                                    Value::Null,
                                    self.inner_column_count,
                                ));
                                output.push(combined);
                            }
                        }

                        *outer_row += 1;

                        // Flush when we have enough rows
                        if output.len() >= 1024 {
                            let chunk = Self::build_chunk(
                                output,
                                Self::output_types(
                                    self.typed_passthrough,
                                    self.exists_mode,
                                    self.outer_types.as_ref(),
                                    self.inner_types.as_ref(),
                                    self.inner_column_count,
                                )
                                .as_deref(),
                            );
                            output.clear();
                            return Ok(Some(chunk));
                        }
                    }

                    // Finished this outer chunk; flush any remaining output
                    if !output.is_empty() {
                        let chunk = Self::build_chunk(
                            output,
                            Self::output_types(
                                self.typed_passthrough,
                                self.exists_mode,
                                self.outer_types.as_ref(),
                                self.inner_types.as_ref(),
                                self.inner_column_count,
                            )
                            .as_deref(),
                        );
                        output.clear();
                        self.state = ApplyState::Init;
                        return Ok(Some(chunk));
                    }

                    // Move to next outer chunk
                    self.state = ApplyState::Init;
                }
                ApplyState::Done => return Ok(None),
            }
        }
    }

    fn reset(&mut self) {
        self.outer.reset();
        self.inner.reset();
        self.state = ApplyState::Init;
    }

    fn name(&self) -> &'static str {
        "Apply"
    }
}
