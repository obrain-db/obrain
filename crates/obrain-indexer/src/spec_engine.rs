//! Framework d'extraction GÉNÉRIQUE piloté par spec (RFC M6 étape 2).
//!
//! Un seul extracteur marche n'importe quel arbre tree-sitter en utilisant la
//! [`crate::lang_specs::LangSpec`] du langage : les types de nœuds qui
//! définissent des fonctions/classes/champs/etc. sont déclaratifs, plus aucun
//! extracteur manuel par langage.
//!
//! Ce module réplique le pattern de l'indexeur C (CBMLangSpec + extract_unified) :
//! - champ `name` d'abord pour le nom d'un symbole ;
//! - résolveurs de nom spéciaux par langage quand le nom n'est pas dans un
//!   champ standard ;
//! - les fonctions DANS une classe deviennent des Method (parent_class).

use tree_sitter::{Node, Parser};

use crate::lang_specs::LangSpec;
use crate::parse::{Call, Definition, FileExtraction, Import};
use crate::schema::NodeKind;

/// Extraie un fichier avec le framework générique.
///
/// `grammar` : la grammaire tree-sitter du langage (LanguageFn).
/// `spec` : la spec déclarant les types de nœuds.
pub fn extract_spec(source: &str, parser: &mut Parser, spec: &LangSpec) -> FileExtraction {
    let mut out = FileExtraction::default();
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return out,
    };
    walk_node(source, tree.root_node(), spec, None, &mut out);
    out
}

/// Marche récursivement l'arbre.
///
/// `in_class` : Some(nom) quand on est DANS le corps d'une classe/struct —
/// les fonctions y deviennent des Methods rattachées à cette classe.
fn walk_node<'a>(
    source: &str,
    node: Node<'a>,
    spec: &LangSpec,
    in_class: Option<&str>,
    out: &mut FileExtraction,
) {
    let kind = node.kind();
    let is_function = spec.function.contains(&kind);
    let is_class = spec.class.contains(&kind);
    let is_field = spec.field.contains(&kind);
    let is_variable = spec.variable.contains(&kind);

    // 1. Définition de fonction/méthode.
    if is_function && !is_class {
        let name = resolve_name(source, node, spec);
        if !name.is_empty() {
            let kind_def = if spec.name == "clojure" {
                clojure_form_kind(source, node)
            } else {
                match in_class {
                    Some(_) => NodeKind::Method,
                    None => NodeKind::Function,
                }
            };
            out.definitions.push(Definition {
                name: name.clone(),
                kind: kind_def,
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                signature: Some(text(source, node).to_string()),
                parent_class: in_class.map(|s| s.to_string()),
                metrics: function_metrics(node, spec),
                docstring: crate::docs::docstring_of(node, source),
            });
        }
    }

    // 2. Définition de classe/struct/interface → descend pour les méthodes.
    if is_class {
        let class_name = resolve_name(source, node, spec);
        if !class_name.is_empty() {
            out.definitions.push(Definition {
                name: class_name.clone(),
                kind: class_kind(spec, kind, &class_name),
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                signature: None,
                parent_class: in_class.map(|s| s.to_string()),
                metrics: None,
                docstring: crate::docs::docstring_of(node, source),
            });
        }
        // Descend : les fonctions dans le corps deviennent des Methods.
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            walk_node(source, child, spec, Some(&class_name), out);
        }
        return;
    }

    // 3. Champ → Field (rattaché à la classe courante). Quand un nœud est
    // field ET variable (ex: swift property_declaration), on évite le double
    // comptage : Field si dans une classe, Variable sinon.
    if is_field {
        let name = resolve_name(source, node, spec);
        if !name.is_empty() {
            out.definitions.push(Definition {
                name,
                kind: NodeKind::Field,
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                signature: None,
                parent_class: in_class.map(|s| s.to_string()),
                metrics: None,
                docstring: crate::docs::docstring_of(node, source),
            });
        }
        if is_variable {
            // déjà traité comme Field → ne pas repasser en Variable
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                walk_node(source, child, spec, in_class, out);
            }
            return;
        }
    }

    // 3bis. Variable (ex: pair JSON/TOML, const) → Variable.
    if is_variable {
        let name = variable_name(source, node, spec);
        if !name.is_empty() {
            out.definitions.push(Definition {
                name,
                kind: NodeKind::Variable,
                start_line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                signature: None,
                parent_class: in_class.map(|s| s.to_string()),
                metrics: None,
                docstring: crate::docs::docstring_of(node, source),
            });
        }
    }

    // 4. Descendre (les définitions sont souvent imbriquées : document →
    //    object → pair ; source_file → déclarations).
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        // Dans une fonction/class, on descend SAUF si l'enfant est lui-même
        // une définition (éviter les doubles comptes : une fn dans une fn
        // est rare et le C ne la compte pas comme top-level).
        walk_node(source, child, spec, in_class, out);
    }
}

