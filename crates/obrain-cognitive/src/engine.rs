//! CognitiveEngine trait and `DefaultCognitiveEngine` implementation.
//!
//! The [`CognitiveEngine`] trait provides unified access to all cognitive
//! subsystems. [`CognitiveEngineBuilder`] constructs a [`DefaultCognitiveEngine`]
//! with selected subsystems and registers their listeners with the reactive
//! [`Scheduler`].

use crate::config::CognitiveConfig;
#[cfg(feature = "kernel")]
use crate::config::KernelConfigToml;
use crate::provenance::ProvenanceRecorder;
#[allow(unused_imports)]
use std::sync::Arc;

#[cfg(feature = "engram")]
use crate::engram::traits::VectorIndex;
#[cfg(feature = "engram")]
use crate::engram::{EngramManager, EngramManagerConfig, EngramMetricsCollector, EngramStore};

// Conditional imports based on feature flags
#[cfg(feature = "energy")]
use crate::energy::{EnergyConfig, EnergyListener, EnergyStore};

#[cfg(feature = "synapse")]
use crate::synapse::{SynapseConfig, SynapseListener, SynapseStore};

#[cfg(feature = "fabric")]
use crate::fabric::{FabricListener, FabricStore};

#[cfg(feature = "decision")]
use crate::decision::DecisionStore;

#[cfg(feature = "co-change")]
use crate::co_change::{CoChangeConfig, CoChangeDetector, CoChangeStore};

#[cfg(feature = "kernel")]
use crate::kernel::KernelListener;
#[cfg(feature = "kernel")]
use obrain_adapters::plugins::algorithms::KernelManager;

use crate::tenant::TenantManager;
use obrain_core::graph::GraphStoreMut;
use obrain_reactive::Scheduler;

// ---------------------------------------------------------------------------
// CognitiveEngine trait
// ---------------------------------------------------------------------------

/// Unified access to all cognitive subsystems.
///
/// Each method returns `Option<&T>` — `None` if the subsystem was not enabled
/// at construction time or its feature flag is not compiled in.
///
/// This trait enables polymorphic access to the cognitive engine without
/// tying consumers to a specific implementation.
pub trait CognitiveEngine: Send + Sync + std::fmt::Debug {
    /// Returns the energy store, if the energy subsystem is active.
    #[cfg(feature = "energy")]
    fn energy_store(&self) -> Option<&Arc<EnergyStore>>;

    /// Returns the synapse store, if the synapse subsystem is active.
    #[cfg(feature = "synapse")]
    fn synapse_store(&self) -> Option<&Arc<SynapseStore>>;

    /// Returns the fabric store, if the fabric subsystem is active.
    #[cfg(feature = "fabric")]
    fn fabric_store(&self) -> Option<&Arc<FabricStore>>;

    /// Returns the decision store, if the decision subsystem is active (DEC-P1).
    #[cfg(feature = "decision")]
    fn decision_store(&self) -> Option<&Arc<DecisionStore>>;

    /// Returns the co-change store, if the co-change subsystem is active.
    #[cfg(feature = "co-change")]
    fn co_change_store(&self) -> Option<&Arc<CoChangeStore>>;

    /// Returns the engram store, if the engram subsystem is active.
    #[cfg(feature = "engram")]
    fn engram_store(&self) -> Option<&Arc<EngramStore>>;

    /// Returns the engram metrics collector, if the engram subsystem is active.
    #[cfg(feature = "engram")]
    fn engram_metrics(&self) -> Option<&Arc<EngramMetricsCollector>>;

    /// Returns the vector index used for spectral signature search, if available.
    #[cfg(feature = "engram")]
    fn vector_index(&self) -> Option<&Arc<dyn VectorIndex>>;

    /// (2026-08-31) Returns the engram manager (coordinateur formation/recall/
    /// FSRS), if the engram subsystem is active. Permet d'alimenter les
    /// épisodes (`record_activation`) et le rappel (`recall(cues, k)`).
    #[cfg(feature = "engram")]
    fn engram_manager(&self) -> Option<&Arc<EngramManager>>;

    /// Returns the number of active subsystems.
    fn active_subsystem_count(&self) -> usize;

