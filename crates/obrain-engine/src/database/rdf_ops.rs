//! RDF-specific operations for ObrainDB.
//!
//! This module consolidates all RDF functionality that was previously scattered
//! across `query.rs`, `crud.rs`, `admin.rs`, and `mod.rs`. The entire module
//! is gated behind `#[cfg(feature = "rdf")]` in the parent.

#[cfg(all(feature = "rdf", feature = "wal"))]
use std::path::Path;
use std::sync::Arc;

#[cfg(all(feature = "rdf", feature = "wal"))]
use obrain_adapters::storage::wal::WalRecord;
use obrain_common::utils::error::Result;
use obrain_core::graph::rdf::RdfStore;

use super::ObrainDB;

// =========================================================================
// Query operations
// =========================================================================

impl ObrainDB {
    /// Executes a SPARQL query and returns the result.
    ///
    /// SPARQL queries operate on the RDF triple store.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let result = db.execute_sparql("SELECT ?s ?p ?o WHERE { ?s ?p ?o }")?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "sparql")]
    pub fn execute_sparql(&self, query: &str) -> Result<super::QueryResult> {
        use crate::query::{
            Executor, optimizer::Optimizer, planner::rdf::RdfPlanner, translators::sparql,
        };

        // Parse and translate the SPARQL query to a logical plan
        let logical_plan = sparql::translate(query)?;

        // Optimize the plan using RDF-specific statistics
        let rdf_stats = self.rdf_store.collect_statistics();
        let optimizer = Optimizer::from_rdf_statistics(rdf_stats);
        let optimized_plan = optimizer.optimize(logical_plan)?;

        // EXPLAIN: return the logical plan tree without executing
        if optimized_plan.explain {
            use crate::query::processor::explain_result;
            return Ok(explain_result(&optimized_plan));
        }

        // Convert to physical plan using RDF planner
        let planner = RdfPlanner::new(Arc::clone(&self.rdf_store));
        #[cfg(all(feature = "wal", feature = "rdf"))]
        let planner = planner.with_wal(self.rdf_wal.clone());
        let mut physical_plan = planner.plan(&optimized_plan)?;

        // Execute the plan
        let executor = Executor::with_columns(physical_plan.columns.clone());
        executor.execute(physical_plan.operator.as_mut())
    }

    /// Returns the RDF store.
    ///
    /// This provides direct access to the RDF store for triple operations.
    #[must_use]
    pub fn rdf_store(&self) -> &Arc<RdfStore> {
        &self.rdf_store
    }
}

// =========================================================================
// CRUD operations
// =========================================================================

impl ObrainDB {
    /// Batch-inserts RDF triples into the RDF store.
    ///
    /// Delegates to `RdfStore::batch_insert`, which acquires each index lock
    /// once for the entire batch. Duplicates are silently skipped.
    ///
    /// Returns the number of triples that were newly inserted.
    pub fn batch_insert_rdf(
        &self,
        triples: impl IntoIterator<Item = obrain_core::graph::rdf::Triple>,
    ) -> usize {
        let triples: Vec<_> = triples.into_iter().collect();
        #[cfg(all(feature = "wal", feature = "rdf"))]
        if let Some(wal) = &self.rdf_wal {
            for t in &triples {
                journaliser_rdf(
                    wal,
                    &WalRecord::InsertRdfTriple {
                        subject: t.subject().to_string(),
                        predicate: t.predicate().to_string(),
                        object: t.object().to_string(),
                        graph: None,
                    },
                );
            }
        }
        self.rdf_store.batch_insert(triples)
    }
}

// =========================================================================
// Admin operations
// =========================================================================

impl ObrainDB {
    /// Returns RDF schema information.
    ///
    /// Only available when the RDF feature is enabled.
    #[must_use]
    pub fn rdf_schema(&self) -> crate::admin::SchemaInfo {
        let stats = self.rdf_store.stats();

        let predicates = self
            .rdf_store
            .predicates()
            .into_iter()
            .map(|predicate| {
                let count = self.rdf_store.triples_with_predicate(&predicate).len();
                crate::admin::PredicateInfo {
                    iri: predicate.to_string(),
                    count,
                }
            })
            .collect();

        crate::admin::SchemaInfo::Rdf(crate::admin::RdfSchemaInfo {
            predicates,
            named_graphs: Vec::new(),
            subject_count: stats.subject_count,
            object_count: stats.object_count,
        })
    }
}

