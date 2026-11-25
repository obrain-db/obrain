//! Révision cohérente au niveau d'EngramManager, avec contre-épreuves :
//! poids exacts (règle de reconsolidation), déduplication [C,C], distracteur,
//! indépendance de la signature, réouverture avec tolérances f32.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::traits::InMemoryVectorIndex;
use obrain_cognitive::engram::{EngramManager, EngramManagerConfig, RevisionError};
use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

fn weight_of(ens: &[(NodeId, f64)], n: NodeId) -> f64 {
    ens.iter().find(|(m, _)| *m == n).map(|(_, w)| *w).unwrap_or(0.0)
}

fn members_of(ens: &[(NodeId, f64)]) -> Vec<NodeId> {
    ens.iter().map(|(n, _)| *n).collect()
}

#[test]
fn revision_coherente_survit_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    let (d, e) = (NodeId(4), NodeId(5));

    let formed_id;
    let (before_sig, after_sig);
    {
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
        let sub = SubstrateFile::create(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        formed_id = manager.store().form(&[a, b]).expect("formation engramme");
        // Distracteur : un second engramme {D,E}, pour que le rappel ne soit
        // pas trivial (un seul engramme existerait sinon).
        let distractor = manager.store().form(&[d, e]).expect("distracteur");
        assert_ne!(formed_id, distractor);

        // Rappel AVANT révision + capture de la signature initiale.
        let before = manager.recall(&[a], 5);
        assert!(!before.is_empty(), "rappel avant révision");
        before_sig = before[0].engram.spectral_signature.clone();

        // Observation indépendante (C co-activé) → révision effective.
        let rev = manager.revise(formed_id, &[c], 0.5).expect("révision effectuée");
        after_sig = rev.signature.clone();

        // Règle de reconsolidation : poids unitaires + erreur 0.5 →
        // A=0.95, B=0.95, C=0.25.
        assert!((weight_of(&rev.members, a) - 0.95).abs() < 1e-6, "A décroît à 0.95");
        assert!((weight_of(&rev.members, b) - 0.95).abs() < 1e-6, "B décroît à 0.95");
        assert!((weight_of(&rev.members, c) - 0.25).abs() < 1e-6, "C ajouté à 0.25");
        assert_eq!(rev.members.len(), 3, "pas de doublon : 3 membres");

        // Signature recalculée : non vide ET différente de l'état initial.
        assert!(!after_sig.is_empty(), "signature non vide");
        assert_ne!(before_sig, after_sig, "signature recalculée indépendamment");

        // Rappel par C : doit retrouver l'engramme révisé (pas le distracteur).
        let after = manager.recall(&[c], 5);
        assert!(
            after.iter().any(|r| r.engram_id == formed_id),
            "rappel par C retrouve l'engramme révisé (avec distracteur présent)"
        );
    } // drop → flush

    // Réouverture : état exactement conservé (tolérance f32).
    {
        let manager =
            EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
        let sub = SubstrateFile::open(&path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        manager.attach_substrate(Arc::new(writer));

        let engram = manager.store().get(formed_id).expect("engramme rechargé");
        let members = members_of(&engram.ensemble);
        assert!(members.contains(&a) && members.contains(&b) && members.contains(&c),
            "membres {{A,B,C}} conservés : {members:?}");
        assert!((weight_of(&engram.ensemble, a) - 0.95).abs() < 1e-6, "poids A conservé (f32)");
        assert!((weight_of(&engram.ensemble, b) - 0.95).abs() < 1e-6, "poids B conservé (f32)");
        assert!((weight_of(&engram.ensemble, c) - 0.25).abs() < 1e-6, "poids C conservé (f32)");
    }
}

/// Contre-épreuve : une observation [C, C] ne doit pas ajouter C deux fois.
#[test]
fn reconsolidation_deduplique_les_doublons() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    let rev = manager.revise(id, &[c, c], 0.5).expect("révision");
    let cs: Vec<NodeId> = rev.members.iter().filter(|(n, _)| *n == c).map(|(n, _)| *n).collect();
    assert_eq!(cs.len(), 1, "C ajouté une seule fois malgré [C, C]");
    assert_eq!(rev.members.len(), 3, "3 membres, pas de doublon");
}

/// Observation invalide : erreur non finie → refusée (pas d'état persistant).
#[test]
fn observation_invalide_refusee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));

    let manager =
        EngramManager::new(EngramManagerConfig::default(), Arc::new(InMemoryVectorIndex::new()));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();
    assert!(matches!(
        manager.revise(id, &[c], f64::NAN),
        Err(RevisionError::InvalidObservation(_))
    ));
    // L'engramme reste {A,B} (pas de C ajouté silencieusement).
    let e = manager.store().get(id).unwrap();
    assert_eq!(e.ensemble.len(), 2, "état inchangé après observation invalide");
}

/// Contre-épreuve CONCURRENTE : deux révisions simultanées du même engramme.
/// Sans le verrou par engramme, un read-modify-write entrelacé perdrait l'une
/// des deux additions. La barrière force un départ simultané.
#[test]
fn deux_revisions_concurrentes_coherentes() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c, d) = (NodeId(1), NodeId(2), NodeId(3), NodeId(4));

    let manager = Arc::new(EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    ));
    let sub = SubstrateFile::create(&path).unwrap();
    let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
    manager.attach_substrate(Arc::new(writer));

    let id = manager.store().form(&[a, b]).unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (m1, m2) = (manager.clone(), manager.clone());
    let (b1, b2) = (barrier.clone(), barrier.clone());
    let t1 = std::thread::spawn(move || {
        b1.wait();
        m1.revise(id, &[c], 0.5).expect("révision C");
    });
    let t2 = std::thread::spawn(move || {
        b2.wait();
        m2.revise(id, &[d], 0.5).expect("révision D");
    });
    t1.join().unwrap();
    t2.join().unwrap();

    let e = manager.store().get(id).expect("engramme présent");
    let members = members_of(&e.ensemble);
    assert!(members.contains(&c) && members.contains(&d),
        "les deux révisions sont appliquées (pas de mise à jour perdue) : {members:?}");
    assert_eq!(members.len(), 4, "aucun doublon, aucune perte : {members:?}");
    // Les poids sont décroissants mais cohérents (deux décroissances successives).
    assert!(
        (weight_of(&e.ensemble, a) - 0.9025).abs() < 1e-6,
        "poids A = 0.9025 : deux décroissances successives (sérialisé)"
    );
}
