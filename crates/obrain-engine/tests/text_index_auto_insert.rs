//! Maintien de l'index texte enregistré (`substrate_text_indexes`) par les
//! mutations, en mode substrat (`ObrainDB::open`, chemin de production).
//!
//! Défaut constaté (PR obrain-db #128, test « text search > should find new
//! nodes after mutation ») : après `create_text_index`, un nœud créé ensuite
//! avec la propriété indexée n'est pas trouvé par `text_search` ; après
//! `rebuild_text_index` il l'est. Le chemin d'auto-insertion ne mettait à jour
//! que le registre du store, pas le registre lu par la recherche.
//!
//! ```bash
//! cargo test -p obrain-engine --features text-index --test text_index_auto_insert
//! ```

#![cfg(feature = "text-index")]

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use tempfile::tempdir;

/// Ids trouvés par la recherche texte sur `Doc.text`.
fn trouves(db: &ObrainDB, requete: &str) -> Vec<NodeId> {
    db.text_search("Doc", "text", requete, 10)
        .expect("recherche texte")
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Création d'un nœud avec `create_node_with_props` après `create_text_index`.
#[test]
fn create_node_with_props_apres_create_text_index_est_trouve() {
    let dir = tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path().join("base")).expect("open");
    db.create_text_index("Doc", "text").expect("index texte");

    let nouveau = db.create_node_with_props(
        &["Doc"],
        [("text", Value::String("graphe de connaissances".into()))],
    );

    assert!(
        trouves(&db, "graphe").contains(&nouveau),
        "un nœud créé après create_text_index doit être trouvé par text_search"
    );
}

/// Ajout d'un label après `create_text_index` : `add_node_label` doit aussi
/// alimenter le registre lu par la recherche.
#[test]
fn add_node_label_apres_create_text_index_est_trouve() {
    let dir = tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path().join("base")).expect("open");
    let id = db.create_node_with_props(
        &["Autre"],
        [("text", Value::String("graphe de connaissances".into()))],
    );
    db.create_text_index("Doc", "text").expect("index texte");

    assert!(db.add_node_label(id, "Doc"));

    assert!(
        trouves(&db, "graphe").contains(&id),
        "un nœud qui reçoit le label indexé doit être trouvé par text_search"
    );
}

/// SET d'une propriété indexée après `create_text_index`.
#[test]
fn set_node_property_apres_create_text_index_est_trouve() {
    let dir = tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path().join("base")).expect("open");
    db.create_text_index("Doc", "text").expect("index texte");
    let id = db.create_node(&["Doc"]);

    db.set_node_property(id, "text", Value::String("graphe de connaissances".into()));

    assert!(
        trouves(&db, "graphe").contains(&id),
        "un SET de la propriété indexée doit être trouvé par text_search"
    );
}

/// DELETE d'un nœud indexé : il ne doit plus sortir de la recherche.
#[test]
fn delete_node_apres_create_text_index_sort_de_la_recherche() {
    let dir = tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path().join("base")).expect("open");
    db.create_text_index("Doc", "text").expect("index texte");
    let id = db.create_node_with_props(
        &["Doc"],
        [("text", Value::String("graphe de connaissances".into()))],
    );
    assert!(trouves(&db, "graphe").contains(&id));

    assert!(db.delete_node(id));

    assert!(
        !trouves(&db, "graphe").contains(&id),
        "un nœud supprimé ne doit plus sortir de text_search"
    );
}

/// Retrait du label indexé : le nœud ne doit plus sortir de la recherche
/// comme un document de `Doc`.
#[test]
fn remove_node_label_apres_create_text_index_sort_de_la_recherche() {
    let dir = tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path().join("base")).expect("open");
    db.create_text_index("Doc", "text").expect("index texte");
    let id = db.create_node_with_props(
        &["Doc"],
        [("text", Value::String("graphe de connaissances".into()))],
    );
    assert!(trouves(&db, "graphe").contains(&id));

    assert!(db.remove_node_label(id, "Doc"));

    assert!(
        !trouves(&db, "graphe").contains(&id),
        "un nœud qui perd le label indexé ne doit plus sortir de text_search"
    );
}
