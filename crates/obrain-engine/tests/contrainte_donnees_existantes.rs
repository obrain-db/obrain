//! `CREATE CONSTRAINT` sur des données qui violent déjà la contrainte est
//! REFUSÉ, en nommant un doublon : une contrainte déclarée sur une base qui la
//! viole serait un mensonge (`SHOW CONSTRAINTS` l'afficherait pendant que la
//! base porte des doublons). C'est aussi le pire cas du « vérifier puis
//! écrire » : le doublon est déjà là, séquentiellement, sans aucune course.
//!
//! La passe est celle de [`ObrainDB::verifier_contraintes`] ; pour la liste
//! COMPLÈTE des doublons d'une contrainte envisagée, sans la déclarer :
//! [`ObrainDB::verifier_contrainte_hypothetique`].

#![cfg(feature = "cypher")]

use obrain_engine::{ObrainDB, Session};

const U_A: &str = "CREATE CONSTRAINT u_a FOR (n:T) REQUIRE n.a IS UNIQUE";
const U_AB: &str = "CREATE CONSTRAINT u_ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE";

/// Nombre de nœuds `:T` qui répondent au motif `{…}` donné.
fn nombre(session: &Session, motif: &str) -> usize {
    session
        .execute(&format!("MATCH (n:T {motif}) RETURN n.a"))
        .expect("MATCH de comptage")
        .rows
        .len()
}

/// `INSERT` est du GQL, le reste (`MATCH … SET`, `MERGE`) du Cypher.
fn lancer(
    session: &Session,
    requete: &str,
) -> obrain_common::utils::error::Result<obrain_engine::database::QueryResult> {
    if requete.trim_start().starts_with("INSERT") {
        session.execute(requete)
    } else {
        session.execute_cypher(requete)
    }
}

fn refuse(session: &Session, requete: &str, pourquoi: &str) {
    let r = lancer(session, requete);
    assert!(r.is_err(), "{pourquoi} — `{requete}` a répondu {r:?}");
    let texte = r.unwrap_err().to_string();
    assert!(
        texte.contains("UNIQUE"),
        "{pourquoi} — l'erreur doit nommer la contrainte d'unicité, reçu : {texte}"
    );
}

fn accepte(session: &Session, requete: &str, pourquoi: &str) {
    let r = lancer(session, requete);
    assert!(
        r.is_ok(),
        "{pourquoi} — `{requete}` a échoué : {:?}",
        r.err()
    );
}

// ─────────────── 3. CREATE CONSTRAINT sur des données en violation ───────────────

#[test]
fn create_constraint_refuse_des_donnees_deja_en_double() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 1, b: 'y'})").unwrap();
    s.execute("INSERT (:T {a: 7, b: 'dup', m: 1})").unwrap();
    s.execute("INSERT (:T {a: 7, b: 'dup', m: 2})").unwrap();

    let e = s
        .execute_cypher(U_AB)
        .expect_err("deux nœuds portent (7,'dup')");
    let texte = e.to_string();
    assert!(
        texte.contains('7') && texte.contains("dup") && texte.contains("both carry"),
        "l'erreur doit NOMMER le doublon (deux nœuds, valeurs du tuple), reçu : {texte}"
    );
    assert!(
        s.execute("SHOW CONSTRAINTS").unwrap().rows.is_empty(),
        "la contrainte refusée n'est pas déclarée"
    );
    // Elle n'est pas en vigueur non plus : un troisième (1,'x') est accepté
    // (puis retiré par son marqueur `m`, sans toucher au premier).
    s.execute("INSERT (:T {a: 1, b: 'x', m: 9})")
        .expect("aucune contrainte en vigueur");
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 2);
    s.execute_cypher("MATCH (n:T {m: 9}) DELETE n").unwrap();
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 1);

    // UNIQUE (a) seul est violé aussi (a = 1 deux fois, a = 7 deux fois).
    assert!(s.execute_cypher(U_A).is_err());
    assert!(s.execute("SHOW CONSTRAINTS").unwrap().rows.is_empty());

    // Une fois le doublon retiré, la contrainte composite passe et tient.
    s.execute_cypher("MATCH (n:T {m: 2}) DELETE n")
        .expect("retrait du doublon");
    accepte(&s, U_AB, "plus aucun doublon de tuple");
    assert_eq!(s.execute("SHOW CONSTRAINTS").unwrap().rows.len(), 1);
    refuse(
        &s,
        "INSERT (:T {a: 7, b: 'dup'})",
        "(7,'dup') est pris par le nœud restant",
    );
}

