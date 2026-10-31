//! SelectionVector for filtering.

/// A selection vector indicating which rows are active.
///
/// Used for efficient filtering without copying data.
///
/// Les indices sont stockés en `u32`. L'ancien stockage `u16` tronquait EN
/// SILENCE (`i as u16` dans `from_predicate`) tout indice ≥ 65 536 : sur le
/// chunk aplati d'une chaîne factorisée (178 593 lignes mesurées en prod, base
/// carto), les indices s'aliasaient modulo 65 536 — lignes dupliquées, lignes
/// d'autrui rendues (fuite de label :Method sous un motif `:Function`), et un
/// filtre EMPILÉ qui relisait ces indices corrompus rendait 20 liaisons sur
/// 117 (note PO 563e670f, lot de tests `gql_16_chemins`).
#[derive(Debug, Clone)]
pub struct SelectionVector {
    /// Indices of selected rows.
    indices: Vec<u32>,
}

impl SelectionVector {
    /// Maximum representable row index.
    pub const MAX_CAPACITY: usize = u32::MAX as usize;

    /// Creates a new selection vector selecting all rows up to count.
    #[must_use]
    pub fn new_all(count: usize) -> Self {
        assert!(count <= Self::MAX_CAPACITY);
        Self {
            indices: (0..count as u32).collect(),
        }
    }

    /// Creates a new empty selection vector.
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            indices: Vec::new(),
        }
    }

    /// Creates a new selection vector with the given capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            indices: Vec::with_capacity(capacity.min(Self::MAX_CAPACITY)),
        }
    }

    /// Creates a selection vector from a predicate.
    ///
    /// Selects all indices where the predicate returns true.
    ///
    /// PANIQUE si `count` dépasse [`Self::MAX_CAPACITY`] : un indice
    /// non représentable doit REFUSER la requête, jamais aliaser une autre
    /// ligne en silence (c'était le défaut : `i as u16` sans garde).
    #[must_use]
    pub fn from_predicate<F>(count: usize, predicate: F) -> Self
    where
        F: Fn(usize) -> bool,
    {
        assert!(
            count <= Self::MAX_CAPACITY,
            "selection sur {count} lignes : dépasse MAX_CAPACITY"
        );
        let indices: Vec<u32> = (0..count)
            .filter(|&i| predicate(i))
            .map(|i| i as u32)
            .collect();
        Self { indices }
    }

    /// Returns the number of selected rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns true if no rows are selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Gets the actual row index at position.
    #[must_use]
    pub fn get(&self, position: usize) -> Option<usize> {
        self.indices.get(position).map(|&i| i as usize)
    }

    /// Pushes a new index.
    pub fn push(&mut self, index: usize) {
        assert!(index <= Self::MAX_CAPACITY);
        self.indices.push(index as u32);
    }

    /// Returns the indices as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u32] {
        &self.indices
    }

    /// Clears all selections.
    pub fn clear(&mut self) {
        self.indices.clear();
    }

    /// Filters this selection by another predicate.
    ///
    /// Returns a new selection containing only indices that pass the predicate.
    #[must_use]
    pub fn filter<F>(&self, predicate: F) -> Self
    where
        F: Fn(usize) -> bool,
    {
        let indices: Vec<u32> = self
            .indices
            .iter()
            .copied()
            .filter(|&i| predicate(i as usize))
            .collect();
        Self { indices }
    }

    /// Computes the intersection of two selection vectors.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        // Assumes both are sorted (which they typically are)
        let mut result = Vec::new();
        let mut i = 0;
        let mut j = 0;

        while i < self.indices.len() && j < other.indices.len() {
            match self.indices[i].cmp(&other.indices[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    result.push(self.indices[i]);
                    i += 1;
                    j += 1;
                }
            }
        }

        Self { indices: result }
    }

    /// Computes the union of two selection vectors.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut result = Vec::new();
        let mut i = 0;
        let mut j = 0;

        while i < self.indices.len() && j < other.indices.len() {
            match self.indices[i].cmp(&other.indices[j]) {
                std::cmp::Ordering::Less => {
                    result.push(self.indices[i]);
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    result.push(other.indices[j]);
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    result.push(self.indices[i]);
                    i += 1;
                    j += 1;
                }
            }
        }

        result.extend_from_slice(&self.indices[i..]);
        result.extend_from_slice(&other.indices[j..]);

        Self { indices: result }
    }

    /// Returns an iterator over selected indices.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.indices.iter().map(|&i| i as usize)
    }

    /// Checks if a given index is in the selection.
    #[must_use]
    pub fn contains(&self, index: usize) -> bool {
        if index > u32::MAX as usize {
            return false;
        }
        // Since indices are typically sorted, use binary search
        self.indices.binary_search(&(index as u32)).is_ok()
    }
}

impl Default for SelectionVector {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl IntoIterator for SelectionVector {
    type Item = usize;
    type IntoIter = std::iter::Map<std::vec::IntoIter<u32>, fn(u32) -> usize>;

    fn into_iter(self) -> Self::IntoIter {
        self.indices.into_iter().map(|i| i as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_all() {
        let sel = SelectionVector::new_all(10);
        assert_eq!(sel.len(), 10);

        for i in 0..10 {
            assert_eq!(sel.get(i), Some(i));
        }
    }

    #[test]
    fn test_selection_from_predicate() {
        let sel = SelectionVector::from_predicate(10, |i| i % 2 == 0);

        assert_eq!(sel.len(), 5);
        assert_eq!(sel.get(0), Some(0));
        assert_eq!(sel.get(1), Some(2));
        assert_eq!(sel.get(2), Some(4));
    }

    #[test]
    fn test_selection_filter() {
        let sel = SelectionVector::new_all(10);
        let filtered = sel.filter(|i| i >= 5);

        assert_eq!(filtered.len(), 5);
        assert_eq!(filtered.get(0), Some(5));
    }

    #[test]
    fn test_selection_intersect() {
        let sel1 = SelectionVector::from_predicate(10, |i| i % 2 == 0); // 0, 2, 4, 6, 8
        let sel2 = SelectionVector::from_predicate(10, |i| i % 3 == 0); // 0, 3, 6, 9

        let intersection = sel1.intersect(&sel2);
        // Intersection: 0, 6

        assert_eq!(intersection.len(), 2);
        assert_eq!(intersection.get(0), Some(0));
        assert_eq!(intersection.get(1), Some(6));
    }

    #[test]
    fn test_selection_union() {
        let sel1 = SelectionVector::from_predicate(10, |i| i == 1 || i == 3); // 1, 3
        let sel2 = SelectionVector::from_predicate(10, |i| i == 2 || i == 3); // 2, 3

        let union = sel1.union(&sel2);
        // Union: 1, 2, 3

        assert_eq!(union.len(), 3);
        assert_eq!(union.get(0), Some(1));
        assert_eq!(union.get(1), Some(2));
        assert_eq!(union.get(2), Some(3));
    }

    #[test]
    fn test_selection_iterator() {
        let sel = SelectionVector::from_predicate(5, |i| i < 3);
        let collected: Vec<_> = sel.iter().collect();

        assert_eq!(collected, vec![0, 1, 2]);
    }
}
