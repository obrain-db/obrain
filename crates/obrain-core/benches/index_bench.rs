//! Benchmarks for index structures.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use obrain_common::types::{EdgeId, NodeId};
use obrain_core::index::adjacency::ChunkedAdjacency;
use obrain_core::index::hash::HashIndex;
use obrain_core::index::vector::{
    DistanceMetric, HnswConfig, HnswIndex, ProductQuantizer, ScalarQuantizer, brute_force_knn,
    compute_distance,
};

fn bench_adjacency_insert(c: &mut Criterion) {
    c.bench_function("adjacency_insert_1000", |b| {
        b.iter(|| {
            let adj = ChunkedAdjacency::new();
            for i in 0..1000u64 {
                adj.add_edge(NodeId(i % 100), NodeId(i), EdgeId(i));
            }
            black_box(adj)
        });
    });
}

fn bench_adjacency_lookup(c: &mut Criterion) {
    let adj = ChunkedAdjacency::new();
    for i in 0..10000u64 {
        adj.add_edge(NodeId(i % 100), NodeId(i), EdgeId(i));
    }

    c.bench_function("adjacency_lookup", |b| {
        b.iter(|| {
            for i in 0..100u64 {
                black_box(adj.neighbors(NodeId(i)));
            }
        });
    });
}

fn bench_hash_index_insert(c: &mut Criterion) {
    c.bench_function("hash_index_insert_1000", |b| {
        b.iter(|| {
            let index: HashIndex<u64, NodeId> = HashIndex::new();
            for i in 0..1000u64 {
                index.insert(i, NodeId(i));
            }
            black_box(index)
        });
    });
}

fn bench_hash_index_lookup(c: &mut Criterion) {
    let index: HashIndex<u64, NodeId> = HashIndex::new();
    for i in 0..10000u64 {
        index.insert(i, NodeId(i));
    }

    c.bench_function("hash_index_lookup", |b| {
        b.iter(|| {
            for i in 0..1000u64 {
                black_box(index.get(&i));
            }
        });
    });
}

// ============================================================================
// Vector Index Benchmarks
// ============================================================================

fn generate_random_vectors(count: usize, dims: usize, seed: u64) -> Vec<Vec<f32>> {
    // Simple deterministic pseudo-random generator
    let mut state = seed;
    (0..count)
        .map(|_| {
            (0..dims)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    ((state >> 33) as f32) / (u32::MAX as f32) - 0.5
                })
                .collect()
        })
        .collect()
}

fn bench_distance_computation(c: &mut Criterion) {
    let dims = 384; // Common embedding size
    let vectors = generate_random_vectors(2, dims, 42);
    let v1 = &vectors[0];
    let v2 = &vectors[1];

    let mut group = c.benchmark_group("distance");

    group.bench_function("cosine_384d", |b| {
        b.iter(|| black_box(compute_distance(v1, v2, DistanceMetric::Cosine)));
    });

    group.bench_function("euclidean_384d", |b| {
        b.iter(|| black_box(compute_distance(v1, v2, DistanceMetric::Euclidean)));
    });

    group.bench_function("dot_product_384d", |b| {
        b.iter(|| black_box(compute_distance(v1, v2, DistanceMetric::DotProduct)));
    });

    group.finish();
}

fn bench_brute_force_knn(c: &mut Criterion) {
    let dims = 128;
    let count = 1000;

    let vectors = generate_random_vectors(count, dims, 42);
    let query = generate_random_vectors(1, dims, 123)[0].clone();

    let indexed: Vec<(NodeId, &[f32])> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (NodeId::new(i as u64), v.as_slice()))
        .collect();

    c.bench_function("brute_force_knn_1k_k10", |b| {
        b.iter(|| {
            black_box(brute_force_knn(
                indexed.iter().map(|(id, v)| (*id, *v)),
                &query,
                10,
                DistanceMetric::Cosine,
            ))
        });
    });
}

fn bench_hnsw_insert(c: &mut Criterion) {
    use std::collections::HashMap;
    use std::sync::Arc;

    let dims = 128;
    let count = 500;

    let vectors = generate_random_vectors(count, dims, 42);

    // Build accessor map
    let map: HashMap<NodeId, Arc<[f32]>> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (NodeId::new(i as u64), Arc::from(v.as_slice())))
        .collect();
    let accessor = move |id: NodeId| -> Option<Arc<[f32]>> { map.get(&id).cloned() };

    c.bench_function("hnsw_insert_500", |b| {
        b.iter(|| {
            let config = HnswConfig::new(dims, DistanceMetric::Cosine);
            let index = HnswIndex::with_seed(config, 12345);
            for (i, vec) in vectors.iter().enumerate() {
                index.insert(NodeId::new(i as u64), vec, &accessor);
            }
            black_box(index)
        });
    });
}

