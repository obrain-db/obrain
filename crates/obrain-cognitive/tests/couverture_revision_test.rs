//! Couverture des branches d'erreur et des cas limites de la révision cognitive
//! (lot 2, PR #418) : identité d'observation, discipline de mutation, réparation.
//!
//! Chaque test ASSERTE le comportement réel (valeur observée), pas un simple
//! appel « pour la couverture ».

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_cognitive::engram::traits::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    AbandonOutcome, AtomicEventLedger, CognitiveObservationAdapter, Engram, EngramId,
    EngramManager, EngramManagerConfig, InMemoryAtomicEventLedger, LedgerOutcome,
    ObservationDefinition, ObservationId, ObservationOutcome, ResolvedState, ResultingState,
    RevisionError, SourceRevision, StoreError,
};
use obrain_common::types::NodeId;

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

fn manager_with(ledger: Arc<dyn AtomicEventLedger>) -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
    .with_observation_ledger(ledger)
}

fn make_engram(m: &EngramManager, nodes: &[u64]) -> EngramId {
    let id = m.store().next_id();
    let ensemble: Vec<(NodeId, f64)> = nodes.iter().map(|n| (NodeId(*n), 1.0)).collect();
    m.store().insert(Engram::new(id, ensemble));
    id
}

// ===================================================================
// Identité d'observation : accesseurs, Display, empreinte
// ===================================================================

#[test]
fn identifiant_d_observation_affiche_et_arrondit() {
    let oid = ObservationId::from_u128(0x1234_5678_9abc_def0_0fed_cba9_8765_4321);
    assert_eq!(
        oid.as_u128(),
        0x1234_5678_9abc_def0_0fed_cba9_8765_4321,
        "as_u128 restitue la valeur encodée"
    );
    assert_eq!(
        format!("{oid}"),
        "obs:123456789abcdef00fedcba987654321",
        "Display écrit l'hexadécimal zéro-paddé sur 32 chiffres"
    );
}

#[test]
fn empreinte_de_definition_est_deterministe_et_non_triviale() {
    let a = ObservationDefinition::new(&[NodeId(2), NodeId(1)], 0.5).unwrap();
    let b = ObservationDefinition::new(&[NodeId(1), NodeId(2)], 0.5).unwrap();
    // Canonisation : l'ordre des nœuds ne change ni la définition ni l'empreinte.
    assert_eq!(a, b);
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_ne!(a.fingerprint(), 0, "empreinte non triviale");
    let autre = ObservationDefinition::new(&[NodeId(3)], 0.5).unwrap();
    assert_ne!(
        a.fingerprint(),
        autre.fingerprint(),
        "contenu différent → empreinte différente"
    );
}

// ===================================================================
// Registre de référence : résolution / abandon / confirmation
// ===================================================================

#[test]
fn resolution_d_un_evenement_inconnu_est_non_applique() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    assert_eq!(
        a.resolve(ObservationId::from_u128(42)).unwrap(),
        ResolvedState::NotApplied,
        "un identifiant jamais vu n'a jamais été appliqué"
    );
}

#[test]
fn resolution_d_un_evenement_abandonne_rend_abandonne() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(7);
    assert_eq!(a.abandon(oid).unwrap(), AbandonOutcome::Abandoned);
    assert_eq!(
        a.resolve(oid).unwrap(),
        ResolvedState::Abandoned,
        "un abandon est résolu comme abandonné, pas comme non-appliqué"
    );
}

#[test]
fn abandon_d_un_deja_abandonne_reste_abandonne() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(7);
    a.abandon(oid).unwrap();
    assert_eq!(
        a.abandon(oid).unwrap(),
        AbandonOutcome::Abandoned,
        "abandonner deux fois reste un abandon (idempotent)"
    );
}

#[test]
fn confirmation_d_une_formation_inexistante_rend_faux() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    assert!(
        !a.complete_formation(ObservationId::from_u128(1)).unwrap(),
        "rien à confirmer : la transition n'a pas eu lieu"
    );
}

#[test]
fn rejeu_d_un_effet_de_revision_tronque_est_une_erreur_nommee() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(1);
    let d = ObservationDefinition::new(&[NodeId(1)], 0.5).unwrap();
    let s = ResultingState {
        ensemble: vec![(NodeId(1), 0.5)],
        strength: 0.5,
        recall_count: 1,
    };
    a.apply(
        oid,
        EngramId(10),
        &10u64.to_be_bytes(),
        &d,
        SourceRevision(0),
        &s,
        1,
    )
    .unwrap();
    // Effet de révision tronqué (moins de 9 octets) : refusé, jamais décodé.
    a.ledger().set_recorded_effect(oid, vec![0x01], 1);
    let r = a.apply(
        oid,
        EngramId(10),
        &10u64.to_be_bytes(),
        &d,
        SourceRevision(0),
        &s,
        1,
    );
    assert!(r.is_err(), "effet de révision tronqué refusé");
}

