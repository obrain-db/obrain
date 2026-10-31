//! S5 step 0 — TurboQuant spike, GO/NO-GO gate (task f14636a1, plan
//! obrain-spectral 1dc957f5, decision d482ec5c).
//!
//! QUESTION THE SPIKE ANSWERS: does TurboQuant-style online quantization
//! (fixed random rotation + per-coordinate scalar quantization) preserve
//! the inner product well enough that connectivity clustering over the
//! quantized vectors is IDENTICAL to clustering over f32? If not, S5 is
//! rejected here and no blackboard/Laplacian work starts.
//!
//! WHAT IS MEASURED (hors-prod, everything lives in this test):
//!  1. Inner-product fidelity: distribution (p50/p99) of the cosine-scale
//!     error between exact f32 dot products and quantized estimates, on
//!     real `hilbert_features` (64d) and `spectral_embedding` (4d)
//!     vectors computed from a clustered synthetic graph.
//!  2. Clustering invariance: an S2-STYLE clustering (cosine-threshold
//!     edges + union-find connected components — the exact method the
//!     nightly generalization pass uses) run on f32 vs int8-quantized
//!     vectors must produce IDENTICAL partitions.
//!
//! HONEST SCOPE NOTES: (a) the operator's verification mentions replaying
//! the real S2 lesson families and the membench corpus — the real S2
//! replay needs tmn data and belongs to step 1 (integration); this spike
//! uses the same *method* on in-repo vectors, which is the strongest
//! in-repo proxy. (b) The paper's theoretical distortion bound is not
//! recomputed here; the gate uses concrete practical thresholds (p99
//! cosine-scale error ≤ 0.05 for int8) which are far below anything that
//! could flip a 0.85-threshold clustering edge.
#![cfg(feature = "algos")]

use std::collections::HashMap;

use obrain_adapters::plugins::algorithms::{
    HilbertFeaturesConfig, hilbert_features, spectral_embedding,
};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::fmt::Write;

// ─── Deterministic RNG (no external dep, no wall clock) ────────────────

struct XorShift(u64);
impl XorShift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_sign(&mut self) -> f32 {
        if self.next_u64() & 1 == 0 { 1.0 } else { -1.0 }
    }
}

// ─── TurboQuant-lite quantizer (spike implementation) ──────────────────

/// Fast Walsh–Hadamard transform, in place. `data.len()` must be a power
/// of two. Orthogonal up to the 1/√n normalisation applied at the end, so
/// exact dot products are preserved BEFORE quantization — all measured
/// error is attributable to the scalar quantization step.
fn fwht(data: &mut [f32]) {
    let n = data.len();
    debug_assert!(n.is_power_of_two());
    let mut h = 1;
    while h < n {
        for i in (0..n).step_by(h * 2) {
            for j in i..i + h {
                let (x, y) = (data[j], data[j + h]);
                data[j] = x + y;
                data[j + h] = x - y;
            }
        }
        h *= 2;
    }
    let norm = 1.0 / (n as f32).sqrt();
    for v in data.iter_mut() {
        *v *= norm;
    }
}

/// A quantized vector: int8 codes + per-vector scale.
struct QVec {
    codes: Vec<i8>,
    scale: f32,
}

/// TurboQuant-lite: random-sign flip (seeded, shared across all vectors)
/// + FWHT rotation + per-vector int8 scalar quantization.
struct Quantizer {
    signs: Vec<f32>,
    dim: usize, // padded power-of-two dimension
}

impl Quantizer {
    fn new(input_dim: usize, seed: u64) -> Self {
        let dim = input_dim.next_power_of_two();
        let mut rng = XorShift(seed | 1);
        let signs = (0..dim).map(|_| rng.next_sign()).collect();
        Self { signs, dim }
    }

    /// Rotate (sign-flip + FWHT) without quantizing — used to verify the
    /// rotation itself is dot-product-preserving.
    fn rotate(&self, v: &[f32]) -> Vec<f32> {
        let mut buf = vec![0.0f32; self.dim];
        buf[..v.len()].copy_from_slice(v);
        for (b, s) in buf.iter_mut().zip(&self.signs) {
            *b *= s;
        }
        fwht(&mut buf);
        buf
    }

    fn quantize(&self, v: &[f32]) -> QVec {
        let rotated = self.rotate(v);
        let max_abs = rotated.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let scale = if max_abs > 0.0 { max_abs / 127.0 } else { 1.0 };
        let codes = rotated
            .iter()
            .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8)
            .collect();
        QVec { codes, scale }
    }
}

fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn dot_quantized(a: &QVec, b: &QVec) -> f32 {
    let acc: i32 = a
        .codes
        .iter()
        .zip(&b.codes)
        .map(|(&x, &y)| x as i32 * y as i32)
        .sum();
    acc as f32 * a.scale * b.scale
}

fn norm(v: &[f32]) -> f32 {
    dot_f32(v, v).sqrt()
}

// ─── S2-style clustering: cosine-threshold edges + union-find ──────────

