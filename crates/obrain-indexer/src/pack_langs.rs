//! Langages fournis par `tree-sitter-language-pack` (371 grammaires).
//!
//! Le crate `tree-sitter-language-pack` fournit des grammaires pour 371
//! langages (chargées/téléchargées à la demande via `get_language`) — couvre
//! les langages de la spec C qui n'ont pas de crate Rust individuel à API
//! moderne (ada, cobol, clojure, scss, vue, svelte, awk, fish, ...).
//!
//! Stratégie : on enregistre via le pack CHAQUE spec de lang_specs.rs que le
//! pack supporte ET qui n'est pas déjà branchée par un crate dédié
//! (spec_langs.rs s'exécute avant — le registre prend le premier match par
//! extension, donc les dédiés gagnent).

use crate::lang_specs::LANG_SPECS;
use crate::parse::LanguageRegistry;
use crate::spec_engine::SpecLang;

/// Noms des langages déjà branchés par des crates dédiés (spec_langs.rs) ou
/// manuels (lang/) — on les saute pour ne pas dupliquer.
const DEDICATED: &[&str] = &[
    // lang/ manuels
    "rust",
    "python",
    "go",
    "java",
    "c",
    "cpp",
    "javascript",
    "typescript",
    // spec_langs.rs dédiés
    "json",
    "yaml",
    "toml",
    "markdown",
    "ruby",
    "php",
    "bash",
    "kotlin",
    "swift",
    "dart",
    "csharp",
    "scala",
    "lua",
    "haskell",
    "elixir",
    "erlang",
    "ocaml",
    "fsharp",
    "julia",
    "groovy",
    "r",
    "zig",
    "crystal",
    "dlang",
    "css",
    "html",
    "hcl",
    "cmake",
    "xml",
    "graphql",
    "nix",
    "elm",
    "ini",
    "properties",
    "jsonnet",
    "solidity",
    "vhdl",
    "powershell",
    "protobuf",
    "gleam",
    "odin",
    "racket",
    "scheme",
    "fortran",
    "cuda",
    "glsl",
    "hlsl",
    "objc",
    "jinja2",
    "templ",
    "starlark",
    "bicep",
    "gdscript",
    "luau",
    "ada",
    "systemverilog",
    "puppet",
    "slang",
    "matlab",
    "commonlisp",
    "verilog",
    "regex",
    "svelte",
];

/// Enregistre toutes les specs C supportées par le language-pack.
pub fn register(reg: &mut LanguageRegistry) {
    for spec in LANG_SPECS {
        if DEDICATED.contains(&spec.name) {
            continue;
        }
        if tree_sitter_language_pack::has_language(spec.name) {
            let lang = match spec.name {
                // cobol : la grammaire actuelle produit data_description (le C
                // avait data_description_entry — version antérieure).
                "cobol" => {
                    SpecLang::from_pack("cobol", "cobol").variable_types(&["data_description"])
                }
                _ => SpecLang::from_pack(spec.name, spec.name),
            };
            reg.register(Box::new(lang));
        }
    }
}
