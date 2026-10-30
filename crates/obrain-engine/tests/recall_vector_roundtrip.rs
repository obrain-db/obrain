//! Étape 1 (BLOQUANTE) — round-trip write test du plan recall-vecteur-natif.
//!
//! Question : le write-path GQL `SET n.embedding = vector([...])` fait-il
//! atterrir un `Value::Vector` (routé vers la colonne dense veccol par
//! `SubstrateStore::set_node_property`) ou un `Value::List` ?
//!
//! Lecture faisant autorité = `store().get_node_property(id, "embedding")`,
//! qui consulte la colonne veccol (store.rs). La projection GQL
//! `RETURN n.embedding` NE remonte PAS une prop veccol (elle vit hors de la
//! property-map), donc on ne s'y fie pas ici.
//!
//!   PASSE  → Branche A : émettre `vector([...])` (au lieu de la liste nue
//!            `[...]`) dans le MERGE/SET du write-path recall.
//!   ÉCHOUE → Branche B : support natif `SubstrateStore::set_node_vector`.
//!
//! cargo test -p obrain-engine --test recall_vector_roundtrip -- --nocapture

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::{Config, ObrainDB};

/// Littéraux flottants GQL "0.0, 1.0, 2.0, ..." de longueur `n`.
fn floats_gql(n: usize) -> String {
    (0..n)
        .map(|i| format!("{}.0", i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Branche A : `SET n.embedding = vector([...])` (GQL) doit stocker un
/// `Value::Vector` de bonne dimension, relu via le store (chemin veccol).
#[test]
fn gql_set_vector_roundtrips_as_vector_not_list() {
    let db = ObrainDB::with_config(Config::in_memory()).expect("db in-memory");
    const DIM: usize = 1024; // dimension réelle des embeddings (f32 1024-dim exact)

    // Noeud connu (NodeId retourné) + id scalaire pour que le MERGE le matche.
    let n = db.create_node(&["EpisodicLesson"]);
    db.set_node_property(n, "id", Value::String("rt-A".into()));

    // Write-path Branche A : le SET émet `vector([...])`.
    let ecrit = format!(
        "MERGE (n:EpisodicLesson {{id: 'rt-A'}}) SET n.embedding = vector([{}]) RETURN n.id",
        floats_gql(DIM)
    );
    db.execute(&ecrit).expect("exécution GQL vector() SET");

    // Lecture faisant autorité : consulte veccol.
    let key = PropertyKey::new("embedding");
    match db.store().get_node_property(n, &key) {
        Some(Value::Vector(v)) => assert_eq!(v.len(), DIM, "dimension préservée (veccol)"),
        autre => panic!("Branche A cassée : get_node_property rend {autre:?}, pas Value::Vector"),
    }
}

/// Contrôle négatif : la liste nue `[...]` (write-path ACTUEL) reste
/// `Value::List` — jamais routée veccol. C'est la cause du full-scan 42-90 s
/// et la preuve que Branche A (vector([...])) change bien le type stocké.
#[test]
fn gql_set_bare_list_stays_list_the_slow_path() {
    let db = ObrainDB::with_config(Config::in_memory()).expect("db in-memory");
    const DIM: usize = 8;

    let n = db.create_node(&["EpisodicLesson"]);
    db.set_node_property(n, "id", Value::String("rt-list".into()));

    let ecrit = format!(
        "MERGE (n:EpisodicLesson {{id: 'rt-list'}}) SET n.embedding = [{}] RETURN n.id",
        floats_gql(DIM)
    );
    db.execute(&ecrit).expect("exécution GQL list SET");

    let key = PropertyKey::new("embedding");
    match db.store().get_node_property(n, &key) {
        Some(Value::List(_)) => {} // attendu : liste nue → jamais veccol
        Some(Value::Vector(_)) => {
            panic!("inattendu : la liste nue est devenue Vector — le write-path actuel ne serait pas lent")
        }
        autre => panic!("inattendu : la liste nue rend {autre:?}"),
    }
}
