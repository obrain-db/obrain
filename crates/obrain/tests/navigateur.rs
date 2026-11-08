//! Fumée NAVIGATEUR (`wasm32-unknown-unknown`, feature `browser`).
//!
//! Aucun test ne s'exécutait sur cette cible : la base compilait mais paniquait à
//! la première ligne d'usage (`ObrainDB::new_in_memory` → `GroupCommit::neuf` →
//! `std::time::Instant::now`, qui n'existe pas sans horloge de la std). Ces tests
//! tournent dans node ou un navigateur via `wasm-bindgen-test-runner` :
//!
//! ```text
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test --target wasm32-unknown-unknown -p obrain \
//!   --no-default-features --features browser --test navigateur
//! ```
#![cfg(target_arch = "wasm32")]

use obrain::{ObrainDB, Value};
use wasm_bindgen_test::wasm_bindgen_test;

fn textes(rows: &[Vec<Value>]) -> Vec<String> {
    let mut v: Vec<String> = rows
        .iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => format!("{autre:?}"),
        })
        .collect();
    v.sort();
    v
}

#[wasm_bindgen_test]
fn une_base_en_memoire_ecrit_puis_relit() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:Person {name: 'Alix'})").unwrap();
    s.execute("INSERT (:Person {name: 'Bao'})").unwrap();
    let r = s.execute("MATCH (p:Person) RETURN p.name").unwrap();
    assert_eq!(textes(&r.rows), vec!["Alix".to_string(), "Bao".to_string()]);
    assert_eq!(db.node_count(), 2);
}

#[wasm_bindgen_test]
fn une_arete_se_traverse() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:A {name: 'a'})-[:R]->(:B {name: 'b'})")
        .unwrap();
    let r = s.execute("MATCH (x:A)-[:R]->(y:B) RETURN y.name").unwrap();
    assert_eq!(textes(&r.rows), vec!["b".to_string()]);
    assert_eq!(db.edge_count(), 1);
}

#[wasm_bindgen_test]
fn supprimer_un_noeud_detache_ses_aretes() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:A {name: 'a'})-[:R]->(:B {name: 'b'})")
        .unwrap();
    s.execute("MATCH (x:A) DETACH DELETE x").unwrap();
    assert_eq!(db.node_count(), 1, "il reste B");
    assert_eq!(db.edge_count(), 0, "l'arête est partie avec A");
}
