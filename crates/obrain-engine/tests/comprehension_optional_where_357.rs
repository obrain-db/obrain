//! Compréhension de motif dans le `WHERE` d'un `OPTIONAL MATCH`
//! (obrain-db/obrain#357, partie 2 ; PO `persona_10_find_adjacent_personas`).
//!
//! `OPTIONAL MATCH (t)-[:I]-(x:F) WHERE size([(x)-[:I]-(y:F) | y]) <= 1` mourait
//! en « PatternComprehension reached the planner without being rewritten » :
//! `translate_where` sautait la réécriture sous une `LeftJoin`. La compréhension
//! est maintenant relevée sur le côté DROIT, pour que la conjonction soit un
//! filtre de droite : une ligne de gauche sans candidat valide sort avec des
//! NULL, elle ne disparaît jamais.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test comprehension_optional_where_357
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Tous les nœuds sont `F` ; les ancres de gauche sont aussi `T`.
/// - `lone`  : aucun voisin.
/// - `t_hub` : un seul voisin `hub` (degré 3) -> le candidat échoue.
/// - `t_leaf`: un seul voisin `leaf` (degré 1) -> le candidat passe.
/// - `t_mix` : voisins `ok` (degré 1) et `hub2` (degré 3) -> seul `ok` passe.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (:F:T {p: 'lone'}), \
                    (th:F:T {p: 't_hub'})-[:I]->(hub:F {p: 'hub'}), \
                    (hub)-[:I]->(:F {p: 'k1'}), (hub)-[:I]->(:F {p: 'k2'}), \
                    (tl:F:T {p: 't_leaf'})-[:I]->(:F {p: 'leaf'}), \
                    (tm:F:T {p: 't_mix'})-[:I]->(:F {p: 'ok'}), \
                    (tm)-[:I]->(h2:F {p: 'hub2'}), \
                    (h2)-[:I]->(:F {p: 'm1'}), (h2)-[:I]->(:F {p: 'm2'})",
        )
        .unwrap();
    db
}

fn paires(db: &ObrainDB, requete: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = db
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
        .iter()
        .map(|l| (texte(&l[0]), texte(&l[1])))
        .collect();
    rows.sort();
    rows
}

fn texte(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::String(s) => s.to_string(),
        autre => format!("{autre:?}"),
    }
}

fn p(a: &str, b: &str) -> (String, String) {
    (a.into(), b.into())
}

/// Attendu pour `size(voisins de x) <= 1` : (a)(b)(c)(d) de l'issue.
fn attendu_ancre_x() -> Vec<(String, String)> {
    vec![
        p("lone", "NULL"),   // (a) aucun candidat
        p("t_hub", "NULL"),  // (b) le seul candidat échoue : la ligne reste
        p("t_leaf", "leaf"), // (c) candidat valide gardé
        p("t_mix", "ok"),    // (d) l'un passe, l'autre non
    ]
}

#[test]
fn ancre_sur_la_variable_de_l_optional() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE size([(x)-[:I]-(y:F) | y]) <= 1 RETURN t.p, x.p",
    );
    assert_eq!(rows, attendu_ancre_x());
}

/// Formulation sans compréhension : `OPTIONAL MATCH` puis comptage explicite
/// du degré de chaque candidat ; la règle (garder `n <= 1`, sinon une ligne
/// NULL par ancre) est appliquée ici. Même résultat que le `WHERE`.
#[test]
fn equivalent_sans_comprehension() {
    let db = base();
    let rows = db
        .execute_cypher(
            "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
             OPTIONAL MATCH (x)-[:I]-(y:F) \
             RETURN t.p, x.p, count(y) AS n",
        )
        .unwrap()
        .rows;
    let mut attendu: Vec<(String, String)> = Vec::new();
    for ancre in ["lone", "t_hub", "t_leaf", "t_mix"] {
        let gardes: Vec<String> = rows
            .iter()
            .filter(|l| texte(&l[0]) == ancre && matches!(&l[2], Value::Int64(n) if *n <= 1))
            .map(|l| texte(&l[1]))
            .collect();
        if gardes.is_empty() {
            attendu.push(p(ancre, "NULL"));
        } else {
            attendu.extend(gardes.iter().map(|x| p(ancre, x)));
        }
    }
    attendu.sort();
    assert_eq!(attendu, attendu_ancre_x());
    let direct = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE size([(x)-[:I]-(y:F) | y]) <= 1 RETURN t.p, x.p",
    );
    assert_eq!(direct, attendu);
}

