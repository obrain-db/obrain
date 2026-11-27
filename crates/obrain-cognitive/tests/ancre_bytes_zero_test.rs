//! Contre-épreuve immédiate : une ancre Bytes TOUT À ZÉRO (longueur attendue)
//! n'est PAS une absence. La convention « null = absent » est propre aux
//! colonnes VECTORIELLES ; le chemin Bytes doit restituer la valeur, et le
//! DÉCODEUR (decode_anchor) décide alors de sa validité.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_clock::AnchorState;
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::sync::Arc;

#[test]
fn une_ancre_bytes_tout_a_zero_n_est_pas_une_absence() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();

    // Ancre Bytes de 32 octets, TOUS à zéro (longueur attendue par le format).
    let zeros = Arc::from(vec![0u8; 32].as_slice());
    sub.set_node_property(node, "_cog_energy_anchor", Value::Bytes(zeros));
    sub.flush().unwrap();

    // Lecture directe : la valeur Bytes doit être RESTITUÉE, pas lue absente.
    let v = sub.get_node_property(node, &PropertyKey::from("_cog_energy_anchor"));
    match &v {
        Some(Value::Bytes(b)) => {
            assert_eq!(b.len(), 32, "Bytes 32 restitués");
            assert!(b.iter().all(|&x| x == 0), "octets tous nuls conservés");
        }
        autre => panic!("une ancre Bytes tout-à-zéro NE doit pas être lue absente : {autre:?}"),
    }

    // Le décodeur la traite comme CORROMPUE (e_ref=0 invalide), pas Absente.
    let etat = decay_clock::read_anchor_state(sub.as_ref(), node);
    assert!(
        !matches!(etat, AnchorState::Absent),
        "tout-à-zéro n'est pas une absence : {etat:?}"
    );
    eprintln!("get_node_property = {v:?} ; read_anchor_state = {etat:?}");
}
