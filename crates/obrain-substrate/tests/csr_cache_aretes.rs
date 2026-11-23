//! Le cache CSR doit voir les mutations d'ARÊTES de son propre processus :
//! une insertion entre deux nœuds EXISTANTS, et une suppression.
//!
//! Aucune des deux ne crée de nœud, et `tombstone_edge` n'incrémente ni
//! `feature_epoch` ni `edge_slot_high_water` (relu dans `writer.rs`). La clé
//! `(époque, taille du WAL)` ne les rattrape que parce que chacune ajoute une
//! trame au WAL : ces tests le gardent, pour qu'un futur changement de clé ne
//! réintroduise pas un CSR périmé en silence (critères d'acceptation de #292).
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::store::SubstrateStore;
use std::sync::Arc;

#[test]
fn une_insertion_entre_noeuds_existants_invalide_le_cache() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let a = s.create_node(&["N"]);
    let b = s.create_node(&["N"]);
    s.create_edge(a, b, "LIEN");

    let avant = s.csr_cached();
    assert_eq!(avant.edges.len(), 1);
    s.create_edge(b, a, "LIEN");
    let apres = s.csr_cached();

    assert!(
        !Arc::ptr_eq(&avant, &apres),
        "une arête créée entre deux nœuds existants n'a pas invalidé le cache"
    );
    assert_eq!(apres.edges.len(), 2);
}

#[test]
fn une_suppression_d_arete_invalide_le_cache() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let a = s.create_node(&["N"]);
    let b = s.create_node(&["N"]);
    let e = s.create_edge(a, b, "LIEN");

    let avant = s.csr_cached();
    assert_eq!(avant.edges.len(), 1);
    assert!(s.delete_edge(e));
    let apres = s.csr_cached();

    assert!(
        !Arc::ptr_eq(&avant, &apres),
        "une arête supprimée n'a pas invalidé le cache : le CSR servi contient \
         une arête qui n'existe plus"
    );
    assert_eq!(apres.edges.len(), 0);
}
