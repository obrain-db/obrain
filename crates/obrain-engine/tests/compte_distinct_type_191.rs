//! `count(DISTINCT type(r))` : raccourci par cardinalité de dictionnaire (obrain#191).
//!
//! Avant : le planneur écartait la forme dès l'extraction de `count_var` — une
//! expression `type(r)` n'est pas une `Variable` — donc la requête balayait
//! toutes les arêtes pour en déduire le nombre de types distincts. Sur l'export
//! PO du 17/11 (6 013 429 arêtes) : **20 134 ms**.
//!
//! Après : le cardinal des types de degré > 0 se lit dans
//! `edge_counts_by_type()`. Mesuré **35,8 ms**, résultat identique (80), et
//! 14× plus rapide que Neo4j (502 ms) sur le même graphe.
//!
//! Ce fichier vérifie la correction, pas la vitesse : un test chronométré serait
//! le flake qu'on vient de passer deux jours à traquer.
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Trois arêtes `:R` et deux `:S`, sur deux nœuds `:A` et `:B`.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:A {k: 'a'}), (b:B {k: 'b'}), \
                    (c:C {k: 'c'}), \
                    (a)-[:R]->(b), (a)-[:R]->(c), (b)-[:R]->(c), \
                    (a)-[:S]->(b), (a)-[:S]->(c)",
        )
        .unwrap();
    db
}

fn entier(db: &ObrainDB, requete: &str) -> i64 {
    let lignes = db
        .session()
        .execute_cypher(requete)
        .unwrap_or_else(|erreur| panic!("{requete}\n{erreur}"))
        .rows;
    match lignes.first().and_then(|l| l.first()) {
        Some(Value::Int64(valeur)) => *valeur,
        autre => panic!("entier attendu pour {requete}, reçu {autre:?}"),
    }
}

/// Le raccourci : deux types distincts (`R` et `S`).
#[test]
fn compte_les_types_distincts() {
    let db = base();
    assert_eq!(
        entier(&db, "MATCH ()-[r]->() RETURN count(DISTINCT type(r)) AS c"),
        2
    );
}

/// Restreint à un type : une seule valeur distincte.
#[test]
fn compte_un_type_restreint() {
    let db = base();
    assert_eq!(
        entier(
            &db,
            "MATCH ()-[r:R]->() RETURN count(DISTINCT type(r)) AS c"
        ),
        1
    );
}

/// Témoin : `count(r)` sans `distinct` compte les ARÊTES, pas les types.
#[test]
fn count_sans_distinct_reste_le_nombre_d_aretes() {
    let db = base();
    assert_eq!(entier(&db, "MATCH ()-[r]->() RETURN count(r) AS c"), 5);
}

/// Témoin : la forme groupée, qui passait déjà par son propre raccourci.
#[test]
fn forme_groupee_inchangee() {
    let db = base();
    let lignes = db
        .session()
        .execute_cypher("MATCH ()-[r]->() RETURN type(r) AS t, count(*) AS n")
        .unwrap()
        .rows;
    assert_eq!(lignes.len(), 2, "deux types : R et S");
    let total: i64 = lignes
        .iter()
        .filter_map(|l| match l.get(1) {
            Some(Value::Int64(n)) => Some(*n),
            _ => None,
        })
        .sum();
    assert_eq!(total, 5);
}

/// Témoin : un agrégat `DISTINCT` sur une propriété garde son chemin normal.
#[test]
fn distinct_sur_propriete_non_raccourci() {
    let db = base();
    assert_eq!(entier(&db, "MATCH (n) RETURN count(DISTINCT n.k) AS c"), 3);
}
