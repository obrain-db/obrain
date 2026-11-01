//! Writer Graphviz DOT : sortie déterministe, ids toujours entre guillemets
//! (échappés), libellés échappés. Rendu immédiat : `dot -Tsvg`.

use crate::ir::Subgraph;
use crate::sanitize::dot_quote;
use crate::{VizError, VizResult};

/// Options d'export DOT.
#[derive(Debug, Clone, Copy)]
pub struct DotOptions {
    /// Vrai (défaut) : `digraph` ; faux : `graph` (arêtes non dirigées).
    pub directed: bool,
    /// Orientation du rang (`rankdir` : `LR`, `TB`…). `None` = attribut omis.
    pub rankdir: Option<&'static str>,
}

impl Default for DotOptions {
    fn default() -> Self {
        Self {
            directed: true,
            rankdir: None,
        }
    }
}

impl DotOptions {
    /// Options par défaut : digraphe, pas de `rankdir`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn keyword(&self) -> &'static str {
        if self.directed { "digraph" } else { "graph" }
    }

    fn edge_op(&self) -> &'static str {
        if self.directed { "->" } else { "--" }
    }
}

/// Sérialise un sous-graphe en DOT (canonicalisé au préalable : sortie
/// déterministe). Le sous-graphe doit être bien formé (ids non vides).
pub fn to_dot(subgraph: &Subgraph) -> VizResult<String> {
    to_dot_with_options(subgraph, &DotOptions::new())
}

/// Variante paramétrable ([`DotOptions`]).
pub fn to_dot_with_options(subgraph: &Subgraph, options: &DotOptions) -> VizResult<String> {
    if subgraph.nodes.is_empty() {
        return Err(VizError::Validation("export DOT: sous-graphe vide".into()));
    }
    for node in &subgraph.nodes {
        if node.id.is_empty() {
            return Err(VizError::InvalidNodeId("id de nœud vide".into()));
        }
    }
    let canon = subgraph.clone().canonical();
    let mut out = String::with_capacity(64 * (canon.nodes.len() + canon.edges.len()));
    out.push_str(options.keyword());
    out.push_str(" obrain_viz {\n");
    if let Some(rankdir) = options.rankdir {
        out.push_str("    rankdir=");
        out.push_str(rankdir);
        out.push_str(";\n");
    }
    for node in &canon.nodes {
        out.push_str("    ");
        out.push_str(&dot_quote(&node.id));
        out.push_str(" [label=");
        out.push_str(&dot_quote(node.display_label()));
        out.push(']');
        if let Some(file) = &node.file_path {
            out.push_str(" [tooltip=");
            out.push_str(&dot_quote(file));
            out.push(']');
        }
        if let Some(color) = &node.color_hint {
            out.push_str(" [style=\"filled\", fillcolor=");
            out.push_str(&dot_quote(color));
            out.push(']');
        }
        out.push_str(";\n");
    }
    for edge in &canon.edges {
        out.push_str("    ");
        out.push_str(&dot_quote(&edge.src));
        out.push(' ');
        out.push_str(options.edge_op());
        out.push(' ');
        out.push_str(&dot_quote(&edge.dst));
        out.push_str(" [label=");
        out.push_str(&dot_quote(&edge.edge_type));
        out.push(']');
        if edge.color_hint.is_some() || edge.size_hint.is_some() {
            out.push_str(" [");
            if let Some(color) = &edge.color_hint {
                out.push_str("color=");
                out.push_str(&dot_quote(color));
            }
            if let Some(size) = edge.size_hint {
                if edge.color_hint.is_some() {
                    out.push_str(", ");
                }
                out.push_str("penwidth=");
                out.push_str(&format!("{size:.2}"));
            }
            out.push(']');
        }
        out.push_str(";\n");
    }
    out.push_str("}\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{DotOptions, to_dot, to_dot_with_options};
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
    fn basic_digraph_shape() {
        let dot = to_dot(&sample()).unwrap();
        assert!(dot.starts_with("digraph obrain_viz {"), "{dot}");
        assert_eq!(dot.lines().filter(|l| l.contains("->")).count(), 2);
        assert_eq!(dot.lines().filter(|l| l.contains("[label=")).count(), 5);
        assert!(dot.ends_with("}\n"), "{dot}");
    }

    #[test]
    fn undirected_option_uses_graph_and_dash() {
        let dot = to_dot_with_options(&sample(), &DotOptions::new().directed_or(false)).unwrap();
        assert!(dot.starts_with("graph obrain_viz {"), "{dot}");
        assert!(!dot.contains("->"));
        assert!(dot.contains("--"));
    }

    #[test]
    fn hostile_ids_and_labels_are_quoted_and_escaped() {
        let mut s = sample();
        s.nodes[0].label = Some("hub \"chaud\"\nline2".into());
        let mut hostile = VizNode::new("id/with-dash.and\"quote");
        hostile.qn = Some("a\\b".into());
        s.nodes.push(hostile);
        s.edges.push(VizEdge::new(
            "id/with-dash.and\"quote",
            "fn:parse",
            "IMPORTS",
        ));
        let dot = to_dot(&s).unwrap();
        // L'id hostile doit apparaître échappé entre guillemets.
        assert!(dot.contains("\"id/with-dash.and\\\"quote\""), "{dot}");
        // Le libellé hostile : guillemet échappé, saut de ligne → \n littéral.
        assert!(dot.contains("hub \\\"chaud\\\"\\nline2"), "{dot}");
        // Aucun guillemet nu hors structure.
        assert!(!dot.contains("chaud\""), "{dot}");
    }

    #[test]
    fn output_is_byte_deterministic() {
        let shuffled = sample();
        let a = to_dot(&shuffled.clone().canonical()).unwrap();
        let b = to_dot(&shuffled).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn empty_subgraph_is_rejected() {
        let err = to_dot(&Subgraph::new()).unwrap_err();
        assert!(matches!(err, VizError::Validation(_)));
    }

    #[test]
    fn isolated_node_is_declared() {
        let mut s = sample();
        s.nodes.push(VizNode::new("iso"));
        let dot = to_dot(&s).unwrap();
        assert!(dot.contains("\"iso\" [label=\"iso\"]"), "{dot}");
    }

    #[test]
    fn overlay_hints_render_fillcolor_color_and_penwidth() {
        let mut s = sample();
        s.nodes[0].color_hint = Some("#3b4cc0".into());
        s.edges[0].color_hint = Some("#d62728".into());
        s.edges[0].size_hint = Some(2.5);
        let dot = to_dot(&s).unwrap();
        assert!(
            dot.contains("style=\"filled\", fillcolor=\"#3b4cc0\""),
            "{dot}"
        );
        assert!(dot.contains("color=\"#d62728\", penwidth=2.50"), "{dot}");
    }

    #[test]
    fn hint_free_output_has_no_style_attributes() {
        let dot = to_dot(&sample()).unwrap();
        assert!(!dot.contains("fillcolor"), "{dot}");
        assert!(!dot.contains("penwidth"), "{dot}");
    }

    trait DirectedExt {
        fn directed_or(self, directed: bool) -> Self;
    }
    impl DirectedExt for DotOptions {
        fn directed_or(mut self, directed: bool) -> Self {
            self.directed = directed;
            self
        }
    }
}
