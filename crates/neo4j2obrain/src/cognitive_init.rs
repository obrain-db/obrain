//! # Cognitive initialisation batches (stages 4-6 of the pipeline)
//!
//! Each batch runs over a freshly written [`SubstrateStore`] and fills
//! a cognitive column (`community_id`, `centrality_cached`, `edge.ricci`,
//! `node.curvature`). The goal is "cognitively alive on open" — nothing
//! lazy, no cold-start latency on the first query after import.
//!
//! ## Implementation status (plan T15)
//!
//! | Stage | Status                                                        |
//! |------:|----------------------------------------------------------------|
//! |     4 | Real — wraps [`obrain_cognitive::community::leiden_batch`]      |
//! |       | and persists via [`Writer::update_community_batch`].            |
//! |     5 | Real — wraps [`obrain_adapters::plugins::algorithms::pagerank`] |
//! |       | and persists via [`SubstrateStore::update_centrality_batch_f32`]. |
//! |     6 | Real — wraps [`obrain_substrate::refresh_all_ricci`] and        |
//! |       | [`obrain_substrate::compute_all_node_curvatures`].              |
//!
//! All three stages degrade gracefully on an empty graph (zero counts,
//! no errors).

#[cfg(feature = "cognitive-init")]
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use obrain_adapters::plugins::algorithms::pagerank;
#[cfg(feature = "cognitive-init")]
use obrain_cognitive::community::{Graph, LeidenConfig, leiden_batch};
use obrain_common::NodeId;
#[cfg(feature = "cognitive-init")]
use obrain_core::graph::Direction;
use obrain_core::graph::traits::GraphStore;
use obrain_substrate::SubstrateStore;

use crate::pipeline::PipelineStats;

/// Damping factor used by the PageRank batch (industry-standard 0.85).
const PAGERANK_DAMPING: f64 = 0.85;
/// Upper bound on power-iteration steps. 50 is more than enough for the
/// graph sizes targeted by obrain imports (≤ 10⁷ nodes typically
/// converges within 30 iterations at tol = 1e-6).
const PAGERANK_MAX_ITER: usize = 50;
/// Convergence tolerance (L1 norm of the PageRank vector delta).
const PAGERANK_TOL: f64 = 1.0e-6;

/// Stage 4 — LDleiden community detection batch (unit weights).
///
/// Délègue à [`run_ldleiden_batch_inner`] avec `ricci_boost = 0.0` : les
/// arêtes sont pondérées unitairement — comportement historique, inchangé.
#[cfg(feature = "cognitive-init")]
pub fn run_ldleiden_batch(
    substrate: &Arc<SubstrateStore>,
    stats: &mut PipelineStats,
) -> Result<()> {
    run_ldleiden_batch_inner(substrate, stats, 0.0)
}

/// Stage 4 variant — LDleiden pondéré par la courbure de Ricci (P3b).
///
/// Même pipeline que [`run_ldleiden_batch`] mais chaque arête porte le
/// poids `w(e) = max(0, 1 + ricci_boost · κ(e))`, où `κ(e)` est la
/// courbure d'Ollivier-Ricci persistée (`EdgeRecord.ricci_u8`, décodée par
/// `ricci_f32()`). Les arêtes à κ < 0 (cols/goulots — frontières
/// naturelles entre régions denses) sont affaiblies ; les arêtes à κ > 0
/// (cœurs) renforcées — la modularité préfère alors couper aux cols.
///
/// - `ricci_boost = 0.0` ⇒ strictement identique à la variante unitaire.
/// - Nécessite que la colonne ricci soit déjà calculée (Stage 6) — en
///   re-détection après un premier passage complet, pas en première
///   détection (l'ordre des stages du pipeline est inchangé).
#[cfg(feature = "cognitive-init")]
pub fn run_ldleiden_batch_ricci(
    substrate: &Arc<SubstrateStore>,
    stats: &mut PipelineStats,
    ricci_boost: f64,
) -> Result<()> {
    run_ldleiden_batch_inner(substrate, stats, ricci_boost)
}

