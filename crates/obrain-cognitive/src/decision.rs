//! Decision layer — a calibrated, write-through decision registry.
//!
//! DEC-P1 (see `docs/ADR-2026-09-22-COUCHE-DECISION.md`). Obrain already takes
//! decisions everywhere (weights, thresholds, radii) but none of the numbers
//! that carry them is a probability. This module is the *registry*: a
//! [`DecisionStore`] that keeps, for each decision, the options by name and
//! their `(n essais, k succès)` counts, and derives a calibrated probability
//! from a Beta posterior.
//!
//! # Data model (§4)
//!
//! ```text
//! (:Decision)         _cog_decision_question  (string)
//!                     _cog_decision_type      ("choice"|"noul"|"score")
//!                     _cog_decision_options   (liste de strings)
//!                     _cog_decision_dist      (liste de f64)
//!                     _cog_decision_epoch     (f64, secondes UNIX)
//!                     _cog_decision_settled   (0|1)
//!                     _cog_decision_outcome   (string, null tant que non réglée)
//!                     _cog_decision_acted     (string)
//!                     _cog_decision_source    (string)
//! (:DecisionOption)   _cog_decision_n         (u32 essais)
//!                     _cog_decision_k         (u32 succès)
//! (:Decision)-[:DECIDES_ON]->(:DecisionOption)
//! ```
//!
//! # Calibration (D2)
//!
//! The probability is **never** stored as truth — it is recomputed from the
//! counts as a Beta posterior mean with `α = β = 1`:
//!
//! ```text
//! p = (k + α) / (n + α + β) = (k + 1) / (n + 2)
//! ```
//!
//! `Beta(1,1)` returns `0.5` for an option with zero evidence — honest
//! uncertainty, never a false certainty (D2).
//!
//! # Write-through (D3)
//!
//! Like [`crate::fabric::FabricStore`], this store keeps a [`DashMap`] hot
//! cache and, when a backing [`GraphStoreMut`] is provided, persists the
//! `_cog_decision_*` node properties on write (the substrate path preferred by
//! the T17 cutover). u32 counts are stored as `f64` and read back with
//! `.map_or(0, |v| v as u32)` — the established convention of
//! [`crate::store_trait::persist_node_f64`].