    /// Returns the tenant manager for per-tenant graph isolation.
    fn tenant_manager(&self) -> &TenantManager;

    /// Returns the provenance recorder for cognitive event tracking.
    fn provenance(&self) -> &Arc<ProvenanceRecorder>;
}

// ---------------------------------------------------------------------------
// DefaultCognitiveEngine
// ---------------------------------------------------------------------------

/// Default implementation of [`CognitiveEngine`] constructed via the builder.
///
/// Holds `Arc` references to the stores for each enabled subsystem.
/// The corresponding listeners are registered with the scheduler at build time.
pub struct DefaultCognitiveEngine {
    #[cfg(feature = "energy")]
    energy: Option<Arc<EnergyStore>>,

    #[cfg(feature = "synapse")]
    synapse: Option<Arc<SynapseStore>>,

    #[cfg(feature = "fabric")]
    fabric: Option<Arc<FabricStore>>,

    #[cfg(feature = "decision")]
    decision: Option<Arc<DecisionStore>>,

    #[cfg(feature = "co-change")]
    co_change: Option<Arc<CoChangeStore>>,

    #[cfg(feature = "kernel")]
    kernel: Option<Arc<KernelManager>>,

    #[cfg(feature = "engram")]
    engram_store: Option<Arc<EngramStore>>,

    #[cfg(feature = "engram")]
    engram_metrics: Option<Arc<EngramMetricsCollector>>,

    #[cfg(feature = "engram")]
    vector_index: Option<Arc<dyn VectorIndex>>,

    /// (2026-08-31) EngramManager — le coordinateur (formation/recall/FSRS).
    /// Exposé au moteur pour alimenter les épisodes (`record_activation`) et
    /// pour le rappel (`recall(cues, k)`). Rempli par `build()` quand
    /// `with_engrams` a été appelé.
    #[cfg(feature = "engram")]
    engram_manager: Option<Arc<EngramManager>>,

    /// Count of active subsystems.
    active_count: usize,

    /// Per-tenant named graph isolation manager.
    tenant_manager: TenantManager,

    /// Provenance recorder for automatic cognitive event tracking.
    provenance: Arc<ProvenanceRecorder>,
}

impl CognitiveEngine for DefaultCognitiveEngine {
    #[cfg(feature = "energy")]
    fn energy_store(&self) -> Option<&Arc<EnergyStore>> {
        self.energy.as_ref()
    }

    #[cfg(feature = "synapse")]
    fn synapse_store(&self) -> Option<&Arc<SynapseStore>> {
        self.synapse.as_ref()
    }

    #[cfg(feature = "fabric")]
    fn fabric_store(&self) -> Option<&Arc<FabricStore>> {
        self.fabric.as_ref()
    }

    #[cfg(feature = "decision")]
    fn decision_store(&self) -> Option<&Arc<DecisionStore>> {
        self.decision.as_ref()
    }

    #[cfg(feature = "co-change")]
    fn co_change_store(&self) -> Option<&Arc<CoChangeStore>> {
        self.co_change.as_ref()
    }

    #[cfg(feature = "engram")]
    fn engram_store(&self) -> Option<&Arc<EngramStore>> {
        self.engram_store.as_ref()
    }

    #[cfg(feature = "engram")]
    fn engram_metrics(&self) -> Option<&Arc<EngramMetricsCollector>> {
        self.engram_metrics.as_ref()
    }

    #[cfg(feature = "engram")]
    fn vector_index(&self) -> Option<&Arc<dyn VectorIndex>> {
        self.vector_index.as_ref()
    }

    #[cfg(feature = "engram")]
    fn engram_manager(&self) -> Option<&Arc<EngramManager>> {
        self.engram_manager.as_ref()
    }

    fn active_subsystem_count(&self) -> usize {
        self.active_count
    }

    fn tenant_manager(&self) -> &TenantManager {
        &self.tenant_manager
    }

    fn provenance(&self) -> &Arc<ProvenanceRecorder> {
        &self.provenance
    }
}

impl DefaultCognitiveEngine {
    /// Returns the tenant manager for per-tenant named graph isolation.
    ///
    /// Use this to create, delete, list, and switch tenants.
    /// When a tenant is active, cognitive operations should use the
    /// tenant-scoped stores via `tenant_manager().energy_store()` etc.
    pub fn tenants(&self) -> &TenantManager {
        &self.tenant_manager
    }

