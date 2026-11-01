//! Langage C — grammaire tree-sitter (`tree-sitter-c`).
//!
//! Suit le pattern du module Rust de référence ([`super::rust`], RFC M6 §5) :
//! parser `OnceLock<Mutex<Parser>>`, marche de l'arbre, et production de
//! définitions (Function/Struct/Enum/Type/Macro/Field), d'appels et
//! d'imports (`#include`).
//!
//! Notes de fidélité à la grammaire `tree-sitter-c` 0.24 :
//! - les définitions de type en portée fichier (`struct S {…}`, `enum E {…}`,
//!   `union U {…}`) sont des enfants DIRECTS de `translation_unit` (pas
//!   enveloppées dans un nœud `declaration`) ;
//! - un `typedef` est un nœud `type_definition` ; s'il enveloppe un
//!   struct/enum/union (avec corps) on extrait la définition interne, sinon
//!   un `Type` nommé par le dernier identifiant du déclarateur ;
//! - `int a, b;` produit UN `field_declaration` à plusieurs déclarateurs
//!   frères : chaque nom devient un `Field` ;
//! - un membre de type struct/enum/union ANONYME (C11, ex. union embarquée)
//!   est « transparent » : ses champs remontent au struct parent.

use std::cell::RefCell;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::parse::{Call, Definition, FileExtraction, Import, Language};
use crate::schema::NodeKind;

thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Texte brut d'un nœud (utilitaire).
fn text<'a>(source: &'a str, node: Node) -> &'a str {
    &source[node.byte_range()]
}

/// Kinds « nom » (feuille d'une chaîne de déclarateur).
fn is_name(kind: &str) -> bool {
    matches!(kind, "identifier" | "field_identifier" | "type_identifier")
}

/// Ligne de début/end (1-based, inclusive) d'un nœud.
fn line_span(node: Node) -> (u32, u32) {
    (
        node.start_position().row as u32 + 1,
        node.end_position().row as u32 + 1,
    )
}

/// Suit la chaîne de champs `declarator` (pointeur, parenthèses, tableau,
/// fonction, initialiseur…) jusqu'au nom feuille.
///
/// Ex. `int (*handler_t)(int)` → `handler_t` ; `int x` → `x`.
///
/// NB : dans `tree-sitter-c`, `parenthesized_declarator` n'a PAS de champ
/// `declarator` — on descend sur son premier enfant nommé.
fn chain_name(source: &str, node: Node) -> Option<String> {
    match node.kind() {
        k if is_name(k) => Some(text(source, node).to_string()),
        "pointer_declarator" | "array_declarator" | "init_declarator" | "function_declarator" => {
            node.child_by_field_name("declarator")
                .and_then(|d| chain_name(source, d))
        }
        "parenthesized_declarator" => node.named_child(0).and_then(|c| chain_name(source, c)),
        _ => None,
    }
}

/// Nom du champ `name` d'un spécificateur (`struct Point` → `Point`).
fn spec_name(source: &str, spec: Node) -> Option<String> {
    spec.child_by_field_name("name")
        .map(|n| text(source, n).to_string())
        .filter(|s| !s.is_empty())
}

/// Le spécificateur est-il un struct/union/enum avec un corps (définition) ?
fn is_type_spec_with_body(node: Node) -> bool {
    matches!(
        node.kind(),
        "struct_specifier" | "union_specifier" | "enum_specifier"
    ) && node.child_by_field_name("body").is_some()
}

/// Pousse une définition en ignorant les noms vides.
fn push_def(out: &mut FileExtraction, name: String, kind: NodeKind, node: Node) {
    if name.is_empty() {
        return;
    }
    let (start_line, end_line) = line_span(node);
    out.definitions.push(Definition {
        name,
        kind,
        start_line,
        end_line,
        signature: None,
        parent_class: None,
        metrics: None,
        docstring: None,
    });
}

