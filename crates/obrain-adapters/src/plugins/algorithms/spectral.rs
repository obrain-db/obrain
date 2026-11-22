//! Spectral embedding algorithms based on the normalized graph Laplacian.
//!
//! Computes low-dimensional embeddings by extracting the smallest non-trivial
//! eigenvectors of the normalized Laplacian matrix:
//!
//! ```text
//! L_norm = I - D^{-1/2} W D^{-1/2}
//! ```
//!
//! where `W` is the (weighted) adjacency matrix and `D` is the diagonal degree
//! matrix. The eigenvectors corresponding to the smallest eigenvalues capture
//! the global connectivity structure of the graph.
//!
//! Uses a block subspace iteration with Rayleigh–Ritz on the lazy operator
//! `(I + M)/2`, `M = D^{-1/2} W_s D^{-1/2}`, `W_s = (W + Wᵀ)/2` (whose largest
//! non-trivial eigenvectors = smallest of `L_norm`). Graph read as undirected.
//!
//! ## Usage
//!
//! ```no_run
//! use obrain_adapters::plugins::algorithms::spectral_embedding;
//! use obrain_substrate::SubstrateStore;
//!
//! let store = SubstrateStore::open_tempfile().unwrap();
//! // ... populate graph ...
//! let embeddings = spectral_embedding(&store, 4, None);
//! // embeddings: HashMap<NodeId, Vec<f64>> with 4 dimensions per node
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_common::utils::error::Result;
#[cfg(test)]
use obrain_core::graph::GraphStoreMut;
use obrain_core::graph::{Direction, GraphStore};
#[cfg(test)]
use obrain_substrate::SubstrateStore;

use super::super::{AlgorithmResult, ParameterDef, ParameterType, Parameters};
use super::traits::GraphAlgorithm;

// ============================================================================
// Public API
// ============================================================================

/// Result of spectral embedding computation.
#[derive(Debug, Clone)]
pub struct SpectralEmbeddingResult {
    /// Embedding vectors per node: NodeId → `Vec<f64>` of length `dimensions`.
    pub embeddings: HashMap<NodeId, Vec<f64>>,
    /// Number of dimensions.
    pub dimensions: usize,
}

/// Compute spectral embedding for all nodes in a graph.
///
/// Extracts the `dimensions` smallest non-trivial eigenvectors of the
/// normalized Laplacian via power iteration with deflation.
///
/// # Arguments
///
/// * `store` - The graph store (supports `GraphProjection` for filtered views)
/// * `dimensions` - Number of embedding dimensions (typically 2, 4, 6, or 8)
/// * `weight_property` - Optional edge property name to use as weight (default: 1.0)
///
/// # Returns
///
/// A `SpectralEmbeddingResult` with normalized embeddings in [0, 1] per dimension.
///
/// # Complexity
///
/// O(iterations × (k+3) × (V + E)), `iterations` ≤ 3000, stops on Ritz-value convergence.
pub fn spectral_embedding(
    store: &dyn GraphStore,
    dimensions: usize,
    weight_property: Option<&str>,
) -> SpectralEmbeddingResult {
    let node_ids = store.node_ids();
    let n = node_ids.len();

    if n <= 1 {
        let embeddings = node_ids
            .iter()
            .map(|&nid| (nid, vec![0.5; dimensions]))
            .collect();
        return SpectralEmbeddingResult {
            embeddings,
            dimensions,
        };
    }

    // Build dense index: NodeId ↔ usize
    let id_to_idx: HashMap<NodeId, usize> = node_ids
        .iter()
        .enumerate()
        .map(|(i, &nid)| (nid, i))
        .collect();

    // Build weighted adjacency list
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (idx, &nid) in node_ids.iter().enumerate() {
        for (neighbor, edge_id) in store.edges_from(nid, Direction::Outgoing) {
            if let Some(&j) = id_to_idx.get(&neighbor) {
                let w = weight_property
                    .and_then(|prop| store.get_edge_property(edge_id, &PropertyKey::from(prop)))
                    .and_then(|v| match v {
                        Value::Float64(f) => Some(f),
                        Value::Int64(i) => Some(i as f64),
                        _ => None,
                    })
                    .unwrap_or(1.0);
                adj[idx].push((j, w));
            }
        }
    }

    // Compute eigenvectors
    let eigenvectors = spectral_eigenvectors(&adj, dimensions);

    // Build result
    let mut embeddings = HashMap::with_capacity(n);
    for (idx, &nid) in node_ids.iter().enumerate() {
        let mut vec = Vec::with_capacity(dimensions);
        for ev in &eigenvectors {
            vec.push(ev[idx]);
        }
        embeddings.insert(nid, vec);
    }

    SpectralEmbeddingResult {
        embeddings,
        dimensions,
    }
}

