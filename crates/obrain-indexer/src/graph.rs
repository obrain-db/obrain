//! Graphe en cours de construction (buffers de nœuds/arêtes).
//!
//! L'indexeur produit un [`CodeGraph`] en mémoire, consommable par le moteur
//! (l'écriture substrate bulk est une étape ultérieure, RFC M6 §5 point 4).

use std::collections::HashMap;

/// (T27) Table INTERNE à hachage rapide. Les clés (FQN de 60 à 100 octets,
/// identifiants) sont des données locales : la résistance de SipHash aux
/// attaques par collision n'achète rien ici, et la construction du graphe du
/// noyau Linux en fait des millions. Les `extra` publics gardent `HashMap`.
type Carte<K, V> = std::collections::HashMap<K, V, foldhash::fast::RandomState>;

use serde::{Deserialize, Serialize};

use crate::schema::{EdgeKind, NodeKind};

/// Identifiant stable d'un nœud dans le graphe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Identifiant stable d'une arête.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub u64);

/// Propriétés d'un nœud (JSON — le contrat C les stocke en JSON libre).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeProps {
    /// Nom court (fonction, classe…).
    pub name: Option<String>,
    /// Chemin du fichier source.
    pub file_path: Option<String>,
    /// Ligne de début dans le fichier.
    pub start_line: Option<u32>,
    /// Ligne de fin.
    pub end_line: Option<u32>,
    /// Champs libres supplémentaires (signature, docstring, parent_class,
    /// complexity, is_test, … — le contrat C est ouvert).
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Un nœud du graphe de code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Qualified name (FQN) — identité dans le graphe.
    pub qualified_name: String,
    pub props: NodeProps,
}

/// Propriétés d'une arête (strategy/confidence/line… — contrat C ouvert).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EdgeProps {
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Une arête du graphe de code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub kind: EdgeKind,
    pub source: NodeId,
    pub target: NodeId,
    pub props: EdgeProps,
}

impl CodeGraph {
    /// Graphe avec capacités pré-réservées (le nombre de définitions est
    /// connu avant la fusion) — évite les reallocations du Vec pendant les
    /// 1,55 M upserts du kernel.
    pub fn with_capacity(nodes: usize, edges: usize) -> Self {
        let mut g = CodeGraph::default();
        g.nodes.reserve(nodes);
        g.edges.reserve(edges);
        g.by_qn.reserve(nodes);
        g.by_id.reserve(nodes);
        g
    }
}

