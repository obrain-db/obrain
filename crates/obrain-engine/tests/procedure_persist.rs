//! PERSISTANCE DURABLE DES PROCEDURES STOCKEES (2026-09-07).
//!
//! Le catalogue GQL (`CREATE PROCEDURE`) vit en RAM par processus : un
//! process neuf ne connait aucune procedure creee par un process precedent
//! (le `WalRecord::CreateProcedure` n'est jamais rejoue — meme constat que
//! les index le 2026-08-14). Le remede : la definition est declaree cote
//! substrate (`substrate.proccat`, JSON par ligne, ecrit atomiquement) a
//! chaque CREATE/REPLACE/DROP, et le catalogue est hydrate au boot.
//!
//! Ce test verifie le cycle COMPLET à travers de vraies ouvertures/fermetures
//! de store substrate (pas une session unique) :
//!
//!   1. process A : CREATE PROCEDURE + fermeture du store ;
//!   2. process B (nouvelle ouverture) : CALL reussit — la procedure a
//!      survecu ;
//!   3. process B : DROP PROCEDURE + fermeture ;
//!   4. process C (nouvelle ouverture) : CALL echoue (Unknown procedure) —
//!      le retrait a survecu ;
//!   5. OR REPLACE : le corps remplace par un process est celui que voit un
//!      process suivant.
//!
//! ```text
//!   cargo test -p obrain-engine --features gql,algos --test procedure_persist
//! ```

#![cfg(all(feature = "gql", feature = "algos"))]

use obrain_engine::ObrainDB;

/// Ouvre (ou cree) un store substrate au chemin donne, execute `requete`,
/// puis FERME le store — simule un processus distinct a chaque appel.
fn ouvrir_executer_fermer(chemin: &std::path::Path, requete: &str) -> String {
    let db = ObrainDB::open_substrate(chemin).expect("ouverture substrate");
    let session = db.session();
    let resultat = session.execute(requete).expect("execution reussie");
    // La fermeture est portee par le drop du db + son flush.
    drop(session);
    drop(db);
    format!("{} rows", resultat.row_count())
}

#[test]
fn test_procedure_persiste_entre_ouvertures() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("p");

    // 1. Process A : cree la procedure, ferme.
    ouvrir_executer_fermer(
        &chemin,
        "CREATE PROCEDURE bonjour(n STRING) RETURNS (msg STRING) AS \
         { RETURN 'bonjour ' || $n AS msg }",
    );

    // Le fichier de declaration existe dans le repertoire substrate.
    let proccat = chemin.join("substrate.proccat");
    assert!(
        proccat.is_file(),
        "substrate.proccat doit exister apres CREATE PROCEDURE"
    );
    let contenu = std::fs::read_to_string(&proccat).expect("lecture proccat");
    assert!(
        contenu.contains("bonjour"),
        "le corps GQL doit etre persisté : {contenu}"
    );

    // 2. Process B (nouvelle ouverture) : CALL reussit — persistence prouvee.
    let db = ObrainDB::open_substrate(&chemin).expect("reouverture");
    let session = db.session();
    let resultat = session
        .execute("CALL bonjour('monde') YIELD msg RETURN msg")
        .expect("CALL apres reopen");
    assert_eq!(resultat.row_count(), 1);
    match &resultat.rows[0][0] {
        obrain_common::types::Value::String(s) => {
            assert_eq!(s.as_str(), "bonjour monde");
        }
        autre => panic!("msg attendu STRING, obtenu {autre:?}"),
    }
    drop(session);
    drop(db);

    // 3. Process B (encore une ouverture) : DROP, ferme.
    ouvrir_executer_fermer(&chemin, "DROP PROCEDURE bonjour");

    let contenu_apres = std::fs::read_to_string(&proccat).expect("lecture proccat");
    assert!(
        !contenu_apres.contains("bonjour"),
        "le retrait doit vider la declaration : {contenu_apres}"
    );

    // 4. Process C : CALL echoue — le retrait a survecu.
    let db = ObrainDB::open_substrate(&chemin).expect("reouverture 2");
    let session = db.session();
    let resultat = session.execute("CALL bonjour('x') YIELD msg RETURN msg");
    assert!(resultat.is_err(), "la procedure droppee doit etre inconnue");
    let erreur = resultat.unwrap_err().to_string();
    assert!(
        erreur.contains("Unknown procedure"),
        "attendu 'Unknown procedure', obtenu : {erreur}"
    );
    drop(session);
    drop(db);
}

#[test]
fn test_or_replace_persiste_le_nouveau_corps() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("p2");

    ouvrir_executer_fermer(
        &chemin,
        "CREATE PROCEDURE saluer() RETURNS (msg STRING) AS { RETURN 'v1' AS msg }",
    );
    ouvrir_executer_fermer(
        &chemin,
        "CREATE OR REPLACE PROCEDURE saluer() RETURNS (msg STRING) AS { RETURN 'v2' AS msg }",
    );

    // Nouvelle ouverture : c'est le corps v2 qui doit repondre.
    let db = ObrainDB::open_substrate(&chemin).expect("reouverture");
    let session = db.session();
    let resultat = session
        .execute("CALL saluer() YIELD msg RETURN msg")
        .expect("CALL apres OR REPLACE");
    assert_eq!(resultat.row_count(), 1);
    match &resultat.rows[0][0] {
        obrain_common::types::Value::String(s) => {
            assert_eq!(s.as_str(), "v2");
        }
        autre => panic!("msg attendu STRING, obtenu {autre:?}"),
    }
}
