//! Reranking topologique du retrieval — RFC zorder-superposition C2
//! (« RAG topologique »).
//!
//! Principe (v1 prouvée dans `vector_latency_scale.rs::
//! topological_rerank_synthetic_c2`, precision@5 0.530 → 0.944) : quand le
//! signal sémantique (BM25 + vecteur) est ambigu, la STRUCTURE du graphe
//! départage — les candidats issus du même cluster topologique sont plus
//! susceptibles d'être la bonne réponse ensemble.
//!
//! Ce module extrait le rerank du test vers une API consommable :
//! - [`rerank_by_cluster_dominance`] — fonction pure, réordonne des candidats
//!   `(node_id, score)` par score + dominance de cluster (résolveur injecté) ;
//! - [`ObrainDB::hybrid_search_topological`](crate::ObrainDB::hybrid_search_topological) — pipeline complet : hybrid
//!   search sur-échantillonné → résolution du cluster de chaque candidat via
//!   la closure fournie → rerank par dominance → top-k.
//!
//! # Bit-identité
//!
//! `boost = 0.0` (ou résolveur rendant `None` partout, ou candidats vides)
//! ⇒ les candidats traversent inchangés (seul le tri descendant du score
//! existant est appliqué) : le chemin non-topologique n'est jamais altéré.

use obrain_common::types::NodeId;

