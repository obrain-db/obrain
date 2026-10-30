//! Parsing d'un fichier source (RFC M6 §2.3, phase « extract »).
//!
//! Le trait [`Language`] est le contrat « une grammaire = un module » (RFC M6
//! §5) : chaque langage parse un fichier avec sa grammaire tree-sitter et
//! extrait les symboles DÉFINIS, les APPELS, les IMPORTS et les références.
//! Le résultat par fichier est [`FileExtraction`] — l'équivalent natif du
//! `CBMFileResult` de l'indexeur C (defs/calls/usages/imports).

use std::path::Path;

use crate::schema::NodeKind;

/// Une définition de symbole extraite d'un fichier.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Definition {
    /// Nom court du symbole.
    pub name: String,
    /// Label de nœud cible (Function, Method, Class, Struct, Enum…).
    pub kind: NodeKind,
    /// Ligne de début (1-based).
    pub start_line: u32,
    /// Ligne de fin (1-based, inclusive).
    pub end_line: u32,
    /// Signature si disponible (ex: `fn foo(a: i32) -> bool`).
    pub signature: Option<String>,
    /// Nom de la classe/impl parente (pour Method/Field).
    pub parent_class: Option<String>,
    /// Métriques structurelles (complexity, cognitive…) — calculées par le
    /// framework spec / langages manuels quand la spec a des branch types.
    pub metrics: Option<crate::metrics::Metrics>,
    /// Docstring / commentaire précédant la définition (texte brut tronqué).
    pub docstring: Option<String>,
}

/// Un appel (call site) : le nom appelé et sa position. La résolution du
/// callee_qn se fait plus tard ([`crate::resolve`]), comme dans le C.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Call {
    /// Nom de la fonction/méthode appelée (segment final).
    pub name: String,
    /// Éventuel qualificatif (ex: `foo::bar` → name=bar, qualifier=foo).
    pub qualifier: Option<String>,
    /// Ligne de l'appel (1-based).
    pub line: u32,
}

/// Un import (ex: `use crate::foo::bar` / `import x from "y"`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Import {
    /// Module importé (le FQN du module, ex: `crate.foo`).
    pub module: String,
    /// Nom local éventuel (ex: `use foo::Bar as Baz` → Baz).
    pub local_name: Option<String>,
}

/// Un let-binding local (`let x = Type::new(); …` / `let x: Type = …`) — la
/// base de la passe « récepteur » : résoudre `x.méthode()` vers les méthodes
/// du type lié quand l'AST ne donne pas le type du récepteur.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Binding {
    /// Nom de la variable liée.
    pub name: String,
    /// Référence de type (annotation `let x: T` ou chemin du constructeur
    /// `let x = T::new()` — texte brut, segment final = nom court du type).
    pub type_ref: Option<String>,
    /// Ligne du `let` (1-based).
    pub line: u32,
    /// Initialiseur = appel de méthode `recv.méthode(…)` : la passe cascade
    /// type le binding via le TYPE DE RETOUR de la méthode résolue du
    /// récepteur (`let session = db.session(); session.execute(…)`).
    pub init_call: Option<(String, String)>,
    /// Initialiseur = appel de FONCTION LIBRE `fn_setup(…)` (pas un
    /// constructeur typé ni un appel de méthode) : la passe cascade type le
    /// binding via le TYPE DE RETOUR de la fonction libre connue du module
    /// (`fn setup() -> ObrainDB` puis `let db = setup(); db.…()`). Étape 2 du
    /// chantier « résolution sémantique ».
    pub init_fn: Option<String>,
}

/// Résultat complet de l'extraction d'un fichier.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FileExtraction {
    /// FQN du module (chemin sans extension) — rempli par le pipeline, pas
    /// par le langage.
    pub module_qn: String,
    pub definitions: Vec<Definition>,
    pub calls: Vec<Call>,
    pub imports: Vec<Import>,
    /// Bindings locaux (let) — passe « récepteur » (résolution CALLS).
    pub bindings: Vec<Binding>,
    /// Champs de struct (nom du struct, nom du champ, type brut) — la passe
    /// « récepteur » résout `self.field.method()` via le type du champ
    /// (étape 2 du chantier « résolution sémantique »).
    pub struct_fields: Vec<(String, String, String)>,
}

impl FileExtraction {
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty() && self.calls.is_empty() && self.imports.is_empty()
    }
}

/// Contrat d'un module de langage : reconnaître + extraire un fichier.
pub trait Language: Send + Sync {
    /// Extensions de fichiers reconnues (sans le point).
    fn extensions(&self) -> &'static [&'static str];

    /// Nom du langage (ex: "rust").
    fn name(&self) -> &'static str;

    /// Parse `source` et extrait définitions + appels + imports du fichier.
    fn extract(&self, source: &str, _path: &Path) -> FileExtraction;
}

/// Registre des langages branchés.
pub struct LanguageRegistry {
    langs: Vec<Box<dyn Language>>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self { langs: Vec::new() }
    }

    /// Enregistre un langage (toutes ses extensions).
    pub fn register(&mut self, lang: Box<dyn Language>) {
        self.langs.push(lang);
    }

    /// Sélectionne le langage pour un chemin (par extension).
    pub fn language_for_path(&self, path: &Path) -> Option<&dyn Language> {
        let ext = path.extension()?.to_str()?;
        self.langs
            .iter()
            .find(|l| l.extensions().contains(&ext))
            .map(|b| b.as_ref())
    }

    /// Nombre de langages enregistrés.
    pub fn len(&self) -> usize {
        self.langs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.langs.is_empty()
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct FakeLang;
    impl Language for FakeLang {
        fn extensions(&self) -> &'static [&'static str] {
            &["rs"]
        }
        fn name(&self) -> &'static str {
            "fake"
        }
        fn extract(&self, _source: &str, _path: &Path) -> FileExtraction {
            FileExtraction::default()
        }
    }

    #[test]
    fn registre_par_extension() {
        let mut reg = LanguageRegistry::new();
        reg.register(Box::new(FakeLang));
        assert_eq!(reg.len(), 1);
        assert!(reg.language_for_path(Path::new("a.rs")).is_some());
        assert!(reg.language_for_path(Path::new("a.py")).is_none());
        assert_eq!(
            reg.language_for_path(Path::new("a.rs")).unwrap().name(),
            "fake"
        );
    }
}
