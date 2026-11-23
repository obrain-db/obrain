//! Query processor that orchestrates the query pipeline.
//!
//! The `QueryProcessor` is the central component that executes queries through
//! the full pipeline: Parse → Bind → Optimize → Plan → Execute.
//!
//! It supports multiple query languages (GQL, Cypher, Gremlin, GraphQL) for LPG
//! and SPARQL for RDF (when the `rdf` feature is enabled).

use std::collections::HashMap;
use std::sync::Arc;

use obrain_common::types::{EpochId, TransactionId, Value};
use obrain_common::utils::error::{Error, Result};
use obrain_core::graph::GraphStoreMut;

use crate::catalog::Catalog;
use crate::database::QueryResult;
use crate::query::binder::Binder;
use crate::query::executor::Executor;
use crate::query::optimizer::Optimizer;
use crate::query::plan::{LogicalExpression, LogicalOperator, LogicalPlan};
use crate::query::planner::Planner;
use crate::transaction::TransactionManager;

/// Supported query languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryLanguage {
    /// GQL (ISO/IEC 39075:2024) - default for LPG
    Gql,
    /// openCypher 9.0
    #[cfg(feature = "cypher")]
    Cypher,
    /// Apache TinkerPop Gremlin
    #[cfg(feature = "gremlin")]
    Gremlin,
    /// GraphQL for LPG
    #[cfg(feature = "graphql")]
    GraphQL,
    /// SQL/PGQ (SQL:2023 GRAPH_TABLE)
    #[cfg(feature = "sql-pgq")]
    SqlPgq,
    /// SPARQL 1.1 for RDF
    #[cfg(feature = "sparql")]
    Sparql,
    /// GraphQL for RDF
    #[cfg(all(feature = "graphql", feature = "rdf"))]
    GraphQLRdf,
}

impl QueryLanguage {
    /// Returns whether this language targets LPG (vs RDF).
    #[must_use]
    pub const fn is_lpg(&self) -> bool {
        match self {
            Self::Gql => true,
            #[cfg(feature = "cypher")]
            Self::Cypher => true,
            #[cfg(feature = "gremlin")]
            Self::Gremlin => true,
            #[cfg(feature = "graphql")]
            Self::GraphQL => true,
            #[cfg(feature = "sql-pgq")]
            Self::SqlPgq => true,
            #[cfg(feature = "sparql")]
            Self::Sparql => false,
            #[cfg(all(feature = "graphql", feature = "rdf"))]
            Self::GraphQLRdf => false,
        }
    }
}

/// Query parameters for prepared statements.
pub type QueryParams = HashMap<String, Value>;

/// Processes queries through the full pipeline.
///
/// The processor holds references to the stores and provides a unified
/// interface for executing queries in any supported language.
///
/// # Example
///
/// Post-T17: the public entry point is [`for_graph_store_with_transaction`],
/// which accepts any [`GraphStoreMut`] — in production this is a
/// [`SubstrateStore`](obrain_substrate::SubstrateStore) routed through
/// [`ObrainDB::graph_store_mut()`](crate::ObrainDB::graph_store_mut).
///
/// ```ignore
/// use std::sync::Arc;
/// use obrain_engine::{ObrainDB, query::processor::{QueryProcessor, QueryLanguage}};
/// use obrain_engine::transaction::TransactionManager;
///
/// let db = ObrainDB::new_in_memory();
/// let store = db.graph_store_mut();
/// let tm = Arc::new(TransactionManager::new());
/// let processor = QueryProcessor::for_graph_store_with_transaction(store, tm)?;
/// let _result = processor.process("MATCH (n:Person) RETURN n", QueryLanguage::Gql, None)?;
/// # Ok::<(), obrain_common::utils::error::Error>(())
/// ```
///
/// [`for_graph_store_with_transaction`]: Self::for_graph_store_with_transaction
pub struct QueryProcessor {
    /// Graph store trait object for pluggable storage backends.
    ///
    /// T17 cutover: the legacy `lpg_store: Arc<LpgStore>` field is gone. All
    /// queries now route through this single trait-object; tests that need a
    /// typed LpgStore construct their own before wrapping it here.
    graph_store: Arc<dyn GraphStoreMut>,
    /// Transaction manager for MVCC operations.
    transaction_manager: Arc<TransactionManager>,
    /// Catalog for schema and index metadata.
    catalog: Arc<Catalog>,
    /// Query optimizer.
    optimizer: Optimizer,
    /// Current transaction context (if any).
    transaction_context: Option<(EpochId, TransactionId)>,
    /// Contexte de session (schema courant, introspection, registre
    /// multi-store — aretes inter-bases V2). Ce processeur batit son
    /// PROPRE Planner : sans ce champ, execute_with_params/gremlin/
    /// graphql/sql perdaient le registre en silence (trou P1).
    session_context: obrain_core::execution::operators::SessionContext,
    /// RDF store for triple pattern queries (optional).
    #[cfg(feature = "rdf")]
    rdf_store: Option<Arc<obrain_core::graph::rdf::RdfStore>>,
    /// Journal WAL of the RDF store, so RDF mutations planned here are logged.
    #[cfg(all(feature = "rdf", feature = "wal"))]
    rdf_wal: Option<Arc<obrain_adapters::storage::wal::LpgWal>>,
    /// Optional cognitive engine (3 piliers), threaded from the owning
    /// `Session` so `CALL obrain.cognitive.*` resolves in `execute()`'s own
    /// `Planner::with_context` calls (this processor builds its own planner
    /// independently of `Session::create_planner_for_store*`).
    #[cfg(feature = "cognitive")]
    cognitive_engine: Option<Arc<dyn obrain_cognitive::CognitiveEngine>>,
    /// Optional typed substrate handle, threaded from the owning `Session`
    /// so substrate-native procedures (`CALL obrain.window_2d`) resolve in
    /// this processor's own planner builds (same pattern as the cognitive
    /// engine above).
    substrate_store: Option<Arc<obrain_substrate::SubstrateStore>>,
    /// Optional CDC log handle (feature `cdc`), threaded from the owning
    /// `Session` so change-data procedures (`CALL obrain.search_delta`)
    /// resolve in this processor's own planner builds (same pattern).
    #[cfg(feature = "cdc")]
    cdc_log: Option<Arc<crate::cdc::CdcLog>>,
    /// Poignée de recherche hybride (`CALL obrain.hybrid_search`), même
    /// câblage que `substrate_store`.
    #[cfg(feature = "hybrid-search")]
    recherche_hybride: Option<Arc<dyn crate::database::hybrid_procedure::RechercheHybride>>,
    /// Poignée des index spatiaux : le planificateur en a besoin pour router
    /// `WHERE distance(a, b) < r` vers un scan d'index au lieu d'un balayage.
    index_spatiaux: Option<Arc<dyn crate::database::spatial_procedure::IndexSpatiaux>>,
    /// Poignée des index vectoriels nommés (`CALL db.index.vector.queryNodes`).
    #[cfg(feature = "vector-index")]
    index_vectoriels_nommes:
        Option<Arc<dyn crate::database::vector_query_nodes::IndexVectorielsNommes>>,
}

impl QueryProcessor {
    /// Creates a new query processor for LPG queries (test-only).
    ///
    /// Post-T17 W3b / W4.p4: production code uses
    /// [`for_graph_store_with_transaction`] exclusively. This helper is kept
    /// for in-crate unit tests that want a no-transaction processor on top of
    /// any `GraphStoreMut`-implementing backend — non-test callers must go
    /// through the trait-based constructor.
    ///
    /// [`for_graph_store_with_transaction`]: Self::for_graph_store_with_transaction
    #[cfg(test)]
    #[must_use]
    pub fn for_lpg(graph_store: Arc<dyn GraphStoreMut>) -> Self {
        let optimizer = Optimizer::from_graph_store(&*graph_store);
        Self {
            graph_store,
            transaction_manager: Arc::new(TransactionManager::new()),
            catalog: Arc::new(Catalog::new()),
            optimizer,
            transaction_context: None,
            session_context: obrain_core::execution::operators::SessionContext::default(),
            #[cfg(feature = "rdf")]
            rdf_store: None,
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            substrate_store: None,
            #[cfg(feature = "cdc")]
            cdc_log: None,
            #[cfg(feature = "hybrid-search")]
            recherche_hybride: None,
            index_spatiaux: None,
            #[cfg(feature = "vector-index")]
            index_vectoriels_nommes: None,
        }
    }

    /// Creates a new query processor with a transaction manager (test-only).
    #[cfg(test)]
    #[must_use]
    #[allow(dead_code)]
    pub fn for_lpg_with_transaction(
        graph_store: Arc<dyn GraphStoreMut>,
        transaction_manager: Arc<TransactionManager>,
    ) -> Self {
        let optimizer = Optimizer::from_graph_store(&*graph_store);
        Self {
            graph_store,
            transaction_manager,
            catalog: Arc::new(Catalog::new()),
            optimizer,
            transaction_context: None,
            session_context: obrain_core::execution::operators::SessionContext::default(),
            #[cfg(feature = "rdf")]
            rdf_store: None,
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            substrate_store: None,
            #[cfg(feature = "cdc")]
            cdc_log: None,
            #[cfg(feature = "hybrid-search")]
            recherche_hybride: None,
            index_spatiaux: None,
            #[cfg(feature = "vector-index")]
            index_vectoriels_nommes: None,
        }
    }

    /// Creates a query processor backed by any `GraphStoreMut` implementation.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal arena allocation fails (out of memory).
    pub fn for_graph_store_with_transaction(
        store: Arc<dyn GraphStoreMut>,
        transaction_manager: Arc<TransactionManager>,
    ) -> Result<Self> {
        let optimizer = Optimizer::from_graph_store(&*store);
        Ok(Self {
            graph_store: store,
            transaction_manager,
            catalog: Arc::new(Catalog::new()),
            optimizer,
            transaction_context: None,
            session_context: obrain_core::execution::operators::SessionContext::default(),
            #[cfg(feature = "rdf")]
            rdf_store: None,
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            substrate_store: None,
            #[cfg(feature = "cdc")]
            cdc_log: None,
            #[cfg(feature = "hybrid-search")]
            recherche_hybride: None,
            index_spatiaux: None,
            #[cfg(feature = "vector-index")]
            index_vectoriels_nommes: None,
        })
    }

    /// Sets the transaction context for MVCC visibility.
    ///
    /// This should be called when the processor is used within a transaction.
    #[must_use]
    pub fn with_transaction_context(
        mut self,
        viewing_epoch: EpochId,
        transaction_id: TransactionId,
    ) -> Self {
        self.transaction_context = Some((viewing_epoch, transaction_id));
        self
    }

    /// Attache le contexte de session (registre multi-store compris —
    /// aretes inter-bases V2) aux planners construits par ce processeur.
    #[must_use]
    pub fn with_session_context(
        mut self,
        context: obrain_core::execution::operators::SessionContext,
    ) -> Self {
        self.session_context = context;
        self
    }

    /// Sets a custom catalog.
    #[must_use]
    pub fn with_catalog(mut self, catalog: Arc<Catalog>) -> Self {
        self.catalog = catalog;
        self
    }

    /// Sets a custom optimizer.
    #[must_use]
    pub fn with_optimizer(mut self, optimizer: Optimizer) -> Self {
        self.optimizer = optimizer;
        self
    }

    /// Sets the cognitive engine (3 piliers), so `CALL obrain.cognitive.*`
    /// resolves in this processor's own `Planner::with_context` calls inside
    /// `execute()`, instead of falling through to "Unknown procedure".
    #[cfg(feature = "cognitive")]
    #[must_use]
    pub fn with_cognitive_engine(
        mut self,
        engine: Arc<dyn obrain_cognitive::CognitiveEngine>,
    ) -> Self {
        self.cognitive_engine = Some(engine);
        self
    }

