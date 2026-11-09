//! Langage Rust — grammaire tree-sitter (implémentation de RÉFÉRENCE).
//!
//! C'est le module modèle pour les autres langages (RFC M6 §5) : il montre le
//! pattern complet d'extraction — parser, marcher l'arbre, produire
//! définitions (Function/Method/Struct/Enum/Trait/Module/Macro/Const/Static/
//! Type), appels et imports. Le corpus de mesure M6 est le repo obrain
//! (Rust) : la fidélité de ce module détermine la comparaison avec
//! l'indexeur C.

use std::cell::RefCell;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::parse::{Binding, Call, Definition, FileExtraction, Import, Language};
use crate::schema::NodeKind;

// Parser tree-sitter PAR THREAD (thread_local) — le Mutex global sérialisait
// les parses (2,13 s séquentiel → 1,87 s avec rayon : AUCUN gain). Un parser
// par thread : 1,92 s → 0,29 s (×6,6) sur 823 fichiers .rs. Chaque thread
// de rayon crée le sien au premier use.
thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Extraits texte d'un nœud (utilitaire).
fn text<'a>(source: &'a str, node: Node) -> &'a str {
    &source[node.byte_range()]
}

#[derive(Clone, Default)]
pub struct RustLang;

impl RustLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for RustLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn name(&self) -> &'static str {
        "rust"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_rust::LANGUAGE.into())
                    .expect("grammaire rust valide");
                *borrow = Some(p);
            }
            borrow.as_mut().unwrap().parse(source, None)
        });
        let Some(tree) = tree else { return out };
        walk_items(source, tree.root_node(), None, &mut out);
        // let-bindings typables : base de la passe « récepteur » — un fichier
        // peut contenir plusieurs impl/fn ; le filtrage par def se fait à la
        // résolution (index.rs), ici on collecte tout le fichier en un pass.
        collect_bindings(source, tree.root_node(), &mut out.bindings);
        out
    }
}

/// Collecte les let-declarations typables (`let x = Type::…`, `let x: Type`)
/// et les paramètres de fonctions/méthodes (`fn f(x: &Type)`) — la passe
/// « récepteur » résout `x.méthode()` pour les deux sources.
fn collect_bindings(source: &str, node: Node, out: &mut Vec<Binding>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "let_declaration" => {
                if let Some(b) = binding_of(source, child) {
                    out.push(b);
                }
            }
            "function_item" | "function_signature_item" => {
                // Paramètres typés du def → pseudo-bindings à la ligne du def.
                let line = child.start_position().row as u32 + 1;
                if let Some(params) = child.child_by_field_name("parameters") {
                    let mut pc = params.walk();
                    for p in params.children(&mut pc) {
                        if p.kind() != "parameter" {
                            continue;
                        }
                        let Some(pat) = p.child_by_field_name("pattern") else {
                            continue;
                        };
                        if pat.kind() != "identifier" {
                            continue;
                        }
                        let name = text(source, pat).to_string();
                        if name == "_" {
                            continue;
                        }
                        let ty = p
                            .child_by_field_name("type")
                            .map(|t| text(source, t).to_string());
                        out.push(Binding {
                            name,
                            type_ref: ty,
                            line,
                            init_call: None,
                            init_fn: None,
                        });
                    }
                }
            }
            _ => {}
        }
        if child.child_count() > 0 {
            collect_bindings(source, child, out);
        }
    }
}

