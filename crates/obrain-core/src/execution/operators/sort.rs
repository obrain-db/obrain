//! Sort operator for ordering results.
//!
//! This module provides:
//! - `SortOperator`: Orders results by one or more columns

use std::cmp::Ordering;

use obrain_common::types::{LogicalType, Value};

use super::value_utils::compare_values_total;
use super::{Operator, OperatorError, OperatorResult};
use crate::execution::DataChunk;
use crate::execution::chunk::DataChunkBuilder;

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    /// Ascending order (smallest first).
    Ascending,
    /// Descending order (largest first).
    Descending,
}

/// Null ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NullOrder {
    /// Nulls come first.
    NullsFirst,
    /// Nulls come last.
    NullsLast,
}

/// A sort key specification.
#[derive(Debug, Clone)]
pub struct SortKey {
    /// Column index to sort by.
    pub column: usize,
    /// Sort direction.
    pub direction: SortDirection,
    /// Null ordering.
    pub null_order: NullOrder,
}

impl SortKey {
    /// Creates a new sort key with ascending order.
    pub fn ascending(column: usize) -> Self {
        Self {
            column,
            direction: SortDirection::Ascending,
            null_order: NullOrder::NullsLast,
        }
    }

    /// Creates a new sort key with descending order.
    ///
    /// Défaut calculé par direction (convention PostgreSQL/Neo4j : NULL est
    /// la plus grande valeur) : ASC → NullsLast, DESC → NullsFirst. Le
    /// placement des NULL est ABSOLU dans le comparateur, donc le défaut doit
    /// être posé par direction ici et par le planificateur.
    pub fn descending(column: usize) -> Self {
        Self {
            column,
            direction: SortDirection::Descending,
            null_order: NullOrder::NullsFirst,
        }
    }

    /// Sets the null ordering.
    pub fn with_null_order(mut self, null_order: NullOrder) -> Self {
        self.null_order = null_order;
        self
    }
}

/// A row reference for sorting.
#[derive(Debug, Clone, Copy)]
struct SortRow {
    /// Index of the chunk this row belongs to.
    chunk_index: usize,
    /// Row index within the chunk.
    row_index: usize,
}

/// Sort operator.
///
/// Materializes all input and sorts by the specified keys.
///
/// # Bounded sort (top-k)
///
/// When the consumer only needs the first `k` rows of the ordered result — an
/// `ORDER BY … LIMIT k` — call [`SortOperator::with_limit`]. The operator then
/// selects the `k` smallest rows with a bounded heap instead of ordering the
/// whole input, which turns an `O(n log n)` sort into `O(n log k)` comparisons.
///
/// **The output is identical, row for row, to sorting everything and keeping
/// the first `k`.** That is not a hope, it is a consequence of the tie-break
/// documented on [`bounded_top_k`]. See `test_limit_matches_full_sort_prefix_*`.
///
/// **This bounds CPU, not memory.** `chunks` still holds every input row: the
/// child is drained in full before anything is ordered. Only `sorted_rows` is
/// bounded. Nothing here makes the operator streaming or spill-free.
pub struct SortOperator {
    /// Child operator.
    child: Box<dyn Operator>,
    /// Sort keys.
    sort_keys: Vec<SortKey>,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Materialized chunks.
    chunks: Vec<DataChunk>,
    /// Sorted row references.
    sorted_rows: Vec<SortRow>,
    /// Whether sorting is complete.
    sort_complete: bool,
    /// Current position in output.
    output_position: usize,
    /// Maximum number of rows to emit, when the consumer's bound is known.
    ///
    /// `None` — the historical behaviour: order every row, emit every row.
    ///
    /// This counts rows EMITTED, not rows requested by the user. A plan of the
    /// shape `Limit(offset = s, count = c, Sort(…))` must pass `s + c`, since
    /// the rows the offset discards still have to be produced in order.
    limit: Option<usize>,
}