/// Reranke des candidats `(node_id, score)` par dominance de cluster.
///
/// Algorithme (repris à l'identique de la v1 du test) :
/// 1. pour chaque candidat, résoudre son cluster via `cluster_of` ;
///    les candidats sans cluster (None) ne reçoivent aucun bonus ;
/// 2. compter, parmi les candidats, l'effectif de chaque cluster ;
/// 3. score' = score + dominance × boost, où
///    `dominance = count(cluster) / len(candidats)` ;
/// 4. re-trier par score' descendant et tronquer à `k`.
///
/// Résultats **bit-identiques** à l'entrée (à l'ordre de tri près) quand
/// `boost == 0.0`. Ne mute jamais le graphe — pure fonction de classement.
///
/// # Paramètres
///
/// - `boost` : poids du signal topologique (v1 du test : `1.0`).
/// - `k` : nombre de résultats à rendre (top-k final).
pub fn rerank_by_cluster_dominance(
    mut candidates: Vec<(NodeId, f64)>,
    k: usize,
    boost: f64,
    cluster_of: impl Fn(NodeId) -> Option<u64>,
) -> Vec<(NodeId, f64)> {
    if boost == 0.0 || candidates.is_empty() || k == 0 {
        // Chemin bit-identique : tri descendant (le contrat du retrieval)
        // puis top-k, sans aucune influence topologique.
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(k);
        return candidates;
    }

    // 1–2. Résolution + comptage des clusters parmi les candidats.
    let mut cluster_count: std::collections::HashMap<u64, f64> = Default::default();
    let mut resolved: Vec<Option<u64>> = Vec::with_capacity(candidates.len());
    for (id, _) in &candidates {
        let cl = cluster_of(*id);
        if let Some(c) = cl {
            *cluster_count.entry(c).or_insert(0.0) += 1.0;
        }
        resolved.push(cl);
    }
    let total = candidates.len().max(1) as f64;

    // 3–4. Score + dominance, re-tri, top-k.
    let mut scored: Vec<(NodeId, f64)> = candidates
        .into_iter()
        .zip(resolved)
        .map(|((id, score), cl)| {
            let dominance = cl
                .and_then(|c| cluster_count.get(&c).copied())
                .unwrap_or(0.0)
                / total;
            (id, score + dominance * boost)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// Composantes connexes d'un type d'arête donné, restreintes à l'ensemble
/// `candidates ∪ {voisins 1-hop du type}` — le résolveur de cluster RÉEL
/// (aucun oracle) pour `hybrid_search_topological`.
///
/// Retourne `node → composante`. Deux nœuds sont dans la même composante si
/// un chemin d'arêtes du type les relie (en passant par des nœuds hors
/// candidats, via les voisins 1-hop inclus). Borné : l'union-find ne voit
/// que les candidats et leurs voisins immédiats du type — O(k × deg) par
/// requête, pas O(V+E).
pub fn cluster_components_by_edge_type(
    store: &dyn obrain_core::graph::GraphStore,
    candidates: &[NodeId],
    edge_type: &str,
) -> std::collections::HashMap<NodeId, u64> {
    use obrain_core::graph::Direction;
    use std::collections::{HashMap, HashSet};

    if candidates.is_empty() {
        return HashMap::new();
    }

    // Univers : candidats + voisins 1-hop sortants/entrants du type.
    let mut universe: HashSet<NodeId> = HashSet::new();
    universe.extend(candidates.iter().copied());
    let mut edges: Vec<(NodeId, NodeId)> = Vec::new();
    for &c in candidates {
        for (nb, eid) in store.edges_from(c, Direction::Outgoing) {
            if store.edge_type(eid).as_deref() == Some(edge_type) {
                universe.insert(nb);
                edges.push((c, nb));
            }
        }
        for (nb, eid) in store.edges_from(c, Direction::Incoming) {
            if store.edge_type(eid).as_deref() == Some(edge_type) {
                universe.insert(nb);
                edges.push((c, nb));
            }
        }
    }

    // Union-find sur l'univers.
    let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
    for &n in &universe {
        parent.insert(n, n);
    }
    fn find(parent: &mut HashMap<NodeId, NodeId>, x: NodeId) -> NodeId {
        let p = parent[&x];
        if p == x {
            return x;
        }
        let root = find(parent, p);
        parent.insert(x, root);
        root
    }
    fn union(parent: &mut HashMap<NodeId, NodeId>, a: NodeId, b: NodeId) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }
    for (a, b) in &edges {
        union(&mut parent, *a, *b);
    }

    // Compresser : node → racine (u64 compacte).
    let mut roots: Vec<u64> = Vec::new();
    let mut id_of: HashMap<NodeId, u64> = HashMap::new();
    let mut out: HashMap<NodeId, u64> = HashMap::new();
    for &n in &universe {
        let root = find(&mut parent, n);
        let rid = *id_of.entry(root).or_insert_with(|| {
            let id = roots.len() as u64;
            roots.push(id);
            id
        });
        out.insert(n, rid);
    }
    out
}

/// Pipeline « RAG topologique » consommable (RFC zorder-superposition C2).
///
/// 1. `hybrid_search` sur-échantillonné (`k × 4`, plancher 64) pour donner au
///    rerank de la matière au-delà du top-k final ;
/// 2. résolution du cluster de chaque candidat via `cluster_of` (closure
///    injectée : `cluster_components_by_edge_type`, `community_id` substrate,
///    oracle de test, …) ;
/// 3. [`rerank_by_cluster_dominance`] (score + dominance × `boost`) ;
/// 4. top-k final.
impl super::ObrainDB {
    /// Hybrid search enrichi par le signal topologique.
    ///
    /// # Features requises
    ///
    /// `hybrid-search` (et les index texte/vecteur sous-jacents).
    ///
    /// # Paramètres
    ///
    /// Mêmes que [`super::ObrainDB::hybrid_search`], plus :
    /// - `boost` : poids du rerank topologique (`0.0` = désactivé) ;
    /// - `cluster_of` : résout le cluster topologique d'un candidat
    ///   (`None` = candidat non clusterisé → aucun bonus).
    ///
    /// `boost = 0.0` ⇒ comportement identique à `hybrid_search` (à un léger
    /// sur-échantillonnage près, invisible sur le top-k quand `k × 4` ≥ k).
    #[cfg(feature = "hybrid-search")]
    #[allow(clippy::too_many_arguments)] // API publique : paramètres de `hybrid_search` (sans `fusion`), plus `boost` et `cluster_of`
    pub fn hybrid_search_topological(
        &self,
        label: &str,
        text_property: &str,
        vector_property: &str,
        query_text: &str,
        query_vector: Option<&[f32]>,
        k: usize,
        boost: f64,
        cluster_of: impl Fn(NodeId) -> Option<u64>,
    ) -> super::Result<Vec<(NodeId, f64)>> {
        let sur_echantillon = k.saturating_mul(4).max(64);
        let candidates = self.hybrid_search(
            label,
            text_property,
            vector_property,
            query_text,
            query_vector,
            sur_echantillon,
            None,
        )?;
        Ok(rerank_by_cluster_dominance(
            candidates, k, boost, cluster_of,
        ))
    }
}
