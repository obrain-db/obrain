//! Agrégat **dans une expression** (obrain-db/obrain#195, famille 1).
//!
//! `[x IN collect(DISTINCT t.id) + collect(DISTINCT t.status) WHERE x IS NOT NULL]`
//! échouait avec « fonction inconnue : collect ». La cause n'était pas l'évaluateur
//! mais le traducteur : `extract_wrapped_aggregate` n'extrayait qu'**un** agrégat
//! par expression, laissant les autres dans l'arbre, et n'avait aucun bras pour la
//! compréhension de liste. La passe de hissage (`hoist_aggregates`) remplace
//! maintenant chaque agrégat par une variable avant la traduction.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test aggregate_in_expression_195
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Trois `:Task`, chacune avec un statut et `n = 1`.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:Task {id: 't1', status: 'todo', n: 1}), \
                    (b:Task {id: 't2', status: 'done', n: 1}), \
                    (c:Task {id: 't3', status: 'todo', n: 1})",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

fn entier(db: &ObrainDB, requete: &str) -> i64 {
    let l = lignes(db, requete);
    assert_eq!(l.len(), 1, "une agrégation globale rend une seule ligne");
    match &l[0][0] {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, reçu {autre:?}"),
    }
}

/// La requête minimale du ticket : deux `collect` dans une compréhension de liste.
#[test]
fn deux_collect_dans_une_comprehension() {
    let db = base();
    let n = entier(
        &db,
        "MATCH (t:Task) RETURN size([x IN collect(t.id) + collect(t.status) \
         WHERE x IS NOT NULL]) AS n",
    );
    assert_eq!(n, 6, "3 identifiants + 3 statuts");
}

/// Sans compréhension : le défaut était déjà là, `collect(a) + collect(b)`.
#[test]
fn deux_collect_dans_une_addition() {
    let db = base();
    let n = entier(
        &db,
        "MATCH (t:Task) RETURN size(collect(t.id) + collect(t.status)) AS n",
    );
    assert_eq!(n, 6);
}

/// `sum` dans une expression, même mécanisme que `collect`.
#[test]
fn sum_et_count_dans_une_addition() {
    let db = base();
    let n = entier(&db, "MATCH (t:Task) RETURN sum(t.n) + count(t) AS n");
    assert_eq!(n, 6, "sum = 3, count = 3");
}

/// Un agrégat dans un `CASE` : la branche n'était cherchée qu'en premier.
#[test]
fn agregat_dans_un_case() {
    let db = base();
    let l = lignes(
        &db,
        "MATCH (t:Task) RETURN CASE WHEN count(t) > 2 THEN 'plus' ELSE 'moins' END AS x",
    );
    assert_eq!(l.len(), 1);
    match &l[0][0] {
        Value::String(s) => assert_eq!(s.to_string(), "plus"),
        autre => panic!("chaîne attendue, reçu {autre:?}"),
    }
}

/// Non-régression : un agrégat enveloppé par une fonction scalaire, qui marchait.
#[test]
fn un_seul_agregat_enveloppe_ne_regresse_pas() {
    let db = base();
    let n = entier(&db, "MATCH (t:Task) RETURN size(collect(t.id)) AS n");
    assert_eq!(n, 3);
}

/// Sonde : la concaténation de deux listes par `+`, sans aucun agrégat.
#[test]
fn concatenation_de_listes_nue() {
    let db = base();
    let n = entier(&db, "RETURN size([1, 2] + [3]) AS n");
    assert_eq!(n, 3, "liste + liste");
}
