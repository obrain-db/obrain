//! Sémantique des chemins NOMMÉS COMPOSÉS, GQL et Cypher (obrain#189).
//!
//! Chaque attendu est tiré À LA MAIN du graphe ci-dessous, jamais copié depuis
//! le moteur. Les lignes sont comparées comme multisets exacts : nœuds (ids) et
//! arêtes (étiquette `k`) par chemin, plus la longueur quand elle est demandée.
//!
//! Graphe :
//!   n -LINKED_TO(l)-> a
//!   a -T(t1)-> b,  b -T(t2)-> a,  a -T(t3)-> c,  a -T(t4)-> b   (t1 et t4 parallèles)
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test semantique_chemins_composes_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const GRAPHE: &str = "CREATE (n:Note {id: 'n'})-[:LINKED_TO {k: 'l'}]->(a:N {id: 'a'}), \
    (a)-[:T {k: 't1'}]->(b:N {id: 'b'}), (b)-[:T {k: 't2'}]->(a), \
    (a)-[:T {k: 't3'}]->(c:N {id: 'c'}), (a)-[:T {k: 't4'}]->(b)";

/// Texte d'une liste de valeurs texte, separees par `sep` (noeuds : `-`,
/// aretes : `,`). Une valeur qui n'est pas une liste ni un texte est une erreur.
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

fn lignes_canoniques(db: &ObrainDB, requete: &str, gql: bool) -> Vec<String> {
    let r = if gql {
        db.execute(requete)
    } else {
        db.session().execute_cypher(requete)
    }
    .unwrap_or_else(|e| panic!("{requete}\n{e}"));
    let mut lignes: Vec<String> = r
        .rows
        .iter()
        .map(|l| {
            let nb = l.len();
            let noeuds = texte_liste(&l[nb - 2], "-");
            let aretes = texte_liste(&l[nb - 1], ",");
            format!("{noeuds}|{aretes}")
        })
        .collect();
    lignes.sort();
    lignes
}

fn attendu(mut v: Vec<&str>) -> Vec<String> {
    v.sort_unstable();
    v.into_iter().map(String::from).collect()
}

const CHEMIN: &str = "[x IN nodes(p) | x.id], [r IN relationships(p) | r.k]";

fn requete(motif: &str) -> String {
    format!("{motif} RETURN {CHEMIN}")
}

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session().execute_cypher(GRAPHE).expect("CREATE");
    db
}

// ---------------------------------------------------------------- Cypher

#[test]
fn cypher_fixe_sortant_deux_segments() {
    let db = base();
    // n -l-> a, puis a -T-> : t1 et t4 (vers b), t3 (vers c).
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N)"),
        false,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b|l,t1", "n-a-b|l,t4", "n-a-c|l,t3"]),
        "sortant"
    );
}

#[test]
fn cypher_fixe_entrant() {
    let db = base();
    // Entrant en a : seule t2 (b -> a).
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)<-[:T]-(t:N)"),
        false,
    );
    assert_eq!(r, attendu(vec!["n-a-b|l,t2"]), "entrant");
}

#[test]
fn cypher_fixe_non_oriente() {
    let db = base();
    // Dans les deux sens depuis a : t1, t4 (vers b), t3 (vers c), t2 (b -> a, vue depuis a).
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]-(t:N)"),
        false,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b|l,t1", "n-a-b|l,t4", "n-a-c|l,t3", "n-a-b|l,t2"]),
        "non orienté"
    );
}

#[test]
fn cypher_zero_saut_et_variable_cible_liee() {
    let db = base();
    // Cible a liee avant le motif : zero saut (n-a|l), puis a-b-a en 2 sauts
    // via (t4,t2) et (t1,t2). Longueurs 1 et 3 (LINKED_TO compris).
    let r = lignes_canoniques(
        &db,
        "MATCH (t:N {id: 'a'}) MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T*0..2]->(t) \
         RETURN length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
        false,
    );
    // La longueur n'entre pas dans la comparaison : elle est implicite, car la
    // liste d'arêtes de chaque chemin est comparée (1 arête = longueur 1, 3 = 3).
    assert_eq!(
        r,
        attendu(vec!["n-a|l", "n-a-b-a|l,t4,t2", "n-a-b-a|l,t1,t2"]),
        "zero saut + variable"
    );
}

