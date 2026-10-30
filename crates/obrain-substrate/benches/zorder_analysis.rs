//! feat/zorder — Z-order (Morton) superposition analysis bench.
//!
//! Quantifies the cost/benefit of superposing a Morton index on the existing
//! Hilbert curve ordering (`crate::hilbert`). Three sections:
//!
//! 1. **B1 — key fill throughput**: ns/key of `hilbert_key_from_features` vs
//!    `zorder_key_from_features` over m = 100 000 realistic feature pairs
//!    (log-uniform centrality, Pareto degree) at order ∈ {6, 8} and
//!    max_degree ∈ {4096, 32}. The Z column must cost ≈ 2–5 ns/key; the delta
//!    vs Hilbert is the incremental fill cost of the superposition.
//!
//! 2. **B2 — 2D window enumeration**: m = 66 000 community points projected on
//!    the order-6 grid. Two strategies, measured in ns/window:
//!    * SCAN  — linear predicate over the (cx, cy) arrays (baseline);
//!    * ZRUNS — one sorted `(zorder_key, idx)` table (built once, timed
//!      separately), then per window: `zorder_window_ranges` → binary-search
//!      run bounds → exact predicate re-check on every candidate.
//!    Window area fractions: 50 %, 10 %, 1 %, 0.1 %, 0.01 % of the grid —
//!    average over a fixed-seed set of windows per fraction. Prints
//!    matches/window and runs/window (runs validate the quadtree
//!    decomposition; matches are asserted equal between the two strategies).
//!
//! 3. **B3 — curve locality (physical-order trade-off)**: over the same
//!    community, the distribution of |curve(p) − curve(q)| between 4-grid
//!    neighbours present in the sample: mean / p50 / p99 / max for the Hilbert
//!    curve vs the Z curve. Decides whether Z could ever replace Hilbert as
//!    the *physical* page order: expected are rare but O(n²)-sized Z leaps at
//!    quadrant boundaries (documented in `hilbert.rs`).
//!
//! # Feature model vs grid model (important)
//!
//! B1 keys are insensitive to the sample distribution, so it draws the *literal*
//! feature marginals prescribed by the analysis: log-uniform centrality over
//! [1, 65535] and Pareto(α = 2) degree truncated at max_degree.
//!
//! B2/B3 are *geometry* measurements and must spread the community over the
//! 2D plane — but the literal feature draw does **not** do that: the linear
//! quantizer `grid_coords_from_features` collapses log-uniform centrality
//! (62 % of the mass in column cx = 0) and Pareto degree (98 % of the mass in
//! row cy = 0) onto the bottom-left corner. Measured: only 275 of 4096 cells
//! occupied at n = 66 000, ≈ 60 % of the community inside the single cell
//! (0,0) — window runs and curve locality would be measured on a 1-row band,
//! not on a 2D community. B2/B3 therefore draw grid coordinates with the same
//! marginal *laws* at grid resolution (log-uniform over [1, 2^order] per axis:
//! still heavy-tailed, density ∝ 1/coordinate) and invert the quantizer to
//! mid-bin representative features, so every key path below stays real.
//!
//! # Run
//!
//! ```bash
//! cargo bench -p obrain-substrate --bench zorder_analysis
//! ```
//!
//! Env overrides for fast iteration:
//! * `OBRAIN_ZORDER_M`  — B1 feature-pair count (default 100 000),
//! * `OBRAIN_ZORDER_N`  — B2/B3 community size (default 66 000),
//! * `OBRAIN_ZORDER_WIN` — windows per area fraction (default 20).
//!
//! # Precision note
//!
//! All generators are deterministic (fixed seeds) and criterion warm-up /
//! measurement budgets are deliberately small because the machine is shared:
//! read the figures as robust orders of magnitude, not cycle-exact numbers.
//! The B2/B3 per-window/per-pair aggregate statistics are exact deterministic
//! counts printed once (not timed).

use std::hint::black_box;
use web_time::Instant;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use obrain_substrate::hilbert::{
    grid_coords_from_features, hilbert_index_2d, hilbert_key_from_features, zorder_key_2d,
    zorder_key_from_features, zorder_window_ranges,
};

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Community grid resolution used by B2/B3 (side 2^6 = 64).
const ORDER_B2: u32 = 6;
/// Standard production degree-saturation cap (shared by Hilbert and Z keys).
const MAX_DEGREE_COMMUNITY: u32 = 4096;

