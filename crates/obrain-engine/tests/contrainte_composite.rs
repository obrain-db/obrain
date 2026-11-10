//! Régression : un `UNIQUE (a, b)` est une contrainte sur le TUPLE, pas sur
//! chaque colonne prise séparément.
//!
//! Le contrôle d'unicité testait `props.iter().any(|p| p == key)` : une
//! contrainte composite se comportait donc comme « a unique ET b unique »,
//! rejetant des lignes légitimes et laissant passer des doublons de tuple.

#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
fn composite_unique_respecte_le_tuple_pas_les_colonnes() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute_cypher("CREATE CONSTRAINT composite_unique FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
        .expect("création de la contrainte composite");

    // (1) deux tuples distincts partageant une colonne sont TOUS DEUX acceptés.
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");
    let r = session.execute("INSERT (:T {a: 1, b: 'y'})");
    assert!(
        r.is_ok(),
        "UNIQUE(a,b) doit accepter (1,'x') puis (1,'y') — reçu {:?}",
        r.err()
    );

    // (2) le même tuple exact est refusé.
    let r = session.execute("INSERT (:T {a: 1, b: 'x'})");
    assert!(
        r.is_err(),
        "UNIQUE(a,b) doit refuser (1,'x') une seconde fois"
    );

    // (3) un tuple ne différant que sur l'autre colonne est accepté.
    let r = session.execute("INSERT (:T {a: 2, b: 'x'})");
    assert!(
        r.is_ok(),
        "UNIQUE(a,b) doit accepter (2,'x') après (1,'x') — reçu {:?}",
        r.err()
    );
}

#[test]
fn contrainte_composite_survit_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");

    // Process A : créer la contrainte, fermer.
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture A");
        let session = db.session();
        session
            .execute_cypher("CREATE CONSTRAINT cc FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
            .expect("CREATE CONSTRAINT A");
    }

    // Process B (réouverture) : la contrainte tient toujours.
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture B");
    let session = db.session();
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");
    let r = session.execute("INSERT (:T {a: 1, b: 'y'})");
    assert!(
        r.is_ok(),
        "après réouverture UNIQUE(a,b) doit accepter (1,'y') — reçu {:?}",
        r.err()
    );
    let r = session.execute("INSERT (:T {a: 1, b: 'x'})");
    assert!(
        r.is_err(),
        "après réouverture UNIQUE(a,b) doit refuser (1,'x') une seconde fois"
    );
}
