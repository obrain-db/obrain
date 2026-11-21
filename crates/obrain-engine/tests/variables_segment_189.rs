//! Variables UTILISATEUR de forme `<base>__seg<chiffres>` (obrain#189).
//!
//! Le chemin nommé composé produit des alias internes par segment. Une variable
//! utilisateur qui porte le même nom ne doit ni être masquée par `RETURN *` /
//! `WITH *`, ni entrer en collision avec le prefixe d'un chemin. Les oracles
//! sont tires a la main du graphe :
//!   n -l-> a ; a -t1-> b ; b -t2-> a ; a -t3-> c ; a -t4-> b.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test variables_segment_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const GRAPHE: &str = "CREATE (n:Note {id: 'n'})-[:LINKED_TO {k: 'l'}]->(a:N {id: 'a'}), \
    (a)-[:T {k: 't1'}]->(b:N {id: 'b'}), (b)-[:T {k: 't2'}]->(a), \
    (a)-[:T {k: 't3'}]->(c:N {id: 'c'}), (a)-[:T {k: 't4'}]->(b)";

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session().execute_cypher(GRAPHE).expect("CREATE");
    db
}

fn texte_liste(v: &Value, sep: &str) -> String {
    match v {
        Value::String(s) => s.to_string(),
        Value::List(items) => items
            .iter()
            .map(|x| match x {
                Value::String(s) => s.to_string(),
                autre => panic!("element inattendu : {autre:?}"),
            })
            .collect::<Vec<_>>()
            .join(sep),
        autre => panic!("valeur inattendue : {autre:?}"),
    }
}

fn execute(db: &ObrainDB, gql: bool, requete: &str) -> obrain_engine::database::QueryResult {
    if gql {
        db.execute(requete)
    } else {
        db.session().execute_cypher(requete)
    }
    .unwrap_or_else(|e| panic!("{requete}\n{e}"))
}

/// Lignes `"<valeur user>|<longueur>|<noeuds>|<aretes>"`, triees.
fn lignes(db: &ObrainDB, gql: bool, requete: &str) -> Vec<String> {
    let r = execute(db, gql, requete);
    let mut out: Vec<String> = r
        .rows
        .iter()
        .map(|l| {
            let user = texte_liste(&l[0], "");
            let longueur = match &l[1] {
                Value::Int64(i) => i.to_string(),
                autre => panic!("longueur inattendue : {autre:?}"),
            };
            format!(
                "{user}|{longueur}|{}|{}",
                texte_liste(&l[2], "-"),
                texte_liste(&l[3], ",")
            )
        })
        .collect();
    out.sort();
    out
}

fn attendu(mut v: Vec<&str>) -> Vec<String> {
    v.sort_unstable();
    v.into_iter().map(String::from).collect()
}

/// Motif a deux segments, variable utilisateur `p__seg0` liee a `a`.
/// Chemins (p) : n-a-b|l,t4 ; n-a-c|l,t3 ; n-a-b|l,t1. Longueur 2 chacun.
const REQ_DEUX_SEGMENTS: &str = "MATCH (p__seg0:N {id: 'a'}) \
    MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
    RETURN p__seg0.id, length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]";

fn attendu_deux_segments() -> Vec<String> {
    attendu(vec!["a|2|n-a-b|l,t4", "a|2|n-a-c|l,t3", "a|2|n-a-b|l,t1"])
}

// ---------------------------------------------------------------- Cypher

#[test]
fn cypher_variable_utilisateur_p_seg0_avec_chemin_p() {
    let db = base();
    assert_eq!(
        lignes(&db, false, REQ_DEUX_SEGMENTS),
        attendu_deux_segments(),
        "valeur, longueur, noeuds et aretes du chemin p"
    );
}

#[test]
fn cypher_return_etoile_garde_la_variable_utilisateur() {
    let db = base();
    let r = execute(
        &db,
        false,
        "MATCH (p__seg0:N {id: 'a'}) \
         MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *",
    );
    assert!(
        r.columns.contains(&"p__seg0".to_string()),
        "{:?}",
        r.columns
    );
    assert!(r.columns.contains(&"p".to_string()), "{:?}", r.columns);
    assert_eq!(r.rows.len(), 3, "cardinalite");
}

#[test]
fn cypher_with_etoile_garde_la_variable_utilisateur() {
    let db = base();
    let r = lignes(
        &db,
        false,
        "MATCH (p__seg0:N {id: 'a'}) \
         MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
         WITH * RETURN p__seg0.id, length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
    );
    assert_eq!(
        r,
        attendu_deux_segments(),
        "WITH * : valeurs et cardinalite"
    );
}

#[test]
fn cypher_deux_variables_utilisateur_seg_et_chemin_composé_a_trois_segments() {
    let db = base();
    // p__seg0 (a) et p__seg1 (b) sont des variables utilisateur ; le chemin p
    // a trois segments : n -l-> a -t-> b -t-> a. Deux chemins, longueur 3.
    let r = lignes(
        &db,
        false,
        "MATCH (p__seg0:N {id: 'a'}) MATCH (p__seg1:N {id: 'b'}) \
         MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(u:N)-[:T]->(t:N) \
         RETURN p__seg0.id, length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
    );
    assert_eq!(
        r,
        attendu(vec!["a|3|n-a-b-a|l,t4,t2", "a|3|n-a-b-a|l,t1,t2"]),
        "trois segments avec variables utilisateur"
    );
}

