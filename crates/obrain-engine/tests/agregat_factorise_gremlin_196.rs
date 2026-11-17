//! La porte Gremlin atteint le chemin factorisé des agrégats
//! (obrain-db/obrain#196, D4 — réserve 1 de la PR #219) : `sum()` / `mean()`
//! portent sur la variable courante, directement au-dessus de la chaîne
//! `out().out()`, sans filtre d'unicité. Sur `main`, avec un seul arc,
//! `g.V().out().out().count()` rendait 1, `sum()` 2.0 (le nœud
//! intermédiaire) et `g.V().out().out()` échouait (« Column 4 not found »).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher,gremlin --test agregat_factorise_gremlin_196
//! ```
#![cfg(all(feature = "cypher", feature = "gremlin"))]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn un_seul_arc() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:N {w: 1.5})-[:R]->(:N {w: 2.5})")
        .unwrap();
    db
}

fn gremlin(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_gremlin(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

#[test]
fn deux_sauts_sur_un_seul_arc_ne_comptent_aucun_chemin() {
    let db = un_seul_arc();
    assert_eq!(
        gremlin(&db, "g.V().out().out().count()"),
        vec![vec![Value::Int64(0)]]
    );
    // TinkerPop : sum() sur ensemble vide vaut 0.
    assert_eq!(
        gremlin(&db, "g.V().out().out().sum()"),
        vec![vec![Value::Int64(0)]]
    );
    assert_eq!(
        gremlin(&db, "g.V().out().out().mean()"),
        vec![vec![Value::Null]]
    );
    assert_eq!(
        gremlin(&db, "g.V().out().out().min()"),
        vec![vec![Value::Null]]
    );
}

#[test]
fn deux_sauts_sur_un_seul_arc_projettent_zero_ligne() {
    let db = un_seul_arc();
    assert_eq!(gremlin(&db, "g.V().out().out()"), Vec::<Vec<Value>>::new());
}

#[test]
fn graphe_sans_sommet_rend_zero_pour_sum() {
    let db = ObrainDB::new_in_memory();
    assert_eq!(
        gremlin(&db, "g.V().out().out().sum()"),
        vec![vec![Value::Int64(0)]]
    );
    assert_eq!(
        gremlin(&db, "g.V().out().out().count()"),
        vec![vec![Value::Int64(0)]]
    );
}

/// Témoin : la chaîne complète est inchangée.
#[test]
fn chaine_complete_inchangee() {
    let db = un_seul_arc();
    db.session()
        .execute_cypher("MATCH (b:N {w: 2.5}) CREATE (b)-[:R]->(:N {w: 4.0})")
        .unwrap();
    assert_eq!(
        gremlin(&db, "g.V().out().out().count()"),
        vec![vec![Value::Int64(1)]]
    );
    assert_eq!(gremlin(&db, "g.V().out().out()").len(), 1);
}
