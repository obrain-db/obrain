//! Variable-length expand operator for multi-hop path traversal.

use super::{Operator, OperatorError, OperatorResult, SessionContext};
use crate::execution::DataChunk;
use crate::execution::base_registry::{
    self, remote_edge_hydratee, remote_node_value, resolve_node_key,
};
use crate::graph::Direction;
use crate::graph::GraphStore;
use obrain_common::types::{
    EdgeId, EpochId, LogicalType, NodeId, PropertyKey, TransactionId, Value,
};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
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
    /// (obrain#189) La colonne « arete » porte la LISTE des aretes du chemin
    /// (`Value::List` d'identifiants) au lieu de la seule derniere arete.
    /// C'est ce que lie une variable d'arete de longueur variable
    /// (`-[rs:T*1..3]->`) : `all(x IN rs WHERE …)`, `size(rs)`, `RETURN rs`.
    /// Sans ce drapeau, le filtre d'unicite entre relation simple et
    /// longueur variable recevait un EdgeId et eliminait toutes les lignes.
    output_edge_list: bool,
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
    /// (obrain#189) Colonne d'entree qui porte la CIBLE exacte du motif
    /// (`...-[*0..2]->(target)` avec `target` deja lie). Quand elle est
    /// posee, l'expansion est guidee par la cible : voir `with_target_column`.
    target_column: Option<usize>,
    /// (obrain#189) Colonne portant le chemin deja parcouru (`Value::Path`)
    /// par les expansions precedentes du meme motif nomme : voir
    /// `with_path_prefix`.
    path_prefix_column: Option<usize>,
    /// Distances (en sauts, sens inverse) de chaque noeud a une cible, par
    /// cible distincte. Calculees paresseusement, bornees par `max_hops`.
    reach_cache: HashMap<NodeId, Arc<HashMap<NodeId, u32>>>,
    /// (obrain#189) Chemins deja enumeres, par `(base, source, cible guide)`.
    /// L'expansion d'une ligne ne depend QUE de la source et de la cible :
    /// quand plusieurs lignes d'entree partagent la meme source (des milliers
    /// de notes liees au meme noeud), la BFS n'est faite qu'une fois et les
    /// lignes suivantes copient le resultat avec leur propre `input_idx`.
    chemins_par_source: HashMap<(u16, NodeId, Option<NodeId>), Vec<OutputRow>>,
    /// Nombre de lignes retenues dans `chemins_par_source` (borne la memoire).
    lignes_en_cache: usize,
}

/// Plafond de lignes de chemins gardees pour les sources deja parcourues.
const LIGNES_CACHE_MAX: usize = 500_000;

/// Clé d'identité (base, id) d'un élément de chemin déjà émis en `Value` :
/// primaire = `Int64` (base vide), distant = `Map` avec `_base` et `_id`.
/// `None` pour une valeur sans identité (ex. `Null`, absent) : non comparée.
fn cle_valeur(valeur: &Value) -> Option<(String, u64)> {
    match valeur {
        Value::Int64(i) => Some((String::new(), *i as u64)),
        Value::Map(map) => {
            let slug = match map.get(&PropertyKey::new(base_registry::BASE_KEY)) {
                Some(Value::String(s)) => s.to_string(),
                _ => return None,
            };
            match map.get(&PropertyKey::new("_id")) {
                Some(Value::Int64(i)) => Some((slug, *i as u64)),
                _ => None,
            }
        }
        _ => None,
    }
}

thread_local! {
    /// (obrain#189) Nombre de chemins ENUMERES (emis par l'expansion avant
    /// tout filtre aval), sur ce thread. Observabilite des tests : c'est la
    /// quantite que le guidage par la cible doit faire tomber.
    static CHEMINS_ENUMERES: Cell<u64> = const { Cell::new(0) };
}

