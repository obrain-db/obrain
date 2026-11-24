//! #356 — `WITH *, expr AS alias` : toutes les variables en portée, plus les items nommés
//! (openCypher 9). Avant : `syntax error: Expected end of query` à la virgule.
//! Requête PO `ana_11` : `WITH *, reduce(...) / cnt AS bw_var`.

#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:F {v: 1.0}), (:F {v: 3.0})")
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, q: &str) -> Vec<Vec<Value>> {
    db.session().execute_cypher(q).unwrap().rows
}

#[test]
fn item_ajoute_a_la_portee_la_requete_du_ticket() {
    let r = lignes(
        &base(),
        "MATCH (n:F) WITH collect(n.v) AS vs WITH *, size(vs) AS cnt RETURN cnt",
    );
    assert_eq!(r, vec![vec![Value::Int64(2)]]);
}

#[test]
fn les_variables_d_avant_restent_visibles() {
    let r = lignes(
        &base(),
        "MATCH (n:F) WITH collect(n.v) AS vs WITH *, size(vs) AS cnt RETURN size(vs) AS t, cnt",
    );
    assert_eq!(r, vec![vec![Value::Int64(2), Value::Int64(2)]]);
}

#[test]
fn plusieurs_items_et_un_item_qui_lit_le_precedent_de_la_portee() {
    let r = lignes(
        &base(),
        "MATCH (n:F) WITH collect(n.v) AS vs WITH *, size(vs) AS cnt, reduce(s = 0.0, x IN vs | s + x) AS somme RETURN cnt, somme",
    );
    assert_eq!(r, vec![vec![Value::Int64(2), Value::Float64(4.0)]]);
}

#[test]
fn where_apres_les_items() {
    let q = "MATCH (n:F) WITH collect(n.v) AS vs WITH *, size(vs) AS cnt WHERE cnt > 5 RETURN cnt";
    assert!(lignes(&base(), q).is_empty());
    let q = "MATCH (n:F) WITH collect(n.v) AS vs WITH *, size(vs) AS cnt WHERE cnt > 1 RETURN cnt";
    assert_eq!(lignes(&base(), q).len(), 1);
}

#[test]
fn sur_des_noeuds_les_lignes_sont_conservees() {
    let r = lignes(
        &base(),
        "MATCH (n:F) WITH *, n.v * 2 AS d RETURN n.v AS v, d ORDER BY v",
    );
    assert_eq!(
        r,
        vec![
            vec![Value::Float64(1.0), Value::Float64(2.0)],
            vec![Value::Float64(3.0), Value::Float64(6.0)],
        ]
    );
}

#[test]
fn with_etoile_nu_est_inchange() {
    let r = lignes(&base(), "MATCH (n:F) WITH * RETURN count(n) AS c");
    assert_eq!(r, vec![vec![Value::Int64(2)]]);
}

#[test]
fn virgule_sans_item_reste_une_erreur() {
    assert!(
        base()
            .session()
            .execute_cypher("MATCH (n:F) WITH *, RETURN n")
            .is_err()
    );
}
