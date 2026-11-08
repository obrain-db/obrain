//! Modules de langage — « une grammaire = un module » (RFC M6 §5).
//!
//! Chaque langage vit dans son fichier et implémente
//! [`crate::parse::Language`]. Ce module expose le registre pré-rempli des
//! langages branchés. Les 8 langages prioritaires (RFC M6 §5) sont branchés :
//! rust (référence), python, go, java, c, cpp, javascript, typescript.

mod c;
mod cpp;
mod go;
mod java;
mod javascript;
mod python;
mod rust;
mod typescript;

use crate::pack_langs;
use crate::parse::LanguageRegistry;
use crate::spec_langs;

/// Construit le registre des langages branchés.
///
/// Une extension de fichier = un langage (les extensions sont disjointes —
/// vérifié à l'intégration). `language_for_path` trouve le premier langage
/// dont les extensions contiennent celle du fichier.
pub fn build_registry() -> LanguageRegistry {
    let mut reg = LanguageRegistry::new();
    reg.register(Box::new(rust::RustLang::new()));
    reg.register(Box::new(python::PythonLang::new()));
    reg.register(Box::new(go::GoLang::new()));
    reg.register(Box::new(java::JavaLang::new()));
    reg.register(Box::new(c::CLang::new()));
    reg.register(Box::new(cpp::CppLang::new()));
    reg.register(Box::new(javascript::JavaScriptLang::new()));
    reg.register(Box::new(typescript::TypeScriptLang::new()));
    // Langages via le framework spec (lot 1 : json, yaml).
    spec_langs::register(&mut reg);
    // Langages manquants via tree-sitter-language-pack (371 grammaires).
    pack_langs::register(&mut reg);
    reg
}
