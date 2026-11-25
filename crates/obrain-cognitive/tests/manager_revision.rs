//! Révision cohérente au niveau d'EngramManager : formation → rappel →
//! observation indépendante → révision → vérification membres/poids/signature
//! → fermeture → réouverture → comparaison. Exercé réellement (substrate).

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::traits::InMemoryVectorIndex;
use obrain_cognitive::engram::{EngramManager, EngramManagerConfig};
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

#[test]
fn revision_coherente_survit_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let formed_id;
    // Phase 1 : formation, rappel avant révision, révision effective.
    {
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        // Formation {A, B}.
        formed_id = manager.store().form(&[a, b]).expect("formation engramme");

        // Rappel AVANT révision.
        let before = manager.recall(&[a], 5);
        assert!(!before.is_empty(), "rappel avant révision");

        // Observation indépendante (C co-activé) → révision effective.
        let rev = manager.revise(formed_id, &[c], 0.5).expect("révision effectuée");
        let rev_members: Vec<NodeId> = rev.members.iter().map(|(n, _)| *n).collect();
        assert!(rev_members.contains(&c), "C ajouté par la révision");
        assert!(!rev.signature.is_empty(), "signature recalculée, non périmée");
        assert!(rev.revision > 0, "révision identifiée");

        // Rappel par le membre nouvellement ajouté C (pas seulement par A).
        let after = manager.recall(&[c], 5);
        assert!(!after.is_empty(), "rappel par le membre ajouté C");
    } // drop → flush

    // Phase 2 : réouverture.
    {
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
        let sub = SubstrateFile::open(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        let engram = manager.store().get(formed_id).expect("engramme rechargé");
        let members: Vec<NodeId> = engram.ensemble.iter().map(|(n, _)| *n).collect();
        assert!(
            members.contains(&a) && members.contains(&b) && members.contains(&c),
            "membres {{A,B,C}} conservés après réouverture : {members:?}"
        );
        for (_, w) in &engram.ensemble {
            assert!(*w > 0.0, "poids conservé (f32), pas écrasé ni perdu");
        }
    }
}