const FRACTIONS: [(&str, f64); 5] = [
    ("f50", 0.5),
    ("f10", 0.1),
    ("f1", 0.01),
    ("f01", 0.001),
    ("f001", 0.0001),
];

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Deterministic xorshift64 RNG (no `rand` dep at bench time).
// ---------------------------------------------------------------------------

struct Xs(u64);
impl Xs {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    #[inline]
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in (0, 1].
    #[inline]
    fn unit(&mut self) -> f64 {
        let v = self.next() >> 11; // 53 random bits
        ((v as f64) + 1.0) / ((1u64 << 53) as f64)
    }
    #[inline]
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

// ---------------------------------------------------------------------------
// Synthetic feature sample (shared distribution model for B1/B2/B3)
// ---------------------------------------------------------------------------

/// Generate `m` feature pairs:
/// * centrality: log2-uniform over [1, 65535] — cached PageRank Q0.16 piles up
///   at low values, so equal mass per octave is the honest null model;
/// * degree: Pareto(α = 2) on [1, ∞), truncated at `degree_cap`.
///
/// Used by B1 only (key cost is distribution-insensitive). B2/B3 use
/// [`gen_community`] instead — see the module doc for why.
fn gen_sample(seed: u64, m: usize, degree_cap: u32) -> (Vec<u16>, Vec<u32>) {
    let mut rng = Xs::new(seed);
    let mut cen = Vec::with_capacity(m);
    let mut deg = Vec::with_capacity(m);
    for _ in 0..m {
        let u = rng.unit();
        let c = (2f64.powf(16.0 * u)).min(65_535.0) as u16;
        cen.push(c.max(1));
        let u = rng.unit();
        let d = (1.0 / u).min(f64::from(degree_cap)).max(1.0);
        deg.push(d as u32);
    }
    (cen, deg)
}

/// Community feature model for B2/B3: draws (cx, cy) log-uniform per grid axis
/// over [1, 2^order] (density ∝ 1/coordinate per axis — the same heavy-tailed
/// law as the B1 marginals, but at grid resolution so the community spans the
/// plane instead of collapsing into the quantizer's bottom-left corner), then
/// returns mid-bin representative features that project back onto exactly the
/// drawn cells through `grid_coords_from_features`.
fn gen_community(
    seed: u64,
    m: usize,
    order: u32,
    max_degree: u32,
) -> (Vec<u16>, Vec<u32>) {
    let n = 1u32 << order; // grid side
    let last = u64::from(n - 1); // max coordinate
    let mut rng = Xs::new(seed);
    let mut cen = Vec::with_capacity(m);
    let mut deg = Vec::with_capacity(m);
    for _ in 0..m {
        let u = rng.unit();
        let cx = ((2f64.powf(f64::from(order) * u)) as u32).clamp(1, n) - 1;
        let u = rng.unit();
        let cy = ((2f64.powf(f64::from(order) * u)) as u32).clamp(1, n) - 1;
        // Mid-bin representatives: c = ((2·cx+1)·65535)/(2·(n−1)) ⟹
        // (c·(n−1))/65535 == cx (same construction for degree).
        let c = (((2 * u64::from(cx) + 1) * 65_535) / (2 * last)).min(65_535);
        let d = (((2 * u64::from(cy) + 1) * u64::from(max_degree)) / (2 * last))
            .min(u64::from(max_degree));
        cen.push(c as u16);
        deg.push(d as u32);
    }
    (cen, deg)
}

/// Project a feature sample onto the shared (cx, cy) grid via the crate's
/// single point of definition `grid_coords_from_features`.
fn project_points(
    cen: &[u16],
    deg: &[u32],
    order: u32,
    max_degree: u32,
) -> (Vec<u32>, Vec<u32>) {
    let mut cx = Vec::with_capacity(cen.len());
    let mut cy = Vec::with_capacity(cen.len());
    for i in 0..cen.len() {
        let (x, y) = grid_coords_from_features(cen[i], deg[i], order, max_degree);
        cx.push(x);
        cy.push(y);
    }
    (cx, cy)
}

// ---------------------------------------------------------------------------
// B2 helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Win {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

/// Random compact axis-aligned rectangles fully inside the n×n grid whose area
/// is ~`area_fraction` of the grid. Aspect ratio log-uniform in [0.5, 2].
/// Returns the windows plus the *effective* mean covered area fraction.
fn gen_windows(seed: u64, n: u32, area_fraction: f64, count: usize) -> (Vec<Win>, f64) {
    let mut rng = Xs::new(seed);
    let mut wins = Vec::with_capacity(count);
    let total = (n * n) as f64;
    let target = (area_fraction * total).max(1.0);
    for _ in 0..count {
        let flip = rng.range(2) == 0;
        let ar = 2f64.powf(rng.unit());
        let ar = if flip { 1.0 / ar } else { ar };
        let w = ((target * ar).sqrt().round() as u32).clamp(1, n);
        let h = ((target / ar).sqrt().round() as u32).clamp(1, n);
        let x0 = rng.range(u64::from(n - w + 1)) as u32;
        let y0 = rng.range(u64::from(n - h + 1)) as u32;
        wins.push(Win {
            x0,
            y0,
            x1: x0 + w - 1,
            y1: y0 + h - 1,
        });
    }
    let eff = wins
        .iter()
        .map(|w| {
            ((u64::from(w.x1 - w.x0 + 1)) * u64::from(w.y1 - w.y0 + 1)) as f64 / total
        })
        .sum::<f64>()
        / count.max(1) as f64;
    (wins, eff)
}

/// SCAN baseline: exact predicate over every point of the community.
fn scan_count(points_cx: &[u32], points_cy: &[u32], w: &Win) -> u64 {
    let mut n = 0u64;
    for i in 0..points_cx.len() {
        let x = points_cx[i];
        let y = points_cy[i];
        if x >= w.x0 && x <= w.x1 && y >= w.y0 && y <= w.y1 {
            n += 1;
        }
    }
    n
}

/// One entry of the sorted Z table: the fill computes
/// `zorder_key_from_features` exactly like the production sort pass.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ZEntry {
    z: u32,
    idx: u32,
}

