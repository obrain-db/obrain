//! Langage C++ — grammaire tree-sitter (`tree-sitter-cpp`).
//!
//! Le C++ hérite de l'ensemble des kinds du C (fonctions, struct/union/enum,
//! typedef, macros, includes) et y ajoute : classes, méthodes (définitions
//! INLINE dans le corps de la classe et prototypes sans corps), namespaces et
//! templates.
//!
//! Notes de fidélité à la grammaire `tree-sitter-cpp` 0.23 :
//! - les définitions en portée fichier/namespace sont des enfants directs du
//!   scope (pas enveloppées dans `declaration`) ;
//! - dans un corps de classe, une méthode inline est un `function_definition`
//!   (avec corps), un prototype est un `field_declaration` dont le déclarateur
//!   est un `function_declarator` menant DIRECTEMENT à un nom ; un pointeur de
//!   fonction membre (`void (*cb)(int);`) passe par `parenthesized_declarator`
//!   et reste donc un `Field` ;
//! - les destructeurs (`~A();`, kind `destructor_name`) ne sont pas émis ;
//! - `namespace_definition`/`template_declaration` sont descendus : les
//!   définitions internes gardent leur nom court, le FQN est géré plus loin
//!   par le pipeline (RFC M6 §5).

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

/// Suit la chaîne de champs `declarator` jusqu'au nom feuille.
///
/// NB : dans `tree-sitter-cpp`, `parenthesized_declarator` n'a PAS de champ
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

/// Nom du champ `name` d'un spécificateur (`class Point` → `Point`).
fn spec_name(source: &str, spec: Node) -> Option<String> {
    spec.child_by_field_name("name")
        .map(|n| text(source, n).to_string())
        .filter(|s| !s.is_empty())
}

/// Spécificateur de type avec corps (définition effective) ?
fn is_type_spec_with_body(node: Node) -> bool {
    matches!(
        node.kind(),
        "struct_specifier" | "union_specifier" | "enum_specifier" | "class_specifier"
    ) && node.child_by_field_name("body").is_some()
}

/// Kind de nœud pour un spécificateur de type (class → Class, sinon Struct).
fn spec_kind(node: Node) -> NodeKind {
    if node.kind() == "class_specifier" {
        NodeKind::Class
    } else if node.kind() == "enum_specifier" {
        NodeKind::Enum
    } else {
        NodeKind::Struct
    }
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

/// Marche des déclarations d'une portée (translation_unit, corps de
/// namespace, contenu de template). Ne descend JAMAIS dans les corps de
/// fonction pour les définitions (méthodes inline → corps de classe).
fn walk_decls(source: &str, node: Node, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "function_definition" => {
                // Définition de fonction (éventuellement hors classe, ex.
                // `Point::at(...)`) → nom + parent résolus via le déclarateur.
                if let Some((name, parent)) = function_name(source, child) {
                    let kind = if parent.is_some() {
                        NodeKind::Method
                    } else {
                        NodeKind::Function
                    };
                    let (start_line, end_line) = line_span(child);
                    out.definitions.push(Definition {
                        name,
                        kind,
                        start_line,
                        end_line,
                        signature: Some(text(source, child).to_string()),
                        parent_class: parent,
                        metrics: None,
                        docstring: None,
                    });
                }
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                record_def(source, child, out);
            }
            "enum_specifier" => {
                if let Some(name) = spec_name(source, child)
                    && child.child_by_field_name("body").is_some()
                {
                    push_def(out, name, NodeKind::Enum, child);
                }
            }
            "type_definition" => {
                // `typedef` : type interne sinon alias `Type`.
                typedef_def(source, child, out);
            }
            "declaration" => {
                // Type défini en ligne avec une variable (`class X {…} x;`).
                if let Some(ty) = child.child_by_field_name("type")
                    && is_type_spec_with_body(ty)
                {
                    record_def(source, ty, out);
                }
            }
            "namespace_definition" => {
                // Descend dans le corps : le préfixe FQN est géré par le
                // pipeline (pas de nœud Module émis ici).
                if let Some(body) = child.child_by_field_name("body") {
                    walk_decls(source, body, out);
                }
            }
            "template_declaration" => {
                // `template<…> class/fonction` : la déclaration est dedans.
                walk_decls(source, child, out);
            }
            "preproc_def" | "preproc_function_def" => {
                if let Some(name) = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                {
                    push_def(out, name, NodeKind::Macro, child);
                }
            }
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
            _ => {}
        }
    }
}

