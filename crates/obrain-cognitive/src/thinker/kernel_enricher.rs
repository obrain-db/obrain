//! # KernelEnricher — canonical `_kernel_embedding` production (T17l Ph2)
//!
//! Background thinker that computes the 80d kernel embeddings
//! (`KernelManager` → `_kernel_embedding`) for nodes lacking them, using
//! the canonical `_hilbert_features` already on the store. This is the
//! **second phase** of the canonical cognitive feature chain :
//!
//! ```text
//!   HilbertEnricher → _hilbert_features (64-72d)
//!         │
//!         ▼
//!   KernelEnricher  → _kernel_embedding (80d, depends on Hilbert)   ← this
//!         │
//!         ▼
//!   StEnricher      → _st_embedding (384d, ONNX MiniLM, text-based)
//!
//!   CompositeEmbedding pipeline (obrain-chat::retrieval) fuses the
//!   three into a ~532d composite vector for cosine retrieval.
//! ```
//!
//! ## Semantics
//!
//! - **Idempotent** : tick first probes `_kernel_embedding` coverage ; if ≥
//!   threshold it returns a no-op report (the coverage probe short-circuits
//!   in < 1 ms on an enriched base).
//! - **Dependency-aware** : nodes without `_hilbert_features` are silently
//!   skipped by `KernelManager::compute_all` (`extract_neighborhood` yields
//!   `None` → no write). Running KernelEnricher before HilbertEnricher on a
//!   fresh base is therefore a safe no-op for those nodes — no error, no
//!   partial garbage.
//! - **Crash-safe** : every `set_node_property` flows through the substrate
//!   WAL path — partial progress is persisted on crash.
//! - **Bounded** : single tick runs the full-graph kernel computation then
//!   writes ; large bases take minutes per tick (same contract as
//!   [`HilbertEnricher`](super::hilbert_enricher::HilbertEnricher)).
//!
//! ## Dependency chain
//!
//! Lives under `thinker/`, gated by the `enrichment` feature (see
//! `Cargo.toml`), which pulls `obrain-adapters` with the `algos`
//! sub-feature to access [`KernelManager`].

use std::sync::Arc;
use std::time::Duration;

use obrain_adapters::plugins::algorithms::kernel_manager::KernelManager;
use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

use super::{Thinker, ThinkerBudget, ThinkerKind, ThinkerTickError, ThinkerTickReport};

const KERNEL_KEY: &str = "_kernel_embedding";

/// Coverage threshold below which the enrichment tick actually runs.
/// `0.10` : if less than 10% of sampled nodes carry `_kernel_embedding`,
/// recompute. Above that, treat as done.
pub const DEFAULT_COVERAGE_THRESHOLD: f64 = 0.10;

/// Sample size for the coverage probe (mirrors HilbertEnricher — a 1000-node
/// sample statistically distinguishes "not populated" from "populated").
const COVERAGE_SAMPLE_SIZE: usize = 1000;

/// PRNG seed for the untrained `Phi_0` kernel weights — fixed so repeated
/// enrichment of the same base is reproducible.
const KERNEL_SEED: u64 = 42;

/// Runtime configuration for [`KernelEnricher`].
#[derive(Debug, Clone)]
pub struct KernelEnricherConfig {
    /// Per-tick wall-clock + CPU budget.
    pub budget: ThinkerBudget,
    /// Sleep between ticks. Default 600 s so the enricher is low-impact on
    /// a warm base ; the coverage probe fast-exits when already enriched.
    pub interval: Duration,
    /// Coverage threshold (fraction in `[0.0, 1.0]`). See
    /// [`DEFAULT_COVERAGE_THRESHOLD`].
    pub coverage_threshold: f64,
    /// Kernel sampling hyper-parameters (alpha, max neighbors, threads).
    /// `Default` keeps the algorithm defaults ; the seed is pinned to
    /// `KERNEL_SEED` for reproducibility.
    pub alpha: f64,
    /// Maximum sampled neighbors per node for the single-pass attention.
    pub max_neighbors: usize,
    /// Worker threads for the batch kernel computation.
    pub n_threads: usize,
}

impl Default for KernelEnricherConfig {
    fn default() -> Self {
        let n_threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self {
            budget: ThinkerBudget::new(0.10, Duration::from_secs(300)),
            interval: Duration::from_secs(600),
            coverage_threshold: DEFAULT_COVERAGE_THRESHOLD,
            alpha: obrain_adapters::plugins::algorithms::kernel::DEFAULT_ALPHA,
            max_neighbors: obrain_adapters::plugins::algorithms::kernel::DEFAULT_MAX_NEIGHBORS,
            n_threads,
        }
    }
}