/// ZRUNS query: decompose the window into contiguous Z runs, binary-search the
/// sorted table for each run, then re-verify the exact 2D predicate on every
/// candidate (loading cx/cy). Returns (matches, candidates_checked, n_runs).
fn zruns_query(sorted: &[ZEntry], cx: &[u32], cy: &[u32], w: &Win, order: u32) -> (u64, u64, usize) {
    let runs = zorder_window_ranges(w.x0, w.x1, w.y0, w.y1, order);
    let mut matches = 0u64;
    let mut candidates = 0u64;
    for &(lo, hi) in &runs {
        let start = sorted.partition_point(|e| e.z < lo);
        let end = sorted.partition_point(|e| e.z <= hi);
        candidates += (end - start) as u64;
        for e in &sorted[start..end] {
            let idx = e.idx as usize;
            let x = cx[idx];
            let y = cy[idx];
            if x >= w.x0 && x <= w.x1 && y >= w.y0 && y <= w.y1 {
                matches += 1;
            }
        }
    }
    (matches, candidates, runs.len())
}

// ---------------------------------------------------------------------------
// B3 helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Curve {
    Hilbert,
    Z,
}

#[inline]
fn pack(x: u32, y: u32) -> u32 {
    (x << 16) | y
}
#[inline]
fn unpack(c: u32) -> (u32, u32) {
    (c >> 16, c & 0xFFFF)
}

#[inline]
fn curve_key(c: Curve, x: u32, y: u32, order: u32) -> u32 {
    match c {
        Curve::Hilbert => hilbert_index_2d(x, y, order),
        Curve::Z => zorder_key_2d(x, y, order),
    }
}

