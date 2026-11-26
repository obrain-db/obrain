//! SpectralEncoder — converts an engram's node ensemble into a compact spectral
//! signature for similarity search.
//!
//! The spectral signature is a fixed-dimensional float vector that captures the
//! "shape" of a node group. It is used with [`VectorIndex`] for nearest-neighbor
//! retrieval during recall and warm-up selection.
//!
//! # Extensibility
//! Users can provide a custom encoder by implementing [`SpectralEncoderTrait`]
//! and passing it to [`SpectralEncoder::with_encoder`].

use obrain_common::types::NodeId;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// Trait — pluggable encoding strategy
// ---------------------------------------------------------------------------

/// Trait for converting a weighted node ensemble into a spectral signature.
///
/// Implementations must be `Send + Sync` so the encoder can be shared across
/// threads (e.g., inside an `Arc<SpectralEncoder>`).
pub trait SpectralEncoderTrait: Send + Sync + std::fmt::Debug {
    /// Encode a slice of (NodeId, weight) pairs into a fixed-length vector.
    ///
    /// The output vector should be L2-normalized.
    fn encode(&self, nodes: &[(NodeId, f64)]) -> Vec<f64>;

    /// The number of dimensions in the output vector.
    fn dimensions(&self) -> usize;
}

// ---------------------------------------------------------------------------
// DefaultSpectralEncoder — hash-based projection
// ---------------------------------------------------------------------------

/// Default spectral encoder that projects node IDs into a fixed-dimensional
/// space using a deterministic hashing scheme.
///
/// Each node is hashed to determine which dimensions it contributes to, and
/// its contribution is weighted by its ensemble weight. The final vector is
/// L2-normalized.
#[derive(Debug, Clone)]
pub struct DefaultSpectralEncoder {
    dims: usize,
}