/// Accumulated stats since the thinker started.
#[derive(Debug, Clone, Copy, Default)]
pub struct KernelEnricherStats {
    /// Total ticks executed since the thinker started.
    pub ticks: u64,
    /// Ticks that fast-exited because coverage was already above threshold.
    pub ticks_skipped_coverage: u64,
    /// Total nodes that received a `_kernel_embedding` property write.
    pub nodes_written: u64,
    /// Coverage fraction measured by the most recent probe.
    pub last_coverage: f64,
    /// Wall-clock duration of the most recent tick, in milliseconds.
    pub last_tick_elapsed_ms: u64,
}

/// Canonical `_kernel_embedding` producer thinker (T17l phase 2).
pub struct KernelEnricher {
    config: KernelEnricherConfig,
    stats: parking_lot::Mutex<KernelEnricherStats>,
}

impl KernelEnricher {
    /// Builds a KernelEnricher with the given configuration and zeroed stats.
    pub fn new(config: KernelEnricherConfig) -> Self {
        Self {
            config,
            stats: parking_lot::Mutex::new(KernelEnricherStats::default()),
        }
    }

    /// Returns a snapshot of the accumulated stats.
    pub fn stats(&self) -> KernelEnricherStats {
        *self.stats.lock()
    }

    /// Probe the sample coverage on `_kernel_embedding`. Returns the
    /// fraction of sampled nodes that carry a non-empty vector property.
    fn probe_coverage(store: &Arc<SubstrateStore>) -> (f64, usize) {
        let node_ids = store.node_ids();
        if node_ids.is_empty() {
            return (1.0, 0);
        }
        let sample_n = node_ids.len().min(COVERAGE_SAMPLE_SIZE);
        let key = PropertyKey::from(KERNEL_KEY);
        let mut hits = 0usize;
        for nid in node_ids.iter().take(sample_n) {
            if let Some(Value::Vector(_)) = store.get_node_property(*nid, &key) {
                hits += 1;
            }
        }
        ((hits as f64) / (sample_n as f64), sample_n)
    }

    fn tick_impl(
        &self,
        store: &Arc<SubstrateStore>,
    ) -> Result<ThinkerTickReport, ThinkerTickError> {
        let mut report = ThinkerTickReport::start();

        let (coverage, sample_n) = Self::probe_coverage(store);
        {
            let mut s = self.stats.lock();
            s.ticks += 1;
            s.last_coverage = coverage;
        }

        if sample_n == 0 {
            tracing::debug!(kind = "kernel_enricher", "tick skipped — empty store");
            let mut s = self.stats.lock();
            s.ticks_skipped_coverage += 1;
            return Ok(report.finish());
        }

        if coverage >= self.config.coverage_threshold {
            tracing::debug!(
                kind = "kernel_enricher",
                coverage = coverage,
                threshold = self.config.coverage_threshold,
                sample = sample_n,
                "tick skipped — coverage already above threshold"
            );
            let mut s = self.stats.lock();
            s.ticks_skipped_coverage += 1;
            return Ok(report.finish());
        }

        tracing::info!(
            kind = "kernel_enricher",
            coverage = coverage,
            threshold = self.config.coverage_threshold,
            "tick starting — computing canonical _kernel_embedding"
        );

        // Full-graph compute + persist. `compute_all` needs a
        // `GraphStoreMut` ; the substrate store implements it (edition-2024
        // trait upcasting covers the GraphStore read side used internally).
        let erased: Arc<dyn obrain_core::graph::GraphStoreMut> =
            Arc::clone(store) as Arc<dyn obrain_core::graph::GraphStoreMut>;
        let mut km = KernelManager::new_untrained(erased, KERNEL_SEED);
        // Apply config overrides (alpha / neighbors / threads).
        km.set_alpha(self.config.alpha);
        km.set_max_neighbors(self.config.max_neighbors);
        km.set_threads(self.config.n_threads);
        km.compute_all();
        // Cache frais (manager neuf) : embedding_count == nœuds écrits avec
        // succès (nœuds sans `_hilbert_features` → None → non écrits).
        let nodes_written = km.embedding_count() as u64;
        report.nodes_touched = nodes_written;
        report.side_counter = 80; // canonical kernel embedding dimension
        let r = report.finish();
        {
            let mut s = self.stats.lock();
            s.nodes_written += nodes_written;
            s.last_tick_elapsed_ms = r.elapsed.as_millis() as u64;
        }

        tracing::info!(
            kind = "kernel_enricher",
            embeddings = nodes_written,
            elapsed_ms = r.elapsed.as_millis() as u64,
            "tick complete — _kernel_embedding populated"
        );

        Ok(r)
    }
}

