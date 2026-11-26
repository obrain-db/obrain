//! Discriminant de la FRONTIÈRE D'ENGAGEMENT du registre d'observation durable
//! `SubstrateAtomicEventLedger`
//! (crates/obrain-cognitive/src/engram/observation_substrate.rs).
//!
//! Témoin historique (84ae1dd) PRÉSERVÉ + assertions normatives : le défaut
//! « resolve(K2) rend Applied alors que l'effet de K2 n'est pas matérialisé »
//! est CORRIGÉ — K2 se résout désormais en `ResolvedState::CommittedNotMaterialized`
//! (identité engagée, effet non matérialisé, récupérable), JAMAIS en `Applied`.
//!
//! Scénario : deux ObservationId DISTINCTS K1 et K2, MÊME ressource, MÊME
//! SourceRevision. Les deux passent la pré-vérification, tentent d'engager leur
//! identité ; K1 matérialise son effet, K2 rencontre un conflit de révision.
//!
//! Frontière affirmée, durablement et sans confusion :
//!   Réservé / Autorisé / Effet engagé / Matérialisation en attente /
//!   Matérialisé / Refusé / Abandonné / Indéterminé.
//!
//! Registre observé : ordre d'engagement (1) déduplication globale sur le nœud
//! autorité, (2) pré-vérification de précondition (lecture seule), (3) commit
//! d'identité `APPLIQUE` atomique (avec la référence de matérialisation
//! PERSÉVÉRÉE), (4) matérialisation conditionnelle de l'effet sur la ressource.
//! Un arrêt entre (3) et (4) laisse « engagé non matérialisé ».

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_common::types::{NodeId, Value};
use obrain_cognitive::engram::{
    AbandonOutcome, AtomicEventLedger, LedgerOutcome, ResolvedState, SubstrateAtomicEventLedger,
};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;

/// `resource_id` attendu par le registre : `NodeId` big-endian 8 octets.
fn ressource_bytes(id: NodeId) -> Vec<u8> {
    id.0.to_be_bytes().to_vec()
}

/// Un `ObservationId` de test (`[u8; 16]`).
fn oid(n: u128) -> [u8; 16] {
    n.to_be_bytes()
}

/// Store mémoire + une ressource (nœud substrat).
fn registre_memoire() -> (SubstrateAtomicEventLedger, NodeId) {
    let store = Arc::new(SubstrateStore::open_memory().unwrap());
    let r = store.create_node(&["ressource"]);
    (SubstrateAtomicEventLedger::new(store), r)
}

// ===================================================================
// 1) Frontière CORRECTE : K1 matérialise, K2 (même SourceRevision) est
//    refusée AVANT tout engagement — la ressource ne porte QUE l'effet de K1.
// ===================================================================