/// Nom + éventuel parent d'une `function_definition`.
///
/// - `int foo(int) {…}` → (`foo`, None) ;
/// - `Point::at(int) {…}` (hors classe) → (`at`, Some("Point")) ;
/// - constructeur/destructeur inline gérés via le déclarateur.
fn function_name(source: &str, node: Node) -> Option<(String, Option<String>)> {
    let decl = node.child_by_field_name("declarator")?;
    match decl.kind() {
        k if is_name(k) => Some((text(source, decl).to_string(), None)),
        "function_declarator" => {
            let inner = decl.child_by_field_name("declarator")?;
            match inner.kind() {
                k if is_name(k) => Some((text(source, inner).to_string(), None)),
                "qualified_identifier" => {
                    // `Foo::bar` → bar, parent Foo.
                    let (name, scope) = qualified_parts(source, inner)?;
                    Some((name, Some(scope)))
                }
                // Destructeurs (`~A`) et surcharges d'opérateurs : non émis.
                "destructor_name" | "operator_name" => None,
                _ => None,
            }
        }
        _ => None,
    }
}

/// Segments d'un `qualified_identifier` : `ns::Foo::bar` → (bar, «ns::Foo»).
fn qualified_parts(source: &str, qid: Node) -> Option<(String, String)> {
    let scope = qid
        .child_by_field_name("scope")
        .map(|n| text(source, n).to_string())?;
    let name_node = qid.child_by_field_name("name")?;
    let name = match name_node.kind() {
        k if is_name(k) => text(source, name_node).to_string(),
        "template_function" | "template_method" => name_node
            .child_by_field_name("name")
            .map(|n| text(source, n).to_string())?,
        // `Foo::~Foo` / surcharges : ignorés.
        "destructor_name" | "operator_name" => return None,
        _ => return None,
    };
    Some((name, scope))
}

/// Définition d'une classe/struct/union + marche de ses membres.
fn record_def(source: &str, spec: Node, out: &mut FileExtraction) {
    let Some(body) = spec.child_by_field_name("body") else {
        return; // déclaration anticipée
    };
    let Some(name) = spec_name(source, spec) else {
        return; // spécificateur anonyme seul (sans déclarateur)
    };
    let kind = spec_kind(spec);
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

/// Marche du corps d'une classe/struct/union : méthodes (inline + prototypes)
/// et champs rattachés à `parent`, types imbriqués, templates membres.
fn walk_members(source: &str, parent: &str, body: Node, out: &mut FileExtraction) {
    let mut cur = body.walk();
    for child in body.children(&mut cur) {
        match child.kind() {
            "function_definition" => {
                // Méthode inline (avec corps) : `T get() const {…}`.
                if let Some((name, _q)) = function_name(source, child) {
                    let (start_line, end_line) = line_span(child);
                    out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Method,
                        start_line,
                        end_line,
                        signature: Some(text(source, child).to_string()),
                        parent_class: Some(parent.to_string()),
                        metrics: crate::metrics::for_function(child, "cpp"),
                        docstring: crate::docs::docstring_of(child, source),
                    });
                }
            }
            "field_declaration" => {
                member_decl(source, child, parent, out);
            }
            "declaration" => {
                // Forme des prototypes de destructeurs (`~A();`) : ignorés
                // (le déclarateur est un `destructor_name`, pas un nom).
                if let Some(decl) = child.child_by_field_name("declarator")
                    && decl.kind() == "function_declarator"
                    && let Some(inner) = decl.child_by_field_name("declarator")
                    && is_name(inner.kind())
                {
                    push_method(source, child, parent, out);
                }
            }
            "template_declaration" => {
                // Méthodes template dans la classe (`template<…> void f();`).
                walk_members(source, parent, child, out);
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => {
                record_def(source, child, out);
            }
            "enum_specifier" => {
                if let Some(name) = spec_name(source, child)
                    && child.child_by_field_name("body").is_some()
                {
                    push_def(out, name, NodeKind::Enum, child);
                }
            }
            // access_specifier (`public:`…), `using_declaration`, etc.
            _ => {}
        }
    }
}

/// Un membre déclaré par `field_declaration` : prototype de méthode, type
/// imbriqué, ou champ de données.
fn member_decl(source: &str, node: Node, parent: &str, out: &mut FileExtraction) {
    let ty = node.child_by_field_name("type");
    if let Some(t) = ty
        && is_type_spec_with_body(t)
    {
        // Type imbriqué défini en ligne (`class B {…};` dans A).
        if let Some(nested) = spec_name(source, t) {
            let (start_line, end_line) = line_span(t);
            let kind = spec_kind(t);
            out.definitions.push(Definition {
                name: nested.clone(),
                kind,
                start_line,
                end_line,
                signature: None,
                parent_class: None,
                metrics: None,
                docstring: None,
            });
            if kind != NodeKind::Enum
                && let Some(body) = t.child_by_field_name("body")
            {
                walk_members(source, &nested, body, out);
            }
        } else if let Some(body) = t.child_by_field_name("body") {
            // Spécificateur anonyme : transparent (champs → parent).
            walk_members(source, parent, body, out);
        }
        return;
    }
    let decl = node.child_by_field_name("declarator");
    if let Some(decl) = decl
        && decl.kind() == "function_declarator"
        && let Some(inner) = decl.child_by_field_name("declarator")
    {
        match inner.kind() {
            // Prototype de méthode sans corps : `void set(T v);`
            k if is_name(k) => push_method(source, node, parent, out),
            // Pointeur de fonction membre `void (*cb)(int);` → Field cb.
            _ => collect_field_names(source, node, parent, out),
        }
        return;
    }
    // Champ de données : chaque déclarateur devient un Field du parent.
    collect_field_names(source, node, parent, out);
}