use dashmap::DashMap;
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{Direction, GraphStore, GraphStoreMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::store_trait::{OptionalGraphStore, now_epoch_secs, persist_node_f64};

// ---------------------------------------------------------------------------
// Property key constants
// ---------------------------------------------------------------------------

/// `:Decision` property — the question this decision answers.
pub const PROP_DECISION_QUESTION: &str = "_cog_decision_question";
/// `:Decision` property — `"choice" | "noul" | "score"`.
pub const PROP_DECISION_TYPE: &str = "_cog_decision_type";
/// `:Decision` property — the option names (list of strings).
pub const PROP_DECISION_OPTIONS: &str = "_cog_decision_options";
/// `:Decision` property — the calibrated distribution (list of f64), a read
/// cache re-derivable from `n`/`k` (D5).
pub const PROP_DECISION_DIST: &str = "_cog_decision_dist";
/// `:Decision` property — epoch seconds (f64).
pub const PROP_DECISION_EPOCH: &str = "_cog_decision_epoch";
/// `:Decision` property — `0.0` (pending) or `1.0` (settled).
pub const PROP_DECISION_SETTLED: &str = "_cog_decision_settled";
/// `:Decision` property — the outcome (null while unsettled).
pub const PROP_DECISION_OUTCOME: &str = "_cog_decision_outcome";
/// `:Decision` property — the option *actually* applied (may differ from the
/// argmax: policy, human, or exploration).
pub const PROP_DECISION_ACTED: &str = "_cog_decision_acted";
/// `:Decision` property — `"modele" | "politique" | "humain" | "exploration"`.
pub const PROP_DECISION_SOURCE: &str = "_cog_decision_source";
/// `:DecisionOption` property — trial count (u32, stored as f64).
pub const PROP_DECISION_OPTION_N: &str = "_cog_decision_n";
/// `:DecisionOption` property — success count (u32, stored as f64).
pub const PROP_DECISION_OPTION_K: &str = "_cog_decision_k";
/// `:DecisionOption` property — the option name (the registry key, D4).
///
/// Persisted on the option node so a lazy reload (DEC-P1-S3) can re-key counts
/// **by name** (D4) rather than by index or by the traversal order of the
/// `DECIDES_ON` edges — which neither `edges_from` nor the substrate guarantees.
pub const PROP_DECISION_OPTION_NAME: &str = "_cog_decision_option_name";

/// Graph label for decision nodes.
pub const LABEL_DECISION: &str = "Decision";
/// Graph label for per-option count nodes.
pub const LABEL_DECISION_OPTION: &str = "DecisionOption";
/// Edge type linking a decision to its option nodes.
pub const EDGE_DECIDES_ON: &str = "DECIDES_ON";

// ---------------------------------------------------------------------------
// Beta posterior
// ---------------------------------------------------------------------------

/// The Beta posterior mean with `α = β = 1` (Laplace smoothing).
///
/// `p = (k + 1) / (n + 2)`. With no evidence (`k = 0`, `n = 0`) this returns
/// `0.5` — the honest prior (D2).
#[inline]
#[must_use]
pub fn posterior_beta(k: u32, n: u32) -> f64 {
    (k as f64 + 1.0) / (n as f64 + 2.0)
}

/// The variance of the Beta posterior with `α = β = 1`.
///
/// `Var = (k+1)(n-k+1) / ((n+2)² (n+3))`. Used to prove that the credible
/// interval tightens as `n` grows (DEC-T4).
#[must_use]
pub fn posterior_beta_variance(k: u32, n: u32) -> f64 {
    let kf = k as f64;
    let nf = n as f64;
    let a = kf + 1.0;
    let b = nf - kf + 1.0;
    let ab = a + b;
    (a * b) / (ab * ab * (ab + 1.0))
}

// ---------------------------------------------------------------------------
// Decision / DecisionOption records
// ---------------------------------------------------------------------------

/// A registered decision (the `:Decision` node content).
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The question this decision answers (the registry key).
    pub question: String,
    /// `"choice" | "noul" | "score"`.
    pub decision_type: String,
    /// The option names, in order.
    pub options: Vec<String>,
    /// Epoch seconds when the decision was registered.
    pub epoch: f64,
    /// Whether an outcome has been recorded.
    pub settled: bool,
    /// The recorded outcome (if settled).
    pub outcome: Option<String>,
    /// The option actually applied (may differ from the argmax).
    pub acted: Option<String>,
    /// The origin of the decision (`"modele" | "politique" | "humain" | "exploration"`).
    pub source: Option<String>,
}

/// Per-option trial/success counts (the `:DecisionOption` node content).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecisionOption {
    /// Trial count `n`.
    pub n: u32,
    /// Success count `k`.
    pub k: u32,
}

// ---------------------------------------------------------------------------
// DecisionStore
// ---------------------------------------------------------------------------

/// Thread-safe store for decisions and their per-option counts.
///
/// Uses [`DashMap`] for concurrent read/write. When a backing
/// [`GraphStoreMut`] is provided, the `:Decision` / `:DecisionOption` nodes and
/// their `_cog_decision_*` properties are persisted (write-through).
pub struct DecisionStore {
    /// Hot cache: question → [`Decision`].
    decisions: DashMap<String, Decision>,
    /// Hot cache: `question\u{0}option` → [`DecisionOption`] counts.
    options: DashMap<String, DecisionOption>,
    /// Mapping: question → NodeId of the `:Decision` node.
    decision_node_ids: DashMap<String, NodeId>,
    /// Mapping: `question\u{0}option` → NodeId of the `:DecisionOption` node.
    option_node_ids: DashMap<String, NodeId>,
    /// Optional backing graph store for write-through persistence.
    graph_store: OptionalGraphStore,
    /// DEC-P1-S3 — whether the lazy reload from the substrate has already run.
    /// Set exactly once, on first access, so a warm store never re-scans.
    reloaded: AtomicBool,
}