#[test]
fn rejeu_d_un_effet_a_prefixe_inconnu_est_une_erreur_nommee() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(1);
    let d = ObservationDefinition::new(&[NodeId(1)], 0.5).unwrap();
    let s = ResultingState {
        ensemble: vec![(NodeId(1), 0.5)],
        strength: 0.5,
        recall_count: 1,
    };
    a.apply(
        oid,
        EngramId(10),
        &10u64.to_be_bytes(),
        &d,
        SourceRevision(0),
        &s,
        1,
    )
    .unwrap();
    a.ledger().set_recorded_effect(oid, vec![0xFF], 1);
    let r = a.apply(
        oid,
        EngramId(10),
        &10u64.to_be_bytes(),
        &d,
        SourceRevision(0),
        &s,
        1,
    );
    assert!(r.is_err(), "préfixe d'effet inconnu refusé");
}

#[test]
fn rejeu_d_une_formation_a_prefixe_inconnu_est_deja_formee() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(1);
    let d = ObservationDefinition::new(&[NodeId(1)], 0.5).unwrap();
    a.apply_formation(oid, b"F", &d).unwrap();
    a.ledger().set_recorded_effect(oid, vec![0xFF], 1);
    assert_eq!(
        a.apply_formation(oid, b"F", &d).unwrap(),
        ObservationOutcome::AlreadyFormed,
        "préfixe inconnu d'une formation → traitée comme déjà formée"
    );
}

#[test]
fn formation_d_un_evenement_abandonne_rend_abandonne() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(1);
    a.abandon(oid).unwrap();
    let d = ObservationDefinition::new(&[NodeId(1)], 0.5).unwrap();
    assert_eq!(
        a.apply_formation(oid, b"F", &d).unwrap(),
        ObservationOutcome::Abandoned,
        "une formation abandonnée ne s'applique jamais"
    );
}

#[test]
fn formation_meme_identite_contenu_different_est_un_conflit() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let oid = ObservationId::from_u128(1);
    let d1 = ObservationDefinition::new(&[NodeId(1)], 0.5).unwrap();
    let d2 = ObservationDefinition::new(&[NodeId(2)], 0.5).unwrap();
    a.apply_formation(oid, b"F", &d1).unwrap();
    match a.apply_formation(oid, b"F", &d2).unwrap() {
        ObservationOutcome::IdentityConflict { recorded } => {
            assert_eq!(recorded, d1, "définition ENREGISTRÉE restituée");
        }
        other => panic!("attendu IdentityConflict, reçu {other:?}"),
    }
}

#[test]
fn formation_sur_une_ressource_deja_revisee_est_un_conflit_de_revision() {
    let a = CognitiveObservationAdapter::new(InMemoryAtomicEventLedger::new());
    let d = ObservationDefinition::new(&[NodeId(1)], 0.5).unwrap();
    a.apply_formation(ObservationId::from_u128(1), b"F", &d)
        .unwrap();
    // Même ressource, autre identité : la ressource a avancé → conflit de révision.
    assert_eq!(
        a.apply_formation(ObservationId::from_u128(2), b"F", &d)
            .unwrap(),
        ObservationOutcome::RevisionConflict {
            current_revision: 1
        },
        "la formation calculée depuis la révision 0 est périmée"
    );
}

// ===================================================================
// Manager : branches d'erreur de revise / revise_with_id / rattrapage
// ===================================================================

#[test]
fn revise_refuse_une_observation_vide() {
    let m = manager();
    assert!(matches!(
        m.revise(EngramId(1), &[], 0.5),
        Err(RevisionError::InvalidObservation(_))
    ));
}

#[test]
fn revise_d_un_engramme_absent_rend_engramme_absent() {
    let m = manager();
    assert!(matches!(
        m.revise(EngramId(999), &[NodeId(1)], 0.5),
        Err(RevisionError::EngramAbsent(_))
    ));
}