#[test]
fn cypher_chaine_mixte_trois_segments() {
    let db = base();
    // n -l-> a -T-> b -T-> a : deux chemins (t4,t2) et (t1,t2), longueur 3.
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(u:N)-[:T]->(t:N)"),
        false,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b-a|l,t4,t2", "n-a-b-a|l,t1,t2"]),
        "trois segments"
    );
}

#[test]
fn cypher_unicite_des_aretes_entre_segments() {
    let db = base();
    // Relation isomorphism (Cypher) : aucune arete reprise entre les deux segments.
    // Exclus : a-b-a par (t1,t1), (t4,t4), (t2,t2) et a-c-a par (t3,t3).
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (s:N {id: 'a'})-[:T]-(u)-[:T]-(t)"),
        false,
    );
    assert_eq!(
        r,
        attendu(vec![
            "a-b-a|t4,t2",
            "a-b-a|t4,t1",
            "a-b-a|t1,t2",
            "a-b-a|t1,t4",
            "a-b-a|t2,t4",
            "a-b-a|t2,t1",
        ]),
        "unicité des arêtes entre segments"
    );
}

#[test]
fn cypher_chemins_nommes_distincts_deux_match() {
    let db = base();
    // p : 3 chemins (longueur 2). q : n -l-> a (longueur 1). Deux MATCH : pas
    // d'isomorphisme entre clauses, donc 3 lignes, chacune (2,1).
    let r = db
        .session()
        .execute_cypher(
            "MATCH (n:Note) MATCH p = (n)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
             MATCH q = (n)-[:LINKED_TO]->(s2:N) RETURN length(p), length(q)",
        )
        .expect("requete");
    let couples: Vec<(i64, i64)> = r
        .rows
        .iter()
        .map(|l| match (&l[0], &l[1]) {
            (Value::Int64(a), Value::Int64(b)) => (*a, *b),
            autre => panic!("{autre:?}"),
        })
        .collect();
    assert_eq!(couples.len(), 3, "{couples:?}");
    assert!(couples.iter().all(|c| *c == (2, 1)), "{couples:?}");
}

#[test]
fn cypher_chemins_nommes_meme_motif_meme_arete() {
    let db = base();
    // Un seul MATCH : p et q partagent la relation LINKED_TO ; l'isomorphisme
    // d'aretes du MATCH l'interdit. Attendu : aucune ligne.
    let r = db
        .session()
        .execute_cypher(
            "MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N), \
             q = (n)-[:LINKED_TO]->(s2:N) RETURN length(p)",
        )
        .expect("requete");
    assert_eq!(r.rows.len(), 0, "{:?}", r.rows);
}

#[test]
fn cypher_return_etoile_n_expose_aucun_alias_de_segment() {
    let db = base();
    let r = db
        .session()
        .execute_cypher("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *")
        .expect("requete");
    assert!(
        r.columns.iter().all(|c| !c.contains("__seg")),
        "colonnes exposees : {:?}",
        r.columns
    );
    assert!(r.columns.contains(&"p".to_string()), "{:?}", r.columns);
}

// ------------------------------------------------------------------ GQL

#[test]
fn gql_fixe_sortant_deux_segments() {
    let db = base();
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N)"),
        true,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b|l,t1", "n-a-b|l,t4", "n-a-c|l,t3"]),
        "sortant"
    );
}

#[test]
fn gql_fixe_entrant() {
    let db = base();
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)<-[:T]-(t:N)"),
        true,
    );
    assert_eq!(r, attendu(vec!["n-a-b|l,t2"]), "entrant");
}

#[test]
fn gql_fixe_non_oriente() {
    let db = base();
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]-(t:N)"),
        true,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b|l,t1", "n-a-b|l,t4", "n-a-c|l,t3", "n-a-b|l,t2"]),
        "non orienté"
    );
}

#[test]
fn gql_zero_saut_et_variable_cible_liee() {
    let db = base();
    let r = lignes_canoniques(
        &db,
        "MATCH (t:N {id: 'a'}) MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T*0..2]->(t) \
         RETURN length(p), [x IN nodes(p) | x.id], [r IN relationships(p) | r.k]",
        true,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a|l", "n-a-b-a|l,t4,t2", "n-a-b-a|l,t1,t2"]),
        "zero saut + variable (GQL)"
    );
}

#[test]
fn gql_chaine_mixte_trois_segments() {
    let db = base();
    let r = lignes_canoniques(
        &db,
        &requete("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(u:N)-[:T]->(t:N)"),
        true,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b-a|l,t4,t2", "n-a-b-a|l,t1,t2"]),
        "trois segments"
    );
}

