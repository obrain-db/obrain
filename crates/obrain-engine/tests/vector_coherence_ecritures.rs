//! ── COHÉRENCE DES ÉCRITURES VECTORIELLES (T5) ───────────────────────────────
//!
//! Un index vectoriel doit refléter TOUS les chemins d'écriture : API Rust,
//! GQL `SET` / `CREATE` / `MERGE`, `remove_node_property`, `delete_node`.
//! Défaut relevé le 10/11/2026 : seule l'API Rust entretient l'index ; les
//! opérateurs GQL écrivent dans le store sans prévenir le moteur.
//!
//! Ces tests DOIVENT ÉCHOUER sur `main` avant l'implémentation (preuve du défaut).

#![cfg(feature = "vector-index")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const D: usize = 8;

fn axe(i: usize) -> Vec<f32> {
    let mut v = vec![0f32; D];
    v[i] = 1.0;
    v
}

fn litteral(v: &[f32]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x:.1}")).collect();
    format!("[{}]", parts.join(", "))
}

/// Base en mémoire avec un index vide sur `:Doc(emb)`, plus un nœud de départ
/// portant l'axe 0 (pour que l'index existe et soit non vide).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let n = db.create_node(&["Doc"]);
    db.set_node_property(n, "emb", Value::Vector(axe(0).into()));
    db.create_vector_index("Doc", "emb", Some(D), Some("cosine"), None, None)
        .unwrap();
    db
}

fn meilleur(db: &ObrainDB, q: &[f32]) -> Option<(u64, f32)> {
    db.vector_search("Doc", "emb", q, 1, None, None)
        .unwrap()
        .into_iter()
        .next()
        .map(|(id, d)| (id.as_u64(), d))
}

#[test]
fn set_gql_puis_recherche_retrouve_le_vecteur() {
    let db = base();
    db.execute(&format!("CREATE (:Doc {{name: 'x'}})")).unwrap();
    db.execute(&format!(
        "MATCH (n:Doc) WHERE n.name = 'x' SET n.emb = vector({})",
        litteral(&axe(5))
    ))
    .unwrap();
    let (_, d) = meilleur(&db, &axe(5)).expect("un résultat");
    assert!(d < 1e-4, "le vecteur SET en GQL doit être retrouvé (distance {d})");
}

#[test]
fn create_gql_avec_vecteur_est_indexe() {
    let db = base();
    db.execute(&format!(
        "CREATE (:Doc {{emb: vector({})}})",
        litteral(&axe(6))
    ))
    .unwrap();
    let (_, d) = meilleur(&db, &axe(6)).expect("un résultat");
    assert!(d < 1e-4, "CREATE GQL avec vecteur doit être indexé (distance {d})");
}

#[test]
fn merge_gql_avec_vecteur_est_indexe() {
    let db = base();
    db.execute(&format!(
        "MERGE (:Doc {{name: 'm', emb: vector({})}})",
        litteral(&axe(7))
    ))
    .unwrap();
    let (_, d) = meilleur(&db, &axe(7)).expect("un résultat");
    assert!(d < 1e-4, "MERGE GQL avec vecteur doit être indexé (distance {d})");
}

#[test]
fn remove_node_property_retire_le_vecteur_de_l_index() {
    let db = base();
    let id = db.create_node(&["Doc"]);
    db.set_node_property(id, "emb", Value::Vector(axe(3).into()));
    assert!(meilleur(&db, &axe(3)).is_some_and(|(i, _)| i == id.as_u64()));
    assert!(db.remove_node_property(id, "emb"));
    let hits = db.vector_search("Doc", "emb", &axe(3), 10, None, None).unwrap();
    assert!(
        hits.iter().all(|(i, _)| i.as_u64() != id.as_u64()),
        "un nœud dont le vecteur a été retiré ne doit plus être retrouvé"
    );
}

#[test]
fn delete_node_retire_le_vecteur_de_l_index() {
    let db = base();
    let id = db.create_node(&["Doc"]);
    db.set_node_property(id, "emb", Value::Vector(axe(4).into()));
    assert!(db.delete_node(id));
    let hits = db.vector_search("Doc", "emb", &axe(4), 10, None, None).unwrap();
    assert!(
        hits.iter().all(|(i, _)| i.as_u64() != id.as_u64()),
        "un nœud supprimé ne doit plus être retrouvé"
    );
}

#[test]
fn rollback_d_un_set_vecteur_ne_laisse_pas_de_fantome() {
    // Comportement attendu : la transaction annulée ne doit pas laisser le
    // vecteur retrouvable. Si l'implémentation laisse une entrée orpheline,
    // le test (et LIMITES-v0.0.1.md) doit l'écrire explicitement.
    let db = base();
    let id = db.create_node(&["Doc"]);
    let mut tx = db.begin_transaction();
    tx.execute(&format!(
        "MATCH (n) WHERE id(n) = {} SET n.emb = vector({})",
        id.as_u64(),
        litteral(&axe(2))
    ))
    .ok();
    tx.rollback().ok();
    let hits = db.vector_search("Doc", "emb", &axe(2), 10, None, None).unwrap();
    assert!(
        hits.iter().all(|(i, _)| i.as_u64() != id.as_u64()),
        "une transaction annulée ne doit pas laisser de vecteur retrouvable"
    );
}