    /// Attaches the typed substrate handle so substrate-native procedures
    /// (`CALL obrain.window_2d`) resolve in this processor's own planner
    /// builds. Same pattern as [`Self::with_cognitive_engine`].
    #[must_use]
    pub fn with_substrate_store(mut self, store: Arc<obrain_substrate::SubstrateStore>) -> Self {
        self.substrate_store = Some(store);
        self
    }

    /// Attache la poignée de recherche hybride (`CALL obrain.hybrid_search`).
    /// Même câblage que [`Self::with_substrate_store`].
    #[cfg(feature = "hybrid-search")]
    #[must_use]
    pub fn with_recherche_hybride(
        mut self,
        poignee: Arc<dyn crate::database::hybrid_procedure::RechercheHybride>,
    ) -> Self {
        self.recherche_hybride = Some(poignee);
        self
    }

    /// Pose la poignée des index spatiaux, transmise au planificateur.
    ///
    /// `mut` + retour `Self` comme les autres `with_*` : la poignée voyage avec
    /// le processeur jusqu'au planificateur, qui route le prédicat de distance.
    #[must_use]
    pub fn with_index_spatiaux(
        mut self,
        poignee: Arc<dyn crate::database::spatial_procedure::IndexSpatiaux>,
    ) -> Self {
        self.index_spatiaux = Some(poignee);
        self
    }

    /// Attache la poignée des index vectoriels nommés (`CALL
    /// db.index.vector.queryNodes`, #209). Même câblage que
    /// [`Self::with_recherche_hybride`].
    #[cfg(feature = "vector-index")]
    #[must_use]
    pub fn with_index_vectoriels_nommes(
        mut self,
        poignee: Arc<dyn crate::database::vector_query_nodes::IndexVectorielsNommes>,
    ) -> Self {
        self.index_vectoriels_nommes = Some(poignee);
        self
    }

    /// Attaches the CDC log so change-data procedures (`CALL
    /// obrain.search_delta`) resolve in this processor's own planner builds.
    #[cfg(feature = "cdc")]
    #[must_use]
    pub fn with_cdc_log(mut self, cdc: Arc<crate::cdc::CdcLog>) -> Self {
        self.cdc_log = Some(cdc);
        self
    }

    /// Processes a query string and returns results.
    ///
    /// Pipeline:
    /// 1. Parse (language-specific parser → AST)
    /// 2. Translate (AST → LogicalPlan)
    /// 3. Bind (semantic validation)
    /// 4. Optimize (filter pushdown, join reorder, etc.)
    /// 5. Plan (logical → physical operators)
    /// 6. Execute (run operators, collect results)
    ///
    /// # Arguments
    ///
    /// * `query` - The query string
    /// * `language` - Which query language to use
    /// * `params` - Optional query parameters for prepared statements
    ///
    /// # Errors
    ///
    /// Returns an error if any stage of the pipeline fails.
    pub fn process(
        &self,
        query: &str,
        language: QueryLanguage,
        params: Option<&QueryParams>,
    ) -> Result<QueryResult> {
        if language.is_lpg() {
            self.process_lpg(query, language, params)
        } else {
            #[cfg(feature = "rdf")]
            {
                self.process_rdf(query, language, params)
            }
            #[cfg(not(feature = "rdf"))]
            {
                Err(Error::Internal(
                    "RDF support not enabled. Compile with --features rdf".to_string(),
                ))
            }
        }
    }

    /// Processes an LPG query (GQL, Cypher, Gremlin, GraphQL).
    fn process_lpg(
        &self,
        query: &str,
        language: QueryLanguage,
        params: Option<&QueryParams>,
    ) -> Result<QueryResult> {
        #[cfg(not(target_arch = "wasm32"))]
        let start_time = web_time::Instant::now();

        // 1. Parse and translate to logical plan
        let mut logical_plan = self.translate_lpg(query, language)?;

        // 2. Substitute parameters if provided
        if let Some(params) = params {
            substitute_params(&mut logical_plan, params)?;
        }

        // 3. Semantic validation
        let mut binder = Binder::new();
        let _binding_context = binder.bind(&logical_plan)?;

        // 4. Optimize the plan
        let optimized_plan = self.optimizer.optimize(logical_plan)?;

        // 4a. EXPLAIN: annotate pushdown hints and return the plan tree
        if optimized_plan.explain {
            let mut plan = optimized_plan;
            annotate_pushdown_hints(&mut plan.root, self.graph_store.as_ref());
            return Ok(explain_result(&plan));
        }

        // ── FRONTIÈRE ÉCRITURE / LECTURE : LA DÉCISION QUI FAIT AUTORITÉ
        //    (2026-08-24, revue adversariale) ───────────────────────────
        //
        // Les quatre entrées `_with_params` de la session remettent ici un
        // TEXTE de requête ; elles n'ont pas de plan et ne peuvent donc pas
        // répondre à « est-ce que ça mute ? » autrement qu'en devinant.
        // Elles devinaient : `upper.contains("SET")`, vrai de
        // `RETURN n.offset` — un identifiant utilisateur décidait de
        // l'interruptibilité d'une lecture.
        //
        // Le plan optimisé est disponible ICI, et il répond
        // structurellement. Cette frontière-ci est INTÉRIEURE à celle de
        // `Session::with_auto_commit` : elle la corrige dans les DEUX sens —
        // elle remet en vigueur une lecture que le texte avait prise pour
        // une écriture, et elle met hors vigueur une écriture que le texte
        // n'avait pas reconnue (gremlin `addV`, par exemple, ne contient
        // aucun mot-clé de mutation).
        //
        // La garde vit jusqu'à la fin de la fonction, donc couvre la
        // planification physique ET l'exécution.
        let mute = optimized_plan.root.mutations_avec(Some(&self.catalog));
        let _frontiere = obrain_common::utils::cancel::frontiere_heritee(mute);

        // 5. Convert to physical plan with transaction context
        // Read-only fast path: safe when no mutations AND no active transaction
        // (an active transaction may have prior uncommitted writes from earlier statements)
        let is_read_only = !mute && self.transaction_context.is_none();
        let planner = if let Some((epoch, transaction_id)) = self.transaction_context {
            Planner::with_context(
                Arc::clone(&self.graph_store),
                Arc::clone(&self.transaction_manager),
                Some(transaction_id),
                epoch,
            )
        } else {
            Planner::with_context(
                Arc::clone(&self.graph_store),
                Arc::clone(&self.transaction_manager),
                None,
                self.transaction_manager.current_epoch(),
            )
        }
        .with_read_only(is_read_only)
        // ── UN SEUL CATALOGUE POUR LES DEUX DECISIONS (2026-08-24) ───
        //
        // `mute` ci-dessus CLASSE contre `self.catalog` ; ce planificateur
        // EXECUTE. Sans cette ligne, il n'avait pas de catalogue du tout :
        // les deux repondaient a partir de sources differentes. C'est
        // inerte aujourd'hui — les cinq constructions de production du
        // `QueryProcessor` n'appellent jamais `with_catalog`, donc les deux
        // cotes voient un catalogue vide et s'accordent par accident. Le
        // jour ou quelqu'un cablerait `with_catalog` d'un seul cote, une
        // procedure utilisateur serait EXECUTEE par l'un et classee LECTURE
        // par l'autre : la frontiere armerait le jeton sur une ecriture.
        //
        // On ne laisse pas une divergence dormir en attendant ce jour-la :
        // les deux consommateurs partagent desormais la meme source, et
        // cabler le catalogue les servira tous les deux d'un coup.
        .with_catalog(Arc::clone(&self.catalog))
        // Aretes inter-bases (V2) : sans cette ligne, le planner du
        // processeur ignorait le registre multi-store (trou P1).
        .with_session_context(self.session_context.clone());
        #[cfg(feature = "cognitive")]
        let planner = if let Some(ref engine) = self.cognitive_engine {
            planner.with_cognitive_engine(Arc::clone(engine))
        } else {
            planner
        };
        let planner = if let Some(ref sub) = self.substrate_store {
            planner.with_substrate_store(Arc::clone(sub))
        } else {
            planner
        };
        #[cfg(feature = "cdc")]
        let planner = if let Some(ref cdc) = self.cdc_log {
            planner.with_cdc_log(Arc::clone(cdc))
        } else {
            planner
        };
        #[cfg(feature = "hybrid-search")]
        let planner = if let Some(ref poignee) = self.recherche_hybride {
            planner.with_recherche_hybride(Arc::clone(poignee))
        } else {
            planner
        };
        #[cfg(feature = "vector-index")]
        let planner = if let Some(ref poignee) = self.index_vectoriels_nommes {
            planner.with_index_vectoriels_nommes(Arc::clone(poignee))
        } else {
            planner
        };
        let planner = if let Some(ref poignee) = self.index_spatiaux {
            planner.with_index_spatiaux(Arc::clone(poignee))
        } else {
            planner
        };
        let planner =
            planner.avec_marqueur_segment(crate::query::plan::marqueur_segment_pour(query));
        let mut physical_plan = planner.plan(&optimized_plan)?;

        // 6. Execute and collect results
        let executor = Executor::with_columns(physical_plan.columns.clone());
        let mut result = executor.execute(physical_plan.operator.as_mut())?;

        // Add execution metrics
        let rows_scanned = result.rows.len() as u64; // Approximate: rows returned
        #[cfg(not(target_arch = "wasm32"))]
        {
            let elapsed_ms = start_time.elapsed().as_secs_f64() * 1000.0;
            result.execution_time_ms = Some(elapsed_ms);
        }
        result.rows_scanned = Some(rows_scanned);

        Ok(result)
    }

    /// Translates an LPG query to a logical plan.
    fn translate_lpg(&self, query: &str, language: QueryLanguage) -> Result<LogicalPlan> {
        let _span = tracing::debug_span!("obrain::query::parse", ?language).entered();
        match language {
            QueryLanguage::Gql => {
                use crate::query::translators::gql;
                gql::translate(query)
            }
            #[cfg(feature = "cypher")]
            QueryLanguage::Cypher => {
                use crate::query::translators::cypher;
                cypher::translate(query)
            }
            #[cfg(feature = "gremlin")]
            QueryLanguage::Gremlin => {
                use crate::query::translators::gremlin;
                gremlin::translate(query)
            }
            #[cfg(feature = "graphql")]
            QueryLanguage::GraphQL => {
                use crate::query::translators::graphql;
                graphql::translate(query)
            }
            #[cfg(feature = "sql-pgq")]
            QueryLanguage::SqlPgq => {
                use crate::query::translators::sql_pgq;
                sql_pgq::translate(query)
            }
            #[allow(unreachable_patterns)]
            _ => Err(Error::Internal(format!(
                "Language {:?} is not an LPG language",
                language
            ))),
        }
    }

    /// Returns a reference to the catalog.
    #[must_use]
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    /// Returns a reference to the optimizer.
    #[must_use]
    pub fn optimizer(&self) -> &Optimizer {
        &self.optimizer
    }
}

impl QueryProcessor {
    /// Returns a reference to the transaction manager.
    #[must_use]
    pub fn transaction_manager(&self) -> &Arc<TransactionManager> {
        &self.transaction_manager
    }
}

// =========================================================================
// RDF-specific methods (gated behind `rdf` feature)
// =========================================================================

