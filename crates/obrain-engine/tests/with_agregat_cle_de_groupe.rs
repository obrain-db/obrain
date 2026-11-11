//! `WITH` groupé : un agrégat à côté d'une clé de groupement.
//!
//! Formes dont dépend Project Orchestrator (migration Neo4j -> obrain) :
//!
//! ```text
//! MATCH (n:Note) WITH n.project_id AS p, count(*) AS c RETURN p, c
//! MATCH ... WITH f1, f2, count(*) AS co_count WHERE co_count > 1 ...
//! ```
//!
//! Chaque forme est jouée, par un test distinct, en Cypher et en GQL (le langage
//! par défaut) quand GQL connaît la forme, et comparée au `RETURN` groupé
//! équivalent, qui sert de référence.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test with_agregat_cle_de_groupe
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// 6 notes sur 3 projets, 3 commits qui touchent 3 fichiers.
///
/// | projet | notes (genre, poids)        | nombre | somme | moyenne |
/// |--------|-----------------------------|--------|-------|---------|
/// | a      | (x, 1), (x, 3)              | 2      | 4     | 2.0     |
/// | b      | (x, 10), (y, 20), (y, 30)   | 3      | 60    | 20.0    |
/// | c      | (x, 7)                      | 1      | 7     | 7.0     |
///
/// Commits : c1 -> {x, y, z}, c2 -> {x, y}, c3 -> {x}. Paires co-modifiées
/// (chemin1 < chemin2) : (x, y) deux fois, (x, z) une fois, (y, z) une fois.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    for (projet, nom) in [("a", "Alpha"), ("b", "Beta"), ("c", "Gamma")] {
        session.create_node_with_props(
            &["Project"],
            [
                ("id", Value::String(projet.into())),
                ("name", Value::String(nom.into())),
            ],
        );
    }
    for (projet, genre, poids) in [
        ("a", "x", 1),
        ("a", "x", 3),
        ("b", "x", 10),
        ("b", "y", 20),
        ("b", "y", 30),
        ("c", "x", 7),
    ] {
        session.create_node_with_props(
            &["Note"],
            [
                ("project_id", Value::String(projet.into())),
                ("kind", Value::String(genre.into())),
                ("weight", Value::Int64(poids)),
            ],
        );
    }

    let fichier = |chemin: &str| {
        session.create_node_with_props(&["File"], [("path", Value::String(chemin.into()))])
    };
    let (x, y, z) = (fichier("x"), fichier("y"), fichier("z"));
    for (sha, touches) in [("c1", vec![x, y, z]), ("c2", vec![x, y]), ("c3", vec![x])] {
        let commit =
            session.create_node_with_props(&["Commit"], [("sha", Value::String(sha.into()))]);
        for f in touches {
            session.create_edge(commit, f, "TOUCHES");
        }
    }
    db
}

#[derive(Clone, Copy, Debug)]
enum Langage {
    Cypher,
    Gql,
}

fn lignes(db: &ObrainDB, langage: Langage, requete: &str) -> Vec<Vec<Value>> {
    let session = db.session();
    let resultat = match langage {
        Langage::Cypher => session.execute_cypher(requete),
        Langage::Gql => session.execute(requete),
    };
    match resultat {
        Ok(r) => r.rows,
        Err(e) => panic!("[{langage:?}] {requete}\n  -> {e}"),
    }
}

/// Lignes triées par leur représentation : l'ordre des groupes n'est pas garanti
/// sans `ORDER BY`.
fn lignes_triees(db: &ObrainDB, langage: Langage, requete: &str) -> Vec<Vec<Value>> {
    let mut l = lignes(db, langage, requete);
    l.sort_by_key(|ligne| format!("{ligne:?}"));
    l
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

/// Trie les éléments d'une liste (l'ordre d'un `collect` n'est pas garanti).
fn liste_triee(v: &Value) -> Vec<i64> {
    let Value::List(elements) = v else {
        panic!("liste attendue, reçu {v:?}");
    };
    let mut entiers: Vec<i64> = elements
        .iter()
        .map(|e| match e {
            Value::Int64(i) => *i,
            autre => panic!("entier attendu, reçu {autre:?}"),
        })
        .collect();
    entiers.sort_unstable();
    entiers
}

// ============================================================================
// Les quatre agrégats avec une clé `expr AS alias`
// ============================================================================

fn with_count_et_cle_aliasee(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, count(*) AS c RETURN p, c",
    );
    assert_eq!(
        l,
        vec![
            vec![s("a"), Value::Int64(2)],
            vec![s("b"), Value::Int64(3)],
            vec![s("c"), Value::Int64(1)],
        ],
        "{langage:?}"
    );
}

fn with_sum_et_cle_aliasee(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, sum(n.weight) AS total RETURN p, total",
    );
    assert_eq!(
        l,
        vec![
            vec![s("a"), Value::Int64(4)],
            vec![s("b"), Value::Int64(60)],
            vec![s("c"), Value::Int64(7)],
        ],
        "{langage:?}"
    );
}

