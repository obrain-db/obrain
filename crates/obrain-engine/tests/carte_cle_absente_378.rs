//! #378 — un littéral de carte dont une valeur est ABSENTE (`{a: m.id}` avec
//! `m.id` non posée) doit GARDER la clé avec la valeur NULL, comme Neo4j et
//! comme le littéral `{a: null}`.
//!
//! Les cartes de PROPRIÉTÉS d'un nœud (`RETURN m`, `properties(m)`) n'ont
//! légitimement PAS les clés absentes : ce fichier le verrouille aussi.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test carte_cle_absente_378
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:File {path: 'p'})")
        .unwrap();
    db
}

fn une_valeur(db: &ObrainDB, requete: &str) -> Value {
    let r = db.execute_cypher(requete).expect(requete);
    assert_eq!(r.rows.len(), 1, "{requete} : {:?}", r.rows);
    r.rows[0][0].clone()
}

fn cles(v: &Value) -> Vec<String> {
    match v {
        Value::Map(m) => m.keys().map(|k| k.as_str().to_string()).collect(),
        autre => panic!("carte attendue, reçu {autre:?}"),
    }
}

fn entree<'a>(v: &'a Value, cle: &str) -> Option<&'a Value> {
    match v {
        Value::Map(m) => m.get(&obrain_common::types::PropertyKey::new(cle)),
        autre => panic!("carte attendue, reçu {autre:?}"),
    }
}

/// La requête de l'issue.
#[test]
fn keys_garde_la_cle_dont_la_valeur_est_absente() {
    let db = base();
    let k = une_valeur(&db, "MATCH (m:File) RETURN keys({a: m.id, b: m.path}) AS k");
    assert_eq!(
        k,
        Value::List(vec![Value::from("a"), Value::from("b")].into())
    );
}

#[test]
fn size_compte_la_cle_dont_la_valeur_est_absente() {
    let db = base();
    let n = une_valeur(
        &db,
        "MATCH (m:File) RETURN size(keys({a: m.id, b: m.path})) AS n",
    );
    assert_eq!(n, Value::Int64(2));
}

#[test]
fn la_carte_rendue_porte_la_cle_a_null() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN {a: m.id, b: m.path} AS m");
    assert_eq!(cles(&m), vec!["a", "b"]);
    assert_eq!(entree(&m, "a"), Some(&Value::Null));
    assert_eq!(entree(&m, "b"), Some(&Value::from("p")));
}

#[test]
fn litteral_null_garde_la_cle() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN {a: null} AS m");
    assert_eq!(cles(&m), vec!["a"]);
    assert_eq!(entree(&m, "a"), Some(&Value::Null));
}

#[test]
fn valeur_presente_et_litteral_inchanges() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN {a: 1, b: m.path} AS m");
    assert_eq!(cles(&m), vec!["a", "b"]);
    assert_eq!(entree(&m, "a"), Some(&Value::Int64(1)));
    assert_eq!(entree(&m, "b"), Some(&Value::from("p")));
}

#[test]
fn carte_vide() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN {} AS m");
    assert_eq!(cles(&m), Vec::<String>::new());
}

#[test]
fn carte_imbriquee_avec_valeur_absente() {
    let db = base();
    let m = une_valeur(
        &db,
        "MATCH (m:File) RETURN {x: {a: m.id, b: m.path}, y: 2} AS m",
    );
    assert_eq!(cles(&m), vec!["x", "y"]);
    let x = entree(&m, "x").unwrap();
    assert_eq!(cles(x), vec!["a", "b"]);
    assert_eq!(entree(x, "a"), Some(&Value::Null));
}

#[test]
fn les_proprietes_du_noeud_n_ont_pas_les_cles_absentes() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN properties(m) AS p");
    assert_eq!(cles(&m), vec!["path"]);
}

/// Les entrées `*` (propriétés d'un nœud) restent aplaties, sans clé `*`.
#[test]
fn projection_de_carte_avec_toutes_proprietes_est_aplatie() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN m{.*, extra: 1} AS m");
    assert_eq!(cles(&m), vec!["extra", "path"]);
}

/// Projection `m{.id, .path}` : même chemin d'évaluation que le littéral, donc
/// `id` reste à NULL comme chez Neo4j (rouge sur `main` : `id` disparaissait).
#[test]
fn projection_de_carte_propriete_absente_garde_la_cle() {
    let db = base();
    let m = une_valeur(&db, "MATCH (m:File) RETURN m{.id, .path} AS m");
    assert_eq!(cles(&m), vec!["id", "path"]);
    assert_eq!(entree(&m, "id"), Some(&Value::Null));
}

#[test]
fn gql_litteral_de_carte_garde_la_cle() {
    let db = ObrainDB::new_in_memory();
    db.execute("INSERT (:File {path: 'p'})").unwrap();
    let r = db
        .execute("MATCH (m:File) RETURN {a: m.id, b: m.path} AS k")
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(cles(&r.rows[0][0]), vec!["a", "b"]);
}
