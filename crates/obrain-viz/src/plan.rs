//! Arbre d'opérateurs de plan → sous-graphe rendable (V2-D).
//!
//! Le moteur expose les plans EXPLAIN en **texte indenté** (2 espaces par
//! niveau, cf. `LogicalOperator::explain_tree`) avec les hints pushdown
//! annotés dans les libellés (`Filter (…) [index: p]`, `[label-first]`,
//! `[cap: n]`…). Ce module parse ce texte en un [`Subgraph`] d'opérateurs
//! (arêtes parent→enfant, type `PLAN`) — rendu ensuite par les writers
//! DOT/Mermaid existants, sans aucune dépendance au moteur.

use crate::ir::{Subgraph, VizEdge, VizNode};
use crate::{VizError, VizResult};

/// Construit un sous-graphe d'opérateurs depuis un plan EXPLAIN texte.
/// Déterministe : ids `op_001…` dans l'ordre du plan (préfixe zéro → tri
/// canonique préservant l'ordre de lecture).
pub fn tree_subgraph_from_explain(text: &str) -> VizResult<Subgraph> {
    let mut nodes: Vec<VizNode> = Vec::new();
    let mut edges: Vec<VizEdge> = Vec::new();
    // Pile (profondeur, index nœud) du chemin courant.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut seq: usize = 0;

    for raw in text.lines().filter(|l| !l.trim().is_empty()) {
        let trimmed = raw.trim_start();
        let lead = raw.len() - trimmed.len();
        if trimmed.is_empty() {
            continue;
        }
        let depth = lead / 2; // 2 espaces par niveau (tolérant à l'impair)
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            stack.pop();
        }
        let idx = nodes.len();
        let mut node = VizNode::new(format!("op_{:03}", seq));
        seq += 1;
        node.label = Some(trimmed.to_string());
        nodes.push(node);
        if let Some(&(_, parent)) = stack.last() {
            edges.push(VizEdge::new(
                format!("op_{:03}", parent),
                format!("op_{:03}", idx),
                "PLAN",
            ));
        }
        stack.push((depth, idx));
    }

    if nodes.is_empty() {
        return Err(VizError::Validation(
            "EXPLAIN : aucun opérateur à rendre (plan texte vide ?)".into(),
        ));
    }
    Ok(Subgraph {
        nodes,
        edges,
        truncated: false,
    }
    .canonical())
}

#[cfg(test)]
mod tests {
    use super::tree_subgraph_from_explain;

    #[test]
    fn parses_indented_tree_with_hints() {
        let plan = "\
Project ([n.id, r.type])
  Filter (n.label = 'x') [label-first]
    Expand (a)->[:CALLS](b)
      NodeScan (a:Function)
      NodeScan (n:*) 
  Limit (10)
";
        let sg = tree_subgraph_from_explain(plan).unwrap();
        assert_eq!(sg.nodes.len(), 6, "{sg:#?}");
        // Arêtes : Project→Filter, Filter→Expand, Expand→NS1, Expand→NS2,
        // Project→Limit (Limit est un 2e enfant de la racine, depth 1).
        assert_eq!(sg.edges.len(), 5);
        assert!(sg.validate().is_ok());
        // Ordre canonique : ids op_000.. op_004 (préfixe zéro).
        let ids: Vec<&str> = sg.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            ["op_000", "op_001", "op_002", "op_003", "op_004", "op_005"]
        );
        // Hint pushdown conservé dans le label.
        assert!(
            sg.nodes
                .iter()
                .any(|n| n.label.as_deref() == Some("Filter (n.label = 'x') [label-first]"))
        );
        // Racine = premier nœud (op_000) ; arêtes parent→enfant bien formées.
        assert!(sg.edges.iter().all(|e| e.edge_type == "PLAN"));
        // La racine n'a pas de parent.
        assert!(!sg.edges.iter().any(|e| e.dst == "op_000"));
    }

    #[test]
    fn join_has_two_sibling_subtrees() {
        let plan = "\
Join (Inner)
  NodeScan (a:Person)
  NodeScan (b:Person)
Project ([a.name])
";
        let sg = tree_subgraph_from_explain(plan).unwrap();
        assert_eq!(sg.nodes.len(), 4);
        // Join (op_000) a ses deux scans pour enfants ; Project (op_003) est
        // une seconde racine (sibling de Join, depth 0).
        assert_eq!(sg.edges.len(), 2);
        let join_children: Vec<&str> = sg
            .edges
            .iter()
            .filter(|e| e.src == "op_000")
            .map(|e| e.dst.as_str())
            .collect();
        assert_eq!(join_children, ["op_001", "op_002"]);
        assert!(!sg.edges.iter().any(|e| e.dst == "op_003"));
    }

    #[test]
    fn empty_plan_rejected() {
        assert!(tree_subgraph_from_explain("   \n\n").is_err());
    }

    #[test]
    fn deterministic_for_same_input() {
        let plan = "Aggregate (group: [n], aggs: [count(...)])\n  NodeScan (n:Function)\n";
        let a = tree_subgraph_from_explain(plan).unwrap();
        let b = tree_subgraph_from_explain(plan).unwrap();
        assert_eq!(a, b);
    }
}
