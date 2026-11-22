//! Chemins nommés composés et registre multi-bases (obrain#189, revue).
//!
//! Les listes de chemin (`Value::Path`) ne portent que des identifiants BRUTS, sans
//! base. Leur projection (`[n IN nodes(p) | n.x]`) résout un identifiant dans le store
//! PRIMAIRE : un nœud distant de même identifiant brut renvoie la propriété d'un nœud
//! primaire. La garde du planificateur refuse donc tout chemin nommé composé dès qu'un
//! registre multi-bases est attaché. Ces tests ne prouvent PAS l'exactitude
//! multi-bases : ils prouvent le refus, et l'exactitude mono-base.
//!
//! ```bash
//! cargo test -p obrain-engine --features gql --test chemins_multibases_189
//! ```
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// alpha : Note n -[:L]-> Hub h -[:T]-> ref(beta, doc_1). beta : Doc doc_1 (titre distinct).
fn deux_bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', name: 'remote'})")
        .expect("insert beta");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'primaire-n'})")
        .expect("note");
    sa.execute("INSERT (:Hub {name: 'primaire-h'})")
        .expect("hub");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("arete L");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"doc_1\"))")
        .expect("arete externe T");
    sa.execute("INSERT (:Zed {name: 'primaire-z'})")
        .expect("zed");
    sa.execute("MATCH (h:Hub), (z:Zed) CREATE (h)-[:M]->(z)")
        .expect("arete primaire M");
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
fn precondition_fixture_identifiants_bruts_identiques() {
    // Sans cette coïncidence, le scénario de collision n'existe pas et le test échoue.
    let (alpha, beta) = deux_bases();
    let note = id_brut(&alpha, "MATCH (n:Note) RETURN id(n)");
    let doc = id_brut(&beta, "MATCH (d:Doc) RETURN id(d)");
    assert_eq!(note, doc, "fixture invalide : identifiants bruts distincts");
}

#[test]
fn composes_refuses_pour_tous_les_modes_avec_registre() {
    // Refus conservateur : WALK, TRAIL, SIMPLE, ACYCLIC. Le graphe ici franchit une
    // frontière, mais la garde ne le vérifie pas (conservateur, voulu).
    let (alpha, _beta) = deux_bases();
    for mode in ["WALK", "TRAIL", "SIMPLE", "ACYCLIC"] {
        let requete = format!("MATCH {mode} p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p)");
        let erreur = match alpha.session().execute(&requete) {
            Ok(_) => panic!("{mode} : chemin composé avec registre doit être refusé"),
            Err(e) => e,
        };
        assert!(
            format!("{erreur}").contains("multi-bases"),
            "{mode} : message de refus inattendu : {erreur}"
        );
    }
}

#[test]
fn composes_refuses_meme_si_le_parcours_reste_primaire() {
    // Chemin composé entièrement dans la base PRIMAIRE (Note -L-> Hub -M-> Zed), registre
    // attaché : refus quand même. C'est le coût assumé de la garde conservatrice.
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:M]->(z:Zed) RETURN length(p)");
    assert!(
        r.is_err(),
        "garde conservatrice : refus attendu même primaire"
    );
}

#[test]
fn controle_mono_base_composes_projection_exacte() {
    // Sans registre : composé, projection des noms DANS L'ORDRE du chemin, exacte.
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:Note {name: 'n'})").expect("note");
    s.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    s.execute("INSERT (:Doc {name: 'd'})").expect("doc");
    s.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    s.execute("MATCH (h:Hub), (d:Doc) CREATE (h)-[:T]->(d)")
        .expect("T");
    let r = db
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) \
             RETURN length(p), [x IN nodes(p) | x.name]",
        )
        .expect("composé mono-base");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(r.rows[0][0], Value::Int64(2), "longueur");
    assert_eq!(
        r.rows[0][1],
        Value::List(
            vec![
                Value::String("n".into()),
                Value::String("h".into()),
                Value::String("d".into()),
            ]
            .into()
        ),
        "noms dans l'ordre du chemin"
    );
}
