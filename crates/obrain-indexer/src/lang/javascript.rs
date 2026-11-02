//! Langage JavaScript — grammaire tree-sitter-javascript (.js/.jsx/.mjs/.cjs).
//!
//! Module « une grammaire = un module » (RFC M6 §5), calqué sur le module
//! Rust de référence et jumeau du module TypeScript (mêmes formes ESM) — sans
//! les types, avec en plus : les fonctions fléchées/expressions assignées à
//! une variable (`const f = (…) => …`) et les imports CommonJS
//! (`require("m")`).

use std::cell::RefCell;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::parse::{Call, Definition, FileExtraction, Import, Language};
use crate::schema::NodeKind;

thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Extraits texte d'un nœud (utilitaire).
fn text<'a>(source: &'a str, node: Node) -> &'a str {
    &source[node.byte_range()]
}

/// Nom déclaré d'un nœud : champ `name` (déclarations, méthodes), repli sur
/// `property` (champs de classe). Vide si anonyme ou motif de déstructuration.
fn declared_name(source: &str, node: Node) -> String {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("property"))
        .filter(|n| !matches!(n.kind(), "array_pattern" | "object_pattern"))
        .map(|n| text(source, n).to_string())
        .unwrap_or_default()
}

/// Ajoute une définition au résultat (ignorée si anonyme — `name` vide).
fn push_definition(
    out: &mut FileExtraction,
    node: Node,
    name: String,
    kind: NodeKind,
    signature: Option<String>,
    parent_class: Option<&str>,
) {
    if name.is_empty() {
        return;
    }
    let metrics = if kind == NodeKind::Function || kind == NodeKind::Method {
        crate::metrics::for_function(node, "javascript")
    } else {
        None
    };
    out.definitions.push(Definition {
        name,
        kind,
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        signature,
        parent_class: parent_class.map(str::to_string),
        metrics,
        docstring: None,
    });
}

/// Contenu d'un littéral `string` tree-sitter (sans les guillemets).
fn string_content<'a>(source: &'a str, node: Node) -> &'a str {
    let raw = text(source, node);
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && matches!(bytes[0], b'"' | b'\'' | b'`')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    }
}

/// Premier enfant `identifier` d'un nœud (ex. alias d'un `namespace_import`).
fn first_identifier<'a>(source: &'a str, node: Node) -> Option<&'a str> {
    let mut cur = node.walk();
    node.children(&mut cur)
        .find(|c| c.kind() == "identifier")
        .map(|c| text(source, c))
}

/// Si `node` est un appel `require("m")` → le module demandé.
fn require_module<'a>(source: &'a str, node: Node) -> Option<&'a str> {
    if node.kind() != "call_expression" {
        return None;
    }
    let func = node.child_by_field_name("function")?;
    if func.kind() != "identifier" || text(source, func) != "require" {
        return None;
    }
    let arg = node.child_by_field_name("arguments")?.named_child(0)?;
    if arg.kind() != "string" {
        return None;
    }
    let m = string_content(source, arg);
    if m.is_empty() { None } else { Some(m) }
}

#[derive(Clone, Default)]
pub struct JavaScriptLang;

impl JavaScriptLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for JavaScriptLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["js", "jsx", "mjs", "cjs"]
    }

    fn name(&self) -> &'static str {
        "javascript"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_javascript::LANGUAGE.into())
                    .expect("grammaire valide");
                *borrow = Some(p);
            }
            borrow.as_mut().unwrap().parse(source, None)
        });
        let Some(tree) = tree else { return out };
        let root = tree.root_node();
        walk_items(source, root, None, &mut out);
        collect_calls(source, root, &mut out.calls);
        out
    }
}

