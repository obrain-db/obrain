//! Langage TypeScript — grammaire tree-sitter-typescript (.ts / .tsx).
//!
//! Module « une grammaire = un module » (RFC M6 §5), calqué sur le module
//! Rust de référence : parser partagé (mutex), marche de l'arbre, production
//! de définitions (Function/Method/Class/Interface/Enum/Type/Field), d'appels
//! et d'imports. TypeScript = JavaScript + annotations de type ; la crate
//! tree-sitter-typescript expose DEUX langages : `LANGUAGE_TYPESCRIPT` (.ts)
//! et `LANGUAGE_TSX` (.tsx, avec JSX) — l'extension du fichier choisit la
//! grammaire au moment du parse.

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
/// `property` (champs de classe de la grammaire JS). Vide si anonyme.
fn declared_name(source: &str, node: Node) -> String {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("property"))
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
        crate::metrics::for_function(node, "typescript")
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

#[derive(Clone, Default)]
pub struct TypeScriptLang;

impl TypeScriptLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for TypeScriptLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx"]
    }

    fn name(&self) -> &'static str {
        "typescript"
    }

    fn extract(&self, source: &str, path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        // Deux langages dans la crate : LANGUAGE_TYPESCRIPT (.ts) et
        // LANGUAGE_TSX (.tsx) — la grammaire est réinstallée selon l'extension.
        let grammar = if path.extension().and_then(|e| e.to_str()) == Some("tsx") {
            tree_sitter_typescript::LANGUAGE_TSX
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT
        };
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&grammar.into())
                    .expect("grammaire typescript/tsx valide");
                *borrow = Some(p);
            } else {
                borrow
                    .as_mut()
                    .unwrap()
                    .set_language(&grammar.into())
                    .expect("grammaire typescript/tsx valide");
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
            // ── Classes (concrètes et abstraites) + corps ────────────────
            "class_declaration" | "abstract_class_declaration" => {
                let name = declared_name(source, child);
                if !name.is_empty() {
                    push_definition(out, child, name.clone(), NodeKind::Class, None, None);
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_items(source, body, Some(&name), out);
                    }
                }
            }
            // ── Types structurels TS ──────────────────────────────────────
            "interface_declaration" => {
                push_definition(
                    out,
                    child,
                    declared_name(source, child),
                    NodeKind::Interface,
                    None,
                    None,
                );
            }
            "enum_declaration" => {
                push_definition(
                    out,
                    child,
                    declared_name(source, child),
                    NodeKind::Enum,
                    None,
                    None,
                );
            }
            "type_alias_declaration" => {
                push_definition(
                    out,
                    child,
                    declared_name(source, child),
                    NodeKind::Type,
                    None,
                    None,
                );
            }
            // ── Champs de classe (optionnels) ─────────────────────────────
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
            _ => {}
        }
    }
}