    /// Returns the provenance recorder for querying cognitive event history.
    pub fn provenance_recorder(&self) -> &Arc<ProvenanceRecorder> {
        &self.provenance
    }

    /// Returns the kernel manager, if the kernel subsystem is active.
    #[cfg(feature = "kernel")]
    pub fn kernel_manager(&self) -> Option<&Arc<KernelManager>> {
        self.kernel.as_ref()
    }

    /// Sets the engram store, metrics collector, and vector index.
    ///
    /// Called by higher-level coordinators (e.g., EngramManager) to wire
    /// the engram subsystem into the cognitive engine for procedure access.
    #[cfg(feature = "engram")]
    pub fn set_engram_subsystem(
        &mut self,
        store: Arc<EngramStore>,
        metrics: Arc<EngramMetricsCollector>,
        vector_index: Arc<dyn VectorIndex>,
    ) {
        self.engram_store = Some(store);
        self.engram_metrics = Some(metrics);
        self.vector_index = Some(vector_index);
    }
}

impl std::fmt::Debug for DefaultCognitiveEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("DefaultCognitiveEngine");
        d.field("active_subsystems", &self.active_count);

        #[cfg(feature = "energy")]
        d.field("energy", &self.energy.is_some());

        #[cfg(feature = "synapse")]
        d.field("synapse", &self.synapse.is_some());

        #[cfg(feature = "fabric")]
        d.field("fabric", &self.fabric.is_some());

        #[cfg(feature = "decision")]
        d.field("decision", &self.decision.is_some());

        #[cfg(feature = "co-change")]
        d.field("co_change", &self.co_change.is_some());

        #[cfg(feature = "kernel")]
        d.field("kernel", &self.kernel.is_some());

        d.field("tenant_count", &self.tenant_manager.tenant_count());
        d.field("provenance_events", &self.provenance.total_events());

        d.finish()
    }
}

// ---------------------------------------------------------------------------
// CognitiveEngineBuilder
// ---------------------------------------------------------------------------

/// Builder for [`DefaultCognitiveEngine`].
///
/// Allows selective activation of cognitive subsystems. When [`build`](Self::build)
/// is called, the corresponding [`MutationListener`](obrain_reactive::MutationListener)s
/// are registered with the provided [`Scheduler`].
///
/// # Example
///
/// ```ignore
/// let engine = CognitiveEngineBuilder::new()
///     .with_energy(EnergyConfig::default())
///     .with_synapses(SynapseConfig::default())
///     .build(&scheduler);
/// ```
pub struct CognitiveEngineBuilder {
    #[cfg(feature = "energy")]
    energy_config: Option<EnergyConfig>,

    #[cfg(feature = "synapse")]
    synapse_config: Option<SynapseConfig>,

    #[cfg(feature = "fabric")]
    fabric_enabled: bool,

    #[cfg(feature = "decision")]
    decision_enabled: bool,

    #[cfg(feature = "co-change")]
    co_change_config: Option<CoChangeConfig>,

    #[cfg(feature = "kernel")]
    kernel_config: Option<KernelConfigToml>,

    /// Optional backing graph store for write-through persistence.
    graph_store: Option<Arc<dyn GraphStoreMut>>,

    /// Optional graph store for kernel embedding manager.
    ///
    /// T17 W2c (2026-04-23): retyped from `Arc<LpgStore>` to
    /// `Arc<dyn GraphStoreMut>` after the subscription-based event loop on
    /// `KernelManager` was removed (dead code). External callers should pass
    /// the substrate-aware handle via `db.graph_store()` when available, to
    /// avoid the historical dummy-LpgStore leak (cf. knowledge_store.rs gotcha).
    #[cfg(feature = "kernel")]
    kernel_store: Option<Arc<dyn GraphStoreMut>>,

    /// Optional substrate handle for column-backed cognitive stores
    /// (T17 cutover — preferred over [`graph_store`] when present).
    ///
    /// When set, `EnergyStore::with_substrate` / `SynapseStore::with_substrate`
    /// / analogous constructors route boost/decay/reinforce through the
    /// substrate Q1.15 / Q0.16 columns + dedicated WAL records instead of
    /// the generic property API. This is the supported path since the T17
    /// substrate cutover.
    #[cfg(feature = "substrate")]
    substrate: Option<Arc<obrain_substrate::SubstrateStore>>,