#[test]
fn frontiere_correcte_k2_refusee_avant_engagement() {
    let (ledger, r) = registre_memoire();
    let rid = ressource_bytes(r);
    let def1 = b"def-K1".to_vec();
    let eff1 = b"eff-K1".to_vec();
    let def2 = b"def-K2".to_vec();
    let eff2 = b"eff-K2".to_vec();

    // K1 : première application, identité + effet matérialisés (révision 0 -> 1).
    let out1 = ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap();
    assert_eq!(out1, LedgerOutcome::Applied);

    // K2 : MÊME SourceRevision (0), désormais périmée : refus à la
    // PRÉ-VÉRIFICATION (lecture seule), AUCUN commit d'identité.
    let out2 = ledger.apply_event(&oid(2), &rid, &def2, 0, &eff2, 2).unwrap();
    assert_eq!(out2, LedgerOutcome::PreconditionFailed { current_revision: 1 });

    // resolve : K1 = Matérialisé ; K2 = jamais engagé (Refusé, pas « Autorisé »).
    match ledger.resolve(&oid(1)).unwrap() {
        ResolvedState::Applied {
            definition,
            state,
            revision,
        } => {
            assert_eq!(definition, def1);
            assert_eq!(state, eff1);
            assert_eq!(revision, 1);
        }
        other => panic!("K1 attendue Applied, reçu {other:?}"),
    }
    assert_eq!(ledger.resolve(&oid(2)).unwrap(), ResolvedState::NotApplied);
    assert_eq!(
        ledger.resolve_on(&rid, &oid(2)).unwrap(),
        ResolvedState::NotApplied
    );

    // Projection ressource : seule K1 y figure.
    assert_eq!(ledger.resource_revision(&rid).unwrap(), 1);
    assert_eq!(ledger.resource_state(&rid).unwrap(), Some((eff1.clone(), 1)));

    // Rejeu de K1 : idempotent, AUCUNE re-matérialisation.
    let rejeu = ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap();
    assert!(matches!(
        rejeu,
        LedgerOutcome::AlreadyApplied { revision: 1, .. }
    ));

    // Abandon : K1 (appliquée) refusé ; K2 (jamais engagée) accepté.
    assert_eq!(
        ledger.abandon(&oid(1)).unwrap(),
        AbandonOutcome::AlreadyApplied { revision: 1 }
    );
    assert_eq!(ledger.abandon(&oid(2)).unwrap(), AbandonOutcome::Abandoned);
}

// ===================================================================
// 2) NORME (défaut CORRIGÉ) : « identité engagée » n'est PLUS confondue avec
//    « effet matérialisé ».
//
//    État reproduit (témoin 84ae1dd) : K1 matérialise (révision 0 -> 1) ; K2
//    COMMIT son identité (APPLIQUE, révision résultante 2) mais son effet n'est
//    PAS matérialisé sur la ressource — l'état laissé par un arrêt entre les
//    étapes (3) et (4), ou par la course « K1 matérialise entre le commit
//    d'identité de K2 et sa matérialisation ».
//
//    CORRIGÉ : `resolve(K2)` et `resolve_on(K2)` comparent l'enregistrement
//    d'identité à la projection ressource (CLE_REVISION / CLE_EFFET) et rendent
//    `CommittedNotMaterialized` — JAMAIS `Applied` (qui exige effet engagé ET
//    matérialisé).
// ===================================================================

