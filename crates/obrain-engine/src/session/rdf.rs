//! RDF-specific session methods.
//!
//! This module consolidates all RDF functionality from the session layer.
//! The entire module is gated behind `#[cfg(feature = "rdf")]` in the parent.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
#[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
use web_time::Instant;

use obrain_common::types::{TransactionId, Value};
use obrain_common::utils::error::Result;
use obrain_core::graph::GraphStoreMut;
use obrain_core::graph::rdf::RdfStore;

use crate::database::QueryResult;

use super::{Nature, Session, SessionConfig};

impl Session {
    /// Creates a new session with RDF store and adaptive configuration.
    ///
    /// Post-T17 cutover: takes the backend as `Arc<dyn GraphStoreMut>`
    /// (SubstrateStore in practice) — the legacy `LpgStore`-typed field
    /// was removed with the cutover.
    pub(crate) fn with_rdf_store_and_adaptive(
        graph_store: Arc<dyn GraphStoreMut>,
        rdf_store: Arc<RdfStore>,
        cfg: SessionConfig,
    ) -> Self {
        Self {
            graph_store,
            catalog: cfg.catalog,
            rdf_store,
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            transaction_manager: cfg.transaction_manager,
            query_cache: cfg.query_cache,
            current_transaction: parking_lot::Mutex::new(None),
            read_only_tx: parking_lot::Mutex::new(cfg.read_only),
            db_read_only: cfg.read_only,
            auto_commit: true,
            adaptive_config: cfg.adaptive_config,
            factorized_execution: cfg.factorized_execution,
            graph_model: cfg.graph_model,
            query_timeout: cfg.query_timeout,
            commit_counter: cfg.commit_counter,
            gc_interval: cfg.gc_interval,
            transaction_start_node_count: AtomicUsize::new(0),
            transaction_start_edge_count: AtomicUsize::new(0),
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            substrate_store: None,
            #[cfg(feature = "hybrid-search")]
            recherche_hybride: None,
            #[cfg(feature = "wal")]
            wal: None,
            #[cfg(feature = "wal")]
            wal_graph_context: None,
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
            // (2026-08-29, fix/gql-divers) Meme lecon que la note du
            // 2026-08-14 ci-dessous : les trois champs V2/V3 (registre
            // multi-store, contexte de droits, drapeau auto-commit)
            // avaient ete ajoutes a Session sans completer CE troisieme
            // site de construction, invisible hors --features rdf/full.
            // Memes valeurs que le constructeur de session/mod.rs:311-313.
            base_registry: parking_lot::Mutex::new(None),
            access_context: parking_lot::Mutex::new(None),
            transaction_auto: std::sync::atomic::AtomicBool::new(false),
            // (2026-08-14) TROISIEME SITE DE CONSTRUCTION, derriere la
            // fonctionnalite `rdf`. Les deux autres etaient dans
            // session/mod.rs et je les avais vus ; celui-ci ne compile
            // qu'avec `--features rdf`, que ni `cargo check --workspace`
            // ni la suite de tests n'activent. Meme classe d'erreur que le
            // `use Value` sous cfg trouve plus tot cette nuit : un champ
            // ajoute a une structure ne se voit complet QUE si l'on
            // compile toutes les combinaisons de fonctionnalites.
            tx_baselines: parking_lot::Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "reactive")]
            post_commit_hook: None,
            #[cfg(feature = "metrics")]
            metrics: None,
            #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
            tx_start_time: parking_lot::Mutex::new(None),
        }
    }

    /// Executes a GraphQL query against the RDF store.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "graphql")]
    pub fn execute_graphql_rdf(&self, query: &str) -> Result<QueryResult> {
        use crate::query::{
            Executor, optimizer::Optimizer, planner::rdf::RdfPlanner, translators::graphql_rdf,
        };

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let logical_plan = graphql_rdf::translate(query, "http://example.org/")?;
        let active = self.active_store();
        let optimizer = Optimizer::from_graph_store(&*active);
        let optimized_plan = optimizer.optimize(logical_plan)?;

        let planner = RdfPlanner::new(Arc::clone(&self.rdf_store))
            .with_transaction_id(*self.current_transaction.lock());
        #[cfg(all(feature = "rdf", feature = "wal"))]
        let planner = planner.with_wal(self.rdf_wal.clone());
        let mut physical_plan = planner.plan(&optimized_plan)?;

        let executor = Executor::with_columns(physical_plan.columns.clone())
            .with_deadline(self.query_deadline());
        let result = executor.execute(physical_plan.operator.as_mut());

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

    /// Executes a GraphQL query against the RDF store with parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "graphql")]
    pub fn execute_graphql_rdf_with_params(
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
            #[cfg(all(feature = "rdf", feature = "wal"))]
            let processor = processor.with_rdf_wal(self.rdf_wal.clone());
            #[cfg(feature = "cdc")]
            let processor = processor.with_cdc_log(Arc::clone(self.cdc_log()));
            processor.process(query, QueryLanguage::GraphQLRdf, Some(&params))
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

    /// Executes a SPARQL query.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "sparql")]
    pub fn execute_sparql(&self, query: &str) -> Result<QueryResult> {
        use crate::query::{
            Executor, optimizer::Optimizer, planner::rdf::RdfPlanner, translators::sparql,
        };

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let logical_plan = sparql::translate(query)?;
        let rdf_stats = self.rdf_store.collect_statistics();
        let optimizer = Optimizer::from_rdf_statistics(rdf_stats);
        let optimized_plan = optimizer.optimize(logical_plan)?;

        // EXPLAIN: return the logical plan tree without executing
        if optimized_plan.explain {
            use crate::query::processor::explain_result;
            return Ok(explain_result(&optimized_plan));
        }

        let planner = RdfPlanner::new(Arc::clone(&self.rdf_store))
            .with_transaction_id(*self.current_transaction.lock());
        #[cfg(all(feature = "rdf", feature = "wal"))]
        let planner = planner.with_wal(self.rdf_wal.clone());
        let mut physical_plan = planner.plan(&optimized_plan)?;

        let executor = Executor::with_columns(physical_plan.columns.clone())
            .with_deadline(self.query_deadline());
        let result = executor.execute(physical_plan.operator.as_mut());

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("sparql", elapsed_ms, &result);
        }

        result
    }

    /// Executes a SPARQL query with parameters.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails to parse or execute.
    #[cfg(feature = "sparql")]
    pub fn execute_sparql_with_params(
        &self,
        query: &str,
        params: std::collections::HashMap<String, Value>,
    ) -> Result<QueryResult> {
        use crate::query::{
            Executor, optimizer::Optimizer, planner::rdf::RdfPlanner, processor::substitute_params,
            translators::sparql,
        };

        #[cfg(all(feature = "metrics", not(target_arch = "wasm32")))]
        let start_time = Instant::now();

        let mut logical_plan = sparql::translate(query)?;
        substitute_params(&mut logical_plan, &params)?;

        let rdf_stats = self.rdf_store.collect_statistics();
        let optimizer = Optimizer::from_rdf_statistics(rdf_stats);
        let optimized_plan = optimizer.optimize(logical_plan)?;

        // EXPLAIN: return the logical plan tree without executing
        if optimized_plan.explain {
            use crate::query::processor::explain_result;
            return Ok(explain_result(&optimized_plan));
        }

        let planner = RdfPlanner::new(Arc::clone(&self.rdf_store))
            .with_transaction_id(*self.current_transaction.lock());
        #[cfg(all(feature = "rdf", feature = "wal"))]
        let planner = planner.with_wal(self.rdf_wal.clone());
        let mut physical_plan = planner.plan(&optimized_plan)?;

        let executor = Executor::with_columns(physical_plan.columns.clone())
            .with_deadline(self.query_deadline());
        let result = executor.execute(physical_plan.operator.as_mut());

        #[cfg(feature = "metrics")]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed_ms = Some(start_time.elapsed().as_secs_f64() * 1000.0);
            #[cfg(target_arch = "wasm32")]
            let elapsed_ms = None;
            self.record_query_metrics("sparql", elapsed_ms, &result);
        }

        result
    }

    /// Shares the RDF store of the owning database. A session built by
    /// `with_external_store` starts with a private store; without this, its
    /// SPARQL writes and reads would not see the database's triples (nor the
    /// triples replayed from the journal on open).
    #[cfg(feature = "rdf")]
    pub(crate) fn set_rdf_store(&mut self, rdf_store: Arc<RdfStore>) {
        self.rdf_store = rdf_store;
    }

    /// Wires the RDF journal of the owning database (persistent substrate
    /// databases only). Called by `ObrainDB` right after session creation.
    #[cfg(all(feature = "rdf", feature = "wal"))]
    pub(crate) fn set_rdf_wal(
        &mut self,
        rdf_wal: Option<Arc<obrain_adapters::storage::wal::LpgWal>>,
    ) {
        self.rdf_wal = rdf_wal;
    }

    /// Commits RDF transaction state.
    ///
    /// Called from the main commit path to finalize RDF changes.
    ///
    /// Explicit-transaction writes are not journaled while they execute (the
    /// planner buffers them), so their journal records are written here, before
    /// the in-memory apply, and the journal is synced. Without this, a committed
    /// triple vanished on reopen. Every graph is committed, default and named:
    /// named-graph writes are buffered in their own store.
    pub(super) fn commit_rdf_transaction(&self, transaction_id: TransactionId) {
        #[cfg(all(feature = "rdf", feature = "wal"))]
        if let Some(wal) = &self.rdf_wal {
            for (graph, store) in self.rdf_stores_with_pending() {
                for (is_insert, triple) in store.pending_ops(transaction_id) {
                    let (subject, predicate, object) = (
                        triple.subject().to_string(),
                        triple.predicate().to_string(),
                        triple.object().to_string(),
                    );
                    let record = if is_insert {
                        obrain_adapters::storage::wal::WalRecord::InsertRdfTriple {
                            subject,
                            predicate,
                            object,
                            graph: graph.clone(),
                        }
                    } else {
                        obrain_adapters::storage::wal::WalRecord::DeleteRdfTriple {
                            subject,
                            predicate,
                            object,
                            graph: graph.clone(),
                        }
                    };
                    if let Err(err) = wal.log(&record) {
                        tracing::warn!("RDF WAL log failed at commit: {err}");
                    }
                }
            }
            if let Err(err) = wal.sync() {
                tracing::warn!("RDF WAL sync failed at commit: {err}");
            }
        }

        self.rdf_store.commit_transaction(transaction_id);
        for name in self.rdf_store.graph_names() {
            if let Some(graph) = self.rdf_store.graph(&name) {
                graph.commit_transaction(transaction_id);
            }
        }
    }

    /// Rolls back RDF transaction state.
    ///
    /// Called from the main commit-conflict and rollback paths to discard RDF changes.
    pub(super) fn rollback_rdf_transaction(&self, transaction_id: TransactionId) {
        self.rdf_store.rollback_transaction(transaction_id);
        for name in self.rdf_store.graph_names() {
            if let Some(graph) = self.rdf_store.graph(&name) {
                graph.rollback_transaction(transaction_id);
            }
        }
    }

    /// The default store and every named graph, each tagged with its graph name
    /// (`None` for the default graph), for journaling a transaction's writes.
    #[cfg(all(feature = "rdf", feature = "wal"))]
    fn rdf_stores_with_pending(&self) -> Vec<(Option<String>, Arc<RdfStore>)> {
        let mut stores = vec![(None, Arc::clone(&self.rdf_store))];
        for name in self.rdf_store.graph_names() {
            if let Some(graph) = self.rdf_store.graph(&name) {
                stores.push((Some(name), graph));
            }
        }
        stores
    }
}
