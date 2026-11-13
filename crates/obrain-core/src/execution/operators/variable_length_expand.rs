//! Variable-length expand operator for multi-hop path traversal.

use super::{Operator, OperatorError, OperatorResult, SessionContext};
use crate::execution::DataChunk;
use crate::execution::base_registry::{self, remote_node_value, resolve_node_key};
use crate::graph::Direction;
use crate::graph::GraphStore;
use obrain_common::types::{
    EdgeId, EpochId, LogicalType, NodeId, PropertyKey, TransactionId, Value,
};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

/// Noeud du parcours multi-bases (V4) : `(indice de base, id)`. L'indice 0
/// est TOUJOURS la base primaire ; les autres indexent le petit interneur
/// `bases` de l'operateur. En mono-base tout vit en base 0 et la clef se
/// comporte exactement comme l'ancien `NodeId` nu — la generalisation
/// evite la collision d'ids entre bases dans les modes TRAIL/SIMPLE/
/// ACYCLIC (deux NodeId(7) de deux bases sont deux noeuds distincts).
#[derive(Clone, Copy, PartialEq, Eq)]
struct NoeudM {
    base: u16,
    id: NodeId,
}

/// Arete du parcours multi-bases : `(base SOURCE de l'arete, id)`. Les
/// aretes externes gardent leur bit haut — unique dans leur base source.
#[derive(Clone, Copy, PartialEq, Eq)]
struct AreteM {
    base: u16,
    id: EdgeId,
}

/// Path traversal mode controlling which paths are allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathMode {
    /// Allows repeated nodes and edges (default).
    #[default]
    Walk,
    /// No repeated edges in a path.
    Trail,
    /// No repeated nodes except the start and end may be equal.
    Simple,
    /// No repeated nodes at all.
    Acyclic,
}

/// An expand operator that handles variable-length path patterns like `*1..3`.
///
/// For each input row containing a source node, this operator produces
/// output rows for each neighbor reachable within the hop range.
#[allow(clippy::struct_excessive_bools)]
pub struct VariableLengthExpandOperator {
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
    /// Minimum number of hops.
    min_hops: u32,
    /// Maximum number of hops.
    max_hops: u32,
    /// Chunk capacity.
    chunk_capacity: usize,
    /// Transaction ID for MVCC visibility.
    transaction_id: Option<TransactionId>,
    /// Epoch for version visibility.
    viewing_epoch: Option<EpochId>,
    /// When true, skip versioned MVCC lookups (fast path for read-only queries).
    read_only: bool,
    /// Contexte de session — registre multi-store (aretes inter-bases V2).
    session_context: SessionContext,
    /// (V4) Bases internees du parcours : indice 0 = primaire. VIDE tant
    /// qu'aucun franchissement n'a lieu — une session mono-base n'y touche
    /// jamais (zero cout). Mutex jamais contendu (operateur
    /// mono-thread) : le trait Operator exige Sync.
    bases: parking_lot::Mutex<Vec<(String, Arc<dyn GraphStore>)>>,
    /// Materialized input rows.
    input_rows: Option<Vec<InputRow>>,
    /// Current input row index.
    current_input_idx: usize,
    /// Output buffer for pending results.
    output_buffer: Vec<OutputRow>,
    /// Whether the operator is exhausted.
    exhausted: bool,
    /// Whether to output path length as an additional column.
    output_path_length: bool,
    /// Whether to output full path detail (node list and edge list).
    output_path_detail: bool,
    /// Path traversal mode (WALK, TRAIL, SIMPLE, ACYCLIC).
    path_mode: PathMode,
}

/// A materialized input row.
struct InputRow {
    /// All column values from the input.
    columns: Vec<ColumnValue>,
    /// The source node for expansion (base 0 = primaire).
    source_node: NoeudM,
}

/// A column value that can be node ID, edge ID, or generic value.
#[derive(Clone)]
enum ColumnValue {
    NodeId(NodeId),
    EdgeId(EdgeId),
    Value(obrain_common::types::Value),
}

