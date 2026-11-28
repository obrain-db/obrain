//! Incarnation DURABLE d'un engramme : allocation native monotone, liaison
//! attestée, révision et rejeu après réouverture, voisin legacy refusé,
//! observations obsolètes refusées.
//!
//! Les deux bancs `formation_engram_reutilise_slot` (compteur local : 1→1) et
//! `allocateur_engram_reouverture` (allocateur natif : 1→2) restent la mesure de
//! départ ; ce fichier prouve le RACCORD : le manager attaché à un store natif
//! se comporte comme l'allocateur natif (1→2), sur de vrais membres.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::path::Path;
use std::sync::Arc;

use obrain_cognitive::InMemoryVectorIndex;
use obrain_cognitive::engram::{
    AtomicEventLedger, Engram, EngramId, EngramManager, EngramManagerConfig, ObservationId,
    ObservationOutcome, ResolvedState, RevisionError, SourceRevision, SubstrateAtomicEventLedger,
};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{EtatPropriete, SubstrateStore};

/// Point d'entrée EXPLICITE : manager + registre durable + allocateur natif sur
/// UN SEUL store (même domaine).
fn manager_natif(store: &Arc<SubstrateStore>) -> EngramManager {
    let m = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    m.attach_substrate_natif(store.clone());
    m
}

fn ouvrir(chemin: &Path) -> Arc<SubstrateStore> {
    Arc::new(SubstrateStore::open(chemin).unwrap())
}

fn fermer(store: Arc<SubstrateStore>) {
    store.flush().unwrap();
    store.close().unwrap();
}

fn ressource(id: EngramId) -> Vec<u8> {
    let mut r = b"E:".to_vec();
    r.extend_from_slice(&id.0.to_be_bytes());
    r
}

fn obs(n: u128) -> ObservationId {
    ObservationId::from_u128(0xE000_0000_0000_0000 + n)
}

fn applique(
    m: &EngramManager,
    o: ObservationId,
    e: EngramId,
    nodes: &[NodeId],
    rev: u64,
) -> Result<ObservationOutcome, RevisionError> {
    m.revise_with_id(o, e, nodes, 0.5, SourceRevision(rev))
        .map(|(outcome, _)| outcome)
}

/// Vrais membres, formation réelle, révision, réouverture, rejeu, nouvelle
/// révision ; l'identifiant retiré n'est JAMAIS réattribué (1→2).
#[test]
fn formation_native_revision_et_rejeu_apres_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    // ── Session 1 ────────────────────────────────────────────────────────
    let (a, foreign) = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        // NodeId(1) est un nœud ÉTRANGER homonyme de l'engramme n°1.
        let foreign = store.create_node(&["etranger"]);
        let m1 = store.create_node(&["membre"]);
        let m2 = store.create_node(&["membre"]);
        let nouveau = store.create_node(&["membre"]);
        let m = manager_natif(&store);

        let a = m.store().form(&[m1, m2]).expect("former A");
        assert_eq!(a, EngramId(1), "premier id natif");
        assert_eq!(foreign.0, a.0, "le nœud étranger porte le même numéro");

        assert_eq!(m.observation_revision(a).unwrap(), 0);
        let r = applique(&m, obs(1), a, &[nouveau], 0).expect("révision A");
        assert!(
            matches!(r, ObservationOutcome::Applied { revision: 1, .. }),
            "{r:?}"
        );
        assert_eq!(m.observation_revision(a).unwrap(), 1);

        // Le voisin étranger n'a reçu NI révision NI effet.
        for cle in ["obs:revision", "obs:effect"] {
            assert!(matches!(
                store.lire_etat_propriete(foreign, cle).unwrap(),
                EtatPropriete::Absente | EtatPropriete::RessourceInexistante
            ));
        }
        fermer(store);
        (a, foreign)
    };

    // ── Session 2 : réouverture ──────────────────────────────────────────
    let store = ouvrir(&chemin);
    let m = manager_natif(&store);
    assert_eq!(m.store().count(), 1, "l'engramme A est rechargé");
    // La révision d'observation SURVIT (portée par le nœud d'incarnation).
    assert_eq!(m.observation_revision(a).unwrap(), 1);
    // Rejeu de la même identité : AUCUNE seconde application.
    let rejeu = applique(&m, obs(1), a, &[NodeId(foreign.0 + 3)], 0).expect("rejeu");
    assert!(
        matches!(rejeu, ObservationOutcome::Replayed { .. }),
        "{rejeu:?}"
    );
    assert_eq!(m.observation_revision(a).unwrap(), 1, "rejeu sans effet");
    // L'identité résolue après réouverture est confirmée (preuve historique).
    let ledger = SubstrateAtomicEventLedger::new(store.clone());
    assert!(matches!(
        ledger.resolve(&obs(1).0).unwrap(),
        ResolvedState::Applied { .. }
    ));
    // Nouvelle révision sur la révision courante.
    let suiv = applique(&m, obs(2), a, &[NodeId(foreign.0 + 3)], 1).expect("révision 2");
    assert!(
        matches!(suiv, ObservationOutcome::Applied { revision: 2, .. }),
        "{suiv:?}"
    );

    // ── Retrait, formation d'un second, réouverture : 1→2 ────────────────
    m.store().remove(a).expect("retirer A");
    let b = m.store().form(&[NodeId(2), NodeId(3)]).expect("former B");
    assert_ne!(a, b, "l'identifiant retiré n'est PAS réattribué");
    assert_eq!(b, EngramId(2));
    fermer(store);

    let store = ouvrir(&chemin);
    let m = manager_natif(&store);
    let c = m.store().form(&[NodeId(2), NodeId(3)]).expect("former C");
    assert_eq!(c, EngramId(3), "monotone après réouverture (A=1, B=2, C=3)");
}

