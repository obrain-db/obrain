//! Génère les golden files de référence de `obrain-viz`
//! (`tests/golden/example.{json,dot,mmd}`) — utilisés par `tests/golden.rs`
//! pour verrouiller les octets de sortie (anti-régression de schéma).
//!
//! Usage : `cargo run -p obrain-viz --example print_golden`
//!
//! ⚠️ Le sous-graphe échantillon est **dupliqué volontairement** dans
//! `tests/golden.rs` : les deux copies doivent rester identiques — si le test
//! golden échoue, relancer cet exemple puis re-committer les fichiers.

use std::path::PathBuf;

use obrain_viz::{
    Overlay, Subgraph, VizEdge, VizNode, apply_overlay, to_dot, to_graphml, to_json_string,
    to_mermaid,
};

/// Échantillon canonique (déterministe) : ids hostiles (`:`, `/`, `-`, `.`),
/// libellés avec guillemet/saut de ligne, signaux (energy/curvature/scc/ricci)
/// pour verrouiller le schéma v1 dès maintenant.
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
    nodes[1].label = Some("hub \"central\"".into()); // guillemet hostile
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
    nodes[3].label = Some("lib\n(core)".into()); // saut de ligne hostile
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
    edges[1].stale = Some(true); // verrouille le champ `stale`
    edges[2].weight = Some(1.0);

    Subgraph {
        nodes,
        edges,
        truncated: false,
    }
    .canonical()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subgraph = sample();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("example.json"), to_json_string(&subgraph)?)?;
    std::fs::write(dir.join("example.dot"), to_dot(&subgraph)?)?;
    std::fs::write(dir.join("example.mmd"), to_mermaid(&subgraph)?)?;
    std::fs::write(dir.join("example.graphml"), to_graphml(&subgraph)?)?;

    // Variante overlay `energy` (verrouille le schéma json-graph v1 avec hints
    // color_hint/size_hint + le rendu writers correspondant).
    let (styled, report) = apply_overlay(&subgraph, Overlay::Energy);
    std::fs::write(dir.join("example.energy.json"), to_json_string(&styled)?)?;
    std::fs::write(dir.join("example.energy.dot"), to_dot(&styled)?)?;
    std::fs::write(dir.join("example.energy.mmd"), to_mermaid(&styled)?)?;

    println!(
        "golden écrits dans {} (overlay energy : {} nœuds stylés)",
        dir.display(),
        report.nodes_styled
    );
    Ok(())
}
