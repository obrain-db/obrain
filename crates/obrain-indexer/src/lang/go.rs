//! Langage Go — grammaire tree-sitter (RFC M6 §5).
//!
//! Calqué sur le module Rust de référence (`rust.rs`) : parser statique,
//! marche des déclarations de niveau fichier (`package_clause`,
//! `import_declaration`, `function_declaration`, `method_declaration`,
//! `type_declaration`, `const_declaration`, `var_declaration`) puis
//! collecte récursive des appels (`call_expression` → `identifier` simple ou
//! `selector_expression` qualifié, ex `fmt.Println`).

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

/// Construit une `Definition` depuis un nœud (lignes 1-based).
fn definition(
    name: String,
    kind: NodeKind,
    node: Node,
    parent_class: Option<String>,
) -> Definition {
    Definition {
        name,
        kind,
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        signature: None,
        parent_class,
        metrics: None,
        docstring: None,
    }
}

/// Nom de champ d'un nœud, vide si absent.
fn field_text<'a>(source: &'a str, node: Node, field: &str) -> Option<&'a str> {
    node.child_by_field_name(field).map(|n| text(source, n))
}

#[derive(Clone, Default)]
pub struct GoLang;

impl GoLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for GoLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn name(&self) -> &'static str {
        "go"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_go::LANGUAGE.into())
                    .expect("grammaire valide");
                *borrow = Some(p);
            }
            borrow.as_mut().unwrap().parse(source, None)
        });
        let Some(tree) = tree else { return out };
        walk_items(source, tree.root_node(), &mut out);
        collect_calls(source, tree.root_node(), &mut out.calls);
        out
    }
}

/// Marche les déclarations de niveau fichier (Go n'a pas de déclarations
/// imbriquées — seuls les `func`/`type`/`const`/`var` du `source_file`).
fn walk_items(source: &str, node: Node, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            // `func foo(args) result { … }`
            "function_declaration" => {
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Function,
                        start_line: child.start_position().row as u32 + 1,
                        end_line: child.end_position().row as u32 + 1,
                        signature: Some(text(source, child).to_string()),
                        parent_class: None,
                        metrics: crate::metrics::for_function(child, "go"),
                        docstring: crate::docs::docstring_of(child, source),
                    });
                }
            }
            // `func (r *Receiver) m(args) { … }` — Method rattachée au
            // type du receiver (déréférencé : `*T` → T).
            "method_declaration" => {
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                let parent = receiver_type(source, child).map(|t| t.to_string());
                if !name.is_empty() {
                    out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Method,
                        start_line: child.start_position().row as u32 + 1,
                        end_line: child.end_position().row as u32 + 1,
                        signature: Some(text(source, child).to_string()),
                        parent_class: parent,
                        metrics: crate::metrics::for_function(child, "go"),
                        docstring: crate::docs::docstring_of(child, source),
                    });
                }
            }
            // `type X struct{…}` / `type X interface{…}` / `type X T` —
            // un (ou plusieurs, forme parenthésée) type_spec par nœud.
            "type_declaration" => {
                let mut c2 = child.walk();
                for spec in child.children(&mut c2) {
                    if spec.kind() == "type_spec" {
                        collect_type_spec(source, spec, &mut out.definitions);
                    }
                }
            }
            // `const X = …` / `var X T` — un Variable par spec.
            "const_declaration" | "var_declaration" => {
                let spec_kind = if child.kind() == "const_declaration" {
                    "const_spec"
                } else {
                    "var_spec"
                };
                let mut c2 = child.walk();
                for spec in child.children(&mut c2) {
                    if spec.kind() == spec_kind {
                        let name = field_text(source, spec, "name")
                            .unwrap_or_default()
                            .to_string();
                        if !name.is_empty() {
                            out.definitions
                                .push(definition(name, NodeKind::Variable, spec, None));
                        }
                    }
                }
            }
            "import_declaration" => {
                collect_imports(source, child, &mut out.imports);
            }
            _ => {}
        }
    }
}