fn bench_hnsw_search(c: &mut Criterion) {
    use std::collections::HashMap;
    use std::sync::Arc;

    let dims = 128;
    let count = 2000;

    let vectors = generate_random_vectors(count, dims, 42);

    // Build accessor map
    let map: HashMap<NodeId, Arc<[f32]>> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (NodeId::new(i as u64), Arc::from(v.as_slice())))
        .collect();
    let accessor = move |id: NodeId| -> Option<Arc<[f32]>> { map.get(&id).cloned() };

    let config = HnswConfig::new(dims, DistanceMetric::Cosine);
    let index = HnswIndex::with_seed(config, 12345);

    for (i, vec) in vectors.iter().enumerate() {
        index.insert(NodeId::new(i as u64), vec, &accessor);
    }

    let query = generate_random_vectors(1, dims, 999)[0].clone();

    c.bench_function("hnsw_search_2k_k10", |b| {
        b.iter(|| black_box(index.search(&query, 10, &accessor)));
    });
}

fn bench_scalar_quantization(c: &mut Criterion) {
    let dims = 384;
    let count = 1000;

    let vectors = generate_random_vectors(count, dims, 42);
    let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
    let quantizer = ScalarQuantizer::train(&refs);

    let query = &vectors[0];

    let mut group = c.benchmark_group("scalar_quantization");

    group.bench_function("train_1000x384", |b| {
        b.iter(|| black_box(ScalarQuantizer::train(&refs)));
    });

    group.bench_function("quantize_384d", |b| {
        b.iter(|| black_box(quantizer.quantize(query)));
    });

    let quantized = quantizer.quantize(query);
    let quantized2 = quantizer.quantize(&vectors[1]);

    group.bench_function("distance_u8_384d", |b| {
        b.iter(|| black_box(quantizer.distance_u8(&quantized, &quantized2)));
    });

    group.finish();
}

fn bench_product_quantization(c: &mut Criterion) {
    let dims = 128; // Must be divisible by num_subvectors
    let count = 500;
    let num_subvectors = 8;

    let vectors = generate_random_vectors(count, dims, 42);
    let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
    let quantizer = ProductQuantizer::train(&refs, num_subvectors, 256, 5);

    let query = &vectors[0];

    let mut group = c.benchmark_group("product_quantization");

    group.bench_function("train_500x128_pq8", |b| {
        b.iter(|| black_box(ProductQuantizer::train(&refs, num_subvectors, 256, 5)));
    });

    group.bench_function("quantize_128d_pq8", |b| {
        b.iter(|| black_box(quantizer.quantize(query)));
    });

    let codes = quantizer.quantize(query);

    group.bench_function("asymmetric_distance_128d_pq8", |b| {
        b.iter(|| black_box(quantizer.asymmetric_distance(query, &codes)));
    });

    group.bench_function("build_distance_table_128d_pq8", |b| {
        b.iter(|| black_box(quantizer.build_distance_table(query)));
    });

    let table = quantizer.build_distance_table(query);

    group.bench_function("distance_with_table_pq8", |b| {
        b.iter(|| black_box(quantizer.distance_with_table(&table, &codes)));
    });

    group.finish();
}

// ── VP-Tree Benchmarks ─────────────────────────────────────────────

use obrain_core::index::vp_tree::VpTree;

fn euclidean_64d(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Deterministic pseudo-random f32 in [0, 1).
fn lcg_f32(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as f32 / (1u64 << 31) as f32
}

fn generate_64d_points(n: usize) -> Vec<(u32, Vec<f32>)> {
    let mut rng: u64 = 42;
    (0..n)
        .map(|i| {
            let v: Vec<f32> = (0..64).map(|_| lcg_f32(&mut rng)).collect();
            (i as u32, v)
        })
        .collect()
}

fn bench_vp_tree_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("vp_tree_build");

    for &size in &[10_000, 100_000] {
        let points = generate_64d_points(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &points, |b, pts| {
            b.iter(|| {
                black_box(VpTree::build(pts.clone(), euclidean_64d));
            });
        });
    }

    group.finish();
}

fn bench_vp_tree_knn(c: &mut Criterion) {
    let mut group = c.benchmark_group("vp_tree_knn");

    for &size in &[10_000, 100_000] {
        let points = generate_64d_points(size);
        let tree = VpTree::build(points, euclidean_64d);
        let mut rng: u64 = 123;
        let query: Vec<f32> = (0..64).map(|_| lcg_f32(&mut rng)).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                black_box(tree.knn(&query, 10, euclidean_64d));
            });
        });
    }

    group.finish();
}