/// Marche un bloc d'instructions : définitions + imports. `in_class` = classe
/// courante (corps de classe revisitée par la déclaration parente). Les corps
/// de fonctions ne sont pas revisités pour les définitions (pattern du module
/// Rust de référence) ; les appels, eux, sont collectés sur tout l'arbre.
fn walk_items(source: &str, node: Node, in_class: Option<&str>, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            // ── Fonctions & méthodes ─────────────────────────────────────
            "function_declaration" | "generator_function_declaration" | "method_definition" => {
                // Dans un corps de classe → Method ; ailleurs (top-level,
                // littéral d'objet…) → Function.
                let (kind, parent) = match in_class {
                    Some(c) => (NodeKind::Method, Some(c)),
                    None => (NodeKind::Function, None),
                };
                let sig = text(source, child).to_string();
                push_definition(
                    out,
                    child,
                    declared_name(source, child),
                    kind,
                    Some(sig),
                    parent,
                );
            }
            // ── Classes + corps ───────────────────────────────────────────
            "class_declaration" => {
                let name = declared_name(source, child);
                if !name.is_empty() {
                    push_definition(out, child, name.clone(), NodeKind::Class, None, None);
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_items(source, body, Some(&name), out);
                    }
                }
            }
            // ── Champs de classe ──────────────────────────────────────────
            "field_definition" | "public_field_definition" => {
                if let Some(c) = in_class {
                    push_definition(
                        out,
                        child,
                        declared_name(source, child),
                        NodeKind::Field,
                        None,
                        Some(c),
                    );
                }
            }
            // ── Imports / exports ─────────────────────────────────────────
            "import_statement" => {
                collect_imports(source, child, &mut out.imports);
            }
            "export_statement" => {
                // Re-export (`export … from "mod"`) → import.
                if child.child_by_field_name("source").is_some() {
                    collect_reexports(source, child, &mut out.imports);
                }
                // Déclaration exportée (`export function/class …`) : la
                // déclaration est un enfant direct — on revisite.
                walk_items(source, child, in_class, out);
            }
            // ── Fonction fléchée / expression assignée à une variable ────
            "lexical_declaration" | "variable_declaration" => {
                // `const f = (…) => …` / `var f = function () {}` → Function.
                let mut c2 = child.walk();
                for decl in child.children(&mut c2) {
                    if decl.kind() != "variable_declarator" {
                        continue;
                    }
                    let name = declared_name(source, decl);
                    let Some(value) = decl.child_by_field_name("value") else {
                        continue;
                    };
                    match value.kind() {
                        "arrow_function" | "function_expression" => {
                            let sig = text(source, value).to_string();
                            push_definition(
                                out,
                                decl,
                                name,
                                NodeKind::Function,
                                Some(sig),
                                in_class,
                            );
                        }
                        "call_expression" => {
                            // `const fs = require("fs")` — import avec alias.
                            if !name.is_empty()
                                && let Some(m) = require_module(source, value)
                            {
                                out.imports.push(Import {
                                    module: m.to_string(),
                                    local_name: Some(name),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            // ── `require("m");` nu (side-effect) ─────────────────────────
            "expression_statement" => {
                let mut c2 = child.walk();
                for stmt_child in child.children(&mut c2) {
                    if let Some(m) = require_module(source, stmt_child) {
                        out.imports.push(Import {
                            module: m.to_string(),
                            local_name: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Extrait les imports ESM d'un `import_statement`.
///
/// Formes tree-sitter-javascript :
/// - `import "m";`                   → import seul (side-effect)
/// - `import d from "m";`            → import_clause [identifier]
/// - `import * as ns from "m";`      → namespace_import
/// - `import {a, b as c} from "m";`  → named_imports{import_specifier}
fn collect_imports(source: &str, node: Node, out: &mut Vec<Import>) {
    let module = node
        .child_by_field_name("source")
        .map(|n| string_content(source, n))
        .unwrap_or_default();
    let mut clause_seen = false;
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "import_clause" {
            clause_seen = true;
            if !module.is_empty() {
                import_clause_imports(source, child, module, out);
            }
        }
    }
    // `import "m";` — pas de clause : import du module seul (side-effect).
    if !module.is_empty() && !clause_seen {
        out.push(Import {
            module: module.to_string(),
            local_name: None,
        });
    }
}

/// Une `import_clause` : import par défaut, namespace ou specifiers nommés.
fn import_clause_imports(source: &str, node: Node, module: &str, out: &mut Vec<Import>) {
    let mut cur = node.walk();
    for item in node.children(&mut cur) {
        match item.kind() {
            "identifier" => {
                // Import par défaut : liaison locale sur le module.
                let local = text(source, item);
                out.push(Import {
                    module: module.to_string(),
                    local_name: Some(local.to_string()),
                });
            }
            "namespace_import" => {
                // `import * as ns from "m"` — l'identifiant enfant est l'alias.
                if let Some(local) = first_identifier(source, item) {
                    out.push(Import {
                        module: module.to_string(),
                        local_name: Some(local.to_string()),
                    });
                }
            }
            "named_imports" => {
                let mut c2 = item.walk();
                for spec in item.children(&mut c2) {
                    if spec.kind() != "import_specifier" {
                        continue;
                    }
                    let name = specifier_name(source, spec);
                    let alias = spec
                        .child_by_field_name("alias")
                        .map(|n| text(source, n).to_string());
                    if !name.is_empty() {
                        out.push(Import {
                            module: format!("{module}.{name}"),
                            local_name: alias,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Re-export (`export … from "mod"`) → traité comme un import du module cible.
///
/// Formes : `export {a, b as c} from "m"`, `export * from "m"`,
/// `export * as ns from "m"`.
fn collect_reexports(source: &str, node: Node, out: &mut Vec<Import>) {
    let module = node
        .child_by_field_name("source")
        .map(|n| string_content(source, n))
        .unwrap_or_default();
    if module.is_empty() {
        return;
    }
    let mut cur = node.walk();
    let mut covered = false;
    for child in node.children(&mut cur) {
        match child.kind() {
            "export_clause" => {
                covered = true;
                let mut c2 = child.walk();
                for spec in child.children(&mut c2) {
                    if spec.kind() != "export_specifier" {
                        continue;
                    }
                    let name = specifier_name(source, spec);
                    if !name.is_empty() {
                        out.push(Import {
                            module: format!("{module}.{name}"),
                            local_name: None,
                        });
                    }
                }
            }
            "namespace_export" => {
                // `export * as ns from "m"`.
                covered = true;
                if let Some(local) = first_identifier(source, child) {
                    out.push(Import {
                        module: module.to_string(),
                        local_name: Some(local.to_string()),
                    });
                }
            }
            _ => {}
        }
    }
    // `export * from "m"` — pas de clause.
    if !covered {
        out.push(Import {
            module: module.to_string(),
            local_name: None,
        });
    }
}

/// Nom importé d'un `import_specifier` / `export_specifier` (champ `name`).
fn specifier_name(source: &str, node: Node) -> String {
    let Some(n) = node.child_by_field_name("name") else {
        return String::new();
    };
    if n.kind() == "string" {
        string_content(source, n).to_string()
    } else {
        text(source, n).to_string()
    }
}

/// Collecte récursivement les appels (`call_expression`) et constructions
/// (`new_expression`) — chaque nœud n'est visité qu'une fois.
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "call_expression" => {
                if let Some(func) = child.child_by_field_name("function") {
                    let (name, qualifier) = call_target(source, func);
                    if !name.is_empty() {
                        out.push(Call {
                            name,
                            qualifier,
                            line: child.start_position().row as u32 + 1,
                        });
                    }
                }
            }
            "new_expression" => {
                // `new Foo()` — le type construit comme cible d'appel.
                if let Some(ctor) = child.child_by_field_name("constructor") {
                    let (name, qualifier) = call_target(source, ctor);
                    if !name.is_empty() {
                        out.push(Call {
                            name,
                            qualifier,
                            line: child.start_position().row as u32 + 1,
                        });
                    }
                }
            }
            _ => {}
        }
        if child.child_count() > 0 {
            collect_calls(source, child, out);
        }
    }
}

/// Nom + qualifiant d'une cible d'appel : `identifier` → (nom, None) ;
/// `member_expression` → (propriété, objet) — `obj.méthode()`.
fn call_target(source: &str, node: Node) -> (String, Option<String>) {
    match node.kind() {
        "identifier" | "type_identifier" => (text(source, node).to_string(), None),
        "member_expression" => {
            let name = node
                .child_by_field_name("property")
                .map(|n| text(source, n).to_string())
                .unwrap_or_default();
            let qualifier = node
                .child_by_field_name("object")
                .map(|n| text(source, n).to_string());
            (name, qualifier)
        }
        _ => (String::new(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> FileExtraction {
        JavaScriptLang::new().extract(src, Path::new("test.js"))
    }

    #[test]
    fn fonctions_et_flechees() {
        let src = r#"
function helper(a) { return a + 1; }
function* sequence() { yield 1; }
const add = (a, b) => a + b;
var legacy = function (x) { return x; };
"#;
        let ex = extract_src(src);
        let names: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label()))
            .collect();
        assert!(names.contains(&("helper", "Function")));
        assert!(names.contains(&("sequence", "Function")));
        // Fonction fléchée / expression assignée à une variable → Function.
        assert!(names.contains(&("add", "Function")));
        assert!(names.contains(&("legacy", "Function")));
        let helper = ex.definitions.iter().find(|d| d.name == "helper").unwrap();
        assert!(
            helper
                .signature
                .as_deref()
                .unwrap()
                .starts_with("function helper")
        );
    }

    #[test]
    fn classe_et_methodes() {
        let src = r#"
class Greeter {
    greeting = "hello";
    constructor(msg) { this.msg = msg; }
    greet(name) { return "hi " + name; }
}
"#;
        let ex = extract_src(src);
        assert_eq!(
            ex.definitions
                .iter()
                .filter(|d| d.kind == NodeKind::Class)
                .count(),
            1
        );
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.kind == NodeKind::Class && d.name == "Greeter")
        );
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(methods.len(), 2); // constructor + greet
        assert!(
            methods
                .iter()
                .all(|m| m.parent_class.as_deref() == Some("Greeter"))
        );
        assert!(methods.iter().any(|m| m.name == "greet"));
        // Champ de classe.
        assert!(ex.definitions.iter().any(|d| d.kind == NodeKind::Field
            && d.name == "greeting"
            && d.parent_class.as_deref() == Some("Greeter")));
    }

    #[test]
    fn imports_esm() {
        let src = r#"
import assert from "assert";
import { readFile, writeFile as wf } from "fs";
import * as path from "path";
import "reflect-metadata";
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex
            .imports
            .iter()
            .map(|i| (i.module.as_str(), i.local_name.as_deref()))
            .collect();
        assert!(mods.contains(&("assert", Some("assert"))));
        assert!(mods.contains(&("fs.readFile", None)));
        assert!(mods.contains(&("fs.writeFile", Some("wf"))));
        assert!(mods.contains(&("path", Some("path"))));
        assert!(mods.contains(&("reflect-metadata", None)));
    }

    #[test]
    fn imports_commonjs() {
        let src = r#"
const fs = require("fs");
require("dotenv");
const { readFile } = require("node:fs");
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex
            .imports
            .iter()
            .map(|i| (i.module.as_str(), i.local_name.as_deref()))
            .collect();
        assert!(mods.contains(&("fs", Some("fs"))));
        assert!(mods.contains(&("dotenv", None)));
        // Déstructuration : pas de nom local simple → non capturé.
        assert!(!mods.contains(&("node:fs", Some("readFile"))));
    }

    #[test]
    fn appels_et_constructions() {
        let src = r#"
function main() {
    helper();
    obj.run();
    new Widget();
    console.log("x");
}
"#;
        let ex = extract_src(src);
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "helper" && c.qualifier.is_none())
        );
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "run" && c.qualifier.as_deref() == Some("obj"))
        );
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "Widget" && c.qualifier.is_none())
        );
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "log" && c.qualifier.as_deref() == Some("console"))
        );
    }

    #[test]
    fn exports_et_reexports() {
        let src = r#"
export function api() { return 1; }
export default class Handler {}
export { api as ap } from "./inner";
export * from "./extra";
"#;
        let ex = extract_src(src);
        // Déclarations exportées : la classe et la fonction sont définies.
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "api" && d.kind == NodeKind::Function)
        );
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Handler" && d.kind == NodeKind::Class)
        );
        // Re-exports vus comme imports.
        let mods: Vec<_> = ex.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(mods.contains(&"./inner.api"));
        assert!(mods.contains(&"./extra"));
    }
}
