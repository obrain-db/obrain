//! obrain#196 (famille D2) — une propriete vectorielle ABSENTE ne doit pas se
//! lire comme un vecteur de zeros.
//!
//! Le format de colonne dense alloue tous les slots jusqu'au plus haut slot
//! ecrit (« all-zero slot == absent », cf. l'en-tete de `vec_column.rs`) : un
//! slot simplement traverse vaut zero. Lire le slot brut faisait donc apparaitre
//! une propriete absente comme `[0.0; dim]`.
//!
//! Consequence mesuree dans PO avant ce correctif : `list_notes_without_embedding`
//! (`WHERE n.embedding IS NULL`) rendait **1 ligne au lieu de 50** — les notes a
//! reindexer devenaient invisibles, et `note_30` rendait 13 notes en trop.

use std::sync::Arc;

use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Ecrit le vecteur sur le DERNIER noeud pour que les slots des precedents
/// soient alloues (donc lisibles) sans jamais avoir ete ecrits : c'est la
/// condition exacte de D2.
#[test]
fn un_slot_alloue_mais_jamais_ecrit_est_absent_pas_nul() {
    let dir = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(dir.path()).unwrap();

    let cle = PropertyKey::new("embedding");
    let premier = store.create_node(&["Note"]);
    let milieu = store.create_node(&["Note"]);
    let dernier = store.create_node(&["Note"]);

    let v: Arc<[f32]> = vec![0.5_f32, 0.25, 0.125, 1.0].into();
    store.set_node_property(dernier, "embedding", Value::Vector(Arc::clone(&v)));

    // En session : les slots alloues par l'ecriture du dernier ne portent rien.
    assert!(
        store.get_node_property(premier, &cle).is_none(),
        "slot jamais ecrit : la propriete est ABSENTE, pas un vecteur de zeros"
    );
    assert!(store.get_node_property(milieu, &cle).is_none());
    assert!(
        store.get_node_property(dernier, &cle).is_some(),
        "le noeud qui porte reellement le vecteur doit le rendre"
    );

    // Apres reouverture a froid : meme contrat, la colonne est relue du disque.
    store.checkpoint().unwrap();
    let relu = SubstrateStore::open(dir.path()).unwrap();
    assert!(
        relu.get_node_property(premier, &cle).is_none(),
        "apres reouverture, un slot jamais ecrit reste absent"
    );
    assert!(relu.get_node_property(milieu, &cle).is_none());
    let rendu = relu.get_node_property(dernier, &cle);
    assert!(rendu.is_some());
    if let Some(Value::Vector(vec)) = rendu {
        assert_eq!(
            vec.as_ref(),
            v.as_ref(),
            "le vecteur ecrit se relit a l'identique"
        );
    } else {
        panic!("le noeud porteur doit rendre un Value::Vector");
    }
}

/// Meme contrat du cote des aretes : `lire_propriete_arete_vivante` emprunte le
/// meme chemin de colonne dense.
#[test]
fn une_propriete_vectorielle_absente_sur_une_arete_est_absente() {
    let dir = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(dir.path()).unwrap();

    let cle = PropertyKey::new("embedding");
    let a = store.create_node(&["A"]);
    let b = store.create_node(&["A"]);
    let c = store.create_node(&["A"]);
    let e1 = store.create_edge(a, b, "LIEN");
    let e2 = store.create_edge(b, c, "LIEN");

    let v: Arc<[f32]> = vec![0.1_f32, 0.2, 0.3].into();
    store.set_edge_property(e2, "embedding", Value::Vector(Arc::clone(&v)));

    assert!(
        store.get_edge_property(e1, &cle).is_none(),
        "arete dont le slot est alloue mais jamais ecrit : propriete absente"
    );
    assert!(store.get_edge_property(e2, &cle).is_some());
}