/// Prototype de méthode → Method rattaché à `parent`.
fn push_method(source: &str, node: Node, parent: &str, out: &mut FileExtraction) {
    if let Some(decl) = node.child_by_field_name("declarator")
        && let Some(inner) = decl.child_by_field_name("declarator")
        && let Some(name) = chain_name(source, inner)
    {
        let (start_line, end_line) = line_span(node);
        out.definitions.push(Definition {
            name,
            kind: NodeKind::Method,
            start_line,
            end_line,
            signature: Some(text(source, node).to_string()),
            parent_class: Some(parent.to_string()),
            metrics: crate::metrics::for_function(node, "cpp"),
            docstring: crate::docs::docstring_of(node, source),
        });
    }
}

/// Noms de champs d'une déclaration de données : déclarateurs simples frères
/// (`int a, b;`) ou chaînes enveloppées (`void (*cb)(int);` → `cb`).
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

/// Chemin d'un `#include` : `"util.h"` / `<vector>` → `util.h` / `vector`.
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

/// `typedef` : struct/class/enum/union interne → la définition interne, sinon
/// un `Type` nommé par le dernier identifiant du déclarateur.
fn typedef_def(source: &str, node: Node, out: &mut FileExtraction) {
    let Some(ty) = node.child_by_field_name("type") else {
        return;
    };
    if is_type_spec_with_body(ty) {
        let name = match spec_name(source, ty) {
            Some(n) => n,
            // `typedef struct {…} Anon;` → on prend l'alias.
            None => node
                .child_by_field_name("declarator")
                .and_then(|d| chain_name(source, d))
                .unwrap_or_default(),
        };
        let kind = spec_kind(ty);
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
    if let Some(name) = node
        .child_by_field_name("declarator")
        .and_then(|d| chain_name(source, d))
    {
        push_def(out, name, NodeKind::Type, node);
    }
}

/// Collecte les appels : `foo(...)`, `obj.m(...)`, `ns::fn(...)`,
/// `fn<T>(...)`. Descend dans tout le sous-arbre.
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind() == "call_expression"
            && let Some(func) = child.child_by_field_name("function")
        {
            let (name, qualifier) = match func.kind() {
                "identifier" | "field_identifier" => (text(source, func).to_string(), None),
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
                "qualified_identifier" => {
                    // `ns::fn()` → fn qualifié par le scope.
                    match qualified_parts(source, func) {
                        Some((name, scope)) => (name, Some(scope)),
                        None => (String::new(), None),
                    }
                }
                "template_function" | "template_method" => {
                    // `fn<T>(...)` → nom depuis le champ `name`.
                    let name = func
                        .child_by_field_name("name")
                        .map(|n| text(source, n).to_string())
                        .unwrap_or_default();
                    (name, None)
                }
                // Destructeurs / surcharges d'opérateurs : non émis.
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
pub struct CppLang;

impl CppLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for CppLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["cpp", "cc", "cxx", "hpp", "hh", "hxx"]
    }

    fn name(&self) -> &'static str {
        "cpp"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_cpp::LANGUAGE.into())
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
        CppLang::new().extract(src, Path::new("test.cpp"))
    }

    #[test]
    fn classe_methodes_et_champs() {
        let src = r#"
class Point {
public:
    double get() const { return x_; }
    void set(double v);
private:
    double x_;
};
"#;
        let ex = extract_src(src);
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Point" && d.kind == NodeKind::Class)
        );
        // Méthode inline ET prototype → Method, parent_class = classe.
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(methods.len(), 2);
        for m in &methods {
            assert_eq!(m.parent_class.as_deref(), Some("Point"));
            assert!(m.signature.is_some());
        }
        let names: Vec<_> = methods.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"get"));
        assert!(names.contains(&"set"));
        // Champ de données privé.
        assert!(ex.definitions.iter().any(|d| {
            d.name == "x_"
                && d.kind == NodeKind::Field
                && d.parent_class.as_deref() == Some("Point")
        }));
    }

    #[test]
    fn struct_cpp_avec_methodes() {
        let src = r#"
struct Vec2 {
    int len() const { return 2; }
    void clear();
    int a;
};
"#;
        let ex = extract_src(src);
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Vec2" && d.kind == NodeKind::Struct)
        );
        let methods: Vec<_> = ex
            .definitions
            .iter()
            .filter(|d| d.kind == NodeKind::Method)
            .collect();
        assert_eq!(methods.len(), 2); // len (inline) + clear (prototype)
        assert!(
            methods
                .iter()
                .all(|m| m.parent_class.as_deref() == Some("Vec2"))
        );
        assert!(ex.definitions.iter().any(|d| {
            d.name == "a" && d.kind == NodeKind::Field && d.parent_class.as_deref() == Some("Vec2")
        }));
    }

    #[test]
    fn namespace_et_template() {
        let src = r#"
namespace geom {
template <typename T>
class Box {
public:
    T get() const { return v_; }
private:
    T v_;
};
}
"#;
        let ex = extract_src(src);
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "Box" && d.kind == NodeKind::Class)
        );
        assert!(ex.definitions.iter().any(|d| {
            d.name == "get"
                && d.kind == NodeKind::Method
                && d.parent_class.as_deref() == Some("Box")
        }));
        assert!(ex.definitions.iter().any(|d| {
            d.name == "v_" && d.kind == NodeKind::Field && d.parent_class.as_deref() == Some("Box")
        }));
        // Pas de nœud Module pour le namespace (FQN géré par le pipeline).
        assert!(
            !ex.definitions
                .iter()
                .any(|d| d.kind == NodeKind::Module || d.name == "geom")
        );
    }

    #[test]
    fn enum_typedef_macros_et_includes() {
        let src = r#"
#include <vector>
#define N 3
enum Status { OK, FAIL };
typedef unsigned long ulong_t;
typedef struct { int q; } Data;
"#;
        let ex = extract_src(src);
        let names: Vec<_> = ex
            .definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label()))
            .collect();
        assert!(names.contains(&("N", "Macro")));
        assert!(names.contains(&("Status", "Enum")));
        assert!(names.contains(&("ulong_t", "Type")));
        assert!(names.contains(&("Data", "Struct")));
        assert!(ex.definitions.iter().any(|d| {
            d.name == "q" && d.kind == NodeKind::Field && d.parent_class.as_deref() == Some("Data")
        }));
        assert!(ex.imports.iter().any(|i| i.module == "vector"));
    }

    #[test]
    fn appels_qualifies() {
        let src = r#"
#include <vector>
namespace util { int twice(int x) { return x * 2; } }
void fill(std::vector<int>& v) {
    v.push_back(1);
    int y = util::twice(3);
}
"#;
        let ex = extract_src(src);
        // `v.push_back(...)` → méthode qualifiée par l'objet.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "push_back" && c.qualifier.as_deref() == Some("v"))
        );
        // `util::twice(...)` → qualifié par le namespace.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "twice" && c.qualifier.as_deref() == Some("util"))
        );
    }

    #[test]
    fn classe_imbriquee_pointeur_fonction_destructeur() {
        let src = r#"
class A {
public:
    void (*cb)(int);
    ~A();
    class B { int z; void m(); };
    static int count;
};
"#;
        let ex = extract_src(src);
        // A et B présents.
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "A" && d.kind == NodeKind::Class)
        );
        assert!(
            ex.definitions
                .iter()
                .any(|d| d.name == "B" && d.kind == NodeKind::Class)
        );
        // Méthode du type imbriqué rattachée à B.
        assert!(ex.definitions.iter().any(|d| {
            d.name == "m" && d.kind == NodeKind::Method && d.parent_class.as_deref() == Some("B")
        }));
        assert!(ex.definitions.iter().any(|d| {
            d.name == "z" && d.kind == NodeKind::Field && d.parent_class.as_deref() == Some("B")
        }));
        // Pointeur de fonction membre → Field (pas Method).
        assert!(ex.definitions.iter().any(|d| {
            d.name == "cb" && d.kind == NodeKind::Field && d.parent_class.as_deref() == Some("A")
        }));
        assert!(
            !ex.definitions
                .iter()
                .any(|d| d.name == "cb" && d.kind == NodeKind::Method)
        );
        // Membre statique → Field, destructeur non émis.
        assert!(ex.definitions.iter().any(|d| {
            d.name == "count" && d.kind == NodeKind::Field && d.parent_class.as_deref() == Some("A")
        }));
        assert!(!ex.definitions.iter().any(|d| d.name.contains('~')));
    }
}