/// Extrait les imports ESM d'un `import_statement`.
///
/// Formes tree-sitter-typescript :
/// - `import "m";`                   → import seul (side-effect)
/// - `import d from "m";`            → import_clause [identifier]
/// - `import * as ns from "m";`      → namespace_import
/// - `import {a, b as c} from "m";`  → named_imports{import_specifier}
/// - `import x = require("m");`      → import_require_clause
fn collect_imports(source: &str, node: Node, out: &mut Vec<Import>) {
    // Source commune de l'import_statement (absente pour la forme
    // `import x = require("m")`, où elle vit sur la clause).
    let module = node
        .child_by_field_name("source")
        .map(|n| string_content(source, n))
        .unwrap_or_default();
    let mut clause_seen = false;
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "import_clause" => {
                clause_seen = true;
                if !module.is_empty() {
                    import_clause_imports(source, child, module, out);
                }
            }
            "import_require_clause" => {
                clause_seen = true;
                // `import fs = require("fs")` — alias local sur le module.
                let m = child
                    .child_by_field_name("source")
                    .map(|n| string_content(source, n))
                    .unwrap_or_default();
                if let Some(local) = first_identifier(source, child) {
                    out.push(Import {
                        module: m.to_string(),
                        local_name: Some(local.to_string()),
                    });
                }
            }
            _ => {}
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
        TypeScriptLang::new().extract(src, Path::new("test.ts"))
    }

    #[test]
    fn fonctions_et_generateurs() {
        let src = r#"
function helper(a: number): number { return a + 1; }
function* sequence() { yield 1; }
const result = helper(2);
"#;
        let ex = extract_src(src);
        let names: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label()))
            .collect();
        assert!(names.contains(&("helper", "Function")));
        assert!(names.contains(&("sequence", "Function")));
        // helper appelé dans le corps du fichier.
        assert!(ex.calls.iter().any(|c| c.name == "helper"));
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
    fn classe_methodes_et_champs() {
        let src = r#"
export abstract class Animal {
    name: string;
    count = 0;
    constructor(name: string) { this.name = name; }
    speak(): void { console.log(this.name); }
    move(): void {}
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
        let animal = ex
            .definitions
            .iter()
            .find(|d| d.kind == NodeKind::Class)
            .unwrap();
        assert_eq!(animal.name, "Animal");
        // Méthodes rattachées à la classe.
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(methods.len(), 3); // constructor, speak, move
        assert!(
            methods
                .iter()
                .all(|m| m.parent_class.as_deref() == Some("Animal"))
        );
        assert!(methods.iter().any(|m| m.name == "speak"));
        // Champs (optionnels).
        let fields: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Field)
            .collect();
        assert!(
            fields
                .iter()
                .any(|f| f.name == "name" && f.parent_class.as_deref() == Some("Animal"))
        );
        assert!(fields.iter().any(|f| f.name == "count"));
        // Appel membre console.log.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "log" && c.qualifier.as_deref() == Some("console"))
        );
    }

    #[test]
    fn interface_enum_et_type_alias() {
        let src = r#"
interface Shape { area(): number; }
enum Color { Red, Green }
type Point = { x: number; y: number };
"#;
        let ex = extract_src(src);
        let kinds: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind))
            .collect();
        assert!(kinds.contains(&("Shape", NodeKind::Interface)));
        assert!(kinds.contains(&("Color", NodeKind::Enum)));
        assert!(kinds.contains(&("Point", NodeKind::Type)));
    }

    #[test]
    fn imports_esm_et_reexports() {
        let src = r#"
import React, { useState, useEffect as eff } from "react";
import * as lodash from "lodash";
import { helper } from "./utils";
import "./side-effect.css";
export { helper as h } from "./utils";
export * from "./types";
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex
            .imports
            .iter()
            .map(|i| (i.module.as_str(), i.local_name.as_deref()))
            .collect();
        // Import par défaut + specifiers nommés.
        assert!(mods.contains(&("react", Some("React"))));
        assert!(mods.contains(&("react.useState", None)));
        assert!(mods.contains(&("react.useEffect", Some("eff"))));
        assert!(mods.contains(&("lodash", Some("lodash"))));
        assert!(mods.contains(&("./utils.helper", None)));
        // Side-effect + re-exports.
        assert!(mods.contains(&("./side-effect.css", None)));
        assert!(mods.contains(&("./utils.helper", None)));
        assert!(mods.contains(&("./types", None)));
    }

    #[test]
    fn appels_et_constructions() {
        let src = r#"
class Server {}
const srv = new Server();
srv.start();
config.db.connect();
helper(srv);
"#;
        let ex = extract_src(src);
        // `new Server()` → cible de construction.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "Server" && c.qualifier.is_none())
        );
        // Appel membre simple.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "start" && c.qualifier.as_deref() == Some("srv"))
        );
        // Chaîne de membres : qualifiant = objet complet.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "connect" && c.qualifier.as_deref() == Some("config.db"))
        );
        assert!(ex.calls.iter().any(|c| c.name == "helper"));
    }

    #[test]
    fn parse_tsx_avec_grammaire_tsx() {
        let src = r#"
export function App(): JSX.Element {
    return <div onClick={handler}>hello</div>;
}
"#;
        let ex = TypeScriptLang::new().extract(src, Path::new("app.tsx"));
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "App" && d.kind == NodeKind::Function)
        );
    }
}
