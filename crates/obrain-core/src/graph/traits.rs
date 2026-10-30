//! Storage traits for the graph engine.
//!
//! These traits capture the minimal surface that query operators need from
//! the graph store. The split is intentional:
//!
//! - [`GraphStore`]: Read-only operations (scans, lookups, traversal, statistics)
//! - [`GraphStoreMut`]: Write operations (create, delete, mutate)
//!
//! Admin operations (index management, MVCC internals, schema introspection,
//! statistics recomputation, WAL recovery) stay on the concrete store
//! (post-T17: `obrain_substrate::SubstrateStore`) and are not part of these
//! traits.
//!
//! ## Design rationale
//!
//! The traits work with typed graph objects (`Node`, `Edge`, `Value`) rather
//! than raw bytes. Since T17 cutover, `SubstrateStore` (in the
//! `obrain-substrate` crate) is the canonical backend (mmap + WAL-native).
//! The traits are kept generic so legacy shims (e.g. the projection adapter)
//! and future backends can implement the same interface. Intra-doc links
//! cannot resolve `SubstrateStore` from here because `obrain-core` does not
//! depend on `obrain-substrate`.

use crate::graph::Direction;
use crate::graph::lpg::CompareOp;
use crate::graph::lpg::{Edge, Node};
use crate::statistics::Statistics;
use arcstr::ArcStr;
use obrain_common::memory::arena::AllocError;
use obrain_common::types::{EdgeId, EpochId, NodeId, PropertyKey, TransactionId, Value};
use obrain_common::utils::hash::FxHashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Aretes inter-bases (2026-08-17, spec << aretes inter-bases natives >> V1)
// ---------------------------------------------------------------------------

/// Bit haut des identifiants d'aretes EXTERNES. Les aretes inter-bases
/// vivent dans un registre sidecar du store SOURCE, avec un espace d'ids
/// SEPARE des slots d'aretes internes ; le bit haut rend les deux espaces
/// distinguables partout ou un `EdgeId` circule (suppression routee,
/// projections). Aucun store n'approche 2^63 aretes internes.
pub const EXTERNAL_EDGE_ID_BIT: u64 = 1 << 63;

/// Arete inter-bases : la cible est un noeud d'un AUTRE store, adresse par
/// `(to_host, to_store, to_key)` — une ADRESSE, pas un pointeur memoire.
///
/// - `to_host` : hote de la base cible. `None` = machine locale (seul cas
///   resolu aujourd'hui) ; `Some(..)` (nom ou URL, format libre) est
///   RESERVE pour le multi-host — encode et rejoue des maintenant pour ne
///   jamais avoir a migrer le format, resolution reseau a venir (V5).
/// - `to_store` : slug de la base cible (ex. "agent/system").
/// - `to_key` : clef du noeud cible. Si elle est entierement numerique,
///   elle designe un `NodeId` brut ; sinon la valeur de la propriete "id"
///   du noeud cible (lookup indexe T17b, O(1) quand l'index existe).
///
/// L'arete est stockee dans le store SOURCE uniquement (une seule WAL par
/// ecriture, atomicite mono-store — spec paragraphe 1). Une cible disparue ne
/// casse rien : la resolution rend le noeud absent (logique trivaluee).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEdge {
    /// Identifiant (bit haut [`EXTERNAL_EDGE_ID_BIT`] pose).
    pub id: EdgeId,
    /// Noeud source, dans CE store.
    pub from: NodeId,
    /// Type d'arete, en clair.
    pub edge_type: String,
    /// Hote de la base cible (`None` = local).
    pub to_host: Option<String>,
    /// Slug de la base cible.
    pub to_store: String,
    /// Clef du noeud cible (voir doc du type).
    pub to_key: String,
}

