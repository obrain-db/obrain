//! Orchestration cognitive du contrat d'identité d'observation (RFC d59af717),
//! branchée sur le registre de RÉFÉRENCE en mémoire (le mock corrigé).
//!
//! Qualifie : calcul d'empreinte (définition canonique), mapping des issues,
//! conflit de révision → recalcul côté appelant, rejeu sans réapplication,
//! abandon, et non-restauration d'un ancien candidat.

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::traits::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    AtomicEventLedger, Engram, EngramManager, EngramManagerConfig, InMemoryAtomicEventLedger,
    ObservationId, ObservationOutcome, ResultingState, SourceRevision,
};

fn manager() -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
}

fn manager_with(ledger: Arc<InMemoryAtomicEventLedger>) -> EngramManager {
    EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
    .with_observation_ledger(ledger)
}

fn obs(n: u128) -> ObservationId {
    ObservationId::from_u128(n)
}

fn make_engram(manager: &EngramManager, nodes: &[u64]) -> obrain_cognitive::engram::EngramId {
    let id = manager.store().next_id();
    let ensemble: Vec<(NodeId, f64)> = nodes.iter().map(|n| (NodeId(*n), 1.0)).collect();
    manager.store().insert(Engram::new(id, ensemble));
    id
}

/// Application puis rejeu : le rejeu rend `Replayed`, n'applique RIEN.
#[test]
fn application_puis_rejeu() {
    let m = manager();
    let a = NodeId(1);
    let b = NodeId(2);
    let id = make_engram(&m, &[a.0]);
    assert_eq!(m.observation_revision(id).unwrap(), 0);

    let (o1, rev1) = m
        .revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0))
        .unwrap();
    assert!(matches!(o1, ObservationOutcome::Applied { revision: 1, .. }));
    assert!(rev1.is_some(), "une application publie le nouvel état");
    assert_eq!(m.observation_revision(id).unwrap(), 1);
    let e = m.store().get(id).unwrap();
    assert_eq!(e.ensemble.len(), 2, "b ajouté");

    let (o2, rev2) = m
        .revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0))
        .unwrap();
    assert!(matches!(o2, ObservationOutcome::Replayed { revision: 1, .. }));
    assert!(rev2.is_none(), "le rejeu n'applique rien");
    assert_eq!(m.observation_revision(id).unwrap(), 1, "révision inchangée au rejeu");
}

/// Même identité, contenu incompatible → conflit d'identité nommé.
#[test]
fn conflit_d_identite() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    m.revise_with_id(obs(1), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    let (o, _) = m
        .revise_with_id(obs(1), id, &[NodeId(3)], 0.5, SourceRevision(1))
        .unwrap();
    assert!(matches!(o, ObservationOutcome::IdentityConflict { .. }));
}

/// Deux observations de contenu identique mais d'identités différentes = DEUX
/// applications réelles.
#[test]
fn deux_observations_identiques_deux_applications() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    let (o1, _) = m
        .revise_with_id(obs(1), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    let (o2, _) = m
        .revise_with_id(obs(2), id, &[NodeId(2)], 0.5, SourceRevision(1))
        .unwrap();
    assert!(matches!(o1, ObservationOutcome::Applied { revision: 1, .. }));
    assert!(matches!(o2, ObservationOutcome::Applied { revision: 2, .. }));
    assert_eq!(m.observation_revision(id).unwrap(), 2);
}