#[test]
fn cypher_chemin_nomme_seg_ne_masque_pas_ses_colonnes() {
    // Un chemin dont le NOM est de la forme p__seg0 : ses colonnes ne sont pas
    // des alias internes et doivent apparaitre dans RETURN *.
    let db = base();
    let r = execute(
        &db,
        false,
        "MATCH p__seg0 = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *",
    );
    assert!(
        r.columns.contains(&"p__seg0".to_string()),
        "{:?}",
        r.columns
    );
    assert_eq!(r.rows.len(), 3);
}

// ------------------------------------------------------------------- GQL

#[test]
fn gql_variable_utilisateur_p_seg0_avec_chemin_p() {
    let db = base();
    assert_eq!(
        lignes(&db, true, REQ_DEUX_SEGMENTS),
        attendu_deux_segments(),
        "valeur, longueur, noeuds et aretes du chemin p (GQL)"
    );
}

#[test]
fn gql_return_etoile_garde_la_variable_utilisateur() {
    let db = base();
    let r = execute(
        &db,
        true,
        "MATCH (p__seg0:N {id: 'a'}) \
         MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *",
    );
    assert!(
        r.columns.contains(&"p__seg0".to_string()),
        "{:?}",
        r.columns
    );
    assert!(r.columns.contains(&"p".to_string()), "{:?}", r.columns);
    assert_eq!(r.rows.len(), 3, "cardinalite");
}

#[test]
fn gql_with_etoile_garde_la_variable_utilisateur() {
    let db = base();
    let r = lignes(
        &db,
        true,
        "MATCH (p__seg0:N {id: 'a'}) \
         MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
         WITH * RETURN p__seg0.id, length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
    );
    assert_eq!(
        r,
        attendu_deux_segments(),
        "WITH * : valeurs et cardinalite (GQL)"
    );
}

#[test]
fn gql_chemin_nomme_seg_ne_masque_pas_ses_colonnes() {
    let db = base();
    let r = execute(
        &db,
        true,
        "MATCH p__seg0 = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *",
    );
    assert!(
        r.columns.contains(&"p__seg0".to_string()),
        "{:?}",
        r.columns
    );
    assert_eq!(r.rows.len(), 3);
}

// --------------------------------------------- identifiants avec U+0001 (acceptés)

/// Exécute `f` une fois en Cypher et une fois en GQL.
fn pour_les_deux(f: impl Fn(&ObrainDB, bool)) {
    let db = base();
    f(&db, false);
    f(&db, true);
}

#[test]
fn identifiant_avec_u0001_reste_accepte_et_valeurs_justes() {
    pour_les_deux(|db, gql| {
        // Identifiant cité avec U+0001 : ancien comportement préservé.
        let r = lignes(
            db,
            gql,
            "MATCH (`p\u{1}seg0`:N {id: 'a'}) \
             MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
             RETURN `p\u{1}seg0`.id, length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
        );
        assert_eq!(r, attendu_deux_segments(), "gql={gql}");
    });
}

#[test]
fn collision_adversariale_avec_l_alias_interne_candidat() {
    pour_les_deux(|db, gql| {
        // Le texte porte exactement l'alias interne du segment 0 de `p` dans la
        // forme naive (`p` + U+0001seg0U+0001 + `0`) : le marqueur doit l'éviter.
        let r = lignes(
            db,
            gql,
            "MATCH (`p\u{1}seg0\u{1}0`:N {id: 'a'}) \
             MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
             RETURN `p\u{1}seg0\u{1}0`.id, length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
        );
        assert_eq!(r, attendu_deux_segments(), "gql={gql}");
    });
}

#[test]
fn return_etoile_et_with_etoile_gardent_l_identifiant_avec_u0001() {
    pour_les_deux(|db, gql| {
        let ident = "p\u{1}seg0";
        let r = execute(
            db,
            gql,
            "MATCH (`p\u{1}seg0`:N {id: 'a'}) \
             MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *",
        );
        assert!(
            r.columns.contains(&ident.to_string()),
            "RETURN * {:?}",
            r.columns
        );
        assert!(
            r.columns
                .iter()
                .all(|c| !c.contains("\u{1}seg0\u{1}") || c == ident || c.starts_with('_')),
            "colonne interne exposee : {:?}",
            r.columns
        );
        assert_eq!(r.rows.len(), 3, "cardinalite RETURN *");
        let w = execute(
            db,
            gql,
            "MATCH (`p\u{1}seg0`:N {id: 'a'}) \
             MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) WITH * RETURN *",
        );
        assert!(
            w.columns.contains(&ident.to_string()),
            "WITH * {:?}",
            w.columns
        );
        assert_eq!(w.rows.len(), 3, "cardinalite WITH *");
    });
}

#[test]
fn chemin_nomme_avec_u0001_a_des_segments_justes() {
    pour_les_deux(|db, gql| {
        let r = lignes(
            db,
            gql,
            "MATCH `q\u{1}x` = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
             RETURN 'x' AS v, length(`q\u{1}x`), [x IN nodes(`q\u{1}x`) | x.id], [r IN relationships(`q\u{1}x`) | r.k]",
        );
        assert_eq!(
            r,
            attendu(vec!["x|2|n-a-b|l,t4", "x|2|n-a-c|l,t3", "x|2|n-a-b|l,t1"]),
            "gql={gql}"
        );
    });
}