/// Nom d'une variable/pair : la CLÉ (premier enfant nommable, en descendant
/// — ex: YAML block_mapping_pair → flow_node → plain_scalar → string_scalar).
/// Kinds qui sont des TYPES (pas des noms de variable) — à sauter quand on
/// cherche le nom d'une déclaration (ex: fortran `integer :: a` → "a").
const TYPE_KINDS: &[&str] = &[
    "intrinsic_type",
    "type",
    "type_identifier",
    "primitive_type",
    "declaration_type",
    "type_specifier",
    "type_descriptor",
    "type_annotation",
    "return_type",
    "sized_type_specifier",
    "storage_class_specifier",
    "attribute_specifier",
    "type_qualifier",
];

fn variable_name<'a>(source: &str, node: Node<'a>, spec: &LangSpec) -> String {
    // Cas spéciaux d'abord (cobol data_description → entry_name, ...).
    if let Some(n) = special_name(source, node, spec) {
        return n;
    }
    // Le premier enfant est la clé (le 2e est la valeur) pour les paires
    // (json/yaml/toml/ini). Pour les déclarations typées (fortran: `integer
    // :: a`), on saute le type pour prendre le déclarateur.
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if TYPE_KINDS.contains(&child.kind()) {
            continue;
        }
        let r = key_scalar(source, child);
        if !r.is_empty() {
            return r;
        }
    }
    String::new()
}

/// Descend vers le premier scalaire/clé d'un nœud (la clé d'une paire).
fn key_scalar<'a>(source: &str, node: Node<'a>) -> String {
    match node.kind() {
        // Scalaires feuilles.
        "string"
        | "string_literal"
        | "string_scalar"
        | "plain_scalar"
        | "number"
        | "number_literal"
        | "integer"
        | "float"
        | "true"
        | "false"
        | "null"
        | "boolean"
        | "identifier"
        | "property_identifier"
        | "field_identifier"
        | "key"
        | "constant"
        | "name"
        | "variable_name"
        | "bare_key"
        | "quoted_key"
        | "dotted_key"
        | "section_name"
        | "setting_name"
        | "property_name"
        | "text" => {
            let t = text(source, node);
            t.trim_matches('"').trim_matches('\'').to_string()
        }
        _ => {
            // Descend dans le premier enfant nommable.
            let mut cur = node.walk();
            for child in node.children(&mut cur) {
                let r = key_scalar(source, child);
                if !r.is_empty() {
                    return r;
                }
            }
            String::new()
        }
    }
}

/// Kind de nœud pour une classe selon la spec (Class par défaut ; Struct si
/// le nom du langage suggère, Interface si le type de nœud le suggère).
/// Kind d'une forme clojure selon son mot-clé : (ns ...) → Module,
/// (defrecord/defstruct ...) → Struct, (defn/defmulti/...) → Function.
fn clojure_form_kind<'a>(source: &str, node: Node<'a>) -> NodeKind {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        let t = text(source, child).trim();
        if child.kind() == "sym_lit" || child.kind() == "sym_name" {
            if t.starts_with("ns") || t == "in-ns" {
                return NodeKind::Module;
            }
            if t.starts_with("defrecord") || t.starts_with("defstruct") || t.starts_with("deftype")
            {
                return NodeKind::Struct;
            }
            return NodeKind::Function;
        }
    }
    NodeKind::Function
}

