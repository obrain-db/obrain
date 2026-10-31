//! Scan operator for reading data from storage.

use super::{Operator, OperatorResult};
use crate::execution::DataChunk;
use crate::graph::GraphStore;
use obrain_common::types::{EpochId, LogicalType, NodeId, TransactionId};
use std::sync::Arc;

/// A scan operator that reads nodes from storage.
pub struct ScanOperator {
    /// The store to scan from.
    store: Arc<dyn GraphStore>,
    /// Label filter (None = all nodes).
    label: Option<String>,
    /// Current position in the scan.
    position: usize,
    /// Batch of node IDs to scan.
    batch: Vec<NodeId>,
    /// Whether the scan is exhausted.
    exhausted: bool,
    /// Chunk capacity.
    chunk_capacity: usize,
    /// Optional row cap. When set, the scan uses the LIMIT-aware
    /// `nodes_by_label_capped` variant which stops as soon as `max_rows`
    /// matches are collected. Set by `plan_limit` when the parent
    /// operator is a simple `Limit(N) → NodeScan(label)` (no Sort, no
    /// Filter in between).
    max_rows: Option<usize>,
    /// Transaction ID for MVCC visibility (None = use current epoch).
    transaction_id: Option<TransactionId>,
    /// Epoch for version visibility.
    viewing_epoch: Option<EpochId>,
}

impl ScanOperator {
    /// Creates a new scan operator for all nodes.
    pub fn new(store: Arc<dyn GraphStore>) -> Self {
        Self {
            store,
            label: None,
            position: 0,
            batch: Vec::new(),
            exhausted: false,
            chunk_capacity: 2048,
            max_rows: None,
            transaction_id: None,
            viewing_epoch: None,
        }
    }

    /// Creates a new scan operator for nodes with a specific label.
    pub fn with_label(store: Arc<dyn GraphStore>, label: impl Into<String>) -> Self {
        Self {
            store,
            label: Some(label.into()),
            position: 0,
            batch: Vec::new(),
            exhausted: false,
            chunk_capacity: 2048,
            max_rows: None,
            transaction_id: None,
            viewing_epoch: None,
        }
    }

    /// Sets an explicit row cap. Lets the underlying store stop scanning
    /// as soon as `max` matches are found (via `nodes_by_label_capped`).
    /// Use this when the plan knows a LIMIT is coming and there's no Sort
    /// or Filter that would need the full result set.
    pub fn with_max_rows(mut self, max: usize) -> Self {
        self.max_rows = Some(max);
        self
    }

    /// Sets the chunk capacity.
    pub fn with_chunk_capacity(mut self, capacity: usize) -> Self {
        self.chunk_capacity = capacity;
        self
    }

    /// Sets the transaction context for MVCC visibility.
    ///
    /// When set, the scan will only return nodes visible to this transaction.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    fn load_batch(&mut self) {
        if !self.batch.is_empty() || self.exhausted {
            return;
        }

        // Get nodes. When we have transaction context, use all_node_ids()
        // to include uncommitted/PENDING versions (nodes_by_label already
        // returns unfiltered IDs from the label index, but node_ids()
        // pre-filters by epoch which excludes PENDING nodes).
        //
        // LIMIT-aware fast path : when self.max_rows is set AND there's
        // no transaction context (no MVCC filter that could drop rows),
        // ask the store for AT MOST that many node IDs. Saves an O(N)
        // scan on big labels (681K ChatEvents → microseconds).
        let all_ids = match &self.label {
            Some(label) => {
                // Use the capped variant when we have a row cap AND there is
                // no active transaction. `viewing_epoch=Some(EpochId(0))` is
                // the read-only baseline — visibility filtering on baseline
                // nodes is a no-op (TOMBSTONED is already filtered inside
                // nodes_by_label_capped), so the cap is safe.
                //
                // Only fall back to the full scan when `transaction_id` is
                // Some, which is the case for queries inside an explicit
                // MVCC transaction where visibility might drop matches.
                if let Some(max) = self.max_rows {
                    if self.transaction_id.is_none() {
                        self.store.nodes_by_label_capped(label, max)
                    } else {
                        self.store.nodes_by_label(label)
                    }
                } else {
                    self.store.nodes_by_label(label)
                }
            }
            // ── PLAFOND SANS ETIQUETTE (2026-09-11) ───────────────────
            //
            // C'est le chemin du `MATCH (n) ... LIMIT 30` de production. Il
            // materialisait la TOTALITE des slots vivants avant la premiere
            // ligne : le trait n'exposant aucun iterateur, `node_ids()` rend
            // un `Vec` complet ou rien.
            //
            // TROIS CONDITIONS, et les trois sont necessaires :
            //
            //  1. `max_rows` est pose. Le planificateur ne le pose que pour
            //     une forme dont il a prouve qu'elle PRESERVE LES LIGNES
            //     entre le scan et le LIMIT — pas de Filter, pas de Sort,
            //     pas de Join, pas d'Aggregate, pas de DISTINCT. Un filtre
            //     en aval exigerait de lire JUSQU'A ce que k lignes passent,
            //     pas de lire k slots ; c'est la faute la plus facile a
            //     commettre ici et la plus difficile a voir, parce qu'elle
            //     ne se manifeste que sur les jeux de donnees ou le filtre
            //     rejette beaucoup.
            //
            //  2. Aucune transaction. Avec un `transaction_id`, le balayage
            //     doit passer par `all_node_ids()` — qui inclut les versions
            //     EN ATTENTE — puis filtrer ; le filtre jette alors des
            //     lignes, et un plafond pose avant lui en rendrait moins que
            //     k.
            //
            //  3. Le store declare son filtre de visibilite TRIVIAL. Sans
            //     transaction mais avec des cartes MVCC peuplees, le filtre
            //     peut encore jeter. Le predicat repond a la seule question
            //     qui compte : `filter_visible_node_ids` peut-il retirer
            //     quelque chose ? S'il le peut, on ne plafonne pas.
            //
            // Hors de ces trois, le comportement est celui d'avant, au
            // caractere pres — et un scan SANS `LIMIT` (`max_rows == None`)
            // voit donc toujours tous les noeuds.
            None if self.max_rows.is_some()
                && self.transaction_id.is_none()
                && self.store.scan_visibility_is_trivial() =>
            {
                // `expect` impossible a declencher : garde `is_some()` juste
                // au-dessus.
                let max = self
                    .max_rows
                    .expect("max_rows is_some : teste par la garde du bras");
                self.store.node_ids_capped(max)
            }
            None if self.viewing_epoch.is_some() => self.store.all_node_ids(),
            None => self.store.node_ids(),
        };