/// Shared Stage 4 implementation. `ricci_boost` scales the Ricci-curvature
/// edge weighting; `0.0` reproduces the historical unit-weight behaviour.
#[cfg(feature = "cognitive-init")]
fn run_ldleiden_batch_inner(
    substrate: &Arc<SubstrateStore>,
    stats: &mut PipelineStats,
    ricci_boost: f64,
) -> Result<()> {
    let store: &dyn GraphStore = &**substrate;

    // ---- Dense re-indexing --------------------------------------------------
    let node_ids = store.all_node_ids();
    let n = node_ids.len();
    if n == 0 {
        tracing::info!("Stage 4 (LDleiden): empty graph — no communities to compute");
        stats.communities = 0;
        return Ok(());
    }
    if n > u32::MAX as usize {
        anyhow::bail!(
            "Stage 4 (LDleiden): {} nodes exceeds u32::MAX — leiden_batch \
             addresses nodes with u32 indices",
            n
        );
    }
    let mut dense_of: HashMap<NodeId, u32> = HashMap::with_capacity(n);
    let mut slot_of: Vec<u32> = Vec::with_capacity(n);
    for (dense_idx, nid) in node_ids.iter().enumerate() {
        dense_of.insert(*nid, dense_idx as u32);
        slot_of.push(nid.0 as u32);
    }

    // ---- Edge list (unit-weighted or Ricci-weighted, undirected collapse) --
    let mut edges: Vec<(u32, u32, f64)> = Vec::new();
    let mut dangling = 0u64;
    for src in &node_ids {
        let Some(&src_dense) = dense_of.get(src) else {
            continue;
        };
        for (dst, eid) in store.edges_from(*src, Direction::Outgoing) {
            let Some(&dst_dense) = dense_of.get(&dst) else {
                dangling += 1;
                continue;
            };
            let w = if ricci_boost == 0.0 {
                1.0
            } else {
                // Lecture du κ persisté sur l'arête (inline dans
                // l'EdgeRecord, zone mmap) — zéro si absent (arête dont le
                // refresh ricci n'a pas tourné) = poids unitaire.
                let kappa = substrate
                    .writer()
                    .read_edge(eid.0)
                    .ok()
                    .flatten()
                    .map(|rec| f64::from(rec.ricci_f32()))
                    .unwrap_or(0.0);
                (1.0 + ricci_boost * kappa).max(0.0)
            };
            edges.push((src_dense, dst_dense, w));
        }
    }
    let edge_count = edges.len();
    let graph = Graph::from_edges(n as u32, edges);

    // ---- Solve --------------------------------------------------------------
    let config = LeidenConfig::default();
    let partition = leiden_batch(&graph, config);

    // ---- Persist ------------------------------------------------------------
    // `update_community_batch` takes `(node_slot_idx, community_id)`. The
    // partition is indexed by dense idx, so we zip with `slot_of`.
    let updates: Vec<(u32, u32)> = partition
        .iter()
        .enumerate()
        .map(|(dense_idx, community_id)| (slot_of[dense_idx], *community_id))
        .collect();
    substrate
        .writer()
        .update_community_batch(updates)
        .context("update_community_batch")?;

    // ---- Stats --------------------------------------------------------------
    // Partition communities are 0-based dense ids (no gaps) per leiden
    // contract — num_communities = max + 1.
    let num_communities = partition.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    stats.communities = num_communities as u64;
    tracing::info!(
        "Stage 4 (LDleiden): n={}, edges={}, dangling={}, communities={} (ricci_boost={ricci_boost})",
        n,
        edge_count,
        dangling,
        num_communities,
    );
    Ok(())
}

/// Stage 4 fallback — cognitive layer not compiled in (repo split,
/// plan ad63daf1): community initialisation is skipped with a warning.
/// The `community_id` column stays empty; the cognitive layer (private)
/// can backfill it later. Stages 5-6 are unaffected (public crates only).
#[cfg(not(feature = "cognitive-init"))]
pub fn run_ldleiden_batch(
    _substrate: &Arc<SubstrateStore>,
    _stats: &mut PipelineStats,
) -> Result<()> {
    tracing::warn!(
        "Stage 4 (LDleiden) skipped — built without the `cognitive-init` \
         feature; community_id column left empty"
    );
    Ok(())
}