/// For every community point, its 4 grid neighbours (orthogonal cells)
/// present in the sample — recorded as directed (cell_a, cell_b) pairs, so
/// high-occupancy cells weigh in proportion to their population. Two points in
/// the same cell have gap 0 and are not neighbours of each other here.
fn build_adjacency(cx: &[u32], cy: &[u32], n: u32) -> (Vec<(u32, u32)>, usize) {
    let mut occupied: std::collections::HashSet<u32> =
        std::collections::HashSet::with_capacity(cx.len());
    for i in 0..cx.len() {
        occupied.insert(pack(cx[i], cy[i]));
    }
    let mut pairs = Vec::new();
    for i in 0..cx.len() {
        let x = cx[i] as i64;
        let y = cy[i] as i64;
        for (dx, dy) in [(1i64, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < 0 || ny < 0 || nx >= n as i64 || ny >= n as i64 {
                continue;
            }
            if occupied.contains(&pack(nx as u32, ny as u32)) {
                pairs.push((pack(x as u32, y as u32), pack(nx as u32, ny as u32)));
            }
        }
    }
    (pairs, occupied.len())
}

struct GapStats {
    n: usize,
    mean: f64,
    p50: u64,
    p99: u64,
    max: u64,
}

fn gap_stats(gaps: &mut [u64]) -> GapStats {
    let n = gaps.len();
    if n == 0 {
        return GapStats {
            n: 0,
            mean: 0.0,
            p50: 0,
            p99: 0,
            max: 0,
        };
    }
    gaps.sort_unstable();
    let sum: u128 = gaps.iter().map(|&g| u128::from(g)).sum();
    GapStats {
        n,
        mean: sum as f64 / n as f64,
        p50: gaps[n / 2],
        p99: gaps[((n * 99) / 100).min(n - 1)],
        max: gaps[n - 1],
    }
}

fn print_gap_tail(label: &str, gaps: &[u64]) {
    let over = |t: u64| gaps.iter().filter(|&&g| g >= t).count();
    eprintln!(
        "[zorder]   {label}: gaps >= 16: {} ({:.2}%), >= 128: {} ({:.2}%), >= 1024: {} ({:.2}%)",
        over(16),
        100.0 * over(16) as f64 / gaps.len().max(1) as f64,
        over(128),
        100.0 * over(128) as f64 / gaps.len().max(1) as f64,
        over(1024),
        100.0 * over(1024) as f64 / gaps.len().max(1) as f64,
    );
}

// ---------------------------------------------------------------------------
// B1 — key fill throughput (hilbert vs zorder, order × max_degree grid)
// ---------------------------------------------------------------------------

fn bench_key_fill(c: &mut Criterion) {
    let m = env_usize("OBRAIN_ZORDER_M", 100_000);
    let (cen, deg) = gen_sample(0xB1_5EED_0001, m, 4096);
    eprintln!("[zorder] B1 sample: m = {m} feature pairs (seed fixed)");

    let mut g = c.benchmark_group("b1_key_fill");
    g.sample_size(30);
    g.warm_up_time(Duration::from_millis(1500));
    g.measurement_time(Duration::from_secs(3));
    g.throughput(Throughput::Elements(m as u64));

    for order in [6u32, 8] {
        for max_degree in [4096u32, 32] {
            g.bench_function(
                BenchmarkId::new("hilbert", format!("o{order}_md{max_degree}")),
                |b| {
                    b.iter(|| {
                        let mut s: u64 = 0;
                        for i in 0..m {
                            let k = hilbert_key_from_features(
                                black_box(cen[i]),
                                black_box(deg[i]),
                                order,
                                max_degree,
                            );
                            s = s.wrapping_add(u64::from(k));
                        }
                        black_box(s)
                    });
                },
            );
            g.bench_function(
                BenchmarkId::new("zorder", format!("o{order}_md{max_degree}")),
                |b| {
                    b.iter(|| {
                        let mut s: u64 = 0;
                        for i in 0..m {
                            let k = zorder_key_from_features(
                                black_box(cen[i]),
                                black_box(deg[i]),
                                order,
                                max_degree,
                            );
                            s = s.wrapping_add(u64::from(k));
                        }
                        black_box(s)
                    });
                },
            );
        }
    }
    g.finish();
}

// ---------------------------------------------------------------------------
// B2 — 2D window enumeration: SCAN vs ZRUNS
// ---------------------------------------------------------------------------

fn bench_window_enumeration(c: &mut Criterion) {
    let n_points = env_usize("OBRAIN_ZORDER_N", 66_000);
    let n_windows = env_usize("OBRAIN_ZORDER_WIN", 20);
    let grid_n = 1u32 << ORDER_B2;

    let (cen, deg) = gen_community(0xB2_5EED_0001, n_points, ORDER_B2, MAX_DEGREE_COMMUNITY);
    let (cx, cy) = project_points(&cen, &deg, ORDER_B2, MAX_DEGREE_COMMUNITY);

    // Community shape audit (occupancy of the 2^order × 2^order grid).
    let mut cell_counts: Vec<u64> = vec![0; (grid_n * grid_n) as usize];
    for i in 0..n_points {
        cell_counts[(cy[i] * grid_n + cx[i]) as usize] += 1;
    }
    cell_counts.sort_unstable();
    let distinct = cell_counts.iter().filter(|&&c| c > 0).count();
    let nz: Vec<&u64> = cell_counts.iter().filter(|&&c| c > 0).collect();
    let median_nz = nz[nz.len() / 2];
    let max_cell = cell_counts[cell_counts.len() - 1];
    let n_cells = (grid_n * grid_n) as usize;
    eprintln!(
        "[zorder] B2 community: n = {n_points} on {grid_n}×{grid_n} grid, {} cells \
         occupied ({:.1}% of {n_cells}), max cell occupancy {max_cell} \
         ({:.2}% of points), median occupied cell {median_nz}",
        distinct,
        100.0 * distinct as f64 / n_cells as f64,
        100.0 * max_cell as f64 / n_points as f64,
    );

    // The sorted Z table — built once, reused by every ZRUNS query and by the
    // timed construction bench below.
    let mut zs: Vec<ZEntry> = Vec::with_capacity(n_points);
    for i in 0..n_points {
        let z = zorder_key_from_features(cen[i], deg[i], ORDER_B2, MAX_DEGREE_COMMUNITY);
        zs.push(ZEntry {
            z,
            idx: i as u32,
        });
    }
    let t0 = Instant::now();
    zs.sort_unstable();
    let build_once_ms = t0.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "[zorder] B2: {build_once_ms:.2} ms for one (z,idx) table build over {n_points} points (single shot, untimed)"
    );

    let mut g = c.benchmark_group("b2_window_enumeration");
    g.sample_size(30);
    g.warm_up_time(Duration::from_millis(1500));
    g.measurement_time(Duration::from_secs(3));

    // Rebuild+sort the Z table per iteration (amortised construction cost).
    g.bench_function(BenchmarkId::new("zsort_build", format!("m{n_points}")), |b| {
        b.iter(|| {
            let mut entries: Vec<ZEntry> = Vec::with_capacity(n_points);
            for i in 0..n_points {
                let z = zorder_key_from_features(
                    black_box(cen[i]),
                    black_box(deg[i]),
                    ORDER_B2,
                    MAX_DEGREE_COMMUNITY,
                );
                entries.push(ZEntry {
                    z,
                    idx: i as u32,
                });
            }
            entries.sort_unstable();
            black_box(entries.len())
        });
    });

    for &(label, frac) in &FRACTIONS {
        let seed = 0xB2_5EED_0000 ^ (frac.to_bits());
        let (wins, eff) = gen_windows(seed, grid_n, frac, n_windows);

        // Exact deterministic aggregate per fraction (asserted identical
        // between strategies) — these are the reported matches/runs numbers.
        // candidates == matches always: `zorder_window_ranges` tiles exactly
        // the window's cells (validated by the module tests), so every entry
        // the binary search returns lives in a window cell and the per-candidate
        // predicate re-check never filters — asserted here, window by window.
        let mut tot_match = 0u64;
        let mut tot_runs = 0u64;
        for w in &wins {
            let s = scan_count(&cx, &cy, w);
            let (z, cand, runs) = zruns_query(&zs, &cx, &cy, w, ORDER_B2);
            assert_eq!(
                s, z,
                "scan/zruns mismatch for window ({},{},{},{})",
                w.x0, w.y0, w.x1, w.y1
            );
            assert_eq!(
                cand, z,
                "zruns candidates != matches: window ({},{},{},{}) — runs do not tile the window exactly",
                w.x0, w.y0, w.x1, w.y1
            );
            tot_match += s;
            tot_runs += runs as u64;
        }
        eprintln!(
            "[zorder] B2 {label}: eff_area_frac = {eff:.5}, matches/win = {:.1} \
             (= verified candidates/win), zruns/win = {:.2} (n_windows = {})",
            tot_match as f64 / n_windows as f64,
            tot_runs as f64 / n_windows as f64,
            n_windows,
        );

        let mut k_scan = 0usize;
        g.bench_function(BenchmarkId::new("scan", label), |b| {
            b.iter(|| {
                let w = wins[k_scan % n_windows];
                k_scan += 1;
                black_box(scan_count(&cx, &cy, &w))
            });
        });

        let mut k_z = 0usize;
        g.bench_function(BenchmarkId::new("zruns", label), |b| {
            b.iter(|| {
                let w = wins[k_z % n_windows];
                k_z += 1;
                let (matches, _cand, runs) = zruns_query(&zs, &cx, &cy, &w, ORDER_B2);
                black_box((matches, runs))
            });
        });
    }
    g.finish();
}

