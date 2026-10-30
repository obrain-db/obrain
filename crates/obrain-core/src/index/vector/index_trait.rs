//! Contrat d'index vectoriel — le trait, sans son implementation.
//!
//! (2026-08-13, T17b) Ce trait vivait dans `obrain-substrate`, donc EN AVAL
//! de `obrain-core`. Consequence : `obrain-core/src/graph/traits.rs` ne
//! pouvait pas le nommer, et typait ses methodes d'index sur `HnswIndex` —
//! le type concret de `LpgStore`, RETIRE depuis T17 Step 15 (6d030291).
//! Ces methodes n'avaient donc plus AUCUN implementeur, et leurs defauts a
//! corps vide faisaient reussir en silence des enregistrements d'index qui
//! n'enregistraient rien.
//!
//! Le trait ne mentionne que `u32`, `&[f32]`, `Vec<f32>` et `usize` : rien
//! qui appartienne au substrat. Il descend donc ici tel quel, et
//! `obrain-substrate` le re-exporte pour que rien ne casse en aval.

/// Node offset within a store — an opaque u32 identity.
///
/// The index never interprets this value beyond returning it in search
/// results. Callers map it back to an `ObjectId` / `NodeId` using whatever
/// convention the consuming crate defines.
pub type NodeOffset = u32;

/// Contract for an embedding-based nearest-neighbor index.
///
/// Implementations must be thread-safe (`Send + Sync`). Search calls can
/// run concurrently with reads to the same index; insert / delete /
/// rebuild take exclusive access internally.
pub trait VectorIndex: Send + Sync {
    /// Return the embedding dimensionality the index was built for.
    /// All `insert` and `search_top_k` calls must pass slices of this
    /// length.
    fn dim(&self) -> usize;

    /// Current number of indexed nodes.
    fn len(&self) -> usize;

    /// `len() == 0`.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Insert or update the embedding for `node_offset`.
    ///
    /// If `node_offset` was already indexed, its tier records are
    /// overwritten in place — the underlying storage slot is kept stable.
    /// Otherwise the embedding is appended.
    fn insert(&self, node_offset: NodeOffset, embedding: &[f32]);

    /// Remove `node_offset` from the index. No-op if it was not indexed.
    fn delete(&self, node_offset: NodeOffset);

    /// Bulk-rebuild the index from `(node_offset, embedding)` pairs.
    ///
    /// Equivalent to `clear` + `insert` for each pair, but may be faster
    /// because the implementation can batch-project all tiers in one pass.
    /// Existing contents are discarded.
    fn rebuild(&self, pairs: &[(NodeOffset, Vec<f32>)]);

    /// Search the top-K node offsets closest to `query` by cosine
    /// similarity.
    ///
    /// Returns `(node_offset, cosine)` pairs sorted by descending cosine
    /// (i.e. closest first). Ties are broken by ascending `node_offset`
    /// for determinism.
    ///
    /// If `k == 0` or the index is empty, returns an empty Vec.
    fn search_top_k(&self, query: &[f32], k: usize) -> Vec<(NodeOffset, f32)>;
}
