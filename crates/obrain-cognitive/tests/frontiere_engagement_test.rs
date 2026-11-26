//! Discriminant de la FRONTIÈRE D'ENGAGEMENT du registre d'observation durable
//! `SubstrateAtomicEventLedger`
//! (crates/obrain-cognitive/src/engram/observation_substrate.rs).
//!
//! Scénario : deux ObservationId DISTINCTS K1 et K2, MÊME ressource, MÊME
//! SourceRevision. Les deux passent la pré-vérification, tentent d'engager leur
//! identité ; K1 matérialise son effet, K2 rencontre un conflit de révision.
//!
//! Frontière à affirmer, durablement et sans confusion :
//!   Réservé / Autorisé / Effet engagé / Matérialisation en attente /
//!   Matérialisé / Refusé / Abandonné / Indéterminé.
//!
//! Registre observé : ordre d'engagement (1) déduplication globale sur le nœud
//! autorité, (2) pré-vérification de précondition (lecture seule), (3) commit
//! d'identité `APPLIQUE` atomique, (4) matérialisation conditionnelle de l'effet
//! sur la ressource. Un arrêt entre (3) et (4) laisse « autorisé mais non
//! matérialisé ».

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
// 2) DÉFAUT DOCUMENTÉ : « identité engagée » confondue avec « effet
//    matérialisé ».
//
//    État reproduit : K1 matérialise (révision 0 -> 1) ; K2 COMMIT son identité
//    (APPLIQUE, révision résultante 2) mais son effet n'est PAS matérialisé sur
//    la ressource — l'état laissé par un arrêt entre les étapes (3) et (4), ou
//    par la course « K1 matérialise entre le commit d'identité de K2 et sa
//    matérialisation ».
//
//    `resolve(K2)` rend `Applied` (l'effet de l'identité ENREGISTRÉE), alors que
//    `resource_revision`/`resource_state` ne portent PAS cet effet : l'état
//    « Autorisé / engagé non matérialisé » n'est PAS distinct de « Matérialisé ».
//
//    Emplacement du défaut : `SubstrateAtomicEventLedger::resolve_on`
//    (observation_substrate.rs:541, rend Applied à la ligne 561) et
//    `resolve` (observation_substrate.rs:696, rend Applied à la ligne 706) ne
//    lisent QUE l'enregistrement d'identité (préfixe RECORD_APPLIQUE) et rendent
//    `Applied` SANS vérifier la matérialisation de l'effet sur le nœud ressource.
// ===================================================================

#[test]
fn defaut_identite_engagee_n_est_pas_distincte_de_materialisee() {
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

    // ── Classification observée (sortie réelle) ─────────────────────────────
    let resolve_k1 = ledger.resolve(&oid(1)).unwrap();
    let resolve_k2 = ledger.resolve(&oid(2)).unwrap();
    let resolve_on_k2 = ledger.resolve_on(&rid, &oid(2)).unwrap();
    let rev = ledger.resource_revision(&rid).unwrap();
    let etat = ledger.resource_state(&rid).unwrap();
    eprintln!("resolve(K1)          = {resolve_k1:?}");
    eprintln!("resolve(K2)          = {resolve_k2:?}");
    eprintln!("resolve_on(K2)       = {resolve_on_k2:?}");
    eprintln!("resource_revision    = {rev}");
    eprintln!("resource_state       = {etat:?}");

    // K1 : correctement « Matérialisé » (l'effet est sur la ressource).
    assert!(matches!(
        resolve_k1,
        ResolvedState::Applied { revision: 1, .. }
    ));

    // DÉFAUT observé : K2 rend Applied alors que son effet n'est PAS matérialisé.
    // CORRECT serait : un état distinct « Autorisé / engagé non matérialisé »,
    // JAMAIS `Applied` (qui signifie « effet engagé ET matérialisé »).
    match resolve_k2 {
        ResolvedState::Applied {
            state, revision, ..
        } => {
            eprintln!(
                "DÉFAUT : resolve(K2) = Applied {{ state={state:?}, revision={revision} }} \
                 alors que resource_revision={rev} (effet non matérialisé)"
            );
            assert_eq!(state, eff2);
            assert_eq!(revision, 2);
        }
        other => panic!("comportement inattendu de resolve(K2) : {other:?}"),
    }
    // resolve_on a la MÊME confusion.
    assert!(
        matches!(resolve_on_k2, ResolvedState::Applied { .. }),
        "resolve_on(K2) rend aussi Applied : même confusion engagé/matérialisé"
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
// 3) Classification CORRECTE dérivable aujourd'hui : croiser `resolve`
//    (identité engagée ?) et `resource_state` (effet matérialisé ?) distingue
//    les états. Preuve que l'information existe au niveau SUBSTRAT ; c'est le
//    REGISTRE qui ne l'expose pas comme UN état.
// ===================================================================

#[test]
fn classification_correcte_derivable_mais_non_fournie() {
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

    // Classification CORRECTE (croisement des deux lectures publiques) :
    let identite_engagee =
        matches!(ledger.resolve(&oid(2)).unwrap(), ResolvedState::Applied { .. });
    let effet_materialise = ledger
        .resource_state(&rid)
        .unwrap()
        .map(|(e, rev)| rev == 2 && e == eff2)
        .unwrap_or(false);

    assert!(identite_engagee, "K2 : identité engagée (autorisé)");
    assert!(!effet_materialise, "K2 : effet NON matérialisé");

    // La distinction « Autorisé / engagé non matérialisé » est donc DÉRIVABLE
    // aujourd'hui en croisant deux lectures ; le registre ne l'expose pas comme
    // UN état — `resolve` rend `Applied` sans ce contrôle.
    //
    // Primitive nommée (à coordonner avec le secondaire) :
    //   `ResolvedState::CommittedNotMaterialized` (alias « Autorisé ») — un état
    //   DISTINCT de `Applied`, résolu par `resolve`/`resolve_on` en comparant
    //   l'enregistrement d'identité à la projection ressource (CLE_REVISION /
    //   CLE_EFFET), ou via `SubstrateStore::resoudre_reference` sur la référence
    //   de matérialisation — référence aujourd'hui NON persistée avec l'identité
    //   (l'étape 4 la produit et `apply_avec_reference` la rend, mais l'étape 3
    //   ne la conserve pas : l'arrêt entre 3 et 4 la perd).
    eprintln!(
        "classification correcte : identite_engagee={identite_engagee} effet_materialise={effet_materialise}"
    );
}