/// Composite key for an option under a question. The NUL separator cannot
/// collide with a question/option name (both are user strings with no NUL).
fn option_key(question: &str, option: &str) -> String {
    format!("{question}\u{0}{option}")
}

impl DecisionStore {
    /// Creates a new, empty decision store (in-memory only).
    pub fn new() -> Self {
        Self {
            decisions: DashMap::new(),
            options: DashMap::new(),
            decision_node_ids: DashMap::new(),
            option_node_ids: DashMap::new(),
            graph_store: None,
            reloaded: AtomicBool::new(false),
        }
    }

    /// Creates a new decision store with write-through persistence.
    pub fn with_graph_store(graph_store: Arc<dyn GraphStoreMut>) -> Self {
        Self {
            decisions: DashMap::new(),
            options: DashMap::new(),
            decision_node_ids: DashMap::new(),
            option_node_ids: DashMap::new(),
            graph_store: Some(graph_store),
            reloaded: AtomicBool::new(false),
        }
    }

    /// DEC-P1-S3 — lazy reload from the substrate, performed at most once.
    ///
    /// On the first access to the hot cache, if the backing graph store is
    /// wired **and** the cache is still empty (fresh process, or a cache that
    /// was evicted), the `:Decision` / `:DecisionOption` nodes are re-read from
    /// the substrate and the `DashMap`s repopulated. The [`AtomicBool`]
    /// `reloaded` flag makes this a one-shot: a warm store never re-scans.
    ///
    /// A benign race on concurrent first access may reload more than once; the
    /// inserts are idempotent and each individual read stays atomic through
    /// `DashMap`. The flag is set only *after* the reload so a concurrent
    /// caller re-reads the substrate instead of skipping it and observing an
    /// empty cache (which could otherwise make `register` duplicate a node).
    fn ensure_reloaded(&self) {
        if self.reloaded.load(Ordering::Acquire) {
            return;
        }
        let Some(gs) = self.graph_store.as_ref() else {
            // No backing store — nothing to reload, ever.
            self.reloaded.store(true, Ordering::Release);
            return;
        };
        // Only reload when the cache is empty — if this process already
        // registered decisions, the cache is authoritative.
        if self.decisions.is_empty() {
            self.reload_from_substrate(gs.as_ref());
        }
        self.reloaded.store(true, Ordering::Release);
    }

    /// Reads every `:Decision` node (and its `:DecisionOption` children via the
    /// `DECIDES_ON` edges) back into the hot cache. Read-only: never mutates
    /// the graph.
    fn reload_from_substrate(&self, gs: &dyn GraphStore) {
        for nid in gs.node_ids() {
            let Some(node) = gs.get_node(nid) else {
                continue;
            };
            if !node.has_label(LABEL_DECISION) {
                continue;
            }

            let Some(question) = node
                .get_property(PROP_DECISION_QUESTION)
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let question = question.to_string();

            let decision_type = node
                .get_property(PROP_DECISION_TYPE)
                .and_then(|v| v.as_str())
                .unwrap_or("choice")
                .to_string();
            let options: Vec<String> = node
                .get_property(PROP_DECISION_OPTIONS)
                .and_then(|v| v.as_list())
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let epoch = node
                .get_property(PROP_DECISION_EPOCH)
                .and_then(|v| v.as_float64())
                .unwrap_or(0.0);
            let settled = node
                .get_property(PROP_DECISION_SETTLED)
                .and_then(|v| v.as_float64())
                .is_some_and(|f| f >= 0.5);
            let outcome = node
                .get_property(PROP_DECISION_OUTCOME)
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let acted = node
                .get_property(PROP_DECISION_ACTED)
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let source = node
                .get_property(PROP_DECISION_SOURCE)
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let decision = Decision {
                question: question.clone(),
                decision_type,
                options: options.clone(),
                epoch,
                settled,
                outcome,
                acted,
                source,
            };
            self.decisions.insert(question.clone(), decision);
            self.decision_node_ids.insert(question.clone(), nid);

            // Re-key the option counts by name (D4) via the DECIDES_ON edges.
            for (oid, edge_id) in gs.edges_from(nid, Direction::Outgoing) {
                if gs.edge_type(edge_id).as_deref() != Some(EDGE_DECIDES_ON) {
                    continue;
                }
                let Some(onode) = gs.get_node(oid) else {
                    continue;
                };
                if !onode.has_label(LABEL_DECISION_OPTION) {
                    continue;
                }
                let n = onode
                    .get_property(PROP_DECISION_OPTION_N)
                    .and_then(|v| v.as_float64())
                    .map_or(0, |f| f as u32);
                let k = onode
                    .get_property(PROP_DECISION_OPTION_K)
                    .and_then(|v| v.as_float64())
                    .map_or(0, |f| f as u32);
                let Some(name) = onode
                    .get_property(PROP_DECISION_OPTION_NAME)
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                let key = option_key(&question, name);
                self.option_node_ids.insert(key.clone(), oid);
                self.options.insert(key, DecisionOption { n, k });
            }
        }
    }

