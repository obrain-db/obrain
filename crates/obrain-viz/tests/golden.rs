//! Anti-régression de schéma : compare les octets de sortie actuels aux golden
//! files de référence (`tests/golden/example.{json,dot,mmd}`).
//!
//! Régénération après évolution volontaire du schéma :
//! `cargo run -p obrain-viz --example print_golden`
//! (l'échantillon est dupliqué dans l'exemple et ici — les garder identiques).

use obrain_viz::{
    Overlay, Subgraph, VizEdge, VizNode, apply_overlay, to_dot, to_graphml, to_json_string,
    to_mermaid,
};

/// Échantillon canonique — copie volontairement identique à
/// `examples/print_golden.rs::sample`.
fn sample() -> Subgraph {
    let mut nodes = vec![
        VizNode::new("dep:/usr/deps.json"),
        VizNode::new("fn:hub"),
        VizNode::new("fn:parse"),
        VizNode::new("crate:lib"),
        VizNode::new("iso"),
    ];
    nodes[0].qn = Some("serde_json".into());
    nodes[0].label = Some("serde_json".into());
    nodes[0].file_path = Some("Cargo.lock".into());
    nodes[0].scc_id = Some(2);
    nodes[1].qn = Some("obrain_viz::hub".into());
    nodes[1].label = Some("hub \"central\"".into());
    nodes[1].file_path = Some("crates/obrain-viz/src/hub.rs".into());
    nodes[1].energy = Some(0.9);
    nodes[1].scc_id = Some(3);
    nodes[1].engram_state = Some("stable".into());
    nodes[2].qn = Some("obrain_viz::parse".into());
    nodes[2].label = Some("parse()".into());
    nodes[2].file_path = Some("crates/obrain-viz/src/parse.rs".into());
    nodes[2].energy = Some(0.75);
    nodes[2].curvature = Some(-0.2);
    nodes[2].scc_id = Some(3);
    nodes[3].qn = Some("obrain_viz".into());
    nodes[3].label = Some("lib\n(core)".into());
    nodes[3].file_path = Some("crates/obrain-viz/src/lib.rs".into());
    nodes[3].curvature = Some(0.1);
    nodes[3].scc_id = Some(1);
    nodes[4].qn = Some("obrain_viz::iso".into());
    nodes[4].label = Some("isolé".into());
    nodes[4].file_path = Some("crates/obrain-viz/src/iso.rs".into());
    nodes[4].energy = Some(0.5);
    nodes[4].scc_id = Some(0);

    let mut edges = vec![
        VizEdge::new("fn:parse", "fn:hub", "CALLS"),
        VizEdge::new("fn:hub", "crate:lib", "CALLS"),
        VizEdge::new("fn:parse", "dep:/usr/deps.json", "IMPORTS"),
    ];
    edges[0].weight = Some(3.0);
    edges[0].ricci = Some(-0.35);
    edges[1].ricci = Some(-0.12);
    edges[1].stale = Some(true);
    edges[2].weight = Some(1.0);

    Subgraph {
        nodes,
        edges,
        truncated: false,
    }
    .canonical()
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(name),
    )
    .unwrap_or_else(|e| {
        panic!("golden {name} illisible: {e} (cargo run -p obrain-viz --example print_golden)")
    })
}

fn assert_same(name: &str, actual: &str) {
    let expected = golden(name);
    assert_eq!(
        expected, actual,
        "dérive de schéma sur {name} — si volontaire, régénérer : \
         cargo run -p obrain-viz --example print_golden"
    );
}

#[test]
fn json_matches_golden() {
    assert_same("example.json", &to_json_string(&sample()).unwrap());
}

#[test]
fn dot_matches_golden() {
    assert_same("example.dot", &to_dot(&sample()).unwrap());
}

#[test]
fn mermaid_matches_golden() {
    assert_same("example.mmd", &to_mermaid(&sample()).unwrap());
}

#[test]
fn graphml_matches_golden() {
    assert_same("example.graphml", &to_graphml(&sample()).unwrap());
}

/// Variante overlay `energy` : mêmes assertions, sous-graphe stylé.
fn energy_styled() -> Subgraph {
    let (styled, _) = apply_overlay(&sample(), Overlay::Energy);
    styled
}

#[test]
fn json_overlay_matches_golden() {
    assert_same(
        "example.energy.json",
        &to_json_string(&energy_styled()).unwrap(),
    );
}

#[test]
fn dot_overlay_matches_golden() {
    assert_same("example.energy.dot", &to_dot(&energy_styled()).unwrap());
}

#[test]
fn mermaid_overlay_matches_golden() {
    assert_same("example.energy.mmd", &to_mermaid(&energy_styled()).unwrap());
}
