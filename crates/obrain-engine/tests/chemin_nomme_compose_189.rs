//! Chemin nommé composé : un saut fixe suivi d'un `*0..n` dans UN SEUL motif
//! (obrain-db/obrain#189, forme exacte de `get_propagated_notes` côté PO) :
//! `MATCH path = (n:Note)-[:LINKED_TO]->(source)-[:T*0..k]->(target)`.
//!
//! Les dumps v7 montraient `distance = length(path) - 1 = -1`, `relation_path = []`,
//! `nodes(path)` d'un seul élément : le saut `LINKED_TO` n'apparaissait pas dans le
//! chemin nommé. Ces tests fixent la sémantique attendue (openCypher) :
//! - `length(p)` = nombre d'arêtes du chemin, saut fixe compris (>= 1 ici) ;
//! - `nodes(p)` = `length(p) + 1` nœuds, `relationships(p)` = `length(p)` arêtes ;
//! - zéro saut variable : `source = target`, chemin = `n -> source`, longueur 1.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test chemin_nomme_compose_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// n -LINKED_TO-> a ; a -T-> b ; b -T-> a.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (n:Note {id: 'n'})-[:LINKED_TO]->(a:N {id: 'a'}), \
             (a)-[:T]->(b:N {id: 'b'}), (b)-[:T]->(a)",
        )
        .expect("CREATE");
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    let mut rows = db
        .session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows;
    rows.sort_by_key(|l| format!("{l:?}"));
    rows
}

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, reçu {autre:?}"),
    }
}

/// Motif PO complet, cible liée avant le motif, une seule clause MATCH.
/// Chemins attendus depuis n vers la cible a :
/// - n -> a                 (zéro saut variable)      : length 1, nodes 2, rels 1
/// - n -> a -> b -> a       (deux sauts variables)    : length 3, nodes 4, rels 3
#[test]
fn chemin_compose_longueur_inclut_le_saut_fixe() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH (target:N {id: 'a'}) \
         MATCH path = (n:Note)-[:LINKED_TO]->(source)-[:T*0..2]->(target) \
         RETURN length(path), size(nodes(path)), size(relationships(path))",
    );
    let trouves: Vec<(i64, i64, i64)> = r
        .iter()
        .map(|l| (entier(&l[0]), entier(&l[1]), entier(&l[2])))
        .collect();
    assert_eq!(
        trouves,
        vec![(1, 2, 1), (3, 4, 3)],
        "longueurs de chemin composé (lignes brutes : {r:?})"
    );
}

/// Même motif, sans cible liée : la forme qui rend `distance` négatif en PO.
#[test]
fn chemin_compose_sans_cible_liee() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH path = (n:Note)-[:LINKED_TO]->(source)-[:T*0..2]->(target) \
         RETURN length(path), size(relationships(path))",
    );
    let longueurs: Vec<i64> = r.iter().map(|l| entier(&l[0])).collect();
    assert!(
        longueurs.iter().all(|&l| l >= 1),
        "aucun chemin composé ne doit avoir une longueur nulle ou négative : {r:?}"
    );
    assert!(
        r.iter().all(|l| entier(&l[0]) == entier(&l[1])),
        "length(p) doit égaler le nombre d'arêtes de p : {r:?}"
    );
}

/// Deux sauts FIXES dans un seul motif, sans `*` : si la longueur vaut 1 au lieu
/// de 2, la cause est la collision des colonnes `_path_*_path` entre expansions,
/// indépendante de la longueur variable.
#[test]
fn chemin_compose_deux_sauts_fixes() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH path = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
         RETURN length(path), size(nodes(path)), size(relationships(path))",
    );
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(
        (entier(&r[0][0]), entier(&r[0][1]), entier(&r[0][2])),
        (2, 3, 2),
        "n -> a -> b : deux arêtes, trois nœuds ({r:?})"
    );
}

/// Témoin : même chemin découpé en deux MATCH, chacun avec son propre chemin.
#[test]
fn temoin_deux_match_sans_collision() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH (n:Note)-[:LINKED_TO]->(s:N) MATCH path = (s)-[:T*0..2]->(t) \
         RETURN length(path)",
    );
    let longueurs: Vec<i64> = r.iter().map(|l| entier(&l[0])).collect();
    assert_eq!(longueurs.len(), 3, "{r:?}");
}

/// Témoin : sans saut fixe, `*0..0` reste une longueur 0 sans arête.
#[test]
fn temoin_zero_saut_seul_reste_longueur_zero() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'}) MATCH p = (a)-[:T*0..0]->(b) \
         RETURN length(p), size(relationships(p)), size(nodes(p))",
    );
    assert_eq!(r.len(), 1);
    assert_eq!(entier(&r[0][0]), 0);
    assert_eq!(entier(&r[0][1]), 0);
    assert_eq!(entier(&r[0][2]), 1);
}

/// Témoin : un chemin nommé à saut fixe seul.
#[test]
fn temoin_saut_fixe_seul() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH path = (n:Note)-[:LINKED_TO]->(source) \
         RETURN length(path), size(nodes(path)), size(relationships(path))",
    );
    assert_eq!(r.len(), 1);
    assert_eq!(
        (entier(&r[0][0]), entier(&r[0][1]), entier(&r[0][2])),
        (1, 2, 1)
    );
}

/// Trois segments enchaines : `n -> a -> b -> a` (LINKED_TO, T, T). Le prefixe
/// s'accumule sur deux expansions successives.
#[test]
fn chemin_compose_trois_segments() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH path = (n:Note)-[:LINKED_TO]->(s:N)-[:T]->(u:N)-[:T]->(t:N) \
         RETURN length(path), size(nodes(path)), size(relationships(path))",
    );
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(
        (entier(&r[0][0]), entier(&r[0][1]), entier(&r[0][2])),
        (3, 4, 3),
        "{r:?}"
    );
}

/// Sens entrant dans la chaine : `n -> a`, puis `t -T-> a` (zero saut : t = a ;
/// un saut : t = b, car b -> a). Longueurs 1 et 2.
#[test]
fn chemin_compose_sens_entrant() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH path = (n:Note)-[:LINKED_TO]->(s:N)<-[:T*0..1]-(t:N) \
         RETURN length(path), size(nodes(path))",
    );
    let trouves: Vec<(i64, i64)> = r.iter().map(|l| (entier(&l[0]), entier(&l[1]))).collect();
    assert_eq!(trouves, vec![(1, 2), (2, 3)], "{r:?}");
}

/// Deux chemins nommes distincts dans un meme texte : chaque colonne de
/// chemin garde sa propre longueur (pas de collision entre `p` et `q`).
#[test]
fn deux_chemins_nommes_distincts() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH (n:Note) \
         MATCH p = (n)-[:LINKED_TO]->(s:N)-[:T]->(t:N) \
         MATCH q = (n)-[:LINKED_TO]->(s2:N) \
         RETURN length(p), length(q)",
    );
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!((entier(&r[0][0]), entier(&r[0][1])), (2, 1), "{r:?}");
}