// ── VP-Tree range_search 2D géo ──────────────────────────────────
// Le banc en 64d uniforme (ci-dessus) est le PIRE cas d'un VP-Tree et ne peut
// pas révéler le comportement sur de la donnée géo quantifiée (ex æquo). Ici on
// mesure range_search vs balayage sur trois distributions 2D, pour trancher
// « à partir de quel n et de quelle sélectivité l'index gagne ».

fn euclidean_2d(a: &[f32], b: &[f32]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Uniforme sur [0,1]².
fn gen_2d_uniform(n: usize) -> Vec<(u32, Vec<f32>)> {
    let mut rng: u64 = 42;
    (0..n)
        .map(|i| (i as u32, vec![lcg_f32(&mut rng), lcg_f32(&mut rng)]))
        .collect()
}

/// Quantifiée à 3 décimales (comme le pont d'appareil arrondit lat/lon) :
/// équidistances et ex æquo en masse.
fn gen_2d_quantized(n: usize) -> Vec<(u32, Vec<f32>)> {
    let mut rng: u64 = 7;
    (0..n)
        .map(|i| {
            let x = (lcg_f32(&mut rng) * 1000.0).round() / 1000.0;
            let y = (lcg_f32(&mut rng) * 1000.0).round() / 1000.0;
            (i as u32, vec![x, y])
        })
        .collect()
}

/// Duplicatas massifs : la moitié des points à la même coordonnée (même adresse).
fn gen_2d_duplicates(n: usize) -> Vec<(u32, Vec<f32>)> {
    let mut rng: u64 = 13;
    (0..n)
        .map(|i| {
            if i % 2 == 0 {
                (i as u32, vec![0.5, 0.5])
            } else {
                (i as u32, vec![lcg_f32(&mut rng), lcg_f32(&mut rng)])
            }
        })
        .collect()
}

fn gen_2d(dist: &str, n: usize) -> Vec<(u32, Vec<f32>)> {
    match dist {
        "uniform" => gen_2d_uniform(n),
        "quantized" => gen_2d_quantized(n),
        "duplicates" => gen_2d_duplicates(n),
        _ => unreachable!(),
    }
}

const RADII_2D: [f32; 3] = [0.01, 0.05, 0.15];
const SIZES_2D: [usize; 4] = [1_000, 10_000, 100_000, 1_000_000];
const DISTS_2D: [&str; 3] = ["uniform", "quantized", "duplicates"];

fn bench_vp_tree_range_2d(c: &mut Criterion) {
    let query = vec![0.5_f32, 0.5_f32];
    for dist in DISTS_2D {
        for &size in &SIZES_2D {
            let points = gen_2d(dist, size);
            let tree = VpTree::build(points, euclidean_2d);
            for &r in &RADII_2D {
                c.bench_function(&format!("vp_tree_range_2d/{dist}/{size}/{r}"), |b| {
                    b.iter(|| {
                        black_box(tree.range_search(&query, r, euclidean_2d));
                    });
                });
            }
        }
    }
}

fn bench_balayage_2d(c: &mut Criterion) {
    let query = vec![0.5_f32, 0.5_f32];
    for dist in DISTS_2D {
        for &size in &SIZES_2D {
            let points = gen_2d(dist, size);
            for &r in &RADII_2D {
                c.bench_function(&format!("balayage_2d/{dist}/{size}/{r}"), |b| {
                    b.iter(|| {
                        let mut count = 0usize;
                        for (_, p) in &points {
                            if euclidean_2d(&query, p) <= r {
                                count += 1;
                            }
                        }
                        black_box(count);
                    });
                });
            }
        }
    }
}

fn bench_vp_tree_build_2d(c: &mut Criterion) {
    for dist in DISTS_2D {
        for &size in &SIZES_2D {
            let points = gen_2d(dist, size);
            c.bench_function(&format!("vp_tree_build_2d/{dist}/{size}"), |b| {
                b.iter(|| {
                    black_box(VpTree::build(points.clone(), euclidean_2d));
                });
            });
        }
    }
}

criterion_group!(
    benches,
    bench_adjacency_insert,
    bench_adjacency_lookup,
    bench_hash_index_insert,
    bench_hash_index_lookup,
    bench_distance_computation,
    bench_brute_force_knn,
    bench_hnsw_insert,
    bench_hnsw_search,
    bench_scalar_quantization,
    bench_product_quantization,
    bench_vp_tree_build,
    bench_vp_tree_knn,
    bench_vp_tree_range_2d,
    bench_balayage_2d,
    bench_vp_tree_build_2d,
);

criterion_main!(benches);
