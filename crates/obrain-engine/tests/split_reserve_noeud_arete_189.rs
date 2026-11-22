//! Le partage des clés réservées NOEUD / ARÊTE, éprouvé dans les deux sens (#189).
//!
//! `_type`, `_source`, `_target` ne sont écrites que par l'hydratation d'une
//! ARÊTE ; `_id`, `_labels`, `_base` par celle d'un NOEUD. Le partage n'a de sens
//! que si les deux côtés sont vérifiés :
//!
//!   - un NŒUD distant portant `_type` (clé d'ARÊTE) doit rester traversable, et
//!     sa valeur utilisateur doit survivre : rien ne l'écrase, rien ne la refuse ;
//!   - un NŒUD distant portant `_labels` (clé de NŒUD) doit être refusé.
//!
//! Avant le partage, la liste était commune : le premier cas était refusé à tort.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,cypher" --test split_reserve_noeud_arete_189
//! ```
#![cfg(all(feature = "gql", feature = "cypher"))]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// alpha (primaire) : Note n -[:L]-> Hub h -[:T]-> ref(beta, selon_le_cas).
fn base_avec(propriete: &str, valeur: &str) -> ObrainDB {
    let beta = ObrainDB::new_in_memory();
    let sb = beta.session();
    sb.execute(&format!("INSERT (:A {{id: 'a', {propriete}: '{valeur}'}})"))
        .expect("noeud A");
    sb.execute("INSERT (:B {id: 'b'})").expect("noeud B");
    sb.execute("MATCH (a:A {id: 'a'}), (b:B {id: 'b'}) CREATE (a)-[:E]->(b)")
        .expect("arete E");

    let alpha = ObrainDB::new_in_memory();
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("note");
    sa.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"a\"))")
        .expect("T");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    alpha
}

/// `_type` est une clé d'ARÊTE : sur un nœud distant elle est une propriété
/// utilisateur ordinaire, ni refusée ni écrasée.
#[test]
fn un_noeud_distant_portant_type_reste_traversable_et_garde_sa_valeur() {
    let alpha = base_avec("_type", "forged");
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L|T|E*3..3]->(z) RETURN [x IN nodes(p) | x._type]")
        .expect("un nœud portant _type ne doit PAS être refusé : _type est réservée aux arêtes");

    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    let valeurs: Vec<Option<String>> = match &r.rows[0][0] {
        Value::List(items) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => Some(s.to_string()),
                Value::Null => None,
                autre => panic!("texte ou nul attendu : {autre:?}"),
            })
            .collect(),
        autre => panic!("liste attendue : {autre:?}"),
    };
    assert_eq!(
        valeurs,
        vec![None, None, Some("forged".to_string()), None],
        "seul le nœud distant `a` porte _type ; sa valeur utilisateur doit survivre"
    );
}

/// `_labels` est une clé de NŒUD : sur un nœud distant, elle collisionne avec la
/// métadonnée et doit être refusée — pas écrasée en silence.
#[test]
fn un_noeud_distant_portant_labels_est_refuse() {
    let alpha = base_avec("_labels", "forged");
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L|T|E*3..3]->(z) RETURN length(p)");
    let msg = match r {
        Ok(v) => panic!("_labels sur un nœud distant doit être refusé, obtenu {v:?}"),
        Err(e) => format!("{e}"),
    };
    assert!(
        msg.contains("réservée"),
        "le refus doit nommer la clé réservée : {msg}"
    );
}
