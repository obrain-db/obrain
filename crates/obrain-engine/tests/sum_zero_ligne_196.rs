//! `sum()` sur un ensemble vide : 0 en openCypher, NULL en GQL
//! (obrain-db/obrain#196, famille D4).
//!
//! Neo4j rend 0 ; obrain rendait NULL dans les deux langages. Un compteur qui
//! passe de 0 à NULL casse les expressions qui s'en servent (`total + x`).
//! La norme SQL / ISO GQL dit NULL, et `gql_07_agregats` le garde : la
//! différence se fait donc dans le traducteur Cypher seul.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test sum_zero_ligne_196
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Deux notes actives (w = 2 et 3), aucune archivée.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (:Note {status: 'active', tags: ['a','b'], w: 2}), \
             (:Note {status: 'active', tags: ['c'], w: 3})",
        )
        .unwrap();
    db
}

fn cypher(db: &ObrainDB, requete: &str) -> (Vec<String>, Vec<Vec<Value>>) {
    let r = db
        .session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"));
    (r.columns, r.rows)
}

const VIDE: &str = "MATCH (n:Note) WHERE n.status = 'archived'";

#[test]
fn cypher_sum_sur_zero_ligne_vaut_zero() {
    let db = base();
    // La requête exacte de l'issue.
    assert_eq!(
        cypher(&db, &format!("{VIDE} RETURN sum(size(n.tags)) AS total")).1,
        vec![vec![Value::Int64(0)]]
    );
    assert_eq!(
        cypher(&db, &format!("{VIDE} RETURN sum(n.w) AS total")).1,
        vec![vec![Value::Int64(0)]]
    );
    // Une propriété absente partout : aucune valeur à sommer, donc 0 aussi.
    assert_eq!(
        cypher(&db, "MATCH (n:Note) RETURN sum(n.absente) AS total").1,
        vec![vec![Value::Int64(0)]]
    );
}

#[test]
fn cypher_sum_dans_une_expression() {
    let db = base();
    assert_eq!(
        cypher(&db, &format!("{VIDE} RETURN sum(n.w) + 1 AS t")).1,
        vec![vec![Value::Int64(1)]]
    );
    assert_eq!(
        cypher(&db, &format!("{VIDE} WITH sum(n.w) AS t RETURN t + 1 AS u")).1,
        vec![vec![Value::Int64(1)]]
    );
}

/// `avg`, `min`, `max` restent NULL sur un ensemble vide (Neo4j aussi), et
/// `count` vaut 0.
#[test]
fn cypher_les_autres_agregats_ne_changent_pas() {
    let db = base();
    assert_eq!(
        cypher(
            &db,
            &format!("{VIDE} RETURN count(n) AS c, avg(n.w) AS a, min(n.w) AS mi, max(n.w) AS ma")
        )
        .1,
        vec![vec![Value::Int64(0), Value::Null, Value::Null, Value::Null]]
    );
}

/// Témoins : la somme non vide et le nom de colonne sans alias sont inchangés.
#[test]
fn cypher_somme_non_vide_et_nom_de_colonne_inchanges() {
    let db = base();
    let (colonnes, lignes) = cypher(&db, "MATCH (n:Note) RETURN sum(n.w)");
    assert_eq!(colonnes, vec!["sum(...)".to_string()]);
    assert_eq!(lignes, vec![vec![Value::Int64(5)]]);
    assert_eq!(
        cypher(
            &db,
            "MATCH (n:Note) RETURN n.status AS s, sum(n.w) AS total"
        )
        .1,
        vec![vec![Value::String("active".into()), Value::Int64(5)]]
    );
}

/// GQL garde la règle SQL / ISO : NULL sur un ensemble vide.
#[test]
fn gql_sum_sur_zero_ligne_reste_null() {
    let db = base();
    let r = db
        .session()
        .execute(&format!("{VIDE} RETURN sum(n.w) AS total"))
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Null]]);
}