    /// (2026-08-31) Engram subsystem — Layer 0 (Hebbian + FSRS). When set,
    /// `build()` instantiates the [`EngramManager`] and wires it into the
    /// engine via `set_engram_subsystem`, making the `obrain.engrams.*`
    /// procedures real (they returned empty before — nothing called
    /// `set_engram_subsystem`).
    #[cfg(feature = "engram")]
    engram_config: Option<EngramManagerConfig>,
}

impl CognitiveEngineBuilder {
    /// Creates a new builder with all subsystems disabled.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "energy")]
            energy_config: None,

            #[cfg(feature = "synapse")]
            synapse_config: None,

            #[cfg(feature = "fabric")]
            fabric_enabled: false,

            #[cfg(feature = "decision")]
            decision_enabled: false,

            #[cfg(feature = "co-change")]
            co_change_config: None,

            #[cfg(feature = "kernel")]
            kernel_config: None,

            graph_store: None,

            #[cfg(feature = "kernel")]
            kernel_store: None,

            #[cfg(feature = "substrate")]
            substrate: None,

            #[cfg(feature = "engram")]
            engram_config: None,
        }
    }

    /// Creates a builder from a [`CognitiveConfig`], enabling subsystems
    /// based on their `enabled` flag (and compile-time feature flags).
    pub fn from_config(#[allow(unused_variables)] config: &CognitiveConfig) -> Self {
        #[allow(unused_mut)]
        let mut builder = Self::new();

        #[cfg(feature = "energy")]
        if config.energy.enabled {
            builder.energy_config = Some(config.energy.to_runtime());
        }

        #[cfg(feature = "synapse")]
        if config.synapse.enabled {
            builder.synapse_config = Some(config.synapse.to_runtime());
        }

        #[cfg(feature = "fabric")]
        if config.fabric.enabled {
            builder.fabric_enabled = true;
        }

        #[cfg(feature = "co-change")]
        if config.co_change.enabled {
            builder.co_change_config = Some(config.co_change.to_runtime());
        }

        #[cfg(feature = "kernel")]
        if config.kernel.enabled {
            builder.kernel_config = Some(config.kernel.clone());
        }

        builder
    }

    /// Sets the backing graph store for write-through persistence.
    ///
    /// When set, all cognitive stores will persist their scores as
    /// node/edge properties on the graph. This enables lazy reconstruction
    /// of the hot caches on restart.
    pub fn with_graph_store(mut self, store: Arc<dyn GraphStoreMut>) -> Self {
        self.graph_store = Some(store);
        self
    }

    /// Sets the substrate handle — the T17-preferred backing for cognitive
    /// stores that expose column-native writes (energy / synapse / scar /
    /// utility / affinity).
    ///
    /// When both [`with_graph_store`](Self::with_graph_store) and
    /// [`with_substrate`](Self::with_substrate) are set, the substrate handle
    /// takes precedence for stores that implement `with_substrate`; fabric
    /// and co-change stay on the trait-object path (no substrate variant
    /// yet). For `SynapseStore::with_substrate`, the graph store is still
    /// required for structural edge creation — pass both.
    #[cfg(feature = "substrate")]
    pub fn with_substrate(mut self, store: Arc<obrain_substrate::SubstrateStore>) -> Self {
        self.substrate = Some(store);
        self
    }

    /// Enables the energy subsystem with the given configuration.
    #[cfg(feature = "energy")]
    pub fn with_energy(mut self, config: EnergyConfig) -> Self {
        self.energy_config = Some(config);
        self
    }

    /// Enables the synapse subsystem with the given configuration.
    #[cfg(feature = "synapse")]
    pub fn with_synapses(mut self, config: SynapseConfig) -> Self {
        self.synapse_config = Some(config);
        self
    }

    /// Enables the fabric subsystem.
    #[cfg(feature = "fabric")]
    pub fn with_fabric(mut self) -> Self {
        self.fabric_enabled = true;
        self
    }

    /// Enables the decision subsystem (DEC-P1 calibrated decision registry).
    #[cfg(feature = "decision")]
    pub fn with_decision(mut self) -> Self {
        self.decision_enabled = true;
        self
    }

    /// Enables the co-change detection subsystem with the given configuration.
    #[cfg(feature = "co-change")]
    pub fn with_co_change(mut self, config: CoChangeConfig) -> Self {
        self.co_change_config = Some(config);
        self
    }

    /// Enables the kernel embedding subsystem with the given configuration.
    ///
    /// Requires [`with_kernel_store`](Self::with_kernel_store) to also be
    /// called so that the [`KernelManager`] has a graph store to operate on.
    #[cfg(feature = "kernel")]
    pub fn with_kernel(mut self, config: KernelConfigToml) -> Self {
        self.kernel_config = Some(config);
        self
    }

    /// Sets the graph store (`Arc<dyn GraphStoreMut>`) used by the kernel
    /// embedding manager.
    ///
    /// T17 W2c (2026-04-23): the former `with_lpg_store` alias has been
    /// replaced by this generic accessor. Call alongside
    /// [`with_kernel`](Self::with_kernel). In substrate-backed databases,
    /// pass the handle returned by `ObrainDB::graph_store()` (the
    /// substrate-aware accessor) rather than a dummy `LpgStore` reference,
    /// to avoid the W4.p4.D1.s9 dummy-leak pattern.
    #[cfg(feature = "kernel")]
    pub fn with_kernel_store(mut self, store: Arc<dyn GraphStoreMut>) -> Self {
        self.kernel_store = Some(store);
        self
    }

    /// (2026-08-31) Enables the engram subsystem (Layer 0 — Hebbian + FSRS).
    ///
    /// `build()` instantiates the [`EngramManager`], wires it via
    /// `set_engram_subsystem`, and exposes a `&EngramManager` handle for
    /// episode feeding (`record_activation`) and recall. Before this, the
    /// `obrain.engrams.*` procedures existed but returned empty — nothing
    /// called `set_engram_subsystem`.
    #[cfg(feature = "engram")]
    pub fn with_engrams(mut self, config: EngramManagerConfig) -> Self {
        self.engram_config = Some(config);
        self
    }

    /// Builds the cognitive engine and registers listeners with the scheduler.
    ///
    /// Each enabled subsystem:
    /// 1. Creates a shared store (`Arc<XxxStore>`) — with write-through if a
    ///    graph store was configured via [`with_graph_store`](Self::with_graph_store)
    /// 2. Creates a listener wrapping the store
    /// 3. Registers the listener with the scheduler
    ///
    /// Returns a [`DefaultCognitiveEngine`] holding references to all stores.
    pub fn build(self, #[allow(unused_variables)] scheduler: &Scheduler) -> DefaultCognitiveEngine {
        #[allow(unused_mut)]
        let mut active_count = 0;

        #[allow(unused_variables)]
        let gs = self.graph_store;

        #[cfg(feature = "substrate")]
        #[allow(unused_variables)]
        let sub = self.substrate;

        // Energy — prefer substrate (column-native WAL writes) over
        // graph_store (generic property API) over legacy in-memory.
        #[cfg(feature = "energy")]
        let energy = self.energy_config.map(|config| {
            let store = {
                #[cfg(feature = "substrate")]
                {
                    match (&sub, &gs) {
                        (Some(substrate), _) => {
                            Arc::new(EnergyStore::with_substrate(config, Arc::clone(substrate)))
                        }
                        (None, Some(graph_store)) => Arc::new(EnergyStore::with_graph_store(
                            config,
                            Arc::clone(graph_store),
                        )),
                        (None, None) => Arc::new(EnergyStore::new(config)),
                    }
                }
                #[cfg(not(feature = "substrate"))]
                {
                    match &gs {
                        Some(graph_store) => Arc::new(EnergyStore::with_graph_store(
                            config,
                            Arc::clone(graph_store),
                        )),
                        None => Arc::new(EnergyStore::new(config)),
                    }
                }
            };
            let listener = Arc::new(EnergyListener::new(Arc::clone(&store)));
            scheduler.register_listener(listener);
            active_count += 1;
            #[cfg(feature = "substrate")]
            let routed = if store.is_substrate_backed() {
                "substrate"
            } else if gs.is_some() {
                "graph_store"
            } else {
                "in_memory"
            };
            #[cfg(not(feature = "substrate"))]
            let routed = if gs.is_some() {
                "graph_store"
            } else {
                "in_memory"
            };
            tracing::info!(routed, "cognitive: energy subsystem activated");
            store
        });

        // Synapses — substrate variant requires BOTH substrate (weight
        // column) + graph_store (structural edges).
        #[cfg(feature = "synapse")]
        let synapse = self.synapse_config.map(|config| {
            let store = {
                #[cfg(feature = "substrate")]
                {
                    match (&sub, &gs) {
                        (Some(substrate), Some(graph_store)) => {
                            Arc::new(SynapseStore::with_substrate(
                                config,
                                Arc::clone(graph_store),
                                Arc::clone(substrate),
                            ))
                        }
                        (_, Some(graph_store)) => Arc::new(SynapseStore::with_graph_store(
                            config,
                            Arc::clone(graph_store),
                        )),
                        (_, None) => Arc::new(SynapseStore::new(config)),
                    }
                }
                #[cfg(not(feature = "substrate"))]
                {
                    match &gs {
                        Some(graph_store) => Arc::new(SynapseStore::with_graph_store(
                            config,
                            Arc::clone(graph_store),
                        )),
                        None => Arc::new(SynapseStore::new(config)),
                    }
                }
            };
            let listener = Arc::new(SynapseListener::new(Arc::clone(&store)));
            scheduler.register_listener(listener);
            active_count += 1;
            #[cfg(feature = "substrate")]
            let routed = if sub.is_some() && gs.is_some() {
                "substrate+graph"
            } else if gs.is_some() {
                "graph_store"
            } else {
                "in_memory"
            };
            #[cfg(not(feature = "substrate"))]
            let routed = if gs.is_some() {
                "graph_store"
            } else {
                "in_memory"
            };
            tracing::info!(routed, "cognitive: synapse subsystem activated");
            store
        });

        // Fabric
        #[cfg(feature = "fabric")]
        let fabric = if self.fabric_enabled {
            let store = match &gs {
                Some(graph_store) => {
                    Arc::new(FabricStore::with_graph_store(Arc::clone(graph_store)))
                }
                None => Arc::new(FabricStore::new()),
            };
            let listener = Arc::new(FabricListener::new(Arc::clone(&store)));
            scheduler.register_listener(listener);
            active_count += 1;
            tracing::info!("cognitive: fabric subsystem activated");
            Some(store)
        } else {
            None
        };

        // Decision store (DEC-P1) — passive calibrated-decision registry.
        // Write-through via the generic GraphStoreMut path (no substrate
        // column variant yet); the hot DashMap is the read path.
        #[cfg(feature = "decision")]
        let decision = if self.decision_enabled {
            let store = match &gs {
                Some(graph_store) => {
                    Arc::new(DecisionStore::with_graph_store(Arc::clone(graph_store)))
                }
                None => Arc::new(DecisionStore::new()),
            };
            active_count += 1;
            tracing::info!("cognitive: decision subsystem activated");
            Some(store)
        } else {
            None
        };

        // Co-change
        #[cfg(feature = "co-change")]
        let co_change = self.co_change_config.map(|config| {
            let store = match &gs {
                Some(graph_store) => Arc::new(CoChangeStore::with_graph_store(
                    config,
                    Arc::clone(graph_store),
                )),
                None => Arc::new(CoChangeStore::new(config)),
            };
            let detector = Arc::new(CoChangeDetector::new(Arc::clone(&store)));
            scheduler.register_listener(detector);
            active_count += 1;
            tracing::info!("cognitive: co-change subsystem activated");
            store
        });

        // Kernel embeddings
        #[cfg(feature = "kernel")]
        let kernel = self.kernel_config.and_then(|config| {
            let Some(store) = self.kernel_store else {
                tracing::warn!(
                    "cognitive: kernel subsystem enabled but no graph store provided \
                     (call with_kernel_store). Kernel embeddings will not be active."
                );
                return None;
            };
            let mut manager = KernelManager::new_untrained(Arc::clone(&store), config.seed);
            manager.set_alpha(config.alpha);
            manager.set_max_neighbors(config.max_neighbors);
            manager.debounce_threshold = config.debounce_threshold;
            // Attempt to restore Phi_0 from a previous run persisted in the store.
            // Falls back to random initialization if no weights are found.
            manager.load_phi();
            let manager = Arc::new(manager);
            let listener = Arc::new(KernelListener::new(Arc::clone(&manager)));
            scheduler.register_listener(listener);
            active_count += 1;
            tracing::info!("cognitive: kernel embedding subsystem activated");
            Some(manager)
        });

        // (2026-08-31) ENGRAMS — Layer 0 (Hebbian + FSRS). Instantie le
        // manager et le câble via `set_engram_subsystem` : les procédures
        // `obrain.engrams.*` deviennent réelles (avant, `engram_store` =
        // None → procédures vides). L'index vectoriel en mémoire suffit
        // pour le rappel spectral (le store engram est substrate-routed
        // pour la persistance via les traits VectorIndex).
        #[cfg(feature = "engram")]
        let engram_manager = self.engram_config.map(|config| {
            let index = Arc::new(crate::engram::traits::InMemoryVectorIndex::new());
            let manager = Arc::new(EngramManager::new(config, index));
            active_count += 1;
            tracing::info!("cognitive: engram subsystem activated");
            manager
        });

        let provenance = Arc::new(ProvenanceRecorder::new());

        tracing::info!(
            active = active_count,
            "cognitive engine built with {} active subsystem(s)",
            active_count
        );

        // (2026-08-31) Câble le manager engram dans le moteur construit :
        // `set_engram_subsystem` rend les procédures obrain.engrams.* réelles.
        #[cfg(feature = "engram")]
        let mut engine = DefaultCognitiveEngine {
            #[cfg(feature = "energy")]
            energy,
            #[cfg(feature = "synapse")]
            synapse,
            #[cfg(feature = "fabric")]
            fabric,
            #[cfg(feature = "decision")]
            decision,
            #[cfg(feature = "co-change")]
            co_change,
            #[cfg(feature = "kernel")]
            kernel,
            #[cfg(feature = "engram")]
            engram_store: None,
            #[cfg(feature = "engram")]
            engram_metrics: None,
            #[cfg(feature = "engram")]
            vector_index: None,
            #[cfg(feature = "engram")]
            engram_manager: None,
            active_count,
            tenant_manager: TenantManager::new(),
            provenance,
        };
        #[cfg(feature = "engram")]
        if let Some(ref manager) = engram_manager {
            engine.set_engram_subsystem(
                Arc::clone(manager.store()),
                Arc::clone(manager.metrics()),
                Arc::clone(manager.vector_index_ref()),
            );
            engine.engram_manager = Some(Arc::clone(manager));
        }
        #[cfg(not(feature = "engram"))]
        let engine = DefaultCognitiveEngine {
            #[cfg(feature = "energy")]
            energy,
            #[cfg(feature = "synapse")]
            synapse,
            #[cfg(feature = "fabric")]
            fabric,
            #[cfg(feature = "decision")]
            decision,
            #[cfg(feature = "co-change")]
            co_change,
            #[cfg(feature = "kernel")]
            kernel,
            active_count,
            tenant_manager: TenantManager::new(),
            provenance,
        };
        engine
    }
}

impl Default for CognitiveEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CognitiveEngineBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("CognitiveEngineBuilder");

        #[cfg(feature = "energy")]
        d.field("energy", &self.energy_config.is_some());

        #[cfg(feature = "synapse")]
        d.field("synapse", &self.synapse_config.is_some());

        #[cfg(feature = "fabric")]
        d.field("fabric", &self.fabric_enabled);

        #[cfg(feature = "decision")]
        d.field("decision", &self.decision_enabled);

        #[cfg(feature = "co-change")]
        d.field("co_change", &self.co_change_config.is_some());

        #[cfg(feature = "kernel")]
        d.field("kernel", &self.kernel_config.is_some());

        d.field("graph_store", &self.graph_store.is_some());

        #[cfg(feature = "kernel")]
        d.field("kernel_store", &self.kernel_store.is_some());

        #[cfg(feature = "substrate")]
        d.field("substrate", &self.substrate.is_some());

        d.finish()
    }
}
