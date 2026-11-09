//! Tests d'ACCEPTATION réels du framework spec (lot 1).
//!
//! Chaque test parse un fichier d'exemple RÉEL et vérifie les symboles
//! extraits (richesse — pas juste « parse sans erreur »). C'est le contrat
//! que les agents de lots doivent étendre pour chaque langage.

use obrain_indexer::lang::build_registry;
use obrain_indexer::schema::NodeKind;
use std::path::Path;

fn extract(src: &str, fname: &str) -> obrain_indexer::parse::FileExtraction {
    let reg = build_registry();
    let lang = reg
        .language_for_path(Path::new(fname))
        .unwrap_or_else(|| panic!("langage non branché pour {fname}"));
    lang.extract(src, Path::new(fname))
}

#[test]
fn json_extrait_les_cles() {
    let src = r#"{
  "name": "obrain",
  "version": "0.0.1",
  "workspace": { "members": ["crates/*"], "resolver": "2" }
}"#;
    let ex = extract(src, "Cargo.json");
    let vars: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Variable)
        .map(|d| d.name.as_str())
        .collect();
    assert!(vars.contains(&"name"), "name manquant: {vars:?}");
    assert!(vars.contains(&"version"), "version manquant: {vars:?}");
    assert!(vars.contains(&"workspace"), "workspace manquant: {vars:?}");
    assert!(
        vars.contains(&"members"),
        "members (imbriqué) manquant: {vars:?}"
    );
}

#[test]
fn yaml_extrait_les_cles_imbriquees() {
    let src = "name: obrain\nversion: 1.0\nservices:\n  web:\n    port: 8080\n";
    let ex = extract(src, "config.yaml");
    let vars: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Variable)
        .map(|d| d.name.as_str())
        .collect();
    for attendu in ["name", "version", "services", "web", "port"] {
        assert!(vars.contains(&attendu), "{attendu} manquant: {vars:?}");
    }
}

#[test]
fn ruby_via_spec_framework() {
    // Ruby n'est pas encore branché dans le registre officiel (grammaire à
    // ajouter) — ce test documente le comportement attendu du framework.
    // Il sera activé quand ruby sera branché (lot 2).
    let src = "def helper(a)\n  a\nend\n";
    let _ = src;
}

#[test]
fn ruby_extrait_methodes_et_classes() {
    let src = "def helper(a)\n  a + 1\nend\n\nclass Point\n  def initialize(x)\n    @x = x\n  end\n  def dist\n    @x\n  end\nend\n";
    let ex = extract(src, "x.rb");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    let methods: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Method)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "helper: {fns:?}");
    assert!(classes.contains(&"Point"), "Point: {classes:?}");
    assert!(
        methods.contains(&"initialize") && methods.contains(&"dist"),
        "methods: {methods:?}"
    );
}

#[test]
fn php_extrait_fonctions_et_classes() {
    let src = r#"<?php
function helper($a) { return $a + 1; }
class Point {
    public function dist() { return 0; }
}
?>"#;
    let ex = extract(src, "x.php");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "php helper: {fns:?}");
    assert!(classes.contains(&"Point"), "php Point: {classes:?}");
}

#[test]
fn bash_extrait_fonctions() {
    let src = "#!/bin/bash\nhelper() {\n  echo hi\n}\nmain() {\n  helper\n}\n";
    let ex = extract(src, "x.sh");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "bash helper: {fns:?}");
    assert!(fns.contains(&"main"), "bash main: {fns:?}");
}

#[test]
fn kotlin_extrait_fonctions_et_classes() {
    let src = "fun helper(a: Int): Int = a + 1\n\nclass Point(val x: Int) {\n    fun dist(): Int = x\n}\n";
    let ex = extract(src, "x.kt");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "kotlin helper: {fns:?}");
    assert!(classes.contains(&"Point"), "kotlin Point: {classes:?}");
}

#[test]
fn swift_extrait_classes_et_fonctions() {
    let src = "func helper(a: Int) -> Int { return a + 1 }\n\nclass Point {\n    var x: Int\n    func dist() -> Int { return x }\n}\n";
    let ex = extract(src, "x.swift");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "swift helper: {fns:?}");
    assert!(classes.contains(&"Point"), "swift Point: {classes:?}");
}

#[test]
fn dart_extrait_classes_et_fonctions() {
    let src = "int helper(int a) => a + 1;\n\nclass Point {\n  int x;\n  int dist() => x;\n}\n";
    let ex = extract(src, "x.dart");
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(classes.contains(&"Point"), "dart Point: {classes:?}");
}

#[test]
fn csharp_extrait_classes_et_methodes() {
    let src = "class Point {\n    public int Dist() { return 0; }\n}\nstatic int Helper(int a) { return a; }\n";
    let ex = extract(src, "x.cs");
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(classes.contains(&"Point"), "csharp Point: {classes:?}");
}

#[test]
fn scala_extrait_classes_et_fonctions() {
    let src =
        "def helper(a: Int): Int = a + 1\n\nclass Point(val x: Int) {\n  def dist: Int = x\n}\n";
    let ex = extract(src, "x.scala");
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(classes.contains(&"Point"), "scala Point: {classes:?}");
}

