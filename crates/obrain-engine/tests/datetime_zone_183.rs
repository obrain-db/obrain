//! Temporel, partie sûre de obrain-db/obrain#183 : `toString` rend l'ISO 8601
//! (et non le Debug) pour les valeurs temporelles, et `epochSeconds` /
//! `epochMillis` existent, calculés sur l'instant UTC.
//!
//! Hors de ce fichier, et volontairement : `datetime()` reste un `Timestamp`
//! tant que la comparaison entre `Timestamp` et `ZonedDatetime` n'est pas
//! tranchée (classes distinctes dans `value_cmp`, tests gardés de duration_between
//! et temporal_types). Voir le commentaire de la PR et de l'issue.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test datetime_zone_183
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::{Timestamp, Value, ZonedDatetime};
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

#[test]
fn tostring_zone_stockee_rend_l_iso() {
    let db = ObrainDB::new_in_memory();
    let id = db.create_node(&["Z"]);
    db.set_node_property(
        id,
        "t",
        Value::ZonedDatetime(ZonedDatetime::from_timestamp_offset(
            Timestamp::from_nanos(1_577_934_245_000_000_000),
            0,
        )),
    );
    for langage in LANGAGES {
        let texte = match valeur(&db, langage, "MATCH (z:Z) RETURN toString(z.t) AS v") {
            Value::String(s) => s.to_string(),
            autre => panic!("[{langage}] chaîne attendue, reçu {autre:?}"),
        };
        assert!(
            texte.starts_with("2020-01-02T03:04:05") && !texte.contains("ZonedDatetime("),
            "[{langage}] {texte}"
        );
    }
}

#[test]
fn tostring_datetime_rend_l_iso() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        match valeur(
            &db,
            langage,
            "RETURN toString(datetime('2020-01-02T03:04:05Z')) AS v",
        ) {
            Value::String(s) => assert!(
                s.starts_with("2020-01-02T03:04:05") && !s.contains("Timestamp("),
                "[{langage}] {s}"
            ),
            autre => panic!("[{langage}] chaîne attendue, reçu {autre:?}"),
        }
    }
}

#[test]
fn epoch_secondes_et_millisecondes() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        assert_eq!(
            valeur(
                &db,
                langage,
                "RETURN datetime('2020-01-02T03:04:05Z').epochSeconds AS v"
            ),
            Value::Int64(1_577_934_245),
            "[{langage}] epochSeconds"
        );
        assert_eq!(
            valeur(
                &db,
                langage,
                "RETURN datetime('2020-01-02T03:04:05Z').epochMillis AS v"
            ),
            Value::Int64(1_577_934_245_000),
            "[{langage}] epochMillis"
        );
    }
}