struct UnionFind(Vec<usize>);
impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn find(&mut self, i: usize) -> usize {
        if self.0[i] != i {
            let r = self.find(self.0[i]);
            self.0[i] = r;
        }
        self.0[i]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra] = rb;
        }
    }
}

/// Canonical partition: for each element, the smallest index in its
/// component — comparable across runs regardless of union order.
fn components(sims: &[Vec<f32>], tau: f32) -> Vec<usize> {
    let n = sims.len();
    let mut uf = UnionFind::new(n);
    for i in 0..n {
        for j in (i + 1)..n {
            if sims[i][j] >= tau {
                uf.union(i, j);
            }
        }
    }
    let mut canon: HashMap<usize, usize> = HashMap::new();
    let mut out = vec![0usize; n];
    for i in 0..n {
        let root = uf.find(i);
        let rep = *canon.entry(root).or_insert(i);
        out[i] = rep;
    }
    out
}

/// Pairwise cosine matrix from a dot-product oracle.
fn cosine_matrix(n: usize, dots: impl Fn(usize, usize) -> f32, norms: &[f32]) -> Vec<Vec<f32>> {
    let mut m = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in 0..n {
            let d = norms[i] * norms[j];
            m[i][j] = if d > 1e-12 { dots(i, j) / d } else { 0.0 };
        }
    }
    m
}

// ─── The clustered synthetic graph ──────────────────────────────────────

/// Three communities of `sz` nodes (dense ring + chords inside, single
/// weak bridge between communities) — a shape where hilbert/spectral
/// features separate clusters by construction.
fn make_clustered_graph(sz: usize) -> SubstrateStore {
    let store = SubstrateStore::open_tempfile().expect("substrate");
    let mut all: Vec<NodeId> = Vec::new();
    for c in 0..3 {
        let label = ["A", "B", "C"][c];
        let base: Vec<NodeId> = (0..sz).map(|_| store.create_node(&[label])).collect();
        for i in 0..sz {
            store.create_edge(base[i], base[(i + 1) % sz], "LINK");
            store.create_edge(base[i], base[(i + 2) % sz], "LINK");
        }
        all.extend(base);
    }
    // Weak bridges so the graph is connected but communities stay distinct.
    store.create_edge(all[0], all[sz], "BRIDGE");
    store.create_edge(all[sz], all[2 * sz], "BRIDGE");
    store
}

// ─── Quantizer self-tests ───────────────────────────────────────────────

#[test]
fn fwht_rotation_preserves_dot_products_exactly() {
    let q = Quantizer::new(64, 0xC0FFEE);
    let mut rng = XorShift(42);
    for _ in 0..50 {
        let a: Vec<f32> = (0..64)
            .map(|_| (rng.next_u64() % 2000) as f32 / 1000.0 - 1.0)
            .collect();
        let b: Vec<f32> = (0..64)
            .map(|_| (rng.next_u64() % 2000) as f32 / 1000.0 - 1.0)
            .collect();
        let exact = dot_f32(&a, &b);
        let rotated = dot_f32(&q.rotate(&a), &q.rotate(&b));
        assert!(
            (exact - rotated).abs() <= 1e-3 * exact.abs().max(1.0),
            "rotation must preserve dot products: {exact} vs {rotated}"
        );
    }
}

#[test]
fn quantizer_roundtrip_error_is_bounded_on_random_vectors() {
    let q = Quantizer::new(64, 0xC0FFEE);
    let mut rng = XorShift(7);
    let mut worst: f32 = 0.0;
    for _ in 0..200 {
        let a: Vec<f32> = (0..64)
            .map(|_| (rng.next_u64() % 2000) as f32 / 1000.0 - 1.0)
            .collect();
        let b: Vec<f32> = (0..64)
            .map(|_| (rng.next_u64() % 2000) as f32 / 1000.0 - 1.0)
            .collect();
        let exact = dot_f32(&a, &b);
        let est = dot_quantized(&q.quantize(&a), &q.quantize(&b));
        // cosine-scale error: normalised by the norms, dimension-free.
        let err = (exact - est).abs() / (norm(&a) * norm(&b)).max(1e-9);
        worst = worst.max(err);
    }
    assert!(
        worst < 0.05,
        "int8 cosine-scale error should stay well under 5%, worst={worst}"
    );
}

// ─── The GO/NO-GO spike ─────────────────────────────────────────────────