/// Voisin LEGACY : un engramme formé par l'ancien compteur local n'a aucune
/// provenance. Refus nommé, jamais de correspondance numérique ; le nouvel
/// engramme natif, lui, est révisable et ne reprend pas l'id du legacy.
#[test]
fn voisin_legacy_refuse_et_natif_distinct() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let (legacy, noeud_homonyme) = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        let n1 = store.create_node(&["membre"]);
        let n2 = store.create_node(&["membre"]);
        // Chemin LEGACY : écrivain seul, compteur local.
        let m = EngramManager::new(
            EngramManagerConfig::default(),
            Arc::new(InMemoryVectorIndex::new()),
        );
        m.attach_substrate(store.writer_arc());
        let legacy = m.store().form(&[n1, n2]).expect("former legacy");
        assert_eq!(legacy, EngramId(1));
        assert_eq!(store.next_engram_id(), 1, "le dict natif ignore le legacy");
        fermer(store);
        (legacy, n1)
    };

    let store = ouvrir(&chemin);
    let m = manager_natif(&store);
    assert_eq!(m.store().count(), 1, "legacy rechargé");
    // Plancher relevé : le natif ne rend PAS l'id du legacy.
    let natif = m
        .store()
        .form(&[NodeId(1), NodeId(2)])
        .expect("former natif");
    assert_ne!(natif, legacy, "pas de collision avec le legacy");
    assert_eq!(natif, EngramId(2));

    // Legacy : refus nommé, état intact, identité non engagée.
    let o = obs(10);
    match applique(&m, o, legacy, &[NodeId(2)], 0) {
        Err(RevisionError::PersistenceFailed(msg)) => {
            assert!(msg.contains("incarnation durable"), "{msg}");
        }
        autre => panic!("refus nommé attendu, reçu {autre:?}"),
    }
    assert_eq!(m.store().get(legacy).unwrap().revision, 0);
    for cle in ["obs:revision", "obs:effect"] {
        assert!(matches!(
            store.lire_etat_propriete(noeud_homonyme, cle).unwrap(),
            EtatPropriete::Absente | EtatPropriete::RessourceInexistante
        ));
    }
    let ledger = SubstrateAtomicEventLedger::new(store.clone());
    assert_eq!(ledger.resolve(&o.0).unwrap(), ResolvedState::NotApplied);

    // Voisin natif : révisable.
    let r = applique(&m, obs(11), natif, &[NodeId(3)], 0).expect("révision natif");
    assert!(
        matches!(r, ObservationOutcome::Applied { revision: 1, .. }),
        "{r:?}"
    );
    fermer(store);
}

