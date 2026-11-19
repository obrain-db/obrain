//! Poussée d'un prédicat d'ÉGALITÉ vers l'index (#248) : `MATCH (n) WHERE
//! n.k = <littéral>` sans label ni index DÉCLARÉ.
//!
//! Avant : le planificateur ne demandait que `has_property_index`, faux tant
//! que l'index n'est pas tenu en mémoire ; il choisissait le balayage, et le
//! balayage ne rebâtit jamais l'index (3,9 M de nœuds : 377 ms au p50). Il
//! demande désormais aussi `peut_indexer_propriete` : le substrat compte les
//! demandes par propriété et bâtit à la deuxième.
//!
//! Ces tests jugent la JUSTESSE d'abord — un index qui se trompe rend un
//! résultat faux, pas lent. Chaque requête est rejouée plusieurs fois : le
//! premier tour balaie, les suivants passent par l'index, et TOUS doivent
//! rendre le même ensemble, égal à la sémantique de `=` :
//!   - entier / flottant de même valeur sont égaux ;
//!   - une chaîne n'est PAS le nombre qu'elle épelle ;
//!   - `-0.0 = 0.0` ; NaN n'égale rien ;
//!   - les listes se comparent élément par élément.

#![cfg(feature = "gql")]

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;
use std::sync::Arc;

/// Les `i` des nœuds dont `k` est égal au littéral, triés. La requête est
/// rejouée `tours` fois ; tous les tours doivent concorder.
fn ids(db: &ObrainDB, litteral: &str, tours: usize) -> Vec<i64> {
    let requete = format!("MATCH (n) WHERE n.k = {litteral} RETURN n.i AS i");
    let mut reference: Option<Vec<i64>> = None;
    for tour in 0..tours {
        let r = db.session().execute(&requete).expect(&requete);
        let mut v: Vec<i64> = r
            .rows
            .iter()
            .map(|l| match &l[0] {
                Value::Int64(n) => *n,
                autre => panic!("{requete} : attendu un entier, reçu {autre:?}"),
            })
            .collect();
        v.sort_unstable();
        match &reference {
            None => reference = Some(v),
            Some(premier) => assert_eq!(
                &v, premier,
                "{requete} : le tour {tour} (index) diffère du tour 0 (balayage)"
            ),
        }
    }
    reference.unwrap()
}

fn liste(v: Vec<Value>) -> Value {
    Value::List(Arc::from(v))
}

/// Une base substrat (celle dont `peut_indexer_propriete` répond) : un nœud
/// par cas, `i` = rang du cas, `k` = la valeur à comparer. Aucun index déclaré.
fn base() -> (tempfile::TempDir, ObrainDB) {
    let d = tempfile::tempdir().unwrap();
    let db = ObrainDB::open(d.path().join("base")).unwrap();
    let cas: Vec<Value> = vec![
        Value::Int64(5),                                    // 0
        Value::Float64(5.0),                                // 1
        Value::String("5".into()),                          // 2  la chaîne n'est pas le nombre
        Value::Float64(5.5),                                // 3
        Value::Float64(-0.0),                               // 4
        Value::Float64(0.0),                                // 5
        Value::Int64(0),                                    // 6
        Value::Float64(f64::NAN),                           // 7  n'égale rien
        liste(vec![Value::Int64(1), Value::Int64(2)]),      // 8
        liste(vec![Value::Float64(1.0), Value::Float64(2.0)]), // 9
        liste(vec![Value::String("1".into()), Value::String("2".into())]), // 10
        Value::String("abc".into()),                        // 11
        Value::String("5.0".into()),                        // 12
        Value::Bool(true),                                  // 13
        Value::Int64(1),                                    // 14  `true` n'est pas 1
    ];
    {
        let store = db.graph_store_mut();
        for (i, v) in cas.into_iter().enumerate() {
            let n = store.create_node(&["S"]);
            store.set_node_property(n, "i", Value::Int64(i as i64));
            store.set_node_property(n, "k", v);
        }
    }
    (d, db)
}

#[test]
fn entier_et_flottant_de_meme_valeur_sont_egaux() {
    let (_d, db) = base();
    assert_eq!(ids(&db, "5", 4), vec![0, 1]);
    assert_eq!(ids(&db, "5.0", 4), vec![0, 1]);
    assert_eq!(ids(&db, "5.5", 4), vec![3]);
}

#[test]
fn une_chaine_n_est_pas_le_nombre_qu_elle_epelle() {
    let (_d, db) = base();
    // La chaîne '5' ne trouve QUE la chaîne ; le nombre 5 ne trouve pas '5'.
    assert_eq!(ids(&db, "'5'", 4), vec![2]);
    assert_eq!(ids(&db, "5", 4), vec![0, 1]);
    assert_eq!(ids(&db, "'5.0'", 4), vec![12]);
    assert_eq!(ids(&db, "'abc'", 4), vec![11]);
}

#[test]
fn zero_negatif_egale_zero_et_nan_n_egale_rien() {
    let (_d, db) = base();
    assert_eq!(ids(&db, "0.0", 4), vec![4, 5, 6]);
    assert_eq!(ids(&db, "-0.0", 4), vec![4, 5, 6]);
    assert_eq!(ids(&db, "0", 4), vec![4, 5, 6]);
    // Le nœud 7 porte NaN : aucune sonde ne le rend.
    for sonde in ["0", "0.0", "5", "5.5", "'5'"] {
        assert!(
            !ids(&db, sonde, 3).contains(&7),
            "NaN ne doit égaler aucune sonde ({sonde})"
        );
    }
}

#[test]
fn les_listes_se_comparent_element_par_element() {
    let (_d, db) = base();
    assert_eq!(ids(&db, "[1, 2]", 4), vec![8, 9]);
    assert_eq!(ids(&db, "[1.0, 2.0]", 4), vec![8, 9]);
    assert_eq!(ids(&db, "['1', '2']", 4), vec![10]);
}

#[test]
fn un_booleen_n_est_pas_un_entier() {
    let (_d, db) = base();
    assert_eq!(ids(&db, "1", 4), vec![14]);
    assert_eq!(ids(&db, "true", 4), vec![13]);
}

/// La poussée a bien EU LIEU : après quelques demandes l'index d'égalité de
/// `k` est tenu, et une valeur absente rend vide (jamais « tout »).
#[test]
fn l_index_est_construit_a_la_demande_et_rend_vide_pour_une_valeur_absente() {
    let (_d, db) = base();
    assert!(!db.graph_store().has_property_index("k"));
    for _ in 0..4 {
        assert_eq!(ids(&db, "'inconnu'", 1), Vec::<i64>::new());
    }
    assert!(
        db.graph_store().has_property_index("k"),
        "l'index d'égalité devait être bâti à la demande par le planificateur"
    );
    // Une propriété que personne n'interroge n'est pas indexée pour autant.
    assert!(!db.graph_store().has_property_index("i"));
    let _ = PropertyKey::new("k");
}

/// Un conjonct supplémentaire reste appliqué : l'index ne fait que proposer
/// des candidats.
#[test]
fn un_conjonct_residuel_reste_applique() {
    let (_d, db) = base();
    for _ in 0..4 {
        let r = db
            .session()
            .execute("MATCH (n) WHERE n.k = 5 AND n.i > 0 RETURN n.i AS i")
            .unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0][0], Value::Int64(1));
    }
}
