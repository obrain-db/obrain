//! Langage Java — grammaire tree-sitter (RFC M6 §5).
//!
//! Calqué sur le module Rust de référence (`rust.rs`) : parser statique,
//! marche des déclarations (classes/interfaces/enums/records/annotations de
//! niveau fichier, leurs membres — méthodes, constructeurs, champs — et les
//! types imbriqués), extraction des imports (`import a.b.C;` → "a.b.C") et
//! collecte récursive des appels (`method_invocation`, constructions
//! `new Type(...)`).

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

/// Nom du champ `field` d'un nœud, si présent.
fn field_text<'a>(source: &'a str, node: Node, field: &str) -> Option<&'a str> {
    node.child_by_field_name(field).map(|n| text(source, n))
}

/// Construit une `Definition` depuis un nœud (lignes 1-based, sans signature).
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

#[derive(Clone, Default)]
pub struct JavaLang;

impl JavaLang {
    pub fn new() -> Self {
        Self
    }
}

impl Language for JavaLang {
    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn name(&self) -> &'static str {
        "java"
    }

    fn extract(&self, source: &str, _path: &Path) -> FileExtraction {
        let mut out = FileExtraction::default();
        let tree = PARSER.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_java::LANGUAGE.into())
                    .expect("grammaire valide");
                *borrow = Some(p);
            }
            borrow.as_mut().unwrap().parse(source, None)
        });
        let Some(tree) = tree else { return out };
        walk_items(source, tree.root_node(), None, &mut out);
        collect_calls(source, tree.root_node(), &mut out.calls);
        out
    }
}

/// Marche un corps de déclarations (programme, class_body, interface_body…).
///
/// `parent_class` est la classe/interface/enum/record englobante : porté par
/// les types composés, il rattache les membres (Method/Field) à leur parent.
/// Les types imbriqués reçoivent leur propre corps avec leur propre nom.
fn walk_items(source: &str, node: Node, parent_class: Option<&str>, out: &mut FileExtraction) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            // ── Types composés ──────────────────────────────────────────
            "class_declaration" | "record_declaration" => {
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                // record → Class (comme une classe, RFC M6 §5).
                out.definitions
                    .push(definition(name.clone(), NodeKind::Class, child, None));
                walk_body(source, child, &name, out);
            }
            "interface_declaration" => {
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                out.definitions
                    .push(definition(name.clone(), NodeKind::Interface, child, None));
                walk_body(source, child, &name, out);
            }
            "enum_declaration" => {
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                out.definitions
                    .push(definition(name.clone(), NodeKind::Enum, child, None));
                walk_body(source, child, &name, out);
            }
            "annotation_type_declaration" => {
                // `@interface Foo` — ni classe ni interface au sens strict :
                // étiqueté Type (choix documenté).
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                out.definitions
                    .push(definition(name.clone(), NodeKind::Type, child, None));
                walk_body(source, child, &name, out);
            }
            // ── Membres ─────────────────────────────────────────────────
            "method_declaration" => {
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Method,
                        start_line: child.start_position().row as u32 + 1,
                        end_line: child.end_position().row as u32 + 1,
                        signature: Some(text(source, child).to_string()),
                        parent_class: parent_class.map(|s| s.to_string()),
                        metrics: crate::metrics::for_function(child, "java"),
                        docstring: crate::docs::docstring_of(child, source),
                    });
                }
            }
            "constructor_declaration" | "compact_constructor_declaration" => {
                // Constructeur (nom = celui de la classe, déjà porté par le
                // champ name) → Method. Le compact constructor des records
                // a le même rôle.
                let name = field_text(source, child, "name")
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    out.definitions.push(Definition {
                        name,
                        kind: NodeKind::Method,
                        start_line: child.start_position().row as u32 + 1,
                        end_line: child.end_position().row as u32 + 1,
                        signature: Some(text(source, child).to_string()),
                        parent_class: parent_class.map(|s| s.to_string()),
                        metrics: crate::metrics::for_function(child, "java"),
                        docstring: crate::docs::docstring_of(child, source),
                    });
                }
            }
            "field_declaration" => {
                // `int a, b;` — un Field par déclaration (premier
                // déclarateur ; les multi-déclarateurs sont rares en Java).
                let name = child
                    .child_by_field_name("declarator")
                    .and_then(|decl| decl.child_by_field_name("name"))
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                if !name.is_empty() {
                    out.definitions.push(definition(
                        name,
                        NodeKind::Field,
                        child,
                        parent_class.map(|s| s.to_string()),
                    ));
                }
            }
            // ── Imports (niveau fichier uniquement) ─────────────────────
            "import_declaration" => {
                if let Some(module) = import_path(source, child) {
                    out.imports.push(Import {
                        module,
                        local_name: None,
                    });
                }
            }
            // Conteneur des membres d'un enum après le `;` (méthodes,
            // champs…) — même parent que l'enum.
            "enum_body_declarations" => {
                walk_items(source, child, parent_class, out);
            }
            // Récupération d'erreur tree-sitter : un fragment invalide peut
            // contenir des membres (ex. une méthode isolée hors classe) —
            // on les extrait quand même, sans parent si le parseur n'en
            // fournit pas.
            "ERROR" => {
                walk_items(source, child, parent_class, out);
            }
            _ => {}
        }
    }
}