fn with_avg_et_cle_aliasee(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, avg(n.weight) AS moyenne RETURN p, moyenne",
    );
    assert_eq!(
        l,
        vec![
            vec![s("a"), Value::Float64(2.0)],
            vec![s("b"), Value::Float64(20.0)],
            vec![s("c"), Value::Float64(7.0)],
        ],
        "{langage:?}"
    );
}

fn with_collect_et_cle_aliasee(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, collect(n.weight) AS poids RETURN p, poids",
    );
    let vu: Vec<(Value, Vec<i64>)> = l
        .iter()
        .map(|ligne| (ligne[0].clone(), liste_triee(&ligne[1])))
        .collect();
    assert_eq!(
        vu,
        vec![
            (s("a"), vec![1, 3]),
            (s("b"), vec![10, 20, 30]),
            (s("c"), vec![7]),
        ],
        "{langage:?}"
    );
}

// ============================================================================
// Clés en variables nues
// ============================================================================

fn with_cle_en_variable_nue(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH f, count(*) AS nb \
             RETURN f.path, nb",
    );
    assert_eq!(
        l,
        vec![
            vec![s("x"), Value::Int64(3)],
            vec![s("y"), Value::Int64(2)],
            vec![s("z"), Value::Int64(1)],
        ],
        "{langage:?}"
    );
}

/// Forme réelle de PO : `WITH f1, f2, count(*) AS co_count`.
fn with_deux_cles_en_variables_nues(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (c:Commit)-[:TOUCHES]->(f1:File), (c)-[:TOUCHES]->(f2:File) \
             WHERE f1.path < f2.path \
             WITH f1, f2, count(*) AS co_count \
             RETURN f1.path, f2.path, co_count",
    );
    assert_eq!(
        l,
        vec![
            vec![s("x"), s("y"), Value::Int64(2)],
            vec![s("x"), s("z"), Value::Int64(1)],
            vec![s("y"), s("z"), Value::Int64(1)],
        ],
        "{langage:?}"
    );
}

/// Forme réelle de PO : deux clés et deux agrégats
/// (`WITH a, b, count(*) AS follow_count, avg(delta_ms) AS avg_delta`).
fn with_deux_cles_aliasees_et_deux_agregats(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) \
         WITH n.project_id AS p, n.kind AS k, count(*) AS c, avg(n.weight) AS m \
         RETURN p, k, c, m",
    );
    assert_eq!(
        l,
        vec![
            vec![s("a"), s("x"), Value::Int64(2), Value::Float64(2.0)],
            vec![s("b"), s("x"), Value::Int64(1), Value::Float64(10.0)],
            vec![s("b"), s("y"), Value::Int64(2), Value::Float64(25.0)],
            vec![s("c"), s("x"), Value::Int64(1), Value::Float64(7.0)],
        ],
        "{langage:?}"
    );
}

// ============================================================================
// Ce qui suit le WITH groupé : WHERE, second MATCH, ORDER BY
// ============================================================================

fn with_groupe_suivi_de_where(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, count(*) AS c WHERE c > 1 RETURN p, c",
    );
    assert_eq!(
        l,
        vec![vec![s("a"), Value::Int64(2)], vec![s("b"), Value::Int64(3)]],
        "{langage:?}"
    );
}

fn with_groupe_variables_nues_suivi_de_where(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (c:Commit)-[:TOUCHES]->(f1:File), (c)-[:TOUCHES]->(f2:File) \
             WHERE f1.path < f2.path \
             WITH f1, f2, count(*) AS co_count \
             WHERE co_count > 1 \
             RETURN f1.path, f2.path, co_count",
    );
    assert_eq!(
        l,
        vec![vec![s("x"), s("y"), Value::Int64(2)]],
        "{langage:?}"
    );
}

/// La clé aliasée sert de valeur de jointure dans le `MATCH` qui suit.
fn with_groupe_suivi_d_un_second_match(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, count(*) AS c \
             MATCH (pr:Project) WHERE pr.id = p \
             RETURN pr.name, c",
    );
    assert_eq!(
        l,
        vec![
            vec![s("Alpha"), Value::Int64(2)],
            vec![s("Beta"), Value::Int64(3)],
            vec![s("Gamma"), Value::Int64(1)],
        ],
        "{langage:?}"
    );
}

/// La clé en variable nue reste un noeud : le `MATCH` qui suit en repart.
fn with_groupe_variable_nue_suivi_d_un_second_match(langage: Langage) {
    let db = base();
    let l = lignes_triees(
        &db,
        langage,
        "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH f, count(*) AS nb \
             MATCH (f)<-[:TOUCHES]-(c2:Commit) \
             RETURN f.path, nb, c2.sha",
    );
    assert_eq!(
        l,
        vec![
            vec![s("x"), Value::Int64(3), s("c1")],
            vec![s("x"), Value::Int64(3), s("c2")],
            vec![s("x"), Value::Int64(3), s("c3")],
            vec![s("y"), Value::Int64(2), s("c1")],
            vec![s("y"), Value::Int64(2), s("c2")],
            vec![s("z"), Value::Int64(1), s("c1")],
        ],
        "{langage:?}"
    );
}