#[test]
fn revise_avec_identite_refuse_une_erreur_non_finie() {
    let m = manager();
    assert!(matches!(
        m.revise_with_id(
            ObservationId::from_u128(1),
            EngramId(1),
            &[NodeId(1)],
            f64::NAN,
            SourceRevision(0)
        ),
        Err(RevisionError::InvalidObservation(_))
    ));
}

#[test]
fn revise_avec_identite_refuse_une_observation_vide() {
    let m = manager();
    assert!(matches!(
        m.revise_with_id(
            ObservationId::from_u128(1),
            EngramId(1),
            &[],
            0.5,
            SourceRevision(0)
        ),
        Err(RevisionError::InvalidObservation(_))
    ));
}

/// Registre qui déclare un effet de ressource NON-révision (préfixe formation) :
/// le rattrapage doit refuser, jamais l'interpréter comme une révision.
struct EffetNonRevision;

impl AtomicEventLedger for EffetNonRevision {
    fn resource_revision(&self, _: &[u8]) -> Result<u64, String> {
        Ok(0)
    }
    fn resource_state(&self, _: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String> {
        Ok(Some((vec![0x02], 5)))
    }
    fn apply_event(
        &self,
        _: &[u8; 16],
        _: &[u8],
        _: &[u8],
        _: u64,
        _: &[u8],
        _: u64,
    ) -> Result<LedgerOutcome, String> {
        Ok(LedgerOutcome::Applied)
    }
    fn resolve(&self, _: &[u8; 16]) -> Result<ResolvedState, String> {
        Ok(ResolvedState::NotApplied)
    }
    fn abandon(&self, _: &[u8; 16]) -> Result<AbandonOutcome, String> {
        Ok(AbandonOutcome::Abandoned)
    }
    fn complete_formation(&self, _: &[u8; 16]) -> Result<bool, String> {
        Ok(false)
    }
}

#[test]
fn rattrapage_refuse_un_effet_non_revision() {
    let m = manager_with(Arc::new(EffetNonRevision));
    let id = make_engram(&m, &[1]);
    assert!(matches!(
        m.revise_with_id(
            ObservationId::from_u128(1),
            id,
            &[NodeId(2)],
            0.5,
            SourceRevision(0)
        ),
        Err(RevisionError::PersistenceFailed(_))
    ));
}

/// Registre qui déclare un état ENREGISTRÉ invalide (membre dupliqué) : la
/// réparation doit propager l'échec de validation, jamais l'écraser.
struct EtatDupliqueAReparer;

impl AtomicEventLedger for EtatDupliqueAReparer {
    fn resource_revision(&self, _: &[u8]) -> Result<u64, String> {
        Ok(0)
    }
    fn resource_state(&self, _: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String> {
        let state = ResultingState {
            ensemble: vec![(NodeId(1), 1.0), (NodeId(1), 1.0)],
            strength: 0.5,
            recall_count: 0,
        };
        let mut effect = vec![0x01];
        effect.extend_from_slice(&0u64.to_be_bytes());
        effect.extend_from_slice(&state.canonical_bytes());
        Ok(Some((effect, 5)))
    }
    fn apply_event(
        &self,
        _: &[u8; 16],
        _: &[u8],
        _: &[u8],
        _: u64,
        _: &[u8],
        _: u64,
    ) -> Result<LedgerOutcome, String> {
        Ok(LedgerOutcome::Applied)
    }
    fn resolve(&self, _: &[u8; 16]) -> Result<ResolvedState, String> {
        Ok(ResolvedState::NotApplied)
    }
    fn abandon(&self, _: &[u8; 16]) -> Result<AbandonOutcome, String> {
        Ok(AbandonOutcome::Abandoned)
    }
    fn complete_formation(&self, _: &[u8; 16]) -> Result<bool, String> {
        Ok(false)
    }
}

#[test]
fn rattrapage_propage_un_etat_enregistre_invalide() {
    let m = manager_with(Arc::new(EtatDupliqueAReparer));
    let id = make_engram(&m, &[1]);
    assert!(matches!(
        m.revise_with_id(
            ObservationId::from_u128(1),
            id,
            &[NodeId(2)],
            0.5,
            SourceRevision(0)
        ),
        Err(RevisionError::ValidationFailed(_))
    ));
}

// ===================================================================
// Store : discipline de mutation, matérialisation, réparation
// ===================================================================

#[test]
fn mutation_d_un_engramme_absent_rend_engramme_absent() {
    let m = manager();
    assert!(matches!(
        m.store().mutate(EngramId(999), |_| {}),
        Err(StoreError::EngramAbsent(_))
    ));
}

#[test]
fn mise_a_jour_d_un_engramme_absent_n_ecrit_rien() {
    let m = manager();
    m.store().update(EngramId(999), |e| e.strength += 0.1);
    assert_eq!(m.store().count(), 0, "aucun engramme créé par l'échec");
}

#[test]
fn materialisation_d_un_engramme_absent_rend_engramme_absent() {
    let m = manager();
    assert!(matches!(
        m.store()
            .materialize_state(EngramId(999), 0, vec![], 0.0, 0, vec![], 1),
        Err(StoreError::EngramAbsent(_))
    ));
}

#[test]
fn materialisation_refuse_une_revision_non_croissante() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    // Révision courante 0, nouvelle révision 0 : non croissante → refus.
    assert!(matches!(
        m.store()
            .materialize_state(id, 0, vec![(NodeId(1), 1.0)], 0.5, 0, vec![], 0),
        Err(StoreError::ValidationFailed(_))
    ));
}

#[test]
fn materialisation_sur_une_revision_perimee_rend_un_conflit_sans_ecrire() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    let before = m.store().get(id).unwrap().clone();
    // La connaissance attendue (révision 5) n'est pas la révision courante (0).
    let r = m
        .store()
        .materialize_state(id, 5, vec![(NodeId(2), 1.0)], 0.5, 0, vec![], 6);
    assert!(r.is_ok(), "conflit qualifié (pas une erreur)");
    let after = m.store().get(id).unwrap();
    assert_eq!(after.ensemble, before.ensemble, "rien n'a été écrit");
    assert_eq!(after.revision, before.revision, "révision inchangée");
}