    /// Registers a decision if absent, and returns its `:Decision` NodeId.
    ///
    /// Idempotent: a second call with the same `question` returns the existing
    /// node without touching the store. Options are identified **by name**
    /// (D4); they are created once at registration and never re-keyed by index.
    ///
    /// When backed by a graph store, this creates the `:Decision` node, one
    /// `:DecisionOption` node per option, the `DECIDES_ON` edges, and the
    /// `_cog_decision_*` properties (initial dist = `0.5` per option).
    /// With no graph store, the hot cache is authoritative and the returned
    /// id is `NodeId(0)`.
    pub fn register(&self, question: &str, decision_type: &str, options: &[String]) -> NodeId {
        self.ensure_reloaded();
        if let Some(id) = self.decision_node_ids.get(question) {
            return *id;
        }

        // DEC-C8 — get-or-create ATOMIQUE. `entry().or_insert_with` tient le
        // verrou de shard du DashMap (indexé par `question`) pendant toute la
        // création, donc deux `register` concurrents sur la MÊME question ne
        // peuvent plus passer tous deux le fast-path miss et dupliquer le
        // `:Decision`, ses `:DecisionOption` et ses arêtes `DECIDES_ON` dans le
        // substrat. L'ancien `get` + `insert` (check-then-act) n'était
        // idempotent que séquentiellement.
        let node_id = *self
            .decision_node_ids
            .entry(question.to_string())
            .or_insert_with(|| {
                let epoch = now_epoch_secs();
                let decision = Decision {
                    question: question.to_string(),
                    decision_type: decision_type.to_string(),
                    options: options.to_vec(),
                    epoch,
                    settled: false,
                    outcome: None,
                    acted: None,
                    source: None,
                };

                let id = if let Some(gs) = &self.graph_store {
                    let id = gs.create_node(&[LABEL_DECISION]);
                    gs.set_node_property(id, PROP_DECISION_QUESTION, Value::from(question));
                    gs.set_node_property(id, PROP_DECISION_TYPE, Value::from(decision_type));
                    gs.set_node_property(id, PROP_DECISION_OPTIONS, Value::from(options.to_vec()));
                    // Initial dist: Beta(1,1) = 0.5 for every option — honest, re-derivable (D5).
                    let initial_dist: Vec<f64> = vec![0.5; options.len()];
                    gs.set_node_property(id, PROP_DECISION_DIST, Value::from(initial_dist));
                    gs.set_node_property(id, PROP_DECISION_EPOCH, Value::from(epoch));
                    gs.set_node_property(id, PROP_DECISION_SETTLED, Value::from(0.0_f64));

                    for option in options {
                        let oid = gs.create_node(&[LABEL_DECISION_OPTION]);
                        gs.set_node_property(oid, PROP_DECISION_OPTION_N, Value::from(0.0_f64));
                        gs.set_node_property(oid, PROP_DECISION_OPTION_K, Value::from(0.0_f64));
                        // D4 — persist the option name so a lazy reload can re-key the
                        // counts by name (see `reload_from_substrate`).
                        gs.set_node_property(
                            oid,
                            PROP_DECISION_OPTION_NAME,
                            Value::from(option.as_str()),
                        );
                        gs.create_edge(id, oid, EDGE_DECIDES_ON);
                        let key = option_key(question, option);
                        self.option_node_ids.insert(key.clone(), oid);
                        self.options.insert(key, DecisionOption::default());
                    }
                    id
                } else {
                    NodeId(0)
                };

                self.decisions.insert(question.to_string(), decision);
                id
            });
        node_id
    }

