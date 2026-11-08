//! `ORDER BY` sur une propriété déjà projetée, après une agrégation.
//!
//! `RETURN d.g, count(d) ORDER BY d.g` échouait en « Internal error: Variable 'd' not
//! found for ORDER BY property projection » : le planificateur cherchait à re-projeter
//! `d.g` depuis la variable `d`, qui n'existe plus après l'agrégation, alors que la
//! colonne `d.g` figurait déjà en sortie.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..10 {
        s.execute(&format!("INSERT (:D {{i: {i}, g: {}}})", i % 5))
            .unwrap();
    }
    db
}

fn entiers(db: &ObrainDB, q: &str, col: usize) -> Vec<i64> {
    db.session()
        .execute(q)
        .unwrap_or_else(|e| panic!("{q} : {e}"))
        .rows
        .iter()
        .map(|l| match &l[col] {
            Value::Int64(n) => *n,
            autre => panic!("entier attendu, obtenu {autre:?}"),
        })
        .collect()
}

#[test]
fn order_by_sur_la_propriete_deja_projetee_apres_un_agregat() {
    let db = base();
    assert_eq!(
        entiers(&db, "MATCH (d:D) RETURN d.g, count(d) ORDER BY d.g", 0),
        vec![0, 1, 2, 3, 4]
    );
}

#[test]
fn order_by_descendant_apres_un_agregat() {
    let db = base();
    assert_eq!(
        entiers(&db, "MATCH (d:D) RETURN d.g, count(d) ORDER BY d.g DESC", 0),
        vec![4, 3, 2, 1, 0]
    );
}

#[test]
fn les_formes_voisines_continuent_de_marcher() {
    let db = base();
    assert_eq!(
        entiers(
            &db,
            "MATCH (d:D) RETURN d.g AS g, count(d) AS n ORDER BY g",
            0
        ),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        entiers(
            &db,
            "MATCH (d:D) RETURN d.g, count(d) AS n ORDER BY n, d.g",
            1
        ),
        vec![2, 2, 2, 2, 2]
    );
    assert_eq!(
        entiers(&db, "MATCH (d:D) RETURN d.i ORDER BY d.i DESC LIMIT 3", 0),
        vec![9, 8, 7]
    );
    // la propriété à trier n'est PAS dans le RETURN : la variable existe encore
    assert_eq!(
        entiers(&db, "MATCH (d:D) RETURN d.i ORDER BY d.g, d.i LIMIT 2", 0),
        vec![0, 5]
    );
}