/// Conflit de révision puis recalcul côté appelant : la seconde observation
/// calculée depuis la même révision est refusée, puis réappliquée depuis la
/// révision courante avec la MÊME identité.
#[test]
fn conflit_de_revision_puis_recalcul() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    // obs1 depuis la révision 0.
    m.revise_with_id(obs(1), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    // obs2 avec la MÊME source révision (0) : périmée.
    let (o2, _) = m
        .revise_with_id(obs(2), id, &[NodeId(3)], 0.5, SourceRevision(0))
        .unwrap();
    match o2 {
        ObservationOutcome::RevisionConflict { current_revision } => {
            assert_eq!(current_revision, 1);
        }
        other => panic!("attendu RevisionConflict, reçu {other:?}"),
    }
    // Rien n'a été appliqué : le nœud 3 n'est PAS dans l'ensemble.
    let e = m.store().get(id).unwrap();
    assert!(!e.ensemble.iter().any(|(n, _)| *n == NodeId(3)), "aucune application sur conflit");

    // Recalcul depuis la révision courante (1), MÊME identité.
    let (o3, _) = m
        .revise_with_id(obs(2), id, &[NodeId(3)], 0.5, SourceRevision(1))
        .unwrap();
    assert!(matches!(o3, ObservationOutcome::Applied { revision: 2, .. }));
    let e = m.store().get(id).unwrap();
    assert!(e.ensemble.iter().any(|(n, _)| *n == NodeId(3)), "appliqué après recalcul");
}

/// Rejeu d'une ancienne observation après de nouvelles révisions : NE restaure
/// JAMAIS son ancien candidat par-dessus une connaissance plus récente.
#[test]
fn rejeu_ancien_ne_restaure_pas() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    m.revise_with_id(obs(1), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    m.revise_with_id(obs(2), id, &[NodeId(3)], 0.5, SourceRevision(1))
        .unwrap();
    let before = m.store().get(id).unwrap();
    assert_eq!(before.ensemble.len(), 3, "état courant : {{1,2,3}}");

    // Rejeu de l'observation ancienne : Replayed, cache intouché.
    let (o, rev) = m
        .revise_with_id(obs(1), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    assert!(matches!(o, ObservationOutcome::Replayed { revision: 1, .. }));
    assert!(rev.is_none());
    let after = m.store().get(id).unwrap();
    assert_eq!(after.ensemble.len(), 3, "l'ancien candidat n'a PAS été restauré");
    assert_eq!(m.observation_revision(id).unwrap(), 2, "révision courante préservée");
}

/// Abandon : une observation abandonnée n'est jamais appliquée.
#[test]
fn abandon_n_applique_jamais() {
    let ledger = Arc::new(InMemoryAtomicEventLedger::new());
    ledger.abandon(&obs(9).0).unwrap();
    let m = manager_with(ledger);
    let id = make_engram(&m, &[1]);
    let (o, rev) = m
        .revise_with_id(obs(9), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    assert_eq!(o, ObservationOutcome::Abandoned);
    assert!(rev.is_none());
    let e = m.store().get(id).unwrap();
    assert_eq!(e.ensemble.len(), 1, "aucune transformation appliquée");
}

// ===================================================================
// Point 2 : échec de matérialisation APRÈS application au registre
// ===================================================================

/// Échec déterministe entre l'application au registre et la matérialisation dans
/// le store : le registre dit Applied, le store est resté en arrière. Le rejeu
/// DOIT réparer (matérialiser l'état enregistré), pas seulement rendre Replayed.
#[cfg(feature = "fault-injection")]
#[test]
fn echec_materialisation_est_repare_au_rejeu() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    let b = NodeId(2);
    assert_eq!(m.observation_revision(id).unwrap(), 0);

    // Le registre applique, la matérialisation échoue.
    m.store().set_fail_materialise(true);
    let failed = m.revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0));
    assert!(failed.is_err(), "l'échec de matérialisation est propagé");

    // Registre : Applied (révision 1). Store : EN RETARD (pas de nœud 2).
    assert_eq!(m.observation_revision(id).unwrap(), 1, "le registre a enregistré l'observation");
    let stale = m.store().get(id).unwrap();
    assert!(
        !stale.ensemble.iter().any(|(n, _)| *n == b),
        "matérialisation absente : le store n'a pas le nœud 2"
    );
    assert_eq!(stale.revision, 0, "révision du store en retard");

    // Rejeu : Replayed ET réparation de la matérialisation manquante.
    let source = SourceRevision(m.observation_revision(id).unwrap());
    let (out, _) = m.revise_with_id(obs(1), id, &[b], 0.5, source).unwrap();
    assert!(matches!(out, ObservationOutcome::Replayed { .. }));

    let repaired = m.store().get(id).unwrap();
    assert!(
        repaired.ensemble.iter().any(|(n, _)| *n == b),
        "le rejeu a matérialisé l'état enregistré"
    );
    assert!(repaired.revision >= 1, "révision du store rattrapée");
}

