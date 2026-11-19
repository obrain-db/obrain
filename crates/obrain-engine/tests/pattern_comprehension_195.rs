//! Compréhension de motif — `[(n)-[:R]->(m) | m.v]` (obrain#195).
//!
//! La famille était comptée à 6 requêtes en échec dans le ticket, avec pour
//! message « PatternComprehension reached the planner without being rewritten ».
//! Ces six formes-là ne sont pas fournies : le banc des 325 requêtes PO vit dans
//! un `TRIAGE_DIVERGENCES.md` qui n'existe sur aucune machine accessible.
//!
//! Ce fichier épingle donc les formes **voisines**, constatées correctes sur
//! `main` le 19/11 — pour qu'une régression sur la réécriture se voie. Il ne
//! prétend pas couvrir les six requêtes du ticket : chacune des fonctions
//! ci-dessous est une réduction écrite à la main.
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// `a` a deux voisins `:B` (v = 1) et (v = 2), plus un `:C` non retenu.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:A {k: 'a'})-[:R]->(b:B {v: 1}), \
                    (a)-[:R]->(c:B {v: 2}), \
                    (a)-[:S]->(d:C {v: 9})",
        )
        .unwrap();
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

fn taille_liste(db: &ObrainDB, requete: &str) -> i64 {
    let l = lignes(db, requete);
    match &l[0][0] {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, reçu {autre:?}"),
    }
}

/// Forme nue : projection d'une propriété sur les motifs retenus.
#[test]
fn projection_simple() {
    let db = base();
    let n = taille_liste(&db, "MATCH (a:A) RETURN size([(a)-[:R]->(m:B) | m.v]) AS n");
    assert_eq!(n, 2, "deux arêtes :R, le :S ne doit pas entrer");
}

/// Avec filtre : le `WHERE` de la compréhension réduit la liste.
#[test]
fn avec_filtre() {
    let db = base();
    let n = taille_liste(
        &db,
        "MATCH (a:A) RETURN size([(a)-[:R]->(m:B) WHERE m.v > 1 | m.v]) AS n",
    );
    assert_eq!(n, 1);
}

/// Dans un `WITH` : la valeur traverse la projection.
#[test]
fn dans_un_with() {
    let db = base();
    let l = lignes(
        &db,
        "MATCH (a:A) WITH a, [(a)-[:R]->(m:B) | m.v] AS vs RETURN a.k AS k, size(vs) AS n",
    );
    assert_eq!(l.len(), 1);
    match (&l[0][0], &l[0][1]) {
        (Value::String(k), Value::Int64(n)) => {
            assert_eq!(k.to_string(), "a");
            assert_eq!(*n, 2);
        }
        autre => panic!("(chaîne, entier) attendu, reçu {autre:?}"),
    }
}

/// Témoin : un motif sans arête sortante rend une liste vide, pas une erreur.
#[test]
fn liste_vide_sans_arete() {
    let db = base();
    let n = taille_liste(&db, "MATCH (b:B) RETURN size([(b)-[:R]->(m) | m.v]) AS n");
    assert_eq!(n, 0);
}

/// Témoin : la projection peut rendre la valeur d'un nœud, pas seulement un scalaire.
#[test]
fn projection_d_un_noeud_entier() {
    let db = base();
    let l = lignes(&db, "MATCH (a:A) RETURN [(a)-[:R]->(m:B) | m] AS ms");
    match &l[0][0] {
        Value::List(v) => assert_eq!(v.len(), 2),
        autre => panic!("liste attendue, reçu {autre:?}"),
    }
}