/// Ricci-weighted variant fallback (same split as [`run_ldleiden_batch`]).
#[cfg(not(feature = "cognitive-init"))]
pub fn run_ldleiden_batch_ricci(
    _substrate: &Arc<SubstrateStore>,
    _stats: &mut PipelineStats,
    _ricci_boost: f64,
) -> Result<()> {
    tracing::warn!(
        "Stage 4 (LDleiden, ricci-weighted) skipped — built without the \
         `cognitive-init` feature; community_id column left empty"
    );
    Ok(())
}

/// Stage 5 — PageRank batch.
///
/// Wraps [`obrain_adapters::plugins::algorithms::pagerank`] (damping =
/// [`PAGERANK_DAMPING`], max_iter = [`PAGERANK_MAX_ITER`], tol =
/// [`PAGERANK_TOL`]) and persists the result via
/// [`SubstrateStore::update_centrality_batch_f32`] which quantises to
/// the `centrality_cached` u16 column (Q0.16) under a WAL record.
///
/// The `update_centrality_batch_f32` path also clears
/// `CENTRALITY_STALE`, so after this stage every node's cached
/// centrality matches the graph state at import time.
pub fn run_pagerank_batch(
    substrate: &Arc<SubstrateStore>,
    stats: &mut PipelineStats,
) -> Result<()> {
    let store: &dyn GraphStore = &**substrate;
    if store.node_count() == 0 {
        tracing::info!("Stage 5 (PageRank): empty graph — no scores to compute");
        stats.pagerank_nonzero = 0;
        return Ok(());
    }

    let scores = pagerank(store, PAGERANK_DAMPING, PAGERANK_MAX_ITER, PAGERANK_TOL);

    // Filter: only persist non-zero scores. Zero is the default column
    // value — writing zero is a no-op on a fresh store but still emits
    // a WAL record, so skipping keeps the replay stream lean.
    let mut updates: Vec<(NodeId, f32)> = Vec::with_capacity(scores.len());
    let mut nonzero = 0u64;
    for (nid, score) in scores {
        if score > 0.0 {
            nonzero += 1;
            updates.push((nid, score as f32));
        }
    }
    let total = updates.len() as u64;
    substrate
        .update_centrality_batch_f32(updates)
        .context("update_centrality_batch_f32")?;

    stats.pagerank_nonzero = nonzero;
    tracing::info!(
        "Stage 5 (PageRank): damping={}, max_iter={}, tol={:.0e}, persisted={}, nonzero={}",
        PAGERANK_DAMPING,
        PAGERANK_MAX_ITER,
        PAGERANK_TOL,
        total,
        nonzero,
    );
    Ok(())
}