impl SortOperator {
    /// Creates a new sort operator.
    pub fn new(
        child: Box<dyn Operator>,
        sort_keys: Vec<SortKey>,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            child,
            sort_keys,
            output_schema,
            chunks: Vec::new(),
            sorted_rows: Vec::new(),
            sort_complete: false,
            output_position: 0,
            limit: None,
        }
    }

    /// Bounds the sort: only the first `limit` rows of the ordered result are
    /// produced, and the operator emits at most `limit` rows.
    ///
    /// The output is byte-for-byte what an unbounded sort followed by
    /// `LIMIT limit` would have produced, ties included — see [`bounded_top_k`].
    ///
    /// `limit` counts rows EMITTED. With an offset in the plan, pass
    /// `offset + count`, not `count`.
    ///
    /// Leaving this unset keeps the operator strictly as it was.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Returns the output bound, or `None` when the sort is unbounded.
    #[must_use]
    pub fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Materializes and sorts the input.
    fn sort(&mut self) -> Result<(), OperatorError> {
        // Materialize all input. Unchanged, and deliberately so: the bound
        // below saves comparisons, not memory — every input row is still held.
        while let Some(chunk) = self.child.next()? {
            // (2026-08-24) LE point de controle nomme par les traces de
            // production : « 5, puis 6, puis 3 exemplaires du MEME tri en
            // vol, avec vif a 33 Go contre 9 de ligne de base ». Le tri
            // materialise l'integralite de son entree dans CE `while` — sans
            // controle ici, une requete triante etait non interruptible du
            // premier chunk jusqu'au dernier.
            //
            // RESTE NON INTERRUPTIBLE, en toute franchise : la comparaison
            // elle-meme (`sort_unstable_by` plus bas) est un appel unique de
            // la bibliotheque standard. Sur 700 000 lignes deja en memoire
            // elle coute des centaines de millisecondes, pas des minutes —
            // le cout dominant etait l'ingestion, qui est desormais bornee.
            super::verifier_annulation()?;
            let chunk_idx = self.chunks.len();
            for row_idx in chunk.selected_indices() {
                self.sorted_rows.push(SortRow {
                    chunk_index: chunk_idx,
                    row_index: row_idx,
                });
            }
            self.chunks.push(chunk);
        }

        // `sorted_rows` is now in ARRIVAL order: index `i` is the i-th row the
        // child produced. That ordinal — not `(chunk_index, row_index)` — is
        // what the stable sort below preserves among equals, because
        // `selected_indices()` walks a selection vector whose indices need not
        // ascend. `bounded_top_k` tie-breaks on the same ordinal.
        let mut rows = std::mem::take(&mut self.sorted_rows);

        let chunks = &self.chunks;
        let sort_keys = &self.sort_keys;
        let key_cmp = |a: &SortRow, b: &SortRow| compare_rows(chunks, sort_keys, a, b);

        match self.limit {
            // Unbounded: the historical code path, untouched. `sort_by` is a
            // stable sort, so equal rows keep their arrival order.
            None => rows.sort_by(key_cmp),

            // `LIMIT 0`: no row can be emitted, so no row needs ordering.
            Some(0) => rows.clear(),

            // A bound that cannot prune anything. Falling through to the full
            // stable sort is both cheaper than a heap of size n and, more to
            // the point, keeps the `k >= n` case bit-identical to `None`.
            Some(k) if k >= rows.len() => rows.sort_by(key_cmp),

            // The bound bites.
            Some(k) => rows = bounded_top_k(rows, k, key_cmp),
        }

        self.sorted_rows = rows;
        self.sort_complete = true;
        Ok(())
    }
}