/// Le rejeu d'une ANCIENNE observation ne restaure JAMAIS son candidat
/// au-dessus d'une connaissance plus récente (révision du store supérieure).
#[cfg(feature = "fault-injection")]
#[test]
fn rejeu_ancien_ne_restaure_pas_une_connaissance_plus_recente() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    // obs1 : appliquée + matérialisée (révision 1).
    m.revise_with_id(obs(1), id, &[NodeId(2)], 0.5, SourceRevision(0))
        .unwrap();
    let after_one = m.store().get(id).unwrap();
    assert!(after_one.revision >= 1);

    // Une mutation HORS observation avance la connaissance (revise sans id).
    m.revise(id, &[NodeId(3)], 0.5).expect("revise sans identité");
    let advanced = m.store().get(id).unwrap();
    assert!(
        advanced.ensemble.iter().any(|(n, _)| *n == NodeId(3)),
        "le nœud 3 a été ajouté par revise"
    );
    let rev_advanced = advanced.revision;
    assert!(rev_advanced > 1);

    // Rejeu de l'observation ancienne.
    let source = SourceRevision(m.observation_revision(id).unwrap());
    let (out, _) = m.revise_with_id(obs(1), id, &[NodeId(2)], 0.5, source).unwrap();
    assert!(matches!(out, ObservationOutcome::Replayed { .. }));

    // La connaissance plus récente n'a PAS été écrasée.
    let final_state = m.store().get(id).unwrap();
    assert!(
        final_state.ensemble.iter().any(|(n, _)| *n == NodeId(3)),
        "le nœud 3 (plus récent) est préservé"
    );
    assert!(final_state.revision >= rev_advanced, "révision non régressée");
}

/// Point 3 : un registre qui applique RÉELLEMENT puis rapporte Indeterminate ;
/// la résolution doit restituer l'état EFFECTIVEMENT enregistré (pas le
/// candidat local) et matérialiser.
#[cfg(feature = "fault-injection")]
#[test]
fn resolution_restitue_l_etat_enregistre() {
    let ledger = Arc::new(InMemoryAtomicEventLedger::new());
    let m = manager_with(Arc::clone(&ledger));
    let id = make_engram(&m, &[1]);
    let b = NodeId(2);

    ledger.set_indeterminate_but_applied(true);
    let (out, _) = m
        .revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0))
        .unwrap();
    // La résolution a restitué l'état enregistré → Replayed, et la
    // matérialisation a eu lieu.
    assert!(matches!(out, ObservationOutcome::Replayed { .. }), "reçu {out:?}");
    let e = m.store().get(id).unwrap();
    assert!(
        e.ensemble.iter().any(|(n, _)| *n == b),
        "l'état EFFECTIVEMENT enregistré a été matérialisé"
    );
}