/// Nœud racine des définitions : `translation_unit` (les modules de langage
/// ne descendent pas dans les corps de fonction pour les définitions).
fn walk_decls(source: &str, node: Node, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "function_definition" => {
                // `int foo(int a) { … }` → Function ; signature = texte complet.
                if let Some(decl) = child.child_by_field_name("declarator")
                    && let Some(name) = chain_name(source, decl)
                {
                    let (start_line, end_line) = line_span(child);
                    out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Function,
                        start_line,
                        end_line,
                        signature: Some(text(source, child).to_string()),
                        parent_class: None,
                        metrics: crate::metrics::for_function(child, "c"),
                        docstring: crate::docs::docstring_of(child, source),
                    });
                }
            }
            "struct_specifier" | "union_specifier" => {
                // Définition de type en portée fichier.
                record_def(source, child, NodeKind::Struct, out);
            }
            "enum_specifier" => {
                if let Some(name) = spec_name(source, child)
                    && child.child_by_field_name("body").is_some()
                {
                    push_def(out, name, NodeKind::Enum, child);
                }
            }
            "type_definition" => {
                // `typedef` : struct/enum/union interne sinon `Type`.
                typedef_def(source, child, out);
            }
            "declaration" => {
                // `struct S { … } g;` (variable globale de type défini en
                // ligne) : la définition vit dans le type, pas la variable.
                if let Some(ty) = child.child_by_field_name("type")
                    && is_type_spec_with_body(ty)
                    && let Some(name) = spec_name(source, ty)
                {
                    let kind = match ty.kind() {
                        "enum_specifier" => NodeKind::Enum,
                        _ => NodeKind::Struct,
                    };
                    if kind == NodeKind::Enum {
                        push_def(out, name, kind, ty);
                    } else {
                        record_def(source, ty, kind, out);
                    }
                }
            }
            // #define : NE PAS créer de nœud Macro — le C de référence n'en
            // indexe aucune (0 Macro sur drivers/net) et leur volume sur les
            // headers kernel (milliers de #define conditionnels) triple le
            // graphe pour un gain marginal pour les agents.
            "preproc_def" | "preproc_function_def" => {}
            "preproc_include" => {
                if let Some(path) = child
                    .child_by_field_name("path")
                    .and_then(|p| include_path(source, p))
                {
                    out.imports.push(Import {
                        module: path,
                        local_name: None,
                    });
                }
            }
            // Blocs conditionnels du préprocesseur (#if/#ifdef/#else/#elif) :
            // les déclarations à l'intérieur sont du code réel (le kernel
            // cache des fonctions entières sous #if IS_ENABLED(...)).
            "preproc_if" | "preproc_ifdef" | "preproc_else" | "preproc_elif" => {
                walk_decls(source, child, out);
            }
            _ => {}
        }
    }
}