#[test]
fn frontiere_corrigee_k2_committed_not_materialized() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (r, eff1, def2, eff2) = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        let r = store.create_node(&["ressource"]);
        let ledger = SubstrateAtomicEventLedger::new(store.clone());
        let rid = ressource_bytes(r);
        let def1 = b"def-K1".to_vec();
        let eff1 = b"eff-K1".to_vec();
        let def2 = b"def-K2".to_vec();
        let eff2 = b"eff-K2".to_vec();

        // K1 matérialisée : révision 0 -> 1.
        assert_eq!(
            ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap(),
            LedgerOutcome::Applied
        );
        // K2 : identité COMMISE + matérialisée (révision 1 -> 2) ...
        assert_eq!(
            ledger.apply_event(&oid(2), &rid, &def2, 1, &eff2, 2).unwrap(),
            LedgerOutcome::Applied
        );

        // ... puis on SIMULE un arrêt après le commit d'identité de K2 (étape 3)
        // et AVANT sa matérialisation (étape 4) : la projection ressource est
        // remise à l'état post-K1, l'identité de K2 reste COMMISE (APPLIQUE).
        store.set_node_property(r, "obs:revision", Value::from(1i64));
        store.set_node_property(r, "obs:effect", Value::from(eff1.clone()));
        store.close().unwrap();
        (r, eff1, def2, eff2)
    };

    // Redémarrage : l'identité COMMISE survit, la matérialisation est absente.
    let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let ledger = SubstrateAtomicEventLedger::new(store);
    let rid = ressource_bytes(r);

    let resolve_k1 = ledger.resolve(&oid(1)).unwrap();
    let resolve_k2 = ledger.resolve(&oid(2)).unwrap();
    let resolve_on_k2 = ledger.resolve_on(&rid, &oid(2)).unwrap();
    let rev = ledger.resource_revision(&rid).unwrap();
    let etat = ledger.resource_state(&rid).unwrap();

    // K1 : correctement « Matérialisé » (l'effet est sur la ressource).
    assert!(matches!(
        resolve_k1,
        ResolvedState::Applied { revision: 1, .. }
    ));

    // NORME : K2 = engagée non matérialisée, JAMAIS Applied.
    match resolve_k2 {
        ResolvedState::CommittedNotMaterialized {
            definition,
            state,
            revision,
        } => {
            assert_eq!(definition, def2);
            assert_eq!(state, eff2);
            assert_eq!(revision, 2);
        }
        other => panic!("NORME : resolve(K2) attendu CommittedNotMaterialized, reçu {other:?}"),
    }
    // resolve_on a la MÊME classification.
    assert!(
        matches!(resolve_on_k2, ResolvedState::CommittedNotMaterialized { .. }),
        "resolve_on(K2) rend aussi CommittedNotMaterialized"
    );

    // La ressource ne porte que K1 : K2 n'y a jamais matérialisé son effet.
    assert_eq!(rev, 1, "révision ressource = celle de K1 (K2 jamais matérialisé)");
    assert_eq!(etat, Some((eff1, 1)), "état ressource = effet de K1 seulement");

    // Rejeu de K2 : AlreadyApplied SANS matérialiser (le registre ne répare PAS
    // lui-même la projection — la réparation relève du manager).
    let rejeu = ledger
        .apply_event(&oid(2), &rid, &def2, 1, &eff2, 2)
        .unwrap();
    assert!(
        matches!(rejeu, LedgerOutcome::AlreadyApplied { revision: 2, .. }),
        "rejeu = AlreadyApplied (l'effet enregistré est restitué, pas matérialisé)"
    );
    assert_eq!(
        ledger.resource_revision(&rid).unwrap(),
        1,
        "le rejeu ne matérialise pas (registre)"
    );

    // Abandon de K2 : refusé (identité APPLIQUE) — l'état « engagé non
    // matérialisé » est COLLANT au niveau du registre.
    assert_eq!(
        ledger.abandon(&oid(2)).unwrap(),
        AbandonOutcome::AlreadyApplied { revision: 2 }
    );
}

// ===================================================================
// 3) Classification CORRECTE désormais FOURNIE : `resolve` rend UN état
//    distinct (`CommittedNotMaterialized`) sans croiser deux lectures.
// ===================================================================

#[test]
fn classification_correcte_desormais_fournie() {
    let (ledger, r) = registre_memoire();
    let rid = ressource_bytes(r);
    let def1 = b"def-K1".to_vec();
    let eff1 = b"eff-K1".to_vec();
    let def2 = b"def-K2".to_vec();
    let eff2 = b"eff-K2".to_vec();

    ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap();
    ledger.apply_event(&oid(2), &rid, &def2, 1, &eff2, 2).unwrap();
    // Simule la perte de matérialisation de K2 (arrêt entre les étapes 3 et 4).
    ledger
        .store()
        .set_node_property(r, "obs:revision", Value::from(1i64));
    ledger
        .store()
        .set_node_property(r, "obs:effect", Value::from(eff1.clone()));

    // La distinction est désormais FOURNIE comme UN état, plus dérivable.
    assert!(
        matches!(
            ledger.resolve(&oid(2)).unwrap(),
            ResolvedState::CommittedNotMaterialized { .. }
        ),
        "K2 : resolve rend directement CommittedNotMaterialized"
    );
    let effet_materialise = ledger
        .resource_state(&rid)
        .unwrap()
        .map(|(e, rev)| rev == 2 && e == eff2)
        .unwrap_or(false);
    assert!(!effet_materialise, "K2 : effet NON matérialisé");
}