#[cfg(feature = "rdf")]
impl QueryProcessor {
    /// Creates a new query processor with both LPG and RDF stores (test-only).
    ///
    /// Post-T17 W3b / W4.p4: zero non-test call sites. Retained behind
    /// `#[cfg(test)]` + `#[allow(dead_code)]` for future RDF integration tests
    /// that want a backend-agnostic processor alongside an `RdfStore`.
    /// Production code uses [`for_graph_store_with_transaction`] and attaches
    /// the RDF store via the session-level plumbing.
    ///
    /// [`for_graph_store_with_transaction`]: Self::for_graph_store_with_transaction
    #[cfg(test)]
    #[must_use]
    #[allow(dead_code)]
    pub fn with_rdf(
        graph_store: Arc<dyn GraphStoreMut>,
        rdf_store: Arc<obrain_core::graph::rdf::RdfStore>,
    ) -> Self {
        let optimizer = Optimizer::from_graph_store(&*graph_store);
        Self {
            graph_store,
            transaction_manager: Arc::new(TransactionManager::new()),
            catalog: Arc::new(Catalog::new()),
            optimizer,
            transaction_context: None,
            session_context: obrain_core::execution::operators::SessionContext::default(),
            rdf_store: Some(rdf_store),
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            // (2026-11-01) Mêmes symptômes que ci-dessous pour deux autres
            // champs : visibles seulement sous `--all-features --all-targets`,
            // que la CI ne compilait pas.
            substrate_store: None,
            #[cfg(feature = "cdc")]
            cdc_log: None,
            #[cfg(feature = "hybrid-search")]
            recherche_hybride: None,
            index_spatiaux: None,
            #[cfg(feature = "vector-index")]
            index_vectoriels_nommes: None,
            // (2026-08-14) CE CHAMP MANQUAIT, et le trou n'etait visible que
            // sous `--all-features --all-targets` : `with_rdf` est
            // `#[cfg(test)]` et exige `rdf`, tandis que `cognitive_engine` est
            // derriere `cognitive`. Il fallait donc les TROIS a la fois pour
            // que le compilateur en parle — combinaison que ni
            // `cargo check --workspace` ni `cargo test` n'activent.
            //
            // L'etape clippy de la CI, elle, l'active
            // (`--all-targets --all-features -- -D warnings`). Elle echouait
            // donc AVANT le travail de cette nuit, pour une raison qui ne
            // tient ni au gel de `main` ni a l'acces au depot.
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
        }
    }

    /// Attaches the RDF journal so RDF mutations planned by this processor are
    /// logged. Same pattern as [`Self::with_substrate_store`].
    #[cfg(all(feature = "rdf", feature = "wal"))]
    #[must_use]
    pub fn with_rdf_wal(
        mut self,
        rdf_wal: Option<Arc<obrain_adapters::storage::wal::LpgWal>>,
    ) -> Self {
        self.rdf_wal = rdf_wal;
        self
    }

    /// Returns a reference to the RDF store (if configured).
    #[must_use]
    pub fn rdf_store(&self) -> Option<&Arc<obrain_core::graph::rdf::RdfStore>> {
        self.rdf_store.as_ref()
    }

    /// Processes an RDF query (SPARQL, GraphQL-RDF).
    fn process_rdf(
        &self,
        query: &str,
        language: QueryLanguage,
        params: Option<&QueryParams>,
    ) -> Result<QueryResult> {
        use crate::query::planner::rdf::RdfPlanner;

        let rdf_store = self.rdf_store.as_ref().ok_or_else(|| {
            Error::Internal("RDF store not configured for this processor".to_string())
        })?;

        // 1. Parse and translate to logical plan
        let mut logical_plan = self.translate_rdf(query, language)?;

        // 2. Substitute parameters if provided
        if let Some(params) = params {
            substitute_params(&mut logical_plan, params)?;
        }

        // 3. Semantic validation
        let mut binder = Binder::new();
        let _binding_context = binder.bind(&logical_plan)?;

        // 3. Optimize the plan
        let optimized_plan = self.optimizer.optimize(logical_plan)?;

        // 3a. EXPLAIN: return the optimized plan tree without executing
        if optimized_plan.explain {
            return Ok(explain_result(&optimized_plan));
        }

        // Même frontière que sur le chemin LPG (voir `process_lpg`) : la
        // décision structurelle du plan, prise au seul endroit où le plan
        // existe sur ce chemin.
        let _frontiere = obrain_common::utils::cancel::frontiere_heritee(
            optimized_plan.root.mutations_avec(Some(&self.catalog)),
        );

        // 4. Convert to physical plan (using RDF planner)
        let planner = RdfPlanner::new(Arc::clone(rdf_store));
        #[cfg(all(feature = "rdf", feature = "wal"))]
        let planner = planner.with_wal(self.rdf_wal.clone());
        let mut physical_plan = planner.plan(&optimized_plan)?;

        // 5. Execute and collect results
        let executor = Executor::with_columns(physical_plan.columns.clone());
        executor.execute(physical_plan.operator.as_mut())
    }

    /// Translates an RDF query to a logical plan.
    fn translate_rdf(&self, query: &str, language: QueryLanguage) -> Result<LogicalPlan> {
        match language {
            #[cfg(feature = "sparql")]
            QueryLanguage::Sparql => {
                use crate::query::translators::sparql;
                sparql::translate(query)
            }
            #[cfg(all(feature = "graphql", feature = "rdf"))]
            QueryLanguage::GraphQLRdf => {
                use crate::query::translators::graphql_rdf;
                // Default namespace for GraphQL-RDF queries
                graphql_rdf::translate(query, "http://example.org/")
            }
            _ => Err(Error::Internal(format!(
                "Language {:?} is not an RDF language",
                language
            ))),
        }
    }
}

/// Annotates filter operators in the plan with pushdown hints.
///
/// Walks the plan tree looking for `Filter -> NodeScan` patterns and checks
/// whether a property index exists for equality predicates.
pub(crate) fn annotate_pushdown_hints(
    op: &mut LogicalOperator,
    store: &dyn obrain_core::graph::GraphStore,
) {
    annotate_pushdown_hints_capped(op, store, None);
}