/// Type/binding d'un `let_declaration` :
/// - annotation `let x: Type` → type_ref ;
/// - initialiseur `let x = Type::fn(…)` / `Type { … }` → type_ref ;
/// - initialiseur `let x = recv.méthode(…)` → init_call (cascade par type de
///   retour de la méthode résolue du récepteur) ;
/// - initialiseur `let x = fn_libre(…)` → init_fn (étape 2 : cascade par type
///   de retour de la fonction libre du module — `fn setup() -> ObrainDB` puis
///   `let db = setup(); db.…()`).
fn binding_of(source: &str, letn: Node) -> Option<Binding> {
    let pat = letn.child_by_field_name("pattern")?;
    // Seulement les patterns identificateur simples (pas de destructuring).
    let name = match pat.kind() {
        "identifier" => text(source, pat).to_string(),
        _ => return None,
    };
    let line = letn.start_position().row as u32 + 1;
    // 1. Annotation explicite `let x: Type` (le plus fiable).
    let ty_annot = letn
        .child_by_field_name("type")
        .map(|t| text(source, t).to_string());
    // 2. Initialiseur.
    let mut init_call: Option<(String, String)> = None;
    let mut init_fn: Option<String> = None;
    let ty_val = letn.child_by_field_name("value").and_then(|v| {
        match v.kind() {
            "call_expression" => {
                let func = v.child_by_field_name("function")?;
                match func.kind() {
                    // `ObrainDB::open_substrate(…)` → chemin du type.
                    "scoped_identifier" => func
                        .child_by_field_name("path")
                        .map(|p| text(source, p).to_string()),
                    // `db.session(…)` → cascade de bindings.
                    "field_expression" => {
                        let m = func
                            .child_by_field_name("field")
                            .map(|n| text(source, n).to_string())?;
                        let recv = func
                            .child_by_field_name("value")
                            .map(|n| text(source, n).to_string())?;
                        init_call = Some((recv, m));
                        None
                    }
                    // `setup(…)` (fonction libre, pas un constructeur typé) →
                    // typage par retour de la fonction (étape 2).
                    "identifier" => {
                        let f = text(source, func).to_string();
                        if f != name {
                            init_fn = Some(f);
                        }
                        None
                    }
                    _ => None,
                }
            }
            // `Point { x: 1 }` → nom du struct.
            "struct_expression" => v
                .child_by_field_name("name")
                .map(|n| text(source, n).to_string()),
            _ => None,
        }
    });
    let type_ref = ty_annot.or(ty_val);
    Some(Binding {
        name,
        type_ref,
        line,
        init_call,
        init_fn,
    })
}

/// Marche un bloc de déclarations (source ou corps d'impl/mod).
fn walk_items(source: &str, node: Node, parent_class: Option<&str>, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            // ── Définitions top-level ────────────────────────────────────
            // function_item : fn avec corps. function_signature_item : méthode
            // de trait REQUISE sans corps (`fn foo(&self);`). Les deux portent
            // un champ name et deviennent Function (module) ou Method (impl/
            // trait) selon parent_class.
            "function_item" | "function_signature_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                let sig = text(source, child).to_string();
                match parent_class {
                    Some(cls) => out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Method,
                        start_line: child.start_position().row as u32 + 1,
                        end_line: child.end_position().row as u32 + 1,
                        signature: Some(sig),
                        parent_class: Some(cls.to_string()),
                        metrics: crate::metrics::for_function(child, "rust"),
                        docstring: crate::docs::docstring_of(child, source),
                    }),
                    None => out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Function,
                        start_line: child.start_position().row as u32 + 1,
                        end_line: child.end_position().row as u32 + 1,
                        signature: Some(sig),
                        parent_class: None,
                        metrics: crate::metrics::for_function(child, "rust"),
                        docstring: crate::docs::docstring_of(child, source),
                    }),
                }
            }
            "struct_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name: name.clone(),
                    kind: NodeKind::Struct,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
                // Champs du struct (`struct Wal { store: T, … }`) — la passe
                // « récepteur » (étape 2) résout `self.store.method()` via le
                // type du champ.
                if let Some(body) = child.child_by_field_name("body") {
                    let mut bc = body.walk();
                    for fd in body.children(&mut bc) {
                        if fd.kind() != "field_declaration" {
                            continue;
                        }
                        let fname = fd
                            .child_by_field_name("name")
                            .map(|n| text(source, n).to_string())
                            .unwrap_or_default();
                        let fty = fd
                            .child_by_field_name("type")
                            .map(|t| text(source, t).to_string())
                            .unwrap_or_default();
                        if !fname.is_empty() && !fty.is_empty() {
                            out.struct_fields.push((name.clone(), fname, fty));
                        }
                    }
                }
            }
            "enum_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name,
                    kind: NodeKind::Enum,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
            }
            "trait_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name: name.clone(),
                    kind: NodeKind::Interface,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
                // Les méthodes du trait (requises SANS corps = function_signature_item,
                // et par défaut AVEC corps = function_item) deviennent des Methods
                // rattachées au trait. Le C les compte — l'équivalence l'exige.
                if let Some(body) = child.child_by_field_name("body") {
                    walk_items(source, body, Some(&name), out);
                }
            }
            "type_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name,
                    kind: NodeKind::Type,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
            }
            "const_item" | "static_item" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name,
                    kind: NodeKind::Variable,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: parent_class.map(|s| s.to_string()),
                    metrics: None,
                    docstring: None,
                });
            }
            "macro_definition" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name,
                    kind: NodeKind::Macro,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
            }
            "mod_item" => {
                // `mod foo { … }` — les items internes sont rattachés à ce module.
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                out.definitions.push(Definition {
                    name,
                    kind: NodeKind::Module,
                    start_line: child.start_position().row as u32 + 1,
                    end_line: child.end_position().row as u32 + 1,
                    signature: None,
                    parent_class: None,
                    metrics: None,
                    docstring: None,
                });
                if let Some(body) = child.child_by_field_name("body") {
                    // Descend dans le body (declaration_list) pour trouver les
                    // définitions internes (ex: `mod tests { fn helper() {} }`).
                    walk_items(source, body, parent_class, out);
                }
            }
            "impl_item" => {
                // `impl Foo { fn m() {} }` — les fonctions deviennent des Methods
                // rattachées à Foo.
                let ty = child
                    .child_by_field_name("type")
                    .map(|n| text(source, n).to_string());
                let body = child.child_by_field_name("body");
                if let (Some(ty), Some(body)) = (ty, body) {
                    walk_items(source, body, Some(&ty), out);
                }
            }
            "declaration_list" => {
                // Conteneur générique (body de mod) : itère son contenu.
                walk_items(source, child, parent_class, out);
            }
            "use_declaration" => {
                collect_use(source, child, &mut out.imports);
            }
            _ => {}
        }
        // Appels n'importe où dans le sous-arbre (fonctions imbriquées, bodies).
        collect_calls(source, child, &mut out.calls);
    }
}