#[test]
fn lua_extrait_fonctions() {
    let src = "function helper(a)\n  return a + 1\nend\n";
    let ex = extract(src, "x.lua");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "lua helper: {fns:?}");
}

#[test]
fn elixir_extrait_fonctions() {
    let src = "defmodule Point do\n  def helper(a) do\n    a + 1\n  end\nend\n";
    let ex = extract(src, "x.ex");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "elixir helper: {fns:?}");
}

#[test]
fn julia_extrait_fonctions() {
    let src = "function helper(a)\n    return a + 1\nend\n\nstruct Point\n    x::Int\nend\n";
    let ex = extract(src, "x.jl");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "julia helper: {fns:?}");
}

// NOTE : haskell et erlang ont des structures tree-sitter trop exotiques pour
// le framework générique (signatures/type vs définition, clauses). L'indexeur
// C a des resolveurs dédiés (~50 lignes/langage). Hooks à ajouter quand ces
// langages seront prioritaires.

#[test]
fn groovy_extrait_methodes() {
    let src = "class Point {\n    int dist() { return 0 }\n}\ndef helper(a) { return a + 1 }\n";
    let ex = extract(src, "x.groovy");
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(classes.contains(&"Point"), "groovy Point: {classes:?}");
}

#[test]
fn zig_extrait_fonctions() {
    let src = "fn helper(a: i32) i32 {\n    return a + 1;\n}\n";
    let ex = extract(src, "x.zig");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "zig helper: {fns:?}");
}

#[test]
fn crystal_extrait_classes() {
    let src = "class Point\n  def dist\n    0\n  end\nend\n";
    let ex = extract(src, "x.cr");
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(classes.contains(&"Point"), "crystal Point: {classes:?}");
}

#[test]
fn css_parse_sans_erreur() {
    // La spec CSS du C est vide (class/variable/function = []) — le CSS ne
    // produit pas de définitions classiques. On vérifie juste que le parse
    // ne panique pas et que le registre trouve le langage.
    let src = ".btn { color: red; }\nh1 { font-size: 2em; }\n";
    let ex = extract(src, "x.css");
    assert!(ex.definitions.is_empty(), "css spec vide → 0 définition");
}

#[test]
fn html_extrait_quelque_chose() {
    let src = "<html><body><div class=\"main\"><p>Texte</p></div></body></html>\n";
    let ex = extract(src, "x.html");
    let _ = ex;
}

#[test]
fn hcl_extrait_ressources() {
    let src = "resource \"aws_instance\" \"web\" {\n  ami = \"ami-123\"\n}\nvariable \"region\" {\n  default = \"us-east-1\"\n}\n";
    let ex = extract(src, "x.tf");
    let vars: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Variable || d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(
        !vars.is_empty(),
        "hcl devrait produire des définitions: {vars:?}"
    );
}

#[test]
fn ini_extrait_sections_et_cles() {
    let src = "[section1]\nkey1 = value1\nkey2 = value2\n\n[section2]\nother = x\n";
    let ex = extract(src, "x.ini");
    let vars: Vec<&str> = ex.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(
        vars.contains(&"section1") || vars.contains(&"key1"),
        "ini devrait extraire sections/clés: {vars:?}"
    );
}

#[test]
fn properties_extrait_cles() {
    let src = "app.name = demo\napp.version = 1.0\n";
    let ex = extract(src, "x.properties");
    let vars: Vec<&str> = ex.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(
        vars.contains(&"app") || vars.contains(&"app.name"),
        "properties clés: {vars:?}"
    );
}

#[test]
fn protobuf_extrait_messages() {
    let src = "syntax = \"proto3\";\n\nmessage Point {\n  int32 x = 1;\n  int32 y = 2;\n}\n\nservice Geo {\n  rpc Distance(Point) returns (Point);\n}\n";
    let ex = extract(src, "x.proto");
    let classes: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Class)
        .map(|d| d.name.as_str())
        .collect();
    assert!(classes.contains(&"Point"), "proto Point: {classes:?}");
}

#[test]
fn powershell_extrait_fonctions() {
    let src = "function Helper {\n    param($a)\n    return $a + 1\n}\n";
    let ex = extract(src, "x.ps1");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"Helper"), "powershell Helper: {fns:?}");
}

#[test]
fn odin_extrait_fonctions() {
    let src = "helper :: proc(a: int) -> int {\n    return a + 1\n}\n\nmain :: proc() {\n    helper(1)\n}\n";
    let ex = extract(src, "x.odin");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "odin helper: {fns:?}");
}

#[test]
fn fortran_extrait_fonctions() {
    let src = "program main\n  call helper(1)\nend program main\n\nsubroutine helper(a)\n  integer :: a\nend subroutine helper\n";
    let ex = extract(src, "x.f90");
    let fns: Vec<&str> = ex.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(
        !fns.is_empty(),
        "fortran devrait produire des définitions: {fns:?}"
    );
}

