//! # obrain-indexer — indexeur de code natif (RFC M6)
//!
//! Remplace le rôle de l'indexeur C externe `codebase-memory-mcp` : parse un
//! dépôt de code source (tree-sitter), résout les symboles et les appels, et
//! produit un graphe de code (nœuds `Function`/`Method`/`Class`/`File`…,
//! arêtes `CALLS`/`USAGE`/`IMPORTS`/`DEFINES`…) — le contrat de la RFC M6
//! (`docs/rfc/m6-code-graph-natif.md`, §2.1bis).
//!
//! Architecture (RFC M6 §5) :
//!
//! - [`discovery`] — découvre les fichiers d'un dépôt (gitignore/.cbmignore,
//!   tailles max), crée les nœuds `Project`/`Folder`/`File`.
//! - [`parse`] — parse un fichier avec une grammaire tree-sitter (une
//!   grammaire = un module de langage) et extrait les définitions.
//! - [`schema`] — le contrat cible : labels de nœuds, types d'arêtes, FQN.
//! - [`resolve`] — résolution des appels (registre à confiance décroissante).
//! - [`graph`] — le graphe en cours de construction (buffers de nœuds/arêtes).
//! - [`index`] — l'orchestration : pipeline ordonné d'une indexation complète.
//!
//! L'écriture dans le substrate (bulk zone-direct) est une étape ultérieure
//! (RFC M6 §5 point 4) — le crate produit d'abord un [`CodeGraph`] en mémoire,
//! consommable par le moteur.
//!
//! ## Statut
//!
//! Squelette initial (RFC en cours de validation) : types de base + modules
//! déclarés. Le parsing effectif, la résolution et l'écriture substrate sont
//! les étapes suivantes (RFC M6 §5).

pub mod discovery;
pub mod docs;
pub mod git_ctx;
pub mod git_history;
pub mod graph;
pub mod index;
pub mod lang;
pub mod lang_specs;
pub mod metrics;
pub mod pack_langs;
pub mod parse;
pub mod resolve;
pub mod schema;
pub mod spec_engine;
pub mod spec_langs;
pub mod tests_detect;

pub use graph::{CodeGraph, Edge, Node};
pub use index::IndexOptions;
pub use schema::{EdgeKind, NodeKind};

/// Version du contrat de graphe produit (indépendante de
/// `SUBSTRATE_FORMAT_VERSION` — ce crate écrit dans le format substrate
/// existant, n'en crée pas un nouveau).
pub const INDEXER_CONTRACT_VERSION: u32 = 1;
