//! Expand operator for relationship traversal.

use super::{Operator, OperatorError, OperatorResult, SessionContext};
use crate::execution::DataChunk;
use crate::execution::base_registry::{
    self, BaseRegistry, external_edge_value, remote_edge_value, remote_node_value, resolve_node_key,
};
use crate::graph::Direction;
use crate::graph::GraphStore;
use obrain_common::types::{
    EdgeId, EpochId, LogicalType, NodeId, PropertyKey, TransactionId, Value,
};
use std::sync::Arc;

/// An expand operator that traverses edges from source nodes.
///
/// For each input row containing a source node, this operator produces
/// output rows for each neighbor connected via matching edges.
pub struct ExpandOperator {
    /// The store to traverse.
    store: Arc<dyn GraphStore>,
    /// Input operator providing source nodes.
    input: Box<dyn Operator>,
    /// Index of the source node column in input.
    source_column: usize,
    /// Direction of edge traversal.
    direction: Direction,
    /// Edge type filter (empty = match all types, multiple = match any).
    edge_types: Vec<String>,
    /// Résolution CANONIQUE (insensible à la casse) du type unique demandé,
    /// pour le pré-filtre typed-degree : `None` = pas encore résolu,
    /// `Some(None)` = aucune correspondance (pré-filtre désactivé),
    /// `Some(Some(name))` = nom canonique du registre.
    typed_prefilter_canonical: Option<Option<String>>,
    /// Chunk capacity.
    chunk_capacity: usize,
    /// Current input chunk being processed.
    current_input: Option<DataChunk>,
    /// Current row index in the input chunk.
    current_row: usize,
    /// Current edge iterator for the current row.
    current_edges: Vec<(NodeId, EdgeId)>,
    /// (2026-08-17, aretes inter-bases V2) Resultats NON LOCAUX de la
    /// ligne courante : aretes externes du noeud source, et TOUTE
    /// l'expansion quand la source est un noeud distant. Paires
    /// (valeur d'arete, valeur de cible) emises apres `current_edges` —
    /// vide en session mono-base.
    current_external: Vec<(Value, Value)>,
    /// Current edge index.
    current_edge_idx: usize,
    /// Whether the operator is exhausted.
    exhausted: bool,
    /// Transaction ID for MVCC visibility (None = use current epoch).
    transaction_id: Option<TransactionId>,
    /// Epoch for version visibility.
    viewing_epoch: Option<EpochId>,
    /// When true, skip versioned (MVCC) lookups even if a transaction ID is
    /// present.  Safe for read-only queries where the transaction has no
    /// pending writes, avoiding the cost of walking version chains.
    read_only: bool,
    /// Contexte de session — porte le registre multi-store (aretes
    /// inter-bases V2). Sans registre, comportement historique inchange.
    session_context: SessionContext,
}

impl ExpandOperator {
    /// Creates a new expand operator.
    pub fn new(
        store: Arc<dyn GraphStore>,
        input: Box<dyn Operator>,
        source_column: usize,
        direction: Direction,
        edge_types: Vec<String>,
    ) -> Self {
        Self {
            store,
            input,
            source_column,
            direction,
            edge_types,
            typed_prefilter_canonical: None,
            chunk_capacity: 2048,
            current_input: None,
            current_row: 0,
            current_edges: Vec::with_capacity(16), // typical node degree
            current_external: Vec::new(),
            session_context: SessionContext::default(),
            current_edge_idx: 0,
            exhausted: false,
            transaction_id: None,
            viewing_epoch: None,
            read_only: false,
        }
    }

    /// Résout (une fois) le nom canonique du type unique demandé, en
    /// comparant insensiblement à la casse contre `all_edge_types()` du
    /// registre. `None` = aucune correspondance → le pré-filtre typed-degree
    /// est désactivé pour cet opérateur.
    fn resolve_typed_prefilter_type(&mut self) -> Option<String> {
        if let Some(cached) = &self.typed_prefilter_canonical {
            return cached.clone();
        }
        let resolved = if self.edge_types.len() == 1 {
            let requested = &self.edge_types[0];
            self.store
                .all_edge_types()
                .into_iter()
                .find(|t| t.eq_ignore_ascii_case(requested.as_str()))
        } else {
            None
        };
        self.typed_prefilter_canonical = Some(resolved.clone());
        resolved
    }