// ============================================================================
// Core Linear Algebra
// ============================================================================

/// Extract `k` smallest non-trivial eigenvectors of the normalized Laplacian.
///
/// Works on the similarity matrix `M = D^{-1/2} W_s D^{-1/2}` with
/// `W_s = (W + Wᵀ)/2` (the graph is read as undirected: the former code
/// iterated the *transpose* of the directed `W`, and a node without
/// outgoing edge turned every embedding into a constant). Nodes of degree 0
/// in `W_s` are set aside and receive the neutral value 0.5 — only them.
///
/// Returns `k` vectors (length `adj.len()`), each normalized to [0, 1].
fn spectral_eigenvectors(adj: &[Vec<(usize, f64)>], k: usize) -> Vec<Vec<f64>> {
    let n = adj.len();
    let (_values, vectors, active) = spectral_eigenpairs(adj, k);
    let mut out = Vec::with_capacity(k);
    for c in 0..k {
        let mut full = vec![0.5; n];
        if let Some(v) = vectors.get(c) {
            let unit = normalize_to_unit(v);
            for (a, &idx) in active.iter().enumerate() {
                full[idx] = unit[a];
            }
        }
        out.push(full);
    }
    out
}

/// Eigenpairs of `M = D^{-1/2} W_s D^{-1/2}` beyond the trivial one
/// (eigenvalue 1, vector ∝ `D^{1/2}·1`), largest first.
///
/// Returns `(values, vectors, active)`: `vectors[c]` is a unit vector over
/// the `active` nodes (those with symmetrised degree > 0); `values[c]` its
/// eigenvalue of `M` (`λ(L_norm) = 1 − value`). Fewer than `k` pairs are
/// returned when the active graph is too small.
///
/// The solver is a block subspace iteration with Rayleigh–Ritz on the lazy
/// operator `(I + M)/2` (spectrum in [0, 1], so bipartite graphs' `−1` is
/// mapped to 0 and can no longer dominate), deflated against the trivial
/// vector. A block handles multiple eigenvalues (rings) that a sequential
/// deflation with a fixed iteration budget cannot.
#[allow(clippy::many_single_char_names)] // notation matricielle : i, j, w (indices et poids de l'arête)
fn spectral_eigenpairs(
    adj: &[Vec<(usize, f64)>],
    k: usize,
) -> (Vec<f64>, Vec<Vec<f64>>, Vec<usize>) {
    let n_all = adj.len();

    // Symmetrise: W_s = (W + Wᵀ)/2, over all nodes.
    let mut ws: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n_all];
    for (i, row) in adj.iter().enumerate() {
        for &(j, w) in row {
            // Équivalent EXACT de `!(w > 0.0)` : `w <= 0.0` retourné laisserait
            // NaN passer (toute comparaison avec NaN est fausse), donc on isole
            // le cas NaN au lieu de changer la sémantique pour l'infini.
            if i == j || w.is_nan() || w <= 0.0 {
                continue;
            }
            ws[i].push((j, w / 2.0));
            ws[j].push((i, w / 2.0));
        }
    }

    // Set aside degree-0 nodes; dense re-indexing of the others.
    let deg_all: Vec<f64> = ws
        .iter()
        .map(|r| r.iter().map(|&(_, w)| w).sum::<f64>())
        .collect();
    let active: Vec<usize> = (0..n_all).filter(|&i| deg_all[i] > 1e-12).collect();
    let n = active.len();
    if n < 2 || k == 0 {
        return (Vec::new(), Vec::new(), active);
    }
    let mut pos = vec![usize::MAX; n_all];
    for (a, &i) in active.iter().enumerate() {
        pos[i] = a;
    }
    let rows: Vec<Vec<(usize, f64)>> = active
        .iter()
        .map(|&i| ws[i].iter().map(|&(j, w)| (pos[j], w)).collect())
        .collect();
    let d_inv_sqrt: Vec<f64> = active.iter().map(|&i| 1.0 / deg_all[i].sqrt()).collect();

    // Lazy operator: (I + M)/2 · v
    let lazy = |v: &[f64]| -> Vec<f64> {
        let mut r = vec![0.0f64; n];
        for i in 0..n {
            let mut acc = 0.0;
            for &(j, w) in &rows[i] {
                acc += w * d_inv_sqrt[j] * v[j];
            }
            r[i] = 0.5 * (v[i] + d_inv_sqrt[i] * acc);
        }
        r
    };

    // Trivial eigenvector D^{1/2}·1 (unit).
    let mut trivial: Vec<f64> = active.iter().map(|&i| deg_all[i].sqrt()).collect();
    let tn = vec_norm(&trivial);
    for x in &mut trivial {
        *x /= tn;
    }

    // Block size: k wanted + oversampling, at most n − 1 (trivial excluded).
    let want = k.min(n - 1);
    let q = (want + 3).min(n - 1);

    // Deterministic starting block (golden-ratio sequences, one per column).
    let mut x: Vec<Vec<f64>> = (0..q)
        .map(|c| {
            (0..n)
                .map(|i| {
                    let t =
                        (i as f64 + 1.0) * 0.618033988749895 * (c as f64 + 1.0) + 0.377 * c as f64;
                    t - t.floor() - 0.5
                })
                .collect()
        })
        .collect();
    orthonormalize(&mut x, &trivial);

    let mut ritz_prev = vec![f64::NAN; q];
    let mut ritz_vals = vec![0.0; q];
    for _ in 0..3000 {
        let mut y: Vec<Vec<f64>> = x.iter().map(|v| lazy(v)).collect();
        orthonormalize(&mut y, &trivial);

        // Rayleigh–Ritz: H = Yᵀ A Y (q×q), then rotate Y by its eigenvectors.
        let ay: Vec<Vec<f64>> = y.iter().map(|v| lazy(v)).collect();
        let mut h = vec![vec![0.0; q]; q];
        for a in 0..q {
            for b in 0..q {
                h[a][b] = dot(&y[a], &ay[b]);
            }
        }
        let (vals, vecs) = jacobi_eigen(h); // sorted descending
        let mut rotated: Vec<Vec<f64>> = vec![vec![0.0; n]; q];
        for (r, rv) in rotated.iter_mut().enumerate() {
            for (b, yb) in y.iter().enumerate() {
                let c = vecs[b][r];
                for i in 0..n {
                    rv[i] += c * yb[i];
                }
            }
        }
        x = rotated;
        ritz_vals = vals;
        let delta = ritz_vals
            .iter()
            .zip(ritz_prev.iter())
            .take(want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        if delta < 1e-13 && ritz_prev[0].is_finite() {
            break;
        }
        ritz_prev.clone_from(&ritz_vals);
    }

    // Eigenvalue of M from the lazy one: λ_M = 2·λ_lazy − 1.
    let values: Vec<f64> = ritz_vals
        .iter()
        .take(want)
        .map(|&l| 2.0 * l - 1.0)
        .collect();
    let vectors: Vec<Vec<f64>> = x.into_iter().take(want).collect();
    (values, vectors, active)
}

