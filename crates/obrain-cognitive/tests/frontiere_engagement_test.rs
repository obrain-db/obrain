//! Discriminant de la FRONTIÈRE D'ENGAGEMENT du registre d'observation durable
//! `SubstrateAtomicEventLedger`
//! (crates/obrain-cognitive/src/engram/observation_substrate.rs).
//!
//! Assertions normatives : la classification d'une RÉVISION repose sur la
//! PREUVE TRANSACTIONNELLE HISTORIQUE (référence persévérée +
//! `resoudre_reference` + `verdict_effet`), jamais sur la projection courante.
//! « Effet supplanté » ≠ « jamais matérialisé » : un effet ENGAGÉ puis supplanté
//! se résout en `ResolvedState::Applied` ; un effet dont la preuve historique
//! est absente (jamais engagé OU preuve compactée) se résout en
//! `ResolvedState::Indeterminate` — JAMAIS en « non-engagé ».
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
//! Un arrêt entre (3) et (4) laisse « engagé, matérialisation indéterminée ».

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
// 2) EFFET SUPPLANTÉ ≠ JAMAIS MATÉRIALISÉ (défaut corrigé), durable : K2 est
//    APPLIQUÉE (révision 1 -> 2) puis la projection ressource est remise à
//    l'état post-K1 (écriture ultérieure = supplantation). Après REDÉMARRAGE,
//    la preuve transactionnelle HISTORIQUE (référence persévérée + chaîne
//    canonique persistée) identifie l'effet de K2 comme ENGAGÉ :
//    `resolve(K2)` rend `Applied` — jamais `CommittedNotMaterialized`.
// ===================================================================

#[test]
fn frontiere_effet_supplante_durable_resout_applied() {
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

        // ... puis la projection ressource est remise à l'état post-K1 : une
        // écriture ultérieure SUPPLANTE l'effet de K2 (l'identité de K2 reste
        // COMMISE, et sa tentative reste dans la chaîne canonique persistée).
        store.set_node_property(r, "obs:revision", Value::from(1i64));
        store.set_node_property(r, "obs:effect", Value::from(eff1.clone()));
        store.close().unwrap();
        (r, eff1, def2, eff2)
    };

    // Redémarrage : l'identité COMMISE et la chaîne canonique survivent.
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

    // NORME : K2 = ENGAGÉE historiquement (supplantée par la réécriture), donc
    // `Applied` — JAMAIS `CommittedNotMaterialized` (désormais `Indeterminate`).
    match resolve_k2 {
        ResolvedState::Applied {
            definition,
            state,
            revision,
        } => {
            assert_eq!(definition, def2);
            assert_eq!(state, eff2);
            assert_eq!(revision, 2);
        }
        other => panic!("NORME : resolve(K2) attendu Applied, reçu {other:?}"),
    }
    // resolve_on a la MÊME classification.
    assert!(
        matches!(resolve_on_k2, ResolvedState::Applied { revision: 2, .. }),
        "resolve_on(K2) rend aussi Applied"
    );

    // La projection COURANTE ne porte que K1 (la réécriture l'a remise à K1) —
    // mais cela ne détermine PAS l'engagement historique de K2.
    assert_eq!(rev, 1, "révision ressource = celle de K1 (projection remise à K1)");
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

    // Abandon de K2 : refusé (identité APPLIQUE) — l'état engagé est COLLANT.
    assert_eq!(
        ledger.abandon(&oid(2)).unwrap(),
        AbandonOutcome::AlreadyApplied { revision: 2 }
    );
}

// ===================================================================
// 3) Classification CORRECTE : `resolve` tranche par la preuve historique —
//    un effet ENGAGÉ puis supplanté est `Applied`, jamais « non-engagé ».
// ===================================================================

#[test]
fn classification_supplante_resout_applied() {
    let (ledger, r) = registre_memoire();
    let rid = ressource_bytes(r);
    let def1 = b"def-K1".to_vec();
    let eff1 = b"eff-K1".to_vec();
    let def2 = b"def-K2".to_vec();
    let eff2 = b"eff-K2".to_vec();

    ledger.apply_event(&oid(1), &rid, &def1, 0, &eff1, 1).unwrap();
    ledger.apply_event(&oid(2), &rid, &def2, 1, &eff2, 2).unwrap();
    // Une écriture ultérieure SUPPLANTE l'effet de K2 (projection remise à K1).
    ledger
        .store()
        .set_node_property(r, "obs:revision", Value::from(1i64));
    ledger
        .store()
        .set_node_property(r, "obs:effect", Value::from(eff1.clone()));

    // L'effet de K2 a ÉTÉ engagé : resolve(K2) = Applied (pas Indeterminate).
    assert!(
        matches!(
            ledger.resolve(&oid(2)).unwrap(),
            ResolvedState::Applied { revision: 2, .. }
        ),
        "K2 : effet supplanté → Applied"
    );
    // La projection COURANTE, elle, ne porte plus l'effet de K2.
    let effet_courant = ledger
        .resource_state(&rid)
        .unwrap()
        .map(|(e, rev)| rev == 2 && e == eff2)
        .unwrap_or(false);
    assert!(!effet_courant, "K2 : effet supplanté (plus courant)");
}