/// A ready output row.
struct OutputRow {
    /// Index into input_rows for the source row.
    input_idx: usize,
    /// The final edge in the path.
    edge_id: AreteM,
    /// The target node.
    target_id: NoeudM,
    /// The path length (number of edges/hops).
    path_length: u32,
    /// All nodes along the path (source through target), populated when
    /// tracking. NB multi-bases : les listes de detail portent les ids
    /// BRUTS sans qualification de base (limitation documentee V4).
    path_nodes: Option<Vec<NodeId>>,
    /// All edges along the path, populated when tracking.
    path_edges: Option<Vec<EdgeId>>,
}

/// A shared-prefix path segment for efficient BFS path tracking.
///
/// Instead of cloning entire `Vec<NodeId>` / `Vec<EdgeId>` at each BFS expansion
/// step (O(depth) per clone), segments form an `Rc`-linked list that shares common
/// prefixes. Expansion costs O(1) (one `Rc::clone` + one allocation). Full paths
/// are only materialized when emitting output rows.
struct PathSegment {
    /// The node at this position in the path (multi-bases : (base, id)).
    node: NoeudM,
    /// The edge taken to reach this node. `None` for the source/root node.
    edge: Option<AreteM>,
    /// Parent segment, or `None` for the root.
    parent: Option<Rc<PathSegment>>,
}

impl PathSegment {
    /// Materializes the full node path from root to this segment.
    fn collect_nodes(&self, depth: u32) -> Vec<NodeId> {
        let mut nodes = Vec::with_capacity(depth as usize + 1);
        self.collect_nodes_into(&mut nodes);
        nodes
    }

    fn collect_nodes_into(&self, nodes: &mut Vec<NodeId>) {
        if let Some(parent) = &self.parent {
            parent.collect_nodes_into(nodes);
        }
        nodes.push(self.node.id);
    }

    /// Materializes the full edge path from root to this segment.
    fn collect_edges(&self, depth: u32) -> Vec<EdgeId> {
        let mut edges = Vec::with_capacity(depth as usize);
        self.collect_edges_into(&mut edges);
        edges
    }

    fn collect_edges_into(&self, edges: &mut Vec<EdgeId>) {
        if let Some(parent) = &self.parent {
            parent.collect_edges_into(edges);
        }
        if let Some(edge) = self.edge {
            edges.push(edge.id);
        }
    }

    /// Checks whether a node already appears in this path segment chain.
    /// La clef est (base, id) : pas de collision entre bases.
    fn contains_node(&self, target: NoeudM) -> bool {
        if self.node == target {
            return true;
        }
        if let Some(parent) = &self.parent {
            return parent.contains_node(target);
        }
        false
    }

    /// Checks whether an edge already appears in this path segment chain.
    fn contains_edge(&self, target: AreteM) -> bool {
        if self.edge == Some(target) {
            return true;
        }
        if let Some(parent) = &self.parent {
            return parent.contains_edge(target);
        }
        false
    }
}

impl VariableLengthExpandOperator {
    /// Creates a new variable-length expand operator.
    pub fn new(
        store: Arc<dyn GraphStore>,
        input: Box<dyn Operator>,
        source_column: usize,
        direction: Direction,
        edge_types: Vec<String>,
        min_hops: u32,
        max_hops: u32,
    ) -> Self {
        Self {
            store,
            input,
            source_column,
            direction,
            edge_types,
            min_hops,
            max_hops: max_hops.max(min_hops), // Ensure max >= min
            chunk_capacity: 2048,
            transaction_id: None,
            viewing_epoch: None,
            read_only: false,
            session_context: SessionContext::default(),
            bases: parking_lot::Mutex::new(Vec::new()),
            input_rows: None,
            current_input_idx: 0,
            output_buffer: Vec::new(),
            exhausted: false,
            output_path_length: false,
            output_path_detail: false,
            path_mode: PathMode::Walk,
        }
    }

    /// Sets the path traversal mode.
    pub fn with_path_mode(mut self, mode: PathMode) -> Self {
        self.path_mode = mode;
        self
    }

    /// Enables path length output as an additional column.
    pub fn with_path_length_output(mut self) -> Self {
        self.output_path_length = true;
        self
    }

    /// Enables full path detail output (node list and edge list columns).
    pub fn with_path_detail_output(mut self) -> Self {
        self.output_path_detail = true;
        self
    }

