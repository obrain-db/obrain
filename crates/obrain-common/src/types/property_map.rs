//! Compact property storage for nodes and edges.
//!
//! [`PropertyMap`] replaces `BTreeMap<PropertyKey, Value>` on user-facing
//! [`Node`](crate::graph::lpg::Node) and [`Edge`](crate::graph::lpg::Edge)
//! types.  For the typical case of 1-4 properties the data lives entirely
//! inline (no heap allocation). Entries are kept SORTED by key and looked up
//! by binary search, so bulk hydration can sort once (O(P log P)) instead of
//! inserting P times (O(P²)).

use std::collections::BTreeMap;
use std::fmt;

use smallvec::SmallVec;

use super::value::{PropertyKey, Value};

/// Inline capacity: entries stored on the stack without allocation.
///
/// Covers the common case of 1-4 properties per entity.  Beyond this the
/// backing `SmallVec` spills to the heap but remains a flat array, so
/// iteration is still cache-friendly.
const INLINE_CAP: usize = 4;

/// A compact, ordered property map.
///
/// Behaves like a `BTreeMap<PropertyKey, Value>` but stores small maps
/// inline (up to 4 entries). Entries are kept SORTED by key and looked up
/// by binary search (O(log P)). The bulk-load path
/// ([`Self::from_sorted_pairs`]) sorts the pairs once instead of inserting
/// them one by one — the difference between O(P log P) and O(P²).
#[derive(Clone, PartialEq)]
pub struct PropertyMap {
    entries: SmallVec<[(PropertyKey, Value); INLINE_CAP]>,
}

impl PropertyMap {
    /// Creates an empty property map.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: SmallVec::new(),
        }
    }

    /// Creates a property map with pre-allocated capacity.
    #[inline]
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: SmallVec::with_capacity(cap),
        }
    }

    /// Construit une `PropertyMap` depuis des paires `(clef, valeur)` DÉJÀ
    /// triées par clef et dédupliquées.
    ///
    /// POURQUOI ce constructeur existe : le chemin de lecture en masse
    /// (`read_all_properties_for_node`) décode P paires, les trie et les
    /// déduplique EN UNE passe (O(P log P)), puis les remet ici. Sans lui, la
    /// seule voie de remplissage était `insert` répété — O(P²) par balayage
    /// linéaire. Le `Vec` est adopté tel quel : aucune re-copie pour les maps
    /// qui débordent la capacité en ligne (≤ 4 entrées restent en ligne).
    #[inline]
    #[must_use]
    pub fn from_sorted_pairs(pairs: Vec<(PropertyKey, Value)>) -> Self {
        Self {
            entries: SmallVec::from_vec(pairs),
        }
    }

    /// Returns the number of properties.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the map contains no properties.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up a property by key.
    ///
    /// POURQUOI la dichotomie : les entrées sont maintenues TRIÉES par clef
    /// (`PropertyKey` est `Ord`), donc une recherche coûte O(log P) au lieu du
    /// balayage linéaire O(P). C'est le socle qui fait passer l'hydratation
    /// d'un nœud de O(P²) à O(P log P) une fois le remplissage trié en masse.
    #[must_use]
    pub fn get(&self, key: &PropertyKey) -> Option<&Value> {
        self.entries
            .binary_search_by(|(k, _)| k.cmp(key))
            .ok()
            .map(|pos| &self.entries[pos].1)
    }

    /// Returns `true` if the map contains the given key.
    #[must_use]
    pub fn contains_key(&self, key: &PropertyKey) -> bool {
        self.entries.binary_search_by(|(k, _)| k.cmp(key)).is_ok()
    }

    /// Inserts a property, replacing any existing value for the same key.
    ///
    /// Returns the previous value if the key was already present.
    ///
    /// POURQUOI l'insertion par dichotomie : le tableau reste TRIÉ par clef.
    /// Chaque insertion coûte O(log P) de recherche + O(P) de glissement de
    /// mémoire (inévitable pour un tableau ordonné). L'essentiel est de ne
    /// plus faire P balayages linéaires pour P insertions : le chemin en masse
    /// passe par [`Self::from_sorted_pairs`] pour ne trier qu'UNE seule fois.
    pub fn insert(&mut self, key: PropertyKey, value: Value) -> Option<Value> {
        match self.entries.binary_search_by(|(k, _)| k.cmp(&key)) {
            Ok(pos) => Some(std::mem::replace(&mut self.entries[pos].1, value)),
            Err(pos) => {
                self.entries.insert(pos, (key, value));
                None
            }
        }
    }

    /// Removes a property by key, returning its value if present.
    ///
    /// POURQUOI un retrait ORDONNÉ (`remove` et non `swap_remove`) : il
    /// préserve l'invariant de tri dont `get`/`contains_key`/`insert`
    /// dépendent désormais. Le glissement O(P) est sans objet sur le chemin
    /// chaud, où les suppressions passent par le build trié en masse.
    pub fn remove(&mut self, key: &PropertyKey) -> Option<Value> {
        match self.entries.binary_search_by(|(k, _)| k.cmp(key)) {
            Ok(pos) => Some(self.entries.remove(pos).1),
            Err(_) => None,
        }
    }

    /// Iterates over `(key, value)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&PropertyKey, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    /// Converts to a `BTreeMap` (e.g. for serialization into `Value::Map`).
    #[must_use]
    pub fn to_btree_map(&self) -> BTreeMap<PropertyKey, Value> {
        self.entries.iter().cloned().collect()
    }
}

