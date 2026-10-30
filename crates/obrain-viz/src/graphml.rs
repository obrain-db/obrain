//! Writer GraphML (V1-C, formats interop) : sortie déterministe, échappement
//! XML complet (`& < > " '`), réimportable (networkx/Gephi/Cytoscape…).
//!
//! Clés déclarées : nœuds `qn`/`label`/`file`, arêtes `type`/`weight`/`ricci`.
//! Les `<data>` absents sont omis (schéma minimal stable).

use crate::ir::Subgraph;
use crate::{VizError, VizResult};

/// Sérialise un sous-graphe en GraphML (canonicalisé : sortie déterministe).
pub fn to_graphml(subgraph: &Subgraph) -> VizResult<String> {
    if subgraph.nodes.is_empty() {
        return Err(VizError::Validation(
            "export GraphML: sous-graphe vide".into(),
        ));
    }
    let canon = subgraph.clone().canonical();
    let mut out = String::with_capacity(128 * (canon.nodes.len() + canon.edges.len()));
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">\n");
    out.push_str(
        "  <key id=\"qn\" for=\"node\" attr.name=\"qualified_name\" attr.type=\"string\"/>\n",
    );
    out.push_str("  <key id=\"label\" for=\"node\" attr.name=\"label\" attr.type=\"string\"/>\n");
    out.push_str(
        "  <key id=\"file\" for=\"node\" attr.name=\"file_path\" attr.type=\"string\"/>\n",
    );
    out.push_str("  <key id=\"etype\" for=\"edge\" attr.name=\"type\" attr.type=\"string\"/>\n");
    out.push_str("  <key id=\"weight\" for=\"edge\" attr.name=\"weight\" attr.type=\"double\"/>\n");
    out.push_str("  <key id=\"ricci\" for=\"edge\" attr.name=\"ricci\" attr.type=\"double\"/>\n");
    out.push_str("  <graph id=\"obrain\" edgedefault=\"directed\">\n");
    for node in &canon.nodes {
        out.push_str("    <node id=\"");
        out.push_str(&xml_escape(&node.id));
        out.push_str("\">");
        if let Some(qn) = &node.qn {
            out.push_str("<data key=\"qn\">");
            out.push_str(&xml_escape(qn));
            out.push_str("</data>");
        }
        if let Some(label) = &node.label {
            out.push_str("<data key=\"label\">");
            out.push_str(&xml_escape(label));
            out.push_str("</data>");
        }
        if let Some(file) = &node.file_path {
            out.push_str("<data key=\"file\">");
            out.push_str(&xml_escape(file));
            out.push_str("</data>");
        }
        out.push_str("</node>\n");
    }
    for edge in &canon.edges {
        out.push_str("    <edge source=\"");
        out.push_str(&xml_escape(&edge.src));
        out.push_str("\" target=\"");
        out.push_str(&xml_escape(&edge.dst));
        out.push_str("\">");
        out.push_str("<data key=\"etype\">");
        out.push_str(&xml_escape(&edge.edge_type));
        out.push_str("</data>");
        if let Some(w) = edge.weight {
            out.push_str("<data key=\"weight\">");
            out.push_str(&format!("{w}"));
            out.push_str("</data>");
        }
        if let Some(r) = edge.ricci {
            out.push_str("<data key=\"ricci\">");
            out.push_str(&format!("{r}"));
            out.push_str("</data>");
        }
        out.push_str("</edge>\n");
    }
    out.push_str("  </graph>\n");
    out.push_str("</graphml>\n");
    Ok(out)
}

/// Échappement XML (`& < > " '`) — une seule règle pour ids, labels, données.
#[must_use]
pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{to_graphml, xml_escape};
    use crate::ir::{Subgraph, VizEdge, VizNode};

    fn sample() -> Subgraph {
        let mut n = VizNode::new("fn:hub");
        n.label = Some("hub \"chaud\" <&> \n x".into());
        n.qn = Some("o::hub".into());
        let mut n2 = VizNode::new("fn:parse");
        n2.file_path = Some("crates/a&b.rs".into());
        let mut e = VizEdge::new("fn:hub", "fn:parse", "CALLS");
        e.weight = Some(1.5);
        e.ricci = Some(-0.25);
        Subgraph {
            nodes: vec![n, n2],
            edges: vec![e],
            truncated: false,
        }
    }

    #[test]
    fn xml_escape_covers_all_entities() {
        assert_eq!(xml_escape("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
    }

    #[test]
    fn graphml_shape_and_escaping() {
        let g = to_graphml(&sample()).unwrap();
        assert!(g.starts_with("<?xml version=\"1.0\""), "{g}");
        assert!(g.contains("<graph id=\"obrain\" edgedefault=\"directed\">"));
        // Label hostile échappé (pas de < ou > bruts ; entités &quot;/&lt;/&amp;/&gt;).
        assert!(g.contains("hub &quot;chaud&quot; &lt;&amp;&gt;"), "{g}");
        assert!(!g.contains("<&>"), "{g}");
        assert!(!g.contains("&lt;<"), "{g}");
        // Ids échappés dans les attributs.
        assert!(
            g.contains("<data key=\"file\">crates/a&amp;b.rs</data>"),
            "{g}"
        );
        // Données arête : type + poids + ricci.
        assert!(g.contains("<data key=\"etype\">CALLS</data>"), "{g}");
        assert!(g.contains("<data key=\"weight\">1.5</data>"), "{g}");
        assert!(g.contains("<data key=\"ricci\">-0.25</data>"), "{g}");
    }

    #[test]
    fn graphml_is_deterministic() {
        let a = to_graphml(&sample()).unwrap();
        let b = to_graphml(&sample()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn graphml_empty_rejected() {
        assert!(to_graphml(&Subgraph::new()).is_err());
    }
}
