//! Extend Obrain with custom functions and algorithms.
//!
//! Plugins let you add new capabilities without modifying Obrain's core. Register
//! your custom algorithms with [`PluginRegistry`] to make them available to queries.
//!
//! The `algorithms` module (requires `algos` feature) includes ready-to-use
//! implementations of classic graph algorithms — traversals, shortest paths,
//! centrality measures, and more.

#[cfg(feature = "algos")]
pub mod algorithms;
mod registry;
mod traits;

// `registre_global` + `installer_udfs_dans_le_moteur` : le registre DU PROCESSUS
// et le pont vers l'évaluateur du moteur (`obrain-core`). Sans ces deux
// exports, le registre restait orphelin : on pouvait enregistrer une UDF sans
// qu'aucune requête puisse l'appeler.
pub use registry::{
    PluginRegistry, UserDefinedFunction, installer_udfs_dans_le_moteur, registre_global,
};
pub use traits::{Algorithm, AlgorithmResult, ParameterDef, ParameterType, Parameters, Plugin};