/// Orthogonalise `block` against `against` (unit), then orthonormalise it
/// (modified Gram–Schmidt, twice). A column that collapses is replaced by a
/// deterministic fallback vector.
fn orthonormalize(block: &mut [Vec<f64>], against: &[f64]) {
    let n = against.len();
    for c in 0..block.len() {
        for _pass in 0..2 {
            let p = dot(&block[c], against);
            for i in 0..n {
                block[c][i] -= p * against[i];
            }
            for b in 0..c {
                let (head, tail) = block.split_at_mut(c);
                let p = dot(&tail[0], &head[b]);
                for i in 0..n {
                    tail[0][i] -= p * head[b][i];
                }
            }
        }
        let mut nrm = vec_norm(&block[c]);
        if nrm < 1e-12 {
            // Deterministic replacement, re-orthogonalised once.
            for i in 0..n {
                block[c][i] = (((i * 2654435761 + c * 40503 + 1) % 1000) as f64) / 1000.0 - 0.5;
            }
            let p = dot(&block[c], against);
            for i in 0..n {
                block[c][i] -= p * against[i];
            }
            for b in 0..c {
                let (head, tail) = block.split_at_mut(c);
                let p = dot(&tail[0], &head[b]);
                for i in 0..n {
                    tail[0][i] -= p * head[b][i];
                }
            }
            nrm = vec_norm(&block[c]).max(1e-12);
        }
        for x in &mut block[c] {
            *x /= nrm;
        }
    }
}

