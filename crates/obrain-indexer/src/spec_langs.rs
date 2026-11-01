//! Langages branchés via le framework spec (SpecLang) — lot 1 pilote.
//!
//! Chaque langage = l'adaptateur [`crate::spec_engine::SpecLang`] avec sa
//! grammaire tree-sitter + sa spec dans `lang_specs.rs`. L'extraction est
//! 100 % générique (spec_engine) — aucun code par langage.
//!
//! Seules les grammaires à l'API tree-sitter MODERNE (`LanguageFn`) sont
//! branchables ici. Les grammaires obsolètes (ex: toml 0.20, markdown 0.7 qui
//! traînent tree-sitter 0.19) exigent un fork à jour — voir le plan de
//! délégation (§3, contrainte grammaires).

use crate::parse::LanguageRegistry;
use crate::spec_engine::SpecLang;

/// Grammaires à API moderne branchées (json, yaml — lot 1 validé).
fn grammar_ruby() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_ruby::LANGUAGE.into())
        .expect("grammaire ruby");
    p
}

fn grammar_php() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_php::LANGUAGE_PHP.into())
        .expect("grammaire php");
    p
}

fn grammar_bash() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("grammaire bash");
    p
}

fn grammar_kotlin() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
        .expect("grammaire kotlin");
    p
}

fn grammar_swift() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_swift::LANGUAGE.into())
        .expect("grammaire swift");
    p
}

fn grammar_dart() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_dart::LANGUAGE.into())
        .expect("grammaire dart");
    p
}

fn grammar_c_sharp() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .expect("grammaire c_sharp");
    p
}

fn grammar_scala() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_scala::LANGUAGE.into())
        .expect("grammaire scala");
    p
}

fn grammar_lua() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_lua::LANGUAGE.into())
        .expect("grammaire lua");
    p
}

fn grammar_haskell() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_haskell::LANGUAGE.into())
        .expect("grammaire haskell");
    p
}

fn grammar_elixir() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_elixir::LANGUAGE.into())
        .expect("grammaire elixir");
    p
}

fn grammar_erlang() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_erlang::LANGUAGE.into())
        .expect("grammaire erlang");
    p
}

fn grammar_ocaml() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_ocaml::LANGUAGE_OCAML.into())
        .expect("grammaire ocaml");
    p
}

fn grammar_fsharp() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_fsharp::LANGUAGE_FSHARP.into())
        .expect("grammaire fsharp");
    p
}

fn grammar_julia() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_julia::LANGUAGE.into())
        .expect("grammaire julia");
    p
}

fn grammar_groovy() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_groovy::LANGUAGE.into())
        .expect("grammaire groovy");
    p
}

fn grammar_r() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("grammaire r");
    p
}

fn grammar_zig() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_zig::LANGUAGE.into())
        .expect("grammaire zig");
    p
}

fn grammar_crystal() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_crystal::LANGUAGE.into())
        .expect("grammaire crystal");
    p
}

fn grammar_dlang() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_d::LANGUAGE.into())
        .expect("grammaire dlang");
    p
}

fn grammar_css() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_css::LANGUAGE.into())
        .expect("grammaire css");
    p
}

fn grammar_html() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_html::LANGUAGE.into())
        .expect("grammaire html");
    p
}

fn grammar_hcl() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_hcl::LANGUAGE.into())
        .expect("grammaire hcl");
    p
}

fn grammar_cmake() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_cmake::LANGUAGE.into())
        .expect("grammaire cmake");
    p
}

fn grammar_xml() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_xml::LANGUAGE_XML.into())
        .expect("grammaire xml");
    p
}

fn grammar_graphql() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_graphql::LANGUAGE.into())
        .expect("grammaire graphql");
    p
}

fn grammar_nix() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_nix::LANGUAGE.into())
        .expect("grammaire nix");
    p
}

fn grammar_elm() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_elm::LANGUAGE.into())
        .expect("grammaire elm");
    p
}

fn grammar_ini() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_ini::LANGUAGE.into())
        .expect("grammaire ini");
    p
}

fn grammar_properties() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_properties::LANGUAGE.into())
        .expect("grammaire properties");
    p
}

fn grammar_jsonnet() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_jsonnet::LANGUAGE.into())
        .expect("grammaire jsonnet");
    p
}

fn grammar_solidity() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_solidity::LANGUAGE.into())
        .expect("grammaire solidity");
    p
}

fn grammar_vhdl() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_vhdl::LANGUAGE.into())
        .expect("grammaire vhdl");
    p
}

fn grammar_powershell() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_powershell::LANGUAGE.into())
        .expect("grammaire powershell");
    p
}