// =========================================================================
// RDF journal (WAL) — persistence of triples on the substrate backend
// =========================================================================

/// Name of the directory, inside a database path, that holds the RDF journal.
///
/// The substrate has its own WAL for the LPG graph; the RDF store is a
/// separate in-memory structure, so its mutations go to this dedicated WAL
/// and are replayed on open.
#[cfg(all(feature = "rdf", feature = "wal"))]
pub(crate) const RDF_JOURNAL_DIR: &str = "rdf_wal";

/// Appends one RDF record to the journal. A failure is logged, not propagated:
/// the in-memory mutation has already happened, matching the LPG behaviour of
/// `log_schema_wal`.
#[cfg(all(feature = "rdf", feature = "wal"))]
pub(crate) fn journaliser_rdf(
    wal: &Arc<obrain_adapters::storage::wal::LpgWal>,
    record: &WalRecord,
) {
    if let Err(err) = wal.log(record) {
        tracing::warn!("RDF WAL log failed: {err}");
    }
}

impl ObrainDB {
    /// Opens the RDF journal of a persistent database and replays it into the
    /// (freshly created, empty) RDF store.
    ///
    /// Replay is idempotent for every record type, so a journal that was
    /// partially applied before a crash is safe to replay again. A writable
    /// open also attaches the journal for appending; a read-only open never
    /// creates the directory.
    #[cfg(all(feature = "rdf", feature = "wal"))]
    pub(super) fn ouvrir_journal_rdf(&mut self, base: &Path, read_only: bool) -> Result<()> {
        use obrain_adapters::storage::wal::{WalConfig, WalRecovery};

        let dir = base.join(RDF_JOURNAL_DIR);
        for record in WalRecovery::new(&dir).recover()? {
            replay_rdf_wal_record(&self.rdf_store, &record);
        }
        if !read_only {
            let wal =
                obrain_adapters::storage::wal::LpgWal::with_config(&dir, WalConfig::default())?;
            self.rdf_wal = Some(Arc::new(wal));
        }
        Ok(())
    }
}

/// Replays a single RDF WAL record into the RDF store.
///
/// Records that are not RDF records are ignored.
#[cfg(all(feature = "rdf", feature = "wal"))]
pub(super) fn replay_rdf_wal_record(
    rdf_store: &Arc<RdfStore>,
    record: &obrain_adapters::storage::wal::WalRecord,
) {
    use obrain_adapters::storage::wal::WalRecord;
    use obrain_core::graph::rdf::Term;

    match record {
        WalRecord::InsertRdfTriple {
            subject,
            predicate,
            object,
            graph,
        } => {
            if let (Some(s), Some(p), Some(o)) = (
                Term::from_ntriples(subject),
                Term::from_ntriples(predicate),
                Term::from_ntriples(object),
            ) {
                let triple = obrain_core::graph::rdf::Triple::new(s, p, o);
                let target = match graph {
                    Some(name) => rdf_store.graph_or_create(name),
                    None => Arc::clone(rdf_store),
                };
                target.insert(triple);
            }
        }
        WalRecord::DeleteRdfTriple {
            subject,
            predicate,
            object,
            graph,
        } => {
            if let (Some(s), Some(p), Some(o)) = (
                Term::from_ntriples(subject),
                Term::from_ntriples(predicate),
                Term::from_ntriples(object),
            ) {
                let triple = obrain_core::graph::rdf::Triple::new(s, p, o);
                let target = match graph {
                    Some(name) => rdf_store.graph_or_create(name),
                    None => Arc::clone(rdf_store),
                };
                target.remove(&triple);
            }
        }
        WalRecord::ClearRdfGraph { graph } => {
            rdf_store.clear_graph(graph.as_deref());
        }
        WalRecord::CreateRdfGraph { name } => {
            let _ = rdf_store.create_graph(name);
        }
        WalRecord::DropRdfGraph { name } => match name {
            None => rdf_store.clear(),
            Some(graph_name) => {
                rdf_store.drop_graph(graph_name);
            }
        },
        _ => {}
    }
}
