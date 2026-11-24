//! obrain#376 : une chaîne de deux `Expand` simples sans filtre est planifiée en
//! `LazyFactorizedChainOperator`. Rejouée par une exécution corrélée (`Apply`,
//! `count{}`, compréhension de motif), elle rendait vide : `reset` ne réarmait pas
//! la source. Chaque requête est comparée à sa formulation SANS chaîne (un label
//! sur le nœud intermédiaire pose un `Filter` qui casse la chaîne) et à une valeur
//! écrite à la main.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test reset_chaine_factorisee_376
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn graphe() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let mk = |cle: &str| session.create_node_with_props(&["N"], [("k", Value::String(cle.into()))]);
    let (na, nb, nc, nd, ne) = (mk("A"), mk("B"), mk("C"), mk("D"), mk("E"));
    // C->B->A ; D->B ; E->A (E n'a pas de prédécesseur)
    session.create_edge(nb, na, "R");
    session.create_edge(nc, nb, "R");
    session.create_edge(nd, nb, "R");
    session.create_edge(ne, na, "R");
    db
}

fn lignes(db: &ObrainDB, q: &str) -> String {
    let r = db
        .session()
        .execute_cypher(q)
        .unwrap_or_else(|e| panic!("requête refusée : {q}\n{e}"));
    let mut v: Vec<String> = r.rows.iter().map(|l| format!("{l:?}")).collect();
    v.sort();
    v.join(" | ")
}

/// `chaine` (deux sauts sans filtre) doit rendre `attendu`, comme `temoin`
/// (même motif avec `m:N`, donc sans chaîne factorisée).
fn verifie(db: &ObrainDB, chaine: &str, temoin: &str, attendu: &str) {
    assert_eq!(lignes(db, temoin), attendu, "témoin : {temoin}");
    assert_eq!(lignes(db, chaine), attendu, "chaîne : {chaine}");
}

#[test]
fn call_correle_compte_a_travers_la_chaine() {
    let db = graphe();
    verifie(
        &db,
        "MATCH (p:N) CALL { WITH p MATCH (p)<-[]-(m)<-[]-(q) RETURN count(*) AS c } RETURN p.k, c",
        "MATCH (p:N) CALL { WITH p MATCH (p)<-[]-(m:N)<-[]-(q) RETURN count(*) AS c } RETURN p.k, c",
        r#"[String("A"), Int64(2)] | [String("B"), Int64(0)] | [String("C"), Int64(0)] | [String("D"), Int64(0)] | [String("E"), Int64(0)]"#,
    );
}

#[test]
fn call_correle_rend_les_lignes_de_la_chaine() {
    let db = graphe();
    verifie(
        &db,
        "MATCH (p:N) CALL { WITH p MATCH (p)<-[]-(m)<-[]-(q) RETURN q.k AS qk } RETURN p.k, qk",
        "MATCH (p:N) CALL { WITH p MATCH (p)<-[]-(m:N)<-[]-(q) RETURN q.k AS qk } RETURN p.k, qk",
        r#"[String("A"), String("C")] | [String("A"), String("D")]"#,
    );
}

#[test]
fn count_sous_requete_a_travers_la_chaine() {
    let db = graphe();
    verifie(
        &db,
        "MATCH (p:N) RETURN p.k, count{(p)<-[]-(m)<-[]-(q)} AS c",
        "MATCH (p:N) RETURN p.k, count{(p)<-[]-(m:N)<-[]-(q)} AS c",
        r#"[String("A"), Int64(2)] | [String("B"), Int64(0)] | [String("C"), Int64(0)] | [String("D"), Int64(0)] | [String("E"), Int64(0)]"#,
    );
}

#[test]
fn comprehension_de_motif_a_travers_la_chaine() {
    let db = graphe();
    verifie(
        &db,
        "MATCH (p:N) RETURN p.k, size([(p)<-[]-(m)<-[]-(q) | q.k]) AS n",
        "MATCH (p:N) RETURN p.k, size([(p)<-[]-(m:N)<-[]-(q) | q.k]) AS n",
        r#"[String("A"), Int64(2)] | [String("B"), Int64(0)] | [String("C"), Int64(0)] | [String("D"), Int64(0)] | [String("E"), Int64(0)]"#,
    );
}

#[test]
fn exists_et_optional_match_restent_justes() {
    let db = graphe();
    verifie(
        &db,
        "MATCH (p:N) WHERE EXISTS { (p)<-[]-(m)<-[]-(q) } RETURN p.k",
        "MATCH (p:N) WHERE EXISTS { (p)<-[]-(m:N)<-[]-(q) } RETURN p.k",
        r#"[String("A")]"#,
    );
    verifie(
        &db,
        "MATCH (p:N) OPTIONAL MATCH (p)<-[]-(m)<-[]-(q) RETURN p.k, q.k",
        "MATCH (p:N) OPTIONAL MATCH (p)<-[]-(m:N)<-[]-(q) RETURN p.k, q.k",
        r#"[String("A"), String("C")] | [String("A"), String("D")] | [String("B"), Null] | [String("C"), Null] | [String("D"), Null] | [String("E"), Null]"#,
    );
}

#[test]
fn unwind_a_travers_la_chaine() {
    let db = graphe();
    verifie(
        &db,
        "UNWIND ['A','B'] AS x MATCH (p:N {k:x})<-[]-(m)<-[]-(q) RETURN x, q.k",
        "UNWIND ['A','B'] AS x MATCH (p:N {k:x})<-[]-(m:N)<-[]-(q) RETURN x, q.k",
        r#"[String("A"), String("C")] | [String("A"), String("D")]"#,
    );
}