    /// Sets the chunk capacity.
    pub fn with_chunk_capacity(mut self, capacity: usize) -> Self {
        self.chunk_capacity = capacity;
        self
    }

    /// Sets the transaction context for MVCC visibility.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Marks this expand as read-only, enabling fast-path lookups.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Attache le contexte de session (registre multi-store — aretes
    /// inter-bases V2). Le motif de longueur variable ne traverse pas
    /// encore la frontiere (reste V4) : le registre sert uniquement a
    /// honorer le contrat trivalue — une source DISTANTE (Map) donne une
    /// ligne absente, jamais une erreur.
    #[must_use]
    pub fn with_session_context(mut self, ctx: SessionContext) -> Self {
        self.session_context = ctx;
        self
    }

    /// Materializes all input rows.
    fn materialize_input(&mut self) -> Result<(), OperatorError> {
        let mut rows = Vec::new();

        while let Some(mut chunk) = self.input.next()? {
            super::verifier_annulation()?;
            // Flatten to handle selection vectors
            chunk.flatten();

            for row_idx in 0..chunk.row_count() {
                // Extract the source node ID
                let col = chunk.column(self.source_column).ok_or_else(|| {
                    OperatorError::ColumnNotFound(format!(
                        "Column {} not found",
                        self.source_column
                    ))
                })?;

                // Source NULL : aucune ligne (voir `ExpandOperator`).
                if col.is_null(row_idx) {
                    continue;
                }
                let source_node = match col.get_node_id(row_idx) {
                    Some(id) => NoeudM { base: 0, id },
                    None => {
                        if let Some(reg) = self.session_context.base_registry.clone() {
                            // (V4) Source DISTANTE (Map _base/_id) : le
                            // parcours reprend dans SA base — c'est un
                            // franchissement, donc resolution GARDEE
                            // (droits du lecteur). Irresoluble/refuse/
                            // absent = ligne ABSENTE, jamais une erreur.
                            if let Some(Value::Map(map)) = col.get_value(row_idx)
                                && let Some(Value::String(slug)) =
                                    map.get(&PropertyKey::new(base_registry::BASE_KEY))
                                && let Some(Value::Int64(id)) = map.get(&PropertyKey::new("_id"))
                                && let Some(store) = reg.store_for_reader(
                                    self.session_context.access_context.as_deref(),
                                    None,
                                    slug.as_str(),
                                    "",
                                )
                            {
                                NoeudM {
                                    base: self.indice_base(slug.as_str(), &store),
                                    id: NodeId(*id as u64),
                                }
                            } else {
                                continue;
                            }
                        } else {
                            return Err(OperatorError::Execution(
                                "Expected node ID in source column".into(),
                            ));
                        }
                    }
                };

                // Materialize all columns
                let mut columns = Vec::with_capacity(chunk.column_count());
                for col_idx in 0..chunk.column_count() {
                    let col = chunk
                        .column(col_idx)
                        .expect("col_idx within column_count range");
                    let value = if let Some(node_id) = col.get_node_id(row_idx) {
                        ColumnValue::NodeId(node_id)
                    } else if let Some(edge_id) = col.get_edge_id(row_idx) {
                        ColumnValue::EdgeId(edge_id)
                    } else if let Some(val) = col.get_value(row_idx) {
                        ColumnValue::Value(val)
                    } else {
                        ColumnValue::Value(obrain_common::types::Value::Null)
                    };
                    columns.push(value);
                }

                rows.push(InputRow {
                    columns,
                    source_node,
                });
            }
        }

        self.input_rows = Some(rows);
        Ok(())
    }

    /// (V4) Indice interne d'une base du parcours. 0 = primaire ;
    /// l'interneur ne grandit qu'aux franchissements (mono-base : jamais).
    fn indice_base(&self, slug: &str, store: &Arc<dyn GraphStore>) -> u16 {
        let mut bases = self.bases.lock();
        if bases.is_empty() {
            let primaire = self
                .session_context
                .base_registry
                .as_ref()
                .map_or_else(String::new, |r| r.primary_slug().to_string());
            bases.push((primaire, Arc::clone(&self.store)));
        }
        if let Some(i) = bases.iter().position(|(s, _)| s == slug) {
            return u16::try_from(i).unwrap_or(0);
        }
        bases.push((slug.to_string(), Arc::clone(store)));
        u16::try_from(bases.len() - 1).unwrap_or(0)
    }