/// Cyclic Jacobi eigen-decomposition of a small symmetric matrix.
/// Returns eigenvalues sorted descending and `vecs[row][col]` where column
/// `col` is the eigenvector of the `col`-th eigenvalue.
#[allow(clippy::many_single_char_names)] // notation matricielle : a, v, n (matrice, vecteurs, taille)
fn jacobi_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = a.len();
    let mut v = vec![vec![0.0; n]; n];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _sweep in 0..100 {
        let mut off = 0.0;
        for p in 0..n {
            for q in (p + 1)..n {
                off += a[p][q] * a[p][q];
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for r in 0..n {
                    let (arp, arq) = (a[r][p], a[r][q]);
                    a[r][p] = c * arp - s * arq;
                    a[r][q] = s * arp + c * arq;
                }
                for r in 0..n {
                    let (apr, aqr) = (a[p][r], a[q][r]);
                    a[p][r] = c * apr - s * aqr;
                    a[q][r] = s * apr + c * aqr;
                }
                for r in 0..n {
                    let (vrp, vrq) = (v[r][p], v[r][q]);
                    v[r][p] = c * vrp - s * vrq;
                    v[r][q] = s * vrp + c * vrq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| {
        a[j][j]
            .partial_cmp(&a[i][i])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let vals: Vec<f64> = order.iter().map(|&i| a[i][i]).collect();
    let mut vecs = vec![vec![0.0; n]; n];
    for (newc, &oldc) in order.iter().enumerate() {
        for r in 0..n {
            vecs[r][newc] = v[r][oldc];
        }
    }
    (vals, vecs)
}

/// Dot product of two vectors.
#[inline]
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Euclidean norm of a vector.
#[inline]
fn vec_norm(v: &[f64]) -> f64 {
    dot(v, v).sqrt()
}

/// Normalize a vector to [0, 1] range.
fn normalize_to_unit(v: &[f64]) -> Vec<f64> {
    if v.is_empty() {
        return Vec::new();
    }
    let min = v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    if range < 1e-12 {
        return vec![0.5; v.len()];
    }
    v.iter().map(|&x| (x - min) / range).collect()
}

// ============================================================================
// GraphAlgorithm trait implementation
// ============================================================================

/// Spectral embedding algorithm wrapper for registry integration.
pub struct SpectralEmbeddingAlgorithm;

static SPECTRAL_PARAMS: OnceLock<Vec<ParameterDef>> = OnceLock::new();

fn spectral_params() -> &'static [ParameterDef] {
    SPECTRAL_PARAMS.get_or_init(|| {
        vec![
            ParameterDef {
                name: "dimensions".to_string(),
                description: "Number of embedding dimensions".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("2".to_string()),
            },
            ParameterDef {
                name: "weight_property".to_string(),
                description: "Edge property name to use as weight".to_string(),
                param_type: ParameterType::String,
                required: false,
                default: None,
            },
        ]
    })
}

impl GraphAlgorithm for SpectralEmbeddingAlgorithm {
    fn name(&self) -> &str {
        "obrain.spectral_embedding"
    }

    fn description(&self) -> &str {
        "Spectral embedding via normalized Laplacian eigenvectors"
    }

    fn parameters(&self) -> &[ParameterDef] {
        spectral_params()
    }

    fn execute(&self, store: &dyn GraphStore, params: &Parameters) -> Result<AlgorithmResult> {
        let dimensions = params.get_int("dimensions").unwrap_or(2) as usize;

        let weight_prop_owned = params.get_string("weight_property").map(|s| s.to_string());
        let weight_property = weight_prop_owned.as_deref();

        let result = spectral_embedding(store, dimensions, weight_property);

        // Build result: node_id, embedding (as list of floats)
        let mut algo_result =
            AlgorithmResult::new(vec!["node_id".to_string(), "embedding".to_string()]);

        for (node_id, embedding) in &result.embeddings {
            let embedding_val = Value::List(embedding.iter().map(|&x| Value::Float64(x)).collect());
            algo_result.add_row(vec![Value::Int64(node_id.0 as i64), embedding_val]);
        }

        Ok(algo_result)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_star_graph(n: usize) -> SubstrateStore {
        let store = SubstrateStore::open_tempfile().unwrap();
        let center = store.create_node(&["Center"]);
        for _ in 1..n {
            let leaf = store.create_node(&["Leaf"]);
            store.create_edge(center, leaf, "CONNECTS");
            store.create_edge(leaf, center, "CONNECTS");
        }
        store
    }

    fn make_ring_graph(n: usize) -> SubstrateStore {
        let store = SubstrateStore::open_tempfile().unwrap();
        let nodes: Vec<NodeId> = (0..n).map(|_| store.create_node(&["Node"])).collect();
        for i in 0..n {
            let j = (i + 1) % n;
            store.create_edge(nodes[i], nodes[j], "CONNECTS");
            store.create_edge(nodes[j], nodes[i], "CONNECTS");
        }
        store
    }

    fn make_grid_graph(rows: usize, cols: usize) -> SubstrateStore {
        let store = SubstrateStore::open_tempfile().unwrap();
        let mut nodes = Vec::new();
        for _ in 0..rows {
            let mut row = Vec::new();
            for _ in 0..cols {
                row.push(store.create_node(&["Node"]));
            }
            nodes.push(row);
        }
        for r in 0..rows {
            for c in 0..cols {
                if c + 1 < cols {
                    store.create_edge(nodes[r][c], nodes[r][c + 1], "CONNECTS");
                    store.create_edge(nodes[r][c + 1], nodes[r][c], "CONNECTS");
                }
                if r + 1 < rows {
                    store.create_edge(nodes[r][c], nodes[r + 1][c], "CONNECTS");
                    store.create_edge(nodes[r + 1][c], nodes[r][c], "CONNECTS");
                }
            }
        }
        store
    }

    #[test]
    fn test_spectral_star_dim2() {
        let store = make_star_graph(6); // center + 5 leaves
        let result = spectral_embedding(&store, 2, None);
        assert_eq!(result.dimensions, 2);
        assert_eq!(result.embeddings.len(), 6);
        for emb in result.embeddings.values() {
            assert_eq!(emb.len(), 2);
            for &v in emb {
                assert!((0.0..=1.0).contains(&v), "value {v} not in [0,1]");
            }
        }
    }

    #[test]
    fn test_spectral_ring_dim4() {
        let store = make_ring_graph(10);
        let result = spectral_embedding(&store, 4, None);
        assert_eq!(result.dimensions, 4);
        assert_eq!(result.embeddings.len(), 10);
        for emb in result.embeddings.values() {
            assert_eq!(emb.len(), 4);
        }
    }

    #[test]
    fn test_spectral_grid_dim2() {
        let store = make_grid_graph(4, 4);
        let result = spectral_embedding(&store, 2, None);
        assert_eq!(result.dimensions, 2);
        assert_eq!(result.embeddings.len(), 16);
    }

    #[test]
    fn test_spectral_single_node() {
        let store = SubstrateStore::open_tempfile().unwrap();
        store.create_node(&["Alone"]);
        let result = spectral_embedding(&store, 2, None);
        assert_eq!(result.embeddings.len(), 1);
        for emb in result.embeddings.values() {
            assert_eq!(emb, &vec![0.5, 0.5]);
        }
    }

    #[test]
    fn test_spectral_two_nodes() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let a = store.create_node(&["A"]);
        let b = store.create_node(&["B"]);
        store.create_edge(a, b, "LINK");
        store.create_edge(b, a, "LINK");
        let result = spectral_embedding(&store, 2, None);
        assert_eq!(result.embeddings.len(), 2);
    }

    #[test]
    fn test_spectral_determinism() {
        let store = make_star_graph(8);
        let r1 = spectral_embedding(&store, 4, None);
        let r2 = spectral_embedding(&store, 4, None);
        for (nid, emb1) in &r1.embeddings {
            let emb2 = r2.embeddings.get(nid).unwrap();
            for (a, b) in emb1.iter().zip(emb2.iter()) {
                assert!((a - b).abs() < 1e-10, "Non-deterministic: {a} vs {b}");
            }
        }
    }

    #[test]
    fn test_spectral_higher_dims() {
        let store = make_ring_graph(20);
        let result = spectral_embedding(&store, 8, None);
        assert_eq!(result.dimensions, 8);
        for emb in result.embeddings.values() {
            assert_eq!(emb.len(), 8);
            for &v in emb {
                assert!((0.0..=1.0).contains(&v));
            }
        }
    }

    #[test]
    fn test_spectral_algorithm_trait() {
        let store = make_star_graph(5);
        let algo = SpectralEmbeddingAlgorithm;
        assert_eq!(algo.name(), "obrain.spectral_embedding");

        let mut params = Parameters::new();
        params.set_int("dimensions", 4);
        let result = algo.execute(&store, &params).unwrap();
        assert_eq!(result.columns, vec!["node_id", "embedding"]);
        assert_eq!(result.rows.len(), 5);
    }

    // -----------------------------------------------------------------
    // #289 — oracles spectraux (sans store : liste d'adjacence dirigée).
    // -----------------------------------------------------------------

    /// Adjacence dirigée → W_s = (W + Wᵀ)/2 sous forme de liste de listes.
    fn sym(adj: &[Vec<(usize, f64)>]) -> Vec<Vec<(usize, f64)>> {
        let mut out = vec![Vec::new(); adj.len()];
        for (i, row) in adj.iter().enumerate() {
            for &(j, w) in row {
                out[i].push((j, w / 2.0));
                out[j].push((i, w / 2.0));
            }
        }
        out
    }

    /// Retrouve le vecteur propre brut (avant `normalize_to_unit`, affine)
    /// grâce à l'orthogonalité avec D^{1/2}·1, puis rend son quotient de
    /// Rayleigh pour M = D^{-1/2} W_s D^{-1/2}, restreint aux `actifs`.
    fn rayleigh(adj: &[Vec<(usize, f64)>], x: &[f64], actifs: &[usize]) -> f64 {
        let ws = sym(adj);
        let deg: Vec<f64> = ws.iter().map(|r| r.iter().map(|&(_, w)| w).sum()).collect();
        let sq: Vec<f64> = deg.iter().map(|d| d.sqrt()).collect();
        let c = actifs.iter().map(|&i| sq[i] * x[i]).sum::<f64>()
            / actifs.iter().map(|&i| sq[i]).sum::<f64>();
        let mut v = vec![0.0; adj.len()];
        for &i in actifs {
            v[i] = x[i] - c;
        }
        let mut num = 0.0;
        for &i in actifs {
            for &(j, w) in &ws[i] {
                num += v[i] * w * v[j] / (sq[i] * sq[j]);
            }
        }
        num / v.iter().map(|a| a * a).sum::<f64>()
    }

    fn ring_adj(n: usize) -> Vec<Vec<(usize, f64)>> {
        (0..n)
            .map(|i| vec![((i + 1) % n, 1.0), ((i + n - 1) % n, 1.0)])
            .collect()
    }

    /// Anneau pair (biparti) : λ(M) = −1 existe, l'itération non décalée la
    /// choisissait. Les deux premiers non triviaux valent cos(2π/10) (mult. 2).
    #[test]
    fn spectral_anneau_10_donne_les_vecteurs_de_fiedler() {
        let adj = ring_adj(10);
        let evs = spectral_eigenvectors(&adj, 2);
        let tous: Vec<usize> = (0..10).collect();
        let attendu = (2.0 * std::f64::consts::PI / 10.0).cos();
        for (d, ev) in evs.iter().enumerate() {
            let r = rayleigh(&adj, ev, &tous);
            assert!(
                (r - attendu).abs() < 1e-3,
                "dim {d}: Rayleigh = {r}, attendu cos(2π/10) = {attendu}"
            );
        }
    }

    #[test]
    fn spectral_anneau_11_donne_le_vecteur_de_fiedler() {
        let adj = ring_adj(11);
        let evs = spectral_eigenvectors(&adj, 1);
        let tous: Vec<usize> = (0..11).collect();
        let attendu = (2.0 * std::f64::consts::PI / 11.0).cos();
        let r = rayleigh(&adj, &evs[0], &tous);
        assert!(
            (r - attendu).abs() < 1e-3,
            "Rayleigh = {r}, attendu {attendu}"
        );
    }

    /// Deux K4 reliés par un pont orienté a→b, plus un puits `s` (aucune
    /// arête sortante, une entrante). Avant : un seul puits rendait tout
    /// constant. Après : le signe du vecteur de Fiedler sépare les cliques.
    #[test]
    fn spectral_un_puits_ne_rend_pas_tout_constant() {
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); 9];
        for base in [0usize, 4] {
            for i in 0..4 {
                for j in 0..4 {
                    if i != j {
                        adj[base + i].push((base + j, 1.0));
                    }
                }
            }
        }
        adj[3].push((4, 1.0)); // pont orienté
        adj[0].push((8, 1.0)); // 8 = puits
        let evs = spectral_eigenvectors(&adj, 1);
        let ev = &evs[0];
        assert!(ev.iter().all(|x| x.is_finite()));
        let actifs: Vec<usize> = (0..8).collect();
        let ws = sym(&adj);
        let sq: Vec<f64> = ws
            .iter()
            .map(|r| r.iter().map(|&(_, w)| w).sum::<f64>().sqrt())
            .collect();
        let tous: Vec<usize> = (0..9).collect();
        let c = tous.iter().map(|&i| sq[i] * ev[i]).sum::<f64>()
            / tous.iter().map(|&i| sq[i]).sum::<f64>();
        let signe = |i: usize| (ev[i] - c) > 0.0;
        assert!(
            ev.iter().any(|&x| (x - ev[0]).abs() > 1e-3),
            "plongement constant"
        );
        assert!(
            (0..4).all(|i| signe(i) == signe(0)),
            "clique A non homogène"
        );
        assert!(
            (4..8).all(|i| signe(i) == signe(4)),
            "clique B non homogène"
        );
        assert_ne!(signe(0), signe(4), "le signe doit séparer les deux cliques");
        let _ = actifs;
    }

    /// Valeurs propres de M (convention : λ(L_norm) = 1 − λ(M)) : sur C₁₀ les
    /// deux premières valent cos(2π/10) (multiplicité 2), pas cos(4π/10).
    #[test]
    fn spectral_valeurs_propres_anneau_10_multiplicite_deux() {
        let (vals, vecs, active) = spectral_eigenpairs(&ring_adj(10), 4);
        assert_eq!(active.len(), 10);
        assert_eq!(vecs.len(), 4);
        let l1 = (2.0 * std::f64::consts::PI / 10.0).cos();
        let l2 = (4.0 * std::f64::consts::PI / 10.0).cos();
        assert!((vals[0] - l1).abs() < 1e-8, "{vals:?}");
        assert!((vals[1] - l1).abs() < 1e-8, "{vals:?}");
        assert!((vals[2] - l2).abs() < 1e-8, "{vals:?}");
        assert!((vals[3] - l2).abs() < 1e-8, "{vals:?}");
    }
}