impl Default for PropertyMap {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PropertyMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.entries.iter().map(|(k, v)| (k, v)))
            .finish()
    }
}

impl FromIterator<(PropertyKey, Value)> for PropertyMap {
    fn from_iter<I: IntoIterator<Item = (PropertyKey, Value)>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut map = Self::with_capacity(lower);
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl IntoIterator for PropertyMap {
    type Item = (PropertyKey, Value);
    type IntoIter = smallvec::IntoIter<[(PropertyKey, Value); INLINE_CAP]>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a PropertyMap {
    type Item = (&'a PropertyKey, &'a Value);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (PropertyKey, Value)>,
        fn(&'a (PropertyKey, Value)) -> (&'a PropertyKey, &'a Value),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(k, v)| (k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("name"), Value::from("Alix"));
        map.insert(PropertyKey::new("age"), Value::from(30i64));

        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&PropertyKey::new("name")).and_then(Value::as_str),
            Some("Alix")
        );
        assert_eq!(
            map.get(&PropertyKey::new("age")).and_then(Value::as_int64),
            Some(30)
        );
    }

    #[test]
    fn insert_replaces_existing() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("x"), Value::from(1i64));
        let old = map.insert(PropertyKey::new("x"), Value::from(2i64));
        assert_eq!(old, Some(Value::from(1i64)));
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&PropertyKey::new("x")).and_then(Value::as_int64),
            Some(2)
        );
    }

    #[test]
    fn remove() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("a"), Value::from(1i64));
        map.insert(PropertyKey::new("b"), Value::from(2i64));

        let removed = map.remove(&PropertyKey::new("a"));
        assert_eq!(removed, Some(Value::from(1i64)));
        assert_eq!(map.len(), 1);
        assert!(map.get(&PropertyKey::new("a")).is_none());
        assert!(map.get(&PropertyKey::new("b")).is_some());
    }

    #[test]
    fn contains_key() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("x"), Value::Null);
        assert!(map.contains_key(&PropertyKey::new("x")));
        assert!(!map.contains_key(&PropertyKey::new("y")));
    }

    #[test]
    fn from_iterator() {
        let pairs = vec![
            (PropertyKey::new("a"), Value::from(1i64)),
            (PropertyKey::new("b"), Value::from(2i64)),
        ];
        let map: PropertyMap = pairs.into_iter().collect();
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn into_iterator() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("x"), Value::from(1i64));
        map.insert(PropertyKey::new("y"), Value::from(2i64));

        let pairs: Vec<_> = map.into_iter().collect();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn empty() {
        let map = PropertyMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert!(map.get(&PropertyKey::new("x")).is_none());
    }

    #[test]
    fn inline_capacity() {
        // 4 entries should stay inline (no heap allocation)
        let mut map = PropertyMap::new();
        for i in 0..4 {
            map.insert(PropertyKey::new(format!("k{i}")), Value::from(i as i64));
        }
        assert_eq!(map.len(), 4);
        assert!(!map.entries.spilled());
    }

    #[test]
    fn spills_to_heap_beyond_capacity() {
        let mut map = PropertyMap::new();
        for i in 0..10 {
            map.insert(PropertyKey::new(format!("k{i}")), Value::from(i as i64));
        }
        assert_eq!(map.len(), 10);
        assert!(map.entries.spilled());
        // All entries still accessible
        for i in 0..10 {
            assert!(map.contains_key(&PropertyKey::new(format!("k{i}"))));
        }
    }

    #[test]
    fn debug_format() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("name"), Value::from("Alix"));
        let dbg = format!("{map:?}");
        assert!(dbg.contains("name"));
    }

    #[test]
    fn from_empty_iterator() {
        let map: PropertyMap = std::iter::empty().collect();
        assert!(map.is_empty());
    }

    #[test]
    fn ref_iterator() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("a"), Value::from(1i64));
        map.insert(PropertyKey::new("b"), Value::from(2i64));
        let pairs: Vec<_> = (&map).into_iter().collect();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn default_is_empty() {
        let map = PropertyMap::default();
        assert!(map.is_empty());
    }

    #[test]
    fn remove_nonexistent() {
        let mut map = PropertyMap::new();
        assert_eq!(map.remove(&PropertyKey::new("missing")), None);
    }

    #[test]
    fn to_btree_map_round_trip() {
        let mut map = PropertyMap::new();
        map.insert(PropertyKey::new("x"), Value::from(1i64));
        map.insert(PropertyKey::new("y"), Value::from(2i64));
        let btree = map.to_btree_map();
        assert_eq!(btree.len(), 2);
        assert_eq!(btree[&PropertyKey::new("x")], Value::from(1i64));
    }

    // ===================================================================
    // T17k-opt — invariant TRIÉ + build en masse. Ces tests verrouillent
    // l'hypothèse sur laquelle la dichotomie repose : sans un tableau trié,
    // `get`/`contains_key`/`insert` rendraient des réponses fausses, pas
    // seulement lentes. Le build `from_sorted_pairs` est le pendant O(P) du
    // remplissage O(P²) par `insert` répété.
    // ===================================================================

    /// L'invariant central : quelle que soit la séquence d'`insert`/`remove`,
    /// les entrées restent triées par clef (condition d'exactitude de la
    /// dichotomie), et chaque clef insérée reste lisible.
    #[test]
    fn insert_maintains_sorted_invariant() {
        let mut map = PropertyMap::new();
        // Ordre d'insertion délibérément NON trié.
        for name in ["z", "m", "a", "q", "b", "y", "c", "x"] {
            map.insert(PropertyKey::new(name), Value::from(name.len() as i64));
        }
        // Trié par clef après chaque insertion.
        let keys: Vec<&PropertyKey> = map.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "les entrées doivent rester triées par clef");

        for name in ["z", "m", "a", "q", "b", "y", "c", "x"] {
            assert_eq!(
                map.get(&PropertyKey::new(name)).and_then(Value::as_int64),
                Some(name.len() as i64)
            );
        }
    }

    /// Un `remove` ne doit pas casser l'invariant de tri (l'ancien
    /// `swap_remove` échangeait le trou avec le dernier élément).
    #[test]
    fn remove_maintains_sorted_invariant() {
        let mut map = PropertyMap::new();
        for name in ["a", "b", "c", "d", "e"] {
            map.insert(PropertyKey::new(name), Value::from(1i64));
        }
        map.remove(&PropertyKey::new("c"));
        let keys: Vec<&PropertyKey> = map.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "remove doit préserver l'ordre trié");
        assert!(map.get(&PropertyKey::new("c")).is_none());
        assert!(map.get(&PropertyKey::new("d")).is_some());
    }

    /// Le chemin en masse : P paires triées → `from_sorted_pairs` les adopte
    /// en O(P) (aucun `insert`), et chaque lecture reste O(log P). C'est la
    /// contre-partie fonctionnelle du gain mesuré par `benches/read_path.rs`
    /// (`effet_P` : P=128 ~39,7 µs → ~12 µs).
    #[test]
    fn from_sorted_pairs_bulk_build_of_128_keys() {
        const P: usize = 128;
        let mut paires: Vec<(PropertyKey, Value)> = (0..P)
            .map(|i| {
                (
                    PropertyKey::new(format!("prop{i:03}")),
                    Value::from(i as i64),
                )
            })
            .collect();
        paires.sort_by(|a, b| a.0.cmp(&b.0));

        let map = PropertyMap::from_sorted_pairs(paires);
        assert_eq!(map.len(), P);

        // Chaque clef est lisible par dichotomie, dans les deux sens.
        for i in 0..P {
            let cle = PropertyKey::new(format!("prop{i:03}"));
            assert_eq!(map.get(&cle).and_then(Value::as_int64), Some(i as i64));
            assert!(map.contains_key(&cle));
        }
        assert!(!map.contains_key(&PropertyKey::new("prop999")));

        // L'itération sort bien triée (le `Vec` a été adopté tel quel).
        let keys: Vec<&PropertyKey> = map.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
