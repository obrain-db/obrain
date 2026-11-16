//! Clé de groupement COMPOSITE (carte, liste) : la valeur doit ressortir telle
//! quelle, pas en texte Debug (obrain-db/obrain#185).
//!
//! Avant correctif, `WITH m, count(*) AS n` où `m` est une carte rendait
//! `m.p` = NULL, `size(l)` = 19 (longueur de la chaîne « List([String("x")]) »)
//! et `m = {p:'x'}` faux. Le regroupement lui-même était juste : seule la valeur
//! restituée était fausse.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test cle_groupement_composite_185
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

fn valeur(db: &ObrainDB, langage: &str, requete: &str) -> Value {
    let r = db
        .session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"));
    match r.rows.as_slice() {
        [ligne] => ligne[0].clone(),
        autre => panic!("[{langage}] {requete} : une ligne attendue, reçu {autre:?}"),
    }
}

fn lignes(db: &ObrainDB, langage: &str, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"))
        .rows
}

#[test]
fn carte_en_cle_de_groupe_garde_ses_proprietes() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        assert_eq!(
            valeur(
                &db,
                langage,
                "UNWIND [{p:'x'}] AS m WITH m, count(*) AS n RETURN m.p AS v"
            ),
            Value::String("x".into()),
            "[{langage}] m.p"
        );
        assert_eq!(
            valeur(
                &db,
                langage,
                "UNWIND [{p:'x'}] AS m WITH m, count(*) AS n RETURN m['p'] AS v"
            ),
            Value::String("x".into()),
            "[{langage}] m['p']"
        );
        assert_eq!(
            valeur(
                &db,
                langage,
                "UNWIND [{p:'x'}] AS m WITH m, count(*) AS n RETURN m = {p:'x'} AS v"
            ),
            Value::Bool(true),
            "[{langage}] égalité"
        );
    }
}

#[test]
fn liste_en_cle_de_groupe_garde_ses_elements() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        assert_eq!(
            valeur(
                &db,
                langage,
                "UNWIND [['x']] AS l WITH l, count(*) AS n RETURN size(l) AS v"
            ),
            Value::Int64(1),
            "[{langage}] size"
        );
        assert_eq!(
            valeur(
                &db,
                langage,
                "UNWIND [['x']] AS l WITH l, count(*) AS n RETURN l[0] AS v"
            ),
            Value::String("x".into()),
            "[{langage}] l[0]"
        );
    }
}

/// Le regroupement reste exact : deux cartes égales forment UN groupe.
#[test]
fn regroupement_des_cartes_egales() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        assert_eq!(
            lignes(
                &db,
                langage,
                "UNWIND [{p:'x'},{p:'x'},{p:'y'}] AS m WITH m, count(*) AS n \
                 RETURN m.p AS p, n ORDER BY p"
            ),
            vec![
                vec![Value::String("x".into()), Value::Int64(2)],
                vec![Value::String("y".into()), Value::Int64(1)],
            ],
            "[{langage}]"
        );
    }
}
