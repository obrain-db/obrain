//! Jointure par hachage dont le côté sondé est vide (obrain-db/obrain#189, F2).
//!
//! `MATCH (t:Task {id: $id})-[:MODIFIES]->(f:File) OPTIONAL MATCH
//! (f)<-[:IMPORTS*1..3]-(d:File)` devient un `LeftJoin` haché dont le côté
//! construit (`NodeScan(f:*)` + `IMPORTS*1..3`) ne dépend pas de la gauche.
//! Quand la gauche ne rend AUCUNE ligne (task_05 de PO : tâche sans fichier
//! modifié), le résultat est vide quoi qu'il arrive, mais la table était bâtie
//! en entier : 3 095 865 chemins énumérés pour 0 ligne rendue (3,2 s).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test jointure_cote_sonde_vide_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_core::execution::operators::chemins_enumeres;
use obrain_engine::ObrainDB;

const FICHIERS: usize = 60;

/// `FICHIERS` fichiers en chaîne d'imports (f_i importe f_{i+1}), une tâche
/// `vide` sans fichier modifié et une tâche `pleine` qui modifie f_10.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let fichiers: Vec<_> = (0..FICHIERS)
        .map(|i| {
            s.create_node_with_props(
                &["File"],
                [("path", Value::String(format!("f{i}.rs").into()))],
            )
        })
        .collect();
    for w in fichiers.windows(2) {
        s.create_edge(w[0], w[1], "IMPORTS");
    }
    s.create_node_with_props(&["Task"], [("id", Value::String("vide".into()))]);
    let pleine = s.create_node_with_props(&["Task"], [("id", Value::String("pleine".into()))]);
    s.create_edge(pleine, fichiers[10], "MODIFIES");
    db
}

fn impact(db: &ObrainDB, tache: &str) -> (Vec<Vec<Value>>, u64) {
    let q = format!(
        "MATCH (t:Task {{id: '{tache}'}})-[:MODIFIES]->(f:File) \
         OPTIONAL MATCH (f)<-[:IMPORTS*1..3]-(dependent:File) \
         RETURN f.path AS file, dependent.path AS dep"
    );
    let avant = chemins_enumeres();
    let r = db
        .session()
        .execute_cypher(&q)
        .unwrap_or_else(|e| panic!("{q}\n{e}"));
    (r.rows, chemins_enumeres() - avant)
}

#[test]
fn gauche_vide_ne_construit_pas_le_cote_droit() {
    let db = base();
    let (lignes, enumeres) = impact(&db, "vide");
    assert!(lignes.is_empty(), "résultat : {lignes:?}");
    // Sans court-circuit : ~3 chemins par fichier de la chaîne (≈ 3 × 60).
    assert_eq!(
        enumeres, 0,
        "{enumeres} chemins énumérés pour une gauche vide : le côté construit a été bâti"
    );
}

#[test]
fn gauche_non_vide_meme_resultat() {
    let db = base();
    let (mut lignes, _) = impact(&db, "pleine");
    lignes.sort_by_key(|l| format!("{l:?}"));
    let deps: Vec<String> = lignes
        .iter()
        .map(|l| {
            assert_eq!(l[0], Value::String("f10.rs".into()));
            match &l[1] {
                Value::String(s) => s.to_string(),
                v => panic!("dépendant inattendu : {v:?}"),
            }
        })
        .collect();
    assert_eq!(deps, vec!["f7.rs", "f8.rs", "f9.rs"]);
}