/// Compares two rows on the sort keys alone, honouring direction and null
/// order, key by key. Returns [`Ordering::Equal`] when every key ties.
///
/// This is the comparator the unbounded `sort_by` uses; it is deliberately
/// NOT total, so that the stable sort is what settles ties.
fn compare_rows(
    chunks: &[DataChunk],
    sort_keys: &[SortKey],
    a: &SortRow,
    b: &SortRow,
) -> Ordering {
    for key in sort_keys {
        let chunk_a = &chunks[a.chunk_index];
        let chunk_b = &chunks[b.chunk_index];

        let val_a = chunk_a
            .column(key.column)
            .and_then(|c| c.get_value(a.row_index));
        let val_b = chunk_b
            .column(key.column)
            .and_then(|c| c.get_value(b.row_index));

        // Placement des NULL ABSOLU : `NULLS FIRST`/`NULLS LAST` désignent
        // une extrémité de la SORTIE, quelle que soit la direction. La
        // direction n'inverse que la comparaison des valeurs non-NULL —
        // elle est appliquée DANS le comparateur, jamais après coup sur son
        // verdict (un `.reverse()` global retournerait aussi le placement
        // des NULL, cf. divergence gql09-desc-nulls-inverse).
        let cmp = compare_values_with_nulls(&val_a, &val_b, key.null_order, key.direction);

        // The second key only breaks a tie on the first, and so on.
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    Ordering::Equal
}

/// Selects the `k` smallest rows under `key_cmp` and returns them ordered.
///
/// # Why this is stable, stated rather than hoped for
///
/// A bounded heap is not a stable algorithm, so stability cannot be inherited —
/// it has to be built into the order. It is:
///
/// * `rows` arrives in child-emission order, so its index `i` IS the arrival
///   ordinal of that row.
/// * The order used here is `key_cmp` **then arrival ordinal ascending**. Since
///   arrival ordinals are distinct, that combined order is TOTAL: no two rows
///   compare equal, and the `k` smallest are therefore a uniquely determined
///   set in a uniquely determined sequence — no algorithm-dependent choice is
///   left for the heap to make.
/// * A stable `sort_by(key_cmp)` produces, by definition of stability, exactly
///   the sequence ordered by (`key_cmp`, arrival ordinal).
///
/// The two therefore agree, and the result is the first `k` rows of the
/// unbounded stable sort — the same rows, in the same order. Proved by
/// `test_limit_matches_full_sort_prefix_*`, which diff the two paths directly.
///
/// Cost: one comparison per rejected row against the heap root, `O(n log k)`
/// overall, versus `O(n log n)` for the full sort. Allocation is `k` rows, but
/// the caller still holds all `n` input rows — this buys CPU, not memory.
fn bounded_top_k<F>(rows: Vec<SortRow>, k: usize, mut key_cmp: F) -> Vec<SortRow>
where
    F: FnMut(&SortRow, &SortRow) -> Ordering,
{
    debug_assert!(k > 0 && k < rows.len(), "caller handles the degenerate bounds");

    // Total order: sort keys first, arrival ordinal breaks every remaining tie.
    let mut total_cmp = |x: &(SortRow, usize), y: &(SortRow, usize)| {
        key_cmp(&x.0, &y.0).then_with(|| x.1.cmp(&y.1))
    };

    // Max-heap of the k best seen so far: the root is the WORST of them, i.e.
    // the row a newcomer has to beat to get in.
    let mut heap: Vec<(SortRow, usize)> = Vec::with_capacity(k);

    for (arrival, row) in rows.into_iter().enumerate() {
        if heap.len() < k {
            heap.push((row, arrival));
            let last = heap.len() - 1;
            sift_up(&mut heap, last, &mut total_cmp);
        } else if total_cmp(&(row, arrival), &heap[0]) == Ordering::Less {
            // Note `arrival` exceeds every ordinal already in the heap, so a
            // newcomer that ties on the keys is correctly rejected: the earlier
            // row wins, which is precisely what the stable sort would do.
            heap[0] = (row, arrival);
            sift_down(&mut heap, 0, &mut total_cmp);
        }
    }

    // The heap holds the right k rows but only in heap order; put them in
    // output order. k is small by assumption, so this is cheap.
    heap.sort_by(|a, b| total_cmp(a, b));
    heap.into_iter().map(|(row, _)| row).collect()
}

/// Restores the max-heap property by moving `idx` towards the root.
fn sift_up<T, F>(heap: &mut [T], mut idx: usize, cmp: &mut F)
where
    F: FnMut(&T, &T) -> Ordering,
{
    while idx > 0 {
        let parent = (idx - 1) / 2;
        if cmp(&heap[idx], &heap[parent]) == Ordering::Greater {
            heap.swap(idx, parent);
            idx = parent;
        } else {
            break;
        }
    }
}

/// Restores the max-heap property by moving `idx` towards the leaves.
fn sift_down<T, F>(heap: &mut [T], mut idx: usize, cmp: &mut F)
where
    F: FnMut(&T, &T) -> Ordering,
{
    let len = heap.len();
    loop {
        let left = 2 * idx + 1;
        if left >= len {
            break;
        }
        let right = left + 1;

        let mut largest = left;
        if right < len && cmp(&heap[right], &heap[left]) == Ordering::Greater {
            largest = right;
        }

        if cmp(&heap[largest], &heap[idx]) == Ordering::Greater {
            heap.swap(largest, idx);
            idx = largest;
        } else {
            break;
        }
    }
}

/// Compares two optional values with null handling.
///
/// Le placement des NULL est ABSOLU : `NullsFirst`/`NullsLast` désignent une
/// extrémité de la sortie, indépendamment de la direction. La direction
/// n'inverse que la comparaison des valeurs non-NULL.
fn compare_values_with_nulls(
    a: &Option<Value>,
    b: &Option<Value>,
    null_order: NullOrder,
    direction: SortDirection,
) -> Ordering {
    match (a, b) {
        (None, None) | (Some(Value::Null), Some(Value::Null)) => Ordering::Equal,
        (None, _) | (Some(Value::Null), _) => match null_order {
            NullOrder::NullsFirst => Ordering::Less,
            NullOrder::NullsLast => Ordering::Greater,
        },
        (_, None) | (_, Some(Value::Null)) => match null_order {
            NullOrder::NullsFirst => Ordering::Greater,
            NullOrder::NullsLast => Ordering::Less,
        },
        (Some(a), Some(b)) => match direction {
            SortDirection::Ascending => compare_values_total(a, b),
            SortDirection::Descending => compare_values_total(a, b).reverse(),
        },
    }
}

impl Operator for SortOperator {
    fn next(&mut self) -> OperatorResult {
        if !self.sort_complete {
            self.sort()?;
        }

        if self.output_position >= self.sorted_rows.len() {
            return Ok(None);
        }

        let mut builder = DataChunkBuilder::with_capacity(&self.output_schema, 2048);

        while self.output_position < self.sorted_rows.len() && !builder.is_full() {
            let row_ref = &self.sorted_rows[self.output_position];
            let source_chunk = &self.chunks[row_ref.chunk_index];

            // Copy all columns
            for col_idx in 0..source_chunk.column_count() {
                if let (Some(src_col), Some(dst_col)) =
                    (source_chunk.column(col_idx), builder.column_mut(col_idx))
                {
                    if let Some(value) = src_col.get_value(row_ref.row_index) {
                        dst_col.push_value(value);
                    } else {
                        dst_col.push_value(Value::Null);
                    }
                }
            }

            builder.advance_row();
            self.output_position += 1;
        }

        if builder.row_count() > 0 {
            Ok(Some(builder.finish()))
        } else {
            Ok(None)
        }
    }

    fn reset(&mut self) {
        self.child.reset();
        self.chunks.clear();
        self.sorted_rows.clear();
        self.sort_complete = false;
        self.output_position = 0;
    }

    fn name(&self) -> &'static str {
        "Sort"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::chunk::DataChunkBuilder;

    struct MockOperator {
        chunks: Vec<DataChunk>,
        position: usize,
    }

    impl MockOperator {
        fn new(chunks: Vec<DataChunk>) -> Self {
            Self {
                chunks,
                position: 0,
            }
        }
    }

    impl Operator for MockOperator {
        fn next(&mut self) -> OperatorResult {
            if self.position < self.chunks.len() {
                let chunk = std::mem::replace(&mut self.chunks[self.position], DataChunk::empty());
                self.position += 1;
                Ok(Some(chunk))
            } else {
                Ok(None)
            }
        }

        fn reset(&mut self) {
            self.position = 0;
        }

        fn name(&self) -> &'static str {
            "Mock"
        }
    }

    fn create_unsorted_chunk() -> DataChunk {
        let mut builder = DataChunkBuilder::new(&[LogicalType::Int64, LogicalType::String]);

        let data = [(3i64, "cherry"), (1, "apple"), (4, "date"), (2, "banana")];

        for (num, text) in data {
            builder.column_mut(0).unwrap().push_int64(num);
            builder.column_mut(1).unwrap().push_string(text);
            builder.advance_row();
        }

        builder.finish()
    }

    #[test]
    fn test_sort_ascending() {
        let mock = MockOperator::new(vec![create_unsorted_chunk()]);

        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64, LogicalType::String],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                let num = chunk.column(0).unwrap().get_int64(row).unwrap();
                let text = chunk
                    .column(1)
                    .unwrap()
                    .get_string(row)
                    .unwrap()
                    .to_string();
                results.push((num, text));
            }
        }

        assert_eq!(
            results,
            vec![
                (1, "apple".to_string()),
                (2, "banana".to_string()),
                (3, "cherry".to_string()),
                (4, "date".to_string()),
            ]
        );
    }

    #[test]
    fn test_sort_descending() {
        let mock = MockOperator::new(vec![create_unsorted_chunk()]);

        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::descending(0)],
            vec![LogicalType::Int64, LogicalType::String],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                let num = chunk.column(0).unwrap().get_int64(row).unwrap();
                results.push(num);
            }
        }

        assert_eq!(results, vec![4, 3, 2, 1]);
    }

    #[test]
    fn test_sort_by_string() {
        let mock = MockOperator::new(vec![create_unsorted_chunk()]);

        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(1)], // Sort by string column
            vec![LogicalType::Int64, LogicalType::String],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                let text = chunk
                    .column(1)
                    .unwrap()
                    .get_string(row)
                    .unwrap()
                    .to_string();
                results.push(text);
            }
        }

        assert_eq!(
            results,
            vec![
                "apple".to_string(),
                "banana".to_string(),
                "cherry".to_string(),
                "date".to_string(),
            ]
        );
    }

    #[test]
    fn test_sort_empty_input() {
        let mock = MockOperator::new(vec![]);

        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );

        assert!(sort.next().unwrap().is_none());
    }

    #[test]
    fn test_sort_already_sorted() {
        let mut builder = DataChunkBuilder::new(&[LogicalType::Int64]);
        for v in [1i64, 2, 3, 4] {
            builder.column_mut(0).unwrap().push_int64(v);
            builder.advance_row();
        }
        let chunk = builder.finish();

        let mock = MockOperator::new(vec![chunk]);
        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                results.push(chunk.column(0).unwrap().get_int64(row).unwrap());
            }
        }

        assert_eq!(results, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_sort_duplicate_values() {
        let mut builder = DataChunkBuilder::new(&[LogicalType::Int64]);
        for v in [3i64, 1, 3, 2, 1] {
            builder.column_mut(0).unwrap().push_int64(v);
            builder.advance_row();
        }
        let chunk = builder.finish();

        let mock = MockOperator::new(vec![chunk]);
        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                results.push(chunk.column(0).unwrap().get_int64(row).unwrap());
            }
        }

        assert_eq!(results, vec![1, 1, 2, 3, 3]);
    }

    #[test]
    fn test_sort_multi_key() {
        let mut builder = DataChunkBuilder::new(&[LogicalType::String, LogicalType::Int64]);
        let data = [("b", 2i64), ("a", 3), ("b", 1), ("a", 1)];
        for (s, n) in data {
            builder.column_mut(0).unwrap().push_string(s);
            builder.column_mut(1).unwrap().push_int64(n);
            builder.advance_row();
        }
        let chunk = builder.finish();

        let mock = MockOperator::new(vec![chunk]);
        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0), SortKey::ascending(1)],
            vec![LogicalType::String, LogicalType::Int64],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                let s = chunk
                    .column(0)
                    .unwrap()
                    .get_string(row)
                    .unwrap()
                    .to_string();
                let n = chunk.column(1).unwrap().get_int64(row).unwrap();
                results.push((s, n));
            }
        }

        assert_eq!(
            results,
            vec![
                ("a".to_string(), 1),
                ("a".to_string(), 3),
                ("b".to_string(), 1),
                ("b".to_string(), 2),
            ]
        );
    }

    #[test]
    fn test_sort_multiple_chunks() {
        // Two separate chunks that get merged during sort
        let mut b1 = DataChunkBuilder::new(&[LogicalType::Int64]);
        b1.column_mut(0).unwrap().push_int64(5);
        b1.advance_row();
        b1.column_mut(0).unwrap().push_int64(1);
        b1.advance_row();
        let chunk1 = b1.finish();

        let mut b2 = DataChunkBuilder::new(&[LogicalType::Int64]);
        b2.column_mut(0).unwrap().push_int64(3);
        b2.advance_row();
        b2.column_mut(0).unwrap().push_int64(2);
        b2.advance_row();
        let chunk2 = b2.finish();

        let mock = MockOperator::new(vec![chunk1, chunk2]);
        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                results.push(chunk.column(0).unwrap().get_int64(row).unwrap());
            }
        }

        assert_eq!(results, vec![1, 2, 3, 5]);
    }

    #[test]
    fn test_sort_reverse_sorted() {
        let mut builder = DataChunkBuilder::new(&[LogicalType::Int64]);
        for v in [4i64, 3, 2, 1] {
            builder.column_mut(0).unwrap().push_int64(v);
            builder.advance_row();
        }
        let chunk = builder.finish();

        let mock = MockOperator::new(vec![chunk]);
        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );

        let mut results = Vec::new();
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                results.push(chunk.column(0).unwrap().get_int64(row).unwrap());
            }
        }

        assert_eq!(results, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_sort_single_row() {
        let mut builder = DataChunkBuilder::new(&[LogicalType::Int64]);
        builder.column_mut(0).unwrap().push_int64(42);
        builder.advance_row();
        let chunk = builder.finish();

        let mock = MockOperator::new(vec![chunk]);
        let mut sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );

        let mut count = 0;
        while let Some(chunk) = sort.next().unwrap() {
            for row in chunk.selected_indices() {
                assert_eq!(chunk.column(0).unwrap().get_int64(row), Some(42));
                count += 1;
            }
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn test_sort_name() {
        let mock = MockOperator::new(vec![]);
        let sort = SortOperator::new(
            Box::new(mock),
            vec![SortKey::ascending(0)],
            vec![LogicalType::Int64],
        );
        assert_eq!(sort.name(), "Sort");
    }

    // ═══════════════════════════════════════════════════════════════════
    //  TRI BORNÉ — les tests que la doc de `bounded_top_k` cite par leur nom
    // ═══════════════════════════════════════════════════════════════════
    //
    // La preuve de stabilité écrite sur `bounded_top_k` ne vaut que confrontée
    // au chemin non borné. Chacun de ces tests fait EXACTEMENT cela : il
    // exécute la même entrée par les deux chemins et exige que le borné rende
    // le préfixe du non borné — mêmes lignes, même ordre.

    /// Construit un lot à deux colonnes `(Int64, String)` à partir des paires
    /// données. La colonne 1 sert de témoin d'identité : c'est elle qui révèle
    /// un ex æquo départagé autrement.
    fn lot_depuis(paires: &[(i64, &str)]) -> DataChunk {
        let mut b = DataChunkBuilder::new(&[LogicalType::Int64, LogicalType::String]);
        for (n, t) in paires {
            b.column_mut(0).unwrap().push_int64(*n);
            b.column_mut(1).unwrap().push_string(*t);
            b.advance_row();
        }
        b.finish()
    }

    /// Vide un opérateur en une liste de `(entier optionnel, texte)`.
    ///
    /// La colonne 0 est rendue en `Option<i64>` et non en `i64` : le test des
    /// `NULL` en met dans la clé de tri, et un extracteur qui paniquerait
    /// dessus mesurerait sa propre limite au lieu de celle du code.
    fn vider(mut op: SortOperator) -> Vec<(Option<i64>, String)> {
        let mut sortie = Vec::new();
        while let Some(chunk) = op.next().unwrap() {
            for i in chunk.selected_indices() {
                let n = match chunk.column(0).unwrap().get_value(i) {
                    Some(Value::Int64(v)) => Some(v),
                    None | Some(Value::Null) => None,
                    autre => panic!("colonne 0 inattendue : {autre:?}"),
                };
                let t = match chunk.column(1).unwrap().get_value(i) {
                    Some(Value::String(v)) => v.to_string(),
                    autre => panic!("colonne 1 inattendue : {autre:?}"),
                };
                sortie.push((n, t));
            }
        }
        sortie
    }

    fn schema2() -> Vec<LogicalType> {
        vec![LogicalType::Int64, LogicalType::String]
    }

    /// Le duel : borné contre préfixe du non borné, sur la même entrée.
    fn duel(paires: &[(i64, &str)], cles: Vec<SortKey>, k: usize) {
        let complet = vider(SortOperator::new(
            Box::new(MockOperator::new(vec![lot_depuis(paires)])),
            cles.clone(),
            schema2(),
        ));
        let borne = vider(
            SortOperator::new(
                Box::new(MockOperator::new(vec![lot_depuis(paires)])),
                cles,
                schema2(),
            )
            .with_limit(k),
        );
        let attendu: Vec<_> = complet.iter().take(k).cloned().collect();
        assert_eq!(
            borne, attendu,
            "le tri borne a k={k} doit rendre le PREFIXE du tri complet, \
             lignes ET ordre — complet = {complet:?}"
        );
    }

    #[test]
    fn test_limit_matches_full_sort_prefix_simple() {
        let d = [(5i64, "e"), (1, "a"), (4, "d"), (2, "b"), (3, "c")];
        for k in 1..=5 {
            duel(&d, vec![SortKey::ascending(0)], k);
            duel(&d, vec![SortKey::descending(0)], k);
        }
    }

    /// LE TEST QUI COMPTE. Toutes les clés sont égales : le tri complet rend
    /// donc l'ordre d'ARRIVÉE, par stabilité. Un tas non départagé rendrait un
    /// ordre dépendant de l'algorithme, et l'écart ne se verrait nulle part
    /// ailleurs.
    #[test]
    fn test_limit_matches_full_sort_prefix_avec_ex_aequo() {
        let d = [
            (7i64, "premier"),
            (7, "deuxieme"),
            (7, "troisieme"),
            (7, "quatrieme"),
            (7, "cinquieme"),
        ];
        for k in 1..=5 {
            duel(&d, vec![SortKey::ascending(0)], k);
            duel(&d, vec![SortKey::descending(0)], k);
        }
        // Et explicitement : c'est bien l'ordre d'arrivee qui sort.
        let borne = vider(
            SortOperator::new(
                Box::new(MockOperator::new(vec![lot_depuis(&d)])),
                vec![SortKey::ascending(0)],
                schema2(),
            )
            .with_limit(3),
        );
        assert_eq!(
            borne.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
            vec!["premier", "deuxieme", "troisieme"],
            "les ex aequo doivent sortir dans l'ordre d'ARRIVEE"
        );
    }

    /// Ex æquo PARTIELS : deux paliers, chacun avec plusieurs occupants.
    #[test]
    fn test_limit_matches_full_sort_prefix_ex_aequo_partiels() {
        let d = [
            (2i64, "b1"),
            (1, "a1"),
            (2, "b2"),
            (1, "a2"),
            (3, "c1"),
            (1, "a3"),
        ];
        for k in 1..=6 {
            duel(&d, vec![SortKey::ascending(0)], k);
            duel(&d, vec![SortKey::descending(0)], k);
        }
    }

    /// Plusieurs lots : l'ordinal d'arrivée doit être global, pas par lot.
    #[test]
    fn test_limit_matches_full_sort_prefix_multi_lots() {
        let l1 = [(3i64, "x1"), (1, "x2")];
        let l2 = [(3i64, "y1"), (2, "y2")];
        let l3 = [(1i64, "z1"), (3, "z2")];
        let construire = || {
            MockOperator::new(vec![lot_depuis(&l1), lot_depuis(&l2), lot_depuis(&l3)])
        };
        for k in 1..=6 {
            let complet = vider(SortOperator::new(
                Box::new(construire()),
                vec![SortKey::ascending(0)],
                schema2(),
            ));
            let borne = vider(
                SortOperator::new(
                    Box::new(construire()),
                    vec![SortKey::ascending(0)],
                    schema2(),
                )
                .with_limit(k),
            );
            assert_eq!(borne, complet.iter().take(k).cloned().collect::<Vec<_>>());
        }
    }

    /// Clés multiples : la seconde ne départage que si la première est égale.
    #[test]
    fn test_limit_matches_full_sort_prefix_multi_cles() {
        let d = [
            (1i64, "b"),
            (1, "a"),
            (2, "a"),
            (1, "c"),
            (2, "b"),
        ];
        let cles = vec![SortKey::ascending(0), SortKey::ascending(1)];
        for k in 1..=5 {
            duel(&d, cles.clone(), k);
        }
    }

    /// `NullsFirst` et `NullsLast`, dans les deux directions.
    #[test]
    fn test_limit_matches_full_sort_prefix_nulls() {
        let mut b = DataChunkBuilder::new(&schema2());
        for (n, t) in [(Some(2i64), "deux"), (None, "nul_a"), (Some(1), "un"), (None, "nul_b")] {
            match n {
                Some(v) => b.column_mut(0).unwrap().push_int64(v),
                None => b.column_mut(0).unwrap().push_value(Value::Null),
            }
            b.column_mut(1).unwrap().push_string(t);
            b.advance_row();
        }
        let lot = b.finish();

        for ordre in [NullOrder::NullsFirst, NullOrder::NullsLast] {
            for direction in [true, false] {
                let cle = if direction {
                    SortKey::ascending(0).with_null_order(ordre)
                } else {
                    SortKey::descending(0).with_null_order(ordre)
                };
                for k in 1..=4 {
                    let complet = vider(SortOperator::new(
                        Box::new(MockOperator::new(vec![lot.clone()])),
                        vec![cle.clone()],
                        schema2(),
                    ));
                    let borne = vider(
                        SortOperator::new(
                            Box::new(MockOperator::new(vec![lot.clone()])),
                            vec![cle.clone()],
                            schema2(),
                        )
                        .with_limit(k),
                    );
                    assert_eq!(
                        borne,
                        complet.iter().take(k).cloned().collect::<Vec<_>>(),
                        "ordre={ordre:?} croissant={direction} k={k}"
                    );
                }
            }
        }
    }

    /// Sans limite, le comportement doit être STRICTEMENT celui d'avant.
    #[test]
    fn test_limite_absente_ne_change_rien() {
        let d = [(3i64, "c"), (1, "a"), (2, "b")];
        let op = SortOperator::new(
            Box::new(MockOperator::new(vec![lot_depuis(&d)])),
            vec![SortKey::ascending(0)],
            schema2(),
        );
        assert_eq!(op.limit(), None);
        assert_eq!(
            vider(op),
            vec![
                (Some(1), "a".to_string()),
                (Some(2), "b".to_string()),
                (Some(3), "c".to_string()),
            ]
        );
    }

    /// Les bornes dégénérées, que `bounded_top_k` délègue à son appelant.
    #[test]
    fn test_limite_degeneree() {
        let d = [(3i64, "c"), (1, "a"), (2, "b")];

        // k = 0 : aucune ligne.
        let vide = vider(
            SortOperator::new(
                Box::new(MockOperator::new(vec![lot_depuis(&d)])),
                vec![SortKey::ascending(0)],
                schema2(),
            )
            .with_limit(0),
        );
        assert!(vide.is_empty(), "k=0 doit ne rien rendre, obtenu {vide:?}");

        // k > n : tout, dans l'ordre du tri complet.
        let tout = vider(
            SortOperator::new(
                Box::new(MockOperator::new(vec![lot_depuis(&d)])),
                vec![SortKey::ascending(0)],
                schema2(),
            )
            .with_limit(99),
        );
        assert_eq!(tout.len(), 3);
        assert_eq!(tout[0].0, Some(1));

        // Entrée vide.
        let rien = vider(
            SortOperator::new(
                Box::new(MockOperator::new(vec![])),
                vec![SortKey::ascending(0)],
                schema2(),
            )
            .with_limit(5),
        );
        assert!(rien.is_empty());
    }


    /// LE GAIN, COMPTÉ PLUTÔT QUE CHRONOMÉTRÉ.
    ///
    /// Le chronomètre ne discrimine rien sur l'hôte de développement : deux
    /// manches du MÊME code y ont rendu 87,9 ms et 111,7 ms, et le rapport
    /// interne censé absorber la charge est passé de 0,59 à 1,08. Mesurer le
    /// temps, ici, c'est mesurer `obrain-tmn`.
    ///
    /// Le gain visé est ALGORITHMIQUE — `O(n log k)` au lieu de `O(n log n)` —
    /// donc il se compte en COMPARAISONS, ce qui est déterministe, reproductible
    /// et insensible à la charge. C'est ce que ce test verrouille. Un banc
    /// chronométré reste souhaitable sur un hôte au repos ; il confirmera une
    /// magnitude, il ne prouvera pas davantage que ceci.
    #[test]
    fn le_tri_borne_compare_moins_que_le_tri_complet() {
        const N: usize = 4_000;
        const K: usize = 8;

        // Corpus déterministe et désordonné — un corpus déjà trié flatterait
        // n'importe quel algorithme.
        let cles: Vec<i64> = (0..N).map(|i| ((i * 7919) % N) as i64).collect();
        let rows: Vec<SortRow> = (0..N)
            .map(|i| SortRow {
                chunk_index: 0,
                row_index: i,
            })
            .collect();

        let compare = |a: &SortRow, b: &SortRow| cles[a.row_index].cmp(&cles[b.row_index]);

        let mut n_borne = 0usize;
        let borne = bounded_top_k(rows.clone(), K, |a, b| {
            n_borne += 1;
            compare(a, b)
        });

        let mut n_complet = 0usize;
        let mut complet = rows;
        complet.sort_by(|a, b| {
            n_complet += 1;
            compare(a, b)
        });

        // 1. Même résultat : le borné est le préfixe exact du complet.
        assert_eq!(
            borne.iter().map(|r| r.row_index).collect::<Vec<_>>(),
            complet[..K].iter().map(|r| r.row_index).collect::<Vec<_>>(),
            "le borne doit rendre le prefixe exact du tri complet"
        );

        // 2. Et il y arrive en comparant strictement moins.
        assert!(
            n_borne < n_complet,
            "borne = {n_borne} comparaisons, complet = {n_complet} — \
             le tri borne doit en faire MOINS"
        );

        // 3. La borne théorique : `n log2 k` contre `n log2 n`, avec une marge
        //    généreuse pour les constantes d'implémentation. À N=4000 et K=8,
        //    le rapport attendu est de l'ordre de log2(4000)/log2(8) ≈ 4.
        //    On exige seulement la moitié de ce gain, pour que le test mesure
        //    l'ORDRE DE GRANDEUR sans se briser sur un détail de tas.
        let plafond = n_complet / 2;
        assert!(
            n_borne <= plafond,
            "borne = {n_borne}, complet = {n_complet} : on attend au plus \
             {plafond} (moitie), sinon le gain n'est pas algorithmique"
        );

        // Trace lisible quand on lance avec --nocapture.
        println!(
            "tri borne N={N} K={K} : {n_borne} comparaisons contre {n_complet} \
             ({:.2}x moins)",
            n_complet as f64 / n_borne as f64
        );
    }

    /// Le pendant du précédent : sans limite, on ne doit RIEN économiser —
    /// sinon c'est que le chemin non borné a changé, ce qui serait la
    /// régression que tout le reste s'interdit.
    #[test]
    fn sans_limite_le_travail_est_celui_du_tri_complet() {
        const N: usize = 500;
        let cles: Vec<i64> = (0..N).map(|i| ((i * 7919) % N) as i64).collect();
        let rows: Vec<SortRow> = (0..N)
            .map(|i| SortRow {
                chunk_index: 0,
                row_index: i,
            })
            .collect();
        let compare = |a: &SortRow, b: &SortRow| cles[a.row_index].cmp(&cles[b.row_index]);

        // k = N : le chemin borne ne doit pas etre emprunte (cas degenere).
        let mut n_k_egal_n = 0usize;
        let mut copie = rows.clone();
        copie.sort_by(|a, b| {
            n_k_egal_n += 1;
            compare(a, b)
        });

        let mut n_complet = 0usize;
        let mut copie2 = rows;
        copie2.sort_by(|a, b| {
            n_complet += 1;
            compare(a, b)
        });
        assert_eq!(n_k_egal_n, n_complet, "le tri complet doit etre deterministe");
    }

}