#[test]
fn reparation_d_un_engramme_absent_rend_engramme_absent() {
    let m = manager();
    assert!(matches!(
        m.store()
            .repair_state(EngramId(999), 1, vec![], 0.0, 0, vec![]),
        Err(StoreError::EngramAbsent(_))
    ));
}

#[test]
fn reparation_d_un_etat_deja_a_jour_ne_fait_rien() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    assert!(
        !m.store()
            .repair_state(id, 0, vec![], 0.0, 0, vec![])
            .unwrap(),
        "déjà à jour : aucune réparation"
    );
}

// ===================================================================
// Store + substrate : miroir, chargement, rappel natif
// ===================================================================

#[cfg(feature = "substrate")]
mod substrate {
    use super::*;
    use obrain_substrate::{SubstrateFile, SyncMode, Writer as SubstrateWriter};

    fn manager_with_substrate(path: &std::path::Path) -> EngramManager {
        let m = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let sub = SubstrateFile::create(path).unwrap();
        let writer = SubstrateWriter::new(sub, SyncMode::Never).unwrap();
        m.attach_substrate(Arc::new(writer));
        m
    }

    #[test]
    fn chargement_sans_ecrivain_rend_un_bilan_vide() {
        let m = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        let report = m.store().load_from_substrate();
        assert_eq!(report.coherent, 0, "aucun engramme chargé");
        assert!(
            report.global_failure.is_none(),
            "pas de défaillance sans zone"
        );
    }

    #[test]
    fn miroir_refuse_un_identifiant_non_u16() {
        let td = tempfile::tempdir().unwrap();
        let m = manager_with_substrate(&td.path().join("kb"));
        let id = EngramId(1_000_000); // > u16::MAX : hors adressage substrate.
        m.store().insert(Engram::new(id, vec![(NodeId(1), 1.0)]));
        assert!(matches!(
            m.store().mutate(id, |e| e.strength += 0.1),
            Err(StoreError::ValidationFailed(_))
        ));
    }

    #[test]
    fn miroir_refuse_un_noeud_non_u32() {
        let td = tempfile::tempdir().unwrap();
        let m = manager_with_substrate(&td.path().join("kb"));
        let id = m.store().next_id();
        m.store()
            .insert(Engram::new(id, vec![(NodeId(u64::MAX), 1.0)]));
        assert!(matches!(
            m.store().mutate(id, |e| e.strength += 0.1),
            Err(StoreError::ValidationFailed(_))
        ));
    }

    #[test]
    fn rappel_natif_sans_ecrivain_est_une_erreur() {
        let m = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        assert!(m.store().recall(NodeId(1)).is_err());
    }

    #[test]
    fn rappel_natif_d_un_noeud_non_u32_est_une_erreur() {
        let td = tempfile::tempdir().unwrap();
        let m = manager_with_substrate(&td.path().join("kb"));
        assert!(m.store().recall(NodeId(u64::MAX)).is_err());
    }
}
