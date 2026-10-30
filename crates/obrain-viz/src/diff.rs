//! Diff structurel entre deux sous-graphes (V1-B : `viz_timeline`/`compare`).
//!
//! Identité : nœuds par `id`, arêtes par `(src, dst, type)`. Sortie alignée
//! sur les deltas du journal d'import existant (`ImportJournal { added_nodes,
//! removed_nodes, added_edges, removed_edges }`) — les deltas datés d'un
//! `viz_timeline` se re-dérivent par diff de snapshots json-graph v1, sans
//! nouveau mécanisme (Décision 8 de la roadmap).

use serde::Serialize;

use crate::VizResult;
use crate::ir::{Subgraph, VizEdge, VizNode};

/// Référence compacte d'un nœud dans un diff.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeRef {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Référence compacte d'une arête dans un diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EdgeRef {
    pub src: String,
    pub dst: String,
    #[serde(rename = "type")]
    pub edge_type: String,
}

/// Rapport de diff entre un état de base et un état suivant.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SubgraphDiff {
    pub added_nodes: Vec<NodeRef>,
    pub removed_nodes: Vec<NodeRef>,
    pub added_edges: Vec<EdgeRef>,
    pub removed_edges: Vec<EdgeRef>,
    pub unchanged_nodes: usize,
    pub unchanged_edges: usize,
}

impl SubgraphDiff {
    /// Nombre total de deltas (nœuds + arêtes, ajouts + retraits).
    #[must_use]
    pub fn delta_count(&self) -> usize {
        self.added_nodes.len()
            + self.removed_nodes.len()
            + self.added_edges.len()
            + self.removed_edges.len()
    }
}

impl From<&VizNode> for NodeRef {
    fn from(n: &VizNode) -> Self {
        Self {
            id: n.id.clone(),
            qn: n.qn.clone(),
            label: n.label.clone(),
        }
    }
}

impl From<&VizEdge> for EdgeRef {
    fn from(e: &VizEdge) -> Self {
        Self {
            src: e.src.clone(),
            dst: e.dst.clone(),
            edge_type: e.edge_type.clone(),
        }
    }
}

/// Diff structurel `other − base` (nœuds et arêtes, canoniques au préalable).
/// Déterministe : tri des listes de deltas par clé.
#[must_use]
pub fn diff_subgraphs(base: &Subgraph, other: &Subgraph) -> SubgraphDiff {
    let base_nodes: std::collections::HashMap<&str, &VizNode> =
        base.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let other_nodes: std::collections::HashMap<&str, &VizNode> =
        other.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let base_edges: std::collections::HashSet<(String, String, String)> = base
        .edges
        .iter()
        .map(|e| (e.src.clone(), e.dst.clone(), e.edge_type.clone()))
        .collect();
    let other_edges: std::collections::HashSet<(String, String, String)> = other
        .edges
        .iter()
        .map(|e| (e.src.clone(), e.dst.clone(), e.edge_type.clone()))
        .collect();

    let mut added_nodes: Vec<NodeRef> = other_nodes
        .iter()
        .filter(|(id, _)| !base_nodes.contains_key(*id))
        .map(|(_, n)| NodeRef::from(*n))
        .collect();
    added_nodes.sort_by(|a, b| a.id.cmp(&b.id));

    let mut removed_nodes: Vec<NodeRef> = base_nodes
        .iter()
        .filter(|(id, _)| !other_nodes.contains_key(*id))
        .map(|(_, n)| NodeRef::from(*n))
        .collect();
    removed_nodes.sort_by(|a, b| a.id.cmp(&b.id));

    let mut added_edges: Vec<EdgeRef> = other_edges
        .iter()
        .filter(|e| !base_edges.contains(e))
        .map(|(s, d, t)| EdgeRef {
            src: s.clone(),
            dst: d.clone(),
            edge_type: t.clone(),
        })
        .collect();
    added_edges.sort_by(|a, b| (&a.src, &a.dst, &a.edge_type).cmp(&(&b.src, &b.dst, &b.edge_type)));

    let mut removed_edges: Vec<EdgeRef> = base_edges
        .iter()
        .filter(|e| !other_edges.contains(e))
        .map(|(s, d, t)| EdgeRef {
            src: s.clone(),
            dst: d.clone(),
            edge_type: t.clone(),
        })
        .collect();
    removed_edges
        .sort_by(|a, b| (&a.src, &a.dst, &a.edge_type).cmp(&(&b.src, &b.dst, &b.edge_type)));

    let unchanged_nodes = base_nodes
        .keys()
        .filter(|k| other_nodes.contains_key(*k))
        .count();
    let unchanged_edges = base_edges.intersection(&other_edges).count();

    SubgraphDiff {
        added_nodes,
        removed_nodes,
        added_edges,
        removed_edges,
        unchanged_nodes,
        unchanged_edges,
    }
}

/// Sérialise le diff en JSON (pretty, 2 espaces — stable et trié).
pub fn diff_json_string(base: &Subgraph, other: &Subgraph) -> VizResult<String> {
    let diff = diff_subgraphs(&base.clone().canonical(), &other.clone().canonical());
    Ok(serde_json::to_string_pretty(&diff)?)
}

/// Étape d'un timeline : diff entre deux snapshots consécutifs (V3-E, base
/// d'animation) — compteurs alignés sur les deltas `ImportJournal`, plus
/// échantillons bornés des ids ajoutés/retirés.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineStep {
    /// Index du snapshot de destination (1-based).
    pub step: usize,
    pub added_nodes: usize,
    pub removed_nodes: usize,
    pub added_edges: usize,
    pub removed_edges: usize,
    pub sample_added_nodes: Vec<String>,
    pub sample_removed_nodes: Vec<String>,
}