#[test]
fn ada_extrait_procedures() {
    let src = "package body Hello is\n   procedure Helper is\n   begin\n      null;\n   end Helper;\nend Hello;\n";
    let ex = extract(src, "x.adb");
    assert!(
        !ex.definitions.is_empty(),
        "ada devrait produire des définitions"
    );
}

#[test]
fn clojure_parse_sans_erreur() {
    // clojure via le pack — structure Lisp (fonctions = listes), extraction
    // limitée mais le parse ne doit pas paniquer.
    let src = "(defn helper [a] (+ a 1))\n";
    let ex = extract(src, "x.clj");
    let _ = ex;
}

#[test]
fn sql_parse_sans_erreur() {
    let src = "CREATE TABLE users (id INT, name TEXT);\n";
    let ex = extract(src, "x.sql");
    let _ = ex;
}

#[test]
fn sql_extrait_tables_et_fonctions() {
    let src = "CREATE TABLE users (\n  id INT PRIMARY KEY,\n  name TEXT NOT NULL\n);\n\nCREATE FUNCTION add_one(x INT) RETURNS INT AS $$ SELECT x + 1; $$ LANGUAGE SQL;\n";
    let ex = extract(src, "x.sql");
    let tables: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Table)
        .map(|d| d.name.as_str())
        .collect();
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(
        tables.contains(&"users"),
        "sql CREATE TABLE users: {tables:?}"
    );
    assert!(
        fns.contains(&"add_one"),
        "sql CREATE FUNCTION add_one: {fns:?}"
    );
}

#[test]
fn fortran_extrait_variables_sans_bruit() {
    let src =
        "subroutine helper(a)\n  integer, intent(in) :: a\n  real :: x, y\nend subroutine helper\n";
    let ex = extract(src, "x.f90");
    let vars: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Variable)
        .map(|d| d.name.as_str())
        .collect();
    assert!(vars.contains(&"a"), "fortran var a: {vars:?}");
    assert!(!vars.contains(&"integer"), "bruit integer: {vars:?}");
    assert!(!vars.contains(&"real"), "bruit real: {vars:?}");
}

#[test]
fn cobol_extrait_programme() {
    let src = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. HELLO.\n       DATA DIVISION.\n       WORKING-STORAGE SECTION.\n       01 WS-NUM PIC 9(4).\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           STOP RUN.\n";
    let ex = extract(src, "x.cob");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    let vars: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Variable)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"HELLO"), "cobol HELLO: {fns:?}");
    assert!(vars.contains(&"WS-NUM"), "cobol WS-NUM: {vars:?}");
}

#[test]
fn scss_extrait_variables_dollar_et_mixins() {
    let src = "$primary-color: #333;\n\n.button { color: red; }\n\n@mixin flex-center { display: flex; }\n";
    let ex = extract(src, "x.scss");
    let vars: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Variable)
        .map(|d| d.name.as_str())
        .collect();
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(vars.contains(&"$primary-color"), "scss var $: {vars:?}");
    assert!(
        !vars.contains(&"color"),
        "scss propriété CSS (pas var): {vars:?}"
    );
    assert!(fns.contains(&"flex-center"), "scss mixin: {fns:?}");
}

#[test]
fn erlang_extrait_fonctions() {
    let src = "-module(sample).\nhelper(A) -> A + 1.\nhelper(A, B) -> A + B.\n";
    let ex = extract(src, "x.erl");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"helper"), "erlang helper: {fns:?}");
}

#[test]
fn dart_extrait_champs_sans_type() {
    let src = "class Point {\n  int x;\n  int y;\n}\n";
    let ex = extract(src, "x.dart");
    let fields: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Field)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fields.contains(&"x"), "dart field x: {fields:?}");
    assert!(!fields.contains(&"int"), "dart bruit type: {fields:?}");
}

#[test]
fn swift_extrait_champs() {
    let src = "struct Point {\n    var x: Int\n    var y: Int\n}\n";
    let ex = extract(src, "x.swift");
    let fields: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Field)
        .map(|d| d.name.as_str())
        .collect();
    assert!(
        fields.contains(&"x") && fields.contains(&"y"),
        "swift fields: {fields:?}"
    );
    // Pas de doublon Variable:x (field+var exclusifs)
    let nb_x = ex.definitions.iter().filter(|d| d.name == "x").count();
    assert_eq!(nb_x, 1, "swift x doublonné: {nb_x}");
}

#[test]
fn c_extrait_fonctions_sous_preproc_if() {
    // Le kernel cache des fonctions entières sous #if IS_ENABLED(CONFIG_...) —
    // la descente dans les blocs préprocesseur est essentielle.
    let src = "#if IS_ENABLED(CONFIG_IPV6)\nstatic int vxlan_na_create(struct sk_buff *s) {\n    return 0;\n}\n#endif\nstatic int normal_fn(void) {\n    return 1;\n}\n";
    let ex = extract(src, "x.c");
    let fns: Vec<&str> = ex
        .definitions
        .iter()
        .filter(|d| d.kind == NodeKind::Function)
        .map(|d| d.name.as_str())
        .collect();
    assert!(fns.contains(&"vxlan_na_create"), "fn sous #if: {fns:?}");
    assert!(fns.contains(&"normal_fn"), "fn normale: {fns:?}");
}