impl Thinker for KernelEnricher {
    fn kind(&self) -> ThinkerKind {
        ThinkerKind::KernelEnricher
    }
    fn budget(&self) -> ThinkerBudget {
        self.config.budget
    }
    fn interval(&self) -> Duration {
        self.config.interval
    }
    fn tick(&self, store: &Arc<SubstrateStore>) -> Result<ThinkerTickReport, ThinkerTickError> {
        self.tick_impl(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_core::graph::GraphStoreMut;

    fn fresh_store() -> Arc<SubstrateStore> {
        Arc::new(SubstrateStore::open_tempfile().expect("open tempfile"))
    }

    #[test]
    fn tick_on_empty_store_is_noop() {
        let store = fresh_store();
        let e = KernelEnricher::new(KernelEnricherConfig::default());
        let r = e.tick(&store).expect("tick");
        assert_eq!(r.nodes_touched, 0);
        let s = e.stats();
        assert_eq!(s.ticks, 1);
        assert_eq!(s.ticks_skipped_coverage, 1);
        assert_eq!(s.nodes_written, 0);
    }

    #[test]
    fn tick_skips_when_kernel_embedding_present() {
        let store = fresh_store();
        // Store without hilbert features : kernel can't write (dependency)
        // — tick must not error and must not fabricate embeddings.
        for i in 0..3 {
            store.create_node(&["Node"]);
            let _ = i;
        }
        let e = KernelEnricher::new(KernelEnricherConfig::default());
        let r = e.tick(&store).expect("tick");
        // Coverage probe: nodes have no _kernel_embedding → coverage 0 < 0.1
        // → tick runs ; but without _hilbert_features, compute_all writes
        // nothing (extract_neighborhood → None). nodes_written == 0 is the
        // dependency-aware behaviour.
        assert_eq!(r.nodes_touched, 0);
        let s = e.stats();
        assert_eq!(s.ticks, 1);
        assert_eq!(s.ticks_skipped_coverage, 0, "coverage 0 → tick executes");
    }

    #[test]
    fn tick_produces_kernel_embedding_after_hilbert_enrichment() {
        // Chaîne canonique complète (T17l) : HilbertEnricher d'abord
        // (`_hilbert_features`), puis KernelEnricher (`_kernel_embedding`).
        // KernelManager consomme `_hilbert_features` (centre + voisins) :
        // sans elles, aucun embedding n'est écrit (test précédent) ; avec
        // elles, chaque nœud du graphe reçoit un `_kernel_embedding` 80d.
        use obrain_core::graph::GraphStore as _;
        let store = fresh_store();

        // Petit graphe connexe pour que le kernel ait un voisinage.
        let n: Vec<_> = (0..5).map(|_| store.create_node(&["Node"])).collect();
        store.create_edge(n[0], n[1], "E");
        store.create_edge(n[1], n[2], "E");
        store.create_edge(n[2], n[3], "E");
        store.create_edge(n[3], n[4], "E");
        store.create_edge(n[0], n[2], "E");
        store.create_edge(n[1], n[4], "E");

        // Étape 1 : HilbertEnricher → `_hilbert_features` sur chaque nœud.
        let he = super::super::hilbert_enricher::HilbertEnricher::new(
            super::super::hilbert_enricher::HilbertEnricherConfig::default(),
        );
        let hr = he.tick(&store).expect("hilbert tick");
        assert!(hr.nodes_touched > 0, "hilbert doit enrichir le graphe");

        // Étape 2 : KernelEnricher → `_kernel_embedding` 80d.
        let ke = KernelEnricher::new(KernelEnricherConfig::default());
        let kr = ke.tick(&store).expect("kernel tick");
        assert!(
            kr.nodes_touched > 0,
            "kernel doit produire des embeddings après hilbert"
        );

        let key = PropertyKey::from(KERNEL_KEY);
        for nid in &n {
            match store.get_node_property(*nid, &key) {
                Some(Value::Vector(v)) => {
                    assert!(!v.is_empty(), "embedding non vide");
                }
                other => panic!("node {nid:?} missing _kernel_embedding, got {other:?}"),
            }
        }

        // Second tick : la couverture est maintenant haute → skip.
        let r2 = ke.tick(&store).expect("tick 2");
        assert_eq!(
            r2.nodes_touched, 0,
            "second tick doit être un skip (couverture haute)"
        );
        let s = ke.stats();
        assert_eq!(s.ticks, 2);
        assert_eq!(s.ticks_skipped_coverage, 1, "seul le 2ᵉ tick saute");
    }
}
