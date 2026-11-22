//! Arête externe qui REVIENT vers la base primaire (#189).
//!
//! Un chemin peut franchir une frontière dans les deux sens : primaire -> beta,
//! puis beta -> primaire. Le code traite les deux cas différemment
//! (`expand.rs`, `is_primary`) : une cible dans la base PRIMAIRE s'émet en
//! `Int64` (identifiant du store primaire), une cible distante en `Map` hydratée.
//! Le retour vers la primaire n'était couvert par aucun test : tous les
//! scénarios multi-bases partaient de la primaire et n'y revenaient pas.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,cypher" --test retour_vers_primaire_189
//! ```
#![cfg(all(feature = "gql", feature = "cypher"))]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// alpha : Note n -[:L]-> Hub h -[:T]-> ref(beta, 'a') ; Cible c (id 'cible').
/// beta  : a -[:BACK]-> ref(alpha, 'cible')   (arête externe, retour primaire)
fn deux_bases() -> ObrainDB {
    let beta = ObrainDB::new_in_memory();
    let sb = beta.session();
    sb.execute("INSERT (:A {id: 'a'})").expect("noeud a");
    sb.execute("MATCH (a:A {id: 'a'}) CREATE (a)-[:BACK]->(ref(\"alpha\", \"cible\"))")
        .expect("arete BACK vers alpha");

    let alpha = ObrainDB::new_in_memory();
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("note");
    sa.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    sa.execute("INSERT (:Cible {id: 'cible', nom: 'retour'})")
        .expect("cible");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"a\"))")
        .expect("T");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    alpha
}

/// Le nœud d'arrivée est dans la base PRIMAIRE : il doit être un `Int64`
/// (identifiant primaire), pas une `Map` hydratée, et son identifiant doit
/// résoudre sur la bonne entité.
#[test]
fn une_arete_externe_qui_revient_vers_la_primaire_rend_un_identifiant_primaire() {
    let alpha = deux_bases();
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L|T|BACK*3..3]->(z) \
             RETURN [x IN nodes(p) | x], [r IN relationships(p) | type(r)]",
        )
        .expect("chemin primaire -> beta -> primaire");

    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);

    let noeuds = match &r.rows[0][0] {
        Value::List(items) => items.clone(),
        autre => panic!("liste de nœuds attendue : {autre:?}"),
    };
    assert_eq!(noeuds.len(), 4, "n, h, a, cible : {noeuds:?}");
    // 1er, 2e et 4e : primaires (Int64). 3e : distant (Map).
    assert!(
        matches!(noeuds[0], Value::Int64(_)) && matches!(noeuds[1], Value::Int64(_)),
        "les nœuds primaires de départ doivent être des identifiants bruts : {noeuds:?}"
    );
    assert!(
        matches!(noeuds[2], Value::Map(_)),
        "le nœud de beta doit être une Map hydratée : {noeuds:?}"
    );
    assert!(
        matches!(noeuds[3], Value::Int64(_)),
        "le RETOUR vers la primaire doit être un identifiant primaire, pas une Map : {noeuds:?}"
    );

    let types: Vec<Option<String>> = match &r.rows[0][1] {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => Some(s.to_string()),
                Value::Null => None,
                autre => panic!("texte attendu : {autre:?}"),
            })
            .collect(),
        autre => panic!("liste de types attendue : {autre:?}"),
    };
    assert_eq!(
        types,
        vec![
            Some("L".to_string()),
            Some("T".to_string()),
            Some("BACK".to_string())
        ],
        "les trois arêtes, dont l'externe de retour"
    );

    // Et la propriété de l'entité primaire reste lisible à travers le chemin.
    let nom = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L|T|BACK*3..3]->(z) RETURN [x IN nodes(p) | x.nom]")
        .expect("propriétés");
    let noms: Vec<Option<String>> = match &nom.rows[0][0] {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => Some(s.to_string()),
                Value::Null => None,
                autre => panic!("texte attendu : {autre:?}"),
            })
            .collect(),
        autre => panic!("liste attendue : {autre:?}"),
    };
    assert_eq!(
        noms,
        vec![None, None, None, Some("retour".to_string())],
        "seule la cible primaire porte `nom`"
    );
}