/// Descend dans le corps (`body`) d'un type composé pour en extraire les
/// membres, avec ce type comme `parent_class`. Les membres des types
/// imbriqués sont rattachés au type imbriqué par la récursion de
/// [`walk_items`].
fn walk_body(source: &str, decl: Node, name: &str, out: &mut FileExtraction) {
    if let Some(body) = decl.child_by_field_name("body") {
        walk_items(source, body, Some(name), out);
    }
}

/// Chemin complet d'un import sans `import`, ni `static`, ni `;` :
/// `import java.util.List;` → "java.util.List", `import static a.B.c;` →
/// "a.B.c", `import java.util.*;` → "java.util.*".
fn import_path(source: &str, node: Node) -> Option<String> {
    let raw = text(source, node);
    let body = raw.trim().strip_prefix("import")?.trim_start();
    let body = body
        .strip_prefix("static")
        .map(str::trim_start)
        .unwrap_or(body);
    let path = body.trim_end_matches(';').trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Collecte les appels : `method_invocation` (name = champ `name`,
/// qualifier = champ `object` si présent) et `object_creation_expression`
/// (appel de constructeur — name = type créé, qualifier = "new").
fn collect_calls(source: &str, node: Node, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "method_invocation" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| text(source, n).to_string())
                    .unwrap_or_default();
                let qualifier = child
                    .child_by_field_name("object")
                    .map(|n| text(source, n).to_string());
                if !name.is_empty() {
                    out.push(Call {
                        name,
                        qualifier,
                        line: child.start_position().row as u32 + 1,
                    });
                }
            }
            "object_creation_expression" => {
                // `new Foo(args)` — le type créé peut être qualifié
                // (`a.b.Foo`) ou générique (`Foo<T>`) : on prend le segment.
                let name = child
                    .child_by_field_name("type")
                    .map(|t| type_segment(source, t).to_string())
                    .unwrap_or_default();
                if !name.is_empty() {
                    out.push(Call {
                        name,
                        qualifier: Some("new".to_string()),
                        line: child.start_position().row as u32 + 1,
                    });
                }
            }
            _ => {}
        }
        // Descend pour les appels imbriqués (arguments, lambdas…).
        if child.child_count() > 0 {
            collect_calls(source, child, out);
        }
    }
}