/// Nom du type parent d'une `method_declaration` (champ `receiver` → un
/// `parameter_list` avec un `parameter_declaration` dont le champ `type`
/// nomme le receiver). `func (p *Point)` comme `func (p Point)` → "Point".
fn receiver_type<'a>(source: &'a str, method: Node) -> Option<&'a str> {
    let recv = method.child_by_field_name("receiver")?;
    let mut cur = recv.walk();
    for param in recv.children(&mut cur) {
        if param.kind() == "parameter_declaration"
            && let Some(ty) = param.child_by_field_name("type")
        {
            return first_type_identifier(source, ty);
        }
    }
    None
}

/// Premier `type_identifier` d'un sous-arbre de type (traverse les types
/// composites : `*T`, `[]T`, `pkg.T`, `G[T]`…). Repli : texte brut amputé
/// d'un `*` ou `[]` éventuel.
fn first_type_identifier<'a>(source: &'a str, node: Node) -> Option<&'a str> {
    if node.kind() == "type_identifier" {
        return Some(text(source, node));
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if let Some(found) = first_type_identifier(source, child) {
            return Some(found);
        }
    }
    // Repli défensif (nœud de type sans type_identifier descendant).
    let t = text(source, node).trim().trim_start_matches('*');
    if t.is_empty() { None } else { Some(t) }
}

/// Un `type_spec` : `type Name …`. Selon le nœud du champ `type` :
/// `struct_type` → Struct, `interface_type` → Interface, alias simple
/// (`type N T`, champ `type` = `type_identifier`) → Type. Les alias vers
/// types composites (map/slice/pointeur…) sont ignorés (hors périmètre).
fn collect_type_spec(source: &str, spec: Node, out: &mut Vec<Definition>) {
    let name = field_text(source, spec, "name")
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        return;
    }
    let kind = match spec.child_by_field_name("type").map(|n| n.kind()) {
        Some("struct_type") => NodeKind::Struct,
        Some("interface_type") => NodeKind::Interface,
        Some("type_identifier") => NodeKind::Type,
        _ => return,
    };
    out.push(definition(name, kind, spec, None));
}