fn class_kind(spec: &LangSpec, kind: &str, _name: &str) -> NodeKind {
    // Le type de nœud de la grammaire indique souvent le label exact :
    // struct/record → Struct, interface → Interface, enum → Enum,
    // table → Table (sql), etc. Class en défaut (équivalence C).
    let k = kind;
    if k.contains("struct") || k.contains("record") || k.contains("defrecord") {
        return NodeKind::Struct;
    }
    if k.contains("interface") {
        return NodeKind::Interface;
    }
    if k.contains("enum") {
        return NodeKind::Enum;
    }
    if k.contains("table") || k.contains("materialized_view") {
        return NodeKind::Table;
    }
    if k.contains("type_alias") || k.contains("typedef") {
        return NodeKind::Type;
    }
    let _ = spec;
    NodeKind::Class
}

/// Calcule les métriques structurelles d'un nœud de fonction/méthode
/// (complexity, cognitive, loop_*). Retourne None si la spec n'a pas de
/// types de branchement (ex: clojure list_lit — pas de corps réel) ou si
/// le nœud n'est pas un corps de fonction typé.
fn function_metrics<'a>(node: Node<'a>, spec: &LangSpec) -> Option<crate::metrics::Metrics> {
    if spec.branch.is_empty() {
        return None;
    }
    // Les formes Lisp (clojure list_lit) ou data (json pair) n'ont pas de
    // corps — les branch types ne s'y appliquent pas de façon signifiante.
    Some(crate::metrics::compute(node, spec.branch))
}

