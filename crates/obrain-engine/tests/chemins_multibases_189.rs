//! Chemins nommés composés à travers deux bases (obrain#189, revue).
//!
//! Les listes de chemin ne portent que des identifiants BRUTS, sans base. Deux
//! nœuds de bases distinctes peuvent donc avoir le même identifiant brut. La
//! déduplication TRAIL / SIMPLE / ACYCLIC entre segments ne doit jamais les
//! confondre : le planificateur refuse ces modes pour un chemin composé quand un
//! registre multi-bases est attaché. WALK, qui ne déduplique pas, reste exact.
//!
//! Précondition vérifiée par le test : le nœud `Note` (alpha) et le nœud `Doc`
//! (beta) ont le même identifiant brut. Sans cette précondition, le test échoue
//! au lieu de passer à vide.
//!
//! ```bash
//! cargo test -p obrain-engine --features gql --test chemins_multibases_189
//! ```
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// alpha : Note n -[:L]-> Hub h -[:T]-> ref(beta, doc_1). beta : Doc doc_1.
fn deux_bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', titre: 'Guide'})")
        .expect("insert beta");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("note");
    sa.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("arete L");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"doc_1\"))")
        .expect("arete externe T");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

/// Identifiant brut d'un nœud, lu dans une base.
fn id_brut(db: &ObrainDB, requete: &str) -> i64 {
    let r = db.session().execute(requete).expect("lecture id");
    match &r.rows[0][0] {
        Value::Int64(i) => *i,
        autre => panic!("identifiant attendu : {autre:?}"),
    }
}

#[test]
fn precondition_identifiants_bruts_identiques() {
    let (alpha, beta) = deux_bases();
    let note = id_brut(&alpha, "MATCH (n:Note) RETURN id(n)");
    let doc = id_brut(&beta, "MATCH (d:Doc) RETURN id(d)");
    assert_eq!(note, doc, "fixture invalide : identifiants bruts distincts");
}

#[test]
fn walk_reste_exact_a_travers_les_bases() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p)")
        .expect("walk multi-bases");
    assert_eq!(r.rows.len(), 1, "un chemin attendu : {:?}", r.rows);
}

#[test]
fn trail_fixe_reste_exact_unicite_d_arete_par_identite() {
    // TRAIL sur un chemin à sauts fixes : le traducteur ramène chaque segment à un
    // saut en WALK ; l'unicité d'arête est portée par le filtre de MATCH, qui compare
    // les arêtes par identité (EdgeId primaire, Map {_id, _base} distante). Pas de
    // déduplication sur les listes de chemin : résultat exact, une ligne.
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH TRAIL p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p)")
        .expect("trail fixe multi-bases");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
}

#[test]
fn simple_et_acyclic_composes_refuses_explicitement_avec_registre() {
    // SIMPLE et ACYCLIC gardent leur mode par segment : le filtre composé, qui
    // déduplique les identifiants bruts, est actif. Refus explicite attendu.
    let (alpha, _beta) = deux_bases();
    for mode in ["SIMPLE", "ACYCLIC"] {
        let requete = format!("MATCH {mode} p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p)");
        let erreur = match alpha.session().execute(&requete) {
            Ok(_) => panic!("{mode} doit être refusé, pas rendre un résultat faux"),
            Err(e) => e,
        };
        assert!(
            format!("{erreur}").contains("multi-bases"),
            "{mode} : message de refus inattendu : {erreur}"
        );
    }
}

#[test]
fn controle_mono_base_acyclic_inchange() {
    // Même forme, sans registre : pas de refus, le chemin est rendu.
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:Note {name: 'n'})").expect("note");
    s.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    s.execute("INSERT (:Doc {id: 'doc_1'})").expect("doc");
    s.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    s.execute("MATCH (h:Hub), (d:Doc) CREATE (h)-[:T]->(d)")
        .expect("T");
    let r = db
        .session()
        .execute("MATCH ACYCLIC p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p)")
        .expect("acyclic mono-base");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
}