#[test]
fn s5_spike_fidelity_and_clustering_invariance() {
    println!("\n=== S5 step-0 spike — TurboQuant-lite fidelity GO/NO-GO ===");
    let store = make_clustered_graph(30); // 90 nodes, 3 communities

    // Real embedding sources.
    let hilbert = hilbert_features(&store, &HilbertFeaturesConfig::default());
    let spectral = spectral_embedding(&store, 4, None);

    let mut report = String::from("{\"bench\":\"s5_turboquant_spike\",\"sources\":[");
    let mut first = true;
    let mut go = true;

    for (name, vecs) in [
        (
            "hilbert_features",
            hilbert
                .features
                .values()
                .cloned()
                .collect::<Vec<Vec<f32>>>(),
        ),
        (
            "spectral_embedding",
            spectral
                .embeddings
                .values()
                .map(|v| v.iter().map(|x| *x as f32).collect())
                .collect::<Vec<Vec<f32>>>(),
        ),
    ] {
        // Drop all-zero vectors (absent-feature sentinel) — they carry no
        // clustering signal and their cosine is undefined.
        let vecs: Vec<Vec<f32>> = vecs
            .into_iter()
            .filter(|v| v.iter().any(|x| *x != 0.0))
            .collect();
        let n = vecs.len();
        assert!(n >= 30, "{name}: expected ≥30 usable vectors, got {n}");
        let dim = vecs[0].len();
        let q = Quantizer::new(dim, 0x0B5A17);
        let quantized: Vec<QVec> = vecs.iter().map(|v| q.quantize(v)).collect();
        let norms: Vec<f32> = vecs.iter().map(|v| norm(v)).collect();

        // 1. Inner-product fidelity over all pairs.
        let mut errs: Vec<f32> = Vec::with_capacity(n * (n - 1) / 2);
        for i in 0..n {
            for j in (i + 1)..n {
                let exact = dot_f32(&vecs[i], &vecs[j]);
                let est = dot_quantized(&quantized[i], &quantized[j]);
                let denom = (norms[i] * norms[j]).max(1e-9);
                errs.push((exact - est).abs() / denom);
            }
        }
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = errs[errs.len() / 2];
        let p99 = errs[(errs.len() as f64 * 0.99) as usize];

        // 2. Clustering invariance — τ SWEEP. A single τ giving one giant
        // component would make "partitions identical" trivially true
        // (decorative). Sweeping τ across the cosine range forces the
        // decision boundary through the actual similarity distribution:
        // partitions must match at EVERY τ, and at least one τ must yield
        // a genuinely multi-cluster configuration for the check to count.
        let sims_f32 = cosine_matrix(n, |i, j| dot_f32(&vecs[i], &vecs[j]), &norms);
        let sims_q = cosine_matrix(
            n,
            |i, j| dot_quantized(&quantized[i], &quantized[j]),
            &norms,
        );
        let taus = [0.50, 0.60, 0.70, 0.80, 0.85, 0.90, 0.95, 0.98];
        let mut all_identical = true;
        let mut max_clusters = 1usize;
        let mut cluster_profile = String::new();
        for (ti, tau) in taus.iter().enumerate() {
            let parts_f32 = components(&sims_f32, *tau);
            let parts_q = components(&sims_q, *tau);
            let identical = parts_f32 == parts_q;
            all_identical &= identical;
            let n_clusters = {
                let mut reps: Vec<usize> = parts_f32.clone();
                reps.sort_unstable();
                reps.dedup();
                reps.len()
            };
            max_clusters = max_clusters.max(n_clusters);
            if ti > 0 {
                cluster_profile.push('/');
            }
            let _ = write!(
                cluster_profile,
                "{n_clusters}{}",
                if identical { "" } else { "!" }
            );
        }
        // The sweep must exercise a non-trivial partition somewhere,
        // otherwise the invariance statement is vacuous for this source.
        let non_trivial = max_clusters > 1;

        println!(
            "  [{name}] n={n} dim={dim}→{pad} | IP err p50={p50:.5} p99={p99:.5} | clusters over τ-sweep: {cluster_profile} (max={max_clusters}) | identical at all τ: {all_identical}",
            pad = q.dim
        );

        // GO/NO-GO per source: fidelity + invariance at EVERY τ + the
        // sweep actually exercised a multi-cluster boundary.
        let source_go = p99 <= 0.05 && all_identical && non_trivial;
        go &= source_go;

        if !first {
            report.push(',');
        }
        first = false;
        let _ = write!(
            report,
            "{{\"name\":\"{name}\",\"n\":{n},\"dim\":{dim},\"ip_err_p50\":{p50:.6},\"ip_err_p99\":{p99:.6},\"clusters_profile\":\"{cluster_profile}\",\"max_clusters\":{max_clusters},\"identical_all_tau\":{all_identical},\"non_trivial\":{non_trivial},\"go\":{source_go}}}"
        );
    }

    let _ = write!(
        report,
        "],\"tau_sweep\":[0.50,0.60,0.70,0.80,0.85,0.90,0.95,0.98],\"quant\":\"int8+signflip+fwht\",\"go\":{go}}}"
    );
    let _ = std::fs::write("/tmp/s5-turboquant-spike.json", &report);
    println!("  report → /tmp/s5-turboquant-spike.json");

    // THE GATE (decision d482ec5c): p99 cosine-scale IP error ≤ 0.05 AND
    // clustering partitions identical on BOTH real embedding sources.
    // If this fails, S5 is rejected — document and stop.
    assert!(
        go,
        "S5 spike NO-GO: quantization broke inner-product fidelity or clustering invariance — see /tmp/s5-turboquant-spike.json"
    );
}