/// Résout le nom d'un symbole de façon générique.
///
/// Ordre des heuristiques (pattern du C) :
/// 1. champ `name` ;
/// 2. champ `declarator` → descendre vers l'identifier ;
/// 3. premier descendant identifier/type_identifier ;
/// 4. résolveurs spéciaux par langage (ajoutés au besoin).
fn resolve_name<'a>(source: &str, node: Node<'a>, spec: &LangSpec) -> String {
    // Cas spéciaux par langage d'abord.
    if let Some(n) = special_name(source, node, spec) {
        return n;
    }
    // 1. champ name.
    if let Some(n) = node.child_by_field_name("name") {
        let t = text(source, n);
        if is_name_like(n.kind()) && !t.is_empty() {
            return t.to_string();
        }
    }
    // 2. champ declarator.
    if let Some(id) = node
        .child_by_field_name("declarator")
        .and_then(find_identifier)
    {
        return text(source, id).to_string();
    }
    // 3. premier identifier descendant direct.
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if is_name_like(child.kind()) {
            let t = text(source, child);
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    // 4. dernier identifier (certains langages mettent le nom à la fin).
    let mut last = String::new();
    let mut cur2 = node.walk();
    for child in node.children(&mut cur2) {
        if is_name_like(child.kind()) {
            last = text(source, child).to_string();
        }
    }
    last
}

/// Résolveurs de nom spéciaux par langage (quand le champ name est absent).
fn special_name<'a>(source: &str, node: Node<'a>, spec: &LangSpec) -> Option<String> {
    let lang = spec.name;
    let kind = node.kind();
    // Protobuf : `rpc Foo(...)` → nom dans rpc_name.
    if lang == "proto" && kind == "rpc" {
        return find_child_kind(node, "rpc_name").map(|n| text(source, n).to_string());
    }
    // SQL : CREATE TABLE users / CREATE FUNCTION add_one — le nom est dans
    // le premier `object_reference` (create_table/create_function sont les
    // enfants des statements). Le framework appelle special_name sur le nœud
    // class/function lui-même.
    if lang == "sql"
        && (kind == "create_table"
            || kind == "create_view"
            || kind == "create_materialized_view"
            || kind == "create_function"
            || kind == "create_procedure"
            || kind == "custom_type"
            || kind == "function_declaration")
    {
        return find_child_kind(node, "object_reference")
            .and_then(find_first_identifier)
            .map(|id| text(source, id).trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                // fallback : ancien comportement (name direct).
                find_child_kind(node, "name").map(|n| text(source, n).to_string())
            });
    }
    // Clojure : (defn helper ...) / (defrecord Point ...) / (defmulti area)
    // — la forme est un list_lit dont le 1er enfant sym_lit est le mot-clé
    // et le 2e sym_lit est le NOM. Le C a un résolveur dédié équivalent.
    if lang == "clojure" && kind == "list_lit" {
        let mut cur = node.walk();
        let children: Vec<Node> = node.children(&mut cur).collect();
        // Le 1er enfant est la parenthèse (opening) ; le 1er sym_lit est le
        // mot-clé (defn/ns/...) et le suivant est le NOM (helper/Point/...).
        let mut i = 0;
        while i < children.len() {
            let k = children[i].kind();
            if k == "sym_lit" || k == "sym_name" || k == "keyword" || k == "symbol" {
                let ktext = text(source, children[i]).trim();
                let est_def = ktext.starts_with("def")
                    || ktext == "ns"
                    || ktext == "fn"
                    || ktext.starts_with("defn")
                    || ktext == "defmacro";
                if est_def {
                    for c in &children[i + 1..] {
                        if c.kind() == "sym_lit" || c.kind() == "sym_name" {
                            let t = text(source, *c).trim();
                            if !t.is_empty() && !t.starts_with('.') {
                                return Some(t.to_string());
                            }
                        }
                    }
                }
                break;
            }
            i += 1;
        }
    }
    // Cobol : PROGRAM-ID. HELLO → le programme est program_definition, le
    // nom est le descendant program_name ; les data_description (01 WS-NUM)
    // ont leur nom dans entry_name.
    if lang == "cobol" {
        let target = if kind == "program_definition" {
            Some("program_name")
        } else if kind == "data_description" {
            Some("entry_name")
        } else {
            None
        };
        if let Some(tk) = target {
            // descendant (profondeur d'abord)
            if let Some(name) = find_descendant_kind(node, tk) {
                let t = text(source, name).trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    // SCSS : une declaration (spec var) est une Variable seulement si son
    // nom commence par `$` (les propriétés CSS `color: red` ne sont pas des
    // variables). Retourne le nom pour `$x`, "" sinon.
    if lang == "scss" && kind == "declaration" {
        return find_child_kind(node, "property_name").map(|pn| {
            let t = text(source, pn).trim();
            if t.starts_with('$') {
                t.to_string()
            } else {
                String::new() // propriété CSS → pas une variable
            }
        });
    }
    // Erlang : function_clause → le nom est le premier enfant atom (le nom
    // de fonction, minuscule ; les Variables erlang commencent par majuscule
    // et sont d'un kind différent).
    if lang == "erlang" && kind == "function_clause" {
        return find_child_kind(node, "atom")
            .map(|a| text(source, a).trim().to_string())
            .filter(|t| !t.is_empty());
    }
    // Swift : property_declaration `var x: Int` → le nom est le premier
    // descendant simple_identifier/identifier (le pattern, AVANT le type).
    if lang == "swift" && kind == "property_declaration" {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if child.kind() == "type_annotation" {
                continue; // ne pas prendre le type
            }
            if let Some(id) = find_first_identifier(child) {
                let t = text(source, id).trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        return None;
    }
    // Dart : declaration `int x;` → sauter le type (`int`) et prendre le
    // premier identifier suivant (le nom du champ). Évite Field:int.
    if lang == "dart" && kind == "declaration" {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if child.kind() == "type" || child.kind() == "type_annotation" {
                continue;
            }
            if let Some(id) = find_first_identifier(child) {
                let t = text(source, id).trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        return None;
    }
    // Markdown : atx_heading `# Titre` → le texte après les `#`.
    if lang == "markdown" && (kind == "atx_heading" || kind == "setext_heading") {
        let t = text(source, node).trim();
        let titre = t.trim_start_matches('#').trim();
        if !titre.is_empty() {
            return Some(titre.to_string());
        }
    }
    // Bash : `helper() { ... }` → le nom est le premier enfant `word`.
    if lang == "bash" && kind == "function_definition" {
        return find_child_kind(node, "word")
            .map(|w| text(source, w).trim().to_string())
            .filter(|t| !t.is_empty());
    }
    // Swift : function_declaration → le premier simple_identifier enfant.
    if lang == "swift" && kind == "function_declaration" {
        return find_child_kind(node, "simple_identifier")
            .map(|n| text(source, n).to_string())
            .filter(|t| !t.is_empty());
    }
    // Julia : function_definition → le premier identifier (dans la signature).
    if lang == "julia" && kind == "function_definition" {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if let Some(id) = find_first_identifier(child) {
                let t = text(source, id);
                if !t.is_empty() && t != "function" {
                    return Some(t.to_string());
                }
            }
        }
        return None;
    }
    // Dart : le nom de classe/fonction est souvent dans un type_identifier ou
    // identifier en tête — resolve_name gère déjà ; fallback sur la spec.
    None
}

fn find_descendant_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        if let Some(found) = find_descendant_kind(c, kind) {
            return Some(found);
        }
    }
    None
}

fn find_first_identifier<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if node.kind() == "identifier"
        || node.kind() == "type_identifier"
        || node.kind() == "simple_identifier"
        || node.kind() == "property_identifier"
        || node.kind() == "field_identifier"
    {
        return Some(node);
    }
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        if let Some(found) = find_first_identifier(c) {
            return Some(found);
        }
    }
    None
}

