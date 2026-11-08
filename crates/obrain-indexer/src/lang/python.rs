//! Langage Python — grammaire tree-sitter (RFC M6 §5).
//!
//! Suit le pattern du module Rust de référence (`rust.rs`) : parser statique
//! partagé, marche de l'arbre pour les définitions/imports (niveau module et
//! corps de classe), puis collecte des appels sur tout l'arbre.
//!
//! Particularités de la grammaire `tree-sitter-python` (v0.25) : champs
//! nommés (`function_definition.name`, `class_definition.body`,
//! `import_statement.name` multiple, `import_from_statement.module_name`,
//! `decorated_definition.definition`, `call.function`,
//! `attribute.object`/`attribute`) et définitions décorées
//! (`decorated_definition`) dans lesquelles il faut descendre.

use std::cell::RefCell;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::parse::{Call, Definition, FileExtraction, Import, Language};
use crate::schema::NodeKind;

thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Texte brut couvert par un nœud (utilitaire).
fn text<'a>(source: &'a str, node: Node) -> &'a str {
    &source[node.byte_range()]
}

#[derive(Clone, Default)]
pub struct PythonLang;

impl PythonLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for PythonLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["py"]
    }

    fn name(&self) -> &'static str {
        "python"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_python::LANGUAGE.into())
                    .expect("grammaire valide");
                *borrow = Some(p);
            }
            borrow.as_mut().unwrap().parse(source, None)
        });
        let Some(tree) = tree else { return out };
        let root = tree.root_node();
        walk_block(source, root, None, &mut out);
        collect_calls(source, root, &mut out.calls);
        out
    }
}

/// Marche un bloc de déclarations (module ou corps de classe) et extrait les
/// définitions + imports. `parent_class` = classe englobante éventuelle : une
/// `function_definition` y devient un Method de cette classe. On ne descend
/// PAS dans les corps de fonctions (une `def` imbriquée n'est pas une
/// définition de module), mais on descend dans `decorated_definition` pour
/// atteindre la définition décorée.
fn walk_block(source: &str, node: Node, parent_class: Option<&str>, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "function_definition" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                let sig = text(source, child).to_string();
                out.definitions.push(Definition {
                    name,
                    kind: match parent_class {
                        Some(_) => NodeKind::Method,
                        None => NodeKind::Function,
                    },
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: Some(sig),
                    parent_class: parent_class.map(|s| s.to_string()),
                    metrics: crate::metrics::for_function(child, "python"),
                    docstring: crate::docs::docstring_of(child, source),
                });
            }
            "class_definition" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name: name.clone(),
                    kind: NodeKind::Class,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
                // Les `function_definition` du corps deviennent des Methods.
                if let Some(body) = child.child_by_field_name("body") {
                    walk_block(source, body, Some(&name), out);
                }
            }
            // `@decorateur\ndef f(): …` — descend jusqu'à la définition ; le
            // ou les décorateurs sont ignorés (aucun nœud Decorator produit).
            "decorated_definition" => {
                walk_block(source, child, parent_class, out);
            }
            "import_statement" => collect_import_statement(source, child, &mut out.imports),
            "import_from_statement" => collect_import_from(source, child, &mut out.imports),
            _ => {}
        }
    }
}

