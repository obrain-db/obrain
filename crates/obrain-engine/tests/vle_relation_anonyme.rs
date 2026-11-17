//! Une relation de longueur variable SEULE dans sa clause n'est pas nommée.
//!
//! L'unicité des relations d'une clause MATCH (#182, PR #190) nomme les
//! relations anonymes pour poser des filtres `r_i <> r_j`. Quand la clause
//! n'a aucune paire à confronter (une seule relation, ou seulement des
//! relations variables), ce nom ne pose aucun filtre : il fait seulement
//! matérialiser la liste d'arêtes du chemin. Mesuré sur
//! `aretes_inter_bases_bench`, `-[:DOCUMENTS*1..2]->` : p50 intra 1,16 ms
//! avant #190, 1,97 ms après, 1,23 ms avec ce correctif.
//!
//! Le test lit le plan logique : un chronomètre ne ferait pas un
//! garde-fou fiable. Le témoin positif vérifie que le nommage est toujours
//! posé quand une paire existe.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test vle_relation_anonyme
//! ```

#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:N {name: 'a'})-[:T]->(b:N {name: 'b'})-[:T]->(c:N {name: 'c'}), \
             (c)-[:T]->(a)",
        )
        .expect("graphe");
    db
}

/// Le plan LOGIQUE (sa forme `Debug`) : `EXPLAIN` n'affiche pas la
/// variable d'arête d'un `Expand`, le plan la porte (`edge_variable`).
fn plan_cypher(requete: &str) -> String {
    format!(
        "{:?}",
        obrain_engine::query::translators::cypher::translate(requete).expect("cypher")
    )
}

fn plan_gql(requete: &str) -> String {
    format!(
        "{:?}",
        obrain_engine::query::translators::gql::translate(requete).expect("gql")
    )
}

/// Une relation anonyme a reçu un nom interne : `_anon_N` (Cypher),
/// `_arete_N` (GQL), posé comme `edge_variable` de l'`Expand`.
fn relation_anonyme_nommee(plan: &str) -> bool {
    plan.contains("edge_variable: Some(\"_anon_") || plan.contains("edge_variable: Some(\"_arete_")
}

#[test]
fn cypher_une_relation_variable_seule_n_est_pas_nommee() {
    let plan = plan_cypher("MATCH (a:N)-[:T*1..2]->(b) RETURN count(b)");
    assert!(
        !relation_anonyme_nommee(&plan),
        "aucune paire à confronter : pas de nom interne\n{plan}"
    );
}

#[test]
fn gql_une_relation_variable_seule_n_est_pas_nommee() {
    let plan = plan_gql("MATCH (a:N)-[:T*1..2]->(b) RETURN count(b)");
    assert!(
        !relation_anonyme_nommee(&plan),
        "aucune paire à confronter : pas de nom interne\n{plan}"
    );
}

#[test]
fn deux_relations_variables_sans_relation_simple_ne_sont_pas_nommees() {
    let plan = plan_cypher("MATCH (a:N)-[:T*1..2]->(b)-[:T*1..2]->(c) RETURN count(c)");
    assert!(
        !relation_anonyme_nommee(&plan),
        "seules les paires simple/simple et simple/variable sont confrontées\n{plan}"
    );
}

#[test]
fn temoin_une_paire_garde_son_nommage_et_son_filtre() {
    let plan = plan_cypher("MATCH (a:N)-[:T]->(b)-[:T*1..2]->(c) RETURN count(c)");
    assert!(
        relation_anonyme_nommee(&plan),
        "une relation simple et une variable : le nommage doit rester\n{plan}"
    );
}

#[test]
fn temoin_gql_une_paire_garde_son_nommage() {
    let plan = plan_gql("MATCH (a:N)-[:T]->(b)-[:T*1..2]->(c) RETURN count(c)");
    assert!(
        relation_anonyme_nommee(&plan),
        "une relation simple et une variable : le nommage doit rester\n{plan}"
    );
}

/// Le résultat ne change pas : chemins de 1 à 2 sauts sans arête répétée
/// sur le triangle a→b→c→a, 3 départs × 2 longueurs = 6.
#[test]
fn le_comptage_reste_celui_de_trail() {
    let db = base();
    for (langage, r) in [
        (
            "cypher",
            db.session()
                .execute_cypher("MATCH (a:N)-[:T*1..2]->(b) RETURN count(b)")
                .expect("cypher"),
        ),
        (
            "gql",
            db.session()
                .execute("MATCH (a:N)-[:T*1..2]->(b) RETURN count(b)")
                .expect("gql"),
        ),
    ] {
        assert_eq!(r.rows, vec![vec![Value::Int64(6)]], "{langage}");
    }
}