/// Ancre sur `t` (variable de gauche, aussi présente à droite).
#[test]
fn ancre_sur_la_variable_partagee() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE size([(t)-[:I]-(y:F) | y]) <= 1 RETURN t.p, x.p",
    );
    // t_mix a deux voisins : tous ses candidats échouent -> NULL.
    assert_eq!(
        rows,
        vec![
            p("lone", "NULL"),
            p("t_hub", "hub"),
            p("t_leaf", "leaf"),
            p("t_mix", "NULL"),
        ]
    );
}

/// Conjonction : la compréhension et un prédicat ordinaire sur `x`.
#[test]
fn conjonction_avec_un_predicat_ordinaire() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE x.p <> 'leaf' AND size([(x)-[:I]-(y:F) | y]) <= 1 RETURN t.p, x.p",
    );
    assert_eq!(
        rows,
        vec![
            p("lone", "NULL"),
            p("t_hub", "NULL"),
            p("t_leaf", "NULL"),
            p("t_mix", "ok"),
        ]
    );
}

/// (e) Le `MATCH` simple ne change pas.
#[test]
fn match_simple_inchange() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T)-[:I]-(x:F) WHERE size([(x)-[:I]-(y:F) | y]) <= 1 RETURN t.p, x.p",
    );
    assert_eq!(rows, vec![p("t_leaf", "leaf"), p("t_mix", "ok")]);
}

/// (e) Un `WHERE` sans compréhension ne change pas.
#[test]
fn where_sans_comprehension_inchange() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) WHERE x.p <> 'ok' RETURN t.p, x.p",
    );
    assert_eq!(
        rows,
        vec![
            p("lone", "NULL"),
            p("t_hub", "hub"),
            p("t_leaf", "leaf"),
            p("t_mix", "hub2"),
        ]
    );
}

/// Ancre liée seulement à gauche : erreur de traduction NOMMÉE, pas X001.
#[test]
fn ancre_seulement_a_gauche_erreur_nommee() {
    let db = base();
    let err = db
        .execute_cypher(
            "MATCH (t:T), (u:T {p: 'lone'}) OPTIONAL MATCH (t)-[:I]-(x:F) \
             WHERE size([(u)-[:I]-(y:F) | y]) <= 1 RETURN t.p, x.p",
        )
        .expect_err("ancre seulement à gauche");
    let msg = err.to_string();
    assert!(msg.contains("'u'"), "{msg}");
    assert!(!msg.contains("X001"), "{msg}");
}

/// `NOT (...)` autour de la compréhension : même résultat que `<= 1`.
#[test]
fn negation() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE NOT (size([(x)-[:I]-(y:F) | y]) > 1) RETURN t.p, x.p",
    );
    assert_eq!(rows, attendu_ancre_x());
}

/// Tranche et indice sur la liste de la compréhension.
#[test]
fn tranche_et_indice() {
    let db = base();
    let tranche = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE size([(x)-[:I]-(y:F) | y.p][0..2]) <= 1 RETURN t.p, x.p",
    );
    assert_eq!(tranche, attendu_ancre_x());
    let indice = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE [(x)-[:I]-(y:F) | y.p][1] IS NULL RETURN t.p, x.p",
    );
    assert_eq!(indice, attendu_ancre_x());
}

/// Deux compréhensions dans UNE conjonction (`OR`) : ancres `x` et `t`.
#[test]
fn deux_comprehensions_dans_un_or() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE size([(x)-[:I]-(y:F) | y]) <= 1 OR size([(t)-[:I]-(z:F) | z]) <= 1 \
         RETURN t.p, x.p",
    );
    assert_eq!(
        rows,
        vec![
            p("lone", "NULL"),
            p("t_hub", "hub"),
            p("t_leaf", "leaf"),
            p("t_mix", "ok"),
        ]
    );
}

/// Compréhension NON corrélée (ancre liée nulle part) sous l'`OPTIONAL MATCH`.
#[test]
fn comprehension_non_correlee() {
    let db = base();
    let rows = paires(
        &db,
        "MATCH (t:T) OPTIONAL MATCH (t)-[:I]-(x:F) \
         WHERE x.p IN [(w:F {p: 'ok'}) | w.p] RETURN t.p, x.p",
    );
    assert_eq!(
        rows,
        vec![
            p("lone", "NULL"),
            p("t_hub", "NULL"),
            p("t_leaf", "NULL"),
            p("t_mix", "ok"),
        ]
    );
}