#[test]
fn gql_mode_defaut_et_trail_uniques_entre_segments() {
    let db = base();
    // DIFFERENT EDGES (defaut) et TRAIL : aucune arete reprise, les 6 chemins.
    let attendu_6 = attendu(vec![
        "a-b-a|t4,t2",
        "a-b-a|t4,t1",
        "a-b-a|t1,t2",
        "a-b-a|t1,t4",
        "a-b-a|t2,t4",
        "a-b-a|t2,t1",
    ]);
    for mode in ["", "TRAIL "] {
        let r = lignes_canoniques(
            &db,
            &requete(&format!(
                "MATCH {mode}p = (s:N {{id: 'a'}})-[:T]-(u)-[:T]-(t)"
            )),
            true,
        );
        assert_eq!(r, attendu_6, "mode {mode:?}");
    }
}

#[test]
fn gql_simple_autorise_le_cycle_qui_revient_a_sa_source() {
    let db = base();
    // SIMPLE : aucun noeud repete, sauf le cycle source = cible (a-b-a).
    let r = lignes_canoniques(
        &db,
        &requete("MATCH SIMPLE p = (s:N {id: 'a'})-[:T]-(u)-[:T]-(t)"),
        true,
    );
    assert_eq!(r.len(), 6, "{r:?}");
}

#[test]
fn gql_acyclic_interdit_toute_repetition_de_noeud_entre_segments() {
    let db = base();
    // ACYCLIC : a-b-a repete le noeud a ; aucun chemin de ce motif.
    let r = lignes_canoniques(
        &db,
        &requete("MATCH ACYCLIC p = (s:N {id: 'a'})-[:T]-(u)-[:T]-(t)"),
        true,
    );
    assert_eq!(r, Vec::<String>::new(), "acyclic : {r:?}");
}

#[test]
fn gql_acyclic_chaine_sans_repetition_garde_ses_chemins() {
    let db = base();
    let r = lignes_canoniques(
        &db,
        &requete("MATCH ACYCLIC p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N)"),
        true,
    );
    assert_eq!(
        r,
        attendu(vec!["n-a-b|l,t1", "n-a-b|l,t4", "n-a-c|l,t3"]),
        "acyclic sans repetition"
    );
}

#[test]
fn gql_walk_repeatable_elements_autorise_la_reprise() {
    let db = base();
    // Sans DIFFERENT EDGES, WALK autorise la reprise d'une arete : 3 choix en b
    // (t1, t4, t2) pour chacun des 3 premiers choix, plus t3 puis t3 : 10 chemins.
    let r = lignes_canoniques(
        &db,
        &requete("MATCH REPEATABLE ELEMENTS p = (s:N {id: 'a'})-[:T]-(u)-[:T]-(t)"),
        true,
    );
    assert_eq!(r.len(), 10, "{r:?}");
    assert!(r.contains(&"a-c-a|t3,t3".to_string()), "{r:?}");
    assert!(r.contains(&"a-b-a|t1,t1".to_string()), "{r:?}");
}

#[test]
fn gql_walk_seul_garde_l_unicite_d_arete_du_match() {
    let db = base();
    // ISO : DIFFERENT EDGES est le mode de correspondance par defaut d'un MATCH ;
    // le mode de chemin WALK ne le leve pas. Attendu : les 6 chemins sans reprise.
    let r = lignes_canoniques(
        &db,
        &requete("MATCH WALK p = (s:N {id: 'a'})-[:T]-(u)-[:T]-(t)"),
        true,
    );
    assert_eq!(r.len(), 6, "{r:?}");
}

#[test]
fn gql_chemins_nommes_distincts_deux_match() {
    let db = base();
    let r = db
        .execute(
            "MATCH (n:Note) MATCH p = (n)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
             MATCH q = (n)-[:LINKED_TO]->(s2:N) RETURN length(p), length(q)",
        )
        .expect("requete");
    assert_eq!(r.rows.len(), 3, "{:?}", r.rows);
}

#[test]
fn gql_return_etoile_n_expose_aucun_alias_de_segment() {
    let db = base();
    let r = db
        .execute("MATCH p = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) RETURN *")
        .expect("requete");
    assert!(
        r.columns.iter().all(|c| !c.contains("__seg")),
        "colonnes exposees : {:?}",
        r.columns
    );
}
