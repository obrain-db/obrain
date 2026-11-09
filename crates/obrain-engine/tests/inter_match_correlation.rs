//! Régression exactitude — jointures fermées multi-hop (INCIDENT triangle-faux).
//!
//! Le bug (corrigé RUN 16) : `MATCH (a)→(b)→(c) MATCH (c)→(a)` re-scannait
//! `c`/`a` au lieu de les corréler au 1er MATCH — un triangle retournait
//! 10 778 count(a) au lieu de 33 (faux, ×326). Ce test vérifie que les deux
//! formulations d'un triangle (chemin multi-hop + MATCH de fermeture, vs
//! trois arêtes dans un seul MATCH) retournent le MÊME compte sur un graphe
//! réel avec homonymes (le pire cas qui a exposé le bug).
//!
//! ```bash
//! cargo test -p obrain-engine --test inter_match_correlation --features "gql"
//! ```

use obrain_engine::ObrainDB;

/// Construit un graphe avec des homonymes (le piège du bug) : deux mondes
/// (TS et Rust) partagent des noms de fonctions, avec des triangles dans
/// chaque monde et des arêtes croisées.
fn graph_avec_homonymes() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // Monde TS : triangle a_ts -> b_ts -> c_ts -> a_ts (fonctions TS)
    for name in ["a_ts", "b_ts", "c_ts", "a_rs", "b_rs", "c_rs"] {
        session
            .execute(&format!("CREATE (:Function {{name: '{name}'}})"))
            .expect("create node");
    }

    // Triangle TS : a_ts -> b_ts -> c_ts -> a_ts
    for (src, dst) in [
        ("a_ts", "b_ts"),
        ("b_ts", "c_ts"),
        ("c_ts", "a_ts"),
        ("a_rs", "b_rs"),
        ("b_rs", "c_rs"),
        ("c_rs", "a_rs"),
    ] {
        session
            .execute(&format!(
                "MATCH (a:Function {{name: '{src}'}}), (b:Function {{name: '{dst}'}}) CREATE (a)-[:CALLS]->(b)"
            ))
            .expect("create edge");
    }

    // Homonymes croisés : les noms "a_ts"/"a_rs" sont DISTINCTS (pas homonymes
    // réels) — mais ajoutons de vrais homonymes : deux nœuds nommés "dup"
    // dans des contextes différents, dont un seul forme un triangle.
    for name in ["dup", "dup", "x"] {
        session
            .execute(&format!("CREATE (:Function {{name: '{name}'}})"))
            .expect("create dup");
    }
    // dup_1 -> x -> dup_1 (un triangle avec un homonyme)
    session
        .execute(
            "MATCH (a:Function {name: 'dup'}), (b:Function {name: 'x'}) CREATE (a)-[:CALLS]->(b)",
        )
        .expect("edge dup->x");
    // Le second "dup" n'a PAS d'arête — seul le premier forme un triangle via x.
    session
        .execute(
            "MATCH (a:Function {name: 'x'}), (b:Function {name: 'dup'}) CREATE (a)-[:CALLS]->(b)",
        )
        .expect("edge x->dup");
    db
}

/// Compte les triangles par la formulation « chemin multi-hop + MATCH de
/// fermeture » (le pattern qui était buggy).
fn triangles_chemin_fermeture(db: &ObrainDB) -> i64 {
    let session = db.session();
    let r = session
        .execute(
            "MATCH (a:Function)-[:CALLS]->(b:Function)-[:CALLS]->(c:Function) \
             MATCH (c:Function)-[:CALLS]->(a:Function) RETURN count(a) AS n",
        )
        .expect("triangle chemin+fermeture");
    r.rows
        .first()
        .and_then(|row| row.first())
        .map_or(-1, |v| v.to_string().parse().unwrap_or(-1))
}

/// Compte les triangles par la formulation « trois arêtes dans un seul MATCH »
/// (la référence correcte).
fn triangles_trois_aretes(db: &ObrainDB) -> i64 {
    let session = db.session();
    let r = session
        .execute(
            "MATCH (a:Function)-[:CALLS]->(b:Function), \
             (b:Function)-[:CALLS]->(c:Function), \
             (c:Function)-[:CALLS]->(a:Function) RETURN count(a) AS n",
        )
        .expect("triangle 3 arêtes");
    r.rows
        .first()
        .and_then(|row| row.first())
        .map_or(-1, |v| v.to_string().parse().unwrap_or(-1))
}

#[test]
fn jointure_fermee_multihop_est_exacte() {
    let db = graph_avec_homonymes();

    // Les deux formulations doivent donner le MÊME compte (chaque triangle
    // compte 3× via ses sommets, dans les deux formulations).
    let chemin = triangles_chemin_fermeture(&db);
    let trois = triangles_trois_aretes(&db);

    // Monde TS : 1 triangle (3 rotations). Monde RS : 1 triangle (3 rotations).
    // dup -> x -> dup : 1 triangle via dup_1 (dup_1->x, x->dup_1, dup_1->dup_1 ?
    // non — il faut 3 nœuds distincts ; dup_1->x->dup_1 ferme via x->dup_1
    // MAIS a==c (dup_1) — pas un vrai triangle 3-sommets. On attend donc
    // exactement les 2 triangles TS/RS = 2 × 3 rotations = 6 count(a).
    assert!(
        chemin > 0,
        "le triangle chemin+fermeture doit trouver des triangles (got {chemin})"
    );
    assert_eq!(
        chemin, trois,
        "les deux formulations doivent donner le même compte — bug de corrélation inter-MATCH si différent (chemin={chemin}, 3-arêtes={trois})"
    );
}
