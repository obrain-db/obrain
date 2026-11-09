//! Lightweight handles for database interaction.
//!
//! A session is your conversation with the database. Each session can have
//! its own transaction state, so concurrent sessions don't interfere with
//! each other. Sessions are cheap to create - spin up as many as you need.

#[cfg(feature = "rdf")]
mod rdf;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use web_time::Instant;

use obrain_common::types::{EdgeId, EpochId, NodeId, TransactionId, Value};
use obrain_common::utils::error::Result;
use obrain_core::graph::Direction;
use obrain_core::graph::GraphStoreMut;
use obrain_core::graph::lpg::{Edge, Node};
#[cfg(feature = "rdf")]
use obrain_core::graph::rdf::RdfStore;

use crate::catalog::{Catalog, CatalogConstraintValidator};
use crate::config::{AdaptiveConfig, GraphModel};
use crate::database::QueryResult;
use crate::query::cache::QueryCache;
use crate::transaction::TransactionManager;

/// Parses a DDL default-value literal string into a [`Value`].
///
/// Handles string literals (single- or double-quoted), integers, floats,
/// booleans (`true`/`false`), and `NULL`.
fn parse_default_literal(text: &str) -> Value {
    if text.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    if text.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    // String literal: strip surrounding quotes
    if (text.starts_with('\'') && text.ends_with('\''))
        || (text.starts_with('"') && text.ends_with('"'))
    {
        return Value::String(text[1..text.len() - 1].into());
    }
    // Try integer, then float
    if let Ok(i) = text.parse::<i64>() {
        return Value::Int64(i);
    }
    if let Ok(f) = text.parse::<f64>() {
        return Value::Float64(f);
    }
    // Fallback: treat as string
    Value::String(text.into())
}

/// Runtime configuration for creating a new session.
///
/// Groups the shared parameters passed to all session constructors, keeping
/// call sites readable and avoiding long argument lists.
pub(crate) struct SessionConfig {
    pub transaction_manager: Arc<TransactionManager>,
    pub query_cache: Arc<QueryCache>,
    pub catalog: Arc<Catalog>,
    pub adaptive_config: AdaptiveConfig,
    pub factorized_execution: bool,
    pub graph_model: GraphModel,
    pub query_timeout: Option<Duration>,
    pub commit_counter: Arc<AtomicUsize>,
    pub gc_interval: usize,
    /// When true, the session permanently blocks all mutations.
    pub read_only: bool,
}

/// Your handle to the database - execute queries and manage transactions.
///
/// Get one from [`ObrainDB::session()`](crate::ObrainDB::session). Each session
/// tracks its own transaction state, so you can have multiple concurrent
/// sessions without them interfering.
pub struct Session {
    /// Graph store trait object for pluggable storage backends.
    ///
    /// T17 Step 24 (2026-04-23): the previous `store: Arc<LpgStore>`
    /// field was deleted. MVCC + named-graph bookkeeping now dispatch
    /// entirely through this `Arc<dyn GraphStoreMut>` handle (the
    /// trait methods default to substrate-compatible no-ops; LpgStore
    /// overrides delegate to its inherent impls).
    graph_store: Arc<dyn GraphStoreMut>,
    /// Schema and metadata catalog shared across sessions.
    catalog: Arc<Catalog>,
    /// RDF triple store (if RDF feature is enabled).
    #[cfg(feature = "rdf")]
    rdf_store: Arc<RdfStore>,
    /// Transaction manager.
    transaction_manager: Arc<TransactionManager>,
    /// Query cache shared across sessions.
    query_cache: Arc<QueryCache>,
    /// Current transaction ID (if any). Behind a Mutex so that GQL commands
    /// (`START TRANSACTION`, `COMMIT`, `ROLLBACK`) can manage transactions
    /// from within `execute(&self)`.
    current_transaction: parking_lot::Mutex<Option<TransactionId>>,
    /// Whether the current transaction is read-only (blocks mutations).
    read_only_tx: parking_lot::Mutex<bool>,
    /// Whether the database itself is read-only (set at open time, never changes).
    /// When true, `read_only_tx` is always true regardless of transaction flags.
    db_read_only: bool,
    /// Whether the session is in auto-commit mode.
    auto_commit: bool,
    /// Adaptive execution configuration.
    #[allow(dead_code)] // Stored for future adaptive re-optimization during execution
    adaptive_config: AdaptiveConfig,
    /// Whether to use factorized execution for multi-hop queries.
    factorized_execution: bool,
    /// (2026-08-17, aretes inter-bases V2) Registre multi-store de la
    /// session : `{slug -> store}`, ouverture paresseuse. `None` =
    /// session mono-base (comportement historique). Pose par
    /// `ObrainDB::session()` quand la base a des bases attachees.
    base_registry: parking_lot::Mutex<Option<Arc<obrain_core::execution::BaseRegistry>>>,
    /// (V3) Contexte de droits du LECTEUR de cette session — voyage dans
    /// SessionContext, evalue a chaque franchissement de frontiere.
    /// `None` = acces total (comportement historique).
    access_context: parking_lot::Mutex<Option<Arc<obrain_core::execution::AccessContext>>>,
    /// (V3) Vrai pendant une transaction AUTO-COMMIT (posee par
    /// with_auto_commit) : permet de distinguer une transaction EXPLICITE
    /// (START TRANSACTION) d'un simple auto-commit, alors que
    /// `current_transaction` est Some dans les deux cas.
    transaction_auto: std::sync::atomic::AtomicBool,
    /// The graph data model this session operates on.
    graph_model: GraphModel,
    /// Maximum time a query may run before being cancelled.
    query_timeout: Option<Duration>,
    /// Shared commit counter for triggering auto-GC.
    commit_counter: Arc<AtomicUsize>,
    /// GC every N commits (0 = disabled).
    gc_interval: usize,
    /// Node count at the start of the current transaction (for PreparedCommit stats).
    transaction_start_node_count: AtomicUsize,
    /// Edge count at the start of the current transaction (for PreparedCommit stats).
    transaction_start_edge_count: AtomicUsize,
    /// Optional cognitive engine (3 piliers : causal spreading activation,
    /// gap detection, dynamic synthesis) shared with the owning `ObrainDB`.
    /// Wired via `set_cognitive_engine` right after session creation (same
    /// pattern as `set_wal`/`set_cdc_log`) so `CALL obrain.cognitive.*`
    /// procedures can resolve `Planner::cognitive_engine` at query-plan time.
    #[cfg(feature = "cognitive")]
    cognitive_engine: Option<Arc<dyn obrain_cognitive::CognitiveEngine>>,
    /// Typed substrate handle, shared with the owning `ObrainDB`. Wired via
    /// `set_substrate_store` right after session creation (same pattern as
    /// the cognitive engine above) so substrate-native procedures
    /// (`CALL obrain.window_2d`) can resolve at query-plan time.
    substrate_store: Option<Arc<obrain_substrate::SubstrateStore>>,
    /// WAL for logging schema changes.
    #[cfg(feature = "wal")]
    wal: Option<Arc<obrain_adapters::storage::wal::LpgWal>>,
    /// Shared WAL graph context tracker for named graph awareness.
    #[cfg(feature = "wal")]
    wal_graph_context: Option<Arc<parking_lot::Mutex<Option<String>>>>,
    /// Journal WAL of the RDF store (persistent substrate databases only).
    #[cfg(all(feature = "rdf", feature = "wal"))]
    rdf_wal: Option<Arc<obrain_adapters::storage::wal::LpgWal>>,
    /// CDC log for change tracking.
    #[cfg(feature = "cdc")]
    cdc_log: Arc<crate::cdc::CdcLog>,
    /// Current graph name (for multi-graph USE GRAPH support). None = default graph.
    current_graph: parking_lot::Mutex<Option<String>>,
    /// Current schema name (ISO/IEC 39075 Section 4.7.3: independent from session graph).
    /// None = "not set" (uses default schema).
    current_schema: parking_lot::Mutex<Option<String>>,
    /// Session time zone override.
    time_zone: parking_lot::Mutex<Option<String>>,
    /// Session-level parameters (SET PARAMETER).
    session_params:
        parking_lot::Mutex<std::collections::HashMap<String, obrain_common::types::Value>>,
    /// Override epoch for time-travel queries (None = use transaction/current epoch).
    viewing_epoch_override: parking_lot::Mutex<Option<EpochId>>,
    /// Savepoints within the current transaction.
    savepoints: parking_lot::Mutex<Vec<SavepointState>>,
    /// Nesting depth for nested transactions (0 = outermost).
    /// Nested `START TRANSACTION` creates an auto-savepoint; nested `COMMIT`
    /// releases it, nested `ROLLBACK` rolls back to it.
    transaction_nesting_depth: parking_lot::Mutex<u32>,
    /// Named graphs touched during the current transaction (for cross-graph atomicity).
    /// `None` represents the default graph. Populated at `BEGIN` time and on each
    /// `USE GRAPH` / `SESSION SET GRAPH` switch within a transaction.
    touched_graphs: parking_lot::Mutex<Vec<Option<String>>>,
    /// Empreinte de chaque graphe au moment ou la transaction courante l'a
    /// touche pour la PREMIERE fois : les prochains identifiants de noeud et
    /// d'arete a allouer.
    ///
    /// (2026-08-13) `rollback()` ne rembobinait rien. Il appelait
    /// `discard_uncommitted_versions()` sur chaque graphe touche — une methode
    /// de trait au corps vide, jamais implementee. La comptabilite etait
    /// juste, l'effet nul.
    ///
    /// On ne peut pas se contenter de l'empreinte prise au BEGIN : un
    /// `USE GRAPH alpha` en cours de transaction fait entrer un graphe qui
    /// n'existait pas dans la photo de depart. L'empreinte est donc posee au
    /// premier contact, graphe par graphe.
    tx_baselines: parking_lot::Mutex<std::collections::HashMap<Option<String>, (u64, u64)>>,
    /// Shared metrics registry (populated when the `metrics` feature is enabled).
    #[cfg(feature = "metrics")]
    pub(crate) metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
    /// Transaction start time for duration tracking.
    #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
    tx_start_time: parking_lot::Mutex<Option<Instant>>,
    /// Optional post-commit hook for reactive event publishing.
    ///
    /// When set, called after every successful commit with the graph store
    /// reference. Used by `obrain-reactive` to drain pending mutation events
    /// from an `InstrumentedStore` and publish them to the `MutationBus`.
    ///
    /// This is the single injection point for the reactive substrate — it
    /// compiles to zero cost when unused (Option::None, no branch taken
    /// after optimization).
    #[cfg(feature = "reactive")]
    post_commit_hook: Option<Arc<dyn Fn(&dyn GraphStoreMut) + Send + Sync>>,
}

/// Per-graph savepoint snapshot, capturing the store state at the time of the savepoint.
#[derive(Clone)]
struct GraphSavepoint {
    graph_name: Option<String>,
    next_node_id: u64,
    next_edge_id: u64,
    undo_log_position: usize,
}

/// Savepoint state: name + per-graph snapshots + the graph that was active.
#[derive(Clone)]
struct SavepointState {
    name: String,
    graph_snapshots: Vec<GraphSavepoint>,
    /// The graph that was active when the savepoint was created.
    /// Reserved for future use (e.g., restoring graph context on rollback).
    #[allow(dead_code)]
    active_graph: Option<String>,
}

/// (mission perf) Un conflit d ecriture MVCC (OBRAIN-T001 / write-write)
/// est REJOUABLE pour un statement auto-commit : rien n a survecu au
/// rollback, re-executer est sur. Detection par le code stable T001 et les
/// libelles de conflit — deliberement etroite : tout le reste remonte.
fn erreur_de_conflit_rejouable(e: &obrain_common::utils::error::Error) -> bool {
    let texte = e.to_string();
    texte.contains("OBRAIN-T001")
        || texte.contains("Write conflict")
        || texte.contains("write-write")
        || texte.contains("TransactionConflict")
}

/// (2026-11-08, CONC-1) Budget de TEMPS des re-tentatives d un statement
/// auto-commit sur conflit rejouable. Voir le pave de
/// [`Session::with_auto_commit`] : la borne etait un nombre d essais (12),
/// qui ne mesurait pas la bonne grandeur — c est la duree de drainage de
/// la file des contendants qu il faut absorber. `query_timeout`, s il est
/// pose, plafonne ce budget.
pub(crate) const BUDGET_RETENTATIVE_CONFLIT: Duration = Duration::from_secs(1);

/// Ce qu'un point d'entrée SAIT du plan quand il franchit
/// [`Session::with_auto_commit`].
///
/// Ce type existe pour rendre IMPOSSIBLE la confusion qui a produit le
/// défaut. Deux décisions se prennent au même endroit à partir de la même
/// question — « est-ce que cette requête mute ? » — mais elles ne tolèrent
/// pas la même qualité de réponse :
///
///   · la TRANSACTION implicite tolère une devinette. Un faux négatif ne
///     fait qu'omettre l'enveloppe, ce qui est le comportement d'avant ;
///   · l'INTERRUPTIBILITÉ, non. Un faux positif désarme une lecture, un
///     faux négatif rend une écriture annulable — et une écriture annulée
///     en plein `MERGE` peut PERSISTER un doublon.
///
/// Passer un simple `bool` laissait la devinette textuelle décider des
/// deux. Le type sépare les deux réponses et oblige chaque entrée à dire
/// laquelle elle possède.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nature {
    /// Le plan est là : `plan.root.has_mutations()`. Réponse structurelle,
    /// valable pour les deux décisions.
    Plan(bool),
    /// Seul le TEXTE est là (les chemins qui délèguent au
    /// `QueryProcessor`). Le booléen est l'heuristique
    /// `query_looks_like_mutation` : il sert la transaction implicite, et
    /// RIEN d'autre.
    Texte(bool),
}

impl Nature {
    /// Pour la transaction implicite : la meilleure réponse disponible.
    const fn mute(self) -> bool {
        match self {
            Self::Plan(b) | Self::Texte(b) => b,
        }
    }

    /// Pour la frontière d'annulation : la réponse STRUCTURELLE, ou
    /// `None` — « je ne sais pas » — qui laisse la portée hors vigueur
    /// jusqu'à la frontière intérieure du `QueryProcessor`.
    const fn frontiere(self) -> Option<bool> {
        match self {
            Self::Plan(b) => Some(b),
            Self::Texte(_) => None,
        }
    }
}