fn find_child_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cur = node.walk();
    node.children(&mut cur).find(|c| c.kind() == kind)
}

fn find_identifier<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if is_name_like(node.kind()) {
        return Some(node);
    }
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        if let Some(found) = find_identifier(c) {
            return Some(found);
        }
    }
    None
}

/// Types de nœuds qui portent un nom (heuristique générique).
fn is_name_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "name"
            | "method_name"
            | "function_name"
            | "class_name"
            | "type"
            | "constant"
            | "variable_name"
            | "func_name"
            | "subprogram"
            | "bare_key"
            | "quoted_key"
            | "dotted_key"
            | "section_name"
            | "setting_name"
            | "property_name"
            | "text"
            | "message_name"
            | "enum_name"
            | "service_name"
            | "rpc_name"
    )
}

fn text<'a>(source: &'a str, node: Node) -> &'a str {
    &source[node.byte_range()]
}

/// Collecte les appels génériquement : tout nœud dont le kind ∈ spec.call
/// contient une fonction appelée. (Étape ultérieure — les calls précis
/// dépendent de la structure du nœud call par langage.)
#[allow(dead_code)]
fn collect_calls_generic<'a>(source: &str, node: Node<'a>, spec: &LangSpec, out: &mut Vec<Call>) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if spec.call.contains(&child.kind()) {
            // Le nom appelé est souvent dans un sous-champ (function, name,
            // ou un identifier).
            if let Some(name) = child
                .child_by_field_name("function")
                .or_else(|| child.child_by_field_name("name"))
                .or_else(|| find_identifier(child))
            {
                let t = text(source, name);
                if is_name_like(name.kind()) && !t.is_empty() {
                    out.push(Call {
                        name: t.to_string(),
                        qualifier: None,
                        line: child.start_position().row as u32 + 1,
                    });
                }
            }
        }
        if child.child_count() > 0 {
            collect_calls_generic(source, child, spec, out);
        }
    }
}