fn grammar_proto() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_proto::LANGUAGE.into())
        .expect("grammaire proto");
    p
}

fn grammar_gleam() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_gleam::LANGUAGE.into())
        .expect("grammaire gleam");
    p
}

fn grammar_odin() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_odin::LANGUAGE.into())
        .expect("grammaire odin");
    p
}

fn grammar_racket() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_racket::LANGUAGE.into())
        .expect("grammaire racket");
    p
}

fn grammar_scheme() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_scheme::LANGUAGE.into())
        .expect("grammaire scheme");
    p
}

fn grammar_fortran() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_fortran::LANGUAGE.into())
        .expect("grammaire fortran");
    p
}

fn grammar_cuda() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_cuda::LANGUAGE.into())
        .expect("grammaire cuda");
    p
}

fn grammar_glsl() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_glsl::LANGUAGE_GLSL.into())
        .expect("grammaire glsl");
    p
}

fn grammar_hlsl() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_hlsl::LANGUAGE_HLSL.into())
        .expect("grammaire hlsl");
    p
}

fn grammar_objc() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_objc::LANGUAGE.into())
        .expect("grammaire objc");
    p
}

fn grammar_jinja2() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_jinja2::LANGUAGE.into())
        .expect("grammaire jinja2");
    p
}

fn grammar_templ() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_templ::LANGUAGE.into())
        .expect("grammaire templ");
    p
}

fn grammar_starlark() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_starlark::LANGUAGE.into())
        .expect("grammaire starlark");
    p
}

fn grammar_bicep() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_bicep::LANGUAGE.into())
        .expect("grammaire bicep");
    p
}

fn grammar_gdscript() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_gdscript::LANGUAGE.into())
        .expect("grammaire gdscript");
    p
}

fn grammar_luau() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_luau::LANGUAGE.into())
        .expect("grammaire luau");
    p
}

fn grammar_ada() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_ada::LANGUAGE.into())
        .expect("grammaire ada");
    p
}

fn grammar_systemverilog() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_systemverilog::LANGUAGE.into())
        .expect("grammaire systemverilog");
    p
}

fn grammar_puppet() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_puppet::LANGUAGE.into())
        .expect("grammaire puppet");
    p
}

fn grammar_slang() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_slang::LANGUAGE_SLANG.into())
        .expect("grammaire slang");
    p
}

fn grammar_matlab() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_matlab::LANGUAGE.into())
        .expect("grammaire matlab");
    p
}

fn grammar_commonlisp() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_commonlisp::LANGUAGE_COMMONLISP.into())
        .expect("grammaire commonlisp");
    p
}

fn grammar_verilog() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_verilog::LANGUAGE.into())
        .expect("grammaire verilog");
    p
}

fn grammar_regex() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_regex::LANGUAGE.into())
        .expect("grammaire regex");
    p
}

fn grammar_svelte() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_svelte_ng::LANGUAGE.into())
        .expect("grammaire svelte");
    p
}

fn grammar_json() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_json::LANGUAGE.into())
        .expect("grammaire json");
    p
}

fn grammar_markdown() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_md::LANGUAGE.into())
        .expect("grammaire markdown");
    p
}

fn grammar_toml() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_toml_ng::LANGUAGE.into())
        .expect("grammaire toml");
    p
}

fn grammar_yaml() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_yaml::LANGUAGE.into())
        .expect("grammaire yaml");
    p
}

