//! Plugin registry.

use super::{Algorithm, Plugin};
use obrain_common::types::Value;
use obrain_common::utils::error::Result;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// A user-defined function (UDF) that can be called from query expressions.
///
/// UDFs take a node ID (as `Value::Int64`) and return a scalar `Value`.
/// They are registered by name (e.g., `"obrain.energy"`) and resolved at
/// query planning time.
pub trait UserDefinedFunction: Send + Sync {
    /// Returns the dotted name of this UDF (e.g., `"obrain.energy"`).
    fn name(&self) -> &str;

    /// Returns a short description.
    fn description(&self) -> &str;

    /// Evaluates the UDF for the given argument values.
    ///
    /// For cognitive UDFs the first argument is typically a node-ID (`Value::Int64`).
    fn evaluate(&self, args: &[Value]) -> Result<Value>;
}

/// Registry for managing plugins and algorithms.
pub struct PluginRegistry {
    /// Loaded plugins.
    plugins: RwLock<HashMap<String, Arc<dyn Plugin>>>,
    /// Registered algorithms.
    algorithms: RwLock<HashMap<String, Arc<dyn Algorithm>>>,
    /// Registered user-defined functions.
    udfs: RwLock<HashMap<String, Arc<dyn UserDefinedFunction>>>,
}

impl PluginRegistry {
    /// Creates a new empty registry.
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            algorithms: RwLock::new(HashMap::new()),
            udfs: RwLock::new(HashMap::new()),
        }
    }

    /// Registers a plugin.
    pub fn register_plugin(&self, plugin: Arc<dyn Plugin>) -> Result<()> {
        plugin.on_load()?;
        self.plugins
            .write()
            .insert(plugin.name().to_string(), plugin);
        Ok(())
    }

    /// Unregisters a plugin.
    pub fn unregister_plugin(&self, name: &str) -> Result<()> {
        if let Some(plugin) = self.plugins.write().remove(name) {
            plugin.on_unload()?;
        }
        Ok(())
    }

    /// Gets a plugin by name.
    pub fn get_plugin(&self, name: &str) -> Option<Arc<dyn Plugin>> {
        self.plugins.read().get(name).cloned()
    }

    /// Registers an algorithm.
    pub fn register_algorithm(&self, algorithm: Arc<dyn Algorithm>) {
        self.algorithms
            .write()
            .insert(algorithm.name().to_string(), algorithm);
    }

    /// Gets an algorithm by name.
    pub fn get_algorithm(&self, name: &str) -> Option<Arc<dyn Algorithm>> {
        self.algorithms.read().get(name).cloned()
    }

    /// Lists all registered plugins.
    pub fn list_plugins(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }

    /// Lists all registered algorithms.
    pub fn list_algorithms(&self) -> Vec<String> {
        self.algorithms.read().keys().cloned().collect()
    }

    /// Registers a user-defined function (UDF).
    pub fn register_udf(&self, udf: Arc<dyn UserDefinedFunction>) {
        self.udfs.write().insert(udf.name().to_string(), udf);
    }

    /// Gets a UDF by name.
    pub fn get_udf(&self, name: &str) -> Option<Arc<dyn UserDefinedFunction>> {
        self.udfs.read().get(name).cloned()
    }

    /// Lists all registered UDF names.
    pub fn list_udfs(&self) -> Vec<String> {
        self.udfs.read().keys().cloned().collect()
    }
}

/// ── LE REGISTRE DU PROCESSUS, ET LE PONT VERS LE MOTEUR ────────────────────
///
/// Défaut mesuré le 2026-09-16 : ce registre était **orphelin**. Il existait,
/// il était thread-safe, sa documentation annonçait « UDF … resolved at query
/// planning time » — et aucun appelant de production ne le créait ni ne le
/// lisait (seul un test l'instanciait). Une extension qu'on ne peut pas
/// atteindre n'est pas une extension.
///
/// Il y a désormais un registre PAR PROCESSUS, et un pont vers l'évaluateur :
/// `installer_udfs_dans_le_moteur` pose sur le noyau (`obrain-core`) un
/// résolveur qui consulte ce registre. Le noyau ne dépend pas de ce crate (c'est
/// l'inverse), donc le sens de la flèche est respecté : c'est l'hôte qui
/// s'installe, le noyau appelle.
pub fn registre_global() -> &'static PluginRegistry {
    static REGISTRE: std::sync::OnceLock<PluginRegistry> = std::sync::OnceLock::new();
    REGISTRE.get_or_init(PluginRegistry::new)
}

/// Installe, UNE fois par processus, le pont entre le registre d'UDF et
/// l'évaluateur du moteur.
///
/// Idempotent et sans coût mesurable : appelé à la construction d'une base, il
/// ne fait rien après la première fois. Sans cet appel, une UDF enregistrée
/// reste invisible à `RETURN mon_algo(n)` — le défaut même qu'on corrige.
pub fn installer_udfs_dans_le_moteur() {
    static PONT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    PONT.get_or_init(|| {
        // Le résolveur vit pour le processus : l'installer une fois, puis le
        // prêter au noyau sous forme `'static` (c'est le contrat de
        // `fonction_hote::installer`, qui n'accepte qu'un hôte).
        static RESOLVEUR: std::sync::OnceLock<
            Box<obrain_core::execution::fonction_hote::ResolveurFonctions>,
        > = std::sync::OnceLock::new();
        let resolveur = RESOLVEUR.get_or_init(|| {
            Box::new(|nom: &str, args: &[Value]| {
                let udf = registre_global().get_udf(nom)?;
                Some(udf.evaluate(args).map_err(|e| e.to_string()))
            })
        });
        // `Box` derrière un `OnceLock` : l'adresse ne bouge plus, donc prêter la
        // référence pour la vie du processus est sûr et sans fuite au-delà.
        let statique: &'static obrain_core::execution::fonction_hote::ResolveurFonctions =
            resolveur;
        let _ = obrain_core::execution::fonction_hote::installer(statique);
    });
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlugin;

    impl Plugin for TestPlugin {
        fn name(&self) -> &str {
            "test"
        }

        fn version(&self) -> &str {
            "1.0.0"
        }
    }

    #[test]
    fn test_plugin_registration() {
        let registry = PluginRegistry::new();

        let plugin = Arc::new(TestPlugin);
        registry.register_plugin(plugin).unwrap();

        assert!(registry.get_plugin("test").is_some());
        assert_eq!(registry.list_plugins(), vec!["test"]);
    }

    #[test]
    fn test_plugin_unregistration() {
        let registry = PluginRegistry::new();

        let plugin = Arc::new(TestPlugin);
        registry.register_plugin(plugin).unwrap();

        registry.unregister_plugin("test").unwrap();
        assert!(registry.get_plugin("test").is_none());
    }
}