/// Le graphe de code en mémoire (sortie de l'indexation).
#[derive(Debug, Default)]
pub struct CodeGraph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    /// index qn → node id (unicité du FQN, comme UNIQUE(project, qualified_name)
    /// dans le store C).
    by_qn: Carte<String, NodeId>,
    /// index id → position dans `nodes` (accès O(1) — la résolution CALLS
    /// fait des millions de lookups, le scan linéaire était le goulot :
    /// 2,4 s → 469 s sur 989 fichiers).
    by_id: Carte<NodeId, usize>,
    /// index kind → positions dans `nodes` — les requêtes agents filtrent
    /// par type (Fonction, File…) sans scanner les 1,5 M nœuds du kernel.
    by_kind: Carte<NodeKind, Vec<usize>>,
    /// colonnes de métriques alignées sur `nodes` (par position) — le
    /// top-k lit un Vec<i64> sans hash lookup par nœud (238 ms → ~1 ms
    /// sur les 650 k fonctions du kernel).
    metric_columns: Carte<&'static str, Vec<i64>>,
    /// top-N pré-triés (kind, métrique) → positions triées desc — construit
    /// UNE fois à l'indexation : la requête top-k devient O(k) (~µs) au
    /// lieu de scanner les 650 k fonctions (~7 ms).
    top_caches: Carte<(NodeKind, &'static str), Vec<usize>>,
    /// index kind d'arête → positions dans `edges` — les requêtes par type
    /// d'arête (TESTS, FILE_CHANGES_WITH…) ne scannent pas les 4,8 M edges.
    by_edge_kind: Carte<EdgeKind, Vec<usize>>,
    /// compteurs booléens pré-calculés (kind, métrique) → nb de vrais —
    /// count_bool_metric devient O(1) au lieu de scanner les 650 k nœuds.
    bool_counts: Carte<(NodeKind, &'static str), usize>,
    /// (T27) Types (Struct/Interface/Enum/Type/Class) par DERNIER SEGMENT de
    /// leur FQN. La fusion cherchait la classe parente d'une méthode en
    /// balayant TOUS les nœuds (`qualified_name.ends_with(".Parent")`) — O(N)
    /// par méthode, donc quadratique sur le noyau (1,5 M nœuds). Tenu à la
    /// CRÉATION du nœud : `upsert_node` ne change jamais le type d'un nœud
    /// existant, l'ensemble est donc exactement celui qu'aurait vu le balayage.
    types_par_segment: Carte<String, Vec<NodeId>>,
    next_node_id: u64,
    next_edge_id: u64,
}

impl CodeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insère (ou retrouve) un nœud par FQN. Retourne son id.
    pub fn upsert_node(
        &mut self,
        kind: NodeKind,
        qualified_name: String,
        props: NodeProps,
    ) -> NodeId {
        if let Some(&id) = self.by_qn.get(&qualified_name) {
            // Upsert : complète les props manquantes (le C fait un json_patch).
            if let Some(&pos) = self.by_id.get(&id) {
                self.nodes[pos].props.merge_missing(&props);
            }
            return id;
        }
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        self.by_qn.insert(qualified_name.clone(), id);
        self.by_id.insert(id, self.nodes.len());
        if Self::est_type(kind)
            && let Some((_, dernier)) = qualified_name.rsplit_once('.')
        {
            self.types_par_segment.entry(dernier.to_string()).or_default().push(id);
        }
        self.nodes.push(Node {
            id,
            kind,
            qualified_name,
            props,
        });
        self.by_kind
            .entry(kind)
            .or_default()
            .push(self.nodes.len() - 1);
        id
    }

    /// Insère une arête (source → target). Retourne son id.
    pub fn insert_edge(
        &mut self,
        kind: EdgeKind,
        source: NodeId,
        target: NodeId,
        props: EdgeProps,
    ) -> EdgeId {
        let id = EdgeId(self.next_edge_id);
        self.next_edge_id += 1;
        self.edges.push(Edge {
            id,
            kind,
            source,
            target,
            props,
        });
        self.by_edge_kind
            .entry(kind)
            .or_default()
            .push(self.edges.len() - 1);
        id
    }

    fn est_type(kind: NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::Struct | NodeKind::Interface | NodeKind::Enum | NodeKind::Type | NodeKind::Class
        )
    }

    /// Le type UNIQUE dont le FQN se termine par `.{parent}`, ou `None` s'il
    /// n'y en a aucun ou plusieurs — le même verdict que le balayage complet
    /// qu'il remplace. Un `parent` qui contient lui-même un point ne se
    /// ramène pas à un dernier segment : on balaie alors, comme avant.
    pub fn type_unique_par_suffixe(&self, parent: &str) -> Option<NodeId> {
        if !parent.contains('.') {
            return match self.types_par_segment.get(parent).map(Vec::as_slice) {
                Some([seul]) => Some(*seul),
                _ => None,
            };
        }
        let suffixe = format!(".{parent}");
        let trouves: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|n| Self::est_type(n.kind) && n.qualified_name.ends_with(&suffixe))
            .map(|n| n.id)
            .collect();
        (trouves.len() == 1).then(|| trouves[0])
    }

    pub fn node_by_qn(&self, qn: &str) -> Option<&Node> {
        self.by_qn.get(qn).and_then(|id| self.node(*id))
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.by_id.get(&id).map(|&pos| &self.nodes[pos])
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Accès mutable aux nœuds (passes post-résolution : metrics, …).
    pub fn nodes_mut(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    /// Positions des nœuds d'un kind donné (index — pas de scan global).
    pub fn positions_of_kind(&self, kind: NodeKind) -> &[usize] {
        self.by_kind.get(&kind).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Les top-k nœuds d'un kind selon une métrique numérique (extra) —
    /// BinaryHeap partiel O(n·log k) au lieu du tri complet O(n·log n).
    pub fn top_by_metric(&self, kind: NodeKind, metric: &str, k: usize) -> Vec<(i64, usize)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        // Heap min de taille k : on garde les k plus grands (v, pos).
        let mut heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::with_capacity(k + 1);
        let mut count = 0usize;
        // Colonne vectorisée si dispo (rapide), sinon hash lookup.
        if let Some(col) = self.metric_columns.get(metric) {
            for &pos in self.positions_of_kind(kind) {
                let v = col[pos];
                heap.push(Reverse((v, pos)));
                count += 1;
                if count > k {
                    heap.pop();
                    count -= 1;
                }
            }
        } else {
            for &pos in self.positions_of_kind(kind) {
                let n = &self.nodes[pos];
                let Some(v) = n.props.extra.get(metric).and_then(|v| v.as_i64()) else {
                    continue;
                };
                heap.push(Reverse((v, pos)));
                count += 1;
                if count > k {
                    heap.pop();
                    count -= 1;
                }
            }
        }
        let mut out: Vec<(i64, usize)> =
            heap.into_iter().map(|Reverse((v, pos))| (v, pos)).collect();
        // Tri déterministe : valeur desc, puis position asc.
        out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        out
    }

    /// Vrai si une colonne de métrique existe.
    pub fn has_metric_column(&self, metric: &str) -> bool {
        self.metric_columns.contains_key(metric)
    }

    /// Valeur de métrique d'un nœud (colonne — 0 si absente).
    pub fn metric_value(&self, id: NodeId, metric: &str) -> i64 {
        let Some(col) = self.metric_columns.get(metric) else {
            return 0;
        };
        let Some(&pos) = self.by_id.get(&id) else {
            return 0;
        };
        col[pos]
    }

    /// Compte les nœuds d'un kind dont une métrique booléenne vaut 1
    /// (colonne vectorisée — ~1 ms sur 650 k fonctions au lieu de ~150 ms).
    pub fn count_bool_metric(&self, kind: NodeKind, metric: &str) -> usize {
        if let Some(&c) = self.bool_counts.get(&(kind, metric)) {
            return c;
        }
        if let Some(col) = self.metric_columns.get(metric) {
            return self
                .positions_of_kind(kind)
                .iter()
                .filter(|&&pos| col[pos] != 0)
                .count();
        }
        self.positions_of_kind(kind)
            .iter()
            .filter(|&&pos| {
                self.nodes[pos]
                    .props
                    .extra
                    .get(metric)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .count()
    }

    /// Pré-calcule les compteurs booléens (kind, métrique) — O(n) une fois
    /// à l'indexation, puis count_bool_metric est O(1).
    pub fn build_bool_counts(&mut self, kinds: &[NodeKind], metrics: &[&'static str]) {
        for &kind in kinds {
            for &m in metrics {
                let Some(col) = self.metric_columns.get(m) else {
                    continue;
                };
                let c = self
                    .positions_of_kind(kind)
                    .iter()
                    .filter(|&&pos| col[pos] != 0)
                    .count();
                self.bool_counts.insert((kind, m), c);
            }
        }
    }

    /// Construit les colonnes de métriques (alignées sur `nodes`) — une
    /// passe unique à la fin de l'indexation. `metrics` : les clés extra à
    /// vectoriser (cognitive, complexity, …).
    pub fn build_metric_columns(&mut self, metrics: &[&'static str]) {
        // UN seul passage : chaque nœud remplit toutes ses colonnes (le
        // HashMap extra reste chaud). Avant : N_metrics × nœuds itérations
        // + hash lookups froids (1,58 s kernel → 0,65 s).
        let mut cols: Vec<(usize, &'static str, Vec<i64>)> = metrics
            .iter()
            .enumerate()
            .map(|(i, m)| (i, *m, vec![0i64; self.nodes.len()]))
            .collect();
        for (pos, n) in self.nodes.iter().enumerate() {
            let extra = &n.props.extra;
            for (ci, _, col) in cols.iter_mut() {
                let m = metrics[*ci];
                if m == "has_docstring" {
                    col[pos] = extra.contains_key("docstring") as i64;
                    continue;
                }
                if let Some(v) = extra.get(m) {
                    let val = v.as_i64().or_else(|| v.as_bool().map(|b| b as i64));
                    if let Some(val) = val {
                        col[pos] = val;
                    }
                }
            }
        }
        for (_, m, col) in cols {
            self.metric_columns.insert(m, col);
        }
    }

    /// Pré-calcule les top-N (kind, métrique) triés desc — une passe unique
    /// à l'indexation. `n` : taille du cache (100 suffit pour top-5/10).
    pub fn build_top_caches(&mut self, kinds: &[NodeKind], metrics: &[&'static str], n: usize) {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        for &kind in kinds {
            for &m in metrics {
                let Some(col) = self.metric_columns.get(m) else {
                    continue;
                };
                // Heap min borné à n : garde les n plus grandes valeurs.
                let mut heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::with_capacity(n + 1);
                let mut count = 0usize;
                for &pos in self.positions_of_kind(kind) {
                    let v = col[pos];
                    heap.push(Reverse((v, pos)));
                    count += 1;
                    if count > n {
                        heap.pop();
                        count -= 1;
                    }
                }
                let mut v: Vec<(i64, usize)> = heap.into_iter().map(|Reverse(x)| x).collect();
                v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                self.top_caches
                    .insert((kind, m), v.into_iter().map(|(_, p)| p).collect());
            }
        }
    }

    /// Top-k depuis le cache pré-trié (O(k)) — fallback scan si absent.
    pub fn top_k_cached(&self, kind: NodeKind, metric: &str, k: usize) -> Vec<(i64, usize)> {
        if let Some(cache) = self.top_caches.get(&(kind, metric)) {
            let col = self.metric_columns.get(metric);
            return cache
                .iter()
                .take(k)
                .filter_map(|&pos| col.map(|c| (c[pos], pos)))
                .collect();
        }
        self.top_by_metric(kind, metric, k)
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Positions des arêtes d'un kind donné (index — pas de scan global).
    pub fn edge_positions(&self, kind: EdgeKind) -> &[usize] {
        self.by_edge_kind
            .get(&kind)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Nombre d'arêtes d'un kind (O(1) — compteur indexé).
    pub fn edge_count_kind(&self, kind: EdgeKind) -> usize {
        self.by_edge_kind.get(&kind).map(|v| v.len()).unwrap_or(0)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Insère une arête entre deux FQN existants (retourne None si une des
    /// extrémités est absente).
    pub fn link_qns(
        &mut self,
        kind: EdgeKind,
        source_qn: &str,
        target_qn: &str,
        props: EdgeProps,
    ) -> Option<EdgeId> {
        let s = self.node_by_qn(source_qn)?.id;
        let t = self.node_by_qn(target_qn)?.id;
        Some(self.insert_edge(kind, s, t, props))
    }
}

impl NodeProps {
    /// Complète les props manquantes de `self` depuis `other` (upsert doux).
    fn merge_missing(&mut self, other: &NodeProps) {
        if self.name.is_none() {
            self.name = other.name.clone();
        }
        if self.file_path.is_none() {
            self.file_path = other.file_path.clone();
        }
        if self.start_line.is_none() {
            self.start_line = other.start_line;
        }
        if self.end_line.is_none() {
            self.end_line = other.end_line;
        }
        for (k, v) in &other.extra {
            self.extra.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_exacts_vs_scan() {
        // Construit un graphe, pré-calcule caches/colonnes/compteurs, et
        // vérifie que les accès indexés == les scans complets.
        let mut g = CodeGraph::new();
        for i in 0..50u64 {
            let mut p = NodeProps::default();
            let v = (i % 7) as i64; // ex-aequo volontaires
            p.extra
                .insert("cognitive".into(), serde_json::Value::from(v));
            if i % 3 == 0 {
                p.extra
                    .insert("recursive".into(), serde_json::Value::Bool(true));
            }
            g.upsert_node(NodeKind::Function, format!("m.f{i}"), p);
        }
        // 3 arêtes Tests.
        for i in 0..3 {
            g.insert_edge(
                EdgeKind::Tests,
                NodeId(i),
                NodeId(i + 1),
                EdgeProps::default(),
            );
        }
        g.build_metric_columns(&["cognitive", "recursive"]);
        g.build_top_caches(&[NodeKind::Function], &["cognitive"], 10);
        g.build_bool_counts(&[NodeKind::Function], &["recursive"]);

        // Valeurs top-5 : cache == scan.
        let cached: Vec<i64> = g
            .top_k_cached(NodeKind::Function, "cognitive", 5)
            .iter()
            .map(|(v, _)| *v)
            .collect();
        let scan: Vec<i64> = g
            .top_by_metric(NodeKind::Function, "cognitive", 5)
            .iter()
            .map(|(v, _)| *v)
            .collect();
        assert_eq!(cached, scan, "top cognitive cache != scan");

        // Compteur booléen == scan.
        let n_rec = g.count_bool_metric(NodeKind::Function, "recursive");
        let exp = (0..50).filter(|i| i % 3 == 0).count();
        assert_eq!(n_rec, exp, "compteur recursive != scan");

        // Compteur edges == scan.
        assert_eq!(g.edge_count_kind(EdgeKind::Tests), 3);
        assert_eq!(g.edge_positions(EdgeKind::Tests).len(), 3);
    }

    #[test]
    fn upsert_deduplique_par_fqn() {
        let mut g = CodeGraph::new();
        let a = g.upsert_node(
            NodeKind::Function,
            "proj.src.main.run".to_string(),
            NodeProps {
                name: Some("run".into()),
                ..Default::default()
            },
        );
        // Même FQN → même id (pas de doublon).
        let b = g.upsert_node(
            NodeKind::Function,
            "proj.src.main.run".to_string(),
            NodeProps::default(),
        );
        assert_eq!(a, b);
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn fqn_distincts_donnent_des_noeuds_distincts() {
        let mut g = CodeGraph::new();
        let a = g.upsert_node(NodeKind::Function, "a.f".to_string(), NodeProps::default());
        let b = g.upsert_node(NodeKind::Function, "b.f".to_string(), NodeProps::default());
        assert_ne!(a, b);
        assert_eq!(g.node_count(), 2);
    }

    #[test]
    fn arete_source_target() {
        let mut g = CodeGraph::new();
        let caller = g.upsert_node(
            NodeKind::Function,
            "a.caller".to_string(),
            NodeProps::default(),
        );
        let callee = g.upsert_node(
            NodeKind::Function,
            "b.callee".to_string(),
            NodeProps::default(),
        );
        let e = g.insert_edge(EdgeKind::Calls, caller, callee, EdgeProps::default());
        assert_eq!(g.edge_count(), 1);
        assert!(
            g.edges()
                .iter()
                .any(|x| x.id == e && x.source == caller && x.target == callee)
        );
    }
}