    /// Returns the `:Decision` NodeId for a question, if registered.
    pub fn decision_node_id(&self, question: &str) -> Option<NodeId> {
        self.ensure_reloaded();
        self.decision_node_ids.get(question).map(|id| *id)
    }

    /// Returns the `:DecisionOption` NodeId for `(question, option)`, if any.
    pub fn option_node_id(&self, question: &str, option: &str) -> Option<NodeId> {
        self.ensure_reloaded();
        self.option_node_ids
            .get(&option_key(question, option))
            .map(|id| *id)
    }

    /// Returns the [`Decision`] record for a question, if registered.
    pub fn get(&self, question: &str) -> Option<Decision> {
        self.ensure_reloaded();
        self.decisions.get(question).map(|entry| entry.clone())
    }

    /// Returns the `(n, k)` counts for an option, if known.
    pub fn option_counts(&self, question: &str, option: &str) -> Option<DecisionOption> {
        self.ensure_reloaded();
        self.options
            .get(&option_key(question, option))
            .map(|entry| *entry)
    }

    /// The calibrated distribution over `options`, in their given order.
    ///
    /// Read-only: each entry is [`posterior_beta`] of the option's counts, or
    /// `0.5` for an option with no accumulated evidence.
    pub fn distribution(&self, question: &str, options: &[String]) -> Vec<f64> {
        self.ensure_reloaded();
        options
            .iter()
            .map(|option| match self.option_counts(question, option) {
                Some(counts) => posterior_beta(counts.k, counts.n),
                None => 0.5,
            })
            .collect()
    }

    /// Records a settled outcome (the `settle` side of DEC-P1).
    ///
    /// Increments `n` for `option`, and `k` when `success` is `true`; marks the
    /// decision as settled with `outcome`/`acted` = `option`, and refreshes the
    /// cached distribution. With a backing store, the option counts and the
    /// decision's dist/settled/outcome/acted properties are persisted.
    ///
    /// DEC-C5 — version minimale : écriture **directe** et ordonnée. L'incrément
    /// `(n, k)` et le write-through `persist_node_f64` sont faits sous le même
    /// verrou de shard du DashMap (indexé par `question\u{0}option`) : deux
    /// `settle` concurrents sur la même option ne peuvent pas persister leurs
    /// comptes dans le désordre, et le compteur n'est jamais lu hors du verrou
    /// en instantané périmé.
    pub fn enregistrer(&self, question: &str, option: &str, success: bool) {
        self.ensure_reloaded();
        let key = option_key(question, option);

        // RMW atomique : le RefMut de `entry().or_insert_with` tient le verrou
        // de shard jusqu'à la fin du bloc, donc l'incrément ET les deux
        // `persist_node_f64` sont ordonnés ensemble.
        {
            let mut counts = self
                .options
                .entry(key.clone())
                .or_insert_with(DecisionOption::default);
            counts.n += 1;
            if success {
                counts.k += 1;
            }
            let n = counts.n;
            let k = counts.k;

            if let Some(gs) = &self.graph_store
                && let Some(oid) = self.option_node_ids.get(&key)
            {
                let oid = *oid;
                // Write-through DURABLE : n/k, source de vérité du postérieur
                // (les perdre est la perte asymétrique PI5).
                persist_node_f64(gs.as_ref(), oid, PROP_DECISION_OPTION_N, n as f64);
                persist_node_f64(gs.as_ref(), oid, PROP_DECISION_OPTION_K, k as f64);
            }
        }

        if let Some(mut decision) = self.decisions.get_mut(question) {
            decision.settled = true;
            decision.outcome = Some(option.to_string());
            decision.acted = Some(option.to_string());
        }

        self.persist_decision_state(question);
    }