/// Extrait les imports d'une `use_declaration`.
///
/// Formes tree-sitter (rust) :
/// - `use a::b::c;`            → scoped_identifier
/// - `use a::b::{c, d as e};`  → scoped_use_list (scoped_identifier + use_list)
/// - `use {a, b};`             → use_list nu
fn collect_use(source: &str, node: Node, out: &mut Vec<Import>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "scoped_identifier" => {
                out.push(Import {
                    module: scoped_path(source, child),
                    local_name: None,
                });
            }
            "scoped_use_list" => {
                // préfixe + liste entre accolades.
                let mut c2 = child.walk();
                let mut prefix = String::new();
                for item in child.children(&mut c2) {
                    match item.kind() {
                        "scoped_identifier" => prefix = scoped_path(source, item),
                        "identifier" => {
                            if prefix.is_empty() {
                                prefix = text(source, item).to_string();
                            }
                        }
                        "use_list" => {
                            let mut c3 = item.walk();
                            for ent in item.children(&mut c3) {
                                use_list_entry(source, ent, &prefix, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "use_list" => {
                // use_list nu (`use {a, b};`)
                let mut c2 = child.walk();
                for ent in child.children(&mut c2) {
                    use_list_entry(source, ent, "", out);
                }
            }
            _ => {}
        }
    }
}

/// Une entrée de use_list : identifier, use_as_clause (`d as e`), ou imbrication.
fn use_list_entry(source: &str, node: Node, prefix: &str, out: &mut Vec<Import>) {
    match node.kind() {
        "identifier" => {
            let name = text(source, node);
            out.push(Import {
                module: if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}.{name}")
                },
                local_name: None,
            });
        }
        "use_as_clause" => {
            // `Edge as Arrete` — child0 = nom réel, child2 = alias.
            let name = node
                .child(0)
                .map(|n| text(source, n).to_string())
                .unwrap_or_default();
            let alias = node.child(2).map(|n| text(source, n).to_string());
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            if !full.is_empty() {
                out.push(Import {
                    module: full,
                    local_name: alias,
                });
            }
        }
        "scoped_identifier" => {
            // entrée imbriquée `use a::{b::c}`.
            out.push(Import {
                module: scoped_path(source, node),
                local_name: None,
            });
        }
        "use_wildcard" if !prefix.is_empty() => {
            out.push(Import {
                module: prefix.to_string(),
                local_name: None,
            });
        }
        _ => {}
    }
}

/// Chemin pointé complet d'un scoped_identifier (`a::b::c` → "a.b.c").
/// Structure tree-sitter : imbrication gauche-droite de segments.
fn scoped_path(source: &str, node: Node) -> String {
    let mut parts: Vec<&str> = Vec::new();
    collect_segments(source, node, &mut parts);
    parts.join(".")
}

/// Accumule les segments d'un chemin :: séparé (gauche d'abord).
fn collect_segments<'a>(source: &'a str, node: Node, parts: &mut Vec<&'a str>) {
    match node.kind() {
        "scoped_identifier" => {
            // child0 = partie gauche (chemin), child1 = `::`, child2 = segment.
            if let Some(left) = node.child(0) {
                collect_segments(source, left, parts);
            }
            if let Some(right) = node.child(2) {
                match right.kind() {
                    "identifier" | "type_identifier" | "self" | "crate" | "super" => {
                        parts.push(text(source, right));
                    }
                    _ => collect_segments(source, right, parts),
                }
            }
        }
        "identifier" | "type_identifier" | "self" | "crate" | "super" => {
            parts.push(text(source, node));
        }
        _ => {
            // récupère le champ name si présent (défensif)
            if let Some(n) = node.child_by_field_name("name") {
                parts.push(text(source, n));
            }
        }
    }
}

/// Collecte les appels (`call_expression` → nom appelé + ligne).
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    collect_calls_at(source, node, out, 0);
}