/// Observations OBSOLÈTES : précondition dépassée → conflit qualifié ; sur une
/// incarnation éteinte (engramme supprimé), refus nommé — avant ET après
/// réouverture.
#[test]
fn observations_obsoletes_refusees() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    let a = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        for _ in 0..4 {
            store.create_node(&["membre"]);
        }
        let m = manager_natif(&store);
        let a = m.store().form(&[NodeId(1), NodeId(2)]).unwrap();
        applique(&m, obs(20), a, &[NodeId(3)], 0).unwrap();
        // Observation calculée sur une révision PÉRIMÉE (0 alors que courante = 1).
        let perimee = applique(&m, obs(21), a, &[NodeId(4)], 0).unwrap();
        assert!(
            matches!(
                perimee,
                ObservationOutcome::RevisionConflict {
                    current_revision: 1
                }
            ),
            "{perimee:?}"
        );
        m.store().remove(a).expect("supprimer A");
        fermer(store);
        a
    };

    let store = ouvrir(&chemin);
    let ledger = SubstrateAtomicEventLedger::new(store.clone());
    // Application directe sur l'incarnation éteinte : refus nommé.
    let err = ledger
        .apply_event(&obs(22).0, &ressource(a), b"def", 1, b"etat", 2)
        .expect_err("incarnation éteinte");
    assert!(err.contains("éteinte"), "{err}");
    assert!(ledger.resource_revision(&ressource(a)).is_err());
    // L'identité refusée n'est pas engagée.
    assert_eq!(
        ledger.resolve(&obs(22).0).unwrap(),
        ResolvedState::NotApplied
    );
}

/// CYCLE DE FORMATION RÉEL : `record_activation` (co-activations répétées →
/// motif → `EngramStore::form`) alloue sur l'allocateur natif et atteste
/// l'incarnation ; le résultat est révisable et survit à la réouverture.
#[test]
fn cycle_de_formation_reel_alloue_et_atteste() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let motif: Vec<NodeId>;

    let (id, rev_appliquee) = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        let n: Vec<NodeId> = (0..4).map(|_| store.create_node(&["membre"])).collect();
        motif = vec![n[0], n[1], n[2]];
        let m = manager_natif(&store);
        let mut formes = Vec::new();
        for _ in 0..5 {
            formes.extend(m.record_activation(&motif, 0.5));
        }
        assert_eq!(formes.len(), 1, "un seul engramme formé : {formes:?}");
        let id = formes[0];
        assert_eq!(id, EngramId(1), "id issu de l'allocateur natif");
        assert_eq!(store.next_engram_id(), 2, "le compteur natif a avancé");
        let r = applique(
            &m,
            obs(30),
            id,
            &[n[3]],
            m.observation_revision(id).unwrap(),
        )
        .expect("révision après formation réelle");
        // La révision est celle de la CONNAISSANCE (la formation en a déjà
        // consommé) : on relève la valeur appliquée, on ne la suppose pas.
        let ObservationOutcome::Applied { revision, .. } = r else {
            panic!("Applied attendu, reçu {r:?}");
        };
        assert!(revision >= 1);
        assert_eq!(m.observation_revision(id).unwrap(), revision);
        fermer(store);
        (id, revision)
    };

    let store = ouvrir(&chemin);
    let m = manager_natif(&store);
    assert_eq!(m.observation_revision(id).unwrap(), rev_appliquee);
    let rejeu = applique(&m, obs(30), id, &[NodeId(4)], 0).expect("rejeu");
    assert!(
        matches!(rejeu, ObservationOutcome::Replayed { .. }),
        "{rejeu:?}"
    );
}

fn incarnations(store: &SubstrateStore) -> usize {
    store
        .nodes_by_label_balayage("__engram_incarnation__")
        .len()
}