impl DefaultSpectralEncoder {
    /// Creates a new default encoder with the given number of dimensions.
    #[allow(dead_code)]
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

impl Default for DefaultSpectralEncoder {
    fn default() -> Self {
        Self { dims: 64 }
    }
}

impl SpectralEncoderTrait for DefaultSpectralEncoder {
    fn encode(&self, nodes: &[(NodeId, f64)]) -> Vec<f64> {
        let mut signature = vec![0.0f64; self.dims];

        if nodes.is_empty() {
            return signature;
        }

        for &(node_id, weight) in nodes {
            // Hash the NodeId to get a deterministic spread across dimensions.
            let mut hasher = DefaultHasher::new();
            node_id.0.hash(&mut hasher);
            let hash = hasher.finish();

            // Use different parts of the hash to select multiple dimensions,
            // giving each node a richer fingerprint.
            for sub in 0..4u64 {
                let mut sub_hasher = DefaultHasher::new();
                (hash, sub).hash(&mut sub_hasher);
                let sub_hash = sub_hasher.finish();

                let dim = (sub_hash as usize) % self.dims;

                // Alternate sign based on another bit to allow cancellation,
                // producing a more expressive projection.
                let sign = if (sub_hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };

                signature[dim] += sign * weight;
            }
        }

        // L2-normalize the signature.
        l2_normalize(&mut signature);

        signature
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

// ---------------------------------------------------------------------------
// SpectralEncoder — wrapper with dynamic dispatch
// ---------------------------------------------------------------------------

/// A spectral encoder that delegates to a `SpectralEncoderTrait` implementation.
///
/// By default it uses `DefaultSpectralEncoder` with 64 dimensions. Users can
/// swap in a custom encoder via [`SpectralEncoder::with_encoder`].
#[derive(Debug)]
pub struct SpectralEncoder {
    inner: Box<dyn SpectralEncoderTrait>,
}

impl SpectralEncoder {
    /// Creates a new `SpectralEncoder` with the default hash-based strategy (64 dims).
    pub fn new() -> Self {
        Self {
            inner: Box::new(DefaultSpectralEncoder::default()),
        }
    }

    /// Creates a `SpectralEncoder` backed by a custom encoder.
    pub fn with_encoder(encoder: Box<dyn SpectralEncoderTrait>) -> Self {
        Self { inner: encoder }
    }

    /// Encode a weighted node ensemble into a spectral signature vector.
    pub fn encode(&self, nodes: &[(NodeId, f64)]) -> Vec<f64> {
        self.inner.encode(nodes)
    }

    /// Returns the dimensionality of the output vectors.
    pub fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

impl Default for SpectralEncoder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// L2-normalize a vector in place. If the norm is zero the vector is left unchanged.
fn l2_normalize(v: &mut [f64]) {
    let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_encoder_produces_correct_dimensions() {
        let enc = SpectralEncoder::new();
        assert_eq!(enc.dimensions(), 64);

        let sig = enc.encode(&[(NodeId(1), 1.0), (NodeId(2), 0.5)]);
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn output_is_l2_normalized() {
        let enc = SpectralEncoder::new();
        let sig = enc.encode(&[(NodeId(42), 1.0), (NodeId(99), 0.7), (NodeId(3), 0.3)]);

        let norm: f64 = sig.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-10,
            "expected unit norm, got {}",
            norm
        );
    }

    #[test]
    fn empty_input_returns_zero_vector() {
        let enc = SpectralEncoder::new();
        let sig = enc.encode(&[]);
        assert!(sig.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn deterministic_encoding() {
        let enc = SpectralEncoder::new();
        let nodes = vec![(NodeId(10), 1.0), (NodeId(20), 0.5)];

        let sig1 = enc.encode(&nodes);
        let sig2 = enc.encode(&nodes);
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn different_nodes_produce_different_signatures() {
        let enc = SpectralEncoder::new();
        let sig_a = enc.encode(&[(NodeId(1), 1.0)]);
        let sig_b = enc.encode(&[(NodeId(999), 1.0)]);
        assert_ne!(sig_a, sig_b);
    }

    #[test]
    fn custom_encoder_works() {
        #[derive(Debug)]
        struct TinyEncoder;
        impl SpectralEncoderTrait for TinyEncoder {
            fn encode(&self, nodes: &[(NodeId, f64)]) -> Vec<f64> {
                vec![nodes.len() as f64]
            }
            fn dimensions(&self) -> usize {
                1
            }
        }

        let enc = SpectralEncoder::with_encoder(Box::new(TinyEncoder));
        assert_eq!(enc.dimensions(), 1);
        assert_eq!(enc.encode(&[(NodeId(1), 1.0), (NodeId(2), 0.5)]), vec![2.0]);
    }

    #[test]
    fn custom_dimensions() {
        let enc = SpectralEncoder::with_encoder(Box::new(DefaultSpectralEncoder::new(128)));
        assert_eq!(enc.dimensions(), 128);

        let sig = enc.encode(&[(NodeId(1), 1.0)]);
        assert_eq!(sig.len(), 128);
    }
}

#[cfg(test)]
mod dimension_comparison {
    use super::*;

    fn l2(x: &[f64], y: &[f64]) -> f64 {
        x.iter()
            .zip(y)
            .map(|(u, v)| (u - v).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    /// Comparaison 32/64/128/256 dims sur le même corpus (nombre de paires
    /// CONSTANT = 200, ensembles disjoints de taille 2).
    #[test]
    fn discrimination_par_dimension() {
        let n_pairs = 200usize;
        for dims in [32usize, 64, 128, 256] {
            let enc = DefaultSpectralEncoder::new(dims);
            let mut min_d = f64::INFINITY;
            let mut sum = 0.0f64;
            for p in 0..n_pairs {
                let base = (p * 4) as u64 + 1;
                let e1 = [(NodeId(base), 1.0), (NodeId(base + 1), 1.0)];
                let e2 = [(NodeId(base + 2), 1.0), (NodeId(base + 3), 1.0)];
                let d = l2(&enc.encode(&e1), &enc.encode(&e2));
                if d < min_d {
                    min_d = d;
                }
                sum += d;
            }
            println!(
                "dims={}: L2 min={:.6} moyenne={:.6} ({} paires disjointes taille 2)",
                dims,
                min_d,
                sum / n_pairs as f64,
                n_pairs
            );
        }
    }
}

#[cfg(test)]
mod ranking_fidelity {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Référence EXACTE de similarité structurelle pondérée : cosinus des
    /// vecteurs de poids (sparse) sur l'univers des nœuds.
    fn ref_cosine(a: &[(NodeId, f64)], b: &[(NodeId, f64)]) -> f64 {
        let ma: HashMap<NodeId, f64> = a.iter().copied().collect();
        let mb: HashMap<NodeId, f64> = b.iter().copied().collect();
        let dot: f64 = ma.iter().map(|(k, v)| v * mb.get(k).unwrap_or(&0.0)).sum();
        let na = ma.values().map(|v| v * v).sum::<f64>().sqrt();
        let nb = mb.values().map(|v| v * v).sum::<f64>().sqrt();
        if na == 0.0 || nb == 0.0 {
            return 0.0;
        }
        dot / (na * nb)
    }

    fn cosine(x: &[f64], y: &[f64]) -> f64 {
        let dot: f64 = x.iter().zip(y).map(|(u, v)| u * v).sum();
        let nx = x.iter().map(|u| u * u).sum::<f64>().sqrt();
        let ny = y.iter().map(|u| u * u).sum::<f64>().sqrt();
        if nx == 0.0 || ny == 0.0 {
            return 0.0;
        }
        dot / (nx * ny)
    }

    /// Fidélité du classement : Recall@1 = fraction d'ensembles dont le plus
    /// proche voisin (référence exacte) est aussi le plus proche en projection.
    #[test]
    fn fidelite_classement_par_dimension() {
        let mut rng = 0x9E3779B97F4A7C15u64;
        let mut next = || {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as usize
        };
        let mut corpus: Vec<Vec<(NodeId, f64)>> = Vec::new();
        for _ in 0..40 {
            let size = 2 + next() % 7; // 2..8 membres
            let mut ens = Vec::new();
            let mut seen = HashSet::new();
            while ens.len() < size {
                let n = NodeId((next() % 60 + 1) as u64);
                if seen.insert(n) {
                    ens.push((n, 0.5 + (next() % 11) as f64 * 0.1)); // 0.5..1.5
                }
            }
            corpus.push(ens);
        }

        for dims in [32usize, 64, 128, 256] {
            let enc = DefaultSpectralEncoder::new(dims);
            let sigs: Vec<Vec<f64>> = corpus.iter().map(|e| enc.encode(e)).collect();
            let mut recall1 = 0usize;
            for i in 0..corpus.len() {
                let (mut tn, mut ts) = (usize::MAX, f64::NEG_INFINITY);
                for j in 0..corpus.len() {
                    if i == j {
                        continue;
                    }
                    let s = ref_cosine(&corpus[i], &corpus[j]);
                    if s > ts {
                        (ts, tn) = (s, j);
                    }
                }
                let (mut sn, mut ss) = (usize::MAX, f64::NEG_INFINITY);
                for j in 0..corpus.len() {
                    if i == j {
                        continue;
                    }
                    let s = cosine(&sigs[i], &sigs[j]);
                    if s > ss {
                        (ss, sn) = (s, j);
                    }
                }
                if sn == tn {
                    recall1 += 1;
                }
            }
            println!(
                "dims={}: Recall@1 = {}/{} = {:.3}",
                dims,
                recall1,
                corpus.len(),
                recall1 as f64 / corpus.len() as f64
            );
        }
    }
}

#[cfg(test)]
mod hybrid_recall {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn ref_cosine(a: &[(NodeId, f64)], b: &[(NodeId, f64)]) -> f64 {
        let ma: HashMap<NodeId, f64> = a.iter().copied().collect();
        let mb: HashMap<NodeId, f64> = b.iter().copied().collect();
        let dot: f64 = ma.iter().map(|(k, v)| v * mb.get(k).unwrap_or(&0.0)).sum();
        let na = ma.values().map(|v| v * v).sum::<f64>().sqrt();
        let nb = mb.values().map(|v| v * v).sum::<f64>().sqrt();
        if na == 0.0 || nb == 0.0 {
            return 0.0;
        }
        dot / (na * nb)
    }
    fn cosine(x: &[f64], y: &[f64]) -> f64 {
        let dot: f64 = x.iter().zip(y).map(|(u, v)| u * v).sum();
        let nx = x.iter().map(|u| u * u).sum::<f64>().sqrt();
        let ny = y.iter().map(|u| u * u).sum::<f64>().sqrt();
        if nx == 0.0 || ny == 0.0 {
            return 0.0;
        }
        dot / (nx * ny)
    }

    /// Probabilité que le vrai plus proche voisin (cosinus EXACT) soit présent
    /// dans les k premiers candidats projetés — mesure la qualité du filtrage
    /// candidat (rappel hybride), pour k=1/5/10 et dims=32/64/128/256.
    #[test]
    fn rappel_hybride_candidats() {
        let mut rng = 0x9E3779B97F4A7C15u64;
        let mut next = || {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as usize
        };
        // 80 ensembles sur 120 nœuds, tailles 3..10, poids 0.5..1.5.
        let mut corpus: Vec<Vec<(NodeId, f64)>> = Vec::new();
        for _ in 0..80 {
            let size = 3 + next() % 8;
            let mut ens = Vec::new();
            let mut seen = HashSet::new();
            while ens.len() < size {
                let n = NodeId((next() % 120 + 1) as u64);
                if seen.insert(n) {
                    ens.push((n, 0.5 + (next() % 11) as f64 * 0.1));
                }
            }
            corpus.push(ens);
        }

        for dims in [32usize, 64, 128, 256] {
            let enc = DefaultSpectralEncoder::new(dims);
            let sigs: Vec<Vec<f64>> = corpus.iter().map(|e| enc.encode(e)).collect();
            for k in [1usize, 5, 10] {
                let mut hit = 0usize;
                for i in 0..corpus.len() {
                    // vrai plus proche voisin (exact).
                    let (mut tn, mut ts) = (usize::MAX, f64::NEG_INFINITY);
                    for j in 0..corpus.len() {
                        if i == j {
                            continue;
                        }
                        let c = ref_cosine(&corpus[i], &corpus[j]);
                        if c > ts {
                            (ts, tn) = (c, j);
                        }
                    }
                    // top-k candidats projetés (cosinus de signature), tie-break déterministe.
                    let mut cands: Vec<(f64, usize)> = (0..corpus.len())
                        .filter(|&j| j != i)
                        .map(|j| (cosine(&sigs[i], &sigs[j]), j))
                        .collect();
                    cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
                    if cands.iter().take(k).any(|(_, j)| *j == tn) {
                        hit += 1;
                    }
                }
                println!(
                    "dims={} k={}: Recall@k(candidat) = {}/{} = {:.3}",
                    dims,
                    k,
                    hit,
                    corpus.len(),
                    hit as f64 / corpus.len() as f64
                );
            }
        }
    }
}