impl Session {
    /// Creates a new session with adaptive execution configuration.
    #[allow(dead_code)]
    pub(crate) fn with_adaptive(store: Arc<dyn GraphStoreMut>, cfg: SessionConfig) -> Self {
        Self {
            graph_store: store,
            catalog: cfg.catalog,
            #[cfg(feature = "rdf")]
            rdf_store: Arc::new(RdfStore::new()),
            transaction_manager: cfg.transaction_manager,
            query_cache: cfg.query_cache,
            current_transaction: parking_lot::Mutex::new(None),
            read_only_tx: parking_lot::Mutex::new(cfg.read_only),
            db_read_only: cfg.read_only,
            auto_commit: true,
            adaptive_config: cfg.adaptive_config,
            factorized_execution: cfg.factorized_execution,
            base_registry: parking_lot::Mutex::new(None),
            access_context: parking_lot::Mutex::new(None),
            transaction_auto: std::sync::atomic::AtomicBool::new(false),
            graph_model: cfg.graph_model,
            query_timeout: cfg.query_timeout,
            commit_counter: cfg.commit_counter,
            gc_interval: cfg.gc_interval,
            transaction_start_node_count: AtomicUsize::new(0),
            transaction_start_edge_count: AtomicUsize::new(0),
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            substrate_store: None,
            #[cfg(feature = "wal")]
            wal: None,
            #[cfg(feature = "wal")]
            wal_graph_context: None,
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            #[cfg(feature = "cdc")]
            cdc_log: Arc::new(crate::cdc::CdcLog::new()),
            current_graph: parking_lot::Mutex::new(None),
            current_schema: parking_lot::Mutex::new(None),
            time_zone: parking_lot::Mutex::new(None),
            session_params: parking_lot::Mutex::new(std::collections::HashMap::new()),
            viewing_epoch_override: parking_lot::Mutex::new(None),
            savepoints: parking_lot::Mutex::new(Vec::new()),
            transaction_nesting_depth: parking_lot::Mutex::new(0),
            touched_graphs: parking_lot::Mutex::new(Vec::new()),
            tx_baselines: parking_lot::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "metrics")]
            metrics: None,
            #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
            tx_start_time: parking_lot::Mutex::new(None),
            #[cfg(feature = "reactive")]
            post_commit_hook: None,
        }
    }

    /// (2026-08-17, aretes inter-bases V2) Attache le registre
    /// multi-store de la session. Appele par `ObrainDB::session()` ; peut
    /// aussi etre pose apres coup (les planners suivants le verront).
    pub fn set_base_registry(&self, registry: Arc<obrain_core::execution::BaseRegistry>) {
        *self.base_registry.lock() = Some(registry);
    }

    /// (V3) Attache le contexte de droits du lecteur de cette session.
    /// Les planners construits ensuite (execute, execute_with_params,
    /// procedures) le voient tous via build_session_context.
    pub fn set_access_context(&self, access: Arc<obrain_core::execution::AccessContext>) {
        *self.access_context.lock() = Some(access);
    }

    /// (V3) Note la requete en cours pour l'audit des refus (tronquee).
    /// Best effort : appele par execute/execute_with_params/execute_cypher.
    fn note_requete_pour_audit(&self, query: &str) {
        if let Some(ctx) = &*self.access_context.lock() {
            ctx.note_requete(query);
        }
    }

    /// (V3) Sommes-nous dans une transaction EXPLICITE (START TRANSACTION),
    /// par opposition a l'auto-commit ? `current_transaction` est Some dans
    /// les deux cas — le drapeau `transaction_auto` fait la difference.
    fn in_explicit_transaction(&self) -> bool {
        self.current_transaction.lock().is_some()
            && !self
                .transaction_auto
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Sets the cognitive engine for this session (shared with the database).
    ///
    /// Mirrors `set_wal`/`set_cdc_log`: `ObrainDB` calls this right after
    /// constructing a session so `CALL obrain.cognitive.*` procedures (spread,
    /// detect_gaps, synthesize) can resolve the engine at query-plan time
    /// (see `Planner::with_cognitive_engine`, previously never wired from any
    /// real query-execution path — only exercised directly in unit tests).
    #[cfg(feature = "cognitive")]
    pub(crate) fn set_cognitive_engine(
        &mut self,
        engine: Arc<dyn obrain_cognitive::CognitiveEngine>,
    ) {
        self.cognitive_engine = Some(engine);
    }

    /// Returns this session's cognitive engine, if any (threaded into
    /// per-statement `QueryProcessor`s so `CALL obrain.cognitive.*` resolves).
    #[cfg(feature = "cognitive")]
    pub(crate) fn cognitive_engine(&self) -> Option<&Arc<dyn obrain_cognitive::CognitiveEngine>> {
        self.cognitive_engine.as_ref()
    }

    /// Sets the typed substrate handle for this session (shared with the
    /// database). Mirrors [`Self::set_cognitive_engine`]: `ObrainDB` calls it
    /// right after constructing a session so substrate-native procedures
    /// (`CALL obrain.window_2d`, RFC zorder-superposition C5) resolve at
    /// query-plan time. `None` on non-substrate backends.
    pub(crate) fn set_substrate_store(&mut self, store: Arc<obrain_substrate::SubstrateStore>) {
        self.substrate_store = Some(store);
    }

    /// Returns this session's typed substrate handle, if any (threaded into
    /// per-statement `QueryProcessor`s).
    pub(crate) fn substrate_store(&self) -> Option<&Arc<obrain_substrate::SubstrateStore>> {
        self.substrate_store.as_ref()
    }

    /// (O1) Vue zero-copy sur une propriété `String` d'un nœud : le `&str`
    /// est EMPRUNTÉ aux pages mmap du substrate — aucune copie, aucune
    /// sérialisation — et vit sous une garde de lecture relâchée au drop
    /// de la vue. Contrat complet : `obrain_substrate::view`.
    ///
    /// `None` si le store actif n'est pas un substrate (base en mémoire),
    /// si le nœud est mort, la clé absente ou la valeur non-`String`.
    ///
    /// Bail court : ne pas tenir la vue en écrivant sur la même base.
    #[must_use]
    pub fn prop_str_view(
        &self,
        id: NodeId,
        key: &str,
    ) -> Option<obrain_substrate::view::StrView<'_>> {
        self.substrate_store()?.node_prop_str_view(id, key)
    }

    /// (O1) [`Self::prop_str_view`] emballée pour le seeding spéculatif :
    /// le fait et son texte, prêts à tokeniser sans copie.
    #[must_use]
    pub fn fact_view(
        &self,
        id: NodeId,
        text_key: &str,
    ) -> Option<obrain_substrate::view::FactView<'_>> {
        Some(obrain_substrate::view::FactView {
            node: id,
            text: self.prop_str_view(id, text_key)?,
        })
    }

    /// (VWS) Jumeau ARÊTE de [`Self::prop_str_view`] : vue zero-copy sur
    /// une propriété `String` d'une arête — mêmes trois chemins (colonne
    /// blob, PropsZone v2 LWW, DashMap héritée), même contrat de lifetimes
    /// (`obrain_substrate::view`), même bail court.
    ///
    /// `None` si le store actif n'est pas un substrate (base en mémoire),
    /// si l'arête est morte, la clé absente ou la valeur non-`String`.
    #[must_use]
    pub fn edge_prop_str_view(
        &self,
        id: EdgeId,
        key: &str,
    ) -> Option<obrain_substrate::view::StrView<'_>> {
        self.substrate_store()?.edge_prop_str_view(id, key)
    }

    /// (VWS) Chemin POSSÉDÉ symétrique de [`Self::get_node_property`] : la
    /// valeur d'une propriété d'arête, clonée depuis l'arête matérialisée.
    /// Sert de référence aux benchs vue-contre-possédé.
    #[must_use]
    pub fn get_edge_property(&self, id: EdgeId, key: &str) -> Option<Value> {
        self.get_edge(id)
            .and_then(|edge| edge.get_property(key).cloned())
    }

    /// (VWS) Balayage zero-copy d'une collection de nœuds : appelle `f`
    /// avec le `&str` emprunté de chaque nœud vivant (filtré par `label`,
    /// `None` = tous) porteur d'une valeur `String` sous `key` — sans
    /// matérialiser ni la collection ni les valeurs (impl différée §3 du
    /// design O1).
    ///
    /// UNE prise de garde par zone pour tout le balayage (zone nœuds,
    /// colonne blob de la clé, PropsZone) — pas une par nœud : les appels
    /// sont regroupés PAR SOURCE (blob, zone, héritée), slots croissants
    /// dans chaque groupe. `ControlFlow::Break(())` arrête net. Rend le
    /// nombre de nœuds servis ; `Ok(0)` si la base active n'est pas un
    /// substrate.
    ///
    /// L'emprunt ne s'échappe pas du callback (borrow checker), et le
    /// callback ne doit ni écrire dans la même base ni bloquer : des
    /// gardes de lecture sont tenues pendant l'appel.
    pub fn for_each_prop_view<F>(
        &self,
        label: Option<&str>,
        key: &str,
        f: F,
    ) -> obrain_substrate::SubstrateResult<u64>
    where
        F: FnMut(NodeId, &str) -> std::ops::ControlFlow<()>,
    {
        match self.substrate_store() {
            Some(store) => store.for_each_node_prop_str_view(label, key, f),
            None => Ok(0),
        }
    }

    /// (VWS) Le chemin qui DOIT copier, sans allocations répétées : vide
    /// `buf` (capacités conservées) puis y matérialise `(id, texte)` pour
    /// chaque nœud servi par [`Self::for_each_prop_view`]. L'arène est
    /// amortie entre requêtes — voir `obrain_substrate::ResultBuf`.
    pub fn collect_props_into(
        &self,
        label: Option<&str>,
        key: &str,
        buf: &mut obrain_substrate::ResultBuf,
    ) -> obrain_substrate::SubstrateResult<u64> {
        buf.clear();
        self.for_each_prop_view(label, key, |id, s| {
            buf.push(id.0, s);
            std::ops::ControlFlow::Continue(())
        })
    }

    /// Returns this session's CDC log (threaded into per-statement
    /// `QueryProcessor`s so `CALL obrain.search_delta` resolves).
    #[cfg(feature = "cdc")]
    pub(crate) fn cdc_log(&self) -> &Arc<crate::cdc::CdcLog> {
        &self.cdc_log
    }

    /// Sets the WAL for this session (shared with the database).
    ///
    /// This also wraps `graph_store` in a [`WalGraphStore`] so that mutation
    /// operators (INSERT, DELETE, SET via queries) log to the WAL.
    #[cfg(feature = "wal")]
    pub(crate) fn set_wal(
        &mut self,
        wal: Arc<obrain_adapters::storage::wal::LpgWal>,
        wal_graph_context: Arc<parking_lot::Mutex<Option<String>>>,
    ) {
        // Wrap the graph store so query-engine mutations are WAL-logged
        self.graph_store = Arc::new(crate::database::wal_store::WalGraphStore::new(
            Arc::clone(&self.graph_store),
            Arc::clone(&wal),
            Arc::clone(&wal_graph_context),
        ));
        self.wal = Some(wal);
        self.wal_graph_context = Some(wal_graph_context);
    }

    /// Sets the CDC log for this session (shared with the database).
    #[cfg(feature = "cdc")]
    pub(crate) fn set_cdc_log(&mut self, cdc_log: Arc<crate::cdc::CdcLog>) {
        self.cdc_log = cdc_log;
    }

    /// Sets the metrics registry for this session (shared with the database).
    #[cfg(feature = "metrics")]
    pub(crate) fn set_metrics(&mut self, metrics: Arc<crate::metrics::MetricsRegistry>) {
        self.metrics = Some(metrics);
    }

    /// Sets a post-commit hook for reactive event publishing.
    ///
    /// The hook receives a reference to the graph store after each successful
    /// commit. Used by `obrain-reactive` to drain mutation events from an
    /// `InstrumentedStore` and publish them to the `MutationBus`.
    #[cfg(feature = "reactive")]
    pub fn set_post_commit_hook(&mut self, hook: Arc<dyn Fn(&dyn GraphStoreMut) + Send + Sync>) {
        self.post_commit_hook = Some(hook);
    }

    /// Creates a session backed by an external graph store.
    ///
    /// The external store handles all data operations. Transaction management
    /// (begin/commit/rollback) is not supported for external stores.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal arena allocation fails (out of memory).
    pub(crate) fn with_external_store(
        store: Arc<dyn GraphStoreMut>,
        cfg: SessionConfig,
    ) -> Result<Self> {
        Ok(Self {
            graph_store: store,
            catalog: cfg.catalog,
            #[cfg(feature = "rdf")]
            rdf_store: Arc::new(RdfStore::new()),
            transaction_manager: cfg.transaction_manager,
            query_cache: cfg.query_cache,
            current_transaction: parking_lot::Mutex::new(None),
            read_only_tx: parking_lot::Mutex::new(cfg.read_only),
            db_read_only: cfg.read_only,
            auto_commit: true,
            adaptive_config: cfg.adaptive_config,
            factorized_execution: cfg.factorized_execution,
            base_registry: parking_lot::Mutex::new(None),
            access_context: parking_lot::Mutex::new(None),
            transaction_auto: std::sync::atomic::AtomicBool::new(false),
            graph_model: cfg.graph_model,
            query_timeout: cfg.query_timeout,
            commit_counter: cfg.commit_counter,
            gc_interval: cfg.gc_interval,
            transaction_start_node_count: AtomicUsize::new(0),
            transaction_start_edge_count: AtomicUsize::new(0),
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            substrate_store: None,
            #[cfg(feature = "wal")]
            wal: None,
            #[cfg(feature = "wal")]
            wal_graph_context: None,
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            #[cfg(feature = "cdc")]
            cdc_log: Arc::new(crate::cdc::CdcLog::new()),
            current_graph: parking_lot::Mutex::new(None),
            current_schema: parking_lot::Mutex::new(None),
            time_zone: parking_lot::Mutex::new(None),
            session_params: parking_lot::Mutex::new(std::collections::HashMap::new()),
            viewing_epoch_override: parking_lot::Mutex::new(None),
            savepoints: parking_lot::Mutex::new(Vec::new()),
            transaction_nesting_depth: parking_lot::Mutex::new(0),
            touched_graphs: parking_lot::Mutex::new(Vec::new()),
            tx_baselines: parking_lot::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "metrics")]
            metrics: None,
            #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
            tx_start_time: parking_lot::Mutex::new(None),
            #[cfg(feature = "reactive")]
            post_commit_hook: None,
        })
    }

    /// Returns the graph model this session operates on.
    #[must_use]
    pub fn graph_model(&self) -> GraphModel {
        self.graph_model
    }

    // === Session State Management ===

    /// Sets the current graph for this session (USE GRAPH).
    pub fn use_graph(&self, name: &str) {
        *self.current_graph.lock() = Some(name.to_string());
    }

    /// Returns the current graph name, if any.
    #[must_use]
    pub fn current_graph(&self) -> Option<String> {
        self.current_graph.lock().clone()
    }

    /// Sets the current schema for this session (SESSION SET SCHEMA).
    ///
    /// Per ISO/IEC 39075 Section 7.1 GR1, this is independent of the session graph.
    pub fn set_schema(&self, name: &str) {
        *self.current_schema.lock() = Some(name.to_string());
    }

    /// Returns the current schema name, if any.
    ///
    /// `None` means "not set", which resolves to the default schema.
    #[must_use]
    pub fn current_schema(&self) -> Option<String> {
        self.current_schema.lock().clone()
    }

    /// Computes the effective storage key for a graph, accounting for schema context.
    ///
    /// Per ISO/IEC 39075 Section 17.2, graphs resolve relative to the current schema.
    /// Uses `/` as separator since it is invalid in GQL identifiers.
    fn effective_graph_key(&self, graph_name: &str) -> String {
        let schema = self.current_schema.lock().clone();
        match schema {
            Some(s) => format!("{s}/{graph_name}"),
            None => graph_name.to_string(),
        }
    }

    /// Returns the effective storage key for the current graph, accounting for schema.
    ///
    /// Combines `current_schema` and `current_graph` into a flat lookup key.
    fn active_graph_storage_key(&self) -> Option<String> {
        let graph = self.current_graph.lock().clone();
        let schema = self.current_schema.lock().clone();
        match (schema, graph) {
            (_, None) => None,
            (_, Some(ref name)) if name.eq_ignore_ascii_case("default") => None,
            (None, Some(name)) => Some(name),
            (Some(s), Some(g)) => Some(format!("{s}/{g}")),
        }
    }

    /// Returns the graph store for the currently active graph.
    ///
    /// If `current_graph` is `None` or `"default"`, returns the session's
    /// default `graph_store` (already WAL-wrapped for the default graph).
    /// Otherwise looks up the named graph in the root store and wraps it
    /// in a [`WalGraphStore`] so mutations are WAL-logged with the correct
    /// graph context.
    fn active_store(&self) -> Arc<dyn GraphStoreMut> {
        let key = self.active_graph_storage_key();
        match key {
            None => Arc::clone(&self.graph_store),
            Some(ref name) => match self.graph_store.named_graph(name) {
                Some(named_store) => {
                    #[cfg(feature = "wal")]
                    if let (Some(wal), Some(ctx)) = (&self.wal, &self.wal_graph_context) {
                        return Arc::new(crate::database::wal_store::WalGraphStore::new_for_graph(
                            named_store,
                            Arc::clone(wal),
                            name.clone(),
                            Arc::clone(ctx),
                        )) as Arc<dyn GraphStoreMut>;
                    }
                    named_store as Arc<dyn GraphStoreMut>
                }
                None => Arc::clone(&self.graph_store),
            },
        }
    }

    // T17 Step 24 (2026-04-23): `active_lpg_store()` was deleted.
    // All call sites were migrated to `active_store()` which returns
    // `Arc<dyn GraphStoreMut>`; MVCC + named-graph hooks live on the
    // trait with substrate-compatible defaults. The remaining
    // LpgStore-specific operations (explicit CREATE VECTOR INDEX /
    // CREATE INDEX / DROP INDEX) now return a substrate-era error
    // directly instead of routing to the dummy LpgStore shadow.

    /// Resolves a graph name to an `Arc<dyn GraphStoreMut>`.
    /// `None` and `"default"` resolve to the session's root store.
    ///
    /// T17 Wave B (2026-04-23): the return type is now the erased trait
    /// object instead of `Arc<LpgStore>`. MVCC hooks
    /// (`finalize_version_epochs`, `sync_epoch`, `gc_versions`,
    /// `rollback_transaction_properties*`, `peek_next_node_id`,
    /// `property_undo_log_position`) are on the `GraphStoreMut` trait
    /// with no-op defaults, so callers can drive commit/rollback
    /// bookkeeping through the trait regardless of whether the
    /// concrete backend is `LpgStore` (legacy, keeps inherent MVCC)
    /// or `SubstrateStore` (topology-as-storage, no per-tx epochs).
    fn resolve_store(&self, graph_name: &Option<String>) -> Arc<dyn GraphStoreMut> {
        match graph_name {
            None => Arc::clone(&self.graph_store),
            Some(name) if name.eq_ignore_ascii_case("default") => Arc::clone(&self.graph_store),
            Some(name) => self
                .graph_store
                .named_graph(name)
                .unwrap_or_else(|| Arc::clone(&self.graph_store)),
        }
    }

    /// Records the current graph as "touched" if a transaction is active.
    ///
    /// Uses the full storage key (schema/graph) so that commit/rollback
    /// can resolve the correct store via `resolve_store`.
    fn track_graph_touch(&self) {
        if self.current_transaction.lock().is_some() {
            let key = self.active_graph_storage_key();
            let mut touched = self.touched_graphs.lock();
            if !touched.contains(&key) {
                touched.push(key.clone());
                drop(touched);
                self.pose_empreinte(&key);
            }
        }
    }

    /// Pose l'empreinte d'un graphe : les prochains identifiants qu'il
    /// allouera. Tout ce qui sera cree apres pourra donc etre rembobine.
    ///
    /// Un backend sans rembobinage rend `0` aux deux appels — c'est le contrat
    /// documente du trait. On n'enregistre alors RIEN, plutot qu'une empreinte
    /// a zero : `(0..courant)` designerait la base ENTIERE, et un ROLLBACK
    /// effacerait tout. La difference entre « je ne sais pas » et « il n'y
    /// avait rien » est exactement le genre de confusion qui a produit les
    /// defauts de ce depot.
    fn pose_empreinte(&self, cle: &Option<String>) {
        let store = self.resolve_store(cle);
        let n = store.peek_next_node_id();
        let e = store.peek_next_edge_id();
        if n == 0 && e == 0 {
            return;
        }
        self.tx_baselines.lock().insert(cle.clone(), (n, e));
    }

    /// Rembobine chaque graphe touche jusqu'a son empreinte : la plage
    /// d'identifiants allouee depuis est PROPOSEE au store, qui n'en supprime
    /// que les entites creees par CETTE transaction (`discard_entities_by_id`
    /// filtre par proprietaire). Utilise par ROLLBACK.
    ///
    /// (2026-10-31) La plage n'est pas la propriete : le compteur avance pour
    /// toutes les sessions, et la supprimer en bloc effacait les creations
    /// commitees des voisins (jusqu'a 100 sur 100 mesures sur n3). Le store
    /// tranche, pas la session.
    fn rembobine_aux_empreintes(&self, transaction_id: TransactionId) {
        let empreintes = self.tx_baselines.lock().clone();
        for (cle, (n0, e0)) in &empreintes {
            let store = self.resolve_store(cle);
            let n1 = store.peek_next_node_id();
            let e1 = store.peek_next_edge_id();
            let node_ids: Vec<NodeId> = (*n0..n1).map(NodeId::new).collect();
            let edge_ids: Vec<EdgeId> = (*e0..e1).map(EdgeId::new).collect();
            if !node_ids.is_empty() || !edge_ids.is_empty() {
                store.discard_entities_by_id(transaction_id, &node_ids, &edge_ids);
            }
        }
    }

    /// Sets the session time zone.
    pub fn set_time_zone(&self, tz: &str) {
        *self.time_zone.lock() = Some(tz.to_string());
    }

    /// Returns the session time zone, if set.
    #[must_use]
    pub fn time_zone(&self) -> Option<String> {
        self.time_zone.lock().clone()
    }

    /// Sets a session parameter.
    pub fn set_parameter(&self, key: &str, value: obrain_common::types::Value) {
        self.session_params.lock().insert(key.to_string(), value);
    }

    /// Gets a session parameter by cloning it.
    #[must_use]
    pub fn get_parameter(&self, key: &str) -> Option<obrain_common::types::Value> {
        self.session_params.lock().get(key).cloned()
    }

    /// Resets all session state to defaults (ISO/IEC 39075 Section 7.2).
    pub fn reset_session(&self) {
        *self.current_schema.lock() = None;
        *self.current_graph.lock() = None;
        *self.time_zone.lock() = None;
        self.session_params.lock().clear();
        *self.viewing_epoch_override.lock() = None;
    }

    /// Resets only the session schema (Section 7.2 GR1).
    pub fn reset_schema(&self) {
        *self.current_schema.lock() = None;
    }

    /// Resets only the session graph (Section 7.2 GR2).
    pub fn reset_graph(&self) {
        *self.current_graph.lock() = None;
    }

    /// Resets only the session time zone (Section 7.2 GR3).
    pub fn reset_time_zone(&self) {
        *self.time_zone.lock() = None;
    }

    /// Resets only session parameters (Section 7.2 GR4).
    pub fn reset_parameters(&self) {
        self.session_params.lock().clear();
    }

    // --- Time-travel API ---

    /// Sets a viewing epoch override for time-travel queries.
    ///
    /// While set, all queries on this session see the database as it existed
    /// at the given epoch. Use [`clear_viewing_epoch`](Self::clear_viewing_epoch)
    /// to return to normal behavior.
    pub fn set_viewing_epoch(&self, epoch: EpochId) {
        *self.viewing_epoch_override.lock() = Some(epoch);
    }

    /// Clears the viewing epoch override, returning to normal behavior.
    pub fn clear_viewing_epoch(&self) {
        *self.viewing_epoch_override.lock() = None;
    }

    /// Returns the current viewing epoch override, if any.
    #[must_use]
    pub fn viewing_epoch(&self) -> Option<EpochId> {
        *self.viewing_epoch_override.lock()
    }

    /// Returns all versions of a node with their creation/deletion epochs.
    ///
    /// Properties and labels reflect the current state (not versioned per-epoch).
    #[must_use]
    pub fn get_node_history(&self, id: NodeId) -> Vec<(EpochId, Option<EpochId>, Node)> {
        self.active_store().get_node_history(id)
    }

    /// Returns all versions of an edge with their creation/deletion epochs.
    ///
    /// Properties reflect the current state (not versioned per-epoch).
    #[must_use]
    pub fn get_edge_history(&self, id: EdgeId) -> Vec<(EpochId, Option<EpochId>, Edge)> {
        self.active_store().get_edge_history(id)
    }

    /// Checks that the session's graph model supports LPG operations.
    fn require_lpg(&self, language: &str) -> Result<()> {
        if self.graph_model == GraphModel::Rdf {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "This is an RDF database. {language} queries require an LPG database."
            )));
        }
        Ok(())
    }

    /// Executes a session or transaction command, returning an empty result.
    fn execute_session_command(
        &self,
        cmd: obrain_adapters::query::gql::ast::SessionCommand,
    ) -> Result<QueryResult> {
        use obrain_adapters::query::gql::ast::{SessionCommand, TransactionIsolationLevel};
        use obrain_common::utils::error::{Error, QueryError, QueryErrorKind};

        // Block DDL in read-only transactions (ISO/IEC 39075 Section 8)
        if *self.read_only_tx.lock() {
            match &cmd {
                SessionCommand::CreateGraph { .. } | SessionCommand::DropGraph { .. } => {
                    return Err(Error::Transaction(
                        obrain_common::utils::error::TransactionError::ReadOnly,
                    ));
                }
                _ => {} // Session state + transaction control allowed
            }
        }

        match cmd {
            SessionCommand::CreateGraph {
                name,
                if_not_exists,
                typed,
                like_graph,
                copy_of,
                open: _,
            } => {
                // ISO/IEC 39075 Section 12.4: graphs are created within the current schema
                let storage_key = self.effective_graph_key(&name);

                // Validate source graph exists for LIKE / AS COPY OF
                if let Some(ref src) = like_graph {
                    let src_key = self.effective_graph_key(src);
                    if self.graph_store.named_graph(&src_key).is_none() {
                        return Err(Error::Query(QueryError::new(
                            QueryErrorKind::Semantic,
                            format!("Source graph '{src}' does not exist"),
                        )));
                    }
                }
                if let Some(ref src) = copy_of {
                    let src_key = self.effective_graph_key(src);
                    if self.graph_store.named_graph(&src_key).is_none() {
                        return Err(Error::Query(QueryError::new(
                            QueryErrorKind::Semantic,
                            format!("Source graph '{src}' does not exist"),
                        )));
                    }
                }

                // (2026-08-13) Distinguer « pas supporte » de « existe deja ».
                // Sans ce test, un backend sans graphes nommes rend `false`
                // et l'utilisateur lit « Graph 'x' already exists » sur une
                // base vierge : un message qui designe le mauvais coupable et
                // envoie chercher un doublon inexistant.
                if !self.graph_store.supports_named_graphs() {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!(
                            "named graphs are not supported by this backend \
                             (CREATE GRAPH '{name}') — le substrat n'a pas de \
                             registre de graphes nommes depuis le retrait de \
                             LpgStore ; utilise une base distincte"
                        ),
                    )));
                }
                let created = self
                    .graph_store
                    .create_named_graph(&storage_key)
                    .map_err(|e| Error::Internal(e.to_string()))?;
                if !created && !if_not_exists {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!("Graph '{name}' already exists"),
                    )));
                }
                if created {
                    #[cfg(feature = "wal")]
                    self.log_schema_wal(
                        &obrain_adapters::storage::wal::WalRecord::CreateNamedGraph {
                            name: storage_key.clone(),
                        },
                    );
                }

                // AS COPY OF: copy data from source graph
                if let Some(ref src) = copy_of {
                    let src_key = self.effective_graph_key(src);
                    self.graph_store
                        .copy_named_graph(Some(&src_key), Some(&storage_key))
                        .map_err(Error::Internal)?;
                }

                // Bind to graph type if specified
                if let Some(type_name) = typed
                    && let Err(e) = self
                        .catalog
                        .bind_graph_type(&storage_key, type_name.clone())
                {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    )));
                }

                // LIKE: copy graph type binding from source
                if let Some(ref src) = like_graph {
                    let src_key = self.effective_graph_key(src);
                    if let Some(src_type) = self.catalog.get_graph_type_binding(&src_key) {
                        let _ = self.catalog.bind_graph_type(&storage_key, src_type);
                    }
                }

                Ok(QueryResult::empty())
            }
            SessionCommand::DropGraph { name, if_exists } => {
                let storage_key = self.effective_graph_key(&name);
                let dropped = self.graph_store.drop_named_graph(&storage_key);
                if !dropped && !if_exists {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!("Graph '{name}' does not exist"),
                    )));
                }
                if dropped {
                    #[cfg(feature = "wal")]
                    self.log_schema_wal(
                        &obrain_adapters::storage::wal::WalRecord::DropNamedGraph {
                            name: storage_key.clone(),
                        },
                    );
                    // If this session was using the dropped graph, reset to default
                    let mut current = self.current_graph.lock();
                    if current
                        .as_deref()
                        .is_some_and(|g| g.eq_ignore_ascii_case(&name))
                    {
                        *current = None;
                    }
                }
                Ok(QueryResult::empty())
            }
            SessionCommand::UseGraph(name) => {
                // Verify graph exists (resolve within current schema)
                let effective_key = self.effective_graph_key(&name);
                if !name.eq_ignore_ascii_case("default")
                    && self.graph_store.named_graph(&effective_key).is_none()
                {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!("Graph '{name}' does not exist"),
                    )));
                }
                self.use_graph(&name);
                // Track the new graph if in a transaction
                self.track_graph_touch();
                Ok(QueryResult::empty())
            }
            SessionCommand::SessionSetGraph(name) => {
                // ISO/IEC 39075 Section 7.1 GR2: set session graph (resolved within current schema)
                let effective_key = self.effective_graph_key(&name);
                if !name.eq_ignore_ascii_case("default")
                    && self.graph_store.named_graph(&effective_key).is_none()
                {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!("Graph '{name}' does not exist"),
                    )));
                }
                self.use_graph(&name);
                // Track the new graph if in a transaction
                self.track_graph_touch();
                Ok(QueryResult::empty())
            }
            SessionCommand::SessionSetSchema(name) => {
                // ISO/IEC 39075 Section 7.1 GR1: set session schema (independent of graph)
                if !self.catalog.schema_exists(&name) {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!("Schema '{name}' does not exist"),
                    )));
                }
                self.set_schema(&name);
                Ok(QueryResult::empty())
            }
            SessionCommand::SessionSetTimeZone(tz) => {
                self.set_time_zone(&tz);
                Ok(QueryResult::empty())
            }
            SessionCommand::SessionSetParameter(key, expr) => {
                if key.eq_ignore_ascii_case("viewing_epoch") {
                    match Self::eval_integer_literal(&expr) {
                        Some(n) if n >= 0 => {
                            self.set_viewing_epoch(EpochId::new(n as u64));
                            Ok(QueryResult::status(format!("Set viewing_epoch to {n}")))
                        }
                        _ => Err(Error::Query(QueryError::new(
                            QueryErrorKind::Semantic,
                            "viewing_epoch must be a non-negative integer literal",
                        ))),
                    }
                } else {
                    // For now, store parameter name with Null value.
                    // Full expression evaluation would require building and executing a plan.
                    self.set_parameter(&key, Value::Null);
                    Ok(QueryResult::empty())
                }
            }
            SessionCommand::SessionReset(target) => {
                use obrain_adapters::query::gql::ast::SessionResetTarget;
                match target {
                    SessionResetTarget::All => self.reset_session(),
                    SessionResetTarget::Schema => self.reset_schema(),
                    SessionResetTarget::Graph => self.reset_graph(),
                    SessionResetTarget::TimeZone => self.reset_time_zone(),
                    SessionResetTarget::Parameters => self.reset_parameters(),
                }
                Ok(QueryResult::empty())
            }
            SessionCommand::SessionClose => {
                self.reset_session();
                Ok(QueryResult::empty())
            }
            SessionCommand::StartTransaction {
                read_only,
                isolation_level,
            } => {
                let engine_level = isolation_level.map(|l| match l {
                    TransactionIsolationLevel::ReadCommitted => {
                        crate::transaction::IsolationLevel::ReadCommitted
                    }
                    TransactionIsolationLevel::SnapshotIsolation => {
                        crate::transaction::IsolationLevel::SnapshotIsolation
                    }
                    TransactionIsolationLevel::Serializable => {
                        crate::transaction::IsolationLevel::Serializable
                    }
                });
                self.begin_transaction_inner(read_only, engine_level)?;
                Ok(QueryResult::status("Transaction started"))
            }
            SessionCommand::Commit => {
                self.commit_inner()?;
                Ok(QueryResult::status("Transaction committed"))
            }
            SessionCommand::Rollback => {
                self.rollback_inner()?;
                Ok(QueryResult::status("Transaction rolled back"))
            }
            SessionCommand::Savepoint(name) => {
                self.savepoint(&name)?;
                Ok(QueryResult::status(format!("Savepoint '{name}' created")))
            }
            SessionCommand::RollbackToSavepoint(name) => {
                self.rollback_to_savepoint(&name)?;
                Ok(QueryResult::status(format!(
                    "Rolled back to savepoint '{name}'"
                )))
            }
            SessionCommand::ReleaseSavepoint(name) => {
                self.release_savepoint(&name)?;
                Ok(QueryResult::status(format!("Savepoint '{name}' released")))
            }
        }
    }

    /// Logs a WAL record for a schema change (no-op if WAL is not enabled).
    #[cfg(feature = "wal")]
    fn log_schema_wal(&self, record: &obrain_adapters::storage::wal::WalRecord) {
        if let Some(ref wal) = self.wal
            && let Err(e) = wal.log(record)
        {
            tracing::warn!("Failed to log schema change to WAL: {}", e);
        }
    }

    /// Executes a schema DDL command, returning a status result.
    fn execute_schema_command(
        &self,
        cmd: obrain_adapters::query::gql::ast::SchemaStatement,
    ) -> Result<QueryResult> {
        use crate::catalog::{
            EdgeTypeDefinition, NodeTypeDefinition, PropertyDataType, TypedProperty,
        };
        use obrain_adapters::query::gql::ast::SchemaStatement;
        #[cfg(feature = "wal")]
        use obrain_adapters::storage::wal::WalRecord;
        use obrain_common::utils::error::{Error, QueryError, QueryErrorKind};

        /// Logs a WAL record for schema changes. Compiles to nothing without `wal`.
        macro_rules! wal_log {
            ($self:expr, $record:expr) => {
                #[cfg(feature = "wal")]
                $self.log_schema_wal(&$record);
            };
        }

        let result = match cmd {
            SchemaStatement::CreateNodeType(stmt) => {
                #[cfg(feature = "wal")]
                let props_for_wal: Vec<(String, String, bool)> = stmt
                    .properties
                    .iter()
                    .map(|p| (p.name.clone(), p.data_type.clone(), p.nullable))
                    .collect();
                let def = NodeTypeDefinition {
                    name: stmt.name.clone(),
                    properties: stmt
                        .properties
                        .iter()
                        .map(|p| TypedProperty {
                            name: p.name.clone(),
                            data_type: PropertyDataType::from_type_name(&p.data_type),
                            nullable: p.nullable,
                            default_value: p
                                .default_value
                                .as_ref()
                                .map(|s| parse_default_literal(s)),
                        })
                        .collect(),
                    constraints: Vec::new(),
                    parent_types: stmt.parent_types.clone(),
                };
                // ── LA DÉFINITION PART AU CATALOGUE DURABLE AVANT D'ÊTRE DÉPLACÉE ──
                // `CREATE NODE TYPE` écrivait un `WalRecord` que personne ne
                // rejoue : le type disparaissait au redémarrage (quatre tests de
                // `wal_recovery.rs` le constataient). On sérialise donc ici, à
                // l'instant où la définition est complète — l'écriture durable
                // se fait après le succès, plus bas.
                let definition_json = serde_json::to_string(&def).ok();
                let result = if stmt.or_replace {
                    let _ = self.catalog.drop_node_type(&stmt.name);
                    self.catalog.register_node_type(def)
                } else {
                    self.catalog.register_node_type(def)
                };
                match result {
                    Ok(()) => {
                        wal_log!(
                            self,
                            WalRecord::CreateNodeType {
                                name: stmt.name.clone(),
                                properties: props_for_wal,
                                constraints: Vec::new(),
                            }
                        );
                        // DÉCLARATION DURABLE : c'est elle qui rend le type
                        // visible après un redémarrage (le WAL moteur n'est
                        // jamais rejoué). Un échec est journalisé, jamais fatal :
                        // le type vaut pour la session courante, et le fichier
                        // sera réécrit au prochain CREATE réussi.
                        if let (Some(sub), Some(json)) = (self.substrate_store(), &definition_json)
                            && let Err(e) = sub.declarer_type("node", &stmt.name, json)
                        {
                            tracing::error!(
                                erreur = %e,
                                nom = %stmt.name,
                                "declaration de type de nœud NON persistee (substrate.typecat)"
                            );
                        }
                        Ok(QueryResult::status(format!(
                            "Created node type '{}'",
                            stmt.name
                        )))
                    }
                    Err(e) if stmt.if_not_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::CreateEdgeType(stmt) => {
                #[cfg(feature = "wal")]
                let props_for_wal: Vec<(String, String, bool)> = stmt
                    .properties
                    .iter()
                    .map(|p| (p.name.clone(), p.data_type.clone(), p.nullable))
                    .collect();
                let def = EdgeTypeDefinition {
                    name: stmt.name.clone(),
                    properties: stmt
                        .properties
                        .iter()
                        .map(|p| TypedProperty {
                            name: p.name.clone(),
                            data_type: PropertyDataType::from_type_name(&p.data_type),
                            nullable: p.nullable,
                            default_value: p
                                .default_value
                                .as_ref()
                                .map(|s| parse_default_literal(s)),
                        })
                        .collect(),
                    constraints: Vec::new(),
                    source_node_types: stmt.source_node_types.clone(),
                    target_node_types: stmt.target_node_types.clone(),
                };
                let definition_json = serde_json::to_string(&def).ok();
                let result = if stmt.or_replace {
                    let _ = self.catalog.drop_edge_type_def(&stmt.name);
                    self.catalog.register_edge_type_def(def)
                } else {
                    self.catalog.register_edge_type_def(def)
                };
                match result {
                    Ok(()) => {
                        wal_log!(
                            self,
                            WalRecord::CreateEdgeType {
                                name: stmt.name.clone(),
                                properties: props_for_wal,
                                constraints: Vec::new(),
                            }
                        );
                        if let (Some(sub), Some(json)) = (self.substrate_store(), &definition_json)
                            && let Err(e) = sub.declarer_type("edge", &stmt.name, json)
                        {
                            tracing::error!(
                                erreur = %e,
                                nom = %stmt.name,
                                "declaration de type d'arete NON persistee (substrate.typecat)"
                            );
                        }
                        Ok(QueryResult::status(format!(
                            "Created edge type '{}'",
                            stmt.name
                        )))
                    }
                    Err(e) if stmt.if_not_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::CreateVectorIndex(_stmt) => {
                // T17 Step 24 (2026-04-23): explicit vector-index
                // creation via GQL is an LpgStore-era API. Substrate
                // builds its own tiered-storage vector layout
                // automatically (L0/L1/L2 zones driven by the
                // CommunityWarden thinker) — no per-index HNSW graph
                // to materialise. The GQL statement is retained so
                // existing scripts parse cleanly but evaluating it
                // against a substrate-backed session returns an error.
                Err(Error::Internal(
                    "CREATE VECTOR INDEX is no longer supported since the T17 substrate \
                     cutover — substrate builds its own tiered-storage vector layout \
                     automatically (see CommunityWarden)."
                        .to_string(),
                ))
            }
            SchemaStatement::DropNodeType { name, if_exists } => {
                match self.catalog.drop_node_type(&name) {
                    Ok(()) => {
                        wal_log!(self, WalRecord::DropNodeType { name: name.clone() });
                        // Miroir du CREATE : le type retiré ne doit pas revenir
                        // au prochain redémarrage par le catalogue durable.
                        if let Some(sub) = self.substrate_store()
                            && let Err(e) = sub.retirer_type("node", &name)
                        {
                            tracing::error!(
                                erreur = %e, nom = %name,
                                "retrait de declaration de type NON persiste (substrate.typecat)"
                            );
                        }
                        Ok(QueryResult::status(format!("Dropped node type '{name}'")))
                    }
                    Err(e) if if_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::DropEdgeType { name, if_exists } => {
                match self.catalog.drop_edge_type_def(&name) {
                    Ok(()) => {
                        wal_log!(self, WalRecord::DropEdgeType { name: name.clone() });
                        if let Some(sub) = self.substrate_store()
                            && let Err(e) = sub.retirer_type("edge", &name)
                        {
                            tracing::error!(
                                erreur = %e, nom = %name,
                                "retrait de declaration de type NON persiste (substrate.typecat)"
                            );
                        }
                        Ok(QueryResult::status(format!("Dropped edge type '{name}'")))
                    }
                    Err(e) if if_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::CreateIndex(_stmt) => {
                // T17 Step 24 (2026-04-23): `CREATE INDEX` / `CREATE TEXT
                // INDEX` / `CREATE VECTOR INDEX` were LpgStore-era APIs
                // that materialised per-label/per-property HNSW or BM25
                // side-tables. Substrate's tiered-storage layout
                // subsumes them: embeddings live in L0/L1/L2 zones
                // built automatically by the CommunityWarden, and the
                // text index lives in the substrate's inverted-index
                // column. The GQL statements are retained so existing
                // scripts parse cleanly but evaluating them returns an
                // error on substrate-backed sessions.
                Err(Error::Internal(
                    "CREATE INDEX is no longer supported since the T17 substrate cutover — \
                     substrate builds its own tiered-storage / inverted-index layout \
                     automatically (see CommunityWarden + substrate RetrievalTier)."
                        .to_string(),
                ))
            }
            SchemaStatement::DropIndex { name, if_exists } => {
                // T17 Step 24 (2026-04-23): `DROP INDEX` was an
                // LpgStore-era API. Substrate's tiered storage does
                // not expose named per-property indexes. Returns Ok
                // under `IF EXISTS` for idempotent scripts, error
                // otherwise.
                if if_exists {
                    Ok(QueryResult::status("No change"))
                } else {
                    Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!(
                            "DROP INDEX '{name}' is no longer supported since the T17 \
                             substrate cutover — substrate does not expose named per-property \
                             indexes."
                        ),
                    )))
                }
            }
            SchemaStatement::CreateConstraint(stmt) => {
                use crate::catalog::TypeConstraint;
                use obrain_adapters::query::gql::ast::ConstraintKind;
                let kind_str = match stmt.constraint_kind {
                    ConstraintKind::Unique => "unique",
                    ConstraintKind::NodeKey => "node_key",
                    ConstraintKind::NotNull => "not_null",
                    ConstraintKind::Exists => "exists",
                };
                let constraint_name = stmt
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{}_{kind_str}", stmt.label));

                // Register constraint in catalog type definitions
                match stmt.constraint_kind {
                    ConstraintKind::Unique => {
                        for prop in &stmt.properties {
                            let label_id = self.catalog.get_or_create_label(&stmt.label);
                            let prop_id = self.catalog.get_or_create_property_key(prop);
                            let _ = self.catalog.add_unique_constraint(label_id, prop_id);
                        }
                        let _ = self.catalog.add_constraint_to_type(
                            &stmt.label,
                            TypeConstraint::Unique(stmt.properties.clone()),
                        );
                    }
                    ConstraintKind::NodeKey => {
                        for prop in &stmt.properties {
                            let label_id = self.catalog.get_or_create_label(&stmt.label);
                            let prop_id = self.catalog.get_or_create_property_key(prop);
                            let _ = self.catalog.add_unique_constraint(label_id, prop_id);
                            let _ = self.catalog.add_required_property(label_id, prop_id);
                        }
                        let _ = self.catalog.add_constraint_to_type(
                            &stmt.label,
                            TypeConstraint::PrimaryKey(stmt.properties.clone()),
                        );
                    }
                    ConstraintKind::NotNull | ConstraintKind::Exists => {
                        for prop in &stmt.properties {
                            let label_id = self.catalog.get_or_create_label(&stmt.label);
                            let prop_id = self.catalog.get_or_create_property_key(prop);
                            let _ = self.catalog.add_required_property(label_id, prop_id);
                            let _ = self.catalog.add_constraint_to_type(
                                &stmt.label,
                                TypeConstraint::NotNull(prop.clone()),
                            );
                        }
                    }
                }

                wal_log!(
                    self,
                    WalRecord::CreateConstraint {
                        name: constraint_name.clone(),
                        label: stmt.label.clone(),
                        properties: stmt.properties.clone(),
                        kind: kind_str.to_string(),
                    }
                );
                Ok(QueryResult::status(format!(
                    "Created {kind_str} constraint '{constraint_name}'"
                )))
            }
            SchemaStatement::DropConstraint { name, if_exists } => {
                let _ = if_exists;
                wal_log!(self, WalRecord::DropConstraint { name: name.clone() });
                Ok(QueryResult::status(format!("Dropped constraint '{name}'")))
            }
            SchemaStatement::CreateGraphType(stmt) => {
                use crate::catalog::GraphTypeDefinition;
                use obrain_adapters::query::gql::ast::InlineElementType;

                // GG04: LIKE clause copies type from existing graph
                let (mut node_types, mut edge_types, open) =
                    if let Some(ref like_graph) = stmt.like_graph {
                        // Infer types from the graph's bound type, or use its existing types
                        if let Some(type_name) = self.catalog.get_graph_type_binding(like_graph) {
                            if let Some(existing) = self
                                .catalog
                                .schema()
                                .and_then(|s| s.get_graph_type(&type_name))
                            {
                                (
                                    existing.allowed_node_types.clone(),
                                    existing.allowed_edge_types.clone(),
                                    existing.open,
                                )
                            } else {
                                (Vec::new(), Vec::new(), true)
                            }
                        } else {
                            // GG22: Infer from graph data (labels used in graph)
                            let nt = self.catalog.all_node_type_names();
                            let et = self.catalog.all_edge_type_names();
                            if nt.is_empty() && et.is_empty() {
                                (Vec::new(), Vec::new(), true)
                            } else {
                                (nt, et, false)
                            }
                        }
                    } else {
                        (stmt.node_types.clone(), stmt.edge_types.clone(), stmt.open)
                    };

                // GG03: Register inline element types and add their names
                for inline in &stmt.inline_types {
                    match inline {
                        InlineElementType::Node {
                            name,
                            properties,
                            key_labels,
                            ..
                        } => {
                            let def = NodeTypeDefinition {
                                name: name.clone(),
                                properties: properties
                                    .iter()
                                    .map(|p| TypedProperty {
                                        name: p.name.clone(),
                                        data_type: PropertyDataType::from_type_name(&p.data_type),
                                        nullable: p.nullable,
                                        default_value: None,
                                    })
                                    .collect(),
                                constraints: Vec::new(),
                                parent_types: key_labels.clone(),
                            };
                            // Register or replace so inline defs override existing
                            self.catalog.register_or_replace_node_type(def);
                            #[cfg(feature = "wal")]
                            {
                                let props_for_wal: Vec<(String, String, bool)> = properties
                                    .iter()
                                    .map(|p| (p.name.clone(), p.data_type.clone(), p.nullable))
                                    .collect();
                                self.log_schema_wal(&WalRecord::CreateNodeType {
                                    name: name.clone(),
                                    properties: props_for_wal,
                                    constraints: Vec::new(),
                                });
                            }
                            if !node_types.contains(name) {
                                node_types.push(name.clone());
                            }
                        }
                        InlineElementType::Edge {
                            name,
                            properties,
                            source_node_types,
                            target_node_types,
                            ..
                        } => {
                            let def = EdgeTypeDefinition {
                                name: name.clone(),
                                properties: properties
                                    .iter()
                                    .map(|p| TypedProperty {
                                        name: p.name.clone(),
                                        data_type: PropertyDataType::from_type_name(&p.data_type),
                                        nullable: p.nullable,
                                        default_value: None,
                                    })
                                    .collect(),
                                constraints: Vec::new(),
                                source_node_types: source_node_types.clone(),
                                target_node_types: target_node_types.clone(),
                            };
                            self.catalog.register_or_replace_edge_type_def(def);
                            #[cfg(feature = "wal")]
                            {
                                let props_for_wal: Vec<(String, String, bool)> = properties
                                    .iter()
                                    .map(|p| (p.name.clone(), p.data_type.clone(), p.nullable))
                                    .collect();
                                self.log_schema_wal(&WalRecord::CreateEdgeType {
                                    name: name.clone(),
                                    properties: props_for_wal,
                                    constraints: Vec::new(),
                                });
                            }
                            if !edge_types.contains(name) {
                                edge_types.push(name.clone());
                            }
                        }
                    }
                }

                let def = GraphTypeDefinition {
                    name: stmt.name.clone(),
                    allowed_node_types: node_types.clone(),
                    allowed_edge_types: edge_types.clone(),
                    open,
                };
                let result = if stmt.or_replace {
                    // Drop existing first, ignore error if not found
                    let _ = self.catalog.drop_graph_type(&stmt.name);
                    self.catalog.register_graph_type(def)
                } else {
                    self.catalog.register_graph_type(def)
                };
                match result {
                    Ok(()) => {
                        wal_log!(
                            self,
                            WalRecord::CreateGraphType {
                                name: stmt.name.clone(),
                                node_types,
                                edge_types,
                                open,
                            }
                        );
                        Ok(QueryResult::status(format!(
                            "Created graph type '{}'",
                            stmt.name
                        )))
                    }
                    Err(e) if stmt.if_not_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::DropGraphType { name, if_exists } => {
                match self.catalog.drop_graph_type(&name) {
                    Ok(()) => {
                        wal_log!(self, WalRecord::DropGraphType { name: name.clone() });
                        Ok(QueryResult::status(format!("Dropped graph type '{name}'")))
                    }
                    Err(e) if if_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::CreateSchema {
                name,
                if_not_exists,
            } => match self.catalog.register_schema_namespace(name.clone()) {
                Ok(()) => {
                    wal_log!(self, WalRecord::CreateSchema { name: name.clone() });
                    Ok(QueryResult::status(format!("Created schema '{name}'")))
                }
                Err(e) if if_not_exists => {
                    let _ = e;
                    Ok(QueryResult::status("No change"))
                }
                Err(e) => Err(Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    e.to_string(),
                ))),
            },
            SchemaStatement::DropSchema { name, if_exists } => {
                // ISO/IEC 39075 Section 12.3: schema must be empty before dropping
                let prefix = format!("{name}/");
                let has_graphs = self
                    .graph_store
                    .named_graph_names()
                    .iter()
                    .any(|g| g.starts_with(&prefix));
                if has_graphs {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        format!(
                            "Schema '{name}' is not empty: drop all graphs in the schema first"
                        ),
                    )));
                }
                match self.catalog.drop_schema_namespace(&name) {
                    Ok(()) => {
                        wal_log!(self, WalRecord::DropSchema { name: name.clone() });
                        // If this session was using the dropped schema, reset it
                        let mut current = self.current_schema.lock();
                        if current
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&name))
                        {
                            *current = None;
                        }
                        Ok(QueryResult::status(format!("Dropped schema '{name}'")))
                    }
                    Err(e) if if_exists => {
                        let _ = e;
                        Ok(QueryResult::status("No change"))
                    }
                    Err(e) => Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        e.to_string(),
                    ))),
                }
            }
            SchemaStatement::AlterNodeType(stmt) => {
                use obrain_adapters::query::gql::ast::TypeAlteration;
                let mut wal_alts = Vec::new();
                for alt in &stmt.alterations {
                    match alt {
                        TypeAlteration::AddProperty(prop) => {
                            let typed = TypedProperty {
                                name: prop.name.clone(),
                                data_type: PropertyDataType::from_type_name(&prop.data_type),
                                nullable: prop.nullable,
                                default_value: prop
                                    .default_value
                                    .as_ref()
                                    .map(|s| parse_default_literal(s)),
                            };
                            self.catalog
                                .alter_node_type_add_property(&stmt.name, typed)
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push((
                                "add".to_string(),
                                prop.name.clone(),
                                prop.data_type.clone(),
                                prop.nullable,
                            ));
                        }
                        TypeAlteration::DropProperty(name) => {
                            self.catalog
                                .alter_node_type_drop_property(&stmt.name, name)
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push(("drop".to_string(), name.clone(), String::new(), false));
                        }
                    }
                }
                wal_log!(
                    self,
                    WalRecord::AlterNodeType {
                        name: stmt.name.clone(),
                        alterations: wal_alts,
                    }
                );
                Ok(QueryResult::status(format!(
                    "Altered node type '{}'",
                    stmt.name
                )))
            }
            SchemaStatement::AlterEdgeType(stmt) => {
                use obrain_adapters::query::gql::ast::TypeAlteration;
                let mut wal_alts = Vec::new();
                for alt in &stmt.alterations {
                    match alt {
                        TypeAlteration::AddProperty(prop) => {
                            let typed = TypedProperty {
                                name: prop.name.clone(),
                                data_type: PropertyDataType::from_type_name(&prop.data_type),
                                nullable: prop.nullable,
                                default_value: prop
                                    .default_value
                                    .as_ref()
                                    .map(|s| parse_default_literal(s)),
                            };
                            self.catalog
                                .alter_edge_type_add_property(&stmt.name, typed)
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push((
                                "add".to_string(),
                                prop.name.clone(),
                                prop.data_type.clone(),
                                prop.nullable,
                            ));
                        }
                        TypeAlteration::DropProperty(name) => {
                            self.catalog
                                .alter_edge_type_drop_property(&stmt.name, name)
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push(("drop".to_string(), name.clone(), String::new(), false));
                        }
                    }
                }
                wal_log!(
                    self,
                    WalRecord::AlterEdgeType {
                        name: stmt.name.clone(),
                        alterations: wal_alts,
                    }
                );
                Ok(QueryResult::status(format!(
                    "Altered edge type '{}'",
                    stmt.name
                )))
            }
            SchemaStatement::AlterGraphType(stmt) => {
                use obrain_adapters::query::gql::ast::GraphTypeAlteration;
                let mut wal_alts = Vec::new();
                for alt in &stmt.alterations {
                    match alt {
                        GraphTypeAlteration::AddNodeType(name) => {
                            self.catalog
                                .alter_graph_type_add_node_type(&stmt.name, name.clone())
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push(("add_node_type".to_string(), name.clone()));
                        }
                        GraphTypeAlteration::DropNodeType(name) => {
                            self.catalog
                                .alter_graph_type_drop_node_type(&stmt.name, name)
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push(("drop_node_type".to_string(), name.clone()));
                        }
                        GraphTypeAlteration::AddEdgeType(name) => {
                            self.catalog
                                .alter_graph_type_add_edge_type(&stmt.name, name.clone())
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push(("add_edge_type".to_string(), name.clone()));
                        }
                        GraphTypeAlteration::DropEdgeType(name) => {
                            self.catalog
                                .alter_graph_type_drop_edge_type(&stmt.name, name)
                                .map_err(|e| {
                                    Error::Query(QueryError::new(
                                        QueryErrorKind::Semantic,
                                        e.to_string(),
                                    ))
                                })?;
                            wal_alts.push(("drop_edge_type".to_string(), name.clone()));
                        }
                    }
                }
                wal_log!(
                    self,
                    WalRecord::AlterGraphType {
                        name: stmt.name.clone(),
                        alterations: wal_alts,
                    }
                );
                Ok(QueryResult::status(format!(
                    "Altered graph type '{}'",
                    stmt.name
                )))
            }
            SchemaStatement::CreateProcedure(stmt) => {
                use crate::catalog::ProcedureDefinition;

                let def = ProcedureDefinition {
                    name: stmt.name.clone(),
                    params: stmt
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), p.param_type.clone()))
                        .collect(),
                    returns: stmt
                        .returns
                        .iter()
                        .map(|r| (r.name.clone(), r.return_type.clone()))
                        .collect(),
                    body: stmt.body.clone(),
                };

                if stmt.or_replace {
                    self.catalog.replace_procedure(def).map_err(|e| {
                        Error::Query(QueryError::new(QueryErrorKind::Semantic, e.to_string()))
                    })?;
                } else {
                    match self.catalog.register_procedure(def) {
                        Ok(()) => {}
                        Err(_) if stmt.if_not_exists => {
                            return Ok(QueryResult::empty());
                        }
                        Err(e) => {
                            return Err(Error::Query(QueryError::new(
                                QueryErrorKind::Semantic,
                                e.to_string(),
                            )));
                        }
                    }
                }

                // (2026-09-07) PERSISTANCE DURABLE — le catalogue RAM est par
                // processus : la definition est aussi declaree cote substrate
                // (`substrate.proccat`, hydrate au boot). Le corps GQL vit en
                // donnees, pas seulement en RAM du process qui l'a cree.
                // Note : `def` a ete consomme par register/replace ci-dessus —
                // la declaration est reconstruite depuis `stmt` (succes garanti
                // ici, donc meme nom/params/returns/body).
                if let Some(sub) = self.substrate_store() {
                    let decl = obrain_substrate::DeclarationProcedure {
                        name: stmt.name.clone(),
                        params: stmt
                            .params
                            .iter()
                            .map(|p| (p.name.clone(), p.param_type.clone()))
                            .collect(),
                        returns: stmt
                            .returns
                            .iter()
                            .map(|r| (r.name.clone(), r.return_type.clone()))
                            .collect(),
                        body: stmt.body.clone(),
                    };
                    if let Err(e) = sub.declarer_procedure(decl) {
                        tracing::error!(
                            erreur = %e,
                            nom = %stmt.name,
                            "declaration de procedure NON persistee (substrate.proccat)"
                        );
                    }
                }

                wal_log!(
                    self,
                    WalRecord::CreateProcedure {
                        name: stmt.name.clone(),
                        params: stmt
                            .params
                            .iter()
                            .map(|p| (p.name.clone(), p.param_type.clone()))
                            .collect(),
                        returns: stmt
                            .returns
                            .iter()
                            .map(|r| (r.name.clone(), r.return_type.clone()))
                            .collect(),
                        body: stmt.body,
                    }
                );
                Ok(QueryResult::status(format!(
                    "Created procedure '{}'",
                    stmt.name
                )))
            }
            SchemaStatement::DropProcedure { name, if_exists } => {
                match self.catalog.drop_procedure(&name) {
                    Ok(()) => {}
                    Err(_) if if_exists => {
                        return Ok(QueryResult::empty());
                    }
                    Err(e) => {
                        return Err(Error::Query(QueryError::new(
                            QueryErrorKind::Semantic,
                            e.to_string(),
                        )));
                    }
                }
                wal_log!(self, WalRecord::DropProcedure { name: name.clone() });
                // (2026-09-07) Retire la declaration durable, miroir du CREATE.
                if let Some(sub) = self.substrate_store()
                    && let Err(e) = sub.retirer_declaration_procedure(&name)
                {
                    tracing::error!(
                        erreur = %e,
                        nom = %name,
                        "retrait de declaration de procedure NON persiste (substrate.proccat)"
                    );
                }
                Ok(QueryResult::status(format!("Dropped procedure '{name}'")))
            }
            SchemaStatement::ShowIndexes => {
                return self.execute_show_indexes();
            }
            SchemaStatement::ShowConstraints => {
                return self.execute_show_constraints();
            }
            SchemaStatement::ShowNodeTypes => {
                return self.execute_show_node_types();
            }
            SchemaStatement::ShowEdgeTypes => {
                return self.execute_show_edge_types();
            }
            SchemaStatement::ShowGraphTypes => {
                return self.execute_show_graph_types();
            }
            SchemaStatement::ShowGraphType(name) => {
                return self.execute_show_graph_type(&name);
            }
            SchemaStatement::ShowCurrentGraphType => {
                return self.execute_show_current_graph_type();
            }
            SchemaStatement::ShowGraphs => {
                return self.execute_show_graphs();
            }
            SchemaStatement::ShowSchemas => {
                return self.execute_show_schemas();
            }
        };

        // Invalidate all cached query plans after any successful DDL change.
        // DDL is rare, so clearing the entire cache is cheap and correct.
        if result.is_ok() {
            self.query_cache.clear();
        }

        result
    }

    // T17 Step 24 (2026-04-23): `create_vector_index_on_store` and
    // `create_text_index_on_store` were LpgStore-only helpers (they
    // built HNSW / BM25 side-tables and attached them to the
    // concrete `LpgStore` via `add_vector_index` / `add_text_index`).
    // The `CREATE VECTOR INDEX` / `CREATE INDEX` GQL statements that
    // called these helpers now return a substrate-era error at
    // dispatch, so the helpers themselves are dead code and have been
    // removed (-170 LOC). Substrate builds its own tiered-storage
    // vector + inverted-index layout automatically (L0/L1/L2 zones
    // driven by the CommunityWarden thinker).

    /// Returns a table of all indexes from the catalog.
    fn execute_show_indexes(&self) -> Result<QueryResult> {
        let indexes = self.catalog.all_indexes();
        let columns = vec![
            "name".to_string(),
            "type".to_string(),
            "label".to_string(),
            "property".to_string(),
        ];
        let rows: Vec<Vec<Value>> = indexes
            .into_iter()
            .map(|def| {
                let label_name = self
                    .catalog
                    .get_label_name(def.label)
                    .unwrap_or_else(|| "?".into());
                let prop_name = self
                    .catalog
                    .get_property_key_name(def.property_key)
                    .unwrap_or_else(|| "?".into());
                vec![
                    Value::from(format!("idx_{}_{}", label_name, prop_name)),
                    Value::from(format!("{:?}", def.index_type)),
                    Value::from(&*label_name),
                    Value::from(&*prop_name),
                ]
            })
            .collect();
        Ok(QueryResult {
            columns,
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns a table of all constraints (currently metadata-only).
    fn execute_show_constraints(&self) -> Result<QueryResult> {
        // Constraints are tracked in WAL but not yet in a queryable catalog.
        // Return an empty table with the expected schema.
        Ok(QueryResult {
            columns: vec![
                "name".to_string(),
                "type".to_string(),
                "label".to_string(),
                "properties".to_string(),
            ],
            column_types: Vec::new(),
            rows: Vec::new(),
            ..QueryResult::empty()
        })
    }

    /// Returns a table of all registered node types.
    fn execute_show_node_types(&self) -> Result<QueryResult> {
        let columns = vec![
            "name".to_string(),
            "properties".to_string(),
            "constraints".to_string(),
            "parents".to_string(),
        ];
        let type_names = self.catalog.all_node_type_names();
        let rows: Vec<Vec<Value>> = type_names
            .into_iter()
            .filter_map(|name| {
                let def = self.catalog.get_node_type(&name)?;
                let props: Vec<String> = def
                    .properties
                    .iter()
                    .map(|p| {
                        let nullable = if p.nullable { "" } else { " NOT NULL" };
                        format!("{} {}{}", p.name, p.data_type, nullable)
                    })
                    .collect();
                let constraints: Vec<String> =
                    def.constraints.iter().map(|c| format!("{c:?}")).collect();
                let parents = def.parent_types.join(", ");
                Some(vec![
                    Value::from(name),
                    Value::from(props.join(", ")),
                    Value::from(constraints.join(", ")),
                    Value::from(parents),
                ])
            })
            .collect();
        Ok(QueryResult {
            columns,
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns a table of all registered edge types.
    fn execute_show_edge_types(&self) -> Result<QueryResult> {
        let columns = vec![
            "name".to_string(),
            "properties".to_string(),
            "source_types".to_string(),
            "target_types".to_string(),
        ];
        let type_names = self.catalog.all_edge_type_names();
        let rows: Vec<Vec<Value>> = type_names
            .into_iter()
            .filter_map(|name| {
                let def = self.catalog.get_edge_type_def(&name)?;
                let props: Vec<String> = def
                    .properties
                    .iter()
                    .map(|p| {
                        let nullable = if p.nullable { "" } else { " NOT NULL" };
                        format!("{} {}{}", p.name, p.data_type, nullable)
                    })
                    .collect();
                let src = def.source_node_types.join(", ");
                let tgt = def.target_node_types.join(", ");
                Some(vec![
                    Value::from(name),
                    Value::from(props.join(", ")),
                    Value::from(src),
                    Value::from(tgt),
                ])
            })
            .collect();
        Ok(QueryResult {
            columns,
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns a table of all registered graph types.
    fn execute_show_graph_types(&self) -> Result<QueryResult> {
        let columns = vec![
            "name".to_string(),
            "open".to_string(),
            "node_types".to_string(),
            "edge_types".to_string(),
        ];
        let type_names = self.catalog.all_graph_type_names();
        let rows: Vec<Vec<Value>> = type_names
            .into_iter()
            .filter_map(|name| {
                let def = self.catalog.get_graph_type_def(&name)?;
                Some(vec![
                    Value::from(name),
                    Value::from(def.open),
                    Value::from(def.allowed_node_types.join(", ")),
                    Value::from(def.allowed_edge_types.join(", ")),
                ])
            })
            .collect();
        Ok(QueryResult {
            columns,
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns the list of named graphs visible in the current schema context.
    ///
    /// When a session schema is set, only graphs belonging to that schema are
    /// shown (their compound prefix is stripped). When no schema is set, graphs
    /// without a schema prefix are shown (the default schema).
    fn execute_show_graphs(&self) -> Result<QueryResult> {
        let schema = self.current_schema.lock().clone();
        let all_names = self.graph_store.named_graph_names();

        let mut names: Vec<String> = match &schema {
            Some(s) => {
                let prefix = format!("{s}/");
                all_names
                    .into_iter()
                    .filter_map(|n| n.strip_prefix(&prefix).map(String::from))
                    .collect()
            }
            None => all_names.into_iter().filter(|n| !n.contains('/')).collect(),
        };
        names.sort();

        let rows: Vec<Vec<Value>> = names.into_iter().map(|n| vec![Value::from(n)]).collect();
        Ok(QueryResult {
            columns: vec!["name".to_string()],
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns the list of all schema namespaces.
    fn execute_show_schemas(&self) -> Result<QueryResult> {
        let mut names = self.catalog.schema_names();
        names.sort();
        let rows: Vec<Vec<Value>> = names.into_iter().map(|n| vec![Value::from(n)]).collect();
        Ok(QueryResult {
            columns: vec!["name".to_string()],
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns detailed info for a specific graph type.
    fn execute_show_graph_type(&self, name: &str) -> Result<QueryResult> {
        use obrain_common::utils::error::{Error, QueryError, QueryErrorKind};

        let def = self.catalog.get_graph_type_def(name).ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                format!("Graph type '{name}' not found"),
            ))
        })?;

        let columns = vec![
            "name".to_string(),
            "open".to_string(),
            "node_types".to_string(),
            "edge_types".to_string(),
        ];
        let rows = vec![vec![
            Value::from(def.name),
            Value::from(def.open),
            Value::from(def.allowed_node_types.join(", ")),
            Value::from(def.allowed_edge_types.join(", ")),
        ]];
        Ok(QueryResult {
            columns,
            column_types: Vec::new(),
            rows,
            ..QueryResult::empty()
        })
    }

    /// Returns the graph type bound to the current graph.
    fn execute_show_current_graph_type(&self) -> Result<QueryResult> {
        let graph_name = self
            .current_graph()
            .unwrap_or_else(|| "default".to_string());
        let columns = vec![
            "graph".to_string(),
            "graph_type".to_string(),
            "open".to_string(),
            "node_types".to_string(),
            "edge_types".to_string(),
        ];

        if let Some(type_name) = self.catalog.get_graph_type_binding(&graph_name)
            && let Some(def) = self.catalog.get_graph_type_def(&type_name)
        {
            let rows = vec![vec![
                Value::from(graph_name),
                Value::from(type_name),
                Value::from(def.open),
                Value::from(def.allowed_node_types.join(", ")),
                Value::from(def.allowed_edge_types.join(", ")),
            ]];
            return Ok(QueryResult {
                columns,
                column_types: Vec::new(),
                rows,
                ..QueryResult::empty()
            });
        }

        // No graph type binding found
        Ok(QueryResult {
            columns,
            column_types: Vec::new(),
            rows: vec![vec![
                Value::from(graph_name),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ]],
            ..QueryResult::empty()
        })
    }

    /// Executes a GQL query.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    ///
    /// // Create a node
    /// session.execute("INSERT (:Person {name: 'Alix', age: 30})")?;
    ///
    /// // Query nodes
    /// let result = session.execute("MATCH (n:Person) RETURN n.name, n.age")?;
    /// for row in &result.rows {
    ///     println!("{:?}", row);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn execute(&self, query: &str) -> Result<QueryResult> {
        self.require_lpg("GQL")?;
        self.note_requete_pour_audit(query);

        use crate::query::{
            binder::Binder, cache::CacheKey, optimizer::Optimizer, processor::QueryLanguage,
            translators::gql,
        };

        let _span = tracing::info_span!(
            "obrain::session::execute",
            language = "gql",
            query_len = query.len(),
        )
        .entered();

        // `Option` plutôt qu'un `Instant` nu : sur wasm32 il n'existe AUCUNE
        // horloge (`Instant::now()` panique — « time not implemented on this
        // platform »), donc l'absence de mesure doit être REPRÉSENTABLE, pas
        // simulée. Le type reste `Option<Instant>` sur les deux cibles pour
        // que les appelants ci-dessous n'aient pas deux formes.
        #[cfg(not(target_arch = "wasm32"))]
        let start_time = Some(web_time::Instant::now());
        #[cfg(target_arch = "wasm32")]
        let start_time: Option<web_time::Instant> = None;

        // Cache key construit AVANT le parse (RFC multi-writer, gain
        // 2026-09-06) : sur un HIT du plan optimisé, le parse/translation
        // (~1 µs/op mesuré) devient inutile — le plan optimisé est
        // déterministe du texte + graphe. Les commandes session/schema ne
        // sont JAMAIS mises dans `optimized_cache` → un hit garantit un
        // `Plan`, donc sauter la translation est sûr.
        let cache_key = CacheKey::with_graph(query, QueryLanguage::Gql, self.current_graph());
        if let Some(plan_cache) = self.query_cache.get_optimized(&cache_key) {
            return self.execute_optimized_plan(plan_cache, start_time);
        }

        // Parse and translate, checking for session/schema commands first
        let translation = gql::translate_full(query)?;
        let logical_plan = match translation {
            gql::GqlTranslationResult::SessionCommand(cmd) => {
                // Une commande de session MUTE l'etat de la session et ce
                // `return` n'atteint pas `with_auto_commit`. REDONDANT avec
                // l'inversion de polarite (le jeton n'est pas encore en
                // vigueur ici) : conserve comme second verrou, parce qu'une
                // commande de session ne s'annule pas non plus.
                let _hors_vigueur = obrain_common::utils::cancel::frontiere(Some(true), None);
                return self.execute_session_command(cmd);
            }
            gql::GqlTranslationResult::SchemaCommand(cmd) => {
                // All DDL is a write operation
                if *self.read_only_tx.lock() {
                    return Err(obrain_common::utils::error::Error::Transaction(
                        obrain_common::utils::error::TransactionError::ReadOnly,
                    ));
                }
                // ── LE DDL N'ATTEINT PAS `with_auto_commit` ──────────────
                //
                // Ce `return` precede le point ou la frontiere est posee.
                // REDONDANT avec l'inversion de polarite — le jeton confie
                // par `cancel::scope` n'est pas encore en vigueur ici — mais
                // conserve comme SECOND VERROU, et celui-la se justifie :
                // le DDL construit des index (`create_vector_index`,
                // `create_text_index`, le `rebatir_index_declare` du
                // catalogue) en balayant `nodes_by_label`. Un index bati sur
                // un balayage TRONQUE est PERSISTE, et toutes les requetes
                // suivantes le croiront : le degat survivrait a la requete
                // annulee. C'est la seule categorie de degat qui survit, elle
                // merite ses deux verrous.
                let _hors_vigueur = obrain_common::utils::cancel::frontiere(Some(true), None);
                return self.execute_schema_command(cmd);
            }
            gql::GqlTranslationResult::Plan(plan) => {
                // Block mutations in read-only transactions
                if *self.read_only_tx.lock() && plan.root.has_mutations() {
                    return Err(obrain_common::utils::error::Error::Transaction(
                        obrain_common::utils::error::TransactionError::ReadOnly,
                    ));
                }
                plan
            }
        };

        // Semantic validation
        let mut binder = Binder::new();
        let _binding_context = binder.bind(&logical_plan)?;

        // Optimize the plan
        let active = self.active_store();
        let optimizer = Optimizer::from_graph_store(&*active);
        let plan = optimizer.optimize(logical_plan)?;

        // Cache the optimized plan for future use
        self.query_cache.put_optimized(cache_key, plan.clone());

        // Execute the optimized plan (shared tail with the cache-hit path).
        self.execute_optimized_plan(plan, start_time)
    }

    /// Exécute un plan optimisé (chemin commun cache-hit et cache-miss).
    ///
    /// Extraite de `execute` (2026-09-06) pour permettre le fast-path
    /// « plan optimisé en cache → sauter le parse » : le corps était
    /// inline dans `execute`, dupliquer ~120 lignes aurait été pire.
    fn execute_optimized_plan(
        &self,
        optimized_plan: crate::query::plan::LogicalPlan,
        start_time: Option<web_time::Instant>,
    ) -> Result<QueryResult> {
        use crate::query::Executor;
        let active = self.active_store();

        // ── CHEMIN CACHE-HIT : le parse a été sauté, donc le garde-fou
        //    « mutation en tx read-only » du chemin normal (match Plan)
        //    n'a pas tourné. On le reproduit ici pour les DEUX chemins
        //    (sur miss il est redondant avec le match, mais inoffensif).
        if *self.read_only_tx.lock() && optimized_plan.root.has_mutations() {
            return Err(obrain_common::utils::error::Error::Transaction(
                obrain_common::utils::error::TransactionError::ReadOnly,
            ));
        }

        // EXPLAIN: annotate pushdown hints and return the plan tree
        if optimized_plan.explain {
            use crate::query::processor::{annotate_pushdown_hints, explain_result};
            let mut plan = optimized_plan;
            annotate_pushdown_hints(&mut plan.root, active.as_ref());
            return Ok(explain_result(&plan));
        }

        // PROFILE: execute with per-operator instrumentation
        if optimized_plan.profile {
            let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));
            return self.with_auto_commit(Nature::Plan(has_mutations), || {
                let (viewing_epoch, transaction_id) = self.get_transaction_context();
                let planner = self.create_planner_for_store(
                    Arc::clone(&active),
                    viewing_epoch,
                    transaction_id,
                );
                let (mut physical_plan, entries) = planner.plan_profiled(&optimized_plan)?;

                let executor = Executor::with_columns(physical_plan.columns.clone())
                    .with_deadline(self.query_deadline());
                let _result = executor.execute(physical_plan.operator.as_mut())?;

                // wasm32 : il n'existe aucune horloge sur cette cible — la
                // mesure est ABSENTE, et `0.0` est ce que le profileur en dit.
                // Le `map` n'est jamais appelé quand `start_time` est `None`,
                // donc `Instant::elapsed()` (qui appellerait `now()`) ne part pas.
                let total_time_ms = start_time.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);

                let profile_tree = crate::query::profile::build_profile_tree(
                    &optimized_plan.root,
                    &mut entries.into_iter(),
                );
                Ok(crate::query::profile::profile_result(
                    &profile_tree,
                    total_time_ms,
                ))
            });
        }

        let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));

        let result = self.with_auto_commit(Nature::Plan(has_mutations), || {
            // Get transaction context for MVCC visibility
            let (viewing_epoch, transaction_id) = self.get_transaction_context();

            // Convert to physical plan with transaction context
            // (Physical planning cannot be cached as it depends on transaction state)
            //
            // ── CETTE PHRASE EST VRAIE, ET ELLE A ÉTÉ MESURÉE (2026-09-14) ──
            //
            // « Physical planning cannot be cached as it depends on transaction
            // state » était une affirmation sans mesure. Elle a été éprouvée
            // avec `OBRAIN_PHASES` sur une COPIE de `runtime-dev` (432 910
            // nœuds), textes UNIQUES (le cache de plans logiques ne peut donc
            // pas servir), chaque texte joué DEUX fois — `banc_plan_froid` :
            //
            //   MATCH (n) RETURN count(n) AS c<i>              planif 1-3 µs (1er ET 2e)
            //   MATCH (n:Message) WHERE n.session = '…' …      planif 192 237 µs puis
            //                                                        192 081 µs
            //   MATCH (n:Message) WHERE n.seq >= 3 …           planif 341 034 µs puis
            //                                                        316 725 µs
            //
            // Le 2e appel d'un texte coûte EXACTEMENT ce que coûte le 1er :
            // la planification physique est bien refaite, rien n'est mémoïsé.
            // (Corollaire mesuré : le « premier appel d'un texte neuf » n'est
            // pas plus cher que les suivants — seul le TOUT PREMIER appel du
            // processus paie ~10 à 15 µs d'amorçage, une fois.)
            //
            // Ce que la mesure ajoute, et qui interdit le cache — au-delà du
            // « transaction state » —, c'est POURQUOI ces centaines de
            // microsecondes sont payées : sans index, les heuristiques
            // d'accès ne rendent pas une DÉCISION, elles rendent une LISTE DE
            // NŒUDS lue dans le store (balayage de label + hydratation d'un
            // `Node` par nœud ; `find_nodes_in_range` sans série ordonnée).
            // Un plan ainsi constitué MÉMORISE DES DONNÉES : le resservir sous
            // une clé (empreinte du plan logique, génération de catalogue,
            // époque de lecture) rendrait des lignes périmées dès la première
            // écriture — et l'invalidation par époque rendrait le cache inutile
            // sur toute charge qui écrit. Le cache de plans LOGIQUES ci-dessus
            // reste légitime : un plan logique est une fonction du TEXTE.
            //
            // Ce qu'il fallait corriger n'était donc pas le cache mais le coût
            // lui-même : ces listes sont désormais plafonnées
            // (`OBRAIN_PLAN_MATERIALISATION_MAX`, défaut 1024, et
            // `OBRAIN_PLAN_ELAGAGE=0` pour l'ancien comportement), et le
            // travail repart à l'exécution pour ~0,2 µs/nœud au lieu de
            // ~2,7 µs. Mesuré après : planification 13 µs, exécution 15 ms sur
            // le cas `Message`/`session` (192 ms → 15 ms au total).
            // Détail et protocole : `examples/banc_plan_froid.rs`.
            //
            // Safe to use read-only fast path when: this query has no mutations AND
            // there is no active transaction that may have prior uncommitted writes.
            let has_active_tx = self.current_transaction.lock().is_some();
            let read_only = !has_mutations && !has_active_tx;
            let planner = self.create_planner_for_store_with_read_only(
                Arc::clone(&active),
                viewing_epoch,
                transaction_id,
                read_only,
            );
            // (2026-08-31) RUN 21 — annoter les hints de pushdown (dont
            // SourceCap) sur une COPIE du plan logique avant la planification
            // physique. `annotate_pushdown_hints` n'était appelé que pour
            // EXPLAIN ; en exécution normale le hint SourceCap (Limit → Expand
            // → NodeScan) n'atteignait jamais le ScanOperator. On clone pour
            // ne pas muter le plan mis en cache (le hint est déterministe,
            // mais le clone garde le cache immuable et sûr).
            let mut plan_annote = optimized_plan.clone();
            use crate::query::processor::annotate_pushdown_hints as annoter_hints;
            annoter_hints(&mut plan_annote.root, active.as_ref());

            // ── OÙ PASSENT LES MICROSECONDES D'UNE REQUÊTE ? (2026-09-14) ───
            //
            // Mesuré : 78 à 153 µs pour `MATCH (n) RETURN count(n)` — une
            // requête qui ne lit RIEN, servie par les compteurs. Le cache de
            // plans logiques (branche au-dessus) a déjà sauté l'analyse et
            // l'optimisation ; il ne reste donc que DEUX candidates, et on les
            // sépare sous `OBRAIN_PHASES=1` : la PLANIFICATION PHYSIQUE (les
            // heuristiques de choix d'accès — index, intervalle, préfixe,
            // trigrammes) et l'EXÉCUTION (opérateurs). Sans cette ventilation,
            // on optimise à l'aveugle.
            #[cfg(not(target_arch = "wasm32"))]
            let t_plan = web_time::Instant::now();
            let mut physical_plan = planner.plan(&plan_annote)?;
            #[cfg(not(target_arch = "wasm32"))]
            let us_plan = t_plan.elapsed().as_secs_f64() * 1e6;

            // Execute the plan
            #[cfg(not(target_arch = "wasm32"))]
            let t_exec = web_time::Instant::now();
            let executor = Executor::with_columns(physical_plan.columns.clone())
                .with_deadline(self.query_deadline());
            let mut result = executor.execute(physical_plan.operator.as_mut())?;
            #[cfg(not(target_arch = "wasm32"))]
            if std::env::var_os("OBRAIN_PHASES").is_some() {
                eprintln!(
                    "[phases] planification={us_plan:.0}us execution={:.0}us",
                    t_exec.elapsed().as_secs_f64() * 1e6
                );
            }

            // Add execution metrics
            let rows_scanned = result.rows.len() as u64;
            // `None` sur wasm32 : la mesure est laissée ABSENTE, pas
            // remplacée par un zéro qui se ferait passer pour elle.
            result.execution_time_ms = start_time.map(|t| t.elapsed().as_secs_f64() * 1000.0);
            result.rows_scanned = Some(rows_scanned);

            Ok(result)
        });

        // Record metrics for this query execution.
        #[cfg(feature = "metrics")]
        {
            // (2026-11-01) `start_time` est une Option (voir plus haut) : ce
            // bloc ne compilait que sans la feature `metrics`, et la CI ne
            // compilait jamais avec. Mesuré par `cargo check --all-features`.
            let elapsed_ms = start_time.map(|t| t.elapsed().as_secs_f64() * 1000.0);
            self.record_query_metrics("gql", elapsed_ms, &result);
        }

        result
    }

    /// Exécute une requête GQL sous un JETON D'ANNULATION.
    ///
    /// Le jeton est installé pour la durée de l'appel, sur CE thread. Armer
    /// le jeton depuis un autre thread fait remonter, au prochain point de
    /// contrôle, une erreur que `Error::is_cancelled` identifie.
    ///
    /// ── CE QUE LE JETON PEUT ET NE PEUT PAS ─────────────────────────
    ///
    /// Le jeton est CONFIÉ au thread, pas mis en vigueur : il n'agit qu'à
    /// partir de la frontière écriture/lecture (`Session::with_auto_commit`),
    /// et seulement si le plan LIT. Un plan qui écrit s'exécute exactement
    /// comme si ce mécanisme n'existait pas — voir `cancel::frontiere`.
    ///
    /// La conséquence pratique, à dire franchement : armer le jeton sur une
    /// requête d'écriture ne fait RIEN, et c'est voulu. Une écriture
    /// abandonnée en vol dépend, pour être sûre, de tout le chemin de
    /// retour arrière ; tant que ce chemin n'est pas DÉMONTRÉ, l'annulation
    /// ne s'y applique pas.
    ///
    /// # Errors
    ///
    /// `Error::Query(QueryError::cancelled())` si le jeton est armé pendant
    /// l'exécution d'une LECTURE, ou toute erreur que `execute` peut rendre.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    /// use obrain_common::utils::cancel::CancelToken;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let jeton = CancelToken::new();
    /// let chien_de_garde = jeton.clone();
    /// std::thread::spawn(move || {
    ///     std::thread::sleep(std::time::Duration::from_secs(60));
    ///     chien_de_garde.cancel();
    /// });
    /// match db.session().execute_annulable("MATCH (n) RETURN n", &jeton) {
    ///     Err(e) if e.is_cancelled() => eprintln!("annulée, cœur libéré"),
    ///     autre => { autre?; }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn execute_annulable(
        &self,
        query: &str,
        token: &obrain_common::utils::cancel::CancelToken,
    ) -> Result<QueryResult> {
        obrain_common::utils::cancel::scope(token, || self.execute(query))
    }

    /// Executes a GQL query with visibility at the specified epoch.
    ///
    /// This enables time-travel queries: the query sees the database
    /// as it existed at the given epoch.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or execution fails.
    pub fn execute_at_epoch(&self, query: &str, epoch: EpochId) -> Result<QueryResult> {
        let previous = self.viewing_epoch_override.lock().replace(epoch);
        let result = self.execute(query);
        *self.viewing_epoch_override.lock() = previous;
        result
    }

    /// Executes a GQL query at a specific epoch with optional parameters.
    ///
    /// Combines epoch-based time travel with parameterized queries.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or execution fails.
    pub fn execute_at_epoch_with_params(
        &self,
        query: &str,
        epoch: EpochId,
        params: Option<std::collections::HashMap<String, Value>>,
    ) -> Result<QueryResult> {
        let previous = self.viewing_epoch_override.lock().replace(epoch);
        let result = if let Some(p) = params {
            self.execute_with_params(query, p)
        } else {
            self.execute(query)
        };
        *self.viewing_epoch_override.lock() = previous;
        result
    }

    /// Executes a GQL query with parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    pub fn execute_with_params(
        &self,
        query: &str,
        params: std::collections::HashMap<String, Value>,
    ) -> Result<QueryResult> {
        self.require_lpg("GQL")?;
        self.note_requete_pour_audit(query);

        use crate::query::processor::{QueryLanguage, QueryProcessor};

        let has_mutations = Self::query_looks_like_mutation(query);
        let active = self.active_store();

        self.with_auto_commit(Nature::Texte(has_mutations), || {
            // Get transaction context for MVCC visibility
            let (viewing_epoch, transaction_id) = self.get_transaction_context();

            // Create processor with transaction context
            let processor = QueryProcessor::for_graph_store_with_transaction(
                Arc::clone(&active),
                Arc::clone(&self.transaction_manager),
            )?;
            let processor =
                processor.with_session_context(self.build_session_context(Arc::clone(&active)));

            // Apply transaction context if in a transaction
            let processor = if let Some(transaction_id) = transaction_id {
                processor.with_transaction_context(viewing_epoch, transaction_id)
            } else {
                processor
            };
            #[cfg(feature = "cognitive")]
            #[cfg(feature = "cognitive")]
            let processor = if let Some(engine) = self.cognitive_engine() {
                processor.with_cognitive_engine(Arc::clone(engine))
            } else {
                processor
            };
            let processor = if let Some(sub) = self.substrate_store() {
                processor.with_substrate_store(Arc::clone(sub))
            } else {
                processor
            };
            #[cfg(feature = "cdc")]
            let processor = processor.with_cdc_log(Arc::clone(self.cdc_log()));

            processor.process(query, QueryLanguage::Gql, Some(&params))
        })
    }

    /// Executes a Cypher query.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "cypher")]
    pub fn execute_cypher(&self, query: &str) -> Result<QueryResult> {
        self.note_requete_pour_audit(query);
        use crate::query::{
            Executor, binder::Binder, cache::CacheKey, optimizer::Optimizer,
            processor::QueryLanguage, translators::cypher,
        };
        use obrain_common::utils::error::{Error as ObrainError, QueryError, QueryErrorKind};

        // Handle schema DDL and SHOW commands before the normal query path
        let translation = cypher::translate_full(query)?;
        match translation {
            cypher::CypherTranslationResult::SchemaCommand(cmd) => {
                if *self.read_only_tx.lock() {
                    return Err(ObrainError::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        "Cannot execute schema DDL in a read-only transaction",
                    )));
                }
                return self.execute_schema_command(cmd);
            }
            cypher::CypherTranslationResult::ShowIndexes => {
                return self.execute_show_indexes();
            }
            cypher::CypherTranslationResult::ShowConstraints => {
                return self.execute_show_constraints();
            }
            cypher::CypherTranslationResult::ShowCurrentGraphType => {
                return self.execute_show_current_graph_type();
            }
            cypher::CypherTranslationResult::Plan(_) => {
                // Fall through to normal execution below
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        let start_time = web_time::Instant::now();

        // Create cache key for this query
        let cache_key = CacheKey::with_graph(query, QueryLanguage::Cypher, self.current_graph());

        // Try to get cached optimized plan
        let optimized_plan = if let Some(cached_plan) = self.query_cache.get_optimized(&cache_key) {
            cached_plan
        } else {
            // Parse and translate the query to a logical plan
            let logical_plan = cypher::translate(query)?;

            // Semantic validation
            let mut binder = Binder::new();
            let _binding_context = binder.bind(&logical_plan)?;

            // Optimize the plan
            let active = self.active_store();
            let optimizer = Optimizer::from_graph_store(&*active);
            let plan = optimizer.optimize(logical_plan)?;

            // Cache the optimized plan
            self.query_cache.put_optimized(cache_key, plan.clone());

            plan
        };

        // Resolve the active store for query execution
        let active = self.active_store();

        // EXPLAIN
        if optimized_plan.explain {
            use crate::query::processor::{annotate_pushdown_hints, explain_result};
            let mut plan = optimized_plan;
            annotate_pushdown_hints(&mut plan.root, active.as_ref());
            return Ok(explain_result(&plan));
        }

        // PROFILE
        if optimized_plan.profile {
            let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));
            return self.with_auto_commit(Nature::Plan(has_mutations), || {
                let (viewing_epoch, transaction_id) = self.get_transaction_context();
                let planner = self.create_planner_for_store(
                    Arc::clone(&active),
                    viewing_epoch,
                    transaction_id,
                );
                let (mut physical_plan, entries) = planner.plan_profiled(&optimized_plan)?;

                let executor = Executor::with_columns(physical_plan.columns.clone())
                    .with_deadline(self.query_deadline());
                let _result = executor.execute(physical_plan.operator.as_mut())?;

                let total_time_ms;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    total_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    total_time_ms = 0.0;
                }

                let profile_tree = crate::query::profile::build_profile_tree(
                    &optimized_plan.root,
                    &mut entries.into_iter(),
                );
                Ok(crate::query::profile::profile_result(
                    &profile_tree,
                    total_time_ms,
                ))
            });
        }

        let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));

        let result = self.with_auto_commit(Nature::Plan(has_mutations), || {
            // Get transaction context for MVCC visibility
            let (viewing_epoch, transaction_id) = self.get_transaction_context();

            // Convert to physical plan with transaction context
            let planner =
                self.create_planner_for_store(Arc::clone(&active), viewing_epoch, transaction_id);
            let mut physical_plan = planner.plan(&optimized_plan)?;

            // Execute the plan
            let executor = Executor::with_columns(physical_plan.columns.clone())
                .with_deadline(self.query_deadline());
            executor.execute(physical_plan.operator.as_mut())
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("cypher", elapsed_ms, &result);
        }

        result
    }

    /// Executes a Gremlin query.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    ///
    /// // Create some nodes first
    /// session.create_node(&["Person"]);
    ///
    /// // Query using Gremlin
    /// let result = session.execute_gremlin("g.V().hasLabel('Person')")?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "gremlin")]
    pub fn execute_gremlin(&self, query: &str) -> Result<QueryResult> {
        use crate::query::{Executor, binder::Binder, optimizer::Optimizer, translators::gremlin};

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        // Parse and translate the query to a logical plan
        let logical_plan = gremlin::translate(query)?;

        // Semantic validation
        let mut binder = Binder::new();
        let _binding_context = binder.bind(&logical_plan)?;

        // Optimize the plan
        let active = self.active_store();
        let optimizer = Optimizer::from_graph_store(&*active);
        let optimized_plan = optimizer.optimize(logical_plan)?;

        let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));

        let result = self.with_auto_commit(Nature::Plan(has_mutations), || {
            // Get transaction context for MVCC visibility
            let (viewing_epoch, transaction_id) = self.get_transaction_context();

            // Convert to physical plan with transaction context
            let planner =
                self.create_planner_for_store(Arc::clone(&active), viewing_epoch, transaction_id);
            let mut physical_plan = planner.plan(&optimized_plan)?;

            // Execute the plan
            let executor = Executor::with_columns(physical_plan.columns.clone())
                .with_deadline(self.query_deadline());
            executor.execute(physical_plan.operator.as_mut())
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("gremlin", elapsed_ms, &result);
        }

        result
    }

    /// Executes a Gremlin query with parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "gremlin")]
    pub fn execute_gremlin_with_params(
        &self,
        query: &str,
        params: std::collections::HashMap<String, Value>,
    ) -> Result<QueryResult> {
        use crate::query::processor::{QueryLanguage, QueryProcessor};

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let has_mutations = Self::query_looks_like_mutation(query);
        let active = self.active_store();

        let result = self.with_auto_commit(Nature::Texte(has_mutations), || {
            let (viewing_epoch, transaction_id) = self.get_transaction_context();
            let processor = QueryProcessor::for_graph_store_with_transaction(
                Arc::clone(&active),
                Arc::clone(&self.transaction_manager),
            )?;
            let processor =
                processor.with_session_context(self.build_session_context(Arc::clone(&active)));
            let processor = if let Some(transaction_id) = transaction_id {
                processor.with_transaction_context(viewing_epoch, transaction_id)
            } else {
                processor
            };
            #[cfg(feature = "cognitive")]
            let processor = if let Some(engine) = self.cognitive_engine() {
                processor.with_cognitive_engine(Arc::clone(engine))
            } else {
                processor
            };
            let processor = if let Some(sub) = self.substrate_store() {
                processor.with_substrate_store(Arc::clone(sub))
            } else {
                processor
            };
            #[cfg(feature = "cdc")]
            let processor = processor.with_cdc_log(Arc::clone(self.cdc_log()));
            processor.process(query, QueryLanguage::Gremlin, Some(&params))
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("gremlin", elapsed_ms, &result);
        }

        result
    }

    /// Executes a GraphQL query against the LPG store.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    ///
    /// // Create some nodes first
    /// session.create_node(&["User"]);
    ///
    /// // Query using GraphQL
    /// let result = session.execute_graphql("query { user { id name } }")?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "graphql")]
    pub fn execute_graphql(&self, query: &str) -> Result<QueryResult> {
        use crate::query::{Executor, binder::Binder, optimizer::Optimizer, translators::graphql};

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let logical_plan = graphql::translate(query)?;
        let mut binder = Binder::new();
        let _binding_context = binder.bind(&logical_plan)?;

        let active = self.active_store();
        let optimizer = Optimizer::from_graph_store(&*active);
        let optimized_plan = optimizer.optimize(logical_plan)?;
        let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));

        let result = self.with_auto_commit(Nature::Plan(has_mutations), || {
            let (viewing_epoch, transaction_id) = self.get_transaction_context();
            let planner =
                self.create_planner_for_store(Arc::clone(&active), viewing_epoch, transaction_id);
            let mut physical_plan = planner.plan(&optimized_plan)?;
            let executor = Executor::with_columns(physical_plan.columns.clone())
                .with_deadline(self.query_deadline());
            executor.execute(physical_plan.operator.as_mut())
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("graphql", elapsed_ms, &result);
        }

        result
    }

    /// Executes a GraphQL query with parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "graphql")]
    pub fn execute_graphql_with_params(
        &self,
        query: &str,
        params: std::collections::HashMap<String, Value>,
    ) -> Result<QueryResult> {
        use crate::query::processor::{QueryLanguage, QueryProcessor};

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let has_mutations = Self::query_looks_like_mutation(query);
        let active = self.active_store();

        let result = self.with_auto_commit(Nature::Texte(has_mutations), || {
            let (viewing_epoch, transaction_id) = self.get_transaction_context();
            let processor = QueryProcessor::for_graph_store_with_transaction(
                Arc::clone(&active),
                Arc::clone(&self.transaction_manager),
            )?;
            let processor =
                processor.with_session_context(self.build_session_context(Arc::clone(&active)));
            let processor = if let Some(transaction_id) = transaction_id {
                processor.with_transaction_context(viewing_epoch, transaction_id)
            } else {
                processor
            };
            #[cfg(feature = "cognitive")]
            let processor = if let Some(engine) = self.cognitive_engine() {
                processor.with_cognitive_engine(Arc::clone(engine))
            } else {
                processor
            };
            let processor = if let Some(sub) = self.substrate_store() {
                processor.with_substrate_store(Arc::clone(sub))
            } else {
                processor
            };
            #[cfg(feature = "cdc")]
            let processor = processor.with_cdc_log(Arc::clone(self.cdc_log()));
            processor.process(query, QueryLanguage::GraphQL, Some(&params))
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("graphql", elapsed_ms, &result);
        }

        result
    }

    /// Executes a SQL/PGQ query (SQL:2023 GRAPH_TABLE).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    ///
    /// let result = session.execute_sql(
    ///     "SELECT * FROM GRAPH_TABLE (
    ///         MATCH (n:Person)
    ///         COLUMNS (n.name AS name)
    ///     )"
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "sql-pgq")]
    pub fn execute_sql(&self, query: &str) -> Result<QueryResult> {
        use crate::query::{
            Executor, binder::Binder, cache::CacheKey, optimizer::Optimizer, plan::LogicalOperator,
            processor::QueryLanguage, translators::sql_pgq,
        };

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        // Parse and translate (always needed to check for DDL)
        let logical_plan = sql_pgq::translate(query)?;

        // Handle DDL statements directly (they don't go through the query pipeline)
        if let LogicalOperator::CreatePropertyGraph(ref cpg) = logical_plan.root {
            return Ok(QueryResult {
                columns: vec!["status".into()],
                column_types: vec![obrain_common::types::LogicalType::String],
                rows: vec![vec![Value::from(format!(
                    "Property graph '{}' created ({} node tables, {} edge tables)",
                    cpg.name,
                    cpg.node_tables.len(),
                    cpg.edge_tables.len()
                ))]],
                execution_time_ms: None,
                rows_scanned: None,
                status_message: None,
                gql_status: obrain_common::utils::GqlStatus::SUCCESS,
            });
        }

        let cache_key = CacheKey::with_graph(query, QueryLanguage::SqlPgq, self.current_graph());

        let optimized_plan = if let Some(cached_plan) = self.query_cache.get_optimized(&cache_key) {
            cached_plan
        } else {
            let mut binder = Binder::new();
            let _binding_context = binder.bind(&logical_plan)?;
            let active = self.active_store();
            let optimizer = Optimizer::from_graph_store(&*active);
            let plan = optimizer.optimize(logical_plan)?;
            self.query_cache.put_optimized(cache_key, plan.clone());
            plan
        };

        let active = self.active_store();
        let has_mutations = optimized_plan.root.mutations_avec(Some(&self.catalog));

        let result = self.with_auto_commit(Nature::Plan(has_mutations), || {
            let (viewing_epoch, transaction_id) = self.get_transaction_context();
            let planner =
                self.create_planner_for_store(Arc::clone(&active), viewing_epoch, transaction_id);
            let mut physical_plan = planner.plan(&optimized_plan)?;
            let executor = Executor::with_columns(physical_plan.columns.clone())
                .with_deadline(self.query_deadline());
            executor.execute(physical_plan.operator.as_mut())
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("sql", elapsed_ms, &result);
        }

        result
    }

    /// Executes a SQL/PGQ query with parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "sql-pgq")]
    pub fn execute_sql_with_params(
        &self,
        query: &str,
        params: std::collections::HashMap<String, Value>,
    ) -> Result<QueryResult> {
        use crate::query::processor::{QueryLanguage, QueryProcessor};

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let has_mutations = Self::query_looks_like_mutation(query);
        let active = self.active_store();

        let result = self.with_auto_commit(Nature::Texte(has_mutations), || {
            let (viewing_epoch, transaction_id) = self.get_transaction_context();
            let processor = QueryProcessor::for_graph_store_with_transaction(
                Arc::clone(&active),
                Arc::clone(&self.transaction_manager),
            )?;
            let processor =
                processor.with_session_context(self.build_session_context(Arc::clone(&active)));
            let processor = if let Some(transaction_id) = transaction_id {
                processor.with_transaction_context(viewing_epoch, transaction_id)
            } else {
                processor
            };
            #[cfg(feature = "cognitive")]
            let processor = if let Some(engine) = self.cognitive_engine() {
                processor.with_cognitive_engine(Arc::clone(engine))
            } else {
                processor
            };
            let processor = if let Some(sub) = self.substrate_store() {
                processor.with_substrate_store(Arc::clone(sub))
            } else {
                processor
            };
            #[cfg(feature = "cdc")]
            let processor = processor.with_cdc_log(Arc::clone(self.cdc_log()));
            processor.process(query, QueryLanguage::SqlPgq, Some(&params))
        });

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("sql", elapsed_ms, &result);
        }

        result
    }

    /// Executes a query in the specified language by name.
    ///
    /// Supported language names: `"gql"`, `"cypher"`, `"gremlin"`, `"graphql"`,
    /// `"graphql-rdf"`, `"sparql"`, `"sql"`. Each requires the corresponding feature flag.
    ///
    /// # Errors
    ///
    /// Returns an error if the language is unknown/disabled or the query fails.
    pub fn execute_language(
        &self,
        query: &str,
        language: &str,
        params: Option<std::collections::HashMap<String, Value>>,
    ) -> Result<QueryResult> {
        let _span = tracing::info_span!(
            "obrain::session::execute",
            language,
            query_len = query.len(),
        )
        .entered();
        match language {
            "gql" => {
                if let Some(p) = params {
                    self.execute_with_params(query, p)
                } else {
                    self.execute(query)
                }
            }
            #[cfg(feature = "cypher")]
            "cypher" => {
                if let Some(p) = params {
                    use crate::query::processor::{QueryLanguage, QueryProcessor};

                    #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
                    let start_time = Instant::now();

                    let has_mutations = Self::query_looks_like_mutation(query);
                    let active = self.active_store();
                    let result = self.with_auto_commit(Nature::Texte(has_mutations), || {
                        let processor = QueryProcessor::for_graph_store_with_transaction(
                            Arc::clone(&active),
                            Arc::clone(&self.transaction_manager),
                        )?;
                        let processor = processor
                            .with_session_context(self.build_session_context(Arc::clone(&active)));
                        let (viewing_epoch, transaction_id) = self.get_transaction_context();
                        let processor = if let Some(transaction_id) = transaction_id {
                            processor.with_transaction_context(viewing_epoch, transaction_id)
                        } else {
                            processor
                        };
                        #[cfg(feature = "cognitive")]
                        let processor = if let Some(engine) = self.cognitive_engine() {
                            processor.with_cognitive_engine(Arc::clone(engine))
                        } else {
                            processor
                        };
                        let processor = if let Some(sub) = self.substrate_store() {
                            processor.with_substrate_store(Arc::clone(sub))
                        } else {
                            processor
                        };
                        #[cfg(feature = "cdc")]
                        let processor = processor.with_cdc_log(Arc::clone(self.cdc_log()));
                        processor.process(query, QueryLanguage::Cypher, Some(&p))
                    });

                    #[cfg(feature = "metrics")]
                    {
                        #[cfg(not(target_arch = "wasm32"))]
                        let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
                        #[cfg(target_arch = "wasm32")]
                        let elapsed_ms = None;
                        self.record_query_metrics("cypher", elapsed_ms, &result);
                    }

                    result
                } else {
                    self.execute_cypher(query)
                }
            }
            #[cfg(feature = "gremlin")]
            "gremlin" => {
                if let Some(p) = params {
                    self.execute_gremlin_with_params(query, p)
                } else {
                    self.execute_gremlin(query)
                }
            }
            #[cfg(feature = "graphql")]
            "graphql" => {
                if let Some(p) = params {
                    self.execute_graphql_with_params(query, p)
                } else {
                    self.execute_graphql(query)
                }
            }
            #[cfg(all(feature = "graphql", feature = "rdf"))]
            "graphql-rdf" => {
                if let Some(p) = params {
                    self.execute_graphql_rdf_with_params(query, p)
                } else {
                    self.execute_graphql_rdf(query)
                }
            }
            #[cfg(feature = "sql-pgq")]
            "sql" | "sql-pgq" => {
                if let Some(p) = params {
                    self.execute_sql_with_params(query, p)
                } else {
                    self.execute_sql(query)
                }
            }
            #[cfg(all(feature = "sparql", feature = "rdf"))]
            "sparql" => {
                if let Some(p) = params {
                    self.execute_sparql_with_params(query, p)
                } else {
                    self.execute_sparql(query)
                }
            }
            other => Err(obrain_common::utils::error::Error::Query(
                obrain_common::utils::error::QueryError::new(
                    obrain_common::utils::error::QueryErrorKind::Semantic,
                    format!("Unknown query language: '{other}'"),
                ),
            )),
        }
    }

    /// Begins a new transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if a transaction is already active.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let mut session = db.session();
    ///
    /// session.begin_transaction()?;
    /// session.execute("INSERT (:Person {name: 'Alix'})")?;
    /// session.execute("INSERT (:Person {name: 'Gus'})")?;
    /// session.commit()?; // Both inserts committed atomically
    /// # Ok(())
    /// # }
    /// ```
    /// Clears all cached query plans.
    ///
    /// The plan cache is shared across all sessions on the same database,
    /// so clearing from one session affects all sessions.
    pub fn clear_plan_cache(&self) {
        self.query_cache.clear();
    }

    /// Begins a new transaction on this session.
    ///
    /// Uses the default isolation level (`SnapshotIsolation`).
    ///
    /// # Errors
    ///
    /// Returns an error if a transaction is already active.
    pub fn begin_transaction(&mut self) -> Result<()> {
        self.begin_transaction_inner(false, None)
    }

    /// Begins a transaction with a specific isolation level.
    ///
    /// See [`begin_transaction`](Self::begin_transaction) for the default (`SnapshotIsolation`).
    ///
    /// # Errors
    ///
    /// Returns an error if a transaction is already active.
    pub fn begin_transaction_with_isolation(
        &mut self,
        isolation_level: crate::transaction::IsolationLevel,
    ) -> Result<()> {
        self.begin_transaction_inner(false, Some(isolation_level))
    }

    /// Core transaction begin logic, usable from both `&mut self` and `&self` paths.
    fn begin_transaction_inner(
        &self,
        read_only: bool,
        isolation_level: Option<crate::transaction::IsolationLevel>,
    ) -> Result<()> {
        let _span = tracing::debug_span!("obrain::tx::begin", read_only).entered();
        let mut current = self.current_transaction.lock();
        if current.is_some() {
            // Nested transaction: create an auto-savepoint instead of a new tx.
            drop(current);
            let mut depth = self.transaction_nesting_depth.lock();
            *depth += 1;
            let sp_name = format!("_nested_tx_{}", *depth);
            self.savepoint(&sp_name)?;
            return Ok(());
        }

        let active = self.active_store();
        self.transaction_start_node_count
            .store(active.node_count(), Ordering::Relaxed);
        self.transaction_start_edge_count
            .store(active.edge_count(), Ordering::Relaxed);
        let transaction_id = if let Some(level) = isolation_level {
            self.transaction_manager.begin_with_isolation(level)
        } else {
            self.transaction_manager.begin()
        };
        *current = Some(transaction_id);
        *self.read_only_tx.lock() = read_only || self.db_read_only;

        // Record the initial graph as "touched" for cross-graph atomicity.
        // Uses the full storage key (schema/graph) for schema-scoped resolution.
        let key = self.active_graph_storage_key();
        let mut touched = self.touched_graphs.lock();
        touched.clear();
        touched.push(key.clone());
        drop(touched);
        self.tx_baselines.lock().clear();
        self.pose_empreinte(&key);

        #[cfg(feature = "metrics")]
        {
            crate::metrics::record_metric!(self.metrics, tx_active, inc);
            #[cfg(not(target_arch = "wasm32"))]
            {
                *self.tx_start_time.lock() = Some(Instant::now());
            }
        }

        Ok(())
    }

    /// Commits the current transaction.
    ///
    /// Makes all changes since [`begin_transaction`](Self::begin_transaction) permanent.
    ///
    /// # Errors
    ///
    /// Returns an error if no transaction is active.
    pub fn commit(&mut self) -> Result<()> {
        self.commit_inner()
    }

    /// Core commit logic, usable from both `&mut self` and `&self` paths.
    fn commit_inner(&self) -> Result<()> {
        let _span = tracing::debug_span!("obrain::tx::commit").entered();
        // Nested transaction: release the auto-savepoint (changes are preserved).
        {
            let mut depth = self.transaction_nesting_depth.lock();
            if *depth > 0 {
                let sp_name = format!("_nested_tx_{depth}");
                *depth -= 1;
                drop(depth);
                return self.release_savepoint(&sp_name);
            }
        }

        let transaction_id = self.current_transaction.lock().take().ok_or_else(|| {
            obrain_common::utils::error::Error::Transaction(
                obrain_common::utils::error::TransactionError::InvalidState(
                    "No active transaction".to_string(),
                ),
            )
        })?;

        // Validate the transaction first (conflict detection) before committing data.
        // If this fails, we rollback the data changes instead of making them permanent.
        let touched = self.touched_graphs.lock().clone();
        let commit_epoch = match self.transaction_manager.commit(transaction_id) {
            Ok(epoch) => epoch,
            Err(e) => {
                // Conflict detected: rollback the data changes
                for graph_name in &touched {
                    let store = self.resolve_store(graph_name);
                    store.rollback_transaction_properties(transaction_id);
                    // Parité avec rollback_inner : écarter aussi les versions
                    // en attente (no-op sur substrat, mais le contrat est le
                    // même pour tout backend).
                    store.discard_uncommitted_versions(transaction_id);
                }
                #[cfg(feature = "rdf")]
                self.rollback_rdf_transaction(transaction_id);
                // (2026-11-01, audit v0.0.1) REMBOBINER AVANT DE VIDER.
                // `rembobine_aux_empreintes` lit `tx_baselines` ; l'ancien
                // ordre vidait la liste ICI puis rembobinait plus bas sur une
                // liste vide : rien n'était retiré, et les arêtes d'une
                // transaction refusée restaient visibles de toutes les autres
                // sessions (test `echec_commit_rembobinage`). Même ordre que
                // `rollback_inner`, désormais.
                self.rembobine_aux_empreintes(transaction_id);
                *self.read_only_tx.lock() = self.db_read_only;
                self.savepoints.lock().clear();
                self.touched_graphs.lock().clear();
                self.tx_baselines.lock().clear();
                #[cfg(feature = "metrics")]
                {
                    crate::metrics::record_metric!(self.metrics, tx_active, dec);
                    crate::metrics::record_metric!(self.metrics, tx_conflicts, inc);
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(start) = self.tx_start_time.lock().take() {
                        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
                        crate::metrics::record_metric!(
                            self.metrics,
                            tx_duration,
                            observe duration_ms
                        );
                    }
                }
                // (2026-08-17, mission perf — LE POISON) Cette branche ne
                // marquait JAMAIS la transaction Aborted au manager : elle
                // restait Active pour toujours, son write_set EMPOISONNAIT
                // chaque futur ecrivain des memes entites (record_write,
                // premier-ecrivain-gagne, voit un porteur Actif fantome) et
                // active_count fuyait. Mesure sur la fixture agent : apres
                // UN conflit de validation, MERGE sequentiel sur le noeud =
                // echec A VIE (OBRAIN-T001), immunise contre tout backoff —
                // la signature exacte des 225 conflits sur un seul NodeId
                // et des conflits << soutenus >> de production.
                let _ = self.transaction_manager.abort(transaction_id);
                // Le rembobinage a eu lieu plus haut, AVANT le vidage de
                // `tx_baselines` (voir le pavé 2026-11-01).
                return Err(e);
            }
        };

        // Finalize PENDING epochs: make uncommitted versions visible at the commit epoch.
        for graph_name in &touched {
            let store = self.resolve_store(graph_name);
            store.finalize_version_epochs(transaction_id, commit_epoch);
        }

        // Commit succeeded: discard undo logs (make changes permanent)
        #[cfg(feature = "rdf")]
        self.commit_rdf_transaction(transaction_id);

        for graph_name in &touched {
            let store = self.resolve_store(graph_name);
            store.commit_transaction_properties(transaction_id);
        }

        // Sync epoch for all touched graphs so that convenience lookups
        // (edge_type, get_edge, get_node) can see versions at the latest epoch.
        let current_epoch = self.transaction_manager.current_epoch();
        for graph_name in &touched {
            let store = self.resolve_store(graph_name);
            store.sync_epoch(current_epoch);
        }

        // D-DUR (v0.0.1, 2026-11-01) : SCELLER LE COMMIT DANS LE WAL.
        // Jusqu'ici rien, sur ce chemin, n'écrivait de marqueur de commit ni
        // ne demandait de `fsync` : un `commit()` rendait `Ok` avec ses
        // records encore en tampon, et un `kill -9` les emportait (test
        // `durabilite_commit_kill9`). Le scellement vient APRÈS la
        // publication des versions : ce qui est durable est exactement ce
        // qui est visible. En cas d'erreur, l'état est visible en mémoire
        // mais pas garanti sur disque — on le dit à l'appelant plutôt que de
        // rendre un `Ok` faux.
        for graph_name in &touched {
            let store = self.resolve_store(graph_name);
            store.seal_commit(transaction_id)?;
        }

        // Reset read-only flag, clear savepoints and touched graphs
        *self.read_only_tx.lock() = self.db_read_only;
        self.savepoints.lock().clear();
        self.touched_graphs.lock().clear();
        self.tx_baselines.lock().clear();

        // Auto-GC: periodically prune old MVCC versions
        if self.gc_interval > 0 {
            let count = self.commit_counter.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_multiple_of(self.gc_interval) {
                let min_epoch = self.transaction_manager.min_active_epoch();
                for graph_name in &touched {
                    let store = self.resolve_store(graph_name);
                    store.gc_versions(min_epoch);
                }
                self.transaction_manager.gc();
                #[cfg(feature = "metrics")]
                crate::metrics::record_metric!(self.metrics, gc_runs, inc);
            }
        }

        #[cfg(feature = "metrics")]
        {
            crate::metrics::record_metric!(self.metrics, tx_active, dec);
            crate::metrics::record_metric!(self.metrics, tx_committed, inc);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(start) = self.tx_start_time.lock().take() {
                let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
                crate::metrics::record_metric!(self.metrics, tx_duration, observe duration_ms);
            }
        }

        // Reactive substrate: notify post-commit hook so it can drain pending
        // mutation events from the InstrumentedStore and publish them.
        #[cfg(feature = "reactive")]
        if let Some(ref hook) = self.post_commit_hook {
            hook(self.graph_store.as_ref());
        }

        Ok(())
    }

    /// Aborts the current transaction.
    ///
    /// Discards all changes since [`begin_transaction`](Self::begin_transaction).
    ///
    /// # Errors
    ///
    /// Returns an error if no transaction is active.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let mut session = db.session();
    ///
    /// session.begin_transaction()?;
    /// session.execute("INSERT (:Person {name: 'Alix'})")?;
    /// session.rollback()?; // Insert is discarded
    /// # Ok(())
    /// # }
    /// ```
    pub fn rollback(&mut self) -> Result<()> {
        self.rollback_inner()
    }

    /// Core rollback logic, usable from both `&mut self` and `&self` paths.
    fn rollback_inner(&self) -> Result<()> {
        let _span = tracing::debug_span!("obrain::tx::rollback").entered();
        // Nested transaction: rollback to the auto-savepoint.
        {
            let mut depth = self.transaction_nesting_depth.lock();
            if *depth > 0 {
                let sp_name = format!("_nested_tx_{depth}");
                *depth -= 1;
                drop(depth);
                return self.rollback_to_savepoint(&sp_name);
            }
        }

        let transaction_id = self.current_transaction.lock().take().ok_or_else(|| {
            obrain_common::utils::error::Error::Transaction(
                obrain_common::utils::error::TransactionError::InvalidState(
                    "No active transaction".to_string(),
                ),
            )
        })?;

        // Reset read-only flag
        *self.read_only_tx.lock() = self.db_read_only;

        // Discard uncommitted versions in ALL touched LPG stores (cross-graph atomicity).
        let touched = self.touched_graphs.lock().clone();
        for graph_name in &touched {
            let store = self.resolve_store(graph_name);
            store.discard_uncommitted_versions(transaction_id);
        }

        // (2026-08-13) Rembobinage REEL. `discard_uncommitted_versions` ci-dessus
        // est un no-op sur substrat — il n'y a pas de versions en attente, les
        // ecritures sont deja dans les zones. On supprime donc les entites
        // allouees depuis l'empreinte de chaque graphe touche.
        self.rembobine_aux_empreintes(transaction_id);

        // Discard pending operations in the RDF store
        #[cfg(feature = "rdf")]
        self.rollback_rdf_transaction(transaction_id);

        // Clear savepoints and touched graphs
        self.savepoints.lock().clear();
        self.touched_graphs.lock().clear();
        self.tx_baselines.lock().clear();

        // Mark transaction as aborted in the manager
        let result = self.transaction_manager.abort(transaction_id);

        #[cfg(feature = "metrics")]
        if result.is_ok() {
            crate::metrics::record_metric!(self.metrics, tx_active, dec);
            crate::metrics::record_metric!(self.metrics, tx_rolled_back, inc);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(start) = self.tx_start_time.lock().take() {
                let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
                crate::metrics::record_metric!(self.metrics, tx_duration, observe duration_ms);
            }
        }

        result
    }

    /// Creates a named savepoint within the current transaction.
    ///
    /// The savepoint captures the current node/edge ID counters so that
    /// [`rollback_to_savepoint`](Self::rollback_to_savepoint) can discard
    /// entities created after this point.
    ///
    /// # Errors
    ///
    /// Returns an error if no transaction is active.
    pub fn savepoint(&self, name: &str) -> Result<()> {
        let tx_id = self.current_transaction.lock().ok_or_else(|| {
            obrain_common::utils::error::Error::Transaction(
                obrain_common::utils::error::TransactionError::InvalidState(
                    "No active transaction".to_string(),
                ),
            )
        })?;

        // Capture state for every graph touched so far.
        let touched = self.touched_graphs.lock().clone();
        let graph_snapshots: Vec<GraphSavepoint> = touched
            .iter()
            .map(|graph_name| {
                let store = self.resolve_store(graph_name);
                GraphSavepoint {
                    graph_name: graph_name.clone(),
                    next_node_id: store.peek_next_node_id(),
                    next_edge_id: store.peek_next_edge_id(),
                    undo_log_position: store.property_undo_log_position(tx_id),
                }
            })
            .collect();

        self.savepoints.lock().push(SavepointState {
            name: name.to_string(),
            graph_snapshots,
            active_graph: self.current_graph.lock().clone(),
        });
        Ok(())
    }

    /// Rolls back to a named savepoint, undoing all writes made after it.
    ///
    /// The savepoint and any savepoints created after it are removed.
    /// Entities with IDs >= the savepoint snapshot are discarded.
    ///
    /// # Errors
    ///
    /// Returns an error if no transaction is active or the savepoint does not exist.
    pub fn rollback_to_savepoint(&self, name: &str) -> Result<()> {
        let transaction_id = self.current_transaction.lock().ok_or_else(|| {
            obrain_common::utils::error::Error::Transaction(
                obrain_common::utils::error::TransactionError::InvalidState(
                    "No active transaction".to_string(),
                ),
            )
        })?;

        let mut savepoints = self.savepoints.lock();

        // Find the savepoint by name (search from the end for nested savepoints)
        let pos = savepoints
            .iter()
            .rposition(|sp| sp.name == name)
            .ok_or_else(|| {
                obrain_common::utils::error::Error::Transaction(
                    obrain_common::utils::error::TransactionError::InvalidState(format!(
                        "Savepoint '{name}' not found"
                    )),
                )
            })?;

        let sp_state = savepoints[pos].clone();

        // Remove this savepoint and all later ones
        savepoints.truncate(pos);
        drop(savepoints);

        // (2026-08-13) LES GRAPHES ENTRES APRES LE POINT DE SAUVEGARDE.
        //
        // `graph_snapshots` ne contient que les graphes deja touches AU MOMENT
        // du SAVEPOINT. Un `USE GRAPH alpha` fait ensuite entrer alpha dans la
        // transaction : il n'a aucune photo, donc rien ne le rembobinait, et
        // ses ecritures survivaient a un retour au point de sauvegarde.
        //
        // Or son cas est le plus simple de tous : s'il n'etait pas touche au
        // moment du SAVEPOINT, alors TOUT ce qu'il a recu dans cette
        // transaction est posterieur au point de sauvegarde. Son empreinte de
        // premiere touche fait donc exactement office de photo.
        let mut instantanes = sp_state.graph_snapshots.clone();
        {
            let empreintes = self.tx_baselines.lock();
            for (cle, (n0, e0)) in empreintes.iter() {
                if instantanes.iter().any(|gs| &gs.graph_name == cle) {
                    continue;
                }
                instantanes.push(GraphSavepoint {
                    graph_name: cle.clone(),
                    next_node_id: *n0,
                    next_edge_id: *e0,
                    // Pas de journal de proprietes a rembobiner : le graphe
                    // n'existait pas dans la transaction avant le point de
                    // sauvegarde, donc aucune de ses proprietes preexistantes
                    // n'a pu etre modifiee avant lui.
                    undo_log_position: 0,
                });
            }
        }

        // Roll back each graph that was captured in the savepoint.
        for gs in &instantanes {
            let store = self.resolve_store(&gs.graph_name);

            // Replay property/label undo entries recorded after the savepoint
            store.rollback_transaction_properties_to(transaction_id, gs.undo_log_position);

            // Discard entities created after the savepoint
            let current_next_node = store.peek_next_node_id();
            let current_next_edge = store.peek_next_edge_id();

            // GARDE DU ZERO. Le trait documente `0` comme « ce backend ne
            // sait pas rembobiner ». Sans ce test, `(0..courant)` designerait
            // la base ENTIERE et le retour au point de sauvegarde effacerait
            // tout. La garde etait inoffensive tant que
            // `discard_entities_by_id` ne faisait rien ; elle devient
            // indispensable maintenant qu'il agit.
            if current_next_node == 0 && current_next_edge == 0 {
                continue;
            }
            let node_ids: Vec<NodeId> = (gs.next_node_id..current_next_node)
                .map(NodeId::new)
                .collect();
            let edge_ids: Vec<EdgeId> = (gs.next_edge_id..current_next_edge)
                .map(EdgeId::new)
                .collect();

            if !node_ids.is_empty() || !edge_ids.is_empty() {
                store.discard_entities_by_id(transaction_id, &node_ids, &edge_ids);
            }
        }

        // Also roll back any graphs that were touched AFTER the savepoint
        // but not captured in it. These need full discard since the savepoint
        // didn't include them.
        let touched = self.touched_graphs.lock().clone();
        for graph_name in &touched {
            let already_captured = sp_state
                .graph_snapshots
                .iter()
                .any(|gs| gs.graph_name == *graph_name);
            if !already_captured {
                let store = self.resolve_store(graph_name);
                store.discard_uncommitted_versions(transaction_id);
            }
        }

        // Restore touched_graphs to only the graphs that were known at savepoint time.
        let mut touched = self.touched_graphs.lock();
        touched.clear();
        for gs in &sp_state.graph_snapshots {
            if !touched.contains(&gs.graph_name) {
                touched.push(gs.graph_name.clone());
            }
        }

        Ok(())
    }

    /// Releases (removes) a named savepoint without rolling back.
    ///
    /// # Errors
    ///
    /// Returns an error if no transaction is active or the savepoint does not exist.
    pub fn release_savepoint(&self, name: &str) -> Result<()> {
        let _tx_id = self.current_transaction.lock().ok_or_else(|| {
            obrain_common::utils::error::Error::Transaction(
                obrain_common::utils::error::TransactionError::InvalidState(
                    "No active transaction".to_string(),
                ),
            )
        })?;

        let mut savepoints = self.savepoints.lock();
        let pos = savepoints
            .iter()
            .rposition(|sp| sp.name == name)
            .ok_or_else(|| {
                obrain_common::utils::error::Error::Transaction(
                    obrain_common::utils::error::TransactionError::InvalidState(format!(
                        "Savepoint '{name}' not found"
                    )),
                )
            })?;
        savepoints.remove(pos);
        Ok(())
    }

    /// Returns whether a transaction is active.
    #[must_use]
    pub fn in_transaction(&self) -> bool {
        self.current_transaction.lock().is_some()
    }

    /// Returns the current transaction ID, if any.
    #[must_use]
    pub(crate) fn current_transaction_id(&self) -> Option<TransactionId> {
        *self.current_transaction.lock()
    }

    /// Returns a reference to the transaction manager.
    #[must_use]
    pub(crate) fn transaction_manager(&self) -> &TransactionManager {
        &self.transaction_manager
    }

    /// Returns the store's current node count and the count at transaction start.
    #[must_use]
    pub(crate) fn node_count_delta(&self) -> (usize, usize) {
        (
            self.transaction_start_node_count.load(Ordering::Relaxed),
            self.active_store().node_count(),
        )
    }

    /// Returns the store's current edge count and the count at transaction start.
    #[must_use]
    pub(crate) fn edge_count_delta(&self) -> (usize, usize) {
        (
            self.transaction_start_edge_count.load(Ordering::Relaxed),
            self.active_store().edge_count(),
        )
    }

    /// Prepares the current transaction for a two-phase commit.
    ///
    /// Returns a [`PreparedCommit`](crate::transaction::PreparedCommit) that
    /// lets you inspect pending changes and attach metadata before finalizing.
    /// The mutable borrow prevents concurrent operations while the commit is
    /// pending.
    ///
    /// If the `PreparedCommit` is dropped without calling `commit()` or
    /// `abort()`, the transaction is automatically rolled back.
    ///
    /// # Errors
    ///
    /// Returns an error if no transaction is active.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let mut session = db.session();
    ///
    /// session.begin_transaction()?;
    /// session.execute("INSERT (:Person {name: 'Alix'})")?;
    ///
    /// let mut prepared = session.prepare_commit()?;
    /// println!("Nodes written: {}", prepared.info().nodes_written);
    /// prepared.set_metadata("audit_user", "admin");
    /// prepared.commit()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn prepare_commit(&mut self) -> Result<crate::transaction::PreparedCommit<'_>> {
        crate::transaction::PreparedCommit::new(self)
    }

    /// Sets auto-commit mode.
    pub fn set_auto_commit(&mut self, auto_commit: bool) {
        self.auto_commit = auto_commit;
    }

    /// Returns whether auto-commit is enabled.
    #[must_use]
    pub fn auto_commit(&self) -> bool {
        self.auto_commit
    }

    /// Returns `true` if auto-commit should wrap this execution.
    ///
    /// Auto-commit kicks in when: the session is in auto-commit mode,
    /// no explicit transaction is active, and the query mutates data.
    fn needs_auto_commit(&self, has_mutations: bool) -> bool {
        self.auto_commit && has_mutations && self.current_transaction.lock().is_none()
    }

    /// Wraps `body` in an automatic begin/commit when [`needs_auto_commit`]
    /// returns `true`. On error the transaction is rolled back.
    fn with_auto_commit<F>(&self, nature: Nature, mut body: F) -> Result<QueryResult>
    where
        F: FnMut() -> Result<QueryResult>,
    {
        let has_mutations = nature.mute();
        // ── LA FRONTIÈRE ÉCRITURE / LECTURE, POSÉE ICI ET NULLE PART
        //    AILLEURS (2026-08-24, revue adversariale) ──────────────────
        //
        // POURQUOI CE POINT, ET CE QU'IL NE GARANTIT PAS. Les treize
        // entrées d'exécution LPG — `execute`, sa branche `PROFILE`,
        // `execute_with_params`, les mêmes pour cypher / gremlin / graphql
        // / sql, `execute_language`, et `execute_graphql_rdf_with_params` —
        // passent toutes ici, parce que c'est ici que se décide l'ouverture
        // de la transaction implicite.
        //
        // Une version antérieure de ce pavé affirmait qu'une entrée qui
        // écrit sans traverser ce point « n'ouvre aucune transaction, donc
        // ne fonctionne pas ». C'est FAUX, et la revue l'a montré :
        // `execute_graphql_rdf`, `execute_sparql` et
        // `execute_sparql_with_params` écrivent sans passer par ici — le
        // chemin RDF a son propre couple `commit_rdf_transaction` /
        // `rollback_rdf_transaction`. Ce point est donc très emprunté, pas
        // universel, et c'est précisément pourquoi il ne peut pas être la
        // seule protection.
        //
        // Ce sont ces trois-là qui valident l'inversion de polarité : elles
        // écrivent, elles ne traversent aucune frontière, et elles restent
        // pourtant HORS VIGUEUR — donc sûres — sans qu'une seule ligne ne
        // les vise.
        //
        // La version précédente posait la même décision à la main devant
        // chaque `with_auto_commit`. Sept sites, quatre oublis — dont
        // `PROFILE CREATE`, prouvé annulable par exécution. Reposer la
        // frontière à la main à chaque entrée garantissait un huitième
        // oubli ; elle est donc posée une fois, ici, en amont de tout —
        // planification physique comprise, car le planificateur balaie lui
        // aussi.
        //
        // L'ÉCRITURE FAUSSE, elle, a bien eu lieu, mais par une autre porte
        // que celle qu'on croyait : `has_mutations()` classait
        // `CallProcedure` en lecture seule, si bien qu'un `CALL` de
        // procédure annulé en vol laissait 4 096 nœuds sur 30 000, sans
        // transaction et sans retour arrière. La frontière ne vaut que ce
        // que vaut la réponse qu'on lui donne : voir le pavé de
        // `LogicalOperator::has_mutations` (`plan.rs`).
        //
        // DEUX QUESTIONS, DEUX RÉPONSES, ET C'EST `Nature` QUI LES SÉPARE.
        // La transaction implicite tolère une devinette (au pire elle
        // n'enveloppe rien — c'est le comportement d'avant) ;
        // l'interruptibilité, non : un `contains("SET")` vrai de
        // `RETURN n.offset` désarmait une LECTURE parce qu'un utilisateur
        // avait nommé une propriété `offset`. Les sept entrées qui ont le
        // plan répondent `Nature::Plan`, décision structurelle. Les cinq qui
        // n'ont que le texte répondent `Nature::Texte` : leur devinette sert
        // la transaction, et la frontière reste INDÉCISE — c'est
        // `QueryProcessor::process_lpg`, seul endroit de ce chemin où le
        // plan existe, qui tranche (`cancel::frontiere_heritee`).
        //
        // Et la polarité est inversée en amont : un jeton confié par
        // `cancel::scope` est INERTE tant que cette ligne ne l'a pas mis en
        // vigueur. Une treizième entrée ajoutée dans six mois qui oublierait
        // tout ceci ne serait pas annulable — une occasion manquée, jamais
        // une écriture fausse.
        let _frontiere =
            obrain_common::utils::cancel::frontiere(nature.frontiere(), self.query_deadline());

        if self.needs_auto_commit(has_mutations) {
            // (2026-08-17, mission perf) RE-TENTATIVE BORNEE SUR CONFLIT.
            // Mesure sur la fixture agent : 32 ecrivains x 10 MERGE sur le
            // MEME noeud = 318/320 aborts OBRAIN-T001 — un MERGE concurrent
            // sur un singleton chaud echouait a 99 %, et chaque appelant
            // devait porter son propre retry (la plupart ne le font pas).
            // Un statement AUTO-COMMIT est re-executable par construction :
            // le rollback a tout defait, on rejoue le MEME statement avec
            // un backoff court. Les transactions EXPLICITES ne re-tentent
            // JAMAIS (l appelant tient l etat).
            //
            // ── (2026-11-08, CONC-1) BORNE DE TEMPS, PAS DE NOMBRE ──────
            //
            // La borne etait un NOMBRE (12 tentatives, backoff 50 us →
            // 6,4 ms, soit 22 a 45 ms de sommeil cumule). Mesure sur
            // `unicite_merge::merge_concurrent_avec_index_reste_unique`
            // (16 fils x 12 `MERGE … SET` sur le meme noeud) sous 32
            // boucles CPU : 3/40 runs rouges ici, 13/40 sur la machine de
            // reference. Le conflit remonte est TOUJOURS celui de
            // `record_write_scoped` (manager.rs, premier-ecrivain-gagne) :
            // un AUTRE statement auto-commit tient le noeud et n a pas
            // encore valide. Sous charge, ce porteur est preempte au milieu
            // de son statement pendant un ou plusieurs quanta d ordonnanceur
            // (~10 ms chacun) ; les 15 autres fils epuisent alors leurs 12
            // essais — 45 ms au plus — en attendant UN seul porteur, et
            // le pire d entre eux doit en attendre quinze d affilee (le
            // premier-ecrivain-gagne n est pas equitable). Un nombre
            // d essais ne mesure pas la bonne grandeur : ce qu il faut
            // absorber, c est une DUREE — celle du drainage de la file des
            // contendants — et elle depend de la charge, pas de nous.
            //
            // Relire la cle avant de re-tenter ne change rien : le
            // statement rejoue DEJA en semantique MATCH des la deuxieme
            // passe (le noeud du gagnant est la), et le conflit porte sur
            // le `SET` qui suit — une ecriture legitime, voulue, sur le
            // noeud partage. Elle doit attendre son tour, pas echouer. Et
            // le conflit de VALIDATION (`TransactionManager::commit`) n est
            // pas a contourner : il refuse une mise a jour perdue.
            //
            // LE BUDGET SEUL NE SUFFIT PAS — MESURE. Avec la seule borne de
            // temps : 5 rouges sur 60 runs sous charge. Trace des perdants :
            // 18 a 29 passes en une seconde (~50 ms chacune, preemptees),
            // rejetees autant par `record_write_scoped` (porteur actif) que
            // par la validation du commit (un autre a valide le noeud
            // pendant notre passe). C est une FAMINE : l optimiste n est pas
            // equitable, la passe lente perd contre chaque passe courte. Le
            // remede est la file equitable des rejoues
            // (`TransactionManager::file_des_rejoues`) : des le premier
            // conflit, chaque passe rejouee tient le verrou de begin a
            // commit ; les rejoues passent un a un, et le porteur du verrou
            // ne concourt plus qu avec des premieres tentatives — qui, si
            // elles perdent, se rangent derriere lui.
            //
            // Le budget (une seconde) reste le garde-fou : une ERREUR
            // franche face a une transaction explicite oubliee ouverte —
            // l appelant la voit en une seconde, pas jamais. Il couvre
            // l attente dans la file. `query_timeout`, s il est pose, le
            // plafonne : on ne re-tente jamais au-dela de l echeance.
            let debut = Instant::now();
            let echeance = {
                let budget = debut + BUDGET_RETENTATIVE_CONFLIT;
                match self.query_deadline() {
                    Some(limite) if limite < budget => limite,
                    _ => budget,
                }
            };
            let mut tentative = 0u32;
            // Jeton de la file des rejoues : pris au premier conflit, tenu
            // jusqu a la sortie de la boucle (succes ou erreur finale).
            let mut jeton_file: Option<parking_lot::FairMutexGuard<'_, ()>> = None;
            loop {
                // (V3) Marque la transaction comme AUTO : les refus
                // enseignants des ecritures inter-bases ne visent que les
                // transactions EXPLICITES (in_explicit_transaction).
                self.transaction_auto
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.begin_transaction_inner(false, None)?;
                let resultat = match body() {
                    // Le conflit peut AUSSI eclore a la validation du
                    // commit — il doit passer par la meme boucle de
                    // re-tentative, pas s echapper par un .
                    Ok(result) => match self.commit_inner() {
                        Ok(()) => Ok(result),
                        Err(e) => {
                            let _ = self.rollback_inner();
                            Err(e)
                        }
                    },
                    Err(e) => {
                        let _ = self.rollback_inner();
                        Err(e)
                    }
                };
                self.transaction_auto
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                match resultat {
                    Err(e) if erreur_de_conflit_rejouable(&e) && Instant::now() < echeance => {
                        tentative += 1;
                        // Backoff micro AVEC JITTER. Mesure sans jitter :
                        // 32 ecrivains re-tentaient en LOCKSTEP (meme
                        // echeancier) et re-collisionnaient a chaque tour —
                        // 319/320 echecs malgre 8 tentatives. Le jitter
                        // desynchronise les cohortes ; la fenetre de commit
                        // est ~50 us, la dispersion suffit. Le plafond
                        // (6,4 ms) reste court : il est de l ordre d un
                        // quantum d ordonnanceur, et c est le budget de
                        // temps, pas la pause, qui absorbe la duree.
                        let plafond = 50u64 << tentative.min(7);
                        let jitter = web_time::SystemTime::now()
                            .duration_since(web_time::UNIX_EPOCH)
                            .map_or(0, |d| d.subsec_nanos() as u64)
                            % plafond.max(1);
                        let pause = Duration::from_micros(plafond / 2 + jitter);
                        // Jamais au-dela de l echeance : la derniere passe
                        // part AU plus tard a l echeance, puis l erreur
                        // remonte si elle echoue encore.
                        let restant = echeance.saturating_duration_since(Instant::now());
                        if jeton_file.is_none() {
                            // Premier conflit : on se range dans la file.
                            // L attente du verrou TIENT lieu de backoff ;
                            // elle est bornee par l echeance — au-dela, le
                            // conflit remonte tel quel.
                            match self
                                .transaction_manager
                                .file_des_rejoues()
                                .try_lock_for(restant)
                            {
                                Some(jeton) => jeton_file = Some(jeton),
                                None => return Err(e),
                            }
                        } else {
                            // Deja en tete de file : le conflit vient d une
                            // premiere tentative ou d un porteur explicite,
                            // on lui laisse le temps de valider.
                            std::thread::sleep(pause.min(restant));
                        }
                        continue;
                    }
                    autre => {
                        if tentative > 0 {
                            tracing::debug!(target: "obrain::session",
                                tentatives = tentative + 1,
                                attente_ms = debut.elapsed().as_millis() as u64,
                                ok = autre.is_ok(),
                                "statement auto-commit rejoue apres conflit(s)");
                        }
                        return autre;
                    }
                }
            }
        }
        body()
    }

    /// Heuristique : la requête a-t-elle l'AIR de muter ?
    ///
    /// Sert à décider l'ouverture de la transaction implicite sur les quatre
    /// chemins `_with_params`, qui remettent le texte au `QueryProcessor`
    /// sans jamais construire de plan côté session. Un faux négatif y est
    /// bénin : la donnée n'est simplement pas auto-committée, comme avant.
    ///
    /// ── ELLE NE DÉCIDE PLUS DE L'INTERRUPTIBILITÉ (2026-08-24) ────────
    ///
    /// Elle l'a fait, et c'était la panne d'origine qui revenait par la
    /// fenêtre : `upper.contains("SET")` est vrai de `RETURN n.offset`, donc
    /// cette LECTURE devenait ininterruptible parce qu'un utilisateur avait
    /// nommé une propriété `offset`. La décision d'interruptibilité est
    /// désormais reprise par `cancel::frontiere_heritee` dans
    /// `QueryProcessor::process_lpg`, sur `plan.root.has_mutations()` — la
    /// réponse structurelle, une fois le plan optimisé disponible.
    ///
    /// Le test de sous-chaîne est néanmoins remplacé par un test de MOT :
    /// il reste faux pour l'auto-commit lui-même (`n.offset` ouvrait et
    /// validait une transaction pour rien), et un identifiant utilisateur
    /// n'a aucune raison de peser sur quoi que ce soit. Les mots-clés d'une
    /// mutation sont toujours des lexèmes isolés ; l'heuristique reste une
    /// SUR-approximation (un mot-clé dans un littéral de chaîne compte
    /// encore), ce qui est le bon côté pour l'auto-commit.
    fn query_looks_like_mutation(query: &str) -> bool {
        const MOTS: [&str; 8] = [
            "INSERT", "CREATE", "DELETE", "MERGE", "SET", "REMOVE", "DROP", "ALTER",
        ];
        let upper = query.to_ascii_uppercase();
        upper
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|mot| MOTS.contains(&mot))
    }

    /// Computes the wall-clock deadline for query execution.
    #[must_use]
    fn query_deadline(&self) -> Option<Instant> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.query_timeout.map(|d| Instant::now() + d)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &self.query_timeout;
            None
        }
    }

    /// Records query metrics for any language.
    ///
    /// Called after query execution to update counters, latency histogram,
    /// and per-language tracking. `elapsed_ms` should be `None` on WASM
    /// where `Instant` is unavailable.
    #[cfg(feature = "metrics")]
    fn record_query_metrics(
        &self,
        language: &str,
        elapsed_ms: Option<f64>,
        result: &Result<crate::database::QueryResult>,
    ) {
        use crate::metrics::record_metric;

        record_metric!(self.metrics, query_count, inc);
        if let Some(ref reg) = self.metrics {
            reg.query_count_by_language.increment(language);
        }
        if let Some(ms) = elapsed_ms {
            record_metric!(self.metrics, query_latency, observe ms);
        }
        match result {
            Ok(r) => {
                let returned = r.rows.len() as u64;
                record_metric!(self.metrics, rows_returned, add returned);
                if let Some(scanned) = r.rows_scanned {
                    record_metric!(self.metrics, rows_scanned, add scanned);
                }
            }
            Err(e) => {
                record_metric!(self.metrics, query_errors, inc);
                // Detect timeout errors
                let msg = e.to_string();
                if msg.contains("exceeded timeout") {
                    record_metric!(self.metrics, query_timeouts, inc);
                }
            }
        }
    }

    /// Evaluates a simple integer literal from a session parameter expression.
    fn eval_integer_literal(expr: &obrain_adapters::query::gql::ast::Expression) -> Option<i64> {
        use obrain_adapters::query::gql::ast::{Expression, Literal};
        match expr {
            Expression::Literal(Literal::Integer(n)) => Some(*n),
            _ => None,
        }
    }

    /// Returns the current transaction context for MVCC visibility.
    ///
    /// Returns `(viewing_epoch, transaction_id)` where:
    /// - `viewing_epoch` is the epoch at which to check version visibility
    /// - `transaction_id` is the current transaction ID (if in a transaction)
    #[must_use]
    fn get_transaction_context(&self) -> (EpochId, Option<TransactionId>) {
        // Time-travel override takes precedence (read-only, no tx context)
        if let Some(epoch) = *self.viewing_epoch_override.lock() {
            return (epoch, None);
        }

        if let Some(transaction_id) = *self.current_transaction.lock() {
            // In a transaction: use the transaction's start epoch
            let epoch = self
                .transaction_manager
                .start_epoch(transaction_id)
                .unwrap_or_else(|| self.transaction_manager.current_epoch());
            (epoch, Some(transaction_id))
        } else {
            // No transaction: use current epoch
            (self.transaction_manager.current_epoch(), None)
        }
    }

    /// Creates a planner with transaction context and constraint validator.
    ///
    /// The `store` parameter is the graph store to plan against (use
    /// `self.active_store()` for graph-aware execution).
    fn create_planner_for_store(
        &self,
        store: Arc<dyn GraphStoreMut>,
        viewing_epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> crate::query::Planner {
        self.create_planner_for_store_with_read_only(store, viewing_epoch, transaction_id, false)
    }

    /// Construit le SessionContext porte par TOUT planner de cette
    /// session : contexte schema/graphe, introspection paresseuse, et
    /// registre multi-store (aretes inter-bases V2). Utilise par
    /// `create_planner_for_store*` ET par les chemins QueryProcessor
    /// (execute_with_params, gremlin, graphql, sql) et les procedures
    /// stockees — un chemin qui batirait son planner sans lui perdrait le
    /// registre EN SILENCE (trou P1 de la verification adversariale).
    pub(crate) fn build_session_context(
        &self,
        store: Arc<dyn GraphStoreMut>,
    ) -> obrain_core::execution::operators::SessionContext {
        use obrain_core::execution::operators::{LazyValue, SessionContext};
        // Capture store reference for lazy introspection (only computed if
        // info()/schema() called).
        let info_store = Arc::clone(&store);
        let schema_store = store;
        SessionContext {
            base_registry: self.base_registry.lock().clone(),
            access_context: self.access_context.lock().clone(),
            in_explicit_transaction: self.in_explicit_transaction(),
            current_schema: self.current_schema(),
            current_graph: self.current_graph(),
            db_info: LazyValue::new(move || Self::build_info_value(&*info_store)),
            schema_info: LazyValue::new(move || Self::build_schema_value(&*schema_store)),
        }
    }

    fn create_planner_for_store_with_read_only(
        &self,
        store: Arc<dyn GraphStoreMut>,
        viewing_epoch: EpochId,
        transaction_id: Option<TransactionId>,
        read_only: bool,
    ) -> crate::query::Planner {
        use crate::query::Planner;

        let session_context = self.build_session_context(Arc::clone(&store));

        let mut planner = Planner::with_context(
            Arc::clone(&store),
            Arc::clone(&self.transaction_manager),
            transaction_id,
            viewing_epoch,
        )
        .with_factorized_execution(self.factorized_execution)
        .with_catalog(Arc::clone(&self.catalog))
        .with_session_context(session_context)
        .with_read_only(read_only);

        // Attach the constraint validator for schema enforcement
        let validator =
            CatalogConstraintValidator::new(Arc::clone(&self.catalog)).with_store(store);
        planner = planner.with_validator(Arc::new(validator));

        // Wire the cognitive engine (3 piliers), if this session has one, so
        // `CALL obrain.cognitive.*` procedures resolve instead of falling
        // through to "Unknown procedure" (see `set_cognitive_engine`).
        #[cfg(feature = "cognitive")]
        if let Some(ref engine) = self.cognitive_engine {
            planner = planner.with_cognitive_engine(Arc::clone(engine));
        }
        // Wire the typed substrate handle (same pattern) so substrate-native
        // procedures (`CALL obrain.window_2d`) resolve at plan time.
        if let Some(ref sub) = self.substrate_store {
            planner = planner.with_substrate_store(Arc::clone(sub));
        }
        #[cfg(feature = "cdc")]
        {
            planner = planner.with_cdc_log(Arc::clone(self.cdc_log()));
        }

        planner
    }

    /// Builds a `Value::Map` for the `info()` introspection function.
    fn build_info_value(store: &dyn GraphStoreMut) -> Value {
        use obrain_common::types::PropertyKey;
        use std::collections::BTreeMap;

        let mut map = BTreeMap::new();
        map.insert(PropertyKey::from("mode"), Value::String("lpg".into()));
        map.insert(
            PropertyKey::from("node_count"),
            Value::Int64(store.node_count() as i64),
        );
        map.insert(
            PropertyKey::from("edge_count"),
            Value::Int64(store.edge_count() as i64),
        );
        map.insert(
            PropertyKey::from("version"),
            Value::String(env!("CARGO_PKG_VERSION").into()),
        );
        Value::Map(map.into())
    }

    /// Builds a `Value::Map` for the `schema()` introspection function.
    fn build_schema_value(store: &dyn GraphStoreMut) -> Value {
        use obrain_common::types::PropertyKey;
        use std::collections::BTreeMap;

        let labels: Vec<Value> = store
            .all_labels()
            .into_iter()
            .map(|l| Value::String(l.into()))
            .collect();
        let edge_types: Vec<Value> = store
            .all_edge_types()
            .into_iter()
            .map(|t| Value::String(t.into()))
            .collect();
        let property_keys: Vec<Value> = store
            .all_property_keys()
            .into_iter()
            .map(|k| Value::String(k.into()))
            .collect();

        let mut map = BTreeMap::new();
        map.insert(PropertyKey::from("labels"), Value::List(labels.into()));
        map.insert(
            PropertyKey::from("edge_types"),
            Value::List(edge_types.into()),
        );
        map.insert(
            PropertyKey::from("property_keys"),
            Value::List(property_keys.into()),
        );
        Value::Map(map.into())
    }

    /// Creates a node directly (bypassing query execution).
    ///
    /// This is a low-level API for testing and direct manipulation.
    /// If a transaction is active, the node will be versioned with the transaction ID.
    pub fn create_node(&self, labels: &[&str]) -> NodeId {
        let (epoch, transaction_id) = self.get_transaction_context();
        self.active_store().create_node_versioned(
            labels,
            epoch,
            transaction_id.unwrap_or(TransactionId::SYSTEM),
        )
    }

    /// Creates a node with properties.
    ///
    /// If a transaction is active, the node will be versioned with the transaction ID.
    ///
    /// T17 Step 24: decomposed into `create_node_versioned` +
    /// `set_node_property` so this call dispatches entirely through
    /// `GraphStoreMut` (no LpgStore-inherent
    /// `create_node_with_props_versioned` required).
    pub fn create_node_with_props<'a>(
        &self,
        labels: &[&str],
        properties: impl IntoIterator<Item = (&'a str, Value)>,
    ) -> NodeId {
        let (epoch, transaction_id) = self.get_transaction_context();
        let tx = transaction_id.unwrap_or(TransactionId::SYSTEM);
        let store = self.active_store();
        let id = store.create_node_versioned(labels, epoch, tx);
        for (key, value) in properties {
            store.set_node_property(id, key, value);
        }
        id
    }

    /// Creates an edge between two nodes.
    ///
    /// This is a low-level API for testing and direct manipulation.
    /// If a transaction is active, the edge will be versioned with the transaction ID.
    pub fn create_edge(
        &self,
        src: NodeId,
        dst: NodeId,
        edge_type: &str,
    ) -> obrain_common::types::EdgeId {
        let (epoch, transaction_id) = self.get_transaction_context();
        self.active_store().create_edge_versioned(
            src,
            dst,
            edge_type,
            epoch,
            transaction_id.unwrap_or(TransactionId::SYSTEM),
        )
    }

    // =========================================================================
    // Direct Lookup APIs (bypass query planning for O(1) point reads)
    // =========================================================================

    /// Gets a node by ID directly, bypassing query planning.
    ///
    /// This is the fastest way to retrieve a single node when you know its ID.
    /// Skips parsing, binding, optimization, and physical planning entirely.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(1) average case
    /// - No lock contention (uses DashMap internally)
    /// - ~20-30x faster than equivalent MATCH query
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use obrain_engine::ObrainDB;
    /// # let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    /// let node_id = session.create_node(&["Person"]);
    ///
    /// // Direct lookup - O(1), no query planning
    /// let node = session.get_node(node_id);
    /// assert!(node.is_some());
    /// ```
    #[must_use]
    pub fn get_node(&self, id: NodeId) -> Option<Node> {
        let (epoch, transaction_id) = self.get_transaction_context();
        self.active_store().get_node_versioned(
            id,
            epoch,
            transaction_id.unwrap_or(TransactionId::SYSTEM),
        )
    }

    /// Gets a single property from a node by ID, bypassing query planning.
    ///
    /// More efficient than `get_node()` when you only need one property,
    /// as it avoids loading the full node with all properties.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(1) average case
    /// - No query planning overhead
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use obrain_engine::ObrainDB;
    /// # use obrain_common::types::Value;
    /// # let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    /// let id = session.create_node_with_props(&["Person"], [("name", "Alix".into())]);
    ///
    /// // Direct property access - O(1)
    /// let name = session.get_node_property(id, "name");
    /// assert_eq!(name, Some(Value::String("Alix".into())));
    /// ```
    #[must_use]
    pub fn get_node_property(&self, id: NodeId, key: &str) -> Option<Value> {
        self.get_node(id)
            .and_then(|node| node.get_property(key).cloned())
    }

    /// Gets an edge by ID directly, bypassing query planning.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(1) average case
    /// - No lock contention
    #[must_use]
    pub fn get_edge(&self, id: EdgeId) -> Option<Edge> {
        let (epoch, transaction_id) = self.get_transaction_context();
        self.active_store().get_edge_versioned(
            id,
            epoch,
            transaction_id.unwrap_or(TransactionId::SYSTEM),
        )
    }

    /// Gets outgoing neighbors of a node directly, bypassing query planning.
    ///
    /// Returns (neighbor_id, edge_id) pairs for all outgoing edges.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(degree) where degree is the number of outgoing edges
    /// - Uses adjacency index for direct access
    /// - ~10-20x faster than equivalent MATCH query
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use obrain_engine::ObrainDB;
    /// # let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    /// let alix = session.create_node(&["Person"]);
    /// let gus = session.create_node(&["Person"]);
    /// session.create_edge(alix, gus, "KNOWS");
    ///
    /// // Direct neighbor lookup - O(degree)
    /// let neighbors = session.get_neighbors_outgoing(alix);
    /// assert_eq!(neighbors.len(), 1);
    /// assert_eq!(neighbors[0].0, gus);
    /// ```
    #[must_use]
    pub fn get_neighbors_outgoing(&self, node: NodeId) -> Vec<(NodeId, EdgeId)> {
        self.active_store().edges_from(node, Direction::Outgoing)
    }

    /// Gets incoming neighbors of a node directly, bypassing query planning.
    ///
    /// Returns (neighbor_id, edge_id) pairs for all incoming edges.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(degree) where degree is the number of incoming edges
    /// - Uses backward adjacency index for direct access
    #[must_use]
    pub fn get_neighbors_incoming(&self, node: NodeId) -> Vec<(NodeId, EdgeId)> {
        self.active_store().edges_from(node, Direction::Incoming)
    }

    /// Gets outgoing neighbors filtered by edge type, bypassing query planning.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use obrain_engine::ObrainDB;
    /// # let db = ObrainDB::new_in_memory();
    /// # let session = db.session();
    /// # let alix = session.create_node(&["Person"]);
    /// let neighbors = session.get_neighbors_outgoing_by_type(alix, "KNOWS");
    /// ```
    #[must_use]
    pub fn get_neighbors_outgoing_by_type(
        &self,
        node: NodeId,
        edge_type: &str,
    ) -> Vec<(NodeId, EdgeId)> {
        self.active_store()
            .edges_from(node, Direction::Outgoing)
            .into_iter()
            .filter(|(_, edge_id)| {
                self.get_edge(*edge_id)
                    .is_some_and(|e| e.edge_type.as_str() == edge_type)
            })
            .collect()
    }

    /// Checks if a node exists, bypassing query planning.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(1)
    /// - Fastest existence check available
    #[must_use]
    pub fn node_exists(&self, id: NodeId) -> bool {
        self.get_node(id).is_some()
    }

    /// Checks if an edge exists, bypassing query planning.
    #[must_use]
    pub fn edge_exists(&self, id: EdgeId) -> bool {
        self.get_edge(id).is_some()
    }

    /// Gets the degree (number of edges) of a node.
    ///
    /// Returns (outgoing_degree, incoming_degree).
    #[must_use]
    pub fn get_degree(&self, node: NodeId) -> (usize, usize) {
        let active = self.active_store();
        let out = active.out_degree(node);
        let in_degree = active.in_degree(node);
        (out, in_degree)
    }

    /// Batch lookup of multiple nodes by ID.
    ///
    /// More efficient than calling `get_node()` in a loop because it
    /// amortizes overhead.
    ///
    /// # Performance
    ///
    /// - Time complexity: O(n) where n is the number of IDs
    /// - Better cache utilization than individual lookups
    #[must_use]
    pub fn get_nodes_batch(&self, ids: &[NodeId]) -> Vec<Option<Node>> {
        let (epoch, transaction_id) = self.get_transaction_context();
        let tx = transaction_id.unwrap_or(TransactionId::SYSTEM);
        let active = self.active_store();
        ids.iter()
            .map(|&id| active.get_node_versioned(id, epoch, tx))
            .collect()
    }

    // ── Change Data Capture ─────────────────────────────────────────────

    /// Returns the full change history for an entity (node or edge).
    #[cfg(feature = "cdc")]
    pub fn history(
        &self,
        entity_id: impl Into<crate::cdc::EntityId>,
    ) -> Result<Vec<crate::cdc::ChangeEvent>> {
        Ok(self.cdc_log.history(entity_id.into()))
    }

    /// Returns change events for an entity since the given epoch.
    #[cfg(feature = "cdc")]
    pub fn history_since(
        &self,
        entity_id: impl Into<crate::cdc::EntityId>,
        since_epoch: EpochId,
    ) -> Result<Vec<crate::cdc::ChangeEvent>> {
        Ok(self.cdc_log.history_since(entity_id.into(), since_epoch))
    }

    /// Returns all change events across all entities in an epoch range.
    #[cfg(feature = "cdc")]
    pub fn changes_between(
        &self,
        start_epoch: EpochId,
        end_epoch: EpochId,
    ) -> Result<Vec<crate::cdc::ChangeEvent>> {
        Ok(self.cdc_log.changes_between(start_epoch, end_epoch))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Auto-rollback any active transaction to prevent leaked MVCC state,
        // dangling write locks, and uncommitted versions lingering in the store.
        if self.in_transaction() {
            let _ = self.rollback_inner();
        }

        #[cfg(feature = "metrics")]
        if let Some(ref reg) = self.metrics {
            reg.session_active
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_default_literal;
    use crate::database::ObrainDB;
    use obrain_common::types::Value;

    // -----------------------------------------------------------------------
    // parse_default_literal
    // -----------------------------------------------------------------------

    #[test]
    fn parse_default_literal_null() {
        assert_eq!(parse_default_literal("null"), Value::Null);
        assert_eq!(parse_default_literal("NULL"), Value::Null);
        assert_eq!(parse_default_literal("Null"), Value::Null);
    }

    #[test]
    fn parse_default_literal_bool() {
        assert_eq!(parse_default_literal("true"), Value::Bool(true));
        assert_eq!(parse_default_literal("TRUE"), Value::Bool(true));
        assert_eq!(parse_default_literal("false"), Value::Bool(false));
        assert_eq!(parse_default_literal("FALSE"), Value::Bool(false));
    }

    #[test]
    fn parse_default_literal_string_single_quoted() {
        assert_eq!(
            parse_default_literal("'hello'"),
            Value::String("hello".into())
        );
    }

    #[test]
    fn parse_default_literal_string_double_quoted() {
        assert_eq!(
            parse_default_literal("\"world\""),
            Value::String("world".into())
        );
    }

    #[test]
    fn parse_default_literal_integer() {
        assert_eq!(parse_default_literal("42"), Value::Int64(42));
        assert_eq!(parse_default_literal("-7"), Value::Int64(-7));
        assert_eq!(parse_default_literal("0"), Value::Int64(0));
    }

    #[test]
    fn parse_default_literal_float() {
        assert_eq!(parse_default_literal("9.81"), Value::Float64(9.81_f64));
        assert_eq!(parse_default_literal("-0.5"), Value::Float64(-0.5));
    }

    #[test]
    fn parse_default_literal_fallback_string() {
        // Not a recognized literal, not quoted, not a number
        assert_eq!(
            parse_default_literal("some_identifier"),
            Value::String("some_identifier".into())
        );
    }

    #[test]
    fn test_session_create_node() {
        let db = ObrainDB::new_in_memory();
        let session = db.session();

        let id = session.create_node(&["Person"]);
        assert!(id.is_valid());
        assert_eq!(db.node_count(), 1);
    }

    #[test]
    fn test_session_transaction() {
        let db = ObrainDB::new_in_memory();
        let mut session = db.session();

        assert!(!session.in_transaction());

        session.begin_transaction().unwrap();
        assert!(session.in_transaction());

        session.commit().unwrap();
        assert!(!session.in_transaction());
    }

    #[test]
    fn test_session_transaction_context() {
        let db = ObrainDB::new_in_memory();
        let mut session = db.session();

        // Without transaction - context should have current epoch and no transaction_id
        let (_epoch1, transaction_id1) = session.get_transaction_context();
        assert!(transaction_id1.is_none());

        // Start a transaction
        session.begin_transaction().unwrap();
        let (epoch2, transaction_id2) = session.get_transaction_context();
        assert!(transaction_id2.is_some());
        // Transaction should have a valid epoch
        let _ = epoch2; // Use the variable

        // Commit and verify
        session.commit().unwrap();
        let (epoch3, tx_id3) = session.get_transaction_context();
        assert!(tx_id3.is_none());
        // Epoch should have advanced after commit
        assert!(epoch3.as_u64() >= epoch2.as_u64());
    }

    #[test]
    fn test_session_rollback() {
        let db = ObrainDB::new_in_memory();
        let mut session = db.session();

        session.begin_transaction().unwrap();
        session.rollback().unwrap();
        assert!(!session.in_transaction());
    }

    mod gql_tests {
        use super::*;

        #[test]
        fn test_gql_query_execution() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create some test data
            session.create_node(&["Person"]);
            session.create_node(&["Person"]);
            session.create_node(&["Animal"]);

            // Execute a GQL query
            let result = session.execute("MATCH (n:Person) RETURN n").unwrap();

            // Should return 2 Person nodes
            assert_eq!(result.row_count(), 2);
            assert_eq!(result.column_count(), 1);
            assert_eq!(result.columns[0], "n");
        }

        #[test]
        fn test_gql_empty_result() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // No data in database
            let result = session.execute("MATCH (n:Person) RETURN n").unwrap();

            assert_eq!(result.row_count(), 0);
        }

        #[test]
        fn test_gql_parse_error() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Invalid GQL syntax
            let result = session.execute("MATCH (n RETURN n");

            assert!(result.is_err());
        }

        #[test]
        fn test_gql_relationship_traversal() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create a graph: Alix -> Gus, Alix -> Vincent
            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let vincent = session.create_node(&["Person"]);

            session.create_edge(alix, gus, "KNOWS");
            session.create_edge(alix, vincent, "KNOWS");

            // Execute a path query: MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a, b
            let result = session
                .execute("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a, b")
                .unwrap();

            // Should return 2 rows (Alix->Gus, Alix->Vincent)
            assert_eq!(result.row_count(), 2);
            assert_eq!(result.column_count(), 2);
            assert_eq!(result.columns[0], "a");
            assert_eq!(result.columns[1], "b");
        }

        #[test]
        fn test_gql_relationship_with_type_filter() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create a graph: Alix -KNOWS-> Gus, Alix -WORKS_WITH-> Vincent
            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let vincent = session.create_node(&["Person"]);

            session.create_edge(alix, gus, "KNOWS");
            session.create_edge(alix, vincent, "WORKS_WITH");

            // Query only KNOWS relationships
            let result = session
                .execute("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a, b")
                .unwrap();

            // Should return only 1 row (Alix->Gus)
            assert_eq!(result.row_count(), 1);
        }

        #[test]
        fn test_gql_semantic_error_undefined_variable() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Reference undefined variable 'x' in RETURN
            let result = session.execute("MATCH (n:Person) RETURN x");

            // Should fail with semantic error
            assert!(result.is_err());
            let Err(err) = result else {
                panic!("Expected error")
            };
            assert!(
                err.to_string().contains("Undefined variable"),
                "Expected undefined variable error, got: {}",
                err
            );
        }

        #[test]
        fn test_gql_where_clause_property_filter() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create people with ages
            session.create_node_with_props(&["Person"], [("age", Value::Int64(25))]);
            session.create_node_with_props(&["Person"], [("age", Value::Int64(35))]);
            session.create_node_with_props(&["Person"], [("age", Value::Int64(45))]);

            // Query with WHERE clause: age > 30
            let result = session
                .execute("MATCH (n:Person) WHERE n.age > 30 RETURN n")
                .unwrap();

            // Should return 2 people (ages 35 and 45)
            assert_eq!(result.row_count(), 2);
        }

        #[test]
        fn test_gql_where_clause_equality() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create people with names
            session.create_node_with_props(&["Person"], [("name", Value::String("Alix".into()))]);
            session.create_node_with_props(&["Person"], [("name", Value::String("Gus".into()))]);
            session.create_node_with_props(&["Person"], [("name", Value::String("Alix".into()))]);

            // Query with WHERE clause: name = "Alix"
            let result = session
                .execute("MATCH (n:Person) WHERE n.name = \"Alix\" RETURN n")
                .unwrap();

            // Should return 2 people named Alix
            assert_eq!(result.row_count(), 2);
        }

        #[test]
        fn test_gql_return_property_access() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create people with names and ages
            session.create_node_with_props(
                &["Person"],
                [
                    ("name", Value::String("Alix".into())),
                    ("age", Value::Int64(30)),
                ],
            );
            session.create_node_with_props(
                &["Person"],
                [
                    ("name", Value::String("Gus".into())),
                    ("age", Value::Int64(25)),
                ],
            );

            // Query returning properties
            let result = session
                .execute("MATCH (n:Person) RETURN n.name, n.age")
                .unwrap();

            // Should return 2 rows with name and age columns
            assert_eq!(result.row_count(), 2);
            assert_eq!(result.column_count(), 2);
            assert_eq!(result.columns[0], "n.name");
            assert_eq!(result.columns[1], "n.age");

            // Check that we get actual values
            let names: Vec<&Value> = result.rows.iter().map(|r| &r[0]).collect();
            assert!(names.contains(&&Value::String("Alix".into())));
            assert!(names.contains(&&Value::String("Gus".into())));
        }

        #[test]
        fn test_gql_return_mixed_expressions() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create a person
            session.create_node_with_props(&["Person"], [("name", Value::String("Alix".into()))]);

            // Query returning both node and property
            let result = session
                .execute("MATCH (n:Person) RETURN n, n.name")
                .unwrap();

            assert_eq!(result.row_count(), 1);
            assert_eq!(result.column_count(), 2);
            assert_eq!(result.columns[0], "n");
            assert_eq!(result.columns[1], "n.name");

            // Second column should be the name
            assert_eq!(result.rows[0][1], Value::String("Alix".into()));
        }

        // ==================== NodeByIdSeek Tests ====================

        #[test]
        fn test_gql_id_seek_point_lookup() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let n0 = session.create_node(&["Person"]);
            let n1 = session.create_node(&["Person"]);
            let _n2 = session.create_node(&["Animal"]);

            // Point lookup: WHERE id(n) = <id>
            let result = session
                .execute(&format!("MATCH (n) WHERE id(n) = {} RETURN n", n1.as_u64()))
                .unwrap();

            assert_eq!(result.row_count(), 1, "Should return exactly 1 node");
            assert_eq!(result.columns[0], "n");

            // Verify it's the right node by looking up n0
            let result0 = session
                .execute(&format!("MATCH (n) WHERE id(n) = {} RETURN n", n0.as_u64()))
                .unwrap();
            assert_eq!(result0.row_count(), 1);
        }

        #[test]
        fn test_gql_id_seek_not_found() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session.create_node(&["Person"]);

            // Non-existent node
            let result = session
                .execute("MATCH (n) WHERE id(n) = 999999 RETURN n")
                .unwrap();

            assert_eq!(
                result.row_count(),
                0,
                "Should return empty for non-existent id"
            );
        }

        #[test]
        fn test_gql_id_seek_with_additional_predicate() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let n0 = session.create_node_with_props(&["Person"], [("age", Value::Int64(30))]);
            let n1 = session.create_node_with_props(&["Person"], [("age", Value::Int64(20))]);

            // id(n) = X AND n.age > 25 — should match n0 only
            let result = session
                .execute(&format!(
                    "MATCH (n) WHERE id(n) = {} AND n.age > 25 RETURN n",
                    n0.as_u64()
                ))
                .unwrap();
            assert_eq!(result.row_count(), 1);

            // id(n) = X AND n.age > 25 — should NOT match n1 (age=20)
            let result = session
                .execute(&format!(
                    "MATCH (n) WHERE id(n) = {} AND n.age > 25 RETURN n",
                    n1.as_u64()
                ))
                .unwrap();
            assert_eq!(result.row_count(), 0);
        }

        #[test]
        fn test_gql_id_seek_in_and_chain() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let n0 = session
                .create_node_with_props(&["Person"], [("name", Value::String("Alix".into()))]);

            // id buried in AND chain: n.name = 'Alix' AND id(n) = X
            let result = session
                .execute(&format!(
                    "MATCH (n) WHERE n.name = 'Alix' AND id(n) = {} RETURN n",
                    n0.as_u64()
                ))
                .unwrap();
            assert_eq!(result.row_count(), 1);
        }

        #[test]
        fn test_gql_id_seek_return_id() {
            use obrain_common::types::Value;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let n0 = session.create_node(&["Test"]);

            // RETURN id(n) — verify we get the right id back
            let result = session
                .execute(&format!(
                    "MATCH (n) WHERE id(n) = {} RETURN id(n)",
                    n0.as_u64()
                ))
                .unwrap();
            assert_eq!(result.row_count(), 1);
            // id() returns the node id as i64
            assert_eq!(result.rows[0][0], Value::Int64(n0.as_u64() as i64));
        }

        #[test]
        #[allow(clippy::many_single_char_names)] // nœuds du scénario nommés a, b, c, d, e
        fn test_gql_id_in_list_multi_seed_seek() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let a = session.create_node(&["N"]);
            let b = session.create_node(&["N"]);
            let c = session.create_node(&["N"]);
            let d = session.create_node(&["N"]);
            let e = session.create_node(&["N"]);
            session.create_edge(a, b, "CALLS");
            session.create_edge(a, c, "CALLS");
            session.create_edge(d, e, "CALLS");

            // `id(x) IN [...]` doit utiliser le seek multi-seed (adjacence) :
            // mêmes résultats que l'union des seeks égalité, jamais un scan
            // complet de la table des arêtes.
            let one = session
                .execute(&format!(
                    "MATCH (x)-[r]->(y) WHERE id(x) IN [{}] RETURN count(r)",
                    a.as_u64()
                ))
                .unwrap();
            assert_eq!(one.rows[0][0], Value::Int64(2), "a→b, a→c");

            let two = session
                .execute(&format!(
                    "MATCH (x)-[r]->(y) WHERE id(x) IN [{}, {}] RETURN count(r)",
                    a.as_u64(),
                    d.as_u64()
                ))
                .unwrap();
            assert_eq!(two.rows[0][0], Value::Int64(3), "a→b, a→c, d→e");

            // Ids inexistants dans la liste : filtrés (visibilité MVCC), les
            // vrais voisins restent.
            let ghost = session
                .execute(&format!(
                    "MATCH (x)-[r]->(y) WHERE id(x) IN [{}, 999999] RETURN count(r)",
                    a.as_u64()
                ))
                .unwrap();
            assert_eq!(ghost.rows[0][0], Value::Int64(2));

            // Liste vide : zéro ligne, pas un scan complet.
            let empty = session
                .execute("MATCH (x)-[r]->(y) WHERE id(x) IN [] RETURN count(r)")
                .unwrap();
            assert_eq!(empty.rows[0][0], Value::Int64(0));

            // L'expansion inbound (id(y) IN [...]) suit le même chemin.
            let inbound = session
                .execute(&format!(
                    "MATCH (x)-[r]->(y) WHERE id(y) IN [{}] RETURN count(r)",
                    b.as_u64()
                ))
                .unwrap();
            assert_eq!(inbound.rows[0][0], Value::Int64(1), "a→b");
        }

        // ==================== Count Shortcut Tests ====================

        #[test]
        fn test_gql_count_all_nodes_shortcut() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session.create_node(&["Person"]);
            session.create_node(&["Person"]);
            session.create_node(&["Animal"]);

            // MATCH (n) RETURN count(n) — should use node_count() shortcut
            let result = session.execute("MATCH (n) RETURN count(n)").unwrap();

            assert_eq!(result.row_count(), 1);
            assert_eq!(result.rows[0][0], obrain_common::types::Value::Int64(3));
        }

        #[test]
        fn test_gql_count_by_label_shortcut() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session.create_node(&["Person"]);
            session.create_node(&["Person"]);
            session.create_node(&["Animal"]);

            // MATCH (n:Person) RETURN count(n) — should use nodes_by_label shortcut
            let result = session.execute("MATCH (n:Person) RETURN count(n)").unwrap();

            assert_eq!(result.row_count(), 1);
            assert_eq!(result.rows[0][0], obrain_common::types::Value::Int64(2));
        }

        #[test]
        fn test_gql_count_empty_graph() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Empty graph → count = 0
            let result = session.execute("MATCH (n) RETURN count(n)").unwrap();

            assert_eq!(result.row_count(), 1);
            assert_eq!(result.rows[0][0], obrain_common::types::Value::Int64(0));
        }

        #[test]
        fn test_gql_count_nonexistent_label() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session.create_node(&["Person"]);

            // MATCH (n:Ghost) RETURN count(n) — no Ghost nodes
            let result = session.execute("MATCH (n:Ghost) RETURN count(n)").unwrap();

            assert_eq!(result.row_count(), 1);
            assert_eq!(result.rows[0][0], obrain_common::types::Value::Int64(0));
        }

        #[test]
        fn test_gql_id_seek_with_label_scan() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let _n0 = session.create_node(&["Person"]);
            let n1 = session.create_node(&["Animal"]);

            // MATCH (n:Animal) WHERE id(n) = X — label scan + id filter
            // This should NOT use NodeByIdSeek (label scan has input),
            // but should still return correct results
            let result = session
                .execute(&format!(
                    "MATCH (n:Animal) WHERE id(n) = {} RETURN n",
                    n1.as_u64()
                ))
                .unwrap();
            assert_eq!(result.row_count(), 1);
        }
    }

    #[cfg(feature = "cypher")]
    mod cypher_tests {
        use super::*;

        #[test]
        fn test_cypher_query_execution() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Create some test data
            session.create_node(&["Person"]);
            session.create_node(&["Person"]);
            session.create_node(&["Animal"]);

            // Execute a Cypher query
            let result = session.execute_cypher("MATCH (n:Person) RETURN n").unwrap();

            // Should return 2 Person nodes
            assert_eq!(result.row_count(), 2);
            assert_eq!(result.column_count(), 1);
            assert_eq!(result.columns[0], "n");
        }

        #[test]
        fn test_cypher_empty_result() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // No data in database
            let result = session.execute_cypher("MATCH (n:Person) RETURN n").unwrap();

            assert_eq!(result.row_count(), 0);
        }

        #[test]
        fn test_cypher_parse_error() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Invalid Cypher syntax
            let result = session.execute_cypher("MATCH (n RETURN n");

            assert!(result.is_err());
        }
    }

    // ==================== Direct Lookup API Tests ====================

    mod direct_lookup_tests {
        use super::*;
        use obrain_common::types::Value;

        #[test]
        fn test_get_node() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let id = session.create_node(&["Person"]);
            let node = session.get_node(id);

            assert!(node.is_some());
            let node = node.unwrap();
            assert_eq!(node.id, id);
        }

        #[test]
        fn test_get_node_not_found() {
            use obrain_common::types::NodeId;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Try to get a non-existent node
            let node = session.get_node(NodeId::new(9999));
            assert!(node.is_none());
        }

        #[test]
        fn test_get_node_property() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let id = session
                .create_node_with_props(&["Person"], [("name", Value::String("Alix".into()))]);

            let name = session.get_node_property(id, "name");
            assert_eq!(name, Some(Value::String("Alix".into())));

            // Non-existent property
            let missing = session.get_node_property(id, "missing");
            assert!(missing.is_none());
        }

        #[test]
        fn test_get_edge() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let edge_id = session.create_edge(alix, gus, "KNOWS");

            let edge = session.get_edge(edge_id);
            assert!(edge.is_some());
            let edge = edge.unwrap();
            assert_eq!(edge.id, edge_id);
            assert_eq!(edge.src, alix);
            assert_eq!(edge.dst, gus);
        }

        #[test]
        fn test_get_edge_not_found() {
            use obrain_common::types::EdgeId;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let edge = session.get_edge(EdgeId::new(9999));
            assert!(edge.is_none());
        }

        #[test]
        fn test_get_neighbors_outgoing() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let harm = session.create_node(&["Person"]);

            session.create_edge(alix, gus, "KNOWS");
            session.create_edge(alix, harm, "KNOWS");

            let neighbors = session.get_neighbors_outgoing(alix);
            assert_eq!(neighbors.len(), 2);

            let neighbor_ids: Vec<_> = neighbors.iter().map(|(node_id, _)| *node_id).collect();
            assert!(neighbor_ids.contains(&gus));
            assert!(neighbor_ids.contains(&harm));
        }

        #[test]
        fn test_get_neighbors_incoming() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let harm = session.create_node(&["Person"]);

            session.create_edge(gus, alix, "KNOWS");
            session.create_edge(harm, alix, "KNOWS");

            let neighbors = session.get_neighbors_incoming(alix);
            assert_eq!(neighbors.len(), 2);

            let neighbor_ids: Vec<_> = neighbors.iter().map(|(node_id, _)| *node_id).collect();
            assert!(neighbor_ids.contains(&gus));
            assert!(neighbor_ids.contains(&harm));
        }

        #[test]
        fn test_get_neighbors_outgoing_by_type() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let company = session.create_node(&["Company"]);

            session.create_edge(alix, gus, "KNOWS");
            session.create_edge(alix, company, "WORKS_AT");

            let knows_neighbors = session.get_neighbors_outgoing_by_type(alix, "KNOWS");
            assert_eq!(knows_neighbors.len(), 1);
            assert_eq!(knows_neighbors[0].0, gus);

            let works_neighbors = session.get_neighbors_outgoing_by_type(alix, "WORKS_AT");
            assert_eq!(works_neighbors.len(), 1);
            assert_eq!(works_neighbors[0].0, company);

            // No edges of this type
            let no_neighbors = session.get_neighbors_outgoing_by_type(alix, "LIKES");
            assert!(no_neighbors.is_empty(), "résultat attendu vide");
        }

        #[test]
        fn test_node_exists() {
            use obrain_common::types::NodeId;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let id = session.create_node(&["Person"]);

            assert!(session.node_exists(id));
            assert!(!session.node_exists(NodeId::new(9999)));
        }

        #[test]
        fn test_edge_exists() {
            use obrain_common::types::EdgeId;

            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let edge_id = session.create_edge(alix, gus, "KNOWS");

            assert!(session.edge_exists(edge_id));
            assert!(!session.edge_exists(EdgeId::new(9999)));
        }

        #[test]
        fn test_get_degree() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let harm = session.create_node(&["Person"]);

            // Alix knows Gus and Harm (2 outgoing)
            session.create_edge(alix, gus, "KNOWS");
            session.create_edge(alix, harm, "KNOWS");
            // Gus knows Alix (1 incoming for Alix)
            session.create_edge(gus, alix, "KNOWS");

            let (out_degree, in_degree) = session.get_degree(alix);
            assert_eq!(out_degree, 2);
            assert_eq!(in_degree, 1);

            // Node with no edges
            let lonely = session.create_node(&["Person"]);
            let (out, in_deg) = session.get_degree(lonely);
            assert_eq!(out, 0);
            assert_eq!(in_deg, 0);
        }

        #[test]
        fn test_get_nodes_batch() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);
            let harm = session.create_node(&["Person"]);

            let nodes = session.get_nodes_batch(&[alix, gus, harm]);
            assert_eq!(nodes.len(), 3);
            assert!(nodes[0].is_some());
            assert!(nodes[1].is_some());
            assert!(nodes[2].is_some());

            // With non-existent node
            use obrain_common::types::NodeId;
            let nodes_with_missing = session.get_nodes_batch(&[alix, NodeId::new(9999), harm]);
            assert_eq!(nodes_with_missing.len(), 3);
            assert!(nodes_with_missing[0].is_some());
            assert!(nodes_with_missing[1].is_none()); // Missing node
            assert!(nodes_with_missing[2].is_some());
        }

        #[test]
        fn test_auto_commit_setting() {
            let db = ObrainDB::new_in_memory();
            let mut session = db.session();

            // Default is auto-commit enabled
            assert!(session.auto_commit());

            session.set_auto_commit(false);
            assert!(!session.auto_commit());

            session.set_auto_commit(true);
            assert!(session.auto_commit());
        }

        #[test]
        fn test_transaction_double_begin_nests() {
            let db = ObrainDB::new_in_memory();
            let mut session = db.session();

            session.begin_transaction().unwrap();
            // Second begin_transaction creates a nested transaction (auto-savepoint)
            let result = session.begin_transaction();
            assert!(result.is_ok());
            // Commit the inner (releases savepoint)
            session.commit().unwrap();
            // Commit the outer
            session.commit().unwrap();
        }

        #[test]
        fn test_commit_without_transaction_error() {
            let db = ObrainDB::new_in_memory();
            let mut session = db.session();

            let result = session.commit();
            assert!(result.is_err());
        }

        #[test]
        fn test_rollback_without_transaction_error() {
            let db = ObrainDB::new_in_memory();
            let mut session = db.session();

            let result = session.rollback();
            assert!(result.is_err());
        }

        #[test]
        fn test_create_edge_in_transaction() {
            let db = ObrainDB::new_in_memory();
            let mut session = db.session();

            // Create nodes outside transaction
            let alix = session.create_node(&["Person"]);
            let gus = session.create_node(&["Person"]);

            // Create edge in transaction
            session.begin_transaction().unwrap();
            let edge_id = session.create_edge(alix, gus, "KNOWS");

            // Edge should be visible in the transaction
            assert!(session.edge_exists(edge_id));

            // Commit
            session.commit().unwrap();

            // Edge should still be visible
            assert!(session.edge_exists(edge_id));
        }

        #[test]
        fn test_neighbors_empty_node() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let lonely = session.create_node(&["Person"]);

            assert!(
                session.get_neighbors_outgoing(lonely).is_empty(),
                "résultat attendu vide"
            );
            assert!(
                session.get_neighbors_incoming(lonely).is_empty(),
                "résultat attendu vide"
            );
            assert!(
                session
                    .get_neighbors_outgoing_by_type(lonely, "KNOWS")
                    .is_empty(),
                "résultat attendu"
            );
        }
    }

    #[test]
    fn test_auto_gc_triggers_on_commit_interval() {
        use crate::config::Config;

        let config = Config::in_memory().with_gc_interval(2);
        let db = ObrainDB::with_config(config).unwrap();
        let mut session = db.session();

        // First commit: counter = 1, no GC (not a multiple of 2)
        session.begin_transaction().unwrap();
        session.create_node(&["A"]);
        session.commit().unwrap();

        // Second commit: counter = 2, GC should trigger (multiple of 2)
        session.begin_transaction().unwrap();
        session.create_node(&["B"]);
        session.commit().unwrap();

        // Verify the database is still functional after GC
        assert_eq!(db.node_count(), 2);
    }

    #[test]
    fn test_query_timeout_config_propagates_to_session() {
        use crate::config::Config;
        use std::time::Duration;

        let config = Config::in_memory().with_query_timeout(Duration::from_secs(5));
        let db = ObrainDB::with_config(config).unwrap();
        let session = db.session();

        // Verify the session has a query deadline (timeout was set)
        assert!(session.query_deadline().is_some());
    }

    #[test]
    fn test_no_query_timeout_returns_no_deadline() {
        let db = ObrainDB::new_in_memory();
        let session = db.session();

        // Default config has no timeout
        assert!(session.query_deadline().is_none());
    }

    #[test]
    fn test_graph_model_accessor() {
        use crate::config::GraphModel;

        let db = ObrainDB::new_in_memory();
        let session = db.session();

        assert_eq!(session.graph_model(), GraphModel::Lpg);
    }

    #[test]
    fn test_external_store_session() {
        use obrain_core::graph::GraphStoreMut;
        use obrain_substrate::SubstrateStore;
        use std::sync::Arc;

        let config = crate::config::Config::in_memory();
        let store = Arc::new(SubstrateStore::open_tempfile().unwrap()) as Arc<dyn GraphStoreMut>;
        let db = ObrainDB::with_store(store, config).unwrap();

        let mut session = db.session();

        // Use an explicit transaction so that INSERT and MATCH share the same
        // transaction context. With PENDING epochs, uncommitted versions are
        // only visible to the owning transaction.
        session.begin_transaction().unwrap();
        session.execute("INSERT (:Test {name: 'hello'})").unwrap();

        // Verify we can query through it within the same transaction
        let result = session.execute("MATCH (n:Test) RETURN n.name").unwrap();
        assert_eq!(result.row_count(), 1);

        session.commit().unwrap();
    }

    // ==================== Session Command Tests ====================

    mod session_command_tests {
        use super::*;
        use obrain_common::types::Value;

        #[test]
        fn test_use_graph_nonexistent_errors() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let result = session.execute("USE GRAPH doesnotexist");
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("does not exist"),
                "Expected 'does not exist' error, got: {err}"
            );
        }

        #[test]
        fn test_use_graph_default_always_valid() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // "default" is always valid, even without CREATE GRAPH
            session.execute("USE GRAPH default").unwrap();
            assert_eq!(session.current_graph(), Some("default".to_string()));
        }

        #[test]
        fn test_session_set_graph_nonexistent_errors() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let result = session.execute("SESSION SET GRAPH nosuchgraph");
            assert!(result.is_err());
        }

        #[test]
        fn test_session_set_time_zone() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            assert_eq!(session.time_zone(), None);

            session.execute("SESSION SET TIME ZONE 'UTC'").unwrap();
            assert_eq!(session.time_zone(), Some("UTC".to_string()));

            session
                .execute("SESSION SET TIME ZONE 'America/New_York'")
                .unwrap();
            assert_eq!(session.time_zone(), Some("America/New_York".to_string()));
        }

        #[test]
        fn test_session_set_parameter() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session
                .execute("SESSION SET PARAMETER $timeout = 30")
                .unwrap();

            // Parameter is stored (value is Null for now, since expression
            // evaluation is not yet wired up)
            assert!(session.get_parameter("timeout").is_some());
        }

        #[test]
        fn test_drop_graph_nonexistent_errors() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let result = session.execute("DROP GRAPH nosuchgraph");
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("does not exist"),
                "Expected 'does not exist' error, got: {err}"
            );
        }

        #[test]
        fn test_drop_graph_if_exists() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Should succeed silently with IF EXISTS
            session.execute("DROP GRAPH IF EXISTS nosuchgraph").unwrap();
        }

        #[test]
        fn test_start_transaction_via_gql() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session.execute("START TRANSACTION").unwrap();
            assert!(session.in_transaction());
            session.execute("INSERT (:Person {name: 'Alix'})").unwrap();
            session.execute("COMMIT").unwrap();
            assert!(!session.in_transaction());

            let result = session.execute("MATCH (n:Person) RETURN n.name").unwrap();
            assert_eq!(result.rows.len(), 1);
        }

        #[test]
        fn test_start_transaction_read_only_blocks_insert() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session.execute("START TRANSACTION READ ONLY").unwrap();
            let result = session.execute("INSERT (:Person {name: 'Alix'})");
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("read-only"),
                "Expected read-only error, got: {err}"
            );
            session.execute("ROLLBACK").unwrap();
        }

        #[test]
        fn test_start_transaction_read_only_allows_reads() {
            let db = ObrainDB::new_in_memory();
            let mut session = db.session();
            session.begin_transaction().unwrap();
            session.execute("INSERT (:Person {name: 'Alix'})").unwrap();
            session.commit().unwrap();

            session.execute("START TRANSACTION READ ONLY").unwrap();
            let result = session.execute("MATCH (n:Person) RETURN n.name").unwrap();
            assert_eq!(result.rows.len(), 1);
            session.execute("COMMIT").unwrap();
        }

        #[test]
        fn test_start_transaction_with_isolation_level() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session
                .execute("START TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .unwrap();
            assert!(session.in_transaction());
            session.execute("ROLLBACK").unwrap();
        }

        #[test]
        fn test_current_graph_default_is_none() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            assert_eq!(session.current_graph(), None);
        }

        #[test]
        fn test_time_zone_default_is_none() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            assert_eq!(session.time_zone(), None);
        }

        #[test]
        fn test_show_node_types() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session
                .execute("CREATE NODE TYPE Person (name STRING NOT NULL, age INTEGER)")
                .unwrap();

            let result = session.execute("SHOW NODE TYPES").unwrap();
            assert_eq!(
                result.columns,
                vec!["name", "properties", "constraints", "parents"]
            );
            assert_eq!(result.rows.len(), 1);
            // First column is the type name
            assert_eq!(result.rows[0][0], Value::from("Person"));
        }

        #[test]
        fn test_show_edge_types() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session
                .execute("CREATE EDGE TYPE KNOWS CONNECTING (Person) TO (Person) (since INTEGER)")
                .unwrap();

            let result = session.execute("SHOW EDGE TYPES").unwrap();
            assert_eq!(
                result.columns,
                vec!["name", "properties", "source_types", "target_types"]
            );
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0][0], Value::from("KNOWS"));
        }

        #[test]
        fn test_show_graph_types() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session
                .execute("CREATE NODE TYPE Person (name STRING)")
                .unwrap();
            session
                .execute(
                    "CREATE GRAPH TYPE social (\
                        NODE TYPE Person (name STRING)\
                    )",
                )
                .unwrap();

            let result = session.execute("SHOW GRAPH TYPES").unwrap();
            assert_eq!(
                result.columns,
                vec!["name", "open", "node_types", "edge_types"]
            );
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0][0], Value::from("social"));
        }

        #[test]
        fn test_show_graph_type_named() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            session
                .execute("CREATE NODE TYPE Person (name STRING)")
                .unwrap();
            session
                .execute(
                    "CREATE GRAPH TYPE social (\
                        NODE TYPE Person (name STRING)\
                    )",
                )
                .unwrap();

            let result = session.execute("SHOW GRAPH TYPE social").unwrap();
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0][0], Value::from("social"));
        }

        #[test]
        fn test_show_graph_type_not_found() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let result = session.execute("SHOW GRAPH TYPE nonexistent");
            assert!(result.is_err());
        }

        #[test]
        fn test_show_indexes_via_gql() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let result = session.execute("SHOW INDEXES").unwrap();
            assert_eq!(result.columns, vec!["name", "type", "label", "property"]);
        }

        #[test]
        fn test_show_constraints_via_gql() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            let result = session.execute("SHOW CONSTRAINTS").unwrap();
            assert_eq!(result.columns, vec!["name", "type", "label", "properties"]);
        }

        #[test]
        fn test_pattern_form_graph_type_roundtrip() {
            let db = ObrainDB::new_in_memory();
            let session = db.session();

            // Register the types first
            session
                .execute("CREATE NODE TYPE Person (name STRING NOT NULL)")
                .unwrap();
            session
                .execute("CREATE NODE TYPE City (name STRING)")
                .unwrap();
            session
                .execute("CREATE EDGE TYPE KNOWS (since INTEGER)")
                .unwrap();
            session.execute("CREATE EDGE TYPE LIVES_IN").unwrap();

            // Create graph type using pattern form
            session
                .execute(
                    "CREATE GRAPH TYPE social (\
                        (:Person {name STRING NOT NULL})-[:KNOWS {since INTEGER}]->(:Person),\
                        (:Person)-[:LIVES_IN]->(:City)\
                    )",
                )
                .unwrap();

            // Verify it was created
            let result = session.execute("SHOW GRAPH TYPE social").unwrap();
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0][0], Value::from("social"));
        }
    }
}