    /// Slug et store d'un indice de base (0 = primaire meme sans interneur).
    fn base_de(&self, idx: u16) -> (String, Arc<dyn GraphStore>) {
        let bases = self.bases.lock();
        if let Some((slug, store)) = bases.get(idx as usize) {
            (slug.clone(), Arc::clone(store))
        } else {
            let primaire = self
                .session_context
                .base_registry
                .as_ref()
                .map_or_else(String::new, |r| r.primary_slug().to_string());
            (primaire, Arc::clone(&self.store))
        }
    }

    /// (V4) Expansion d'un niveau, multi-bases : aretes internes de la base
    /// du noeud + bras EXTERNE (resolution GARDEE par saut — droits,
    /// pendantes et refus = branche absente). Sans registre : strictement
    /// equivalent a `get_edges` (le bras externe ne s'active jamais).
    fn get_edges_multi(&self, node: NoeudM) -> Vec<(NoeudM, AreteM)> {
        let mut out: Vec<(NoeudM, AreteM)> = Vec::new();
        if node.base == 0 {
            // Base primaire : chemin historique (filtres MVCC compris).
            for (t, e) in self.get_edges(node.id) {
                out.push((NoeudM { base: 0, id: t }, AreteM { base: 0, id: e }));
            }
        } else {
            // Base distante : lectures NON versionnees (les epoques de la
            // base primaire n'y ont aucun sens), type + vivacite.
            let (_slug, store) = self.base_de(node.base);
            for (t, e) in store.edges_from(node.id, self.direction) {
                if !self.edge_types.is_empty() {
                    let ok = store.edge_type(e).is_some_and(|ty| {
                        self.edge_types
                            .iter()
                            .any(|et| ty.as_str().eq_ignore_ascii_case(et.as_str()))
                    });
                    if !ok {
                        continue;
                    }
                }
                if store.get_node_labels(t).is_none() {
                    continue;
                }
                out.push((
                    NoeudM {
                        base: node.base,
                        id: t,
                    },
                    AreteM {
                        base: node.base,
                        id: e,
                    },
                ));
            }
        }
        // Bras EXTERNE : uniquement avec un registre (zero cout mono-base).
        if let Some(reg) = self.session_context.base_registry.clone()
            && !matches!(self.direction, Direction::Incoming)
        {
            let (_slug, src_store) = self.base_de(node.base);
            for e in src_store.external_edges_from(node.id) {
                if !self.edge_types.is_empty()
                    && !self
                        .edge_types
                        .iter()
                        .any(|et| e.edge_type.eq_ignore_ascii_case(et.as_str()))
                {
                    continue;
                }
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
                let cible_base = if reg.is_primary(e.to_host.as_deref(), &e.to_store) {
                    // Retour a la primaire : base 0, chemin natif.
                    0
                } else {
                    self.indice_base(&e.to_store, &target_store)
                };
                out.push((
                    NoeudM {
                        base: cible_base,
                        id: target_id,
                    },
                    AreteM {
                        base: node.base,
                        id: e.id,
                    },
                ));
            }
        }
        out
    }