    /// Number of registered decisions.
    fn persist_decision_state(&self, question: &str) {
        let Some(gs) = &self.graph_store else {
            return;
        };
        let Some(decision) = self.decisions.get(question) else {
            return;
        };
        let Some(nid) = self.decision_node_ids.get(question) else {
            return;
        };
        let nid = *nid;

        let dist = self.distribution(question, &decision.options);
        gs.set_node_property(nid, PROP_DECISION_DIST, Value::from(dist));
        gs.set_node_property(
            nid,
            PROP_DECISION_SETTLED,
            Value::from(if decision.settled { 1.0_f64 } else { 0.0_f64 }),
        );
        if let Some(outcome) = &decision.outcome {
            gs.set_node_property(nid, PROP_DECISION_OUTCOME, Value::from(outcome.as_str()));
        }
        if let Some(acted) = &decision.acted {
            gs.set_node_property(nid, PROP_DECISION_ACTED, Value::from(acted.as_str()));
        }
        if let Some(source) = &decision.source {
            gs.set_node_property(nid, PROP_DECISION_SOURCE, Value::from(source.as_str()));
        }
    }

    /// Number of registered decisions.
    pub fn len(&self) -> usize {
        self.decisions.len()
    }

    /// Whether no decision is registered.
    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
    }
}