/// Variante avec décalage de ligne (pour le re-parse des arguments de macro :
/// l'arbre du sous-texte commence à la ligne 0, on ajoute la ligne absolue du
/// début de l'argument).
fn collect_calls_at(source: &str, node: Node, out: &mut Vec<Call>, row_base: u32) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "call_expression"
            && let Some(func) = child.child_by_field_name("function")
        {
            let (name, qualifier) = match func.kind() {
                "identifier" => (text(source, func).to_string(), None),
                "field_expression" => {
                    // `obj.method()` → method
                    let m = func
                        .child_by_field_name("field")
                        .map(|n| text(source, n).to_string())
                        .unwrap_or_default();
                    let q = func
                        .child_by_field_name("value")
                        .map(|n| text(source, n).to_string());
                    (m, q)
                }
                "scoped_identifier" => {
                    // `foo::bar()` → bar qualifié foo
                    let name = func
                        .child_by_field_name("name")
                        .map(|n| text(source, n).to_string())
                        .unwrap_or_default();
                    let q = func
                        .child_by_field_name("path")
                        .map(|n| text(source, n).to_string());
                    (name, q)
                }
                "generic_function" => {
                    // `foo::<T>()`
                    func.child_by_field_name("function")
                        .map(|n| (text(source, n).to_string(), None))
                        .unwrap_or_default()
                }
                _ => (String::new(), None),
            };
            if !name.is_empty() {
                out.push(Call {
                    name,
                    qualifier,
                    line: row_base + child.start_position().row as u32 + 1,
                });
            }
        } else if child.kind() == "macro_invocation" {
            // Les appels dans les arguments de macro (`assert!(x.m())`,
            // `panic!("…{}", r.len())`…) ne sont PAS des call_expression de
            // l'arbre (le contenu est un token_tree brut) — on re-parse le
            // texte de l'argument pour en extraire les vrais appels.
            collect_calls_from_macro(source, child, out);
        }
        // Descend pour les appels imbriqués (closures, bodies).
        if child.child_count() > 0 {
            collect_calls_at(source, child, out, row_base);
        }
    }
}