/// `ORDER BY` attaché au `WITH` lui-même.
fn with_groupe_suivi_d_order_by(langage: Langage) {
    let db = base();
    let l = lignes(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, count(*) AS c ORDER BY c DESC RETURN p, c",
    );
    assert_eq!(
        l,
        vec![
            vec![s("b"), Value::Int64(3)],
            vec![s("a"), Value::Int64(2)],
            vec![s("c"), Value::Int64(1)],
        ],
        "{langage:?}"
    );
}

/// `ORDER BY` sur le `RETURN` qui suit le `WITH` groupé.
fn with_groupe_puis_return_order_by(langage: Langage) {
    let db = base();
    let l = lignes(
        &db,
        langage,
        "MATCH (n:Note) WITH n.project_id AS p, sum(n.weight) AS total \
             RETURN p, total ORDER BY total DESC",
    );
    assert_eq!(
        l,
        vec![
            vec![s("b"), Value::Int64(60)],
            vec![s("c"), Value::Int64(7)],
            vec![s("a"), Value::Int64(4)],
        ],
        "{langage:?}"
    );
}

// ============================================================================
// Même résultat que le RETURN groupé équivalent
// ============================================================================

fn with_groupe_egale_return_groupe(langage: Langage) {
    let db = base();
    let paires = [
        (
            "MATCH (n:Note) WITH n.project_id AS p, count(*) AS c RETURN p, c",
            "MATCH (n:Note) RETURN n.project_id AS p, count(*) AS c",
        ),
        (
            "MATCH (n:Note) WITH n.project_id AS p, sum(n.weight) AS v RETURN p, v",
            "MATCH (n:Note) RETURN n.project_id AS p, sum(n.weight) AS v",
        ),
        (
            "MATCH (n:Note) WITH n.project_id AS p, avg(n.weight) AS v RETURN p, v",
            "MATCH (n:Note) RETURN n.project_id AS p, avg(n.weight) AS v",
        ),
        (
            "MATCH (c:Commit)-[:TOUCHES]->(f1:File), (c)-[:TOUCHES]->(f2:File) \
             WHERE f1.path < f2.path \
             WITH f1, f2, count(*) AS n RETURN f1.path, f2.path, n",
            "MATCH (c:Commit)-[:TOUCHES]->(f1:File), (c)-[:TOUCHES]->(f2:File) \
             WHERE f1.path < f2.path \
             RETURN f1.path, f2.path, count(*) AS n",
        ),
    ];
    for (avec_with, avec_return) in paires {
        let reference = lignes_triees(&db, langage, avec_return);
        assert!(!reference.is_empty(), "{langage:?} {avec_return}");
        assert_eq!(
            lignes_triees(&db, langage, avec_with),
            reference,
            "{langage:?} {avec_with}"
        );
    }
}

// ============================================================================
// Chaque cas, une fois par langage
// ============================================================================

/// Déclare un module de tests : chaque cas listé y devient un `#[test]` joué
/// dans le langage donné. Un test par langage : un langage rouge ne masque pas
/// l'état de l'autre.
macro_rules! cas_en {
    ($module:ident, $langage:expr, [$($cas:ident),+ $(,)?]) => {
        mod $module {
            $(#[test] fn $cas() { super::$cas($langage); })+
        }
    };
}

cas_en!(
    cypher,
    super::Langage::Cypher,
    [
        with_count_et_cle_aliasee,
        with_sum_et_cle_aliasee,
        with_avg_et_cle_aliasee,
        with_collect_et_cle_aliasee,
        with_cle_en_variable_nue,
        with_deux_cles_en_variables_nues,
        with_deux_cles_aliasees_et_deux_agregats,
        with_groupe_suivi_de_where,
        with_groupe_variables_nues_suivi_de_where,
        with_groupe_suivi_d_un_second_match,
        with_groupe_variable_nue_suivi_d_un_second_match,
        with_groupe_suivi_d_order_by,
        with_groupe_puis_return_order_by,
        with_groupe_egale_return_groupe,
    ]
);

// Trois cas ne sont pas joués en GQL, qui ne connaît pas ces formes (mesuré sur
// main c4b6ada9) : `WITH ... ORDER BY` et `WITH ... MATCH ... WHERE` y sont des
// erreurs de syntaxe (« Expected RETURN, FINISH, or SELECT »), et un `MATCH`
// écrit après un `WITH` y est planifié AVANT l'agrégat, pas après.
cas_en!(
    gql,
    super::Langage::Gql,
    [
        with_count_et_cle_aliasee,
        with_sum_et_cle_aliasee,
        with_avg_et_cle_aliasee,
        with_collect_et_cle_aliasee,
        with_cle_en_variable_nue,
        with_deux_cles_en_variables_nues,
        with_deux_cles_aliasees_et_deux_agregats,
        with_groupe_suivi_de_where,
        with_groupe_variables_nues_suivi_de_where,
        with_groupe_puis_return_order_by,
        with_groupe_egale_return_groupe,
    ]
);
