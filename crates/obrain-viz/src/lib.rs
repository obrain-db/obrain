//! # obrain-viz — couche de restitution visuelle
//!
//! Bridge entre le store cognitif obrain (formes déjà calculées et persistées :
//! ricci, energy, scc/roles…) et ses consommateurs (LLM via bridge MCP, CLI,
//! markdown/artifacts). Un appel produit **un sous-graphe rendable**, exportable
//! dans plusieurs formats :
//!
//! - [`json_graph`] — `obrain/json-graph@1`, source de vérité versionnée
//!   (base du diff/timeline et de l'interop) ;
//! - [`dot`] — Graphviz DOT, rendu immédiat avec `dot -Tsvg` ;
//! - [`mermaid`] — Mermaid flowchart, rendu immédiat dans tout markdown.
//!
//! Garanties structurantes (porte P0) :
//! - **déterminisme octet-à-octet** : à entrée canonique identique, la sortie
//!   est identique (tri canonique + échappements purs, aucun horodatage par
//!   défaut) ;
//! - **bien formé** : [`Subgraph::validate`] rejette les arêtes pendantes et
//!   les ids dupliqués ;
//! - **une seule implémentation** : CLI et tools MCP appellent cette lib.
//!
//! Le crate est volontairement sans dépendance substrate : il ne connaît que
//! l'IR [`Subgraph`], ce qui le rend testable et câblable partout (les
//! extracteurs substrate vivront côté CLI/bridge).

mod diff;
mod dot;
mod graphml;
mod ir;
mod json_graph;
mod json_graph_v2;
mod mermaid;
mod overlay;
mod plan;
mod sanitize;

pub use diff::{
    EdgeRef, NodeRef, SubgraphDiff, TimelineReport, TimelineStep, diff_json_string, diff_subgraphs,
    timeline_subgraphs,
};
pub use dot::{DotOptions, to_dot};
pub use graphml::{to_graphml, xml_escape};
pub use ir::{Stats, Subgraph, VizEdge, VizNode, validate_edges, validate_ids};
pub use json_graph::{
    JSON_GRAPH_SCHEMA_V1, JsonGraphDoc, doc_from_subgraph, from_json_str, to_json_string,
    to_json_string_compact,
};
pub use json_graph_v2::{
    EdgeColumns, JSON_GRAPH_SCHEMA_V2, JsonGraphDocV2, from_json_str_v2, to_json_string_v2,
};
pub use mermaid::to_mermaid;
pub use overlay::{
    Overlay, OverlayReport, apply_overlay, color_ramp, pastel_for_id, pheromone_color, ricci_color,
};
pub use plan::tree_subgraph_from_explain;
pub use sanitize::{dot_escape, dot_quote, fnv1a64, mermaid_label, mermaid_node_id};

/// Erreurs de la couche viz.
#[derive(Debug, thiserror::Error)]
pub enum VizError {
    /// Id de nœud vide ou invalide pour un format donné.
    #[error("id de nœud invalide: {0}")]
    InvalidNodeId(String),
    /// Deux ids distincts produisent le même id Mermaid (collision de hash).
    #[error("collision d'id Mermaid entre {existing:?} et {id:?}")]
    MermaidIdCollision { id: String, existing: String },
    /// Sérialisation JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Échec de validation structurelle ([`Subgraph::validate`]).
    #[error("validation: {0}")]
    Validation(String),
}

/// Résultat typé de la couche viz.
pub type VizResult<T> = Result<T, VizError>;