        // Filter by visibility if we have tx context.
        // Uses batch methods that hold a single lock for all IDs instead of
        // acquiring/releasing per node (avoids N+1 lock pattern).
        self.batch = if let Some(epoch) = self.viewing_epoch {
            if let Some(tx) = self.transaction_id {
                self.store
                    .filter_visible_node_ids_versioned(&all_ids, epoch, tx)
            } else {
                self.store.filter_visible_node_ids(&all_ids, epoch)
            }
        } else {
            all_ids
        };

        if self.batch.is_empty() {
            self.exhausted = true;
        }
    }
}

impl Operator for ScanOperator {
    fn next(&mut self) -> OperatorResult {
        // ── ANNULATION : LE POINT LE PLUS PROFOND (2026-08-24) ─────────
        //
        // `load_batch` descend dans `store.nodes_by_label()` /
        // `store.node_ids()`, qui MATERIALISENT un `Vec<NodeId>` en
        // parcourant la totalite des slots du substrat. C'est le seul appel
        // du plan qui puisse durer des minutes sans jamais rendre la main :
        // 1,35 million de slots sur la base `po`, et le trait `GraphStore`
        // n'expose aucun iterateur (choix delibere d'object-safety) — tout
        // est materialise avant que le premier chunk n'existe.
        //
        // DEUX controles, et les deux comptent :
        //
        //   AVANT — evite d'ENGAGER un balayage integral pour une requete
        //   deja condamnee (le cas exact d'une requete relancee alors que
        //   la precedente vient d'expirer).
        //
        //   APRES — c'est la GARANTIE DE CORRECTION du mecanisme. Les scans
        //   du substrat s'arretent sur annulation en rendant le `Vec`
        //   PARTIEL deja rempli : ils n'ont aucun canal pour dire « je n'ai
        //   pas fini », leur signature rendant un `Vec` nu. Ce partiel
        //   ressemble a un resultat legitimement court. Le controle
        //   ci-dessous le rattrape SANS FAUTE parce que le jeton est
        //   MONOTONE : arme une fois, il ne se desarme jamais dans la
        //   portee de la requete. Si le balayage a ete tronque, le jeton
        //   etait arme pendant ; il l'est donc encore ici. Aucune ligne
        //   tronquee ne peut sortir sous couvert de resultat complet.
        super::verifier_annulation()?;
        self.load_batch();
        super::verifier_annulation()?;

        if self.exhausted || self.position >= self.batch.len() {
            return Ok(None);
        }

        // Create output chunk with node IDs
        let schema = [LogicalType::Node];
        let mut chunk = DataChunk::with_capacity(&schema, self.chunk_capacity);

        let end = (self.position + self.chunk_capacity).min(self.batch.len());
        let count = end - self.position;

        {
            // Column 0 guaranteed to exist: chunk created with single-column schema above
            let col = chunk
                .column_mut(0)
                .expect("column 0 exists: chunk created with single-column schema");
            for i in self.position..end {
                col.push_node_id(self.batch[i]);
            }
        }

        chunk.set_count(count);
        self.position = end;

        Ok(Some(chunk))
    }

    fn reset(&mut self) {
        self.position = 0;
        self.batch.clear();
        self.exhausted = false;
    }

    fn name(&self) -> &'static str {
        "Scan"
    }
}

// Tests relocated to `crates/obrain-substrate/tests/operators_scan.rs`
// (T17 Step 3 W2 Class-2 migration — decision `b1dfe229`). obrain-core cannot
// take obrain-substrate as a dev-dep (dev-dep cycle, gotcha `598dda40`), so
// the LPG-era fixtures are rebuilt against SubstrateStore in an integration
// test of obrain-substrate.
#[cfg(test)]
mod tests {}