/// Rapport complet d'un timeline (base d'animation : chaque étape est un diff).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineReport {
    pub snapshots: usize,
    pub steps: Vec<TimelineStep>,
}

/// Construit le timeline d'une séquence ordonnée de snapshots (au moins 2).
/// Chaque étape `i` est le diff `snapshots[i] → snapshots[i+1]`.
pub fn timeline_subgraphs(
    snapshots: &[&crate::ir::Subgraph],
    max_samples: usize,
) -> TimelineReport {
    let mut steps = Vec::new();
    for i in 0..snapshots.len().saturating_sub(1) {
        let d = diff_subgraphs(snapshots[i], snapshots[i + 1]);
        let sample = |v: &[NodeRef]| v.iter().take(max_samples).map(|n| n.id.clone()).collect();
        steps.push(TimelineStep {
            step: i + 1,
            added_nodes: d.added_nodes.len(),
            removed_nodes: d.removed_nodes.len(),
            added_edges: d.added_edges.len(),
            removed_edges: d.removed_edges.len(),
            sample_added_nodes: sample(&d.added_nodes),
            sample_removed_nodes: sample(&d.removed_nodes),
        });
    }
    TimelineReport {
        snapshots: snapshots.len(),
        steps,
    }
}

#[cfg(test)]
mod tests {
    use super::{diff_json_string, diff_subgraphs, timeline_subgraphs};
    use crate::ir::{Subgraph, VizEdge, VizNode};

    fn sample() -> Subgraph {
        Subgraph {
            nodes: vec![VizNode::new("a"), VizNode::new("b"), VizNode::new("c")],
            edges: vec![
                VizEdge::new("a", "b", "CALLS"),
                VizEdge::new("b", "c", "CALLS"),
            ],
            truncated: false,
        }
        .canonical()
    }

    #[test]
    fn diff_identical_is_empty() {
        let s = sample();
        let d = diff_subgraphs(&s, &s);
        assert_eq!(d.delta_count(), 0);
        assert_eq!(d.unchanged_nodes, 3);
        assert_eq!(d.unchanged_edges, 2);
    }

    #[test]
    fn diff_detects_additions_and_removals() {
        let base = sample();
        let mut other = base.clone();
        other.nodes.push(VizNode::new("d"));
        other.edges.push(VizEdge::new("c", "d", "CALLS"));
        // Retrait de c (et donc de ses arêtes) dans une seconde variante.
        other.nodes.retain(|n| n.id != "b");
        other.edges.retain(|e| e.src != "b" && e.dst != "b");
        let d = diff_subgraphs(&base, &other.canonical());
        assert_eq!(d.added_nodes.len(), 1); // d
        assert_eq!(d.added_edges.len(), 1); // c->d
        assert_eq!(d.removed_nodes.len(), 1); // b
        // b->c supprimée (b retiré) ; a->b aussi → 2 retraits d'arêtes.
        assert_eq!(d.removed_edges.len(), 2);
        assert_eq!(d.delta_count(), 5);
    }

    #[test]
    fn diff_json_is_deterministic_and_stable() {
        let base = sample();
        let mut other = base.clone();
        other.edges.push(VizEdge::new("c", "a", "IMPORTS"));
        let a = diff_json_string(&base, &other).unwrap();
        let b = diff_json_string(&base, &other).unwrap();
        assert_eq!(a, b);
        assert!(a.contains("\"added_edges\""), "{a}");
    }

    #[test]
    fn edge_identity_is_src_dst_type() {
        let mut base = sample();
        let mut other = sample();
        // Même extrémités, autre type → arête « ajoutée + retirée ».
        base.edges.push(VizEdge::new("a", "b", "IMPORTS"));
        other.edges.push(VizEdge::new("a", "b", "FRIENDS"));
        let d = diff_subgraphs(&base, &other);
        assert_eq!(d.added_edges.len(), 1);
        assert_eq!(d.removed_edges.len(), 1);
        assert_eq!(d.added_edges[0].edge_type, "FRIENDS");
        assert_eq!(d.removed_edges[0].edge_type, "IMPORTS");
    }

    #[test]
    fn timeline_three_snapshots_produces_two_steps() {
        let s0 = sample();
        let mut s1 = s0.clone();
        s1.nodes.push(VizNode::new("d"));
        s1.edges.push(VizEdge::new("c", "d", "CALLS"));
        let mut s2 = s1.clone();
        s2.nodes.retain(|n| n.id != "b");
        s2.edges.retain(|e| e.src != "b" && e.dst != "b");
        let snapshots = [&s0, &s1.canonical(), &s2.canonical()];
        let rep = timeline_subgraphs(&snapshots, 5);
        assert_eq!(rep.snapshots, 3);
        assert_eq!(rep.steps.len(), 2);
        // Étape 1 : +1 nœud (d), 0 retrait.
        assert_eq!(rep.steps[0].added_nodes, 1);
        assert_eq!(rep.steps[0].removed_nodes, 0);
        assert_eq!(rep.steps[0].sample_added_nodes, vec!["d".to_string()]);
        // Étape 2 : -1 nœud (b).
        assert_eq!(rep.steps[1].removed_nodes, 1);
        assert_eq!(rep.steps[1].sample_removed_nodes, vec!["b".to_string()]);
    }

    #[test]
    fn timeline_requires_two_snapshots_minimum() {
        let s = sample();
        let rep = timeline_subgraphs(&[&s], 5);
        assert_eq!(rep.snapshots, 1);
        assert!(rep.steps.is_empty());
    }
}
