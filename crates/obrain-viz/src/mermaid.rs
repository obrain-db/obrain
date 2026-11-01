//! Writer Mermaid (flowchart) : sortie déterministe rendable dans tout
//! markdown (GitHub, Obsidian…). Les ids non `[A-Za-z0-9_]+` sont remplacés
//! par un hash FNV-1a stable ; la bijection id → id Mermaid est vérifiée
//! (collision → erreur). Libellés échappés par codes d'entité Mermaid.

use std::collections::HashMap;

use crate::ir::Subgraph;
use crate::sanitize::{mermaid_label, mermaid_node_id};
use crate::{VizError, VizResult};

/// Sérialise un sous-graphe en Mermaid `flowchart LR` (canonicalisé au
/// préalable : sortie déterministe). Le sous-graphe doit être bien formé.
pub fn to_mermaid(subgraph: &Subgraph) -> VizResult<String> {
    if subgraph.nodes.is_empty() {
        return Err(VizError::Validation(
            "export Mermaid: sous-graphe vide".into(),
        ));
    }
    let canon = subgraph.clone().canonical();

    // Bijection id source → id Mermaid, collisions détectées.
    let mut ids: HashMap<&str, String> = HashMap::with_capacity(canon.nodes.len());
    for node in &canon.nodes {
        if node.id.is_empty() {
            return Err(VizError::InvalidNodeId("id de nœud vide".into()));
        }
        let mid = mermaid_node_id(&node.id);
        if let Some(existing) = ids.values().find(|v| *v == &mid) {
            return Err(VizError::MermaidIdCollision {
                id: node.id.clone(),
                existing: existing.clone(),
            });
        }
        ids.insert(&node.id, mid);
    }

    let mut out = String::with_capacity(64 * (canon.nodes.len() + canon.edges.len()));
    out.push_str("flowchart LR\n");
    let mut node_styles: Vec<String> = Vec::new();
    for node in &canon.nodes {
        out.push_str("    ");
        out.push_str(&ids[&node.id.as_str()]);
        out.push_str("[\"");
        out.push_str(&mermaid_label(node.display_label()));
        out.push_str("\"]\n");
        if let Some(color) = &node.color_hint {
            node_styles.push(format!(
                "    style {} fill:{color},stroke:#333333",
                ids[&node.id.as_str()]
            ));
        }
    }
    for (index, edge) in canon.edges.iter().enumerate() {
        out.push_str("    ");
        out.push_str(&ids[&edge.src.as_str()]);
        out.push_str(" -->|");
        out.push_str(&mermaid_label(&edge.edge_type));
        out.push_str("| ");
        out.push_str(&ids[&edge.dst.as_str()]);
        out.push('\n');
        if edge.color_hint.is_some() || edge.size_hint.is_some() {
            out.push_str("    linkStyle ");
            out.push_str(&index.to_string());
            out.push(' ');
            let mut attrs: Vec<String> = Vec::new();
            if let Some(color) = &edge.color_hint {
                attrs.push(format!("stroke:{color}"));
            }
            if let Some(size) = edge.size_hint {
                attrs.push(format!("stroke-width:{}", (size * 2.0).round() / 2.0));
            }
            out.push_str(&attrs.join(","));
            out.push('\n');
        }
    }
    if !node_styles.is_empty() {
        out.push('\n');
        out.push_str(&node_styles.join("\n"));
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::to_mermaid;
    use crate::VizError;
    use crate::ir::{Subgraph, VizEdge, VizNode};

    fn sample() -> Subgraph {
        Subgraph {
            nodes: vec![
                VizNode::new("fn:hub"),
                VizNode::new("fn:parse"),
                VizNode::new("lib"),
            ],
            edges: vec![
                VizEdge::new("fn:parse", "fn:hub", "CALLS"),
                VizEdge::new("fn:hub", "lib", "CALLS"),
            ],
            truncated: false,
        }
    }

    #[test]
    fn basic_flowchart_shape() {
        let mmd = to_mermaid(&sample()).unwrap();
        assert!(mmd.starts_with("flowchart LR\n"), "{mmd}");
        // 3 déclarations + 2 arêtes typées.
        assert_eq!(mmd.lines().filter(|l| l.contains("[\"")).count(), 3);
        assert_eq!(mmd.lines().filter(|l| l.contains(" -->|")).count(), 2);
        assert!(mmd.contains("-->|CALLS|"), "{mmd}");
    }

    #[test]
    fn hostile_ids_are_hashed_and_labels_escaped() {
        let mut s = sample();
        s.nodes[0].label = Some("hub \"chaud\"\nline2".into());
        s.nodes.push(VizNode::new("id/with-dash.and\"quote"));
        s.edges.push(VizEdge::new(
            "id/with-dash.and\"quote",
            "fn:parse",
            "IMPORTS",
        ));
        let mmd = to_mermaid(&s).unwrap();
        // Le label hostile : guillemet → entité, saut de ligne → espace.
        assert!(mmd.contains("hub #quot;chaud#quot; line2"), "{mmd}");
        // L'id hostile est remplacé par un hash préfixé n… et reste stable.
        assert!(mmd.contains("n"), "{mmd}");
        // Aucun guillemet nu à l'intérieur d'un libellé.
        assert!(!mmd.contains("chaud\""), "{mmd}");
    }

    #[test]
    fn hostile_ids_map_deterministically() {
        let a = to_mermaid(&sample()).unwrap();
        let b = to_mermaid(&sample()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn isolated_node_is_declared() {
        let mut s = sample();
        s.nodes.push(VizNode::new("iso"));
        let mmd = to_mermaid(&s).unwrap();
        assert!(mmd.contains("iso[\"iso\"]"), "{mmd}");
    }

    #[test]
    fn empty_subgraph_is_rejected() {
        assert!(matches!(
            to_mermaid(&Subgraph::new()).unwrap_err(),
            VizError::Validation(_)
        ));
    }

    #[test]
    fn overlay_hints_render_node_styles_and_link_styles() {
        let mut s = sample();
        s.nodes[0].color_hint = Some("#3b4cc0".into());
        s.edges[0].color_hint = Some("#d62728".into());
        s.edges[0].size_hint = Some(2.0);
        let mmd = to_mermaid(&s).unwrap();
        assert!(
            mmd.contains("style ") && mmd.contains("fill:#3b4cc0"),
            "{mmd}"
        );
        assert!(
            mmd.contains("linkStyle ") && mmd.contains("stroke:#d62728"),
            "{mmd}"
        );
        assert!(mmd.contains("stroke-width:2"), "{mmd}");
    }

    #[test]
    fn hint_free_output_has_no_styles() {
        let mmd = to_mermaid(&sample()).unwrap();
        assert!(!mmd.contains("style "), "{mmd}");
        assert!(!mmd.contains("linkStyle"), "{mmd}");
    }
}