/// Collecte les imports génériquement : les nœuds spec.import/import_from —
/// le module est le texte du nœud (approximatif, raffiné par langage ensuite).
#[allow(dead_code)]
fn collect_imports_generic<'a>(
    source: &str,
    node: Node<'a>,
    spec: &LangSpec,
    out: &mut Vec<Import>,
) {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if spec.import.contains(&child.kind()) || spec.import_from.contains(&child.kind()) {
            let t = text(source, child).trim().to_string();
            if !t.is_empty() && t.len() < 200 {
                out.push(Import {
                    module: t,
                    local_name: None,
                });
            }
        }
        if child.child_count() > 0 {
            collect_imports_generic(source, child, spec, out);
        }
    }
}

/// Adaptateur spec → trait Language.
///
/// Permet de brancher un langage avec juste : la grammaire tree-sitter + le
/// nom de sa spec dans LANG_SPECS. L'extraction est 100 % générique.
pub struct SpecLang {
    name: &'static str,
    spec_index: usize,
    /// Crée un parser configuré avec la grammaire du langage.
    grammar: fn() -> tree_sitter::Parser,
    /// Nom dans tree-sitter-language-pack (si la grammaire vient du pack,
    /// `grammar` est inutilisé et le parser est construit via get_language).
    pack_name: Option<&'static str>,
    /// Overrides des types de nœuds de la spec — les versions actuelles des
    /// grammaires tree-sitter diffèrent parfois de celles utilisées par
    /// l'indexeur C (ex: dart class_definition → class_declaration).
    override_class: &'static [&'static str],
    override_function: &'static [&'static str],
    override_variable: &'static [&'static str],
}

impl SpecLang {
    /// Construit un langage piloté par spec.
    pub fn new(spec_name: &'static str, grammar: fn() -> tree_sitter::Parser) -> Self {
        let spec_index = crate::lang_specs::LANG_SPECS
            .iter()
            .position(|s| s.name == spec_name)
            .expect("spec inconnue");
        Self {
            name: spec_name,
            spec_index,
            grammar,
            pack_name: None,
            override_class: &[],
            override_function: &[],
            override_variable: &[],
        }
    }

    /// Construit un langage piloté par spec dont la grammaire vient de
    /// tree-sitter-language-pack (371 langages, chargés/téléchargés à la
    /// demande). `pack_lang` : le nom dans le pack (souvent = nom de spec).
    pub fn from_pack(spec_name: &'static str, pack_lang: &'static str) -> Self {
        let spec_index = crate::lang_specs::LANG_SPECS
            .iter()
            .position(|s| s.name == spec_name)
            .expect("spec inconnue");
        Self {
            name: spec_name,
            spec_index,
            grammar: || unreachable!("pack_name fourni"),
            pack_name: Some(pack_lang),
            override_class: &[],
            override_function: &[],
            override_variable: &[],
        }
    }

    /// Remplace les types de nœuds de classe (quand la grammaire actuelle
    /// diffère de celle du C).
    pub fn class_types(mut self, types: &'static [&'static str]) -> Self {
        self.override_class = types;
        self
    }

    /// Remplace les types de nœuds de fonction.
    pub fn function_types(mut self, types: &'static [&'static str]) -> Self {
        self.override_function = types;
        self
    }

    /// Remplace les types de nœuds de variable.
    pub fn variable_types(mut self, types: &'static [&'static str]) -> Self {
        self.override_variable = types;
        self
    }
}

