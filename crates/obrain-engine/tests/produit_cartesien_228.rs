//! Produit cartésien `MATCH (a), (b) WHERE …` (obrain-db/obrain#228).
//!
//! Le motif à deux parties disjointes devient un scan imbriqué : `b` est
//! balayé pour chaque ligne de `a`. Le filtre du `WHERE` restait EN BLOC
//! au-dessus du produit :
//!
//! - `code_44` (`WHERE a.id = $a AND b.id = $b`) : le garde d'explosion voyait
//!   n × n paires et refusait une requête que Neo4j exécute ;
//! - `topo_04` (`WHERE s.path =~ $r …`) : sous le plafond, les n² paires
//!   étaient réellement calculées avant la regex, et la requête ne rendait
//!   jamais la main sur la copie PO.
//!
//! Les conjoints qui ne citent pas la variable du scan descendent maintenant
//! dans son entrée.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test produit_cartesien_228
//! ```
#![cfg(feature = "cypher")]

use std::collections::HashMap;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const N: i64 = 200;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(&format!(
            "UNWIND range(1, {N}) AS i \
             CREATE (:File {{project_id: 'p', path: 'f' + toString(i), id: 'f' + toString(i)}})"
        ))
        .unwrap();
    db
}

fn params() -> HashMap<String, Value> {
    HashMap::from([
        ("project_id".to_string(), Value::from("p")),
        ("a".to_string(), Value::from("f1")),
        ("b".to_string(), Value::from("f2")),
        ("rs".to_string(), Value::from("f1[0-9]")),
        ("rt".to_string(), Value::from("f2[0-9]")),
    ])
}

fn plan(db: &ObrainDB, requete: &str) -> String {
    match &db
        .execute_cypher_with_params(&format!("EXPLAIN {requete}"), params())
        .unwrap()
        .rows[0][0]
    {
        Value::String(s) => s.to_string(),
        autre => panic!("{autre:?}"),
    }
}

/// Position (ligne) du premier `motif` dans le plan, de haut en bas.
fn ligne(plan: &str, motif: &str) -> usize {
    plan.lines()
        .position(|l| l.contains(motif))
        .unwrap_or_else(|| panic!("{motif} absent de\n{plan}"))
}

/// `code_44` : deux nœuds désignés par leur clé, reliés ensuite. Le filtre de
/// `a` descend sous le scan de `b` : le garde voit 1 × n, et la requête passe
/// même sous un plafond inférieur à n².
#[test]
fn code_44_cles_des_deux_cotes() {
    // SAFETY du test : le plafond est lu à la planification ; aucun autre test
    // de ce binaire ne dépend de sa valeur par défaut.
    unsafe { std::env::set_var("OBRAIN_MAX_PRODUIT", (N * N / 2).to_string()) };
    let db = base();
    let q = "MATCH (a), (b) WHERE a.id = $a AND b.id = $b RETURN a.id, b.id";
    let p = plan(&db, q);
    assert!(
        ligne(&p, "a.id Eq") > ligne(&p, "NodeScan (b"),
        "le filtre de a doit être sous le scan de b :\n{p}"
    );
    let rows = db.execute_cypher_with_params(q, params()).unwrap().rows;
    assert_eq!(rows, vec![vec![Value::from("f1"), Value::from("f2")]]);
}

/// `topo_04` : la regex de `s` est évaluée AVANT le produit.
#[test]
fn topo_04_regex_avant_le_produit() {
    let db = base();
    let q = "MATCH (s:File {project_id: $project_id}), (t:File {project_id: $project_id}) \
             WHERE s.path =~ $rs AND t.path =~ $rt RETURN count(*)";
    let p = plan(&db, q);
    assert!(
        ligne(&p, "s.path Regex") > ligne(&p, "NodeScan (t"),
        "la regex de s doit être sous le scan de t :\n{p}"
    );
    // f10..f19 × f20..f29.
    let rows = db.execute_cypher_with_params(q, params()).unwrap().rows;
    assert_eq!(rows, vec![vec![Value::Int64(100)]]);
}

/// Un conjoint qui cite les deux côtés, ou une sous-requête sur la variable du
/// scan, reste au-dessus : le résultat ne change pas.
#[test]
fn conjoints_non_deplacables() {
    let db = base();
    db.session()
        .execute_cypher("MATCH (a:File {id: 'f1'}), (b:File {id: 'f2'}) CREATE (a)-[:R]->(b)")
        .unwrap();
    let rows = db
        .execute_cypher_with_params(
            "MATCH (a:File), (b:File) \
             WHERE a.id = $a AND a.project_id = b.project_id AND EXISTS { (b)<-[:R]-() } \
             RETURN b.id",
            params(),
        )
        .unwrap()
        .rows;
    assert_eq!(rows, vec![vec![Value::from("f2")]]);
}

/// `=~` couvre toute la chaîne, comme sous Neo4j : littéral, paramètre, NULL.
#[test]
fn regex_couvre_toute_la_chaine() {
    let db = base();
    let un = |q: &str| {
        db.execute_cypher_with_params(q, params()).unwrap_or_else(|e| panic!("{q}\n{e}")).rows[0][0]
            .clone()
    };
    assert_eq!(un("RETURN 'f100' =~ 'f1[0-9]'"), Value::Bool(false));
    assert_eq!(un("RETURN 'f10' =~ 'f1[0-9]'"), Value::Bool(true));
    assert_eq!(un("RETURN 'f100' =~ $rs"), Value::Bool(false));
    assert_eq!(un("RETURN 'f12' =~ $rs"), Value::Bool(true));
    assert_eq!(un("RETURN 'ab' =~ 'a|ab'"), Value::Bool(true));
    assert_eq!(un("RETURN 'xab' =~ '.*ab'"), Value::Bool(true));
    assert_eq!(un("RETURN 'ab' =~ null"), Value::Null);
    assert_eq!(
        un("MATCH (f:File) WHERE f.path =~ 'f1[0-9]' RETURN count(f)"),
        Value::Int64(10)
    );
}