/// Extrait les imports d'un `import_statement` (`import a.b, c as d`).
///
/// Formes tree-sitter (python) : les entrées sont dans le champ multiple
/// `name` — un `dotted_name` (`import os.path`) ou un `aliased_import`
/// (`import os.path as p` → module = `os.path`, local_name = `p`).
fn collect_import_statement(source: &str, node: Node, out: &mut Vec<Import>) {
    let mut cur = node.walk();
    for item in node.children_by_field_name("name", &mut cur) {
        match item.kind() {
            "dotted_name" => {
                let module = text(source, item).to_string();
                if !module.is_empty() {
                    out.push(Import {
                        module,
                        local_name: None,
                    });
                }
            }
            "aliased_import" => {
                let module = item
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                let alias = item
                    .child_by_field_name("alias")
                    .map(|n| text(source, n).to_string());
                if !module.is_empty() {
                    out.push(Import {
                        module,
                        local_name: alias,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Extrait les imports d'un `import_from_statement`
/// (`from m import a as b` → module `m.a`, local_name `b`).
///
/// Le module de base est le champ `module_name` (`from os import path` →
/// base `os`) ; chaque entrée du champ multiple `name` est qualifiée par ce
/// préfixe : `from os import path` → module `os.path`.
fn collect_import_from(source: &str, node: Node, out: &mut Vec<Import>) {
    let base = node
        .child_by_field_name("module_name")
        .map(|n| text(source, n).to_string())
        .unwrap_or_default();
    if base.is_empty() {
        return;
    }
    let mut cur = node.walk();
    for item in node.children_by_field_name("name", &mut cur) {
        match item.kind() {
            "dotted_name" => {
                let name = text(source, item);
                out.push(Import {
                    module: format!("{base}.{name}"),
                    local_name: None,
                });
            }
            "aliased_import" => {
                let name = item
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                let alias = item
                    .child_by_field_name("alias")
                    .map(|n| text(source, n).to_string());
                out.push(Import {
                    module: format!("{base}.{name}"),
                    local_name: alias,
                });
            }
            _ => {}
        }
    }
}

/// Collecte les appels (`call` → nom appelé + ligne) dans tout l'arbre, en
/// visitant chaque nœud une seule fois (les appels imbriqués — arguments,
/// corps de fonctions, lambdas, décorateurs — sont trouvés par la descente).
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "call"
            && let Some(func) = child.child_by_field_name("function")
        {
            let (name, qualifier) = match func.kind() {
                // `helper(...)` → helper
                "identifier" => (text(source, func).to_string(), None),
                // `obj.method()` → method, qualifié par l'objet (`a.b.c()` →
                // name = c, qualifier = a.b).
                "attribute" => {
                    let name = func
                        .child_by_field_name("attribute")
                        .map(|n| text(source, n).to_string())
                        .unwrap_or_default();
                    let qualifier = func
                        .child_by_field_name("object")
                        .map(|n| text(source, n).to_string());
                    (name, qualifier)
                }
                // `call` (ex: `factory()()`), `subscript`, … : rien à
                // reporter ici — le call interne est capturé par la descente
                // récursive ci-dessous.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> FileExtraction {
        PythonLang::new().extract(src, Path::new("test.py"))
    }

    #[test]
    fn fonctions_au_niveau_module() {
        let src = r#"
def helper(x):
    return x + 1

def main():
    total = helper(2)
    return total
"#;
        let ex = extract_src(src);
        let defs: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label(), d.start_line, d.end_line))
            .collect();
        assert!(defs.contains(&("helper", "Function", 2, 3)));
        assert!(defs.contains(&("main", "Function", 5, 7)));
        // Les fonctions de module n'ont pas de classe parente.
        assert!(
            ex.definitions
                .iter()
                .all(|d| d.kind == NodeKind::Function && d.parent_class.is_none())
        );
        // `helper` est appelée dans `main` (ligne 6 = row 5 + 1).
        assert!(ex.calls.iter().any(|c| c.name == "helper" && c.line == 6));
        // Signature = texte du nœud.
        let main = ex.definitions.iter().find(|d| d.name == "main").unwrap();
        assert!(
            main.signature
                .as_deref()
                .unwrap()
                .starts_with("def main():")
        );
    }

    #[test]
    fn methodes_dans_classe() {
        let src = r#"
class Compte:
    def __init__(self, solde):
        self.solde = solde

    def deposer(self, montant):
        self.solde += montant

    @property
    def solde_courant(self):
        return self.solde
"#;
        let ex = extract_src(src);
        // La classe est une Class, ses defs internes des Method de Compte
        // (y compris celle sous `decorated_definition`).
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Compte" && d.kind == NodeKind::Class)
        );
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(methods.len(), 3);
        let names: Vec<_> = methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"__init__"));
        assert!(names.contains(&"deposer"));
        assert!(names.contains(&"solde_courant")); // via @property
        assert!(
            methods
                .iter()
                .all(|m| m.parent_class.as_deref() == Some("Compte"))
        );
        // Aucune Function hors classe ici.
        assert!(!ex.definitions.iter().any(|d| d.kind == NodeKind::Function));
    }

    #[test]
    fn definitions_decorees() {
        let src = r#"
@app.route("/")
def index():
    return "ok"

@dataclass
class Point:
    x: int
    y: int
"#;
        let ex = extract_src(src);
        let kinds: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label(), d.start_line))
            .collect();
        // La descente dans decorated_definition atteint les définitions.
        assert!(kinds.contains(&("index", "Function", 3)));
        assert!(kinds.contains(&("Point", "Class", 7)));
        // Le décorateur `@app.route("/")` contient un vrai call (route).
        assert!(ex.calls.iter().any(|c| c.name == "route"));
    }

    #[test]
    fn imports_et_aliases() {
        let src = r#"
import os
import os.path as osp
import json, re

def f():
    pass
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(mods.contains(&"os"));
        assert!(mods.contains(&"os.path"));
        assert!(mods.contains(&"json"));
        assert!(mods.contains(&"re"));
        // `import os.path as osp` → local_name = osp.
        let alias = ex.imports.iter().find(|i| i.module == "os.path").unwrap();
        assert_eq!(alias.local_name.as_deref(), Some("osp"));
    }

    #[test]
    fn imports_from() {
        let src = r#"
from collections import namedtuple
from os import path
from a.b import c as d, e
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(mods.contains(&"collections.namedtuple"));
        assert!(mods.contains(&"os.path"));
        assert!(mods.contains(&"a.b.c"));
        assert!(mods.contains(&"a.b.e"));
        // `from a.b import c as d` → local_name = d.
        let alias = ex.imports.iter().find(|i| i.module == "a.b.c").unwrap();
        assert_eq!(alias.local_name.as_deref(), Some("d"));
        // Seule cette entrée porte un alias.
        let aliased: Vec<_> = ex
            .imports
            .iter()
            .filter(|i| i.local_name.is_some())
            .collect();
        assert_eq!(aliased.len(), 1);
    }

    #[test]
    fn appels_fonctions_et_methodes() {
        let src = r#"def main():
    helper(1)
    obj.meth()
    a.b.c()
    factory()()
"#;
        let ex = extract_src(src);
        let calls: Vec<_> = ex
            .calls
            .iter()
            .map(|c| (c.name.as_str(), c.qualifier.as_deref(), c.line))
            .collect();
        assert_eq!(calls.len(), 4);
        assert!(calls.contains(&("helper", None, 2)));
        assert!(calls.contains(&("meth", Some("obj"), 3)));
        assert!(calls.contains(&("c", Some("a.b"), 4)));
        // `factory()()` — seul le call interne est reporté (nom du callable).
        assert!(calls.contains(&("factory", None, 5)));
    }
}