impl crate::parse::Language for SpecLang {
    fn extensions(&self) -> &'static [&'static str] {
        crate::lang_specs::LANG_SPECS[self.spec_index].extensions
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn extract(&self, source: &str, _path: &std::path::Path) -> crate::parse::FileExtraction {
        let spec = &crate::lang_specs::LANG_SPECS[self.spec_index];
        // Applique les overrides (classe/fonction) si présents.
        let spec_ajuste;
        let spec_ref = if !self.override_class.is_empty()
            || !self.override_function.is_empty()
            || !self.override_variable.is_empty()
        {
            spec_ajuste = LangSpec {
                name: spec.name,
                extensions: spec.extensions,
                function: if self.override_function.is_empty() {
                    spec.function
                } else {
                    self.override_function
                },
                class: if self.override_class.is_empty() {
                    spec.class
                } else {
                    self.override_class
                },
                variable: if self.override_variable.is_empty() {
                    spec.variable
                } else {
                    self.override_variable
                },
                field: spec.field,
                module: spec.module,
                call: spec.call,
                import: spec.import,
                import_from: spec.import_from,
                decorator: spec.decorator,
                branch: spec.branch,
            };
            &spec_ajuste
        } else {
            spec
        };
        let mut parser = match self.pack_name {
            Some(pack_lang) => {
                let lang = tree_sitter_language_pack::get_language(pack_lang)
                    .unwrap_or_else(|e| panic!("grammaire pack {pack_lang}: {e}"));
                let mut p = tree_sitter::Parser::new();
                p.set_language(&lang).expect("set_language pack");
                p
            }
            None => (self.grammar)(),
        };
        extract_spec(source, &mut parser, spec_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang_specs::LANG_SPECS;

    fn spec(nom: &str) -> &'static LangSpec {
        LANG_SPECS.iter().find(|s| s.name == nom).unwrap()
    }

    #[test]
    fn spec_rust_par_extension() {
        let rust = spec("rust");
        assert!(rust.function.contains(&"function_item"));
        assert!(rust.extensions.contains(&"rs"));
    }
}

#[cfg(test)]
mod tests_toml {
    use super::*;

    #[test]
    fn toml_extrait_pairs() {
        let spec = crate::lang_specs::LANG_SPECS
            .iter()
            .find(|s| s.name == "toml")
            .unwrap();
        let src = "[package]\nname = \"cargo\"\nversion = \"0.1\"\n";
        let mut p = Parser::new();
        p.set_language(&tree_sitter_toml_ng::LANGUAGE.into())
            .unwrap();
        let ex = extract_spec(src, &mut p, spec);
        let noms: Vec<&str> = ex.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(
            noms.contains(&"package"),
            "table package manquante: {noms:?}"
        );
        assert!(noms.contains(&"name"), "pair name manquant: {noms:?}");
        assert!(noms.contains(&"version"), "pair version manquant: {noms:?}");
    }

    #[test]
    fn markdown_extrait_titres() {
        let spec = crate::lang_specs::LANG_SPECS
            .iter()
            .find(|s| s.name == "markdown")
            .unwrap();
        let src = "# Grand titre\n\n## Section\n\nParagraphe.\n";
        let mut p = Parser::new();
        p.set_language(&tree_sitter_md::LANGUAGE.into()).unwrap();
        let ex = extract_spec(src, &mut p, spec);
        let noms: Vec<&str> = ex.definitions.iter().map(|d| d.name.as_str()).collect();
        eprintln!("markdown déf: {noms:?}");
        assert!(
            noms.iter()
                .any(|n| n.contains("titre") || n.contains("Section")),
            "titres manquants: {noms:?}"
        );
    }
}

#[cfg(test)]
mod tests_clojure {
    use super::*;

    #[test]
    fn clojure_defn_extrait() {
        let spec = crate::lang_specs::LANG_SPECS
            .iter()
            .find(|s| s.name == "clojure")
            .unwrap();
        let src = "(defn helper [a] (+ a 1))\n(defrecord Point [x y])\n(defmulti area :shape)\n";
        let mut p = Parser::new();
        p.set_language(&tree_sitter_language_pack::get_language("clojure").unwrap())
            .unwrap();
        let ex = extract_spec(src, &mut p, spec);
        let noms: Vec<&str> = ex.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(noms.contains(&"helper"), "defn helper: {noms:?}");
        assert!(noms.contains(&"Point"), "defrecord Point: {noms:?}");
        assert!(noms.contains(&"area"), "defmulti area: {noms:?}");
    }
}
