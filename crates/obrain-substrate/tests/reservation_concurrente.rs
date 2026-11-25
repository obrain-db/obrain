//! Contre-exemple concurrent de la réservation d'identité (#416, garantie
//! transactionnelle générique #413).
//!
//! Deux chemins comparés :
//! 1. `upsert_simplex` — recherche VÉRIFIÉE + matérialisation sous une section
//!    critique commune (`simplex_index.lock()`). Modèle natif de réservation.
//! 2. Recherche puis insertion SANS section critique — le motif de
//!    `Composition::construct` (`find_by_key` → `create_node`). Course TOCTOU.

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};
use std::thread;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::record::simplex_kind;
use obrain_substrate::SubstrateStore;

/// Le chemin natif : N upserts concurrents du même simplexe → UNE identité.
#[test]
fn upsert_simplex_concurrent_rend_une_seule_identite() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let (a, b, c) = (
        store.create_node(&["fn"]),
        store.create_node(&["fn"]),
        store.create_node(&["fn"]),
    );
    let membres = vec![a, b, c];

    let n = 8;
    let barriere = Arc::new(Barrier::new(n));
    let mut fils = Vec::new();
    for _ in 0..n {
        let store = Arc::clone(&store);
        let membres = membres.clone();
        let barriere = Arc::clone(&barriere);
        fils.push(thread::spawn(move || {
            barriere.wait();
            store
                .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &membres, &[])
                .unwrap()
        }));
    }
    let sorties: Vec<_> = fils.into_iter().map(|f| f.join().unwrap()).collect();
    let ids: BTreeSet<_> = sorties.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids.len(), 1, "upsert_simplex concurrent : {ids:?}");

    let vivants = store.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap();
    assert_eq!(vivants.len(), 1, "un seul simplexe vivant : {vivants:?}");
}

/// Le motif naïf (find_by_key → create_node, sans section critique) : deux
/// lecteurs synchronisés après leur constat d'absence créent DEUX identités.
#[test]
fn recherche_puis_insertion_sans_section_critique_cree_deux_identites() {
    let td = tempfile::tempdir().unwrap();
    let store = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());

    let n = 2;
    let barriere = Arc::new(Barrier::new(n));
    let mut fils = Vec::new();
    for _ in 0..n {
        let store = Arc::clone(&store);
        let barriere = Arc::clone(&barriere);
        fils.push(thread::spawn(move || {
            // find : scan linéaire, aucune section critique commune.
            let existant = store.node_ids().into_iter().find(|id| {
                store.get_node(*id).is_some_and(|noeud| {
                    noeud
                        .properties
                        .contains_key(&PropertyKey::new("canonical_key"))
                })
            });
            if let Some(id) = existant {
                return id;
            }
            // Synchronisation APRÈS le constat d'absence : les deux lecteurs
            // observent « absent » avant que l'un ou l'autre ne crée.
            barriere.wait();
            // create.
            let id = store.create_node(&["structure"]);
            store.set_node_property(id, "canonical_key", Value::Int64(1));
            id
        }));
    }
    let ids: BTreeSet<NodeId> = fils.into_iter().map(|f| f.join().unwrap()).collect();
    // DÉMONSTRATION de la course : deux identités distinctes créées.
    assert_eq!(
        ids.len(),
        2,
        "recherche puis insertion sans section critique : {ids:?}"
    );
}