/// Le diagnostic d'une contrainte ENVISAGÉE liste tous les doublons (là où
/// `CREATE CONSTRAINT` s'arrête au premier), sans rien déclarer.
#[test]
fn diagnostic_d_une_contrainte_hypothetique_liste_tous_les_doublons() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..5 {
        s.execute(&format!("INSERT (:T {{a: 7, b: 'dup', m: {i}}})"))
            .unwrap();
    }
    s.execute("INSERT (:T {a: 1, b: 'seul'})").unwrap();
    let rapport = db.verifier_contrainte_hypothetique("T", &["a", "b"], 100);
    assert_eq!(rapport.violations_totales, 4, "5 porteurs = 4 doublons");
    assert_eq!(rapport.violations.len(), 4);
    let plafonne = db.verifier_contrainte_hypothetique("T", &["a", "b"], 2);
    assert_eq!(plafonne.violations.len(), 2, "décrites : plafonnées");
    assert_eq!(plafonne.violations_totales, 4, "comptées : toutes");
    assert!(s.execute("SHOW CONSTRAINTS").unwrap().rows.is_empty());
}

/// Un type qui hérite d'un autre est soumis à ses contraintes : `CREATE
/// CONSTRAINT` sur le PARENT examine aussi les nœuds qui ne portent que le
/// label de l'enfant, et `verifier_contraintes` ne compte pas deux fois la
/// violation d'un nœud portant les deux labels.
#[test]
fn contrainte_sur_un_type_parent_couvre_les_enfants() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("CREATE NODE TYPE Parent (a INTEGER)").unwrap();
    s.execute("CREATE NODE TYPE Enfant EXTENDS Parent (a INTEGER)")
        .unwrap();
    s.execute("INSERT (:Enfant {a: 1, qui: 'e1'})").unwrap();
    s.execute("INSERT (:Enfant {a: 1, qui: 'e2'})").unwrap();

    let e = s
        .execute_cypher("CREATE CONSTRAINT pu FOR (n:Parent) REQUIRE n.a IS UNIQUE")
        .expect_err("deux nœuds :Enfant portent a = 1");
    assert!(e.to_string().contains("both carry"), "{e}");

    s.execute_cypher("MATCH (n:Enfant {qui: 'e2'}) DELETE n")
        .unwrap();
    accepte(
        &s,
        "CREATE CONSTRAINT pu FOR (n:Parent) REQUIRE n.a IS UNIQUE",
        "plus de doublon",
    );
    refuse(
        &s,
        "INSERT (:Enfant {a: 1, qui: 'e3'})",
        "l'enfant hérite de la contrainte du parent",
    );

    // Un nœud portant les DEUX labels, en violation par l'API directe : une
    // seule violation, pas une par label.
    let store = db.graph_store();
    let a = obrain_common::types::PropertyKey::new("a");
    store.create_node_with_props(
        &["Parent", "Enfant"],
        &[(a.clone(), obrain_common::types::Value::Int64(1))],
    );
    let rapport = db.verifier_contraintes();
    assert_eq!(
        rapport.violations_totales, 1,
        "une violation, comptée une fois : {:?}",
        rapport.violations
    );
}

/// `NODE KEY` exige en plus la présence de chaque membre.
#[test]
fn create_constraint_node_key_refuse_un_membre_absent() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 2})").unwrap();
    let e = s
        .execute_cypher("CREATE CONSTRAINT k FOR (n:T) REQUIRE (n.a, n.b) IS NODE KEY")
        .expect_err("un nœud n'a pas b");
    assert!(e.to_string().contains('b'), "reçu : {e}");
    let e = s
        .execute_cypher("CREATE CONSTRAINT r FOR (n:T) REQUIRE n.b IS NOT NULL")
        .expect_err("un nœud n'a pas b");
    assert!(e.to_string().contains('b'), "reçu : {e}");
}
