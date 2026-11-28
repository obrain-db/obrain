//! Cible canonique d'une révision d'engramme : le registre durable ne doit
//! JAMAIS écrire sur un nœud homonyme par correspondance numérique.
//!
//! `EngramManager` transmettait `engram_id.0.to_be_bytes()`, décodé comme un
//! `NodeId` par le registre durable. La ressource d'engramme est désormais
//! TYPÉE (`E:<id>`, longueur exacte) : le registre durable la REFUSE (aucune
//! incarnation durable attestée), un nœud étranger homonyme reste INTACT et
//! l'identité d'observation n'est pas engagée.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_cognitive::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    AtomicEventLedger, Engram, EngramId, EngramManager, EngramManagerConfig, ObservationId,
    ResolvedState, RevisionError, SourceRevision, SubstrateAtomicEventLedger,
};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{EtatPropriete, SubstrateStore};

#[test]
fn noeud_etranger_homonyme_reste_intact() {
    let store = Arc::new(SubstrateStore::open_memory().unwrap());
    // Deux nœuds ÉTRANGERS (aucun lien avec l'engramme) : NodeId(1) et NodeId(2).
    let _n1 = store.create_node(&["etranger"]);
    let n2 = store.create_node(&["etranger"]);
    assert_eq!(n2.0, 2, "allocation monotone : n2 = NodeId(2)");

    let ledger = Arc::new(SubstrateAtomicEventLedger::new(store.clone()));
    let config = EngramManagerConfig::default();
    let index = Arc::new(InMemoryVectorIndex::new());
    let manager = EngramManager::new(config, index).with_observation_ledger(ledger.clone());

    // Chemin natif : EngramStore::new(None) — PAS de nœud `:Engram`. Un engramme
    // EngramId(2) dont l'ensemble pointe vers le nœud 1.
    manager
        .store()
        .insert(Engram::new(EngramId(2), vec![(NodeId(1), 1.0)]));

    let obs = ObservationId::from_u128(0xABCD_0000_0000_0001);
    let outcome = manager.revise_with_id(obs, EngramId(2), &[NodeId(1)], 0.5, SourceRevision(0));
    eprintln!("outcome = {outcome:?}");

    // Refus SÛR NOMÉ : la ressource d'engramme est TYPÉE mais sans incarnation
    // durable attestée → le registre durable refuse au lieu d'écrire sur NodeId(2).
    match outcome {
        Err(RevisionError::PersistenceFailed(msg)) => {
            assert!(
                msg.contains("incarnation durable"),
                "erreur nommée attendue (incarnation), reçu : {msg}"
            );
        }
        autre => panic!("attendu un refus nommé PersistenceFailed, reçu {autre:?}"),
    }

    // Le nœud étranger homonyme n'a reçu NI obs:revision NI obs:effect.
    for cle in ["obs:revision", "obs:effect"] {
        match store.lire_etat_propriete(n2, cle).unwrap() {
            EtatPropriete::Absente | EtatPropriete::RessourceInexistante => {}
            autre => panic!("le nœud étranger homonyme ne doit pas porter {cle}, reçu {autre:?}"),
        }
    }

    // L'état de l'engramme est INTACT (aucune révision appliquée).
    let engramme = manager.store().get(EngramId(2)).expect("engramme présent");
    assert_eq!(
        engramme.revision, 0,
        "aucune révision ne doit avoir été appliquée"
    );

    // L'identité d'observation n'est PAS engagée.
    assert_eq!(
        ledger.resolve(&obs.0).unwrap(),
        ResolvedState::NotApplied,
        "l'ObservationId refusé ne doit pas être engagé"
    );
}