/// Enregistre les langages spec-compatibles dans `reg`.
pub fn register(reg: &mut LanguageRegistry) {
    reg.register(Box::new(SpecLang::new("json", grammar_json)));
    reg.register(Box::new(SpecLang::new("yaml", grammar_yaml)));
    reg.register(Box::new(SpecLang::new("markdown", grammar_markdown)));
    reg.register(Box::new(SpecLang::new("toml", grammar_toml)));
    // Lot 2 : langages courants.
    reg.register(Box::new(SpecLang::new("ruby", grammar_ruby)));
    reg.register(Box::new(SpecLang::new("php", grammar_php)));
    reg.register(Box::new(SpecLang::new("bash", grammar_bash)));
    reg.register(Box::new(SpecLang::new("kotlin", grammar_kotlin)));
    reg.register(Box::new(SpecLang::new("swift", grammar_swift)));
    // dart : la grammaire actuelle produit class_declaration (le C avait
    // class_definition — version antérieure). Override nécessaire.
    reg.register(Box::new(SpecLang::new("dart", grammar_dart).class_types(
        &[
            "class_declaration",
            "enum_declaration",
            "mixin_declaration",
            "type_alias",
        ],
    )));
    reg.register(Box::new(SpecLang::new("csharp", grammar_c_sharp)));
    reg.register(Box::new(SpecLang::new("scala", grammar_scala)));
    reg.register(Box::new(SpecLang::new("lua", grammar_lua)));
    reg.register(Box::new(SpecLang::new("haskell", grammar_haskell)));
    reg.register(Box::new(SpecLang::new("elixir", grammar_elixir)));
    reg.register(Box::new(SpecLang::new("erlang", grammar_erlang)));
    reg.register(Box::new(SpecLang::new("ocaml", grammar_ocaml)));
    reg.register(Box::new(SpecLang::new("fsharp", grammar_fsharp)));
    reg.register(Box::new(SpecLang::new("julia", grammar_julia)));
    // groovy/crystal : les grammaires actuelles produisent class_declaration
    // (les specs C avaient class_definition/class_def — versions antérieures).
    reg.register(Box::new(
        SpecLang::new("groovy", grammar_groovy).class_types(&["class_declaration"]),
    ));
    reg.register(Box::new(SpecLang::new("r", grammar_r)));
    reg.register(Box::new(SpecLang::new("zig", grammar_zig)));
    reg.register(Box::new(
        SpecLang::new("crystal", grammar_crystal)
            .class_types(&[
                "class_declaration",
                "struct_declaration",
                "module_declaration",
                "enum_declaration",
            ])
            .function_types(&["method_definition", "def"]),
    ));
    reg.register(Box::new(SpecLang::new("dlang", grammar_dlang)));
    reg.register(Box::new(SpecLang::new("css", grammar_css)));
    reg.register(Box::new(SpecLang::new("html", grammar_html)));
    reg.register(Box::new(SpecLang::new("hcl", grammar_hcl)));
    reg.register(Box::new(SpecLang::new("cmake", grammar_cmake)));
    reg.register(Box::new(SpecLang::new("xml", grammar_xml)));
    reg.register(Box::new(SpecLang::new("graphql", grammar_graphql)));
    reg.register(Box::new(SpecLang::new("nix", grammar_nix)));
    reg.register(Box::new(SpecLang::new("elm", grammar_elm)));
    reg.register(Box::new(SpecLang::new("ini", grammar_ini)));
    reg.register(Box::new(SpecLang::new("properties", grammar_properties)));
    reg.register(Box::new(SpecLang::new("jsonnet", grammar_jsonnet)));
    reg.register(Box::new(SpecLang::new("solidity", grammar_solidity)));
    reg.register(Box::new(SpecLang::new("vhdl", grammar_vhdl)));
    reg.register(Box::new(SpecLang::new("powershell", grammar_powershell)));
    reg.register(Box::new(SpecLang::new("protobuf", grammar_proto)));
    reg.register(Box::new(SpecLang::new("gleam", grammar_gleam)));
    reg.register(Box::new(SpecLang::new("odin", grammar_odin)));
    reg.register(Box::new(SpecLang::new("racket", grammar_racket)));
    reg.register(Box::new(SpecLang::new("scheme", grammar_scheme)));
    reg.register(Box::new(SpecLang::new("fortran", grammar_fortran)));
    reg.register(Box::new(SpecLang::new("cuda", grammar_cuda)));
    reg.register(Box::new(SpecLang::new("glsl", grammar_glsl)));
    reg.register(Box::new(SpecLang::new("hlsl", grammar_hlsl)));
    reg.register(Box::new(SpecLang::new("objc", grammar_objc)));
    reg.register(Box::new(SpecLang::new("jinja2", grammar_jinja2)));
    reg.register(Box::new(SpecLang::new("templ", grammar_templ)));
    reg.register(Box::new(SpecLang::new("starlark", grammar_starlark)));
    reg.register(Box::new(SpecLang::new("bicep", grammar_bicep)));
    reg.register(Box::new(SpecLang::new("gdscript", grammar_gdscript)));
    reg.register(Box::new(SpecLang::new("luau", grammar_luau)));
    reg.register(Box::new(SpecLang::new("ada", grammar_ada)));
    reg.register(Box::new(SpecLang::new(
        "systemverilog",
        grammar_systemverilog,
    )));
    reg.register(Box::new(SpecLang::new("puppet", grammar_puppet)));
    reg.register(Box::new(SpecLang::new("slang", grammar_slang)));
    reg.register(Box::new(SpecLang::new("matlab", grammar_matlab)));
    reg.register(Box::new(SpecLang::new("commonlisp", grammar_commonlisp)));
    reg.register(Box::new(SpecLang::new("verilog", grammar_verilog)));
    reg.register(Box::new(SpecLang::new("regex", grammar_regex)));
    reg.register(Box::new(SpecLang::new("svelte", grammar_svelte)));
}