/// Chemins enumeres par les expansions de longueur variable de CE thread
/// depuis sa creation (compteur monotone ; comparer deux lectures).
pub fn chemins_enumeres() -> u64 {
    CHEMINS_ENUMERES.with(Cell::get)
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
#[derive(Clone)]
struct OutputRow {
    /// Index into input_rows for the source row.
    input_idx: usize,
    /// The final edge in the path. `None` for a zero-length path (`*0..n`) :
    /// the target is the source itself and no edge was taken.
    edge_id: Option<AreteM>,
    /// The target node.
    target_id: NoeudM,
    /// The path length (number of edges/hops).
    path_length: u32,
    /// All nodes along the path (source through target), populated when
    /// tracking. Identites (base, id) conservees : la sortie les convertit en
    /// Value (primaire = Int64, distant = Map hydratee).
    path_nodes: Option<Vec<NoeudM>>,
    /// All edges along the path, populated when tracking.
    path_edges: Option<Vec<AreteM>>,
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
    /// Materializes the full node path from root to this segment (identites (base, id)).
    fn collect_nodes(&self, depth: u32) -> Vec<NoeudM> {
        let mut nodes = Vec::with_capacity(depth as usize + 1);
        self.collect_nodes_into(&mut nodes);
        nodes
    }

    fn collect_nodes_into(&self, nodes: &mut Vec<NoeudM>) {
        if let Some(parent) = &self.parent {
            parent.collect_nodes_into(nodes);
        }
        nodes.push(self.node);
    }

    /// Materializes the full edge path from root to this segment (identites (base, id)).
    fn collect_edges(&self, depth: u32) -> Vec<AreteM> {
        let mut edges = Vec::with_capacity(depth as usize);
        self.collect_edges_into(&mut edges);
        edges
    }

    fn collect_edges_into(&self, edges: &mut Vec<AreteM>) {
        if let Some(parent) = &self.parent {
            parent.collect_edges_into(edges);
        }
        if let Some(edge) = self.edge {
            edges.push(edge);
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
            output_edge_list: false,
            session_context: SessionContext::default(),
            bases: parking_lot::Mutex::new(Vec::new()),
            input_rows: None,
            current_input_idx: 0,
            output_buffer: Vec::new(),
            exhausted: false,
            output_path_length: false,
            output_path_detail: false,
            path_mode: PathMode::Walk,
            target_column: None,
            path_prefix_column: None,
            reach_cache: HashMap::new(),
            chemins_par_source: HashMap::new(),
            lignes_en_cache: 0,
        }
    }

    /// (obrain#189) Guide l'expansion par la cible exacte portee par la
    /// colonne `column` de l'entree.
    ///
    /// Le motif `(a)-[*0..k]->(t)` avec `t` deja lie ne garde que les chemins
    /// qui FINISSENT sur `t`. Enumerer tous les chemins de `a` avant de
    /// filtrer coute la taille du `k`-voisinage de CHAQUE source (exponentiel
    /// en `k`, sans borne memoire). On calcule donc une fois par cible
    /// l'ensemble des noeuds qui l'atteignent en au plus `max_hops` sauts
    /// (BFS en sens inverse), et l'on n'avance que vers un noeud pour lequel
    /// `profondeur + distance <= max_hops`. L'ensemble des chemins emis est
    /// exactement celui des chemins qui finissent sur la cible : le filtre
    /// d'egalite en aval reste correct et devient une no-op.
    ///
    /// Sans effet en direction `Both` ni avec un registre multi-bases.
    #[must_use]
    pub fn with_target_column(mut self, column: usize) -> Self {
        self.target_column = Some(column);
        self
    }

    /// (obrain#189) Colonne d'entree qui porte le CHEMIN DEJA PARCOURU
    /// (`Value::Path`) par les expansions precedentes d'un meme motif nomme :
    /// `MATCH p = (n)-[:A]->(s)-[:B*0..k]->(t)` enchaine deux expansions. La
    /// sortie de la seconde est alors `prefixe ++ segment` (noeuds sans le
    /// premier, aretes concatenees, longueur additionnee), au lieu du seul
    /// segment : sans cela `length(p)`, `nodes(p)` et `relationships(p)`
    /// ignoraient tout ce qui precede le `*`.
    ///
    /// Le prefixe est applique a l'emission de chaque ligne, pas dans le cache
    /// des chemins par source : celui-ci reste valable pour toute ligne.
    #[must_use]
    pub fn with_path_prefix(mut self, column: usize) -> Self {
        self.path_prefix_column = Some(column);
        self
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

    /// Emits the edge column as the list of the path's edges (see
    /// `output_edge_list`). Implies path tracking.
    #[must_use]
    pub fn with_edge_list_output(mut self) -> Self {
        self.output_edge_list = true;
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
        self.get_edges_dir(node_id, self.direction)
    }

    /// Comme `get_edges`, dans la direction donnee.
    fn get_edges_dir(&self, node_id: NodeId, direction: Direction) -> Vec<(NodeId, EdgeId)> {
        let epoch = self.viewing_epoch;
        let transaction_id = self.transaction_id;
        let use_versioned = !self.read_only;

        self.store
            .edges_from(node_id, direction)
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

    /// Distances inverses vers `target`, bornees par `max_hops` (BFS).
    /// `None` quand le guidage ne s'applique pas (voir `with_target_column`).
    fn reach_for(&mut self, target: NodeId) -> Option<Arc<HashMap<NodeId, u32>>> {
        let reverse = match self.direction {
            Direction::Outgoing => Direction::Incoming,
            Direction::Incoming => Direction::Outgoing,
            Direction::Both => return None,
        };
        if self.session_context.base_registry.is_some() {
            return None;
        }
        if let Some(cached) = self.reach_cache.get(&target) {
            return Some(Arc::clone(cached));
        }
        let mut dist: HashMap<NodeId, u32> = HashMap::new();
        dist.insert(target, 0);
        let mut frontier = vec![target];
        for depth in 1..=self.max_hops {
            let mut next = Vec::new();
            for node in frontier {
                for (pred, _) in self.get_edges_dir(node, reverse) {
                    if let std::collections::hash_map::Entry::Vacant(v) = dist.entry(pred) {
                        v.insert(depth);
                        next.push(pred);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let dist = Arc::new(dist);
        self.reach_cache.insert(target, Arc::clone(&dist));
        Some(dist)
    }

    /// Process one input row, generating all reachable outputs.
    ///
    /// `guide` : `(cible, distances inverses)` quand l'expansion est guidee
    /// par la cible — seuls les chemins qui finissent sur elle sont emis, et
    /// l'on n'avance que vers les noeuds qui peuvent encore l'atteindre.
    fn process_input_row(
        &self,
        input_idx: usize,
        source_node: NoeudM,
        guide: Option<(NodeId, &HashMap<NodeId, u32>)>,
    ) -> Vec<OutputRow> {
        // Peut-on encore atteindre la cible depuis `node`, arrive en `depth` sauts ?
        let peut_atteindre = |node: NoeudM, depth: u32| -> bool {
            guide.is_none_or(|(_, reach)| {
                reach
                    .get(&node.id)
                    .is_some_and(|d| depth.saturating_add(*d) <= self.max_hops)
            })
        };
        let est_cible = |node: NoeudM| guide.is_none_or(|(t, _)| node.id == t);
        let mut results = Vec::new();
        if !peut_atteindre(source_node, 0) {
            return results;
        }
        let needs_tracking =
            self.output_path_detail || self.output_edge_list || self.path_mode != PathMode::Walk;
        let needs_edges = self.output_path_detail || self.output_edge_list;

        // `*0..n` : le chemin de longueur 0 (source = cible, aucune arete)
        // existe dans tous les modes — un chemin vide ne repete ni noeud ni
        // arete. Il etait perdu : la BFS n'emet qu'a partir de la
        // profondeur 1 (obrain-db/obrain#189, `*0..3` vers un projet).
        if self.min_hops == 0 && est_cible(source_node) {
            results.push(OutputRow {
                input_idx,
                edge_id: None,
                target_id: source_node,
                path_length: 0,
                path_nodes: self.output_path_detail.then(|| vec![source_node]),
                path_edges: needs_edges.then(Vec::new),
            });
        }

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
                if !peut_atteindre(target, 1)
                    || !self.is_expansion_allowed(&root, target, edge_id, source_node)
                {
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
                if depth >= self.min_hops && depth <= self.max_hops && est_cible(current_node) {
                    results.push(OutputRow {
                        input_idx,
                        edge_id: Some(edge_id),
                        target_id: current_node,
                        path_length: depth,
                        path_nodes: if self.output_path_detail {
                            Some(segment.collect_nodes(depth))
                        } else {
                            None
                        },
                        path_edges: if needs_edges {
                            Some(segment.collect_edges(depth))
                        } else {
                            None
                        },
                    });
                }

                if depth < self.max_hops {
                    for (target, next_edge_id) in self.get_edges_multi(current_node) {
                        if !peut_atteindre(target, depth + 1)
                            || !self.is_expansion_allowed(
                                &segment,
                                target,
                                next_edge_id,
                                source_node,
                            )
                        {
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
                if peut_atteindre(target, 1) {
                    frontier.push_back((target, 1, edge_id));
                }
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
                if depth >= self.min_hops && depth <= self.max_hops && est_cible(current_node) {
                    results.push(OutputRow {
                        input_idx,
                        edge_id: Some(edge_id),
                        target_id: current_node,
                        path_length: depth,
                        path_nodes: None,
                        path_edges: None,
                    });
                }

                if depth < self.max_hops {
                    for (target, next_edge_id) in self.get_edges_multi(current_node) {
                        if peut_atteindre(target, depth + 1) {
                            frontier.push_back((target, depth + 1, next_edge_id));
                        }
                    }
                }
            }
        }

        CHEMINS_ENUMERES.with(|c| c.set(c.get() + results.len() as u64));
        results
    }

    /// Clé d'identité (base, id) d'un nœud du chemin (base 0 = primaire).
    fn cle_noeud(&self, n: NoeudM) -> (String, u64) {
        if n.base == 0 {
            (String::new(), n.id.0)
        } else {
            (self.base_de(n.base).0, n.id.0)
        }
    }

    /// Clé d'identité (base, id) d'une arête du chemin (base 0 = primaire).
    fn cle_arete(&self, a: AreteM) -> (String, u64) {
        if a.base == 0 {
            (String::new(), a.id.0)
        } else {
            (self.base_de(a.base).0, a.id.0)
        }
    }

    /// Valeur émise d'un nœud de chemin : primaire = `Int64` ; distant = `Map`
    /// hydratée (`remote_node_value`, convention V4). Absent du store : `Null`.
    fn valeur_noeud(&self, n: NoeudM) -> Value {
        if n.base == 0 {
            return Value::Int64(n.id.0 as i64);
        }
        let (slug, store) = self.base_de(n.base);
        remote_node_value(&slug, store.as_ref(), n.id).unwrap_or(Value::Null)
    }

    /// Valeur émise d'une arête de chemin : primaire = `Int64` ; distante = `Map`
    /// hydratée (`remote_edge_hydratee`). Absente du store : `Null`.
    fn valeur_arete(&self, a: AreteM) -> Value {
        if a.base == 0 {
            return Value::Int64(a.id.0 as i64);
        }
        let (slug, store) = self.base_de(a.base);
        remote_edge_hydratee(&slug, store.as_ref(), a.id).unwrap_or(Value::Null)
    }

    /// (obrain#189) Le chemin COMPOSE (prefixe ++ segment) respecte-t-il le mode
    /// de parcours ? Chaque segment est verifie seul par `is_expansion_allowed` ;
    /// une violation entre deux segments (noeud revisite en ACYCLIC, arete reprise
    /// en TRAIL) ne l'est pas. Le contrôle porte sur les CLÉS (base, id) de tout le
    /// chemin : deux éléments de bases distinctes de même identifiant brut ne sont
    /// jamais confondus. Une clé absente (`Null`) n'est pas comparée.
    fn chemin_compose_respecte_mode(&self, input_row: &InputRow, rang: &OutputRow) -> bool {
        let Some(colonne) = self.path_prefix_column else {
            return true;
        };
        let Some(ColumnValue::Value(Value::Path {
            nodes: prefixe_noeuds,
            edges: prefixe_aretes,
        })) = input_row.columns.get(colonne)
        else {
            return true;
        };
        // Le noeud source du segment est le dernier du prefixe : on ne le repete pas.
        let mut noeuds: Vec<Option<(String, u64)>> =
            prefixe_noeuds.iter().map(cle_valeur).collect();
        noeuds.extend(
            rang.path_nodes
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .skip(1)
                .map(|n| Some(self.cle_noeud(*n))),
        );
        let mut aretes: Vec<Option<(String, u64)>> =
            prefixe_aretes.iter().map(cle_valeur).collect();
        aretes.extend(
            rang.path_edges
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|a| Some(self.cle_arete(*a))),
        );
        let sans_doublon = |v: &[Option<(String, u64)>]| {
            let mut vus = std::collections::HashSet::new();
            v.iter().flatten().all(|cle| vus.insert(cle))
        };
        match self.path_mode {
            PathMode::Walk => true,
            PathMode::Trail => sans_doublon(&aretes),
            PathMode::Acyclic => sans_doublon(&noeuds),
            // Simple : un noeud ne se répète pas, sauf un cycle qui revient à sa source.
            PathMode::Simple => {
                let corps = if noeuds.len() > 1 && noeuds.first() == noeuds.last() {
                    &noeuds[..noeuds.len() - 1]
                } else {
                    &noeuds[..]
                };
                sans_doublon(corps)
            }
        }
    }

    /// Filtre les chemins d'une ligne d'entree selon le mode, quand un prefixe
    /// compose est configure. Sans prefixe ou en WALK : identite (aucun cout).
    fn filtrer_par_mode(&self, idx: usize, rangs: Vec<OutputRow>) -> Vec<OutputRow> {
        if self.path_prefix_column.is_none() || self.path_mode == PathMode::Walk {
            return rangs;
        }
        let Some(input_row) = self.input_rows.as_ref().and_then(|r| r.get(idx)) else {
            return rangs;
        };
        rangs
            .into_iter()
            .filter(|r| self.chemin_compose_respecte_mode(input_row, r))
            .collect()
    }

    /// Fill the output buffer with results from the next input row.
    fn fill_output_buffer(&mut self) {
        while self.output_buffer.is_empty()
            && self
                .input_rows
                .as_ref()
                .is_some_and(|rows| self.current_input_idx < rows.len())
        {
            // Une ligne d entree = une BFS complete. Sur une entree de
            // 100 000 lignes, cette boucle est le vrai moteur du cout.
            if obrain_common::utils::cancel::is_cancelled() {
                break;
            }
            let idx = self.current_input_idx;
            let (source_node, target) = {
                let row = &self.input_rows.as_ref().expect("teste ci-dessus").get(idx);
                let row = row.expect("idx < len");
                let target = self.target_column.and_then(|c| match row.columns.get(c) {
                    Some(ColumnValue::NodeId(id)) => Some(*id),
                    _ => None,
                });
                (row.source_node, target)
            };
            let reach = target.and_then(|t| self.reach_for(t).map(|r| (t, r)));
            let cle = (
                source_node.base,
                source_node.id,
                reach.as_ref().map(|(t, _)| *t),
            );
            if let Some(modele) = self.chemins_par_source.get(&cle) {
                let copies: Vec<OutputRow> = modele
                    .iter()
                    .map(|r| OutputRow {
                        input_idx: idx,
                        ..r.clone()
                    })
                    .collect();
                let copies = self.filtrer_par_mode(idx, copies);
                self.output_buffer.extend(copies);
                self.current_input_idx += 1;
                continue;
            }
            let results = self.process_input_row(
                idx,
                source_node,
                reach.as_ref().map(|(t, r)| (*t, r.as_ref())),
            );
            // Une BFS interrompue rend un partiel : jamais memorise.
            if !obrain_common::utils::cancel::is_cancelled()
                && self.lignes_en_cache + results.len() <= LIGNES_CACHE_MAX
            {
                self.lignes_en_cache += results.len();
                self.chemins_par_source.insert(cle, results.clone());
            }
            let results = self.filtrer_par_mode(idx, results);
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
        } else if self.output_edge_list {
            schema.push(LogicalType::Any); // liste des aretes du chemin
            schema.push(LogicalType::Node);
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
                if self.output_edge_list {
                    // Liste des aretes du chemin (ids bruts, base primaire —
                    // meme limitation documentee que `path_edges` en V4).
                    let liste: Vec<Value> = out_row
                        .path_edges
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| Value::Int64(a.id.0 as i64))
                        .collect();
                    col.push_value(Value::List(liste.into()));
                } else {
                    match out_row.edge_id {
                        // Chemin de longueur 0 : pas d'arete (bitmap de validite).
                        None => col.push_value(Value::Null),
                        Some(edge_id) if edge_id.base == 0 => col.push_edge_id(edge_id.id),
                        Some(edge_id) => {
                            let (slug, _) = self.base_de(edge_id.base);
                            let mut map = std::collections::BTreeMap::new();
                            map.insert(
                                PropertyKey::new("_id"),
                                Value::Int64(edge_id.id.as_u64() as i64),
                            );
                            map.insert(
                                PropertyKey::new(base_registry::BASE_KEY),
                                Value::String(slug.into()),
                            );
                            col.push_value(Value::Map(Arc::new(map)));
                        }
                    }
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

            // (obrain#189) Chemin deja parcouru par les expansions precedentes
            // du motif nomme (vide si aucun prefixe n'est configure) : sa
            // longueur s'ajoute, ses noeuds/aretes precedent ceux du segment.
            let prefixe = self
                .path_prefix_column
                .and_then(|c| match input_row.columns.get(c) {
                    Some(ColumnValue::Value(obrain_common::types::Value::Path {
                        nodes,
                        edges,
                    })) => Some((nodes, edges)),
                    _ => None,
                });

            // Add path length column if requested
            if self.output_path_length
                && let Some(col) = chunk.column_mut(num_input_cols + 2)
            {
                let prefixe_aretes = prefixe.map_or(0, |(_, aretes)| aretes.len());
                col.push_value(obrain_common::types::Value::Int64(
                    i64::from(out_row.path_length) + prefixe_aretes as i64,
                ));
            }

            // Add path detail columns if requested
            if self.output_path_detail {
                let base = num_input_cols + 2 + usize::from(self.output_path_length);
                let noeuds_segment = out_row.path_nodes.as_deref().unwrap_or(&[]);
                let aretes_segment = out_row.path_edges.as_deref().unwrap_or(&[]);

                // Le segment part de la source, qui est le dernier noeud du prefixe :
                // on ne la repete pas. Distant = Map hydratée, primaire = Int64.
                let nodes: Vec<obrain_common::types::Value> = match prefixe {
                    Some((prefixe_noeuds, _)) => prefixe_noeuds
                        .iter()
                        .cloned()
                        .chain(noeuds_segment.iter().skip(1).map(|n| self.valeur_noeud(*n)))
                        .collect(),
                    None => noeuds_segment
                        .iter()
                        .map(|n| self.valeur_noeud(*n))
                        .collect(),
                };
                let edges: Vec<obrain_common::types::Value> = match prefixe {
                    Some((_, prefixe_aretes)) => prefixe_aretes
                        .iter()
                        .cloned()
                        .chain(aretes_segment.iter().map(|a| self.valeur_arete(*a)))
                        .collect(),
                    None => aretes_segment
                        .iter()
                        .map(|a| self.valeur_arete(*a))
                        .collect(),
                };

                let nodes: Arc<[obrain_common::types::Value]> = nodes.into();
                let edges: Arc<[obrain_common::types::Value]> = edges.into();

                // Path nodes column
                if let Some(col) = chunk.column_mut(base) {
                    col.push_value(obrain_common::types::Value::List(Arc::clone(&nodes)));
                }

                // Path edges column
                if let Some(col) = chunk.column_mut(base + 1) {
                    col.push_value(obrain_common::types::Value::List(Arc::clone(&edges)));
                }

                // Value::Path column (first-class path value)
                if let Some(col) = chunk.column_mut(base + 2) {
                    col.push_value(obrain_common::types::Value::Path { nodes, edges });
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
        self.chemins_par_source.clear();
        self.lignes_en_cache = 0;
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