/// Stage 6 — Ricci-Ollivier batch + node curvature aggregation.
///
/// Real implementation: calls into [`obrain_substrate`]'s public batch
/// surfaces exposed in T12 (geometric inference).
///
/// * [`obrain_substrate::refresh_all_ricci`] — walks every live edge,
///   computes Ollivier-Ricci, writes the quantised `ricci_u8` back via
///   the WAL-logged `update_edge` path, clears `RICCI_STALE`.
/// * [`obrain_substrate::compute_all_node_curvatures`] — aggregates the
///   per-edge Ricci into a node curvature property (degree-weighted
///   mean) for nodes with at least one live incident edge.
pub fn run_ricci_batch(substrate: &Arc<SubstrateStore>, stats: &mut PipelineStats) -> Result<()> {
    // ---- Edge-level Ricci (authoritative, WAL-logged) ------------------
    let edge_stats = obrain_substrate::refresh_all_ricci(substrate).context("refresh_all_ricci")?;

    // The substrate reports: edges_visited, edges_updated,
    // edges_skipped_dangling (+ curvature summary stats). For pipeline
    // metrics we count "refreshed" as edges_updated (the ones whose
    // persisted ricci_u8 actually changed).
    stats.ricci_refreshed = edge_stats.edges_updated;
    tracing::info!(
        "Stage 6 (Ricci): visited={}, updated={}, skipped_dangling={}, \
         |κ|_max={:.3}, frac(κ<0)={:.3}, frac(κ>0)={:.3}",
        edge_stats.edges_visited,
        edge_stats.edges_updated,
        edge_stats.edges_skipped_dangling,
        edge_stats.max_abs_curvature,
        edge_stats.negative_fraction,
        edge_stats.positive_fraction,
    );

    // ---- Node-level curvature aggregation ------------------------------
    let node_curv = obrain_substrate::compute_all_node_curvatures(substrate)
        .context("compute_all_node_curvatures")?;
    tracing::info!(
        "Stage 6 (Ricci): aggregated node curvature for {} live nodes",
        node_curv.len(),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn empty_substrate() -> Arc<SubstrateStore> {
        let td = TempDir::new().unwrap();
        let path = td.path().join("cog.obrain");
        let store = SubstrateStore::create(&path).unwrap();
        // keep td alive by leaking — test lifetime is tiny
        std::mem::forget(td);
        Arc::new(store)
    }

    #[test]
    fn ldleiden_on_empty_substrate_is_zero() {
        let sub = empty_substrate();
        let mut stats = PipelineStats::default();
        run_ldleiden_batch(&sub, &mut stats).unwrap();
        assert_eq!(stats.communities, 0);
    }

    #[test]
    fn pagerank_on_empty_substrate_is_zero() {
        let sub = empty_substrate();
        let mut stats = PipelineStats::default();
        run_pagerank_batch(&sub, &mut stats).unwrap();
        assert_eq!(stats.pagerank_nonzero, 0);
    }

    #[test]
    fn ricci_batch_on_empty_substrate_is_zero() {
        let sub = empty_substrate();
        let mut stats = PipelineStats::default();
        run_ricci_batch(&sub, &mut stats).unwrap();
        assert_eq!(stats.ricci_refreshed, 0);
    }

    /// End-to-end: populate a small substrate with two weakly-connected
    /// triangles, run LDleiden + PageRank, verify that:
    ///   * community detection yields 2 communities,
    ///   * PageRank assigns non-zero scores to at least the bridge
    ///     nodes (in-degree > 0).
    #[test]
    fn ldleiden_and_pagerank_on_two_triangles() {
        use obrain_core::graph::traits::GraphStoreMut;

        let td = TempDir::new().unwrap();
        let path = td.path().join("cog.obrain");
        let sub = Arc::new(SubstrateStore::create(&path).unwrap());
        // keep td alive
        std::mem::forget(td);

        // Triangle A: a-b-c-a
        let a = sub.create_node_with_props(&["N"], &[]);
        let b = sub.create_node_with_props(&["N"], &[]);
        let c = sub.create_node_with_props(&["N"], &[]);
        sub.create_edge_with_props(a, b, "E", &[]);
        sub.create_edge_with_props(b, c, "E", &[]);
        sub.create_edge_with_props(c, a, "E", &[]);

        // Triangle B: d-e-f-d
        let d = sub.create_node_with_props(&["N"], &[]);
        let e = sub.create_node_with_props(&["N"], &[]);
        let f = sub.create_node_with_props(&["N"], &[]);
        sub.create_edge_with_props(d, e, "E", &[]);
        sub.create_edge_with_props(e, f, "E", &[]);
        sub.create_edge_with_props(f, d, "E", &[]);

        // Single weak bridge a-d
        sub.create_edge_with_props(a, d, "BRIDGE", &[]);

        sub.flush().unwrap();

        let mut stats = PipelineStats::default();
        run_ldleiden_batch(&sub, &mut stats).unwrap();
        // A well-defined modularity optimum on this graph has exactly
        // two communities (one per triangle); the single bridge edge
        // is not strong enough to merge them.
        assert_eq!(
            stats.communities, 2,
            "expected 2 communities on two-triangle graph, got {}",
            stats.communities
        );

        let mut stats2 = PipelineStats::default();
        run_pagerank_batch(&sub, &mut stats2).unwrap();
        // Every node has at least one in-edge on this graph → every
        // PageRank score is non-zero. We do NOT assert == 6 here
        // because the quantisation floor (1 / 65535) may drop the
        // smallest scores in the `persisted` count; `nonzero` counts
        // f64 values > 0 pre-quantisation, so it must be ≥ 6.
        assert!(
            stats2.pagerank_nonzero >= 6,
            "expected ≥ 6 non-zero PageRank scores, got {}",
            stats2.pagerank_nonzero
        );
    }

    /// P3b — A/B : la variante ricci pondérée est bit-identique à la
    /// variante unitaire quand `ricci_boost = 0.0`, et réutilise la
    /// colonne `ricci_u8` quand le boost est actif.
    #[test]
    fn ldleiden_ricci_boost_zero_equals_unit() {
        use obrain_core::graph::traits::GraphStoreMut;

        let td = TempDir::new().unwrap();
        let path = td.path().join("cog.obrain");
        let sub = Arc::new(SubstrateStore::create(&path).unwrap());
        std::mem::forget(td);

        // Deux triangles reliés par un pont unique (même topologie que le
        // test historique) — partition attendue : 2 communautés.
        let a = sub.create_node_with_props(&["N"], &[]);
        let b = sub.create_node_with_props(&["N"], &[]);
        let c = sub.create_node_with_props(&["N"], &[]);
        sub.create_edge_with_props(a, b, "E", &[]);
        sub.create_edge_with_props(b, c, "E", &[]);
        sub.create_edge_with_props(c, a, "E", &[]);
        let d = sub.create_node_with_props(&["N"], &[]);
        let e = sub.create_node_with_props(&["N"], &[]);
        let f = sub.create_node_with_props(&["N"], &[]);
        sub.create_edge_with_props(d, e, "E", &[]);
        sub.create_edge_with_props(e, f, "E", &[]);
        sub.create_edge_with_props(f, d, "E", &[]);
        sub.create_edge_with_props(a, d, "BRIDGE", &[]);
        sub.flush().unwrap();

        let read_communities = |s: &Arc<SubstrateStore>| -> Vec<u32> {
            let store: &dyn GraphStore = &**s;
            let mut out = Vec::new();
            for nid in store.all_node_ids() {
                let rec = s.writer().read_node(nid.0 as u32).ok().flatten().unwrap();
                out.push(u32::from(rec.community_id));
            }
            out
        };

        // Variante unitaire (chemin historique).
        let mut stats_unit = PipelineStats::default();
        run_ldleiden_batch(&sub, &mut stats_unit).unwrap();
        let part_unit = read_communities(&sub);
        assert_eq!(
            stats_unit.communities, 2,
            "partition historique = 2 communautés"
        );

        // Variante ricci avec boost = 0 : résultats identiques (A/B).
        let mut stats_zero = PipelineStats::default();
        run_ldleiden_batch_ricci(&sub, &mut stats_zero, 0.0).unwrap();
        let part_zero = read_communities(&sub);
        assert_eq!(
            part_zero, part_unit,
            "ricci_boost=0.0 doit reproduire la variante unitaire bit à bit"
        );

        // Boost positif après refresh ricci : le pont (arête unique entre
        // les deux triangles, κ < 0 par construction — deux régions denses
        // reliées par un goulot) est affaibli → les deux triangles restent
        // séparés (2 communautés). Le refresh est nécessaire : sans lui la
        // colonne ricci vaut 0 (poids unitaires partout, boost neutre).
        let ricci_stats = obrain_substrate::refresh_all_ricci(&sub).unwrap();
        assert!(
            ricci_stats.edges_updated >= 7,
            "refresh ricci doit mettre à jour ≥ 7 arêtes (6 intra + 1 pont), got {}",
            ricci_stats.edges_updated
        );

        let mut stats_boost = PipelineStats::default();
        run_ldleiden_batch_ricci(&sub, &mut stats_boost, 1.0).unwrap();
        assert_eq!(
            stats_boost.communities, 2,
            "boost ricci doit préserver les 2 communautés (pont κ<0 affaibli)"
        );
    }
}