impl Default for DecisionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DecisionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionStore")
            .field("decisions", &self.decisions.len())
            .field("options", &self.options.len())
            .field("backed", &self.graph_store.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// DEC-T4 — `beta_naif_et_convergence`.
    ///
    /// `Beta(1,1)` returns `0.5` with no evidence; `k/n → p` converges; the
    /// credible interval tightens as `n` grows at a fixed success ratio.
    #[test]
    fn beta_naif_et_convergence() {
        // Beta(1,1) on zero evidence is the honest 0.5 prior.
        assert_eq!(posterior_beta(0, 0), 0.5);

        // k/n → p converges: 90/100 → (90+1)/(100+2) ≈ 0.892.
        let p = posterior_beta(90, 100);
        assert!(
            (p - 0.89).abs() < 0.01,
            "posterior_beta(90, 100) = {p}, expected ~0.89"
        );

        // The interval tightens: same ratio (9/10 vs 90/100), smaller variance.
        let std_small_n = posterior_beta_variance(9, 10).sqrt();
        let std_large_n = posterior_beta_variance(90, 100).sqrt();
        assert!(
            std_large_n < std_small_n,
            "interval should tighten: std(n=100)={std_large_n} !< std(n=10)={std_small_n}"
        );
    }

    /// DEC-T1 — `decision_store_roundtrip`.
    ///
    /// Write a decision, re-read it from the DashMap, compare `n`/`k`.
    #[test]
    fn decision_store_roundtrip() {
        let store = DecisionStore::new();
        let options = vec!["billing".to_string(), "technical".to_string()];
        store.register("triage.department", "choice", &options);

        store.enregistrer("triage.department", "billing", true);
        store.enregistrer("triage.department", "billing", false);

        // Re-read from the hot cache.
        let counts = store
            .option_counts("triage.department", "billing")
            .expect("option must be registered");
        assert_eq!(counts.n, 2, "two trials recorded");
        assert_eq!(counts.k, 1, "one success recorded");

        let decision = store
            .get("triage.department")
            .expect("decision must be registered");
        assert!(decision.settled, "decision is settled after enregistrer");
        assert_eq!(decision.outcome.as_deref(), Some("billing"));
        assert_eq!(decision.acted.as_deref(), Some("billing"));

        // Distribution round-trips through the same counts: billing (1+1)/(2+2)=0.5,
        // technical (0+1)/(0+2)=0.5.
        let dist = store.distribution("triage.department", &options);
        assert_eq!(dist.len(), 2);
        assert!((dist[0] - 0.5).abs() < 1e-9, "billing dist = {}", dist[0]);
        assert!((dist[1] - 0.5).abs() < 1e-9, "technical dist = {}", dist[1]);
    }

    /// DEC-T4b — `decide_prior_uniforme`.
    ///
    /// `posterior_beta(0, 0)` is the honest Beta(1,1) prior `0.5` — the exact
    /// number `obrain.decide` returns for an option with no evidence.
    #[test]
    fn decide_prior_uniforme() {
        assert_eq!(posterior_beta(0, 0), 0.5);
    }

    /// DEC-T4c — `decide_convergence`.
    ///
    /// `k/n → p` converges (`90/100 → ~0.89`) and the credible interval
    /// tightens as `n` grows (`n=2` vs `n=100`).
    #[test]
    fn decide_convergence() {
        let p = posterior_beta(90, 100);
        assert!(
            (p - 0.89).abs() < 0.01,
            "posterior_beta(90, 100) = {p}, expected ~0.89"
        );

        let std_small_n = posterior_beta_variance(1, 2).sqrt();
        let std_large_n = posterior_beta_variance(90, 100).sqrt();
        assert!(
            std_large_n < std_small_n,
            "interval should tighten: std(n=100)={std_large_n} !< std(n=2)={std_small_n}"
        );
    }

    /// DEC-C5 — `settle_ecriture_directe`.
    ///
    /// `enregistrer` fait un write-through **direct** (pas de file différée) :
    /// l'incrément `(n, k)` est persisté immédiatement dans le substrat, et les
    /// comptes se relisent identiques du cache chaud ET des propriétés du nœud
    /// `:DecisionOption`.
    #[test]
    fn settle_ecriture_directe() {
        use obrain_common::types::PropertyKey;
        use obrain_substrate::SubstrateStore;

        let gs = Arc::new(SubstrateStore::open_tempfile().expect("open_tempfile"));
        let store = DecisionStore::with_graph_store(Arc::clone(&gs) as Arc<dyn GraphStoreMut>);
        let options = vec!["billing".to_string(), "technical".to_string()];
        store.register("triage.department", "choice", &options);

        store.enregistrer("triage.department", "billing", true);
        store.enregistrer("triage.department", "billing", false);

        // n/k re-read correctly from the hot cache.
        let counts = store
            .option_counts("triage.department", "billing")
            .expect("option must be registered");
        assert_eq!(counts.n, 2);
        assert_eq!(counts.k, 1);

        // ... and from the persisted `:DecisionOption` node properties — the
        // write-through was direct (no deferred flush needed).
        let oid = store
            .option_node_id("triage.department", "billing")
            .expect("option node id");
        let n = gs
            .get_node_property(oid, &PropertyKey::from(PROP_DECISION_OPTION_N))
            .and_then(|v| v.as_float64())
            .expect("persisted n");
        let k = gs
            .get_node_property(oid, &PropertyKey::from(PROP_DECISION_OPTION_K))
            .and_then(|v| v.as_float64())
            .expect("persisted k");
        assert_eq!(n, 2.0);
        assert_eq!(k, 1.0);
    }

    /// DEC-P1-S3 — `rechargement_paresseux`.
    ///
    /// A second `DecisionStore` over the same substrate re-reads the
    /// `:Decision`/`:DecisionOption` nodes on first access (re-keying the
    /// counts **by name**, D4) and re-derives the same counts and distribution
    /// without re-registering.
    #[test]
    fn rechargement_paresseux() {
        use obrain_substrate::SubstrateStore;

        let gs = Arc::new(SubstrateStore::open_tempfile().expect("open_tempfile"));
        let options = vec!["billing".to_string(), "technical".to_string()];
        {
            let store = DecisionStore::with_graph_store(Arc::clone(&gs) as Arc<dyn GraphStoreMut>);
            store.register("triage.department", "choice", &options);
            store.enregistrer("triage.department", "billing", true);
        }

        // Fresh store over the same substrate — must reload lazily.
        let store = DecisionStore::with_graph_store(Arc::clone(&gs) as Arc<dyn GraphStoreMut>);
        let counts = store
            .option_counts("triage.department", "billing")
            .expect("reloaded option");
        assert_eq!(counts.n, 1);
        assert_eq!(counts.k, 1);

        // Re-derived distribution, not the persisted cache (D5).
        let dist = store.distribution("triage.department", &options);
        assert!(
            (dist[0] - posterior_beta(1, 1)).abs() < 1e-9,
            "dist[0] = {}",
            dist[0]
        );
    }
}