/// Read-only graph operations used by the query engine.
///
/// This trait captures the minimal surface that scan, expand, filter,
/// project, and shortest-path operators need. Implementations may serve
/// data from memory, disk, or a hybrid of both.
///
/// # Object safety
///
/// This trait is object-safe: you can use `Arc<dyn GraphStore>` for dynamic
/// dispatch. Traversal methods return `Vec` instead of `impl Iterator` to
/// Prédicat TEXTE évaluable sur les octets stockés, SANS matérialiser de `Value`.
///
/// Voir [`GraphStore::verdicts_texte_batch`] : c'est le canal qui permet au
/// substrat de répondre par un verdict sur les octets mmap, sans allouer de
/// `ArcStr` par cellule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicatTexte<'a> {
    /// `CONTAINS`
    Contient(&'a str),
    /// `STARTS WITH`
    CommencePar(&'a str),
    /// `ENDS WITH`
    FiniPar(&'a str),
}

impl PredicatTexte<'_> {
    /// Le verdict sur des octets, ou `None` si ce ne sont pas des octets UTF-8
    /// valides — même tolérance que `Value::as_str`.
    #[must_use]
    pub fn verdict(&self, octets: &[u8]) -> Option<bool> {
        let texte = core::str::from_utf8(octets).ok()?;
        Some(match self {
            Self::Contient(aiguille) => texte.contains(aiguille),
            Self::CommencePar(aiguille) => texte.starts_with(aiguille),
            Self::FiniPar(aiguille) => texte.ends_with(aiguille),
        })
    }
}

/// enable this.
pub trait GraphStore: Send + Sync {
    // --- Point lookups ---

    /// Returns a node by ID (latest visible version at current epoch).
    fn get_node(&self, id: NodeId) -> Option<Node>;

    /// Returns an edge by ID (latest visible version at current epoch).
    fn get_edge(&self, id: EdgeId) -> Option<Edge>;

    /// Returns a node visible to a specific transaction.
    fn get_node_versioned(
        &self,
        id: NodeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> Option<Node>;

    /// Returns an edge visible to a specific transaction.
    fn get_edge_versioned(
        &self,
        id: EdgeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> Option<Edge>;

    /// Returns a node using pure epoch-based visibility (no transaction context).
    ///
    /// The node is visible if `created_epoch <= epoch` and not deleted at or
    /// before `epoch`. Used for time-travel queries where transaction ownership
    /// must not bypass the epoch check.
    fn get_node_at_epoch(&self, id: NodeId, epoch: EpochId) -> Option<Node>;

    /// Returns an edge using pure epoch-based visibility (no transaction context).
    fn get_edge_at_epoch(&self, id: EdgeId, epoch: EpochId) -> Option<Edge>;

    // --- Property access (fast path, avoids loading full entity) ---

    /// Gets a single property from a node without loading all properties.
    fn get_node_property(&self, id: NodeId, key: &PropertyKey) -> Option<Value>;

    /// Gets a single property from an edge without loading all properties.
    fn get_edge_property(&self, id: EdgeId, key: &PropertyKey) -> Option<Value>;

    /// Gets a property for multiple nodes in a single batch operation.
    fn get_node_property_batch(&self, ids: &[NodeId], key: &PropertyKey) -> Vec<Option<Value>>;

    /// Gets all properties for multiple nodes in a single batch operation.
    fn get_nodes_properties_batch(&self, ids: &[NodeId]) -> Vec<FxHashMap<PropertyKey, Value>>;

    /// Gets selected properties for multiple nodes (projection pushdown).
    fn get_nodes_properties_selective_batch(
        &self,
        ids: &[NodeId],
        keys: &[PropertyKey],
    ) -> Vec<FxHashMap<PropertyKey, Value>>;

    /// Gets selected properties for multiple edges (projection pushdown).
    fn get_edges_properties_selective_batch(
        &self,
        ids: &[EdgeId],
        keys: &[PropertyKey],
    ) -> Vec<FxHashMap<PropertyKey, Value>>;

    // --- Traversal ---

    /// Returns neighbor node IDs in the specified direction.
    ///
    /// Returns `Vec` instead of an iterator for object safety. The underlying
    /// `ChunkedAdjacency` already produces a `Vec` internally.
    fn neighbors(&self, node: NodeId, direction: Direction) -> Vec<NodeId>;

    /// Returns (target_node, edge_id) pairs for edges from a node.
    fn edges_from(&self, node: NodeId, direction: Direction) -> Vec<(NodeId, EdgeId)>;

    /// Returns the out-degree of a node (number of outgoing edges).
    fn out_degree(&self, node: NodeId) -> usize;

    /// Returns the in-degree of a node (number of incoming edges).
    fn in_degree(&self, node: NodeId) -> usize;

    /// Returns the out-degree of a node filtered by edge type. O(1) on
    /// backends with a per-edge-type degree index (SubstrateStore
    /// T17h T8) ; default fallback returns 0 for backends that haven't
    /// wired the index, which pushes the query planner to its legacy
    /// expand+count path rather than silently dropping edges.
    ///
    /// `edge_type = None` means "sum across all types" and is
    /// equivalent to `out_degree(node)` ; it exists so planner rewrites
    /// can uniformly route through this method.
    fn out_degree_by_type(&self, node: NodeId, edge_type: Option<&str>) -> usize {
        let _ = (node, edge_type);
        0
    }

    /// Returns the in-degree of a node filtered by edge type. See
    /// [`Self::out_degree_by_type`] for contract.
    fn in_degree_by_type(&self, node: NodeId, edge_type: Option<&str>) -> usize {
        let _ = (node, edge_type);
        0
    }

    /// Returns `true` when this backend supports O(1) typed-degree
    /// lookups via [`Self::out_degree_by_type`] / [`Self::in_degree_by_type`].
    /// Query planners use this to gate the typed-degree TopK rewrite —
    /// backends that return `false` keep the slow expand+count path
    /// for correctness (the default-0 fallback would otherwise silently
    /// return empty top-K results).
    fn supports_typed_degree(&self) -> bool {
        false
    }

    /// Returns `true` when the per-edge-type degree sidecars backing
    /// [`Self::out_degree_by_type`] / [`Self::in_degree_by_type`] can be
    /// TRUSTED for `edge_type` on THIS store instance.
    ///
    /// (2026-09-05) Complète `supports_typed_degree` (capacité statique du
    /// backend) par une garantie de QUALITÉ DE DONNÉES dynamique : les
    /// sidecars `substrate.degrees.node.<TYPE>.u32` sont des fichiers mmap
    /// que des écrivains non-moteur / des binaires plus anciens (mapping de
    /// slots différent) peuvent laisser périmés — CRC valide, compteurs
    /// faux → le pré-filtre typed-degree saute alors tous les nœuds et
    /// `MATCH (a)-[:T]->(b)` renvoie 0 silencieusement (incidents prod
    /// 2026-09-01 et runtime 2026-09-05). Les backends qui auto-guérissent
    /// ou détectent la péremption renvoient `false` pour les types non
    /// fiables ; le pré-filtre/rewrite retombe alors sur le chemin générique
    /// (scan) qui reste correct. Défaut `true` (backend sans sidecars
    /// dédiés — in-memory/legacy — toujours fiable).
    fn typed_degree_reliable(&self, _edge_type: &str) -> bool {
        true
    }

    /// Returns the set of labels observed on the **target** nodes of
    /// every live edge whose type is `edge_type`. Backends with a
    /// persistent histogram (SubstrateStore T17i T2) answer in O(K)
    /// where K ≤ 64 ; the default impl scans every edge once per
    /// call (O(E)) via `all_edge_ids` + `edge_type` + `edge_endpoints` +
    /// `labels(node)`, which is correct but expensive — planners
    /// should gate their use of this method on [`Self::supports_edge_label_histogram`].
    ///
    /// Used by the T17i T3 Cypher planner rewrite to decide whether
    /// a peer-label constraint like `(imported:File)` can be safely
    /// routed through the typed-degree top-K operator : the rewrite
    /// is only accepted when the histogram contains exactly one
    /// entry matching the anchor's label.
    fn edge_target_labels(&self, edge_type: &str) -> std::collections::HashSet<String> {
        let _ = edge_type;
        std::collections::HashSet::new()
    }

    /// Symmetric accessor for **source** node labels per edge type.
    /// Same contract as [`Self::edge_target_labels`].
    fn edge_source_labels(&self, edge_type: &str) -> std::collections::HashSet<String> {
        let _ = edge_type;
        std::collections::HashSet::new()
    }

    /// `true` iff the backend maintains a persistent histogram of
    /// peer labels per edge type — i.e. `edge_target_labels` /
    /// `edge_source_labels` are O(1)/O(K) rather than O(E). Planners
    /// can use the histogram cheaply when this is `true` and should
    /// skip the feature otherwise.
    fn supports_edge_label_histogram(&self) -> bool {
        false
    }

    /// Whether backward adjacency is available for incoming edge queries.
    fn has_backward_adjacency(&self) -> bool;

    // --- Scans ---

    /// Returns all non-deleted node IDs, sorted by ID.
    fn node_ids(&self) -> Vec<NodeId>;

    /// Returns all node IDs including uncommitted/PENDING versions.
    ///
    /// Unlike `node_ids()` which pre-filters by current epoch, this method
    /// returns every node that has a version chain entry. Used by scan operators
    /// that perform their own MVCC visibility filtering (e.g. with transaction context).
    fn all_node_ids(&self) -> Vec<NodeId> {
        // Default: fall back to node_ids() for stores without MVCC
        self.node_ids()
    }

    /// Returns at most `max` non-deleted node IDs, sorted by ID — la
    /// variante PLAFONNEE de [`GraphStore::node_ids`], pendant sans etiquette
    /// de [`GraphStore::nodes_by_label_capped`].
    ///
    /// CONTRAT, et il est etroit : le resultat est EXACTEMENT le prefixe de
    /// `node_ids()` de longueur `min(max, node_ids().len())`. Meme ordre,
    /// meme predicat de vivacite. Un plafond, JAMAIS un filtre different —
    /// c'est ce qui permet a l'appelant de substituer l'un a l'autre sans
    /// changer le resultat, et c'est ce que verifie
    /// `node_ids_capped_est_un_prefixe_de_node_ids`.
    ///
    /// `max` compte les noeuds VIVANTS rendus, pas les slots parcourus : un
    /// substrat troue de tombstones (ou de creneaux jamais ecrits) doit
    /// rendre `max` noeuds, pas `max` slots.
    ///
    /// Defaut : `node_ids()` puis troncature — correct, mais sans le gain.
    /// Les backends qui balaient slot par slot doivent surcharger avec une
    /// boucle qui S'ARRETE.
    fn node_ids_capped(&self, max: usize) -> Vec<NodeId> {
        let mut all = self.node_ids();
        all.truncate(max);
        all
    }

    /// Vrai quand `filter_visible_node_ids(ids, epoch)` est L'IDENTITE sur la
    /// sortie de `node_ids()` / `all_node_ids()`, pour TOUTE epoque.
    ///
    /// POURQUOI CE PREDICAT EXISTE. `ScanOperator` plafonne son balayage
    /// AVANT de filtrer par visibilite. Si le filtre peut ensuite jeter des
    /// lignes, plafonner a k en rend moins de k — silencieusement. Le
    /// plafond sans etiquette n'est donc arme que lorsque ce predicat rend
    /// vrai.
    ///
    /// Defaut : `false` — aucun plafond. Un backend qui ne surcharge pas
    /// garde exactement le comportement d'avant.
    fn scan_visibility_is_trivial(&self) -> bool {
        false
    }

    /// Returns node IDs with a specific label.
    fn nodes_by_label(&self, label: &str) -> Vec<NodeId>;

    /// Returns at most `max` node IDs with the given label.
    /// This is the LIMIT-aware variant : on stores that scan every slot
    /// (e.g. SubstrateStore prior to T17h+), iterating with an early break
    /// saves O(N) work when `max` is much smaller than the live count.
    ///
    /// Default impl : delegate to `nodes_by_label` then truncate. Concrete
    /// backends should override with a scan-and-break loop.
    fn nodes_by_label_capped(&self, label: &str, max: usize) -> Vec<NodeId> {
        let mut all = self.nodes_by_label(label);
        all.truncate(max);
        all
    }

    /// Returns the total number of non-deleted nodes.
    fn node_count(&self) -> usize;

    /// Returns the count of nodes with a specific label (O(1)).
    fn node_count_by_label(&self, label: &str) -> usize {
        // Default fallback (O(n) via nodes_by_label); concrete backends
        // (SubstrateStore) override with an O(1) label-index lookup.
        self.nodes_by_label(label).len()
    }

    /// Returns the total number of non-deleted edges.
    fn edge_count(&self) -> usize;

    // --- Entity metadata ---

    /// Returns the type string of an edge.
    fn edge_type(&self, id: EdgeId) -> Option<ArcStr>;

    /// Returns the type string of an edge visible to a specific transaction.
    ///
    /// Falls back to epoch-based `edge_type` if not overridden.
    fn edge_type_versioned(
        &self,
        id: EdgeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> Option<ArcStr> {
        let _ = (epoch, transaction_id);
        self.edge_type(id)
    }

    // --- Aretes inter-bases (V1) ---

    /// Aretes EXTERNES sortantes du noeud — cible dans un autre store,
    /// voir [`ExternalEdge`]. Defaut : aucune ; seuls les backends qui
    /// materialisent le registre (SubstrateStore) en rendent.
    fn external_edges_from(&self, node: NodeId) -> Vec<ExternalEdge> {
        let _ = node;
        Vec::new()
    }

    /// Registre inverse (spec paragraphe 1, inbound_refs) : qui, dans CE
    /// store, pointe vers `(to_store, to_key)` sur l'hote LOCAL ?
    /// Rend des paires (source, id d'arete). Pour le janitor et les
    /// sauvegardes — jamais sur le chemin de traversee.
    fn external_inbound_refs(&self, to_store: &str, to_key: &str) -> Vec<(NodeId, EdgeId)> {
        let _ = (to_store, to_key);
        Vec::new()
    }

    /// Nombre d'aretes externes vivantes de CE store.
    fn external_edge_count(&self) -> u64 {
        0
    }

    /// (V4, statistiques d'optimiseur — besoin nomme par l'entite, spec
    /// paragraphe 8) Nombre d'aretes externes vivantes d'un TYPE donne
    /// (`None` = tous types). Le planificateur s'en sert pour estimer le
    /// degre d'expansion en session multi-store — sans elle, les plans
    /// sous-estiment les frontieres. Defaut : 0 (backends sans registre).
    fn external_edge_count_by_type(&self, edge_type: Option<&str>) -> u64 {
        let _ = edge_type;
        0
    }

    // --- Index introspection ---

    /// Returns `true` if a property index exists for the given property.
    ///
    /// The default returns `false`, which is correct for stores without indexes.
    fn has_property_index(&self, _property: &str) -> bool {
        false
    }

    /// Returns only the labels of a node without loading its properties.
    ///
    /// The default uses `get_node` and collects labels into a `Vec`; concrete
    /// backends may override with an O(1) lookup when labels live in a
    /// separate index.
    fn get_node_labels(&self, id: NodeId) -> Option<Vec<ArcStr>> {
        self.get_node(id).map(|n| n.labels.into_iter().collect())
    }

    /// Returns whether the node carries `label`, without loading properties
    /// or materialising the label list. `None` when the node does not exist.
    ///
    /// This is the hot path of `hasLabel()` — which label predicates
    /// (`WHERE s:Function OR s:Struct …`) compile to, evaluated once per
    /// label per candidate row. The default delegates to `get_node_labels`
    /// (one `Vec<ArcStr>` allocation + string compares per call); bitset
    /// backends should override with a pure bit test.
    fn node_has_label(&self, id: NodeId, label: &str) -> Option<bool> {
        self.get_node_labels(id)
            .map(|ls| ls.iter().any(|l| l.as_str() == label))
    }

    // --- Filtered search ---

    /// Finds all nodes with a specific property value. Uses indexes when available.
    fn find_nodes_by_property(&self, property: &str, value: &Value) -> Vec<NodeId>;

    /// Finds nodes matching multiple property equality conditions.
    fn find_nodes_by_properties(&self, conditions: &[(&str, Value)]) -> Vec<NodeId>;

    /// ── EXISTENCE DE PROPRIÉTÉ : LA SOURCE DE `n.p IS NOT NULL` ──────────
    ///
    /// Rend la liste EXACTE et COMPLÈTE des nœuds vivants qui portent la
    /// propriété `property`, **quelle que soit sa valeur** — ou `None`, et
    /// l'appelant BALAIE.
    ///
    /// Mesuré sur `runtime-dev` (433 618 nœuds, 2026-09-14) : `MATCH (n)
    /// WHERE n.id IS NOT NULL RETURN count(n)` coûtait **514 ms et 433 633
    /// slots lus** pour rendre **110** lignes. Aucun chemin ne servait
    /// l'existence : `IS NOT NULL` compile en `UnaryFilterOp::IsNotNull`, un
    /// filtre d'expression nu, au-dessus d'un balayage complet.
    ///
    /// CONTRAIREMENT À L'ÉGALITÉ, l'existence n'a pas de valeur à hacher : la
    /// seule source d'index qui la tranche est l'énumération COMPLÈTE d'une
    /// série de propriété. C'est ce que portent les zones d'index déclarées —
    /// d'où le contrat, identique à celui des index de propriété :
    ///
    /// * `Some(v)` : liste exacte, revérifiée sur les valeurs courantes, des
    ///   nœuds vivants portant la propriété. Elle peut être VIDE — quand
    ///   toutes les entrées d'une série fiable se sont révélées périmées
    ///   (nœuds morts, propriétés retirées depuis la dernière fusion) ;
    /// * `None` : « je ne sais pas répondre sans balayer » — propriété non
    ///   déclarée, zone absente ou non fiable (ancres divergentes), série
    ///   illisible, ou série VIDE (ambiguë entre « aucun nœud ne la porte »
    ///   et « fusion sans contenu »). Jamais `Some(vec![])` par ignorance :
    ///   un ensemble vide rendu par ignorance affirme « zéro », définitif et
    ///   sans erreur, là où le balayage rendrait la vérité.
    ///
    /// Défaut : `None` — ne rien accélérer est toujours sûr, et les backends
    /// sans série de propriété gardent le balayage d'origine.
    fn find_nodes_with_property(&self, _property: &str) -> Option<Vec<NodeId>> {
        None
    }

    /// Verdicts d'un prédicat TEXTE sur une propriété, pour tout un LOT.
    ///
    /// POURQUOI (2026-09-12). `CONTAINS` / `STARTS WITH` / `ENDS WITH` ne
    /// s'intéressent qu'aux OCTETS d'une chaîne. Les matérialiser en
    /// `Value::String` alloue un `ArcStr` par cellule présente — mesuré ~1,1 ms
    /// pour 2 000 cellules sur un banc contrôlé, dont la moitié est cette
    /// allocation et son `free` — alors que le test lui-même coûte quelques ns
    /// sur une tranche empruntée à l'arène. Cette méthode rend donc le VERDICT,
    /// sans rien allouer.
    ///
    /// SÉMANTIQUE — celle de l'évaluateur, pas une approximation : il fait
    /// `left.as_str()?` puis `l.contains(r)`. Une valeur absente ou NON
    /// textuelle rend donc NULL (jamais `false`), ce que `None` signifie ici.
    ///
    /// CONTRAT DE RETOUR (tri-valué, pour ne rien confondre) :
    ///   - `None`               : pas de verdict — l'appelant SUIT son chemin
    ///                            d'origine (donc aucune divergence possible) ;
    ///   - `Some(v)` avec `v[i]` = `None`     : NULL pour la ligne `i` ;
    ///   - `Some(v)` avec `v[i]` = `Some(b)`  : verdict du prédicat.
    ///
    /// Le défaut rend `None` : les implémentations qui ne savent pas répondre
    /// sans allouer (projections, wrappers) gardent le chemin d'origine.
    ///
    /// PLUSIEURS PRÉDICATS EN UNE FOIS — et c'est une correction de mesure, pas
    /// une commodité. `n.name CONTAINS 'a' OR n.name CONTAINS 'b'` demandait
    /// quatre verdicts sur la MÊME propriété ; les demander séparément faisait
    /// quatre traversées de chaîne par cellule, là où la lecture de valeurs en
    /// faisait UNE (mesuré : 270 ms contre 218 ms pour 347 970 cellules).
    /// Le substrat rend donc un verdict PAR PRÉDICAT, en une seule traversée.
    fn verdicts_texte_batch(
        &self,
        _ids: &[NodeId],
        _key: &PropertyKey,
        _predicats: &[PredicatTexte<'_>],
    ) -> Option<Vec<Vec<Option<bool>>>> {
        None
    }

    /// L'implémentation peut-elle RÉPONDRE par index d'égalité sur cette
    /// propriété, même quand l'index n'est pas encore bâti ?
    ///
    /// POURQUOI CETTE QUESTION EXISTE, À CÔTÉ DE `has_property_index`. Le
    /// substrat sait CONSTRUIRE l'index d'égalité d'une propriété à la demande
    /// (`find_nodes_by_property` le fait déjà : « la construction reste payée
    /// UNE fois par clé et par processus ; mesuré 80 ms au 1er appel puis
    /// 429 µs »). Mais le planificateur ne demandait que `has_property_index`,
    /// qui est FAUX tant que la construction n'a pas eu lieu : il choisissait
    /// donc un balayage dont le résultat ne rebâtit jamais l'index. Cercle
    /// vicieux mesuré sur le store vivant — `MATCH (m:Message) WHERE m.session
    /// = … AND m.role = 'user'`, la requête du CHAT : **219 ms** en processus
    /// long, 502 ms au CLI, à chaque appel.
    ///
    /// MESURÉ : la même requête, `session` DÉCLARÉ, tombe à **0,27 ms**. Ce
    /// n'était donc pas un problème de données mais de porte.
    ///
    /// Le défaut rend `false` : aucune implémentation n'est présumée capable de
    /// construire un index qu'elle n'a pas.
    fn peut_indexer_propriete(&self, _propriete: &str) -> bool {
        false
    }

    /// Les nœuds qui satisfont un prédicat TEXTE, quand l'implémentation sait y
    /// répondre par un INDEX au lieu de balayer la propriété entière.
    ///
    /// POURQUOI CE CANAL EXISTE. `verdicts_texte_batch` rend le prédicat MOINS
    /// CHER par ligne ; il ne réduit pas le NOMBRE de lignes. Sur le banc figé
    /// (347 970 nœuds), `n.name CONTAINS 'iam'` coûte 148–190 ms de balayage
    /// pour 12 lignes rendues. Un index de sous-chaînes (trigrammes) rend ces
    /// 12 nœuds en quelques microsecondes — mesuré — mais le planificateur
    /// n'avait aucun moyen de le savoir : toutes ses sondes d'index passent par
    /// ce trait.
    ///
    /// CONTRAT DE RETOUR :
    ///   - `None`        : l'implémentation NE SAIT PAS (pas d'index pour cette
    ///                     propriété, index muet faute de grammes assez rares,
    ///                     ou index débordé) — l'appelant garde son chemin
    ///                     d'origine, balayage compris, donc aucune divergence
    ///                     n'est possible ;
    ///   - `Some(vide)`  : CONCLUSIF — l'index a PROUVÉ qu'aucun nœud ne
    ///                     satisfait le prédicat ;
    ///   - `Some(nœuds)` : les nœuds satisfaisant le prédicat, EXACTEMENT —
    ///                     l'implémentation revérifie ses candidats, comme le
    ///                     fait `find_nodes_in_range`. L'appelant n'a donc pas
    ///                     à réévaluer le prédicat.
    ///
    /// Le défaut rend `None` : toute implémentation sans index de sous-chaînes
    /// (projections, wrappers, magasins en mémoire) garde le chemin d'origine.
    fn noeuds_par_predicat_texte(
        &self,
        _key: &PropertyKey,
        _predicat: &PredicatTexte<'_>,
    ) -> Option<Vec<NodeId>> {
        None
    }

    /// La propriété a-t-elle une série ORDONNÉE EXPLOITABLE (déclarée, fiable,
    /// construite) ? — la question que doit poser toute réécriture en intervalle.
    ///
    /// POURQUOI CETTE GARDE EXISTE (2026-09-13, mesuré). Réécrire un prédicat en
    /// intervalle sur une propriété SANS index ordonné ne rend rien plus rapide :
    /// `find_nodes_in_range` retombe alors sur un balayage vérifié par
    /// `valeur_dans_intervalle`, mesuré **290 ms contre 65 ms** pour le filtre
    /// d'expression — sur `name` du snapshot figé, la réécriture transformait un
    /// gain en régression de 4,5×. Avec la garde : ×250 quand l'index sert
    /// (`code` court, préfixe sélectif), ×1,5 sur un préfixe de chemin indexé,
    /// et AUCUN changement quand l'index ne peut pas servir.
    ///
    /// Le défaut répond `false` : ne rien réécrire est toujours sûr, et les
    /// implémentations sans index ordonné (projections, wrappers) gardent donc
    /// le chemin d'origine.
    fn index_ordonne_pour(&self, _propriete: &str) -> bool {
        false
    }

    /// L'index ordonné est-il **déjà** là, sans rien construire ?
    ///
    /// ── POURQUOI CETTE GARDE EXISTE À CÔTÉ DE `index_ordonne_pour` ─────────
    /// Défaut mesuré le 2026-09-13 sur le chat du 8432 (store `runtime-dev`,
    /// 408 832 nœuds) : la réécriture de préfixe consultait `index_ordonne_pour`,
    /// qui CONSTRUIT la série la première fois — un balayage complet des chaînes
    /// de propriétés. Le premier préfixe d'un processus coûtait donc **3 027 à
    /// 16 315 ms** (`MATCH (n:Class) WHERE n.qualified_name STARTS WITH …`, une
    /// clé de cache distincte par mesure), là où le filtre d'expression répond en
    /// **65 ms** et le balayage vérifié en ~200–350 ms. C'était le pire des deux
    /// mondes : payer un balayage complet pour, ensuite, emprunter un chemin que
    /// l'auteur a lui-même mesuré **4,5× plus lent** quand la clé de l'index est
    /// trop courte pour être sélective (ici 11 octets tronqués, que TOUS les
    /// `qualified_name` d'un projet partagent).
    ///
    /// Cette garde ne construit RIEN : elle dit seulement si la série est déjà
    /// disponible (étage mémoire complet, ou série fiable sur disque). Le jour où
    /// l'entretien l'a écrite, le chemin d'intervalle redevient disponible — et
    /// c'est le bon moment pour le payer, pas au milieu d'une question.
    ///
    /// Le défaut répond `false` : on balaie, c'est-à-dire le chemin d'origine.
    fn index_ordonne_disponible(&self, _propriete: &str) -> bool {
        false
    }

    /// Finds nodes whose property value falls within a range.
    fn find_nodes_in_range(
        &self,
        property: &str,
        min: Option<&Value>,
        max: Option<&Value>,
        min_inclusive: bool,
        max_inclusive: bool,
    ) -> Vec<NodeId>;

    /// (NS-PARTOUT) Réponse EXACTE et COMPLÈTE à `property CONTAINS needle`
    /// (sensible à la casse, sémantique `str::contains`), quand une source
    /// indexée complète existe — sinon `None`, et l'appelant BALAIE.
    ///
    /// Le contrat est celui des index de propriété : `Some(v)` est la
    /// liste exacte, revérifiée sur les valeurs courantes, des nœuds
    /// vivants dont la valeur CHAÎNE contient `needle` ; `None` veut dire
    /// « je ne sais pas répondre sans balayer », jamais « zéro résultat ».
    /// À distinguer de `find_nodes_by_label_property_contains_bounded`,
    /// heuristique insensible à la casse et PLAFONNÉE, qui ne promet pas
    /// la complétude.
    ///
    /// Défaut : `None` — seuls les backends portant un index adapté
    /// (trigrammes substrate) surchargent.
    fn find_nodes_by_property_contains(
        &self,
        _property: &str,
        _needle: &str,
    ) -> Option<Vec<NodeId>> {
        None
    }

    /// (NATIF-J6) `min(property)` / `max(property)` en O(1) via l'index ORDONNE
    /// PAR VALEUR, quand il est bati et fiable — sinon `None`, et l'appelant
    /// retombe sur l'agregat balayant. Les NULL sont exclus (comme l'agregat).
    ///
    /// Defaut `None` : seuls les backends portant l'index ordonne par valeur
    /// (substrate) surchargent.
    fn min_par_index(&self, _property: &str) -> Option<Value> {
        None
    }

    /// Voir [`GraphStore::min_par_index`].
    fn max_par_index(&self, _property: &str) -> Option<Value> {
        None
    }

    /// (NATIF-J6) Les `k` NodeId de plus PETITE valeur de `property`, en ordre
    /// ASCENDANT du moteur (`comparer_total`), via l'index ordonne par valeur ;
    /// `None` => l'appelant retombe sur le tri complet. Ne rend que des nœuds
    /// VIVANTS a la construction ; l'appelant filtre encore la visibilite MVCC.
    fn premiers_k_ordonnes(&self, _property: &str, _k: usize) -> Option<Vec<NodeId>> {
        None
    }

    /// (NATIF-J6) Les `k` NodeId de plus GRANDE valeur de `property`, rendus en
    /// ordre ASCENDANT (l'appelant DESC re-trie par le vrai comparateur) ;
    /// `None` => tri complet.
    fn derniers_k_ordonnes(&self, _property: &str, _k: usize) -> Option<Vec<NodeId>> {
        None
    }

    /// (NATIF-J6) Nombre de valeurs non-null indexees par ORDRE pour
    /// `property` (taille de la serie de l'index ordonne par valeur), ou `None`
    /// si l'index n'est pas bati/fiable. Sert la garde "aucun NULL" de la route
    /// ORDER BY : compare a `node_count()`, l'egalite prouve que tous les noeuds
    /// portent la propriete (donc l'ordre ne place aucun NULL dans le top-k).
    fn nb_valeurs_ordonnees(&self, _property: &str) -> Option<usize> {
        None
    }

    /// Finds nodes with a substring match on a property, optionally filtered by label.
    ///
    /// The search is case-insensitive. `max_scan` caps the total number of nodes
    /// inspected to prevent worst-case O(N) full scans. Returns early when either
    /// `limit` results are found **or** `max_scan` nodes have been inspected.
    ///
    /// Default implementation returns empty — this is LpgStore-inherent for now.
    /// SubstrateStore exposes substring search natively through its L2 inverted
    /// index; this default keeps the trait object-safe while letting concrete
    /// backends override.
    fn find_nodes_by_label_property_contains_bounded(
        &self,
        _label: Option<&str>,
        _property: &str,
        _substring: &str,
        _limit: usize,
        _max_scan: usize,
    ) -> Vec<NodeId> {
        Vec::new()
    }

    // --- Auxiliary index registries (text/vector) ---

    /// Returns all registered text (BM25) index entries as `(key, index)` pairs.
    ///
    /// The key format is `"label:property"`. Default returns empty for stores
    /// that do not maintain a BM25 registry. `LpgStore` overrides; in substrate
    /// mode this stays empty because retrieval is served by the substrate L2
    /// inverted index + SYNAPSE layer natively.
    #[cfg(feature = "text-index")]
    fn text_index_entries(
        &self,
    ) -> Vec<(
        String,
        Arc<parking_lot::RwLock<crate::index::text::InvertedIndex>>,
    )> {
        Vec::new()
    }

    /// Returns all registered vector (HNSW) index entries as `(key, index)` pairs.
    ///
    /// The key format is `"label:property"`. Default returns empty. `LpgStore`
    /// overrides; substrate-backed stores expose vector retrieval through their
    /// native L1 vector store instead.
    #[cfg(feature = "vector-index")]
    fn vector_index_entries(&self) -> Vec<(String, Arc<crate::index::vector::HnswIndex>)> {
        Vec::new()
    }

    /// Retrieves the vector index for a label+property pair, if any.
    ///
    /// Default returns `None`. LpgStore overrides to look up its HNSW registry.
    #[cfg(feature = "vector-index")]
    fn get_vector_index(
        &self,
        _label: &str,
        _property: &str,
    ) -> Option<Arc<crate::index::vector::HnswIndex>> {
        None
    }

    /// Retrieves the text (BM25) index for a label+property pair, if any.
    ///
    /// Default returns `None`. LpgStore overrides to look up its text-index
    /// registry. Substrate surfaces equivalent functionality through its
    /// tiered-storage inverted index — callers interested in full-text
    /// retrieval should dispatch on `Arc<SubstrateStore>` directly in
    /// substrate-mode paths.
    #[cfg(feature = "text-index")]
    fn get_text_index(
        &self,
        _label: &str,
        _property: &str,
    ) -> Option<Arc<parking_lot::RwLock<crate::index::text::InvertedIndex>>> {
        None
    }

    /// Registers a pre-built vector (HNSW) index for a label+property pair.
    /// Default no-op — backends without named vector indexes ignore the
    /// registration.
    #[cfg(feature = "vector-index")]
    fn add_vector_index(
        &self,
        _label: &str,
        _property: &str,
        _index: Arc<crate::index::vector::HnswIndex>,
    ) {
    }

    /// Removes the vector index for a label+property pair. Returns `false`
    /// if none existed. Default returns `false`.
    #[cfg(feature = "vector-index")]
    fn remove_vector_index(&self, _label: &str, _property: &str) -> bool {
        false
    }

    /// Registers a pre-built text (BM25) index for a label+property pair.
    /// Default no-op.
    #[cfg(feature = "text-index")]
    fn add_text_index(
        &self,
        _label: &str,
        _property: &str,
        _index: Arc<parking_lot::RwLock<crate::index::text::InvertedIndex>>,
    ) {
    }

    /// Removes the text index for a label+property pair. Returns `false`
    /// if none existed. Default returns `false`.
    #[cfg(feature = "text-index")]
    fn remove_text_index(&self, _label: &str, _property: &str) -> bool {
        false
    }

    // --- Zone maps (skip pruning) ---

    /// Returns `true` if a node property predicate might match any nodes.
    /// Uses zone maps for early filtering.
    fn node_property_might_match(
        &self,
        property: &PropertyKey,
        op: CompareOp,
        value: &Value,
    ) -> bool;

    /// Returns `true` if an edge property predicate might match any edges.
    fn edge_property_might_match(
        &self,
        property: &PropertyKey,
        op: CompareOp,
        value: &Value,
    ) -> bool;

    // --- Statistics (for cost-based optimizer) ---

    /// Returns the current statistics snapshot (cheap Arc clone).
    fn statistics(&self) -> Arc<Statistics>;

    /// Estimates cardinality for a label scan.
    fn estimate_label_cardinality(&self, label: &str) -> f64;

    /// Estimates average degree for an edge type.
    fn estimate_avg_degree(&self, edge_type: &str, outgoing: bool) -> f64;

    // --- Epoch ---

    /// Returns the current MVCC epoch.
    fn current_epoch(&self) -> EpochId;

    // --- Schema introspection ---

    /// Returns all label names in the database.
    fn all_labels(&self) -> Vec<String> {
        Vec::new()
    }

    /// Returns all edge type names in the database.
    fn all_edge_types(&self) -> Vec<String> {
        Vec::new()
    }

    /// Returns all property key names used in the database.
    fn all_property_keys(&self) -> Vec<String> {
        Vec::new()
    }

    /// Returns the number of LIVE edges per edge type, or `None` when this
    /// store cannot count them.
    ///
    /// `None` is NOT zero, and the distinction is the whole point. The
    /// previous census asked the (now dead) `LpgStore` for
    /// `edges_with_type(t)`, got an empty `Vec`, and reported `count: 0` for
    /// edge types that had thousands of live edges. Nothing failed; the
    /// number was simply invented. An agent reading that census concluded it
    /// had no working way to create edges — while the very same database was
    /// answering `MATCH ()-[r]->()` with twelve distinct types.
    ///
    /// So: a store that does not know must SAY it does not know, and callers
    /// must render the absence (`?`, `null`) rather than print a zero they
    /// were never given. One call returns the whole map — asking per type
    /// would re-walk every edge once per type.
    fn edge_counts_by_type(&self) -> Option<std::collections::HashMap<String, usize>> {
        None
    }

    /// Comptes par type d'arête **tels que le PARCOURS générique les voit** —
    /// la clé exacte de `MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS c`.
    ///
    /// ── POURQUOI `edge_counts_by_type` NE SUFFIT PAS ────────────────────
    ///
    /// `edge_counts_by_type` compte une arête dès que son enregistrement n'est
    /// pas tombstoné et que ses deux extrémités sont non nulles. Ce n'est PAS
    /// ce que voit `MATCH ()-[r]->()` : l'expansion part des **nœuds vivants**
    /// (`node_ids()`), donc une arête dont la SOURCE est tombstonée n'est
    /// jamais atteinte — pendant qu'une arête dont la CIBLE est nulle, elle,
    /// est bien émise (`edges_from` ne filtre pas la cible).
    ///
    /// MESURÉ sur `runtime-dev` figé (1,29 M arêtes), le 2026-09-14 :
    ///
    ///   `count(r)` (compteur)                        = 1 158 516
    ///   somme des groupes PAR LE PARCOURS            = 1 151 146
    ///   somme des groupes PAR `edge_counts_by_type`  = 1 158 516
    ///
    /// Six types divergeaient DANS LES DEUX SENS (`EMITTED_BY` 44 vs 138,
    /// `EVOLVES` 13 vs 2, `HAS_STATE` 10 050 vs 17 445…). Remplacer le parcours
    /// par `edge_counts_by_type` aurait donc changé la réponse — le gain ne
    /// vaut pas un compteur qui se met à dire autre chose que la requête qu'il
    /// remplace.
    ///
    /// ── CE QUE CETTE MÉTHODE FAIT, ET CE QUE ÇA COÛTE ───────────────────
    ///
    /// Elle reproduit le parcours à la lettre : une arête compte si son
    /// enregistrement n'est pas tombstoné ET si son nœud source est vivant (la
    /// cible n'est PAS regardée — le parcours ne la regarde pas non plus). La
    /// vivacité est d'abord matérialisée en UN balayage séquentiel de la zone
    /// nœuds (un bit par emplacement), puis testée par un accès tableau pendant
    /// la passe arêtes : aucune lecture aléatoire, aucun verrou supplémentaire.
    /// Le coût mesuré reste celui d'un seul parcours de zone.
    ///
    /// `None` = « ce magasin ne sait pas le faire » ; l'appelant repasse alors
    /// par le plan générique. Même contrat que [`GraphStore::edge_counts_by_type`].
    fn walked_edge_counts_by_type(&self) -> Option<std::collections::HashMap<String, usize>> {
        None
    }

    /// Compte les nœuds VIVANTS par PREMIER label — la clé exacte de
    /// `RETURN labels(n)[0] AS l, count(n) AS c`, en UN SEUL balayage.
    ///
    /// ── POURQUOI CETTE MÉTHODE EXISTE, À CÔTÉ DE `node_count_by_label` ──
    ///
    /// `node_count_by_label` compte les nœuds **qui portent** le label. La
    /// requête d'agrégat compte chaque nœud **UNE fois**, sous son PREMIER
    /// label. Les deux questions ne coïncident que si aucun nœud ne porte deux
    /// labels — ce qui est FAUX : mesuré le 2026-09-14 sur `runtime-dev`,
    /// 15 nœuds portent `[Task, Node]` (14) ou `[Step, Task]` (1).
    ///
    /// Somme des `node_count_by_label` = 422 831 ; `count(n)` = 422 816. Les
    /// 15 nœuds à deux labels seraient comptés DEUX fois, et le total des
    /// groupes dépasserait le total des nœuds. Un `label → count` bâti sur
    /// `node_count_by_label` est donc un agrégat FAUX, pas seulement imprécis.
    ///
    /// La clé ici est celle du moteur : le premier bit ALLUMÉ QUI A UN NOM dans
    /// le registre — c'est exactement ce que rend `labels_for_bitset(...)[0]`,
    /// donc `labels(n)[0]`. Un nœud sans label rend `None`, qui est la clé
    /// `null` du group-by ; l'absence de label n'est pas l'absence de réponse,
    /// et les deux ne se confondent pas avec « ce magasin ne sait pas ».
    ///
    /// `None` (défaut) = « ce magasin ne sait pas le faire » : l'appelant
    /// renonce au raccourci et repasse par le balayage générique. Même contrat
    /// que [`GraphStore::edge_counts_by_type`] : `None` n'est pas « zéro ».
    fn node_counts_by_first_label(&self) -> Option<Vec<(Option<String>, usize)>> {
        None
    }

    // --- Visibility checks (fast path, avoids building full entities) ---

    /// Vrai quand des écritures transactionnelles NON COMMITÉES sont en
    /// attente dans ce store — c'est-à-dire quand « combien de nœuds ? »
    /// n'a plus une réponse unique : elle dépend du lecteur.
    ///
    /// Les raccourcis qui répondent par un COMPTEUR GLOBAL (`node_count`,
    /// `node_count_by_label`, `edge_count`) doivent se désarmer tant que
    /// cette méthode rend vrai, et repasser par un balayage filtré : le
    /// compteur inclut les écritures en attente de TOUTES les transactions,
    /// alors que chaque lecteur ne doit voir que les siennes. Défaut mesuré
    /// (gql_13) : le compte annonçait deux nœuds quand le balayage n'en
    /// rendait qu'un.
    ///
    /// Le défaut (`false`) convient aux stores sans notion d'écriture en
    /// attente : tout y est commité à l'instant où c'est écrit.
    fn has_pending_writes(&self) -> bool {
        false
    }

    /// (NATIF-J8) L'index ORDONNE PAR VALEUR (`substrate.props.ord.idx.v2`)
    /// reflete-t-il TOUT l'etat committe COURANT ? Confrontation FRAICHE des
    /// ancres (offset WAL + pages v2 posees a la derniere fusion) contre l'etat
    /// present, en LECTURE seule. `true` autorise la route `ORDER BY LIMIT k` /
    /// `min`/`max` a servir l'index sans risque de ligne commitee manquante ou
    /// disparue ; `false` des qu'une ecriture (meme auto-commit, qui n'arme pas
    /// `has_pending_writes`) a suivi la derniere fusion — la route doit alors
    /// balayer, lent mais jamais faux.
    ///
    /// Defaut `false` : seuls les backends portant cet index (substrate) le
    /// surchargent ; ailleurs la route ordonnee ne s'engage pas de toute facon.
    fn index_ord_valeur_couvre_courant(&self) -> bool {
        false
    }

    /// Checks if a node is visible at the given epoch without building the full Node.
    ///
    /// More efficient than `get_node_at_epoch(...).is_some()` because it skips
    /// label and property loading. Override in concrete stores for optimal
    /// performance.
    fn is_node_visible_at_epoch(&self, id: NodeId, epoch: EpochId) -> bool {
        self.get_node_at_epoch(id, epoch).is_some()
    }

    /// Checks if a node is visible to a specific transaction without building
    /// the full Node.
    fn is_node_visible_versioned(
        &self,
        id: NodeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> bool {
        self.get_node_versioned(id, epoch, transaction_id).is_some()
    }

    /// Checks if an edge is visible at the given epoch without building the full Edge.
    ///
    /// More efficient than `get_edge_at_epoch(...).is_some()` because it skips
    /// type name resolution and property loading. Override in concrete stores
    /// for optimal performance.
    fn is_edge_visible_at_epoch(&self, id: EdgeId, epoch: EpochId) -> bool {
        self.get_edge_at_epoch(id, epoch).is_some()
    }

    /// Checks if an edge is visible to a specific transaction without building
    /// the full Edge.
    fn is_edge_visible_versioned(
        &self,
        id: EdgeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> bool {
        self.get_edge_versioned(id, epoch, transaction_id).is_some()
    }

    /// Filters node IDs to only those visible at the given epoch (batch).
    ///
    /// More efficient than per-node calls because implementations can hold
    /// a single lock for the entire batch.
    fn filter_visible_node_ids(&self, ids: &[NodeId], epoch: EpochId) -> Vec<NodeId> {
        ids.iter()
            .copied()
            .filter(|id| self.is_node_visible_at_epoch(*id, epoch))
            .collect()
    }

    /// Filters node IDs to only those visible to a transaction (batch).
    fn filter_visible_node_ids_versioned(
        &self,
        ids: &[NodeId],
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> Vec<NodeId> {
        ids.iter()
            .copied()
            .filter(|id| self.is_node_visible_versioned(*id, epoch, transaction_id))
            .collect()
    }

    // --- History ---

    /// Returns all versions of a node with their creation/deletion epochs, newest first.
    ///
    /// Each entry is `(created_epoch, deleted_epoch, Node)`. Properties and labels
    /// reflect the current state (they are not versioned per-epoch).
    ///
    /// Default returns empty (not all backends track version history).
    fn get_node_history(&self, _id: NodeId) -> Vec<(EpochId, Option<EpochId>, Node)> {
        Vec::new()
    }

    /// Returns all versions of an edge with their creation/deletion epochs, newest first.
    ///
    /// Each entry is `(created_epoch, deleted_epoch, Edge)`. Properties reflect
    /// the current state (they are not versioned per-epoch).
    ///
    /// Default returns empty (not all backends track version history).
    fn get_edge_history(&self, _id: EdgeId) -> Vec<(EpochId, Option<EpochId>, Edge)> {
        Vec::new()
    }
}

/// Write operations for graph mutation.
///
/// Separated from [`GraphStore`] so read-only wrappers (snapshots, read
/// replicas) can implement only `GraphStore`. Any mutable store is also
/// readable via the supertrait bound.
pub trait GraphStoreMut: GraphStore {
    /// (2026-09-05) Verrou de sérialisation du composé MERGE
    /// find→create→re-query (unicité sous sessions concurrentes). Défaut :
    /// pas de verrou ; les impls concernées (WalGraphStore par db) renvoient
    /// un mutex PAR STORE. Object-safe (pas de générique).
    fn merge_guard(&self) -> Option<std::sync::Arc<std::sync::Mutex<()>>> {
        None
    }

    // --- Node creation ---

    /// Creates a new node with the given labels.
    fn create_node(&self, labels: &[&str]) -> NodeId;

    /// Les étiquettes demandées peuvent-elles être portées par un nœud NEUF ?
    ///
    /// ── POURQUOI CE CONTRÔLE EXISTE, ET POURQUOI IL EST SÉPARÉ ───────────────
    /// `create_node` est INFAILLIBLE par contrat (des centaines d'appelants) :
    /// le substrat ne peut donc pas lui rendre une erreur. Or il existe une
    /// limite RÉELLE — le `label_bitset` d'un `NodeRecord` fait 64 bits, donc
    /// 64 étiquettes distinctes par base — et dépasser cette limite faisait
    /// **abandonner le processus** (`.expect("label registry overflow (>64
    /// labels)")`). Une requête — ou un agent avec droit d'écriture — pouvait
    /// donc tuer le démon.
    ///
    /// Ce contrôle est ADDITIF et FACULTATIF (défaut : « oui ») pour deux
    /// raisons : il ne change pas une signature que tout le dépôt utilise, et il
    /// ne prétend pas remplacer la garde profonde (le substrat continue de
    /// refuser l'internement). Il donne à l'appelant qui PEUT rendre une erreur
    /// (l'exécuteur de requête) le moyen de refuser PROPREMENT au lieu de
    /// mourir.
    ///
    /// # Errors
    ///
    /// Une chaîne EXPLICATIVE quand les étiquettes ne pourraient pas être
    /// internées (le message est destiné à l'utilisateur, pas au journal).
    fn valider_etiquettes(&self, _labels: &[&str]) -> Result<(), String> {
        Ok(())
    }

    /// Creates a new node within a transaction context.
    fn create_node_versioned(
        &self,
        labels: &[&str],
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> NodeId;

    // --- Edge creation ---

    /// Creates a new edge between two nodes.
    fn create_edge(&self, src: NodeId, dst: NodeId, edge_type: &str) -> EdgeId;

    /// Creates a new edge within a transaction context.
    fn create_edge_versioned(
        &self,
        src: NodeId,
        dst: NodeId,
        edge_type: &str,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> EdgeId;

    /// Creates multiple edges in batch (single lock acquisition).
    fn batch_create_edges(&self, edges: &[(NodeId, NodeId, &str)]) -> Vec<EdgeId>;

    // --- Aretes inter-bases (V1) ---

    /// Cree une arete EXTERNE `src -[edge_type]-> (to_host, to_store,
    /// to_key)`, stockee dans CE store (le store SOURCE — une seule WAL,
    /// atomicite mono-store, spec paragraphe 1). `to_host = None` = hote local.
    /// Rend `None` quand le backend ne materialise pas le registre :
    /// l'appelant en fait une erreur enseignante, jamais un silence.
    fn create_external_edge(
        &self,
        src: NodeId,
        edge_type: &str,
        to_host: Option<&str>,
        to_store: &str,
        to_key: &str,
    ) -> Option<EdgeId> {
        let _ = (src, edge_type, to_host, to_store, to_key);
        None
    }

    /// Supprime une arete externe par id (bit haut
    /// [`EXTERNAL_EDGE_ID_BIT`]). Rend `false` si inconnue.
    fn delete_external_edge(&self, id: EdgeId) -> bool {
        let _ = id;
        false
    }

    // --- Deletion ---

    /// Deletes a node. Returns `true` if the node existed.
    fn delete_node(&self, id: NodeId) -> bool;

    /// Deletes a node within a transaction context. Returns `true` if the node existed.
    fn delete_node_versioned(
        &self,
        id: NodeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> bool;

    /// Deletes all edges connected to a node (DETACH DELETE).
    fn delete_node_edges(&self, node_id: NodeId);

    /// Deletes an edge. Returns `true` if the edge existed.
    fn delete_edge(&self, id: EdgeId) -> bool;

    /// Deletes an edge within a transaction context. Returns `true` if the edge existed.
    fn delete_edge_versioned(
        &self,
        id: EdgeId,
        epoch: EpochId,
        transaction_id: TransactionId,
    ) -> bool;

    // --- Property mutation ---

    /// Sets a property on a node.
    fn set_node_property(&self, id: NodeId, key: &str, value: Value);

    /// Sets a property on an edge.
    fn set_edge_property(&self, id: EdgeId, key: &str, value: Value);

    /// Sets a node property within a transaction, recording the previous value
    /// so it can be restored on rollback.
    ///
    /// Default delegates to [`set_node_property`](Self::set_node_property).
    fn set_node_property_versioned(
        &self,
        id: NodeId,
        key: &str,
        value: Value,
        _transaction_id: TransactionId,
    ) {
        self.set_node_property(id, key, value);
    }

    /// Sets an edge property within a transaction, recording the previous value
    /// so it can be restored on rollback.
    ///
    /// Default delegates to [`set_edge_property`](Self::set_edge_property).
    fn set_edge_property_versioned(
        &self,
        id: EdgeId,
        key: &str,
        value: Value,
        _transaction_id: TransactionId,
    ) {
        self.set_edge_property(id, key, value);
    }

    /// Removes a property from a node. Returns the previous value if it existed.
    fn remove_node_property(&self, id: NodeId, key: &str) -> Option<Value>;

    /// Removes a property from an edge. Returns the previous value if it existed.
    fn remove_edge_property(&self, id: EdgeId, key: &str) -> Option<Value>;

    /// Removes a node property within a transaction, recording the previous value
    /// so it can be restored on rollback.
    ///
    /// Default delegates to [`remove_node_property`](Self::remove_node_property).
    fn remove_node_property_versioned(
        &self,
        id: NodeId,
        key: &str,
        _transaction_id: TransactionId,
    ) -> Option<Value> {
        self.remove_node_property(id, key)
    }

    /// Removes an edge property within a transaction, recording the previous value
    /// so it can be restored on rollback.
    ///
    /// Default delegates to [`remove_edge_property`](Self::remove_edge_property).
    fn remove_edge_property_versioned(
        &self,
        id: EdgeId,
        key: &str,
        _transaction_id: TransactionId,
    ) -> Option<Value> {
        self.remove_edge_property(id, key)
    }

    // --- Label mutation ---

    /// Adds a label to a node. Returns `true` if the label was new.
    fn add_label(&self, node_id: NodeId, label: &str) -> bool;

    /// Removes a label from a node. Returns `true` if the label existed.
    fn remove_label(&self, node_id: NodeId, label: &str) -> bool;

    /// Adds a label within a transaction, recording the change for rollback.
    ///
    /// Default delegates to [`add_label`](Self::add_label).
    fn add_label_versioned(
        &self,
        node_id: NodeId,
        label: &str,
        _transaction_id: TransactionId,
    ) -> bool {
        self.add_label(node_id, label)
    }

    /// Removes a label within a transaction, recording the change for rollback.
    ///
    /// Default delegates to [`remove_label`](Self::remove_label).
    fn remove_label_versioned(
        &self,
        node_id: NodeId,
        label: &str,
        _transaction_id: TransactionId,
    ) -> bool {
        self.remove_label(node_id, label)
    }

    // --- Convenience (with default implementations) ---

    /// Creates a new node with labels and properties in one call.
    ///
    /// The default implementation calls [`create_node`](Self::create_node)
    /// followed by [`set_node_property`](Self::set_node_property) for each
    /// property. Implementations may override for atomicity or performance.
    fn create_node_with_props(
        &self,
        labels: &[&str],
        properties: &[(PropertyKey, Value)],
    ) -> NodeId {
        let id = self.create_node(labels);
        for (key, value) in properties {
            self.set_node_property(id, key.as_str(), value.clone());
        }
        id
    }

    /// Creates a new edge with properties in one call.
    ///
    /// The default implementation calls [`create_edge`](Self::create_edge)
    /// followed by [`set_edge_property`](Self::set_edge_property) for each
    /// property. Implementations may override for atomicity or performance.
    fn create_edge_with_props(
        &self,
        src: NodeId,
        dst: NodeId,
        edge_type: &str,
        properties: &[(PropertyKey, Value)],
    ) -> EdgeId {
        let id = self.create_edge(src, dst, edge_type);
        for (key, value) in properties {
            self.set_edge_property(id, key.as_str(), value.clone());
        }
        id
    }

    // --- MVCC hooks (T17 Wave B: 2026-04-23) ---
    //
    // These methods were LpgStore-inherent in the legacy design. With
    // substrate as the single production backend, substrate's
    // topology-as-storage model no longer tracks per-transaction
    // epochs, undo logs, or next-id-peek snapshots. The methods below
    // therefore default to no-ops: substrate inherits the defaults,
    // LpgStore overrides them (legacy behavior retained for the
    // archival `legacy-read` code path in obrain-migrate, and for
    // cognitive sidecars that still persist via the in-memory
    // LpgStore shadow).
    //
    // Moving these onto the trait lets `Session::resolve_store` and
    // `ObrainDB::store` hold `Arc<dyn GraphStoreMut>` instead of a
    // concrete `Arc<LpgStore>`, which is what finally unblocks the T17
    // cutover (the last concrete-type leak on the production path).

    /// Makes all PENDING version records from this transaction visible at
    /// the given commit epoch. No-op for backends without MVCC.
    fn finalize_version_epochs(&self, _transaction_id: TransactionId, _commit_epoch: EpochId) {}

    /// Commits property changes from this transaction (discards undo logs,
    /// making the mutations permanent). No-op for backends without MVCC.
    fn commit_transaction_properties(&self, _transaction_id: TransactionId) {}

    /// Rolls back property changes from this transaction (replays the undo
    /// log). No-op for backends without MVCC.
    fn rollback_transaction_properties(&self, _transaction_id: TransactionId) {}

    /// Rolls back property changes from this transaction back to the given
    /// undo-log position (savepoint-level rollback). No-op for backends
    /// without MVCC.
    fn rollback_transaction_properties_to(&self, _transaction_id: TransactionId, _since: usize) {}

    /// Publishes the given epoch as the store's "current" read epoch so that
    /// convenience lookups surface committed versions immediately. No-op for
    /// backends without MVCC.
    fn sync_epoch(&self, _epoch: EpochId) {}

    /// Prunes version records strictly older than `min_epoch` when the
    /// backend maintains a version history. No-op for backends without MVCC.
    fn gc_versions(&self, _min_epoch: EpochId) {}

    /// Returns the next node ID that would be allocated by `create_node`.
    /// Used by savepoint bookkeeping to rewind a pending allocator. Backends
    /// without a stable next-id peek return `0` — callers must treat a zero
    /// result as "savepoints unsupported" and skip the rewind path.
    fn peek_next_node_id(&self) -> u64 {
        0
    }

    /// Returns the current position of the property undo log for the given
    /// transaction. Used by savepoints. Backends without undo logs return
    /// `0` — callers interpret zero as "no undo log available".
    fn property_undo_log_position(&self, _transaction_id: TransactionId) -> usize {
        0
    }

    /// Discards all uncommitted (PENDING) version records for the given
    /// transaction. Used when aborting a transaction. No-op for backends
    /// without per-transaction version staging.
    fn discard_uncommitted_versions(&self, _transaction_id: TransactionId) {}

    /// Returns the next edge ID that would be allocated by `create_edge`.
    /// Used by savepoint bookkeeping. Backends without a stable peek
    /// return `0` — callers interpret zero as "savepoints unsupported".
    fn peek_next_edge_id(&self) -> u64 {
        0
    }

    /// Discards nodes and edges with the given ID lists for the current
    /// transaction. Used by savepoint rollback to cull entities allocated
    /// after the savepoint. No-op for backends without savepoint-level
    /// discard.
    fn discard_entities_by_id(
        &self,
        _transaction_id: TransactionId,
        _node_ids: &[NodeId],
        _edge_ids: &[EdgeId],
    ) {
    }

    // --- Named-graph hooks (T17 Step 24: 2026-04-23) ---
    //
    // LpgStore supports named sub-graphs (CREATE GRAPH g / DROP GRAPH g /
    // USE GRAPH g). The substrate backend does not — one database = one
    // graph in the topology-as-storage model. Named-graph methods default
    // to "not supported" values so that the Session layer can drive both
    // backends through `Arc<dyn GraphStoreMut>`; callers are already
    // defensive about `None` / empty results (fall back to the root
    // store when a named graph is missing). A substrate-native
    // multi-graph story is a separate RFC and is not required for T17.

    /// Returns a named graph by name, or `None` if it does not exist /
    /// named graphs are not supported by this backend.
    fn named_graph(&self, _name: &str) -> Option<Arc<dyn GraphStoreMut>> {
        None
    }

    /// Whether this backend implements named graphs AT ALL.
    ///
    /// (2026-08-13) Ce predicat existe parce que `create_named_graph`
    /// confond deux choses depuis le 23 avril. Son defaut rend `Ok(false)`
    /// pour dire « je ne sais pas faire », et l'appelant lit `false` comme
    /// « il existe deja » — d'ou 150+ tests qui echouent sur un
    /// « Graph 'alpha' already exists » leve contre une base VIERGE.
    ///
    /// L'enchainement est date : d77bba75 ajoute ce defaut en affirmant
    /// « LpgStore overrides each via a thin delegation, so the legacy
    /// semantics remain intact » ; 6d030291, le MEME JOUR, supprime
    /// `lpg/store/` et donc les overrides en question ; 7a96ace9 bascule
    /// `new_in_memory()` sur substrat et expose le defaut a tous les tests.
    /// Un defaut poli a survecu a la seule implementation qui le rendait
    /// vrai.
    ///
    /// Le trait se contredisait d'ailleurs lui-meme : `copy_named_graph`
    /// rend deja `Err("named graphs are not supported by this backend")`.
    /// On aligne le reste sur cette franchise-la.
    fn supports_named_graphs(&self) -> bool {
        false
    }

    /// Identité de SCOPE pour la détection de conflits write-write
    /// multi-graphes (RFC multi-writer B1, 2026-09-06).
    ///
    /// Deux stores substrate distincts allouent chacun leurs NodeId à partir
    /// de 1 : sans scope, le write_set GLOBAL du TransactionManager croit que
    /// `NodeId(1)` du graphe g1 et `NodeId(1)` du graphe g2 sont la MÊME
    /// entité → faux conflits inter-graphes (mesuré : 75 % d'échec sur des
    /// transactions longues disjointes). Le scope distingue les espaces
    /// d'identités. Défaut = 0 (mono-graphe / backends non partitionnés :
    /// comportement inchangé). Object-safe, pas de générique.
    fn graph_scope(&self) -> u64 {
        0
    }

    /// Creates a named graph. Returns `Ok(true)` on success, `Ok(false)`
    /// if it already exists.
    ///
    /// ATTENTION : `Ok(false)` signifie EXISTE DEJA, et rien d'autre. Un
    /// backend qui ne sait pas faire de graphes nommes doit le declarer
    /// via `supports_named_graphs()` — l'appelant interroge ce predicat
    /// AVANT d'interpreter le booleen.
    fn create_named_graph(&self, _name: &str) -> Result<bool, AllocError> {
        Ok(false)
    }

    /// Drops a named graph. Returns `false` if it did not exist / named
    /// graphs are not supported.
    fn drop_named_graph(&self, _name: &str) -> bool {
        false
    }

    /// Returns the names of all named graphs. Empty for backends without
    /// named-graph support.
    fn named_graph_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// Creates a node with a caller-specified `NodeId`. Used by
    /// reversible contraction (see `expand_supernode`) to restore
    /// original IDs after a round-trip. Default returns an error —
    /// substrate allocates monotonically and does not support
    /// reusing a specific slot; the legacy `LpgStore` backend
    /// overrides this with its inherent slot-reclamation routine.
    fn create_node_with_id(&self, _id: NodeId, _labels: &[&str]) -> Result<(), AllocError> {
        Err(AllocError::OutOfMemory)
    }

    /// Copies all nodes and edges from the `source` named graph into the
    /// `dest` named graph (both `None` = root graph). Used by
    /// `CREATE GRAPH g AS COPY OF h`. The default returns an error for
    /// backends without named-graph support.
    fn copy_named_graph(&self, _source: Option<&str>, _dest: Option<&str>) -> Result<(), String> {
        Err("named graphs are not supported by this backend".to_string())
    }

    // --- LpgStore-legacy inspection hooks (T17 Step 24: 2026-04-23) ---
    //
    // These methods back the `SHOW DATABASE` / `SHOW INDEXES` admin
    // paths and the GQL index-management statements that were
    // LpgStore-inherent. Substrate publishes the same information
    // through its tiered-storage zone headers + registry, so the
    // defaults return empty / not-supported values — sufficient for
    // the admin.rs paths to compile against `Arc<dyn GraphStoreMut>`.
    // LpgStore overrides each method by delegating to its inherent
    // implementation (legacy-read path + in-memory shadow).

    /// Returns every node in the store. Default iterates through
    /// `node_ids()` + `get_node()` (hydrating each node on demand).
    /// Implementations with a faster path (e.g. DashMap iteration)
    /// may override.
    fn all_nodes(&self) -> Vec<Node> {
        self.node_ids()
            .into_iter()
            .filter_map(|id| self.get_node(id))
            .collect()
    }

    /// Returns all nodes carrying the given label. Used by index
    /// builders. Default returns empty — backends that do not maintain
    /// a label→nodes reverse index should populate on demand via
    /// `nodes_by_label` (IDs) + `get_node` (hydration).
    fn nodes_with_label(&self, _label: &str) -> Vec<Node> {
        Vec::new()
    }

    /// Returns all edges carrying the given edge-type. Used by schema
    /// introspection. Default returns empty.
    fn edges_with_type(&self, _edge_type: &str) -> Vec<Edge> {
        Vec::new()
    }

    /// Returns every edge in the store. Used by schema introspection +
    /// legacy snapshot paths. Default returns empty — substrate
    /// surfaces edges through `edges_from(node, direction)` which
    /// scales to live-graph sizes.
    fn all_edges(&self) -> Vec<Edge> {
        Vec::new()
    }

    /// Creates a property index over the given property name. Default
    /// no-op — substrate does not maintain property-level side
    /// indexes; the tiered-storage column layout provides equivalent
    /// functionality.
    fn create_property_index(&self, _property: &str) {}

    /// Drops a property index. Default returns `false` ("nothing was
    /// dropped") — substrate does not maintain named property
    /// indexes.
    fn drop_property_index(&self, _property: &str) -> bool {
        false
    }

    /// Returns a detailed RAM memory breakdown of the store.
    ///
    /// Default returns all-zero totals: this category of
    /// introspection is LpgStore-specific (VersionChain maps + MVCC
    /// chains + property DashMaps). Substrate exposes zone footprints
    /// through a different API (future
    /// `SubstrateStore::memory_breakdown`) that reports mmap zones
    /// instead of heap allocations.
    fn memory_breakdown(
        &self,
    ) -> (
        obrain_common::memory::usage::StoreMemory,
        obrain_common::memory::usage::IndexMemory,
        obrain_common::memory::usage::MvccMemory,
        obrain_common::memory::usage::StringPoolMemory,
    ) {
        (
            obrain_common::memory::usage::StoreMemory::default(),
            obrain_common::memory::usage::IndexMemory::default(),
            obrain_common::memory::usage::MvccMemory::default(),
            obrain_common::memory::usage::StringPoolMemory::default(),
        )
    }
}