// ---------------------------------------------------------------------------
// B3 — curve locality: gap distribution between 4-grid-neighbours
// ---------------------------------------------------------------------------

fn bench_curve_locality(c: &mut Criterion) {
    let n_points = env_usize("OBRAIN_ZORDER_N", 66_000);
    let grid_n = 1u32 << ORDER_B2;

    let (cen, deg) = gen_community(0xB3_5EED_0001, n_points, ORDER_B2, MAX_DEGREE_COMMUNITY);
    let (cx, cy) = project_points(&cen, &deg, ORDER_B2, MAX_DEGREE_COMMUNITY);
    let (pairs, n_cells) = build_adjacency(&cx, &cy, grid_n);
    eprintln!(
        "[zorder] B3 adjacency: {} occupied cells, {} directed 4-neighbour pairs over {n_points} points",
        n_cells,
        pairs.len(),
    );

    for curve in [Curve::Hilbert, Curve::Z] {
        let label = match curve {
            Curve::Hilbert => "hilbert",
            Curve::Z => "zorder",
        };
        let mut gaps: Vec<u64> = Vec::with_capacity(pairs.len());
        for &(a, b) in &pairs {
            let (ax, ay) = unpack(a);
            let (bx, by) = unpack(b);
            let ka = u64::from(curve_key(curve, ax, ay, ORDER_B2));
            let kb = u64::from(curve_key(curve, bx, by, ORDER_B2));
            gaps.push(ka.abs_diff(kb));
        }
        let st = gap_stats(&mut gaps);
        eprintln!(
            "[zorder] B3 {label}: n_pairs = {}, mean = {:.1}, p50 = {}, p99 = {}, max = {}",
            st.n, st.mean, st.p50, st.p99, st.max
        );
        print_gap_tail(label, &gaps);
    }

    let mut g = c.benchmark_group("b3_locality_walk");
    g.sample_size(30);
    g.warm_up_time(Duration::from_millis(1500));
    g.measurement_time(Duration::from_secs(3));
    g.throughput(Throughput::Elements(pairs.len() as u64));

    g.bench_function("hilbert", |b| {
        b.iter(|| {
            let mut s: u64 = 0;
            for &(a, bb) in pairs.iter() {
                let (ax, ay) = unpack(a);
                let (bx, by) = unpack(bb);
                let ka = u64::from(hilbert_index_2d(ax, ay, ORDER_B2));
                let kb = u64::from(hilbert_index_2d(bx, by, ORDER_B2));
                s = s.wrapping_add(ka.abs_diff(kb));
            }
            black_box(s)
        });
    });
    g.bench_function("zorder", |b| {
        b.iter(|| {
            let mut s: u64 = 0;
            for &(a, bb) in pairs.iter() {
                let (ax, ay) = unpack(a);
                let (bx, by) = unpack(bb);
                let ka = u64::from(zorder_key_2d(ax, ay, ORDER_B2));
                let kb = u64::from(zorder_key_2d(bx, by, ORDER_B2));
                s = s.wrapping_add(ka.abs_diff(kb));
            }
            black_box(s)
        });
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_key_fill,
    bench_window_enumeration,
    bench_curve_locality
);
criterion_main!(benches);