/// Variante avec propagation d'un cap de sources (RUN 21).
///
/// `cap`: nombre de sources à demander au scan amont, posé par un `Limit`
/// et propagé à travers Return/Filter/Expand jusqu'au NodeScan-feuille.
/// Le facteur ×16 compense les sources rejetées par l'expand (un nœud sans
/// arête du type demandé ne produit aucune ligne) — un sur-approvisionnement
/// sûr : le LimitOperator au sommet garantit le contrat de sortie exact.
fn annotate_pushdown_hints_capped(
    op: &mut LogicalOperator,
    store: &dyn obrain_core::graph::GraphStore,
    cap: Option<usize>,
) {
    #[allow(clippy::wildcard_imports)]
    use crate::query::plan::*;

    match op {
        LogicalOperator::Filter(filter) => {
            // Recurse into children first (cap inchangé à travers le filtre)
            annotate_pushdown_hints_capped(&mut filter.input, store, cap);

            // Annotate this filter if it sits on top of a NodeScan
            if let LogicalOperator::NodeScan(scan) = filter.input.as_ref() {
                filter.pushdown_hint = infer_pushdown(&filter.predicate, scan, store);
            }
        }
        LogicalOperator::NodeScan(op) => {
            // Feuille avec label + cap demandé → SourceCap pour
            // nodes_by_label_capped.
            if op.input.is_none()
                && let Some(n) = cap
                && op.label.is_some()
            {
                op.pushdown_hint = Some(PushdownHint::SourceCap { count: n });
            }
            if let Some(input) = &mut op.input {
                annotate_pushdown_hints_capped(input, store, cap);
            }
        }
        LogicalOperator::EdgeScan(op) => {
            if let Some(input) = &mut op.input {
                annotate_pushdown_hints_capped(input, store, cap);
            }
        }
        // L'Expand propage le cap vers sa source (le NodeScan amont) : c'est
        // lui qui a besoin de plusieurs sources pour trouver des arêtes.
        LogicalOperator::Expand(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        LogicalOperator::Project(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        LogicalOperator::Join(op) => {
            annotate_pushdown_hints_capped(&mut op.left, store, cap);
            annotate_pushdown_hints_capped(&mut op.right, store, cap);
        }
        LogicalOperator::Aggregate(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        // Le Limit POSE le cap (×16, voir doc du variant SourceCap).
        LogicalOperator::Limit(op) => {
            let n = op.count.value().saturating_mul(16).max(1);
            annotate_pushdown_hints_capped(&mut op.input, store, Some(n));
        }
        LogicalOperator::Skip(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        LogicalOperator::Sort(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        LogicalOperator::Distinct(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        LogicalOperator::Return(op) => annotate_pushdown_hints_capped(&mut op.input, store, cap),
        LogicalOperator::Union(op) => {
            for input in &mut op.inputs {
                annotate_pushdown_hints_capped(input, store, cap);
            }
        }
        LogicalOperator::Apply(op) => {
            annotate_pushdown_hints_capped(&mut op.input, store, cap);
            annotate_pushdown_hints_capped(&mut op.subplan, store, cap);
        }
        LogicalOperator::Otherwise(op) => {
            annotate_pushdown_hints_capped(&mut op.left, store, cap);
            annotate_pushdown_hints_capped(&mut op.right, store, cap);
        }
        _ => {}
    }
}

/// Infers the pushdown strategy for a filter predicate over a node scan.
fn infer_pushdown(
    predicate: &LogicalExpression,
    scan: &crate::query::plan::NodeScanOp,
    store: &dyn obrain_core::graph::GraphStore,
) -> Option<crate::query::plan::PushdownHint> {
    #[allow(clippy::wildcard_imports)]
    use crate::query::plan::*;

    match predicate {
        // Equality: n.prop = value
        LogicalExpression::Binary { left, op, right } if *op == BinaryOp::Eq => {
            if let Some(prop) = extract_property_name(left, &scan.variable)
                .or_else(|| extract_property_name(right, &scan.variable))
            {
                if store.has_property_index(&prop) {
                    return Some(PushdownHint::IndexLookup { property: prop });
                }
                if scan.label.is_some() {
                    return Some(PushdownHint::LabelFirst);
                }
            }
            None
        }
        // Range: n.prop > value, n.prop < value, etc.
        LogicalExpression::Binary {
            left,
            op: BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Lt | BinaryOp::Le,
            right,
        } => {
            if let Some(prop) = extract_property_name(left, &scan.variable)
                .or_else(|| extract_property_name(right, &scan.variable))
            {
                if store.has_property_index(&prop) {
                    return Some(PushdownHint::RangeScan { property: prop });
                }
                if scan.label.is_some() {
                    return Some(PushdownHint::LabelFirst);
                }
            }
            None
        }
        // AND: check the left side (first conjunct) for pushdown
        LogicalExpression::Binary {
            left,
            op: BinaryOp::And,
            ..
        } => infer_pushdown(left, scan, store),
        _ => {
            // Any other predicate on a labeled scan gets label-first
            if scan.label.is_some() {
                Some(PushdownHint::LabelFirst)
            } else {
                None
            }
        }
    }
}

/// Extracts the property name if the expression is `Property { variable, property }`
/// and the variable matches the scan variable.
fn extract_property_name(expr: &LogicalExpression, scan_var: &str) -> Option<String> {
    if let LogicalExpression::Property { variable, property } = expr
        && variable == scan_var
    {
        Some(property.clone())
    } else {
        None
    }
}

/// Builds a `QueryResult` containing the EXPLAIN plan tree text.
pub(crate) fn explain_result(plan: &LogicalPlan) -> QueryResult {
    let tree_text = plan.root.explain_tree();
    QueryResult {
        columns: vec!["plan".to_string()],
        column_types: vec![obrain_common::types::LogicalType::String],
        rows: vec![vec![Value::String(tree_text.into())]],
        execution_time_ms: None,
        rows_scanned: None,
        status_message: None,
        gql_status: obrain_common::utils::GqlStatus::SUCCESS,
    }
}

/// Substitutes parameters in a logical plan with their values.
pub fn substitute_params(plan: &mut LogicalPlan, params: &QueryParams) -> Result<()> {
    substitute_in_operator(&mut plan.root, params)
}

/// Recursively substitutes parameters in an operator.
fn substitute_in_operator(op: &mut LogicalOperator, params: &QueryParams) -> Result<()> {
    #[allow(clippy::wildcard_imports)]
    use crate::query::plan::*;

    match op {
        LogicalOperator::Filter(filter) => {
            substitute_in_expression(&mut filter.predicate, params)?;
            substitute_in_operator(&mut filter.input, params)?;
        }
        LogicalOperator::Return(ret) => {
            for item in &mut ret.items {
                substitute_in_expression(&mut item.expression, params)?;
            }
            substitute_in_operator(&mut ret.input, params)?;
        }
        LogicalOperator::Project(proj) => {
            for p in &mut proj.projections {
                substitute_in_expression(&mut p.expression, params)?;
            }
            substitute_in_operator(&mut proj.input, params)?;
        }
        LogicalOperator::NodeScan(scan) => {
            if let Some(input) = &mut scan.input {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::EdgeScan(scan) => {
            if let Some(input) = &mut scan.input {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::Expand(expand) => {
            substitute_in_operator(&mut expand.input, params)?;
        }
        LogicalOperator::Join(join) => {
            substitute_in_operator(&mut join.left, params)?;
            substitute_in_operator(&mut join.right, params)?;
            for cond in &mut join.conditions {
                substitute_in_expression(&mut cond.left, params)?;
                substitute_in_expression(&mut cond.right, params)?;
            }
        }
        LogicalOperator::LeftJoin(join) => {
            substitute_in_operator(&mut join.left, params)?;
            substitute_in_operator(&mut join.right, params)?;
            if let Some(cond) = &mut join.condition {
                substitute_in_expression(cond, params)?;
            }
        }
        LogicalOperator::Aggregate(agg) => {
            for expr in &mut agg.group_by {
                substitute_in_expression(expr, params)?;
            }
            for agg_expr in &mut agg.aggregates {
                if let Some(expr) = &mut agg_expr.expression {
                    substitute_in_expression(expr, params)?;
                }
            }
            substitute_in_operator(&mut agg.input, params)?;
        }
        LogicalOperator::Sort(sort) => {
            for key in &mut sort.keys {
                substitute_in_expression(&mut key.expression, params)?;
            }
            substitute_in_operator(&mut sort.input, params)?;
        }
        LogicalOperator::Limit(limit) => {
            resolve_count_param(&mut limit.count, params)?;
            substitute_in_operator(&mut limit.input, params)?;
        }
        LogicalOperator::Skip(skip) => {
            resolve_count_param(&mut skip.count, params)?;
            substitute_in_operator(&mut skip.input, params)?;
        }
        LogicalOperator::Distinct(distinct) => {
            substitute_in_operator(&mut distinct.input, params)?;
        }
        LogicalOperator::CreateNode(create) => {
            for (_, expr) in &mut create.properties {
                substitute_in_expression(expr, params)?;
            }
            if let Some(input) = &mut create.input {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::CreateEdge(create) => {
            for (_, expr) in &mut create.properties {
                substitute_in_expression(expr, params)?;
            }
            substitute_in_operator(&mut create.input, params)?;
        }
        LogicalOperator::CreateExternalEdge(create) => {
            substitute_in_operator(&mut create.input, params)?;
        }
        LogicalOperator::DeleteNode(delete) => {
            substitute_in_operator(&mut delete.input, params)?;
        }
        LogicalOperator::DeleteEdge(delete) => {
            substitute_in_operator(&mut delete.input, params)?;
        }
        LogicalOperator::SetProperty(set) => {
            for (_, expr) in &mut set.properties {
                substitute_in_expression(expr, params)?;
            }
            substitute_in_operator(&mut set.input, params)?;
        }
        LogicalOperator::Union(union) => {
            for input in &mut union.inputs {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::AntiJoin(anti) => {
            substitute_in_operator(&mut anti.left, params)?;
            substitute_in_operator(&mut anti.right, params)?;
        }
        LogicalOperator::Bind(bind) => {
            substitute_in_expression(&mut bind.expression, params)?;
            substitute_in_operator(&mut bind.input, params)?;
        }
        LogicalOperator::TripleScan(scan) => {
            if let Some(input) = &mut scan.input {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::Unwind(unwind) => {
            substitute_in_expression(&mut unwind.expression, params)?;
            substitute_in_operator(&mut unwind.input, params)?;
        }
        LogicalOperator::MapCollect(mc) => {
            substitute_in_operator(&mut mc.input, params)?;
        }
        LogicalOperator::Merge(merge) => {
            for (_, expr) in &mut merge.match_properties {
                substitute_in_expression(expr, params)?;
            }
            for (_, expr) in &mut merge.on_create {
                substitute_in_expression(expr, params)?;
            }
            for (_, expr) in &mut merge.on_match {
                substitute_in_expression(expr, params)?;
            }
            substitute_in_operator(&mut merge.input, params)?;
        }
        LogicalOperator::MergeRelationship(merge_rel) => {
            for (_, expr) in &mut merge_rel.match_properties {
                substitute_in_expression(expr, params)?;
            }
            for (_, expr) in &mut merge_rel.on_create {
                substitute_in_expression(expr, params)?;
            }
            for (_, expr) in &mut merge_rel.on_match {
                substitute_in_expression(expr, params)?;
            }
            substitute_in_operator(&mut merge_rel.input, params)?;
        }
        LogicalOperator::AddLabel(add_label) => {
            substitute_in_operator(&mut add_label.input, params)?;
        }
        LogicalOperator::RemoveLabel(remove_label) => {
            substitute_in_operator(&mut remove_label.input, params)?;
        }
        LogicalOperator::ShortestPath(sp) => {
            substitute_in_operator(&mut sp.input, params)?;
        }
        // SPARQL Update operators
        LogicalOperator::InsertTriple(insert) => {
            if let Some(ref mut input) = insert.input {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::DeleteTriple(delete) => {
            if let Some(ref mut input) = delete.input {
                substitute_in_operator(input, params)?;
            }
        }
        LogicalOperator::Modify(modify) => {
            substitute_in_operator(&mut modify.where_clause, params)?;
        }
        LogicalOperator::ClearGraph(_)
        | LogicalOperator::CreateGraph(_)
        | LogicalOperator::DropGraph(_)
        | LogicalOperator::LoadGraph(_)
        | LogicalOperator::CopyGraph(_)
        | LogicalOperator::MoveGraph(_)
        | LogicalOperator::AddGraph(_) => {}
        LogicalOperator::HorizontalAggregate(op) => {
            substitute_in_operator(&mut op.input, params)?;
        }
        LogicalOperator::Empty => {}
        LogicalOperator::Except(except) => {
            substitute_in_operator(&mut except.left, params)?;
            substitute_in_operator(&mut except.right, params)?;
        }
        LogicalOperator::Intersect(intersect) => {
            substitute_in_operator(&mut intersect.left, params)?;
            substitute_in_operator(&mut intersect.right, params)?;
        }
        LogicalOperator::Otherwise(otherwise) => {
            substitute_in_operator(&mut otherwise.left, params)?;
            substitute_in_operator(&mut otherwise.right, params)?;
        }
        LogicalOperator::Apply(apply) => {
            substitute_in_operator(&mut apply.input, params)?;
            substitute_in_operator(&mut apply.subplan, params)?;
        }
        // ParameterScan has no expressions to substitute
        LogicalOperator::ParameterScan(_) => {}
        LogicalOperator::MultiWayJoin(mwj) => {
            for input in &mut mwj.inputs {
                substitute_in_operator(input, params)?;
            }
            for cond in &mut mwj.conditions {
                substitute_in_expression(&mut cond.left, params)?;
                substitute_in_expression(&mut cond.right, params)?;
            }
        }
        // DDL operators have no expressions to substitute
        LogicalOperator::CreatePropertyGraph(_) => {}
        // Procedure calls: les arguments portent des paramètres (`CALL
        // obrain.hybrid_search(…, $vec)`) ; les familles statiques sont
        // exécutées à la PLANIFICATION, donc la substitution doit avoir eu
        // lieu ici, avant le planificateur.
        LogicalOperator::CallProcedure(call) => {
            for arg in &mut call.arguments {
                substitute_in_expression(arg, params)?;
            }
        }
        // LoadData: file path is a literal, no parameter substitution needed
        LogicalOperator::LoadData(_) => {}
    }
    Ok(())
}

/// Resolves a `CountExpr::Parameter` by looking up the parameter value.
fn resolve_count_param(
    count: &mut crate::query::plan::CountExpr,
    params: &QueryParams,
) -> Result<()> {
    use crate::query::plan::CountExpr;
    use obrain_common::utils::error::{QueryError, QueryErrorKind};

    if let CountExpr::Parameter(name) = count {
        let value = params.get(name.as_str()).ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                format!("Missing parameter for SKIP/LIMIT: ${name}"),
            ))
        })?;
        let n = match value {
            Value::Int64(i) if *i >= 0 => *i as usize,
            Value::Int64(i) => {
                return Err(Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    format!("SKIP/LIMIT parameter ${name} must be non-negative, got {i}"),
                )));
            }
            other => {
                return Err(Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    format!("SKIP/LIMIT parameter ${name} must be an integer, got {other:?}"),
                )));
            }
        };
        *count = CountExpr::Literal(n);
    }
    Ok(())
}

/// Substitutes parameters in an expression with their values.
fn substitute_in_expression(expr: &mut LogicalExpression, params: &QueryParams) -> Result<()> {
    use crate::query::plan::LogicalExpression;

    match expr {
        LogicalExpression::Parameter(name) => {
            if let Some(value) = params.get(name) {
                *expr = LogicalExpression::Literal(value.clone());
            } else {
                return Err(Error::Internal(format!("Missing parameter: ${}", name)));
            }
        }
        LogicalExpression::Binary { left, right, .. } => {
            substitute_in_expression(left, params)?;
            substitute_in_expression(right, params)?;
        }
        LogicalExpression::Unary { operand, .. } => {
            substitute_in_expression(operand, params)?;
        }
        LogicalExpression::FunctionCall { args, .. } => {
            for arg in args {
                substitute_in_expression(arg, params)?;
            }
        }
        LogicalExpression::List(items) => {
            for item in items {
                substitute_in_expression(item, params)?;
            }
        }
        LogicalExpression::Map(pairs) => {
            for (_, value) in pairs {
                substitute_in_expression(value, params)?;
            }
        }
        LogicalExpression::IndexAccess { base, index } => {
            substitute_in_expression(base, params)?;
            substitute_in_expression(index, params)?;
        }
        LogicalExpression::SliceAccess { base, start, end } => {
            substitute_in_expression(base, params)?;
            if let Some(s) = start {
                substitute_in_expression(s, params)?;
            }
            if let Some(e) = end {
                substitute_in_expression(e, params)?;
            }
        }
        LogicalExpression::Case {
            operand,
            when_clauses,
            else_clause,
        } => {
            if let Some(op) = operand {
                substitute_in_expression(op, params)?;
            }
            for (cond, result) in when_clauses {
                substitute_in_expression(cond, params)?;
                substitute_in_expression(result, params)?;
            }
            if let Some(el) = else_clause {
                substitute_in_expression(el, params)?;
            }
        }
        LogicalExpression::Property { .. }
        | LogicalExpression::Variable(_)
        | LogicalExpression::Literal(_)
        | LogicalExpression::Labels(_)
        | LogicalExpression::Type(_)
        | LogicalExpression::Id(_) => {}
        LogicalExpression::ListComprehension {
            list_expr,
            filter_expr,
            map_expr,
            ..
        } => {
            substitute_in_expression(list_expr, params)?;
            if let Some(filter) = filter_expr {
                substitute_in_expression(filter, params)?;
            }
            substitute_in_expression(map_expr, params)?;
        }
        LogicalExpression::ListPredicate {
            list_expr,
            predicate,
            ..
        } => {
            substitute_in_expression(list_expr, params)?;
            substitute_in_expression(predicate, params)?;
        }
        // Sous-requêtes (`EXISTS { … }`, prédicat de motif `(n)-[:T]->(:L
        // {k: $p})`, `NOT (motif)`, `COUNT { … }`, sous-requête de valeur) :
        // leur plan porte ses propres filtres et cartes de propriétés, qui
        // peuvent citer un paramètre. Sans cette descente, le `$p` arrivait
        // tel quel au planificateur (« Parameters not yet supported in
        // filters », obrain-db/obrain#227 et #184).
        LogicalExpression::ExistsSubquery(sub)
        | LogicalExpression::CountSubquery(sub)
        | LogicalExpression::ValueSubquery(sub) => {
            substitute_in_operator(sub, params)?;
        }
        LogicalExpression::PatternComprehension {
            subplan,
            projection,
        } => {
            substitute_in_operator(subplan, params)?;
            substitute_in_expression(projection, params)?;
        }
        LogicalExpression::MapProjection { entries, .. } => {
            for entry in entries {
                if let crate::query::plan::MapProjectionEntry::LiteralEntry(_, expr) = entry {
                    substitute_in_expression(expr, params)?;
                }
            }
        }
        LogicalExpression::Reduce {
            initial,
            list,
            expression,
            ..
        } => {
            substitute_in_expression(initial, params)?;
            substitute_in_expression(list, params)?;
            substitute_in_expression(expression, params)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_substrate::SubstrateStore;

    /// Shared fixture builder — wraps a fresh substrate store into the trait
    /// object expected by [`QueryProcessor::for_lpg`].
    fn make_store() -> Arc<SubstrateStore> {
        Arc::new(SubstrateStore::open_tempfile().unwrap())
    }

    /// Ergonomic bridge over the (slice-of-owned-PropertyKey) trait signature:
    /// `add_node(&store, &["Label"], &[("key", value), ...])` replaces the old
    /// `LpgStore::create_node_with_props(labels, IntoIterator<(&str, Value)>)`
    /// convenience.
    fn add_node<S: GraphStoreMut + ?Sized>(
        store: &S,
        labels: &[&str],
        props: &[(&str, Value)],
    ) -> obrain_common::types::NodeId {
        let id = store.create_node(labels);
        for (k, v) in props {
            store.set_node_property(id, k, v.clone());
        }
        id
    }

    #[test]
    fn test_query_language_is_lpg() {
        assert!(QueryLanguage::Gql.is_lpg());
        #[cfg(feature = "cypher")]
        assert!(QueryLanguage::Cypher.is_lpg());
        #[cfg(feature = "sparql")]
        assert!(!QueryLanguage::Sparql.is_lpg());
    }

    #[test]
    fn test_processor_creation() {
        use obrain_core::graph::GraphStore;
        let store = make_store();
        // Keep a direct handle to assert invariants after wrapping it in the
        // processor — the `lpg_store()` getter was retired in T17 W3b.
        let store_for_assert = Arc::clone(&store);
        let _processor = QueryProcessor::for_lpg(store);
        assert_eq!(store_for_assert.node_count(), 0);
    }

    #[test]
    fn test_process_simple_gql() {
        let store = make_store();
        store.create_node(&["Person"]);
        store.create_node(&["Person"]);

        let processor = QueryProcessor::for_lpg(store);
        let result = processor
            .process("MATCH (n:Person) RETURN n", QueryLanguage::Gql, None)
            .unwrap();

        assert_eq!(result.row_count(), 2);
        assert_eq!(result.columns[0], "n");
    }

    #[cfg(feature = "cypher")]
    #[test]
    fn test_process_simple_cypher() {
        let store = make_store();
        store.create_node(&["Person"]);

        let processor = QueryProcessor::for_lpg(store);
        let result = processor
            .process("MATCH (n:Person) RETURN n", QueryLanguage::Cypher, None)
            .unwrap();

        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn test_process_with_params() {
        let store = make_store();
        add_node(&*store, &["Person"], &[("age", Value::Int64(25))]);
        add_node(&*store, &["Person"], &[("age", Value::Int64(35))]);
        add_node(&*store, &["Person"], &[("age", Value::Int64(45))]);

        let processor = QueryProcessor::for_lpg(store);

        // Query with parameter
        let mut params = HashMap::new();
        params.insert("min_age".to_string(), Value::Int64(30));

        let result = processor
            .process(
                "MATCH (n:Person) WHERE n.age > $min_age RETURN n",
                QueryLanguage::Gql,
                Some(&params),
            )
            .unwrap();

        // Should return 2 people (ages 35 and 45)
        assert_eq!(result.row_count(), 2);
    }

    #[test]
    fn test_missing_param_error() {
        let store = make_store();
        store.create_node(&["Person"]);

        let processor = QueryProcessor::for_lpg(store);

        // Query with parameter but empty params map (missing the required param)
        let params: HashMap<String, Value> = HashMap::new();
        let result = processor.process(
            "MATCH (n:Person) WHERE n.age > $min_age RETURN n",
            QueryLanguage::Gql,
            Some(&params),
        );

        // Should fail with missing parameter error
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Missing parameter"),
            "Expected 'Missing parameter' error, got: {}",
            err
        );
    }

    #[test]
    fn test_params_in_filter_with_property() {
        // Tests parameter substitution in WHERE clause with property comparison
        let store = make_store();
        add_node(&*store, &["Num"], &[("value", Value::Int64(10))]);
        add_node(&*store, &["Num"], &[("value", Value::Int64(20))]);

        let processor = QueryProcessor::for_lpg(store);

        let mut params = HashMap::new();
        params.insert("threshold".to_string(), Value::Int64(15));

        let result = processor
            .process(
                "MATCH (n:Num) WHERE n.value > $threshold RETURN n.value",
                QueryLanguage::Gql,
                Some(&params),
            )
            .unwrap();

        // Only value=20 matches > 15
        assert_eq!(result.row_count(), 1);
        let row = &result.rows[0];
        assert_eq!(row[0], Value::Int64(20));
    }

    #[test]
    fn test_params_in_multiple_where_conditions() {
        // Tests multiple parameters in WHERE clause with AND
        let store = make_store();
        add_node(
            &*store,
            &["Person"],
            &[("age", Value::Int64(25)), ("score", Value::Int64(80))],
        );
        add_node(
            &*store,
            &["Person"],
            &[("age", Value::Int64(35)), ("score", Value::Int64(90))],
        );
        add_node(
            &*store,
            &["Person"],
            &[("age", Value::Int64(45)), ("score", Value::Int64(70))],
        );

        let processor = QueryProcessor::for_lpg(store);

        let mut params = HashMap::new();
        params.insert("min_age".to_string(), Value::Int64(30));
        params.insert("min_score".to_string(), Value::Int64(75));

        let result = processor
            .process(
                "MATCH (n:Person) WHERE n.age > $min_age AND n.score > $min_score RETURN n",
                QueryLanguage::Gql,
                Some(&params),
            )
            .unwrap();

        // Only the person with age=35, score=90 matches both conditions
        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn test_params_with_in_list() {
        // Tests parameter as a value checked against IN list
        let store = make_store();
        add_node(
            &*store,
            &["Item"],
            &[("status", Value::String("active".into()))],
        );
        add_node(
            &*store,
            &["Item"],
            &[("status", Value::String("pending".into()))],
        );
        add_node(
            &*store,
            &["Item"],
            &[("status", Value::String("deleted".into()))],
        );

        let processor = QueryProcessor::for_lpg(store);

        // Check if a parameter value matches any of the statuses
        let mut params = HashMap::new();
        params.insert("target".to_string(), Value::String("active".into()));

        let result = processor
            .process(
                "MATCH (n:Item) WHERE n.status = $target RETURN n",
                QueryLanguage::Gql,
                Some(&params),
            )
            .unwrap();

        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn test_params_same_type_comparison() {
        // Tests that same-type parameter comparisons work correctly
        let store = make_store();
        add_node(&*store, &["Data"], &[("value", Value::Int64(100))]);
        add_node(&*store, &["Data"], &[("value", Value::Int64(50))]);

        let processor = QueryProcessor::for_lpg(store);

        // Compare int property with int parameter
        let mut params = HashMap::new();
        params.insert("threshold".to_string(), Value::Int64(75));

        let result = processor
            .process(
                "MATCH (n:Data) WHERE n.value > $threshold RETURN n",
                QueryLanguage::Gql,
                Some(&params),
            )
            .unwrap();

        // Only value=100 matches > 75
        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn test_process_empty_result_has_columns() {
        // Tests that empty results still have correct column names
        let store = make_store();
        // Don't create any nodes

        let processor = QueryProcessor::for_lpg(store);
        let result = processor
            .process(
                "MATCH (n:Person) RETURN n.name AS name, n.age AS age",
                QueryLanguage::Gql,
                None,
            )
            .unwrap();

        assert_eq!(result.row_count(), 0);
        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.columns[0], "name");
        assert_eq!(result.columns[1], "age");
    }

    #[test]
    fn test_params_string_equality() {
        // Tests string parameter equality comparison
        let store = make_store();
        add_node(
            &*store,
            &["Item"],
            &[("name", Value::String("alpha".into()))],
        );
        add_node(
            &*store,
            &["Item"],
            &[("name", Value::String("beta".into()))],
        );
        add_node(
            &*store,
            &["Item"],
            &[("name", Value::String("gamma".into()))],
        );

        let processor = QueryProcessor::for_lpg(store);

        let mut params = HashMap::new();
        params.insert("target".to_string(), Value::String("beta".into()));

        let result = processor
            .process(
                "MATCH (n:Item) WHERE n.name = $target RETURN n.name",
                QueryLanguage::Gql,
                Some(&params),
            )
            .unwrap();

        assert_eq!(result.row_count(), 1);
        assert_eq!(result.rows[0][0], Value::String("beta".into()));
    }
}

/// Tests de couverture des fonctions pures du processeur : substitution des
/// paramètres (`substitute_params`) sur chaque forme d'opérateur et
/// d'expression, résolution des paramètres de SKIP/LIMIT et ses refus,
/// indices de pushdown (`annotate_pushdown_hints`) et `explain_result`.
/// Aucune requête n'est analysée : les plans sont construits à la main.
#[cfg(test)]
mod tests_couverture_processor {
    #[allow(clippy::wildcard_imports)]
    use crate::query::plan::*;
    use obrain_core::execution::operators::AggregateSemantics;
    use obrain_substrate::SubstrateStore;

    use super::*;

    fn param(name: &str) -> LogicalExpression {
        LogicalExpression::Parameter(name.into())
    }

    fn var(name: &str) -> LogicalExpression {
        LogicalExpression::Variable(name.into())
    }

    fn int(n: i64) -> LogicalExpression {
        LogicalExpression::Literal(Value::Int64(n))
    }

    fn prop(variable: &str, property: &str) -> LogicalExpression {
        LogicalExpression::Property {
            variable: variable.into(),
            property: property.into(),
        }
    }

    fn params(pairs: &[(&str, Value)]) -> QueryParams {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn p_vaut_7() -> QueryParams {
        params(&[("p", Value::Int64(7))])
    }

    fn scan(variable: &str, label: Option<&str>) -> LogicalOperator {
        LogicalOperator::NodeScan(NodeScanOp {
            variable: variable.into(),
            label: label.map(str::to_string),
            input: None,
            pushdown_hint: None,
        })
    }

    fn boxed_scan(variable: &str) -> Box<LogicalOperator> {
        Box::new(scan(variable, None))
    }

    /// Un `Filter` dont le prédicat est le paramètre `$p` : l'opérateur témoin
    /// glissé sous chaque opérateur à tester, pour prouver la récursion.
    fn filtre_sur_p() -> Box<LogicalOperator> {
        Box::new(LogicalOperator::Filter(FilterOp {
            predicate: param("p"),
            input: boxed_scan("n"),
            pushdown_hint: None,
        }))
    }

    fn est_litteral_7(expr: &LogicalExpression) -> bool {
        matches!(expr, LogicalExpression::Literal(Value::Int64(7)))
    }

    /// Vrai si le témoin `filtre_sur_p` sous `op` a été substitué.
    fn temoin_substitue(op: &LogicalOperator) -> bool {
        matches!(op, LogicalOperator::Filter(f) if est_litteral_7(&f.predicate))
    }

    fn substitue(op: LogicalOperator) -> LogicalOperator {
        let mut plan = LogicalPlan::new(op);
        substitute_params(&mut plan, &p_vaut_7()).expect("substitution sans erreur");
        plan.root
    }

    fn substitue_expression(expr: LogicalExpression) -> LogicalExpression {
        let racine = substitue(LogicalOperator::Filter(FilterOp {
            predicate: expr,
            input: boxed_scan("n"),
            pushdown_hint: None,
        }));
        match racine {
            LogicalOperator::Filter(f) => f.predicate,
            autre => panic!("Filter attendu, obtenu {autre:?}"),
        }
    }

    // --- QueryLanguage ---

    #[test]
    fn query_language_is_lpg_par_variante() {
        assert!(QueryLanguage::Gql.is_lpg());
        #[cfg(feature = "cypher")]
        assert!(QueryLanguage::Cypher.is_lpg());
        #[cfg(feature = "gremlin")]
        assert!(QueryLanguage::Gremlin.is_lpg());
        #[cfg(feature = "graphql")]
        assert!(QueryLanguage::GraphQL.is_lpg());
        #[cfg(feature = "sql-pgq")]
        assert!(QueryLanguage::SqlPgq.is_lpg());
        #[cfg(feature = "sparql")]
        assert!(!QueryLanguage::Sparql.is_lpg());
        #[cfg(all(feature = "graphql", feature = "rdf"))]
        assert!(!QueryLanguage::GraphQLRdf.is_lpg());
    }

    // --- substitute_in_expression ---

    #[test]
    fn parametre_remplace_par_sa_valeur_et_manquant_refuse() {
        assert!(est_litteral_7(&substitue_expression(param("p"))));

        let mut plan = LogicalPlan::new(LogicalOperator::Filter(FilterOp {
            predicate: param("absent"),
            input: boxed_scan("n"),
            pushdown_hint: None,
        }));
        let err = substitute_params(&mut plan, &p_vaut_7()).expect_err("paramètre manquant");
        assert!(
            format!("{err:?}").contains("Missing parameter: $absent"),
            "{err:?}"
        );
    }

    #[test]
    fn expressions_feuilles_restent_intactes() {
        for feuille in [
            prop("n", "age"),
            var("n"),
            int(1),
            LogicalExpression::Labels("n".into()),
            LogicalExpression::Type("r".into()),
            LogicalExpression::Id("n".into()),
        ] {
            let avant = format!("{feuille:?}");
            let apres = substitue_expression(feuille);
            assert_eq!(format!("{apres:?}"), avant);
        }
    }

    #[test]
    fn substitution_dans_binaire_unaire_appel_liste_et_map() {
        let binaire = substitue_expression(LogicalExpression::Binary {
            left: Box::new(param("p")),
            op: BinaryOp::Add,
            right: Box::new(param("p")),
        });
        assert!(matches!(
            binaire,
            LogicalExpression::Binary { left, right, .. }
                if est_litteral_7(&left) && est_litteral_7(&right)
        ));

        let unaire = substitue_expression(LogicalExpression::Unary {
            op: UnaryOp::Neg,
            operand: Box::new(param("p")),
        });
        assert!(
            matches!(unaire, LogicalExpression::Unary { operand, .. } if est_litteral_7(&operand))
        );

        let appel = substitue_expression(LogicalExpression::FunctionCall {
            name: "abs".into(),
            args: vec![param("p"), int(1)],
            distinct: false,
        });
        assert!(
            matches!(appel, LogicalExpression::FunctionCall { args, .. } if est_litteral_7(&args[0]))
        );

        let liste = substitue_expression(LogicalExpression::List(vec![int(1), param("p")]));
        assert!(matches!(liste, LogicalExpression::List(items) if est_litteral_7(&items[1])));

        let map = substitue_expression(LogicalExpression::Map(vec![("k".into(), param("p"))]));
        assert!(matches!(map, LogicalExpression::Map(paires) if est_litteral_7(&paires[0].1)));
    }

    #[test]
    fn substitution_dans_index_tranche_et_case() {
        let index = substitue_expression(LogicalExpression::IndexAccess {
            base: Box::new(param("p")),
            index: Box::new(param("p")),
        });
        assert!(matches!(
            index,
            LogicalExpression::IndexAccess { base, index } if est_litteral_7(&base) && est_litteral_7(&index)
        ));

        let tranche = substitue_expression(LogicalExpression::SliceAccess {
            base: Box::new(var("l")),
            start: Some(Box::new(param("p"))),
            end: Some(Box::new(param("p"))),
        });
        assert!(matches!(
            tranche,
            LogicalExpression::SliceAccess { start: Some(s), end: Some(e), .. }
                if est_litteral_7(&s) && est_litteral_7(&e)
        ));
        let tranche_ouverte = substitue_expression(LogicalExpression::SliceAccess {
            base: Box::new(param("p")),
            start: None,
            end: None,
        });
        assert!(matches!(
            tranche_ouverte,
            LogicalExpression::SliceAccess { base, start: None, end: None } if est_litteral_7(&base)
        ));

        let case = substitue_expression(LogicalExpression::Case {
            operand: Some(Box::new(param("p"))),
            when_clauses: vec![(param("p"), param("p"))],
            else_clause: Some(Box::new(param("p"))),
        });
        assert!(matches!(
            case,
            LogicalExpression::Case { operand: Some(o), when_clauses, else_clause: Some(e) }
                if est_litteral_7(&o)
                    && est_litteral_7(&when_clauses[0].0)
                    && est_litteral_7(&when_clauses[0].1)
                    && est_litteral_7(&e)
        ));
        let case_nu = substitue_expression(LogicalExpression::Case {
            operand: None,
            when_clauses: vec![],
            else_clause: None,
        });
        assert!(matches!(
            case_nu,
            LogicalExpression::Case {
                operand: None,
                else_clause: None,
                ..
            }
        ));
    }

    #[test]
    fn substitution_dans_comprehensions_predicats_et_reduce() {
        let comprehension = substitue_expression(LogicalExpression::ListComprehension {
            variable: "x".into(),
            list_expr: Box::new(param("p")),
            filter_expr: Some(Box::new(param("p"))),
            map_expr: Box::new(param("p")),
        });
        assert!(matches!(
            comprehension,
            LogicalExpression::ListComprehension { list_expr, filter_expr: Some(f), map_expr, .. }
                if est_litteral_7(&list_expr) && est_litteral_7(&f) && est_litteral_7(&map_expr)
        ));
        let sans_filtre = substitue_expression(LogicalExpression::ListComprehension {
            variable: "x".into(),
            list_expr: Box::new(var("l")),
            filter_expr: None,
            map_expr: Box::new(param("p")),
        });
        assert!(matches!(
            sans_filtre,
            LogicalExpression::ListComprehension { filter_expr: None, map_expr, .. } if est_litteral_7(&map_expr)
        ));

        let predicat = substitue_expression(LogicalExpression::ListPredicate {
            kind: ListPredicateKind::Any,
            variable: "x".into(),
            list_expr: Box::new(param("p")),
            predicate: Box::new(param("p")),
        });
        assert!(matches!(
            predicat,
            LogicalExpression::ListPredicate { list_expr, predicate, .. }
                if est_litteral_7(&list_expr) && est_litteral_7(&predicate)
        ));

        let reduce = substitue_expression(LogicalExpression::Reduce {
            accumulator: "acc".into(),
            initial: Box::new(param("p")),
            variable: "x".into(),
            list: Box::new(param("p")),
            expression: Box::new(param("p")),
        });
        assert!(matches!(
            reduce,
            LogicalExpression::Reduce { initial, list, expression, .. }
                if est_litteral_7(&initial) && est_litteral_7(&list) && est_litteral_7(&expression)
        ));
    }

    #[test]
    fn substitution_dans_sous_requetes_motif_et_map_projection() {
        for sous_requete in [
            LogicalExpression::ExistsSubquery(filtre_sur_p()),
            LogicalExpression::CountSubquery(filtre_sur_p()),
            LogicalExpression::ValueSubquery(filtre_sur_p()),
        ] {
            let apres = substitue_expression(sous_requete);
            let sous_plan = match &apres {
                LogicalExpression::ExistsSubquery(s)
                | LogicalExpression::CountSubquery(s)
                | LogicalExpression::ValueSubquery(s) => s,
                autre => panic!("sous-requête attendue, obtenu {autre:?}"),
            };
            assert!(temoin_substitue(sous_plan), "{apres:?}");
        }

        let motif = substitue_expression(LogicalExpression::PatternComprehension {
            subplan: filtre_sur_p(),
            projection: Box::new(param("p")),
        });
        assert!(matches!(
            motif,
            LogicalExpression::PatternComprehension { subplan, projection }
                if temoin_substitue(&subplan) && est_litteral_7(&projection)
        ));

        let projection = substitue_expression(LogicalExpression::MapProjection {
            base: "n".into(),
            entries: vec![
                MapProjectionEntry::PropertySelector("name".into()),
                MapProjectionEntry::LiteralEntry("k".into(), param("p")),
                MapProjectionEntry::AllProperties,
            ],
        });
        assert!(matches!(
            projection,
            LogicalExpression::MapProjection { entries, .. }
                if matches!(&entries[1], MapProjectionEntry::LiteralEntry(_, e) if est_litteral_7(e))
        ));
    }

    // --- substitute_in_operator : récursion par forme d'opérateur ---

    #[test]
    fn operateurs_a_entree_unique_propagent_la_substitution() {
        let cas: Vec<LogicalOperator> = vec![
            LogicalOperator::Expand(ExpandOp {
                from_variable: "a".into(),
                to_variable: "b".into(),
                edge_variable: None,
                direction: ExpandDirection::Outgoing,
                edge_types: vec![],
                min_hops: 1,
                max_hops: Some(1),
                input: filtre_sur_p(),
                path_alias: None,
                path_mode: PathMode::Walk,
            }),
            LogicalOperator::Distinct(DistinctOp {
                input: filtre_sur_p(),
                columns: None,
            }),
            LogicalOperator::DeleteNode(DeleteNodeOp {
                variable: "n".into(),
                detach: false,
                input: filtre_sur_p(),
            }),
            LogicalOperator::DeleteEdge(DeleteEdgeOp {
                variable: "r".into(),
                input: filtre_sur_p(),
            }),
            LogicalOperator::MapCollect(MapCollectOp {
                key_var: "k".into(),
                value_var: "v".into(),
                alias: "m".into(),
                input: filtre_sur_p(),
            }),
            LogicalOperator::AddLabel(AddLabelOp {
                variable: "n".into(),
                labels: vec!["A".into()],
                input: filtre_sur_p(),
            }),
            LogicalOperator::RemoveLabel(RemoveLabelOp {
                variable: "n".into(),
                labels: vec!["A".into()],
                input: filtre_sur_p(),
            }),
            LogicalOperator::ShortestPath(ShortestPathOp {
                input: filtre_sur_p(),
                source_var: "a".into(),
                target_var: "b".into(),
                edge_types: vec![],
                direction: ExpandDirection::Both,
                path_alias: "p".into(),
                all_paths: false,
            }),
            LogicalOperator::CreateExternalEdge(CreateExternalEdgeOp {
                from_variable: "a".into(),
                edge_type: "T".into(),
                to_host: None,
                to_store: "s".into(),
                to_key: "k".into(),
                merge: false,
                input: filtre_sur_p(),
            }),
        ];
        for op in cas {
            let nom = op.display_label();
            let apres = substitue(op);
            let enfants = apres.children();
            assert_eq!(enfants.len(), 1, "{nom}");
            assert!(temoin_substitue(enfants[0]), "{nom}: {apres:?}");
        }
    }

    #[test]
    fn scans_avec_entree_optionnelle() {
        let noeud = substitue(LogicalOperator::NodeScan(NodeScanOp {
            variable: "m".into(),
            label: None,
            input: Some(filtre_sur_p()),
            pushdown_hint: None,
        }));
        assert!(temoin_substitue(noeud.children()[0]));

        let arete = substitue(LogicalOperator::EdgeScan(EdgeScanOp {
            variable: "r".into(),
            edge_types: vec![],
            input: Some(filtre_sur_p()),
        }));
        assert!(temoin_substitue(arete.children()[0]));

        let sans_entree = substitue(scan("n", None));
        assert_eq!(sans_entree.children().len(), 0);
    }

    #[test]
    fn return_project_sort_bind_unwind_substituent_leurs_expressions() {
        let retour = substitue(LogicalOperator::Return(ReturnOp {
            items: vec![ReturnItem {
                expression: param("p"),
                alias: None,
            }],
            distinct: false,
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &retour,
            LogicalOperator::Return(r) if est_litteral_7(&r.items[0].expression) && temoin_substitue(&r.input)
        ));

        let projection = substitue(LogicalOperator::Project(ProjectOp {
            projections: vec![Projection {
                expression: param("p"),
                alias: Some("x".into()),
            }],
            input: filtre_sur_p(),
            pass_through_input: false,
        }));
        assert!(matches!(
            &projection,
            LogicalOperator::Project(p) if est_litteral_7(&p.projections[0].expression) && temoin_substitue(&p.input)
        ));

        let tri = substitue(LogicalOperator::Sort(SortOp {
            keys: vec![SortKey {
                expression: param("p"),
                order: SortOrder::Ascending,
                nulls: None,
            }],
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &tri,
            LogicalOperator::Sort(s) if est_litteral_7(&s.keys[0].expression) && temoin_substitue(&s.input)
        ));

        let bind = substitue(LogicalOperator::Bind(BindOp {
            expression: param("p"),
            variable: "x".into(),
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &bind,
            LogicalOperator::Bind(b) if est_litteral_7(&b.expression) && temoin_substitue(&b.input)
        ));

        let unwind = substitue(LogicalOperator::Unwind(UnwindOp {
            expression: param("p"),
            variable: "x".into(),
            ordinality_var: None,
            offset_var: None,
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &unwind,
            LogicalOperator::Unwind(u) if est_litteral_7(&u.expression) && temoin_substitue(&u.input)
        ));
    }

    #[test]
    fn jointures_et_operateurs_ensemblistes() {
        let jointure = substitue(LogicalOperator::Join(JoinOp {
            left: filtre_sur_p(),
            right: filtre_sur_p(),
            join_type: JoinType::Inner,
            conditions: vec![JoinCondition {
                left: param("p"),
                right: param("p"),
            }],
        }));
        assert!(matches!(
            &jointure,
            LogicalOperator::Join(j)
                if temoin_substitue(&j.left)
                    && temoin_substitue(&j.right)
                    && est_litteral_7(&j.conditions[0].left)
                    && est_litteral_7(&j.conditions[0].right)
        ));

        let gauche = substitue(LogicalOperator::LeftJoin(LeftJoinOp {
            left: filtre_sur_p(),
            right: filtre_sur_p(),
            condition: Some(param("p")),
        }));
        assert!(matches!(
            &gauche,
            LogicalOperator::LeftJoin(j)
                if temoin_substitue(&j.left) && temoin_substitue(&j.right)
                    && matches!(&j.condition, Some(c) if est_litteral_7(c))
        ));
        let gauche_sans = substitue(LogicalOperator::LeftJoin(LeftJoinOp {
            left: filtre_sur_p(),
            right: boxed_scan("b"),
            condition: None,
        }));
        assert!(matches!(&gauche_sans, LogicalOperator::LeftJoin(j) if j.condition.is_none()));

        let deux_enfants: Vec<LogicalOperator> = vec![
            LogicalOperator::AntiJoin(AntiJoinOp {
                left: filtre_sur_p(),
                right: filtre_sur_p(),
            }),
            LogicalOperator::Except(ExceptOp {
                left: filtre_sur_p(),
                right: filtre_sur_p(),
                all: false,
            }),
            LogicalOperator::Intersect(IntersectOp {
                left: filtre_sur_p(),
                right: filtre_sur_p(),
                all: true,
            }),
            LogicalOperator::Otherwise(OtherwiseOp {
                left: filtre_sur_p(),
                right: filtre_sur_p(),
            }),
            LogicalOperator::Apply(ApplyOp {
                input: filtre_sur_p(),
                subplan: filtre_sur_p(),
                shared_variables: vec![],
                optional: false,
            }),
        ];
        for op in deux_enfants {
            let apres = substitue(op);
            let enfants = apres.children();
            assert_eq!(enfants.len(), 2, "{apres:?}");
            assert!(enfants.iter().all(|e| temoin_substitue(e)), "{apres:?}");
        }

        let union = substitue(LogicalOperator::Union(UnionOp {
            inputs: vec![*filtre_sur_p(), *filtre_sur_p(), *filtre_sur_p()],
        }));
        assert_eq!(union.children().len(), 3);
        assert!(union.children().iter().all(|e| temoin_substitue(e)));

        let multi = substitue(LogicalOperator::MultiWayJoin(MultiWayJoinOp {
            inputs: vec![*filtre_sur_p(), *filtre_sur_p()],
            conditions: vec![JoinCondition {
                left: param("p"),
                right: param("p"),
            }],
            shared_variables: vec![],
        }));
        assert!(matches!(
            &multi,
            LogicalOperator::MultiWayJoin(m)
                if m.inputs.iter().all(temoin_substitue)
                    && est_litteral_7(&m.conditions[0].left)
                    && est_litteral_7(&m.conditions[0].right)
        ));
    }

    #[test]
    fn agregat_et_appel_de_procedure() {
        let agregat = substitue(LogicalOperator::Aggregate(AggregateOp {
            group_by: vec![param("p")],
            aggregates: vec![
                AggregateExpr {
                    function: AggregateFunction::Sum,
                    expression: Some(param("p")),
                    expression2: None,
                    distinct: false,
                    alias: None,
                    percentile: None,
                    separator: None,
                },
                AggregateExpr {
                    function: AggregateFunction::Count,
                    expression: None,
                    expression2: None,
                    distinct: false,
                    alias: None,
                    percentile: None,
                    separator: None,
                },
            ],
            input: filtre_sur_p(),
            having: None,
            semantics: AggregateSemantics::default(),
        }));
        assert!(matches!(
            &agregat,
            LogicalOperator::Aggregate(a)
                if est_litteral_7(&a.group_by[0])
                    && matches!(&a.aggregates[0].expression, Some(e) if est_litteral_7(e))
                    && a.aggregates[1].expression.is_none()
                    && temoin_substitue(&a.input)
        ));

        let appel = substitue(LogicalOperator::CallProcedure(CallProcedureOp {
            name: vec!["obrain".into(), "proc".into()],
            arguments: vec![param("p"), int(1)],
            yield_items: None,
        }));
        assert!(matches!(
            &appel,
            LogicalOperator::CallProcedure(c) if est_litteral_7(&c.arguments[0])
        ));
    }

    #[test]
    fn mutations_substituent_leurs_proprietes() {
        let creation = substitue(LogicalOperator::CreateNode(CreateNodeOp {
            variable: "n".into(),
            labels: vec![],
            properties: vec![("age".into(), param("p"))],
            input: Some(filtre_sur_p()),
        }));
        assert!(matches!(
            &creation,
            LogicalOperator::CreateNode(c)
                if est_litteral_7(&c.properties[0].1) && matches!(&c.input, Some(i) if temoin_substitue(i))
        ));
        let creation_sans_entree = substitue(LogicalOperator::CreateNode(CreateNodeOp {
            variable: "n".into(),
            labels: vec![],
            properties: vec![],
            input: None,
        }));
        assert!(
            matches!(&creation_sans_entree, LogicalOperator::CreateNode(c) if c.input.is_none())
        );

        let arete = substitue(LogicalOperator::CreateEdge(CreateEdgeOp {
            variable: None,
            from_variable: "a".into(),
            to_variable: "b".into(),
            edge_type: "T".into(),
            properties: vec![("w".into(), param("p"))],
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &arete,
            LogicalOperator::CreateEdge(c) if est_litteral_7(&c.properties[0].1) && temoin_substitue(&c.input)
        ));

        let set = substitue(LogicalOperator::SetProperty(SetPropertyOp {
            variable: "n".into(),
            properties: vec![("age".into(), param("p"))],
            replace: false,
            is_edge: false,
            remove: false,
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &set,
            LogicalOperator::SetProperty(s) if est_litteral_7(&s.properties[0].1) && temoin_substitue(&s.input)
        ));

        let fusion = substitue(LogicalOperator::Merge(MergeOp {
            variable: "n".into(),
            labels: vec![],
            match_properties: vec![("a".into(), param("p"))],
            on_create: vec![("b".into(), param("p"))],
            on_match: vec![("c".into(), param("p"))],
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &fusion,
            LogicalOperator::Merge(m)
                if est_litteral_7(&m.match_properties[0].1)
                    && est_litteral_7(&m.on_create[0].1)
                    && est_litteral_7(&m.on_match[0].1)
                    && temoin_substitue(&m.input)
        ));

        let fusion_relation = substitue(LogicalOperator::MergeRelationship(MergeRelationshipOp {
            variable: "r".into(),
            source_variable: "a".into(),
            target_variable: "b".into(),
            edge_type: "T".into(),
            match_properties: vec![("a".into(), param("p"))],
            on_create: vec![("b".into(), param("p"))],
            on_match: vec![("c".into(), param("p"))],
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &fusion_relation,
            LogicalOperator::MergeRelationship(m)
                if est_litteral_7(&m.match_properties[0].1)
                    && est_litteral_7(&m.on_create[0].1)
                    && est_litteral_7(&m.on_match[0].1)
                    && temoin_substitue(&m.input)
        ));
    }

    #[test]
    fn feuilles_sans_expression_sont_acceptees() {
        let vide = substitue(LogicalOperator::Empty);
        assert!(matches!(vide, LogicalOperator::Empty));
        let parametres = substitue(LogicalOperator::ParameterScan(ParameterScanOp {
            columns: vec!["a".into()],
        }));
        assert!(matches!(parametres, LogicalOperator::ParameterScan(_)));
    }

    // --- resolve_count_param (SKIP / LIMIT) ---

    #[test]
    fn skip_limit_resolvent_un_parametre_entier() {
        let limite = substitue(LogicalOperator::Limit(LimitOp {
            count: CountExpr::Parameter("p".into()),
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &limite,
            LogicalOperator::Limit(l) if l.count == CountExpr::Literal(7) && temoin_substitue(&l.input)
        ));
        let saut = substitue(LogicalOperator::Skip(SkipOp {
            count: CountExpr::Parameter("p".into()),
            input: filtre_sur_p(),
        }));
        assert!(matches!(
            &saut,
            LogicalOperator::Skip(s) if s.count == CountExpr::Literal(7) && temoin_substitue(&s.input)
        ));
        let litteral = substitue(LogicalOperator::Limit(LimitOp {
            count: 3usize.into(),
            input: boxed_scan("n"),
        }));
        assert!(matches!(&litteral, LogicalOperator::Limit(l) if l.count == CountExpr::Literal(3)));
    }

    #[test]
    fn skip_limit_refusent_manquant_negatif_et_non_entier() {
        let cas: [(&str, QueryParams, &str); 3] = [
            (
                "absent",
                p_vaut_7(),
                "Missing parameter for SKIP/LIMIT: $absent",
            ),
            (
                "p",
                params(&[("p", Value::Int64(-1))]),
                "must be non-negative",
            ),
            (
                "p",
                params(&[("p", Value::Float64(1.5))]),
                "must be an integer",
            ),
        ];
        for (nom, parametres, message) in cas {
            let mut plan = LogicalPlan::new(LogicalOperator::Limit(LimitOp {
                count: CountExpr::Parameter(nom.into()),
                input: boxed_scan("n"),
            }));
            let err = substitute_params(&mut plan, &parametres).expect_err(message);
            assert!(format!("{err:?}").contains(message), "{err:?}");
        }
    }

    // --- annotate_pushdown_hints ---

    fn store_temporaire(index: &[&str]) -> SubstrateStore {
        let store = SubstrateStore::open_tempfile().expect("store temporaire");
        for propriete in index {
            store.create_property_index(propriete);
        }
        store
    }

    fn filtre_sur_scan(predicate: LogicalExpression, label: Option<&str>) -> LogicalOperator {
        LogicalOperator::Filter(FilterOp {
            predicate,
            input: Box::new(scan("n", label)),
            pushdown_hint: None,
        })
    }

    fn indice_du_filtre(op: &LogicalOperator) -> Option<&PushdownHint> {
        match op {
            LogicalOperator::Filter(f) => f.pushdown_hint.as_ref(),
            autre => panic!("Filter attendu, obtenu {autre:?}"),
        }
    }

    fn comparaison(op: BinaryOp, variable: &str) -> LogicalExpression {
        LogicalExpression::Binary {
            left: Box::new(prop(variable, "age")),
            op,
            right: Box::new(int(30)),
        }
    }

    #[test]
    fn pushdown_egalite_index_label_ou_rien() {
        let avec_index = store_temporaire(&["age"]);
        let mut op = filtre_sur_scan(comparaison(BinaryOp::Eq, "n"), Some("Person"));
        annotate_pushdown_hints(&mut op, &avec_index);
        assert!(
            matches!(indice_du_filtre(&op), Some(PushdownHint::IndexLookup { property }) if property == "age")
        );

        let sans_index = store_temporaire(&[]);
        let mut op = filtre_sur_scan(comparaison(BinaryOp::Eq, "n"), Some("Person"));
        annotate_pushdown_hints(&mut op, &sans_index);
        assert!(matches!(
            indice_du_filtre(&op),
            Some(PushdownHint::LabelFirst)
        ));

        let mut op = filtre_sur_scan(comparaison(BinaryOp::Eq, "n"), None);
        annotate_pushdown_hints(&mut op, &sans_index);
        assert!(indice_du_filtre(&op).is_none());

        // La propriété comparée porte sur une AUTRE variable : pas de pushdown
        // par la propriété, seulement le label.
        let mut op = filtre_sur_scan(comparaison(BinaryOp::Eq, "m"), Some("Person"));
        annotate_pushdown_hints(&mut op, &avec_index);
        assert!(indice_du_filtre(&op).is_none());
    }

    #[test]
    fn pushdown_intervalle_conjonction_et_autre_predicat() {
        let avec_index = store_temporaire(&["age"]);
        for op_cmp in [BinaryOp::Gt, BinaryOp::Ge, BinaryOp::Lt, BinaryOp::Le] {
            let mut op = filtre_sur_scan(comparaison(op_cmp, "n"), None);
            annotate_pushdown_hints(&mut op, &avec_index);
            assert!(
                matches!(indice_du_filtre(&op), Some(PushdownHint::RangeScan { property }) if property == "age"),
                "{op_cmp:?}"
            );
        }
        let sans_index = store_temporaire(&[]);
        let mut op = filtre_sur_scan(comparaison(BinaryOp::Gt, "n"), Some("Person"));
        annotate_pushdown_hints(&mut op, &sans_index);
        assert!(matches!(
            indice_du_filtre(&op),
            Some(PushdownHint::LabelFirst)
        ));
        let mut op = filtre_sur_scan(comparaison(BinaryOp::Gt, "n"), None);
        annotate_pushdown_hints(&mut op, &sans_index);
        assert!(indice_du_filtre(&op).is_none());

        // AND : seul le membre gauche est regardé.
        let conjonction = LogicalExpression::Binary {
            left: Box::new(comparaison(BinaryOp::Eq, "n")),
            op: BinaryOp::And,
            right: Box::new(var("x")),
        };
        let mut op = filtre_sur_scan(conjonction, None);
        annotate_pushdown_hints(&mut op, &avec_index);
        assert!(matches!(
            indice_du_filtre(&op),
            Some(PushdownHint::IndexLookup { .. })
        ));

        // Autre prédicat : le label seul décide.
        let mut op = filtre_sur_scan(var("x"), Some("Person"));
        annotate_pushdown_hints(&mut op, &sans_index);
        assert!(matches!(
            indice_du_filtre(&op),
            Some(PushdownHint::LabelFirst)
        ));
        let mut op = filtre_sur_scan(var("x"), None);
        annotate_pushdown_hints(&mut op, &sans_index);
        assert!(indice_du_filtre(&op).is_none());
    }

    #[test]
    fn pushdown_limit_pose_un_plafond_sur_le_scan_etiquete() {
        let store = store_temporaire(&[]);
        let mut op = LogicalOperator::Limit(LimitOp {
            count: 10usize.into(),
            input: Box::new(scan("n", Some("Person"))),
        });
        annotate_pushdown_hints(&mut op, &store);
        let LogicalOperator::Limit(l) = &op else {
            panic!("Limit")
        };
        assert!(matches!(
            l.input.as_ref(),
            LogicalOperator::NodeScan(s) if matches!(s.pushdown_hint, Some(PushdownHint::SourceCap { count: 160 }))
        ));

        // LIMIT 0 : plafond d'au moins 1.
        let mut op = LogicalOperator::Limit(LimitOp {
            count: 0usize.into(),
            input: Box::new(scan("n", Some("Person"))),
        });
        annotate_pushdown_hints(&mut op, &store);
        let LogicalOperator::Limit(l) = &op else {
            panic!("Limit")
        };
        assert!(matches!(
            l.input.as_ref(),
            LogicalOperator::NodeScan(s) if matches!(s.pushdown_hint, Some(PushdownHint::SourceCap { count: 1 }))
        ));

        // Sans label, ou sans LIMIT au-dessus : pas de plafond.
        let mut op = LogicalOperator::Limit(LimitOp {
            count: 10usize.into(),
            input: Box::new(scan("n", None)),
        });
        annotate_pushdown_hints(&mut op, &store);
        let LogicalOperator::Limit(l) = &op else {
            panic!("Limit")
        };
        assert!(
            matches!(l.input.as_ref(), LogicalOperator::NodeScan(s) if s.pushdown_hint.is_none())
        );
        let mut op = scan("n", Some("Person"));
        annotate_pushdown_hints(&mut op, &store);
        assert!(matches!(&op, LogicalOperator::NodeScan(s) if s.pushdown_hint.is_none()));
    }

    #[test]
    fn pushdown_traverse_les_operateurs_intermediaires() {
        let store = store_temporaire(&["age"]);
        let filtre_indexe = || Box::new(filtre_sur_scan(comparaison(BinaryOp::Eq, "n"), None));
        let a_indice = |op: &LogicalOperator| {
            matches!(indice_du_filtre(op), Some(PushdownHint::IndexLookup { .. }))
        };

        let a_un_enfant: Vec<LogicalOperator> = vec![
            LogicalOperator::Project(ProjectOp {
                projections: vec![],
                input: filtre_indexe(),
                pass_through_input: false,
            }),
            LogicalOperator::Aggregate(AggregateOp {
                group_by: vec![],
                aggregates: vec![],
                input: filtre_indexe(),
                having: None,
                semantics: AggregateSemantics::default(),
            }),
            LogicalOperator::Skip(SkipOp {
                count: 1usize.into(),
                input: filtre_indexe(),
            }),
            LogicalOperator::Sort(SortOp {
                keys: vec![],
                input: filtre_indexe(),
            }),
            LogicalOperator::Distinct(DistinctOp {
                input: filtre_indexe(),
                columns: None,
            }),
            LogicalOperator::Return(ReturnOp {
                items: vec![],
                distinct: false,
                input: filtre_indexe(),
            }),
            LogicalOperator::Expand(ExpandOp {
                from_variable: "n".into(),
                to_variable: "m".into(),
                edge_variable: None,
                direction: ExpandDirection::Outgoing,
                edge_types: vec![],
                min_hops: 1,
                max_hops: Some(1),
                input: filtre_indexe(),
                path_alias: None,
                path_mode: PathMode::Walk,
            }),
            LogicalOperator::NodeScan(NodeScanOp {
                variable: "m".into(),
                label: None,
                input: Some(filtre_indexe()),
                pushdown_hint: None,
            }),
            LogicalOperator::EdgeScan(EdgeScanOp {
                variable: "r".into(),
                edge_types: vec![],
                input: Some(filtre_indexe()),
            }),
        ];
        for mut op in a_un_enfant {
            annotate_pushdown_hints(&mut op, &store);
            let enfants = op.children();
            assert_eq!(enfants.len(), 1);
            assert!(a_indice(enfants[0]), "{op:?}");
        }

        let a_deux_enfants: Vec<LogicalOperator> = vec![
            LogicalOperator::Join(JoinOp {
                left: filtre_indexe(),
                right: filtre_indexe(),
                join_type: JoinType::Inner,
                conditions: vec![],
            }),
            LogicalOperator::Apply(ApplyOp {
                input: filtre_indexe(),
                subplan: filtre_indexe(),
                shared_variables: vec![],
                optional: false,
            }),
            LogicalOperator::Otherwise(OtherwiseOp {
                left: filtre_indexe(),
                right: filtre_indexe(),
            }),
        ];
        for mut op in a_deux_enfants {
            annotate_pushdown_hints(&mut op, &store);
            assert!(op.children().iter().all(|e| a_indice(e)), "{op:?}");
        }

        let mut union = LogicalOperator::Union(UnionOp {
            inputs: vec![*filtre_indexe(), *filtre_indexe()],
        });
        annotate_pushdown_hints(&mut union, &store);
        assert!(union.children().iter().all(|e| a_indice(e)));

        // Opérateur sans règle : rien ne change, rien ne casse.
        let mut creation = LogicalOperator::CreateNode(CreateNodeOp {
            variable: "n".into(),
            labels: vec![],
            properties: vec![],
            input: Some(filtre_indexe()),
        });
        annotate_pushdown_hints(&mut creation, &store);
        assert!(indice_du_filtre(creation.children()[0]).is_none());
    }

    // --- explain_result ---

    #[test]
    fn explain_result_rend_l_arbre_dans_une_colonne_plan() {
        let plan = LogicalPlan::explain(scan("n", Some("Person")));
        let resultat = explain_result(&plan);
        assert_eq!(resultat.columns, vec!["plan".to_string()]);
        assert_eq!(
            resultat.column_types,
            vec![obrain_common::types::LogicalType::String]
        );
        assert_eq!(resultat.rows.len(), 1);
        assert!(
            matches!(&resultat.rows[0][0], Value::String(s) if s.as_str() == "NodeScan (n:Person)\n")
        );
        assert!(resultat.execution_time_ms.is_none());
        assert!(resultat.rows_scanned.is_none());
        assert!(resultat.status_message.is_none());
        assert_eq!(
            resultat.gql_status,
            obrain_common::utils::GqlStatus::SUCCESS
        );
    }
}