/// Point C : une NOUVELLE observation arrive AVANT la réparation. Elle ne doit
/// pas partir d'un état obsolète ; le rattrapage répare l'échec antérieur et
/// les DEUX apprentissages sont conservés.
#[cfg(feature = "fault-injection")]
#[test]
fn nouvelle_observation_avant_reparation_ne_part_pas_d_un_etat_obsolete() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    let b = NodeId(2);
    let c = NodeId(3);
    // A : acceptée par le registre, matérialisation échouée.
    m.store().set_fail_materialise(true);
    let failed = m.revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0));
    assert!(failed.is_err());
    assert!(
        !m.store().get(id).unwrap().ensemble.iter().any(|(n, _)| *n == b),
        "store en retard (A non matérialisée)"
    );
    assert_eq!(m.observation_revision(id).unwrap(), 1, "registre a accepté A");

    // B arrive AVANT toute réparation.
    let src = SourceRevision(m.observation_revision(id).unwrap());
    let (out_b, _) = m.revise_with_id(obs(2), id, &[c], 0.5, src).unwrap();
    assert!(matches!(out_b, ObservationOutcome::Applied { .. }), "reçu {out_b:?}");

    let e = m.store().get(id).unwrap();
    assert!(e.ensemble.iter().any(|(n, _)| *n == b), "A (réparée) conservée");
    assert!(e.ensemble.iter().any(|(n, _)| *n == c), "B conservée");
}

/// Point C bis : une mutation SANS identité entre l'échec et la réparation.
#[cfg(feature = "fault-injection")]
#[test]
fn mutation_sans_identite_entre_echec_et_reparation() {
    let m = manager();
    let id = make_engram(&m, &[1]);
    let b = NodeId(2);
    let d = NodeId(4);
    m.store().set_fail_materialise(true);
    assert!(m.revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0)).is_err());
    // Mutation sans identité : avance la connaissance (et la révision du store).
    m.revise(id, &[d], 0.5).expect("revise sans identité");
    assert!(m.store().get(id).unwrap().ensemble.iter().any(|(n, _)| *n == d));
    // Le rejeu de A ne doit PAS écraser la mutation plus récente.
    let src = SourceRevision(m.observation_revision(id).unwrap());
    let (out, _) = m.revise_with_id(obs(1), id, &[b], 0.5, src).unwrap();
    assert!(matches!(out, ObservationOutcome::Replayed { .. }), "reçu {out:?}");
    let e = m.store().get(id).unwrap();
    assert!(e.ensemble.iter().any(|(n, _)| *n == d), "mutation récente préservée");
}

/// Point D : la résolution restitue l'état EFFECTIVEMENT ENREGISTRÉ, même s'il
/// DIFFÈRE du candidat local (fixture qui enregistre un autre état).
#[cfg(feature = "fault-injection")]
#[test]
fn resolution_discrimine_etat_enregistre_du_candidat_local() {
    let ledger = Arc::new(InMemoryAtomicEventLedger::new());
    let m = manager_with(Arc::clone(&ledger));
    let id = make_engram(&m, &[1]);
    let b = NodeId(2);
    // Application normale : révision 1, état enregistré = candidat (1,2).
    m.revise_with_id(obs(1), id, &[b], 0.5, SourceRevision(0)).unwrap();
    // On REMPLACE l'effet enregistré par un état DIFFÉRENT (révision 5).
    let recorded = ResultingState {
        ensemble: vec![(NodeId(1), 0.9), (NodeId(9), 0.9)],
        strength: 0.99,
        recall_count: 7,
    };
    let mut effect = vec![1u8];
    effect.extend_from_slice(&recorded.canonical_bytes());
    ledger.set_recorded_effect(obs(1), effect, 5);
    // Rejeu : la réparation doit matérialiser l'ÉTAT ENREGISTRÉ (nœud 9,
    // révision 5), pas le candidat local (nœud 2).
    let src = SourceRevision(m.observation_revision(id).unwrap());
    let (out, _) = m.revise_with_id(obs(1), id, &[b], 0.5, src).unwrap();
    assert!(matches!(out, ObservationOutcome::Replayed { .. }), "reçu {out:?}");
    let e = m.store().get(id).unwrap();
    assert!(
        e.ensemble.iter().any(|(n, _)| *n == NodeId(9)),
        "l'état ENREGISTRÉ (nœud 9) a été matérialisé"
    );
    assert_eq!(e.revision, 5, "révision enregistrée");
}
