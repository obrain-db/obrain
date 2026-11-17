//! `datetime()` est zoné, comme dans Neo4j (obrain-db/obrain#183, décision PO
//! b09d2c91).
//!
//! - `datetime()` rend une `ZonedDatetime`, UTC par défaut ; un texte garde
//!   son décalage.
//! - Une propriété stockée en `ZonedDatetime` (ce qu'écrit l'API, donc toute
//!   donnée migrée d'un `DateTime` Neo4j) se compare par INSTANT à
//!   `datetime(…)` : égalité, ordre, filtre, avec et sans index.
//! - `localdatetime(…)` contre `datetime(…)` reste NULL, comme
//!   `LocalDateTime` contre `DateTime` dans Neo4j.
//! - `toString(datetime('…+02:00'))` rend l'ISO 8601 avec son décalage.
//!
//! Chaque cas est joué en Cypher et en GQL.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test datetime_zone_neo4j_183
//! ```

use obrain_common::types::{Date, Timestamp, Value, ZonedDatetime};
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// 2020-01-02T03:04:05Z.
const INSTANT_NS: i64 = 1_577_934_245_000_000_000;

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

/// Une base avec un nœud `:Z` dont `t` est une `ZonedDatetime` écrite par
/// l'API (forme des données migrées).
fn base_zonee(indexee: bool) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let id = db.create_node(&["Z"]);
    db.set_node_property(
        id,
        "t",
        Value::ZonedDatetime(ZonedDatetime::from_timestamp_offset(
            Timestamp::from_nanos(INSTANT_NS),
            0,
        )),
    );
    if indexee {
        db.create_property_index("t");
    }
    db
}

#[test]
fn datetime_rend_une_valeur_zonee() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        for (requete, decalage) in [
            ("RETURN datetime() AS v", 0),
            ("RETURN datetime('2020-01-02T03:04:05Z') AS v", 0),
            ("RETURN datetime('2020-01-02T03:04:05') AS v", 0),
            ("RETURN datetime('2020-01-02T05:04:05+02:00') AS v", 7200),
        ] {
            match valeur(&db, langage, requete) {
                Value::ZonedDatetime(z) => {
                    assert_eq!(z.offset_seconds(), decalage, "[{langage}] {requete}");
                    if requete.contains("2020") {
                        assert_eq!(
                            z.as_timestamp(),
                            Timestamp::from_nanos(INSTANT_NS),
                            "[{langage}] {requete}"
                        );
                    }
                }
                autre => panic!("[{langage}] {requete} : ZonedDatetime attendue, reçu {autre:?}"),
            }
        }
    }
}

#[test]
fn propriete_zonee_contre_datetime_compare_les_instants() {
    for indexee in [false, true] {
        let db = base_zonee(indexee);
        for langage in LANGAGES {
            for (requete, attendu) in [
                (
                    "MATCH (z:Z) RETURN z.t = datetime('2020-01-02T03:04:05Z') AS v",
                    Value::Bool(true),
                ),
                (
                    "MATCH (z:Z) RETURN z.t = datetime('2020-01-02T05:04:05+02:00') AS v",
                    Value::Bool(true),
                ),
                (
                    "MATCH (z:Z) RETURN z.t < datetime() AS v",
                    Value::Bool(true),
                ),
                (
                    "MATCH (z:Z) WHERE z.t < datetime() RETURN count(z) AS v",
                    Value::Int64(1),
                ),
                (
                    "MATCH (z:Z) WHERE z.t < datetime('2030-01-01T00:00:00Z') RETURN count(z) AS v",
                    Value::Int64(1),
                ),
                (
                    "MATCH (z:Z) WHERE z.t = datetime('2020-01-02T05:04:05+02:00') \
                     RETURN count(z) AS v",
                    Value::Int64(1),
                ),
                (
                    "MATCH (z:Z) RETURN z.t = datetime(toString(z.t)) AS v",
                    Value::Bool(true),
                ),
            ] {
                assert_eq!(
                    valeur(&db, langage, requete),
                    attendu,
                    "[{langage}] indexée = {indexee} : {requete}"
                );
            }
        }
    }
}

#[test]
fn datetime_ecrit_par_cypher_egale_la_propriete_de_l_api() {
    let db = base_zonee(false);
    db.execute_cypher("CREATE (:N {t: datetime('2020-01-02T03:04:05Z')})")
        .unwrap();
    for langage in LANGAGES {
        assert_eq!(
            valeur(&db, langage, "MATCH (n:N), (z:Z) RETURN n.t = z.t AS v"),
            Value::Bool(true),
            "[{langage}]"
        );
    }
}

#[test]
fn localdatetime_contre_datetime_rend_null() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        for requete in [
            "RETURN localdatetime('2020-01-02T03:04:05') = datetime('2020-01-02T03:04:05Z') AS v",
            "RETURN localdatetime('2020-01-02T03:04:05') < datetime('2020-01-02T03:04:05Z') AS v",
        ] {
            assert_eq!(
                valeur(&db, langage, requete),
                Value::Null,
                "[{langage}] {requete}"
            );
        }
    }
}

#[test]
fn tostring_garde_le_decalage() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        assert_eq!(
            valeur(
                &db,
                langage,
                "RETURN toString(datetime('2020-01-02T03:04:05+02:00')) AS v"
            ),
            Value::String("2020-01-02T03:04:05+02:00".into()),
            "[{langage}]"
        );
    }
}

/// Arithmétique et conversions d'une valeur zonée : le calendrier est celui
/// du fuseau de la valeur.
#[test]
fn arithmetique_et_conversions_dans_le_fuseau() {
    let db = ObrainDB::new_in_memory();
    for langage in LANGAGES {
        for (requete, attendu) in [
            (
                "RETURN toString(datetime('2024-01-31T00:30:00+02:00') + duration('P1M')) AS v",
                Value::String("2024-02-29T00:30:00+02:00".into()),
            ),
            (
                "RETURN toString(datetime('2024-03-01T00:30:00+02:00') - duration('P1D')) AS v",
                Value::String("2024-02-29T00:30:00+02:00".into()),
            ),
            (
                "RETURN datetime('2024-01-01T10:00:00+02:00') + duration('PT1H') \
                 = datetime('2024-01-01T09:00:00Z') AS v",
                Value::Bool(true),
            ),
            (
                "RETURN date(datetime('2024-01-01T00:30:00+02:00')) AS v",
                Value::Date(Date::from_ymd(2024, 1, 1).unwrap()),
            ),
            (
                "RETURN datetime('2024-01-01T00:30:00+02:00').day AS v",
                Value::Int64(1),
            ),
            (
                "RETURN toString(datetime({year: 2024, month: 1, day: 1, hour: 10, \
                 timezone: '+02:00'})) AS v",
                Value::String("2024-01-01T10:00:00+02:00".into()),
            ),
        ] {
            assert_eq!(
                valeur(&db, langage, requete),
                attendu,
                "[{langage}] {requete}"
            );
        }
    }
}