/// RÉSERVATION SEULE et DÉSACCORD DE CRASH : ni une réservation d'identifiant
/// sans engramme formé, ni un état natif publié sans attestation, ne sont
/// révisables. L'identifiant réservé n'est jamais réattribué.
#[test]
fn reservation_seule_et_etat_sans_attestation_non_revisables() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (orphelin, sans_attestation, sain) = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        for _ in 0..4 {
            store.create_node(&["membre"]);
        }
        let m = manager_natif(&store);

        // (a) Réservation SEULE : id consommé, aucun engramme, aucune incarnation.
        let orphelin = m.store().next_id();
        assert_eq!(
            incarnations(&store),
            0,
            "aucune attestation à la réservation"
        );
        match m.revise_with_id(obs(40), orphelin, &[NodeId(3)], 0.5, SourceRevision(0)) {
            // Le rattrapage consulte le registre avant le cache : refus nommé
            // (aucune incarnation) ou engramme absent, jamais un succès.
            Err(RevisionError::EngramAbsent(_)) => {}
            Err(RevisionError::PersistenceFailed(msg)) if msg.contains("incarnation durable") => {}
            autre => panic!("refus attendu : {autre:?}"),
        }
        let ledger = SubstrateAtomicEventLedger::new(store.clone());
        assert!(
            ledger
                .apply_event(&obs(40).0, &ressource(orphelin), b"d", 0, b"e", 1)
                .expect_err("réservation orpheline")
                .contains("incarnation durable")
        );

        // (b) DÉSACCORD DE CRASH : état natif publié (id réservé hors du
        // gestionnaire, puis engramme inséré) mais attestation perdue.
        let brut = store.reserve_engram_id().unwrap();
        let sans = EngramId(u64::from(brut));
        m.store()
            .insert(Engram::new(sans, vec![(NodeId(1), 1.0), (NodeId(2), 1.0)]));
        match m.revise_with_id(obs(41), sans, &[NodeId(3)], 0.5, SourceRevision(0)) {
            Err(RevisionError::PersistenceFailed(msg)) => {
                assert!(msg.contains("incarnation durable"), "{msg}");
            }
            autre => panic!("refus nommé attendu : {autre:?}"),
        }
        assert_eq!(m.store().get(sans).unwrap().revision, 0);
        assert_eq!(
            ledger.resolve(&obs(41).0).unwrap(),
            ResolvedState::NotApplied
        );

        // (c) Témoin sain : formation complète → attestée → révisable.
        let sain = m.store().form(&[NodeId(1), NodeId(2)]).unwrap();
        assert_eq!(incarnations(&store), 1);
        applique(&m, obs(42), sain, &[NodeId(3)], 0).expect("sain révisable");
        fermer(store);
        (orphelin, sans, sain)
    };
    assert!(orphelin.0 < sans_attestation.0 && sans_attestation.0 < sain.0);

    // Après réouverture : mêmes verdicts, et l'id suivant ne réutilise rien.
    let store = ouvrir(&chemin);
    let m = manager_natif(&store);
    assert!(m.observation_revision(sans_attestation).is_err());
    assert_eq!(m.observation_revision(sain).unwrap(), 1);
    let suivant = m.store().form(&[NodeId(1), NodeId(2)]).unwrap();
    assert!(suivant.0 > sain.0, "monotone : {suivant} > {sain}");
}

/// ATTACHEMENTS COHÉRENTS : remplacer par l'attachement historique élimine le
/// store natif précédent (allocateur ET registre) ; un registre durable d'un
/// autre domaine refuse l'attachement natif ; le même store est accepté.
#[test]
fn attachements_coherents_un_seul_domaine() {
    let a = Arc::new(SubstrateStore::open_memory().unwrap());
    let b = Arc::new(SubstrateStore::open_memory().unwrap());
    assert_ne!(a.domaine_id(), b.domaine_id(), "deux domaines distincts");
    for _ in 0..3 {
        a.create_node(&["membre"]);
        b.create_node(&["membre"]);
    }

    // natif(A) puis historique(B) : A n'est plus ni allocateur ni registre.
    let m = manager_natif(&a);
    let e1 = m.store().form(&[NodeId(1), NodeId(2)]).unwrap();
    assert_eq!(incarnations(&a), 1);
    let avant = a.next_engram_id();
    m.attach_substrate(b.writer_arc());
    let e2 = m.store().form(&[NodeId(1), NodeId(3)]).unwrap();
    assert_eq!(a.next_engram_id(), avant, "A n'alloue plus");
    assert_eq!(incarnations(&a), 1, "A n'attesté plus rien");
    assert_eq!(
        incarnations(&b),
        0,
        "l'historique n'atteste aucune incarnation"
    );
    assert_ne!(e1, e2);
    // Le registre retombe sur la référence mémoire : plus de lien avec A.
    assert!(m.observation_revision(e2).is_ok());
    assert_eq!(
        ledger_domaine(&a),
        Some(a.domaine_id()),
        "le registre durable de A est inchangé"
    );

    // Registre durable d'un AUTRE domaine : attachement natif refusé, rien modifié.
    let m2 = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    )
    .with_observation_ledger(Arc::new(SubstrateAtomicEventLedger::new(a.clone())));
    let err = m2
        .attach_substrate_store(b.clone())
        .expect_err("domaine différent");
    assert!(err.contains("autre domaine"), "{err}");
    assert!(!m2.store().has_substrate(), "rien n'a été attaché");
    // Même domaine : accepté.
    m2.attach_substrate_store(a.clone()).expect("même domaine");
    assert!(m2.store().has_substrate());
}

fn ledger_domaine(s: &Arc<SubstrateStore>) -> Option<u128> {
    SubstrateAtomicEventLedger::new(s.clone()).domaine_substrat()
}