/// Segment final d'un type dans `new a.b.Foo<T>(…)` : "Foo". Traverse
/// `generic_type` (base) et `scoped_type_identifier` (après le dernier `.`).
fn type_segment<'a>(source: &'a str, node: Node) -> &'a str {
    let t = text(source, node);
    match node.kind() {
        "generic_type" => node
            .child(0)
            .map(|base| type_segment(source, base))
            .unwrap_or(t),
        "scoped_type_identifier" => {
            // dernier segment du chemin pointé.
            t.rsplit('.').next().unwrap_or(t)
        }
        _ => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_src(src: &str) -> FileExtraction {
        JavaLang::new().extract(src, Path::new("Test.java"))
    }

    fn defs(ex: &FileExtraction) -> Vec<(&str, &str, Option<&str>)> {
        ex.definitions
            .iter()
            .map(|d| (d.name.as_str(), d.kind.label(), d.parent_class.as_deref()))
            .collect()
    }

    #[test]
    fn classe_methodes_et_champs() {
        let src = r#"
public class Bank {
    private String name;
    private long balance;

    public Bank(String name) {
        this.name = name;
    }

    public void deposit(long amount) {
        this.balance += amount;
    }

    long balance() {
        return this.balance;
    }
}
"#;
        let ex = extract_src(src);
        let defs = defs(&ex);
        assert!(defs.contains(&("Bank", "Class", None)));
        // Méthodes et constructeur rattachés à la classe.
        assert!(defs.contains(&("Bank", "Method", Some("Bank")))); // constructeur
        assert!(defs.contains(&("deposit", "Method", Some("Bank"))));
        assert!(defs.contains(&("balance", "Method", Some("Bank"))));
        // Champs (Field, parent = classe).
        assert!(defs.contains(&("name", "Field", Some("Bank"))));
        assert!(defs.contains(&("balance", "Field", Some("Bank"))));
    }

    #[test]
    fn interface_enum_et_record() {
        // r##…## : le contenu contient `"#` (couleur hexadécimale).
        let src = r##"
public interface Drawable {
    void draw();
}

public enum Color {
    RED, GREEN;

    public String hex() {
        return "#000000";
    }
}

public record Point(int x, int y) {
    public int sum() {
        return x + y;
    }
}
"##;
        let ex = extract_src(src);
        let defs = defs(&ex);
        assert!(defs.contains(&("Drawable", "Interface", None)));
        assert!(defs.contains(&("Color", "Enum", None)));
        assert!(defs.contains(&("Point", "Class", None))); // record → Class
        // Membres d'interface / enum / record rattachés à leur parent.
        assert!(defs.contains(&("draw", "Method", Some("Drawable"))));
        assert!(defs.contains(&("hex", "Method", Some("Color"))));
        assert!(defs.contains(&("sum", "Method", Some("Point"))));
    }

    #[test]
    fn imports() {
        let src = r#"
import java.util.List;
import java.util.Map;
import static java.util.Arrays.asList;
import java.util.*;

public class Impl { }
"#;
        let ex = extract_src(src);
        let mods: Vec<&str> = ex.imports.iter().map(|i| i.module.as_str()).collect();
        assert!(mods.contains(&"java.util.List"));
        assert!(mods.contains(&"java.util.Map"));
        // static retiré, chemin complet conservé.
        assert!(mods.contains(&"java.util.Arrays.asList"));
        // wildcard conservé.
        assert!(mods.contains(&"java.util.*"));
        assert!(ex.imports.iter().all(|i| i.local_name.is_none()));
    }

    #[test]
    fn appels() {
        let src = r#"
import java.util.List;

public class Service {
    private final Db db = new Db();

    public List<String> run() {
        List<String> items = db.query("x");
        items.addAll(new ArrayList<String>());
        notify();
        return items;
    }
}
"#;
        let ex = extract_src(src);
        // Appel qualifié par objet.
        let q = ex
            .calls
            .iter()
            .find(|c| c.name == "query")
            .expect("db.query présent");
        assert_eq!(q.qualifier.as_deref(), Some("db"));
        // Appel qualifié par objet (collection).
        let add_all = ex
            .calls
            .iter()
            .find(|c| c.name == "addAll")
            .expect("items.addAll présent");
        assert_eq!(add_all.qualifier.as_deref(), Some("items"));
        // Appel sans objet.
        assert!(
            ex.calls
                .iter()
                .any(|c| c.name == "notify" && c.qualifier.is_none())
        );
        // Constructeur : name = type créé, qualifier = "new".
        let ctor = ex
            .calls
            .iter()
            .find(|c| c.name == "ArrayList")
            .expect("new ArrayList présent");
        assert_eq!(ctor.qualifier.as_deref(), Some("new"));
    }

    #[test]
    fn methode_hors_classe() {
        let src = r#"
class A { }

void stray() {
    helper();
}
"#;
        // Java n'a pas de méthodes hors classe ; si le parseur en voit une
        // (fichier invalide), elle est quand même extraite en Method sans
        // parent plutôt que perdue.
        let ex = extract_src(src);
        let stray = ex
            .definitions
            .iter()
            .find(|d| d.name == "stray")
            .expect("méthode isolée conservée");
        assert_eq!(stray.kind, NodeKind::Method);
        assert_eq!(stray.parent_class, None);
        assert!(ex.calls.iter().any(|c| c.name == "helper"));
    }
}