    /// Gets edges from a node, respecting filters and visibility.
    fn get_edges(&self, node_id: NodeId) -> Vec<(NodeId, EdgeId)> {
        let epoch = self.viewing_epoch;
        let transaction_id = self.transaction_id;
        let use_versioned = !self.read_only;

        self.store
            .edges_from(node_id, self.direction)
            .into_iter()
            .filter(|(target_id, edge_id)| {
                // Filter by edge type if specified
                let type_matches = if self.edge_types.is_empty() {
                    true
                } else if let Some(actual_type) = self.store.edge_type(*edge_id) {
                    self.edge_types
                        .iter()
                        .any(|t| actual_type.as_str().eq_ignore_ascii_case(t.as_str()))
                } else {
                    false
                };

                if !type_matches {
                    return false;
                }

                // Filter by visibility
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
            .collect()
    }

    /// Checks whether a candidate expansion is allowed under the current path mode.
    fn is_expansion_allowed(
        &self,
        segment: &PathSegment,
        target: NoeudM,
        edge_id: AreteM,
        source_node: NoeudM,
    ) -> bool {
        match self.path_mode {
            PathMode::Walk => true,
            PathMode::Trail => !segment.contains_edge(edge_id),
            PathMode::Simple => {
                // No repeated nodes except the start may equal the end
                target == source_node || !segment.contains_node(target)
            }
            PathMode::Acyclic => !segment.contains_node(target),
        }
    }

    /// Process one input row, generating all reachable outputs.
    fn process_input_row(&self, input_idx: usize, source_node: NoeudM) -> Vec<OutputRow> {
        let mut results = Vec::new();
        let needs_tracking = self.output_path_detail || self.path_mode != PathMode::Walk;

        if needs_tracking {
            // BFS with shared-prefix path tracking via Rc<PathSegment>.
            // Required for path detail output or non-Walk path modes.
            let mut frontier: VecDeque<(NoeudM, u32, AreteM, Rc<PathSegment>)> = VecDeque::new();

            let root = Rc::new(PathSegment {
                node: source_node,
                edge: None,
                parent: None,
            });

            for (target, edge_id) in self.get_edges_multi(source_node) {
                if !self.is_expansion_allowed(&root, target, edge_id, source_node) {
                    continue;
                }
                let segment = Rc::new(PathSegment {
                    node: target,
                    edge: Some(edge_id),
                    parent: Some(Rc::clone(&root)),
                });
                frontier.push_back((target, 1, edge_id, segment));
            }

            // BFS a frontiere : le nombre de nœuds visites croit avec le
            // degre a chaque saut. Sur un graphe dense, `*1..5` explose bien
            // avant de rendre un seul chunk. Controle par lot de frontiere.
            let mut vigie = obrain_common::utils::cancel::Vigie::avec_intervalle(256);
            while let Some((current_node, depth, edge_id, segment)) = frontier.pop_front() {
                // `process_input_row` rend un `Vec<OutputRow>` nu — aucun
                // canal pour dire « je n ai pas fini ». On sort donc de la
                // BFS avec ce qui a ete trouve, et c est le controle de
                // `next()` (jeton MONOTONE) qui convertit ce partiel en
                // `Err(Cancelled)` avant qu il n atteigne l executeur.
                if vigie.tick_bool() {
                    break;
                }
                if depth >= self.min_hops && depth <= self.max_hops {
                    results.push(OutputRow {
                        input_idx,
                        edge_id,
                        target_id: current_node,
                        path_length: depth,
                        path_nodes: if self.output_path_detail {
                            Some(segment.collect_nodes(depth))
                        } else {
                            None
                        },
                        path_edges: if self.output_path_detail {
                            Some(segment.collect_edges(depth))
                        } else {
                            None
                        },
                    });
                }

                if depth < self.max_hops {
                    for (target, next_edge_id) in self.get_edges_multi(current_node) {
                        if !self.is_expansion_allowed(&segment, target, next_edge_id, source_node) {
                            continue;
                        }
                        let new_segment = Rc::new(PathSegment {
                            node: target,
                            edge: Some(next_edge_id),
                            parent: Some(Rc::clone(&segment)),
                        });
                        frontier.push_back((target, depth + 1, next_edge_id, new_segment));
                    }
                }
            }
        } else {
            // BFS without path tracking (lightweight, Walk mode only)
            let mut frontier: VecDeque<(NoeudM, u32, AreteM)> = VecDeque::new();

            for (target, edge_id) in self.get_edges_multi(source_node) {
                frontier.push_back((target, 1, edge_id));
            }

            // Meme raisonnement que la variante avec segments ci-dessus.
            let mut vigie = obrain_common::utils::cancel::Vigie::avec_intervalle(256);
            while let Some((current_node, depth, edge_id)) = frontier.pop_front() {
                // `process_input_row` rend un `Vec<OutputRow>` nu — aucun
                // canal pour dire « je n ai pas fini ». On sort donc de la
                // BFS avec ce qui a ete trouve, et c est le controle de
                // `next()` (jeton MONOTONE) qui convertit ce partiel en
                // `Err(Cancelled)` avant qu il n atteigne l executeur.
                if vigie.tick_bool() {
                    break;
                }
                if depth >= self.min_hops && depth <= self.max_hops {
                    results.push(OutputRow {
                        input_idx,
                        edge_id,
                        target_id: current_node,
                        path_length: depth,
                        path_nodes: None,
                        path_edges: None,
                    });
                }

                if depth < self.max_hops {
                    for (target, next_edge_id) in self.get_edges_multi(current_node) {
                        frontier.push_back((target, depth + 1, next_edge_id));
                    }
                }
            }
        }

        results
    }

    /// Fill the output buffer with results from the next input row.
    fn fill_output_buffer(&mut self) {
        let Some(input_rows) = &self.input_rows else {
            return;
        };

        while self.output_buffer.is_empty() && self.current_input_idx < input_rows.len() {
            // Une ligne d entree = une BFS complete. Sur une entree de
            // 100 000 lignes, cette boucle est le vrai moteur du cout.
            if obrain_common::utils::cancel::is_cancelled() {
                break;
            }
            let source_node = input_rows[self.current_input_idx].source_node;
            let results = self.process_input_row(self.current_input_idx, source_node);
            self.output_buffer.extend(results);
            self.current_input_idx += 1;
        }
    }
}

impl Operator for VariableLengthExpandOperator {
    fn next(&mut self) -> OperatorResult {
        // Le controle de SORTIE de tout le sous-arbre d expansion : les
        // boucles internes (`process_input_row`, `fill_output_buffer`)
        // s arretent en rendant un partiel faute de canal d erreur ; ce
        // controle-ci le convertit en `Err`. Jeton MONOTONE, donc exhaustif.
        super::verifier_annulation()?;
        if self.exhausted {
            return Ok(None);
        }

        // Materialize input on first call
        if self.input_rows.is_none() {
            self.materialize_input()?;
            if self.input_rows.as_ref().map_or(true, Vec::is_empty) {
                self.exhausted = true;
                return Ok(None);
            }
        }

        // Fill output buffer if empty
        self.fill_output_buffer();

        if self.output_buffer.is_empty() {
            self.exhausted = true;
            return Ok(None);
        }

        let input_rows = self
            .input_rows
            .as_ref()
            .expect("input_rows is Some: populated during BFS");

        // Build output chunk from buffer
        let num_input_cols = input_rows.first().map_or(0, |r| r.columns.len());

        // Schema: [input_columns..., edge, target, (path_length)?, (path_nodes)?, (path_edges)?, (path)?]
        let extra_cols =
            2 + usize::from(self.output_path_length) + usize::from(self.output_path_detail) * 3;
        let mut schema: Vec<LogicalType> = Vec::with_capacity(num_input_cols + extra_cols);
        if let Some(first_row) = input_rows.first() {
            for col_val in &first_row.columns {
                let ty = match col_val {
                    ColumnValue::NodeId(_) => LogicalType::Node,
                    ColumnValue::EdgeId(_) => LogicalType::Edge,
                    ColumnValue::Value(_) => LogicalType::Any,
                };
                schema.push(ty);
            }
        }
        // (V4) Multi-bases : les colonnes arete/cible peuvent porter des
        // valeurs distantes (Map) — type Any (Generic). Mono-base : types
        // historiques, zero changement.
        if self.session_context.base_registry.is_some() {
            schema.push(LogicalType::Any);
            schema.push(LogicalType::Any);
        } else {
            schema.push(LogicalType::Edge);
            schema.push(LogicalType::Node);
        }
        if self.output_path_length {
            schema.push(LogicalType::Int64);
        }
        if self.output_path_detail {
            schema.push(LogicalType::Any); // path_nodes as Value::List
            schema.push(LogicalType::Any); // path_edges as Value::List
            schema.push(LogicalType::Any); // Value::Path (first-class path)
        }

        let mut chunk = DataChunk::with_capacity(&schema, self.chunk_capacity);

        // Take up to chunk_capacity rows from buffer
        let take_count = self.output_buffer.len().min(self.chunk_capacity);
        let to_output: Vec<_> = self.output_buffer.drain(..take_count).collect();

        for out_row in &to_output {
            let input_row = &input_rows[out_row.input_idx];

            // Copy input columns
            for (col_idx, col_val) in input_row.columns.iter().enumerate() {
                if let Some(out_col) = chunk.column_mut(col_idx) {
                    match col_val {
                        ColumnValue::NodeId(id) => out_col.push_node_id(*id),
                        ColumnValue::EdgeId(id) => out_col.push_edge_id(*id),
                        ColumnValue::Value(v) => out_col.push_value(v.clone()),
                    }
                }
            }

            // Add edge column. (V4) Une arete d'une base distante s'emet
            // en Map minimale {_id, _base} — son id nu serait ambigu.
            if let Some(col) = chunk.column_mut(num_input_cols) {
                if out_row.edge_id.base == 0 {
                    col.push_edge_id(out_row.edge_id.id);
                } else {
                    let (slug, _) = self.base_de(out_row.edge_id.base);
                    let mut map = std::collections::BTreeMap::new();
                    map.insert(
                        PropertyKey::new("_id"),
                        Value::Int64(out_row.edge_id.id.as_u64() as i64),
                    );
                    map.insert(
                        PropertyKey::new(base_registry::BASE_KEY),
                        Value::String(slug.into()),
                    );
                    col.push_value(Value::Map(Arc::new(map)));
                }
            }

            // Add target node column. (V4) Cible distante : hydratation
            // frontiere (Map _base + proprietes), comme ExpandOperator.
            if let Some(col) = chunk.column_mut(num_input_cols + 1) {
                if out_row.target_id.base == 0 {
                    col.push_node_id(out_row.target_id.id);
                } else {
                    let (slug, store) = self.base_de(out_row.target_id.base);
                    match remote_node_value(&slug, &*store, out_row.target_id.id) {
                        Some(v) => col.push_value(v),
                        None => col.push_value(Value::Null),
                    }
                }
            }

            // Add path length column if requested
            if self.output_path_length
                && let Some(col) = chunk.column_mut(num_input_cols + 2)
            {
                col.push_value(obrain_common::types::Value::Int64(i64::from(
                    out_row.path_length,
                )));
            }

            // Add path detail columns if requested
            if self.output_path_detail {
                let base = num_input_cols + 2 + usize::from(self.output_path_length);

                // Path nodes column
                if let Some(col) = chunk.column_mut(base) {
                    let nodes_list: Vec<obrain_common::types::Value> = out_row
                        .path_nodes
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|id| obrain_common::types::Value::Int64(id.0 as i64))
                        .collect();
                    col.push_value(obrain_common::types::Value::List(nodes_list.into()));
                }

                // Path edges column
                if let Some(col) = chunk.column_mut(base + 1) {
                    let edges_list: Vec<obrain_common::types::Value> = out_row
                        .path_edges
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|id| obrain_common::types::Value::Int64(id.0 as i64))
                        .collect();
                    col.push_value(obrain_common::types::Value::List(edges_list.into()));
                }

                // Value::Path column (first-class path value)
                if let Some(col) = chunk.column_mut(base + 2) {
                    let nodes: Vec<obrain_common::types::Value> = out_row
                        .path_nodes
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|id| obrain_common::types::Value::Int64(id.0 as i64))
                        .collect();
                    let edges: Vec<obrain_common::types::Value> = out_row
                        .path_edges
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|id| obrain_common::types::Value::Int64(id.0 as i64))
                        .collect();
                    col.push_value(obrain_common::types::Value::Path {
                        nodes: nodes.into(),
                        edges: edges.into(),
                    });
                }
            }
        }

        chunk.set_count(to_output.len());
        Ok(Some(chunk))
    }

    fn reset(&mut self) {
        self.input.reset();
        self.input_rows = None;
        self.current_input_idx = 0;
        self.output_buffer.clear();
        self.exhausted = false;
    }

    fn name(&self) -> &'static str {
        "VariableLengthExpand"
    }
}

// Tests relocated to `crates/obrain-substrate/tests/operators_variable_length_expand.rs`
// (T17 Step 3 W2 Class-2 migration — decision `b1dfe229`). obrain-core cannot
// take obrain-substrate as a dev-dep (dev-dep cycle, gotcha `598dda40`), so
// the LPG-era fixtures are rebuilt against SubstrateStore in an integration
// test of obrain-substrate.
#[cfg(test)]
mod tests {}