/// Extrait les imports d'une `import_declaration` — spec unique
/// (`import "fmt"`) ou forme parenthésée groupée dans un `import_spec_list`.
fn collect_imports(source: &str, node: Node, out: &mut Vec<Import>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "import_spec" => collect_import_spec(source, child, out),
            "import_spec_list" => {
                let mut c2 = child.walk();
                for spec in child.children(&mut c2) {
                    if spec.kind() == "import_spec" {
                        collect_import_spec(source, spec, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Un `import_spec` : champ `path` (chaîne interprétée "x/y" ou raw), alias
/// optionnel (`alias "x/y"` → module = chemin réel, local_name = alias ;
/// `_`/`.` sont des nœuds distincts, non traités comme alias).
fn collect_import_spec(source: &str, spec: Node, out: &mut Vec<Import>) {
    let path = match field_text(source, spec, "path") {
        // interprété ("fmt") ou raw (`` `x` ``) : retire les délimiteurs.
        Some(p) if p.len() >= 2 => &p[1..p.len() - 1],
        Some(p) => p,
        None => "",
    };
    if path.is_empty() {
        return;
    }
    let local_name = match spec.child_by_field_name("name") {
        Some(n) if n.kind() == "package_identifier" => Some(text(source, n).to_string()),
        _ => None,
    };
    out.push(Import {
        module: path.to_string(),
        local_name,
    });
}

/// Collecte les appels (`call_expression`) : `identifier` seul (appel
/// simple) ou `selector_expression` (`fmt.Println` → name=Println,
/// qualifier=fmt, l'objet étant le champ `operand`).
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "call_expression"
            && let Some(func) = child.child_by_field_name("function")
        {
            let (name, qualifier) = match func.kind() {
                "identifier" => (text(source, func).to_string(), None),
                "selector_expression" => {
                    let name = func
                        .child_by_field_name("field")
                        .map(|n| text(source, n).to_string())
                        .unwrap_or_default();
                    let qualifier = func
                        .child_by_field_name("operand")
                        .map(|n| text(source, n).to_string());
                    (name, qualifier)
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
        // Descend pour les appels imbriqués (arguments, closures…).
        if child.child_count() > 0 {
            collect_calls(source, child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> FileExtraction {
        GoLang::new().extract(src, Path::new("test.go"))
    }

    fn def_kinds(ex: &FileExtraction) -> Vec<(&str, &str, Option<&str>)> {
        ex.definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label(), d.parent_class.as_deref()))
            .collect()
    }

    #[test]
    fn fonction_et_appels() {
        let src = r#"
package main

import "fmt"

func helper(a int) int { return a + 1 }

func main() {
    x := helper(1)
    fmt.Println(x)
}
"#;
        let ex = extract_src(src);
        let defs = def_kinds(&ex);
        assert!(defs.contains(&("helper", "Function", None)));
        assert!(defs.contains(&("main", "Function", None)));
        // Signature renseignée sur les fonctions.
        let helper = ex.definitions.iter().find(|d| d.name == "helper").unwrap();
        assert!(
            helper
                .signature
                .as_deref()
                .unwrap()
                .starts_with("func helper")
        );
        // Appel simple + appel qualifié par package.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "helper" && c.qualifier.is_none())
        );
        let println = ex
            .calls
            .iter()
            .find(|c| c.name == "Println")
            .expect("fmt.Println présent");
        assert_eq!(println.qualifier.as_deref(), Some("fmt"));
        // Ligne 1-based (helper appelé dans le corps de main).
        assert_eq!(
            ex.calls.iter().find(|c| c.name == "helper").unwrap().line,
            9
        );
    }

    #[test]
    fn methode_avec_receiver() {
        let src = r#"
package shapes

type Point struct{ X, Y float64 }

func (p *Point) Scale(f float64) { p.X *= f }

func (p Point) Dist() float64 { return 0 }
"#;
        let ex = extract_src(src);
        let defs = def_kinds(&ex);
        // struct + 2 méthodes rattachées au type du receiver (pointeur déréférencé).
        assert!(defs.contains(&("Point", "Struct", None)));
        assert!(defs.contains(&("Scale", "Method", Some("Point"))));
        assert!(defs.contains(&("Dist", "Method", Some("Point"))));
    }

    #[test]
    fn struct_interface_et_alias() {
        let src = r#"
package api

type User struct {
    ID   int
    Name string
}

type Reader interface {
    Read(p []byte) (int, error)
}

type UserID int
"#;
        let ex = extract_src(src);
        let defs = def_kinds(&ex);
        assert!(defs.contains(&("User", "Struct", None)));
        assert!(defs.contains(&("Reader", "Interface", None)));
        assert!(defs.contains(&("UserID", "Type", None)));
    }

    #[test]
    fn imports_avec_alias() {
        let src = r#"
package main

import (
    "fmt"
    alias "github.com/acme/lib/core"
    _ "net/http/pprof"
)

func main() { fmt.Println("ok") }
"#;
        let ex = extract_src(src);
        let mods: Vec<(&str, Option<&str>)> = ex
            .imports
            .iter()
            .map(|i| (i.module.as_str(), i.local_name.as_deref()))
            .collect();
        assert!(mods.contains(&("fmt", None)));
        // alias : module = chemin réel, local_name = alias.
        assert!(mods.contains(&("github.com/acme/lib/core", Some("alias"))));
        // import muet `_` : pas d'alias.
        assert!(mods.contains(&("net/http/pprof", None)));
    }

    #[test]
    fn const_et_var() {
        let src = r#"
package main

const (
    Version = "1.0"
    MaxSize = 1024
)

var counter int

const Solo = 1
"#;
        let ex = extract_src(src);
        let defs = def_kinds(&ex);
        assert!(defs.contains(&("Version", "Variable", None)));
        assert!(defs.contains(&("MaxSize", "Variable", None)));
        assert!(defs.contains(&("counter", "Variable", None)));
        assert!(defs.contains(&("Solo", "Variable", None)));
    }
}