/// Re-parse le contenu d'un `macro_invocation` (`assert!(…)`, `format!(…)`,
/// `vec![…]`…) pour extraire les appels de méthode/fonction qu'il contient —
/// des appels réels (`assert!(db.execute(…).is_ok())`) invisibles à la marche
/// AST normale (token_tree brut, pas de call_expression).
fn collect_calls_from_macro(source: &str, node: Node, out: &mut Vec<Call>) {
    // token_tree = nœud englobant « (…)/[…]/ {…} » de la macro.
    let mut tt = None;
    {
        let mut c = node.walk();
        for ch in node.children(&mut c) {
            if ch.kind() == "token_tree" {
                tt = Some(ch);
                break;
            }
        }
    }
    let Some(tt) = tt else { return };
    let bytes = tt.byte_range();
    if bytes.len() < 2 {
        return;
    }
    let body = &source[bytes.clone()];
    let first = body.as_bytes()[0];
    let last = body.as_bytes()[body.len() - 1];
    let (open, close) = match (first, last) {
        (b'(', b')') | (b'[', b']') | (b'{', b'}') => (1usize, body.len() - 1),
        _ => return,
    };
    let inner = &source[bytes.start + open..bytes.start + close];
    if inner.trim().is_empty() {
        return;
    }
    // Ligne absolue du début de l'argument (la macro peut s'étendre sur
    // plusieurs lignes) : comptage des '\n' entre le début de la macro et
    // l'argument.
    let prefix = &source[node.start_byte()..bytes.start + open];
    let row_base =
        node.start_position().row as u32 + prefix.bytes().filter(|b| *b == b'\n').count() as u32;
    let tree: Option<tree_sitter::Tree> = PARSER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            let mut p = Parser::new();
            p.set_language(&tree_sitter_rust::LANGUAGE.into())
                .expect("grammaire rust valide");
            *borrow = Some(p);
        }
        borrow.as_mut().unwrap().parse(inner, None)
    });
    let Some(tree) = tree else { return };
    collect_calls_at(inner, tree.root_node(), out, row_base);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> FileExtraction {
        RustLang::new().extract(src, Path::new("test.rs"))
    }

    #[test]
    fn fonctions_et_structs() {
        let src = r#"
fn helper(a: i32) -> i32 { a + 1 }
struct Point { x: i32 }
pub fn main() { let p = Point { x: helper(1) }; }
"#;
        let ex = extract_src(src);
        let names: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label()))
            .collect();
        assert!(names.contains(&("helper", "Function")));
        assert!(names.contains(&("Point", "Struct")));
        assert!(names.contains(&("main", "Function")));
        // helper appelé dans main.
        assert!(ex.calls.iter().any(|c| c.name == "helper"));
    }

    #[test]
    fn methodes_dans_impl() {
        let src = r#"
struct S;
impl S {
    fn new() -> Self { S }
    fn run(&self) {}
}
"#;
        let ex = extract_src(src);
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(methods.len(), 2);
        assert!(
            methods
                .iter()
                .all(|m| m.parent_class.as_deref() == Some("S"))
        );
    }

    #[test]
    fn imports_scoped() {
        let src = r#"
use std::collections::HashMap;
use crate::graph::{Node, Edge as Arrete};
use serde::Serialize;
"#;
        let ex = extract_src(src);
        let mods: Vec<_> = ex.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(mods.contains(&"std.collections.HashMap"));
        assert!(mods.contains(&"crate.graph.Node"));
        assert!(mods.contains(&"crate.graph.Edge"));
        assert!(mods.contains(&"serde.Serialize"));
    }

    #[test]
    fn methodes_de_trait_sans_corps() {
        // `fn foo(&self);` dans un trait (requis, SANS corps) = nœud
        // function_signature_item — doit devenir une Method du trait
        // (l'indexeur C les compte ; traits.rs en a 121).
        let src = r#"
trait Foo {
    fn requis(&self, id: u64) -> Option<u64>;
    fn par_defaut(&self) -> bool { true }
}
"#;
        let ex = extract_src(src);
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(
            methods.len(),
            2,
            "méthodes de trait manquantes: {:?}",
            ex.definitions
        );
        assert!(methods.iter().any(|m| m.name == "requis"));
        assert!(methods.iter().any(|m| m.name == "par_defaut"));
        assert!(
            methods
                .iter()
                .all(|m| m.parent_class.as_deref() == Some("Foo"))
        );
    }

    #[test]
    fn enum_trait_type() {
        let src = r#"
enum Color { Red, Green }
trait Draw { fn draw(&self); }
type Alias = u64;
"#;
        let ex = extract_src(src);
        let kinds: Vec<_> = ex.definitions.iter().map(|d| d.kind).collect();
        assert!(kinds.contains(&NodeKind::Enum));
        assert!(kinds.contains(&NodeKind::Interface)); // trait → Interface
        assert!(kinds.contains(&NodeKind::Type));
    }

    #[test]
    fn fonctions_dans_mod_tests_imbrique() {
        // Les `fn` dans `mod tests { ... }` doivent être extraites (le C les
        // compte — gap de richesse identifié vs l'indexeur C).
        let src = r#"
#[cfg(test)]
mod tests {
    fn helper() {}
    #[test]
    fn test_x() {}
}
"#;
        let ex = extract_src(src);
        let fns: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.name == "helper" || d.name == "test_x")
            .collect();
        assert_eq!(
            fns.len(),
            2,
            "fn dans mod tests non extraites: {:?}",
            ex.definitions
        );
        assert!(fns.iter().all(|d| d.kind == NodeKind::Function));
    }

    #[test]
    fn let_bindings_typables() {
        let src = r#"
fn main() {
    let db = ObrainDB::open_substrate("/tmp/x");
    let db2: ObrainDB = db;
    let db3 = crate::database::ObrainDB::new();
    let p = Point { x: 1 };
    let (a, b) = (1, 2);
    let Some(v) = opt else { return };
    db.flush();
}
"#;
        let ex = extract_src(src);
        let b = |n: &str| ex.bindings.iter().find(|b| b.name == n);
        assert_eq!(
            b("db").and_then(|x| x.type_ref.clone()).as_deref(),
            Some("ObrainDB"),
            "constructeur scoped"
        );
        assert_eq!(
            b("db2").and_then(|x| x.type_ref.clone()).as_deref(),
            Some("ObrainDB"),
            "annotation"
        );
        assert_eq!(
            b("db3").and_then(|x| x.type_ref.clone()).as_deref(),
            Some("crate::database::ObrainDB"),
            "chemin crate"
        );
        assert_eq!(
            b("p").and_then(|x| x.type_ref.clone()).as_deref(),
            Some("Point"),
            "struct literal"
        );
        // patterns non-identificateur : ignorés.
        assert!(b("a").is_none());
        assert!(b("v").is_none());
    }

    #[test]
    fn bindings_params_et_init_call() {
        let src = r#"
fn setup() -> ObrainDB { ObrainDB }
fn monter(db: &ObrainDB) {
    let session = db.session();
    session.execute("x");
}
"#;
        let ex = extract_src(src);
        // paramètre typé → pseudo-binding.
        let pdb = ex.bindings.iter().find(|b| b.name == "db");
        assert_eq!(
            pdb.and_then(|x| x.type_ref.clone()).as_deref(),
            Some("&ObrainDB")
        );
        assert_eq!(pdb.map(|x| x.line), Some(3));
        // init_call : `let session = db.session()` → cascade de bindings.
        let s = ex.bindings.iter().find(|b| b.name == "session").unwrap();
        assert!(s.type_ref.is_none(), "pas d'annotation: {:?}", s);
        assert_eq!(s.init_call, Some(("db".to_string(), "session".to_string())));
        // Appel de fonction inconnue (init_fn) : ignoré.
        assert!(ex.bindings.iter().all(|b| b.name != "r"));
    }

    #[test]
    fn struct_fields_capturees() {
        // `struct Wal { store: WalStore, n: u32 }` — champs (nom, type) pour
        // la passe « récepteur » `self.store.method()` (étape 2).
        let src = r#"
struct Wal {
    store: Arc<WalStore>,
    n: u32,
}
struct Vide;
"#;
        let ex = extract_src(src);
        assert!(
            ex.struct_fields
                .iter()
                .any(|(s, f, t)| s == "Wal" && f == "store" && t == "Arc<WalStore>")
        );
        assert!(
            ex.struct_fields
                .iter()
                .any(|(s, f, t)| s == "Wal" && f == "n" && t == "u32")
        );
        assert!(ex.struct_fields.iter().all(|(s, ..)| s != "Vide"));
    }

    #[test]
    fn binding_init_fn_fonction_libre() {
        // `let db = setup();` — initialiseur = appel de fonction libre
        // (pas un constructeur typé ni un appel de méthode) → init_fn.
        let src = r#"
fn setup() -> ObrainDB { ObrainDB }
fn run() {
    let db = setup();
    db.flush();
}
"#;
        let ex = extract_src(src);
        let db = ex.bindings.iter().find(|b| b.name == "db").unwrap();
        assert!(db.type_ref.is_none());
        assert_eq!(db.init_fn.as_deref(), Some("setup"));
        assert_eq!(db.init_call, None);
    }

    #[test]
    fn appels_dans_macro_extraits() {
        // `assert!(catalog.indexes_for_label(x).is_empty())` : l'appel est
        // dans l'argument d'une macro — invisible à la marche AST classique
        // (token_tree) ; la passe macro le re-parse et l'extrait.
        let src = r#"
#[test]
fn t() {
    let catalog = Catalog::new();
    assert!(catalog.indexes_for_label(LabelId::new(999)).is_empty(), "résultat attendu vide");
}
"#;
        let ex = extract_src(src);
        assert!(
            ex.calls.iter().any(|c| c.name == "indexes_for_label"),
            "appel dans assert! non extrait: {:?}",
            ex.calls
        );
        // La ligne doit être absolue (ligne 5 du snippet = l'assert!).
        assert_eq!(
            ex.calls
                .iter()
                .find(|c| c.name == "indexes_for_label")
                .map(|c| c.line),
            Some(5)
        );
        assert!(
            ex.calls.iter().any(|c| c.name == "new"),
            "constructeurs dans la macro manquants: {:?}",
            ex.calls
        );
    }
}