/// Chemin d'un `#include` : `"util.h"` / `<stdio.h>` → `util.h` / `stdio.h`
/// (délimiteurs retirés ; un chemin macro reste tel quel).
fn include_path(source: &str, node: Node) -> Option<String> {
    let raw = text(source, node).trim();
    if raw.is_empty() {
        return None;
    }
    let stripped = raw
        .strip_prefix('<')
        .and_then(|r| r.strip_suffix('>'))
        .or_else(|| raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')))
        .unwrap_or(raw);
    Some(stripped.to_string())
}

/// Définition d'un struct/union + marche de ses membres.
fn record_def(source: &str, spec: Node, kind: NodeKind, out: &mut FileExtraction) {
    let Some(body) = spec.child_by_field_name("body") else {
        return; // déclaration anticipée `struct S;` — pas une définition
    };
    let Some(name) = spec_name(source, spec) else {
        return; // spécificateur anonyme seul (sans déclarateur)
    };
    let (start_line, end_line) = line_span(spec);
    out.definitions.push(Definition {
        name: name.clone(),
        kind,
        start_line,
        end_line,
        signature: None,
        parent_class: None,
        metrics: None,
        docstring: None,
    });
    walk_members(source, &name, body, out);
}

/// Marche du corps d'un struct/union : chaque `field_declaration` devient un
/// `Field` rattaché au type parent (parent_class).
fn walk_members(source: &str, parent: &str, body: Node, out: &mut FileExtraction) {
    let mut cur = body.walk();
    for child in body.children(&mut cur) {
        if child.kind() != "field_declaration" {
            continue;
        }
        let ty = child.child_by_field_name("type");
        if let Some(t) = ty
            && is_type_spec_with_body(t)
        {
            // Membre de type struct/enum/union défini en ligne.
            if let Some(nested) = spec_name(source, t) {
                // Type imbriqué nommé (GNU/C++-like) → sa propre définition.
                let kind = match t.kind() {
                    "enum_specifier" => NodeKind::Enum,
                    _ => NodeKind::Struct,
                };
                if let Some(body_list) = t.child_by_field_name("body") {
                    if kind == NodeKind::Enum {
                        push_def(out, nested, kind, t);
                    } else {
                        let (sl, el) = line_span(t);
                        out.definitions.push(Definition {
                            name: nested.clone(),
                            kind,
                            start_line: sl,
                            end_line: el,
                            signature: None,
                            parent_class: None,
                            metrics: None,
                            docstring: None,
                        });
                        walk_members(source, &nested, body_list, out);
                    }
                }
            } else if let Some(body_list) = t.child_by_field_name("body") {
                // Struct/union ANONYME (C11) : transparent — ses champs
                // remontent au type parent (ex. union embarquée).
                walk_members(source, parent, body_list, out);
            }
            continue;
        }
        // Membres « données » : chaque déclarateur (direct ou enveloppé)
        // devient un Field de `parent`.
        collect_field_names(source, child, parent, out);
    }
}

/// Noms de champs d'un `field_declaration` : déclarateurs simples frères
/// (`int a, b;`) ou chaînes enveloppées (`int (*cb)(int);` → `cb`).
///
/// NB : les noms de membres C sont des `field_identifier` ; le nœud `type`
/// (type_identifier inclus, ex. type typedefé) n'est jamais pris pour un nom.
fn collect_field_names(source: &str, decl: Node, parent: &str, out: &mut FileExtraction) {
    let mut cur = decl.walk();
    for child in decl.children(&mut cur) {
        let name = match child.kind() {
            "field_identifier" | "identifier" => Some(text(source, child).to_string()),
            "pointer_declarator"
            | "parenthesized_declarator"
            | "array_declarator"
            | "init_declarator"
            | "function_declarator" => chain_name(source, child),
            _ => None,
        };
        if let Some(name) = name {
            let (start_line, end_line) = line_span(child);
            out.definitions.push(Definition {
                name,
                kind: NodeKind::Field,
                start_line,
                end_line,
                signature: None,
                parent_class: Some(parent.to_string()),
                metrics: None,
                docstring: None,
            });
        }
    }
}

/// `typedef` : struct/enum/union interne → la définition interne ; sinon un
/// `Type` nommé par le dernier identifiant du déclarateur.
fn typedef_def(source: &str, node: Node, out: &mut FileExtraction) {
    let Some(ty) = node.child_by_field_name("type") else {
        return;
    };
    if is_type_spec_with_body(ty) {
        let name = match spec_name(source, ty) {
            Some(n) => n,
            // `typedef struct { … } Anon;` → on prend l'alias.
            None => node
                .child_by_field_name("declarator")
                .and_then(|d| chain_name(source, d))
                .unwrap_or_default(),
        };
        let kind = match ty.kind() {
            "enum_specifier" => NodeKind::Enum,
            _ => NodeKind::Struct,
        };
        if let Some(body) = ty.child_by_field_name("body") {
            if kind == NodeKind::Enum {
                push_def(out, name, kind, ty);
            } else {
                let (start_line, end_line) = line_span(ty);
                out.definitions.push(Definition {
                    name: name.clone(),
                    kind,
                    start_line,
                    end_line,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
                walk_members(source, &name, body, out);
            }
        }
        return;
    }
    // `typedef unsigned long ulong_t;` / `typedef int (*handler_t)(int);`
    if let Some(name) = node
        .child_by_field_name("declarator")
        .and_then(|d| chain_name(source, d))
    {
        push_def(out, name, NodeKind::Type, node);
    }
}

/// Collecte les appels : `foo(...)` → Call{name=foo}, `obj.m(...)` →
/// Call{name=m, qualifier=obj}. Descend dans tout le sous-arbre (corps de
/// fonctions, initialiseurs…).
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "call_expression"
            && let Some(func) = child.child_by_field_name("function")
        {
            let (name, qualifier) = match func.kind() {
                "identifier" => (text(source, func).to_string(), None),
                "field_expression" => {
                    // `obj.m()` / `ptr->m()` → m qualifié par l'argument.
                    let m = func
                        .child_by_field_name("field")
                        .map(|n| text(source, n).to_string())
                        .unwrap_or_default();
                    let q = func
                        .child_by_field_name("argument")
                        .map(|n| text(source, n).to_string());
                    (m, q)
                }
                _ => (String::new(), None),
            };
            if !name.is_empty() {
                out.push(Call {
                    name,
                    qualifier,
                    line: child.start_position().row as u32 + 1,
                });
            }
        }
        if child.child_count() > 0 {
            collect_calls(source, child, out);
        }
    }
}

#[derive(Clone, Default)]
pub struct CLang;

impl CLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for CLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["c", "h"]
    }

    fn name(&self) -> &'static str {
        "c"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_c::LANGUAGE.into())
                    .expect("grammaire valide");
                *borrow = Some(p);
            }
            borrow.as_mut().unwrap().parse(source, None)
        });
        let Some(tree) = tree else { return out };
        let root = tree.root_node();
        walk_decls(source, root, &mut out);
        collect_calls(source, root, &mut out.calls);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> FileExtraction {
        CLang::new().extract(src, Path::new("test.c"))
    }

    #[test]
    fn fonctions_simples_et_appels() {
        let src = r#"
int helper(int a) { return a + 1; }
int main(void) { return helper(1); }
"#;
        let ex = extract_src(src);
        let names: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label()))
            .collect();
        assert!(names.contains(&("helper", "Function")));
        assert!(names.contains(&("main", "Function")));
        // helper appelé dans main.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "helper" && c.qualifier.is_none())
        );
        // La signature est le texte du nœud.
        let helper = ex.definitions.iter().find(|d| d.name == "helper").unwrap();
        assert!(helper.signature.as_deref().unwrap().contains("int helper"));
    }

    #[test]
    fn struct_union_enum_et_champs() {
        let src = r#"
struct Point { int x; int y; };
union Val { int i; float f; };
enum Color { RED, GREEN, BLUE };
"#;
        let ex = extract_src(src);
        let kinds: Vec<_> = ex.definitions.iter().map(|d| d.kind).collect();
        assert!(kinds.contains(&NodeKind::Struct));
        assert!(kinds.contains(&NodeKind::Enum));
        // union → Struct.
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Val" && d.kind == NodeKind::Struct)
        );
        // Champs de struct rattachés au parent.
        let fields: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Field)
            .collect();
        assert_eq!(fields.len(), 4); // x, y, i, f
        assert!(
            fields
                .iter()
                .filter(|f| f.name == "x" || f.name == "y")
                .all(|f| f.parent_class.as_deref() == Some("Point"))
        );
        assert!(
            fields
                .iter()
                .filter(|f| f.name == "i" || f.name == "f")
                .all(|f| f.parent_class.as_deref() == Some("Val"))
        );
    }

    #[test]
    fn macros_et_typedefs() {
        let src = r#"
#define MAX 10
#define SQ(x) ((x) * (x))
typedef struct _list { int n; } List;
typedef unsigned long ulong_t;
typedef int (*handler_t)(int);
"#;
        let ex = extract_src(src);
        let names: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label()))
            .collect();
        // #define : aucune Macro créée (parité C — le C n'indexe pas les
        // constantes préprocesseur; leur volume sur les headers kernel
        // triple le graphe).
        assert!(!names.contains(&("MAX", "Macro")));
        assert!(!names.contains(&("SQ", "Macro")));
        // typedef d'un struct → la définition interne.
        assert!(names.contains(&("_list", "Struct")));
        // typedef simple → Type (dernier identifiant du déclarateur).
        assert!(names.contains(&("ulong_t", "Type")));
        assert!(names.contains(&("handler_t", "Type")));
    }

    #[test]
    fn typedef_anonyme() {
        let src = "typedef struct { int x; double y; } Anon;\n";
        let ex = extract_src(src);
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Anon" && d.kind == NodeKind::Struct)
        );
        assert!(ex.definitions.iter().any(|d| d.name == "x"
            && d.kind == NodeKind::Field
            && d.parent_class.as_deref() == Some("Anon")));
        assert!(ex.definitions.iter().any(|d| d.name == "y"
            && d.kind == NodeKind::Field
            && d.parent_class.as_deref() == Some("Anon")));
    }

    #[test]
    fn includes() {
        let src = r#"
#include <stdio.h>
#include "util/math.h"
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(mods.contains(&"stdio.h"));
        assert!(mods.contains(&"util/math.h"));
        assert!(ex.imports.iter().all(|i| i.local_name.is_none()));
    }

    #[test]
    fn appel_qualifie_sur_champ() {
        // `o.run(1)` : appel via pointeur de fonction membre d'une struct.
        let src = r#"
struct ops { int (*run)(int); };
int go(struct ops o) { return o.run(1); }
"#;
        let ex = extract_src(src);
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "run" && c.qualifier.as_deref() == Some("o"))
        );
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "run" && d.kind == NodeKind::Field)
        );
    }
}
