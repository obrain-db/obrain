//! Deux valeurs ÉGALES au sens du langage (`=`) sont le même tuple pour une
//! contrainte d'unicité, quelle que soit leur représentation :
//! `5` et `5.0`, ou deux chaînes datetime du même instant écrites dans deux
//! fuseaux.
//!
//! La racine est la clé de l'index d'égalité du substrat (`index_value_key`),
//! plus stricte que l'égalité de revérification : un flottant entier n'entre
//! pas dans l'index, deux écritures d'un même instant occupent deux seaux. Elle
//! est corrigée par la clé canonique de #101, que cette branche contient (elle
//! y est fusionnée, pas dupliquée) : sans elle, les tests qui cherchent PAR
//! L'INDEX une valeur écrite sous l'autre représentation sont rouges.

#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

fn base(contrainte: &str) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(contrainte)
        .expect("CREATE CONSTRAINT");
    db
}

const U_A: &str = "CREATE CONSTRAINT u_a FOR (n:T) REQUIRE n.a IS UNIQUE";
const U_AB: &str = "CREATE CONSTRAINT u_ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE";
const MATIN_UTC: &str = "2026-03-01T10:00:00Z";
const MATIN_PARIS: &str = "2026-03-01T11:00:00+01:00";

#[test]
fn entier_puis_flottant_egal_est_un_doublon() {
    let db = base(U_A);
    let s = db.session();
    s.execute("INSERT (:T {a: 5})").expect("5");
    assert!(
        s.execute("INSERT (:T {a: 5.0})").is_err(),
        "5.0 = 5 : doublon"
    );
    s.execute("INSERT (:T {a: 5.5})").expect("5.5 est distinct");
}

#[test]
fn flottant_puis_entier_egal_est_un_doublon() {
    let db = base(U_A);
    let s = db.session();
    s.execute("INSERT (:T {a: 5.0})").expect("5.0");
    assert!(
        s.execute("INSERT (:T {a: 5})").is_err(),
        "5 = 5.0 : doublon"
    );
}

#[test]
fn tuple_composite_entier_contre_flottant() {
    let db = base(U_AB);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 5.0})").expect("(1, 5.0)");
    assert!(
        s.execute("INSERT (:T {a: 1, b: 5})").is_err(),
        "(1, 5) = (1, 5.0) : doublon"
    );
    s.execute("INSERT (:T {a: 2, b: 5})")
        .expect("(2, 5) distinct");
    assert!(
        s.execute("INSERT (:T {a: 2.0, b: 5.0})").is_err(),
        "(2.0, 5.0) = (2, 5) : doublon"
    );
}

#[test]
fn meme_instant_dans_deux_fuseaux_est_un_doublon() {
    let db = base(U_A);
    let s = db.session();
    s.execute(&format!("INSERT (:T {{a: '{MATIN_UTC}'}})"))
        .expect("instant en UTC");
    assert!(
        s.execute(&format!("INSERT (:T {{a: '{MATIN_PARIS}'}})"))
            .is_err(),
        "le même instant écrit à +01:00 : doublon"
    );
    s.execute("INSERT (:T {a: '2026-03-01T11:00:00Z'})")
        .expect("une heure plus tard : distinct");
}

#[test]
fn set_vers_une_valeur_egale_sous_une_autre_representation() {
    let db = base(U_A);
    let s = db.session();
    s.execute("INSERT (:T {a: 5.0})").expect("5.0");
    s.execute("INSERT (:T {a: 6})").expect("6");
    assert!(
        s.execute_cypher("MATCH (n:T {a: 6}) SET n.a = 5").is_err(),
        "SET a = 5 alors que 5.0 existe : doublon"
    );
}

/// La vérification a posteriori ne passe pas par l'index : elle reconnaît
/// ces doublons quel que soit l'état de #101.
#[test]
fn la_verification_a_posteriori_reconnait_les_valeurs_egales() {
    use obrain_common::types::{PropertyKey, Value};
    let db = base(U_A);
    let store = db.graph_store();
    let a = PropertyKey::new("a");
    store.create_node_with_props(&["T"], &[(a.clone(), Value::Int64(5))]);
    store.create_node_with_props(&["T"], &[(a.clone(), Value::Float64(5.0))]);
    store.create_node_with_props(&["T"], &[(a.clone(), Value::from(MATIN_UTC))]);
    store.create_node_with_props(&["T"], &[(a.clone(), Value::from(MATIN_PARIS))]);
    store.create_node_with_props(&["T"], &[(a, Value::Float64(5.5))]);
    let rapport = db.verifier_contraintes();
    assert_eq!(
        rapport.violations.len(),
        2,
        "5/5.0 et les deux écritures du même instant : {:?}",
        rapport.violations
    );
}

/// Les valeurs temporelles TYPÉES ne sont égales qu'à elles-mêmes : deux
/// `Timestamp` égaux sont un doublon ; un `Timestamp`, un `Date` et une chaîne
/// du « même » instant sont trois valeurs distinctes (classes disjointes, `=`
/// ne les égale pas). Le hachage de la vérification a posteriori ne fabrique ni
/// faux doublon ni faux négatif entre elles.
#[test]
fn les_temporels_types_ne_se_confondent_qu_avec_eux_memes() {
    use obrain_common::types::{Date, PropertyKey, Timestamp, Value};
    let db = base(U_A);
    let store = db.graph_store();
    let a = PropertyKey::new("a");
    let ts = Timestamp::from_secs(1_772_000_000);
    store.create_node_with_props(&["T"], &[(a.clone(), Value::Timestamp(ts))]);
    store.create_node_with_props(&["T"], &[(a.clone(), Value::Timestamp(ts))]);
    store.create_node_with_props(
        &["T"],
        &[(
            a.clone(),
            Value::Timestamp(Timestamp::from_secs(1_772_000_001)),
        )],
    );
    store.create_node_with_props(
        &["T"],
        &[(a.clone(), Value::Date(Date::from_ymd(2026, 3, 1).unwrap()))],
    );
    store.create_node_with_props(&["T"], &[(a, Value::from("2026-03-01"))]);
    let rapport = db.verifier_contraintes();
    assert_eq!(
        rapport.violations_totales, 1,
        "seuls les deux Timestamp égaux sont un doublon : {:?}",
        rapport.violations
    );
}