    /// Sets the chunk capacity.
    pub fn with_chunk_capacity(mut self, capacity: usize) -> Self {
        self.chunk_capacity = capacity;
        self
    }

    /// Sets the transaction context for MVCC visibility.
    ///
    /// When set, the expand will only traverse visible edges and nodes.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Attache le contexte de session (registre multi-store — aretes
    /// inter-bases V2). Sans lui, les aretes externes ne se resolvent pas
    /// (absent trivalue) et les sources distantes sont des erreurs.
    #[must_use]
    pub fn with_session_context(mut self, ctx: SessionContext) -> Self {
        self.session_context = ctx;
        self
    }

    /// Marks this expand as read-only, enabling fast-path lookups.
    ///
    /// When the query has no mutations, versioned MVCC lookups (which walk
    /// version chains to find PENDING writes) can be skipped in favour of
    /// cheaper epoch-only visibility checks.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Loads the next input chunk.
    fn load_next_input(&mut self) -> Result<bool, OperatorError> {
        match self.input.next() {
            Ok(Some(mut chunk)) => {
                // Flatten the chunk if it has a selection vector so we can use direct indexing
                chunk.flatten();
                self.current_input = Some(chunk);
                self.current_row = 0;
                self.current_edges.clear();
                self.current_external.clear();
                self.current_edge_idx = 0;
                Ok(true)
            }
            Ok(None) => {
                self.exhausted = true;
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// Loads edges for the current row.
    fn load_edges_for_current_row(&mut self) -> Result<bool, OperatorError> {
        let Some(chunk) = &self.current_input else {
            return Ok(false);
        };

        if self.current_row >= chunk.row_count() {
            return Ok(false);
        }

        let col = chunk.column(self.source_column).ok_or_else(|| {
            OperatorError::ColumnNotFound(format!("Column {} not found", self.source_column))
        })?;

        self.current_edges.clear();
        self.current_external.clear();
        self.current_edge_idx = 0;

        // Aretes inter-bases (V2) : la colonne source peut porter un noeud
        // DISTANT (Value::Map hydratee a la frontiere) ou Null. Sans
        // registre, le contrat historique est inchange : NodeId ou erreur.
        let source_id = match col.get_node_id(self.current_row) {
            Some(id) => id,
            None => {
                if let Some(reg) = self.session_context.base_registry.clone() {
                    match col.get_value(self.current_row) {
                        Some(Value::Map(map)) => {
                            let slug = match map.get(&PropertyKey::new(base_registry::BASE_KEY)) {
                                Some(Value::String(s)) => s.clone(),
                                _ => {
                                    return Err(OperatorError::Execution(
                                        "Expected node ID in source column".into(),
                                    ));
                                }
                            };
                            let id = match map.get(&PropertyKey::new("_id")) {
                                Some(Value::Int64(i)) => NodeId(*i as u64),
                                _ => {
                                    return Err(OperatorError::Execution(
                                        "Expected node ID in source column".into(),
                                    ));
                                }
                            };
                            self.expand_remote_source(&reg, slug.as_str(), id);
                            return Ok(true);
                        }
                        // Absent (Null) : zero ligne, jamais une erreur.
                        Some(Value::Null) | None => return Ok(true),
                        _ => {
                            return Err(OperatorError::Execution(
                                "Expected node ID in source column".into(),
                            ));
                        }
                    }
                }
                return Err(OperatorError::Execution(
                    "Expected node ID in source column".into(),
                ));
            }
        };

        // Get visibility context.  When `read_only` is true we can skip the
        // more expensive versioned lookups because the transaction has no
        // pending writes: epoch-only visibility is sufficient and avoids
        // walking MVCC version chains.
        let epoch = self.viewing_epoch;
        let transaction_id = self.transaction_id;
        let use_versioned = !self.read_only;

        // (2026-08-31) PRÉ-FILTRE PAR DEGRÉ TYPÉ — le saut qui ferme l'écart
        // vs les serveurs in-memory (index adjacency par label).
        //
        // Mesuré (RUN 13) : `MATCH (a:Function)-[:CALLS]->(b)` coûtait 1,08 ms
        // alors que le scan Function seul coûte 0,01 ms — le coût est la
        // marche `edges_from` par nœud, même pour les nœuds SANS arête du
        // type demandé (40 % des Function sur aletheia n'ont pas de CALLS).
        // Quand l'expand demande UN SEUL type d'arête et que le store expose
        // la colonne de degré typé (O(1), cf. substrate.degrees.node.CALLS),
        // on saute le nœud si `out_degree_by_type == 0` : zéro marche
        // d'adjacence inutile. C'est l'équivalent substrate de l'index
        // adjacency de memgraph.
        //
        // (2026-09-01) KILL SWITCH `OBRAIN_DISABLE_TYPED_DEGREE_PREFILTER` :
        // les colonnes de degrés typés sont des sidecars PEUVENT être
        // périmées (ex. migrées par un binaire plus ancien avec un mapping
        // de slots différent — CRC valide, valeurs fausses). Le pré-filtre
        // saute alors TOUS les nœuds et `MATCH (a)-[:T]->(b)` renvoie 0
        // (incident prod obrain-harness 2026-09-01 : sessions_list vide).
        // Le kill switch restore le chemin scan complet (correct, plus lent)
        // sans redéploiement ; le remède définitif est `obrain repair
        // degrees` (rebuild des sidecars depuis un scan des arêtes).
        if std::env::var_os("OBRAIN_DISABLE_TYPED_DEGREE_PREFILTER").is_none()
            && self.edge_types.len() == 1
            && self.store.supports_typed_degree()
            && self.direction == Direction::Outgoing
        {
            // Le pré-filtre typed-degree est indexé par id de type EXACT. Un
            // nom de casse différente (« knows » vs « KNOWS ») résoudrait 0
            // et court-circuiterait la source à zéro ligne AVANT le filtre
            // insensible à la casse du chemin principal — régression observée
            // (operators_expand::test_expand_edge_type_case_insensitive).
            // On résout donc d'abord le nom canonique (une fois par opérateur,
            // registre de types petit) ; sans correspondance, pas de pré-filtre
            // (le scan versionné reste correct, y compris types en attente).
            if let Some(canonical) = self.resolve_typed_prefilter_type() {
                // (2026-09-05) FIABILITÉ DES SIDECARS — dernière porte avant
                // le saut : `typed_degree_reliable` (backend substrate :
                // auto-vérification somme colonnes vs arêtes réelles, heal si
                // possible). false = colonnes périmées/non hydratables → on
                // NE SAUTE PAS : on retombe sur le scan générique ci-dessous,
                // qui reste correct (jamais de 0 silencieux).
                if self.store.typed_degree_reliable(&canonical) {
                    let deg = self.store.out_degree_by_type(source_id, Some(&canonical));
                    if deg == 0 {
                        // Aucune arête de ce type au départ du nœud : zéro ligne.
                        self.current_row += 1;
                        return self.load_edges_for_current_row();
                    }
                }
            }
        }

        // Get edges from this node
        let edges: Vec<(NodeId, EdgeId)> = self
            .store
            .edges_from(source_id, self.direction)
            .into_iter()
            .filter(|(target_id, edge_id)| {
                // Filter by edge type if specified
                let type_matches = if self.edge_types.is_empty() {
                    true
                } else {
                    // Use versioned type lookup only when we need to see
                    // PENDING (uncommitted) edges created by this transaction.
                    let actual_type =
                        if use_versioned && let (Some(ep), Some(tx)) = (epoch, transaction_id) {
                            self.store.edge_type_versioned(*edge_id, ep, tx)
                        } else {
                            self.store.edge_type(*edge_id)
                        };
                    actual_type.is_some_and(|t| {
                        self.edge_types
                            .iter()
                            .any(|et| t.as_str().eq_ignore_ascii_case(et.as_str()))
                    })
                };

                if !type_matches {
                    return false;
                }

                // Filter by visibility if we have epoch context
                if let Some(epoch) = epoch {
                    if use_versioned && let Some(tx) = transaction_id {
                        self.store.is_edge_visible_versioned(*edge_id, epoch, tx)
                            && self.store.is_node_visible_versioned(*target_id, epoch, tx)
                    } else {
                        self.store.is_edge_visible_at_epoch(*edge_id, epoch)
                            && self.store.is_node_visible_at_epoch(*target_id, epoch)
                    }
                } else {
                    true
                }
            })
            .collect();

        self.current_edges = edges;
        self.current_edge_idx = 0;

        // Aretes inter-bases (V2) : bras EXTERNE de l'expansion — les
        // aretes sortantes du registre externe, resolues par seek dans le
        // store cible. Sans registre : rien a resoudre, la reference est
        // simplement absente (trivalue).
        if let Some(reg) = self.session_context.base_registry.clone() {
            let primary = Arc::clone(&self.store);
            self.charger_aretes_externes(&reg, None, &*primary, source_id);
        }
        Ok(true)
    }

    /// Expansion complete depuis un noeud DISTANT (`slug`) : ses aretes
    /// internes (dans SON store) puis ses aretes externes. Tout est emis
    /// en valeurs (Map / Int64-primaire) — le store primaire n'est jamais
    /// scanne. Store cible inaccessible = zero ligne.
    fn expand_remote_source(&mut self, reg: &Arc<BaseRegistry>, slug: &str, source_id: NodeId) {
        // (V3) Franchissement : les droits du lecteur s'evaluent sur le
        // store cible — un refus rend la suite du motif ABSENTE. Le
        // comptage (denied/dangling) vit dans store_for_reader.
        let Some(store) = reg.store_for_reader(
            self.session_context.access_context.as_deref(),
            None,
            slug,
            "",
        ) else {
            return;
        };
        // Aretes internes du store distant. Lectures NON versionnees :
        // les epoques/transactions de la base primaire n'ont aucun sens
        // dans un autre store.
        for (target_id, edge_id) in store.edges_from(source_id, self.direction) {
            if !self.edge_types.is_empty() {
                let matches = store.edge_type(edge_id).is_some_and(|t| {
                    self.edge_types
                        .iter()
                        .any(|et| t.as_str().eq_ignore_ascii_case(et.as_str()))
                });
                if !matches {
                    continue;
                }
            }
            let Some(target_val) = remote_node_value(slug, &*store, target_id) else {
                continue; // cible morte : absent
            };
            let edge_val = remote_edge_value(slug, &*store, edge_id, source_id, target_id);
            self.current_external.push((edge_val, target_val));
        }
        // Aretes externes du noeud distant (cycle A -> B -> A compris).
        let reg = Arc::clone(reg);
        self.charger_aretes_externes(&reg, Some(slug), &*store, source_id);
    }

    /// Charge les aretes EXTERNES sortantes de `source_id` (portees par
    /// `source_store`), resout chaque cible par seek dans son store et
    /// pousse les paires (arete, cible) dans `current_external`.
    ///
    /// - direction entrante : rien — les aretes entrantes d'autres bases
    ///   ne sont pas enumerables sans scan cross-store (non-but, spec
    ///   paragraphe 6) ;
    /// - cible primaire : re-emise en NodeId natif (Int64) pour que la
    ///   suite du motif reprenne le chemin rapide local ;
    /// - hote distant / base absente / noeud disparu : ligne absente +
    ///   compteur, JAMAIS une erreur.
    fn charger_aretes_externes(
        &mut self,
        reg: &Arc<BaseRegistry>,
        source_slug: Option<&str>,
        source_store: &dyn GraphStore,
        source_id: NodeId,
    ) {
        if matches!(self.direction, Direction::Incoming) {
            return;
        }
        for e in source_store.external_edges_from(source_id) {
            if !self.edge_types.is_empty()
                && !self
                    .edge_types
                    .iter()
                    .any(|et| e.edge_type.eq_ignore_ascii_case(et.as_str()))
            {
                continue;
            }
            // (V3) Resolution GARDEE : droits du lecteur sur le store
            // cible, comptage denied/dangling centralise dedans.
            let Some(target_store) = reg.store_for_reader(
                self.session_context.access_context.as_deref(),
                e.to_host.as_deref(),
                &e.to_store,
                &e.to_key,
            ) else {
                continue;
            };
            let Some(target_id) = resolve_node_key(&*target_store, &e.to_key) else {
                reg.note_dangling();
                continue;
            };
            let edge_val =
                external_edge_value(&e, source_slug.unwrap_or_else(|| reg.primary_slug()));
            let target_val = if reg.is_primary(e.to_host.as_deref(), &e.to_store) {
                Value::Int64(target_id.as_u64() as i64)
            } else {
                match remote_node_value(&e.to_store, &*target_store, target_id) {
                    Some(v) => v,
                    None => {
                        reg.note_dangling();
                        continue;
                    }
                }
            };
            self.current_external.push((edge_val, target_val));
        }
    }
}

impl Operator for ExpandOperator {
    fn next(&mut self) -> OperatorResult {
        super::verifier_annulation()?;
        if self.exhausted {
            return Ok(None);
        }

        // Build output schema: preserve all input columns + edge + target
        // We need to build this dynamically based on input schema
        if self.current_input.is_none() {
            if !self.load_next_input()? {
                return Ok(None);
            }
            self.load_edges_for_current_row()?;
        }
        let input_chunk = self.current_input.as_ref().expect("input loaded above");

        // Build schema: [input_columns..., edge, target]
        let input_col_count = input_chunk.column_count();
        let mut schema: Vec<LogicalType> = (0..input_col_count)
            .map(|i| {
                input_chunk
                    .column(i)
                    .map_or(LogicalType::Any, |c| c.data_type().clone())
            })
            .collect();
        // Aretes inter-bases (V2) : avec un registre multi-store, les
        // colonnes arete/cible peuvent porter des valeurs distantes
        // (Value::Map) — le type passe a Any (vecteur Generic, sur lequel
        // get_node_id/get_edge_id savent relire les Int64 locaux). Sans
        // registre : types Edge/Node historiques, zero changement.
        if self.session_context.base_registry.is_some() {
            schema.push(LogicalType::Any);
            schema.push(LogicalType::Any);
        } else {
            schema.push(LogicalType::Edge);
            schema.push(LogicalType::Node);
        }

        let mut chunk = DataChunk::with_capacity(&schema, self.chunk_capacity);
        let mut count = 0;

        while count < self.chunk_capacity {
            // If we need a new input chunk
            if self.current_input.is_none() {
                if !self.load_next_input()? {
                    break;
                }
                self.load_edges_for_current_row()?;
            }

            // If we've exhausted edges for current row, move to next row
            while self.current_edge_idx >= self.current_edges.len() + self.current_external.len() {
                self.current_row += 1;

                // If we've exhausted the current input chunk, get next one
                if self.current_row >= self.current_input.as_ref().map_or(0, |c| c.row_count()) {
                    self.current_input = None;
                    if !self.load_next_input()? {
                        // No more input chunks
                        if count > 0 {
                            chunk.set_count(count);
                            return Ok(Some(chunk));
                        }
                        return Ok(None);
                    }
                }

                self.load_edges_for_current_row()?;
            }

            // Copy all input columns to output
            let input = self.current_input.as_ref().expect("input loaded above");
            for col_idx in 0..input_col_count {
                if let Some(input_col) = input.column(col_idx)
                    && let Some(output_col) = chunk.column_mut(col_idx)
                {
                    // Use copy_row_to which preserves NodeId/EdgeId types
                    input_col.copy_row_to(self.current_row, output_col);
                }
            }

            if self.current_edge_idx < self.current_edges.len() {
                // Arete locale : chemin historique (ids natifs).
                let (target_id, edge_id) = self.current_edges[self.current_edge_idx];
                if let Some(col) = chunk.column_mut(input_col_count) {
                    col.push_edge_id(edge_id);
                }
                if let Some(col) = chunk.column_mut(input_col_count + 1) {
                    col.push_node_id(target_id);
                }
            } else {
                // Aretes inter-bases (V2) : resultat non local, emis en
                // valeurs (les colonnes sont Generic dans ce mode).
                let (edge_val, target_val) =
                    self.current_external[self.current_edge_idx - self.current_edges.len()].clone();
                if let Some(col) = chunk.column_mut(input_col_count) {
                    col.push_value(edge_val);
                }
                if let Some(col) = chunk.column_mut(input_col_count + 1) {
                    col.push_value(target_val);
                }
            }

            count += 1;
            self.current_edge_idx += 1;
        }

        if count > 0 {
            chunk.set_count(count);
            Ok(Some(chunk))
        } else {
            Ok(None)
        }
    }

    fn reset(&mut self) {
        self.input.reset();
        self.current_input = None;
        self.current_row = 0;
        self.current_edges.clear();
        self.current_external.clear();
        self.current_edge_idx = 0;
        self.exhausted = false;
    }

    fn name(&self) -> &'static str {
        "Expand"
    }
}

// Tests relocated to `crates/obrain-substrate/tests/operators_expand.rs`
// (T17 Step 3 W2 Class-2 migration — decision `b1dfe229`). obrain-core cannot
// take obrain-substrate as a dev-dep (dev-dep cycle, gotcha `598dda40`), so
// the LPG-era fixtures are rebuilt against SubstrateStore in an integration
// test of obrain-substrate.
#[cfg(test)]
mod tests {}
