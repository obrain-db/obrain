//! Contre-épreuve d'identité INTER-RESSOURCES du registre d'observation durable
//! `SubstrateAtomicEventLedger` (crates/obrain-cognitive/src/engram/observation_substrate.rs).
//!
//! QUESTION EXERCÉE (pas une prédiction) : l'`ObservationId` (clé de propriété
//! `obs:<hex>`) est-il GLOBAL ou SCOPÉ PAR RESSOURCE dans l'implémentation durable ?
//!
//! Contraste à garder en tête : le registre VOLATIL (`InMemoryAtomicEventLedger`)
//! tient `HashMap<[u8;16], EventEntry>` → identité GLOBALE (une même observation
//! rejouée sur une AUTRE connaissance est refusée ; cf.
//! `tests/observation_identity_test.rs::rejeu_vers_une_autre_connaissance_est_refuse`).
//! Le registre DURABLE, lui, écrit `obs:<hex>` comme propriété D'UN nœud ressource
//! (`capturer_precondition` mono-cible). Ce test EXERCE le comportement réel du
//! registre durable et classe chaque issue observée.
//!
//! Classification retenue des issues observées :
//!   Applied          -> « Nouvel effet autorisé » (identité scopée par ressource)
//!   AlreadyApplied   -> « Rejeu » (même identité, même définition)
//!   IdentityConflict -> « Conflit d'identité » (même identité, définition autre)
//!   NotApplied       -> « Substitution incompatible » (l'identité ne migre pas)
//!   Indeterminate    -> « Résultat indéterminé »

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::sync::Arc;

use obrain_common::types::NodeId;
use obrain_cognitive::engram::{
    AtomicEventLedger, LedgerOutcome, ResolvedState, SubstrateAtomicEventLedger,
};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{EtatPropriete, SubstrateStore};

/// `resource_id` attendu par le registre : `NodeId` en big-endian 8 octets.
fn ressource_bytes(id: NodeId) -> Vec<u8> {
    id.0.to_be_bytes().to_vec()
}

/// Un `ObservationId` de test (`[u8; 16]`).
fn oid(n: u128) -> [u8; 16] {
    n.to_be_bytes()
}

/// Un store mémoire + DEUX ressources distinctes A et B (deux nœuds substrat).
fn deux_ressources() -> (SubstrateAtomicEventLedger, NodeId, NodeId) {
    let store = Arc::new(SubstrateStore::open_memory().unwrap());
    let a = store.create_node(&["ressource"]);
    let b = store.create_node(&["ressource"]);
    (SubstrateAtomicEventLedger::new(store), a, b)
}

/// Valeur brute de la propriété `obs:<hex>` sur un nœud (diagnostic physique :
/// y a-t-il un enregistrement durable sur CE nœud ?).
fn propriete_evenement(
    ledger: &SubstrateAtomicEventLedger,
    cible: NodeId,
    ev: &[u8; 16],
) -> Option<Vec<u8>> {
    let mut cle = String::with_capacity(4 + 32);
    cle.push_str("obs:");
    for b in ev {
        cle.push_str(&format!("{b:02x}"));
    }
    match ledger.store().lire_etat_propriete(cible, &cle).unwrap() {
        EtatPropriete::Presente { valeur, .. } => valeur.as_bytes().map(|b| b.to_vec()),
        _ => None,
    }
}

// S1 — même ObservationId K, même définition, sur A puis B.
#[test]
fn s1_meme_k_sur_a_puis_b_meme_definition() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(1);
    let def = b"def-D".to_vec();
    let out_a = ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    let out_b = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    eprintln!("S1 out_a={out_a:?} out_b={out_b:?}");
    assert_eq!(out_a, LedgerOutcome::Applied);
    // Détecteur de portée : si l'identité était GLOBALE, B rendrait
    // `AlreadyApplied`. Observé : B rend `Applied` -> identité scopée par ressource.
    assert_eq!(out_b, LedgerOutcome::Applied);
    // Deux enregistrements physiques distincts (un par nœud).
    assert!(
        propriete_evenement(&ledger, a, &k).is_some(),
        "enregistrement physique sur A"
    );
    assert!(
        propriete_evenement(&ledger, b, &k).is_some(),
        "enregistrement physique sur B"
    );
    assert_eq!(ledger.resource_revision(&ressource_bytes(a)).unwrap(), 1);
    assert_eq!(ledger.resource_revision(&ressource_bytes(b)).unwrap(), 1);
}

// S2 — même ObservationId K, définition D1 sur A puis D2 (incompatible) sur B.
#[test]
fn s2_k_sur_a_d1_puis_k_sur_b_d2_incompatible() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(2);
    let out_a = ledger
        .apply_event(&k, &ressource_bytes(a), b"D1", 0, b"eff-A", 1)
        .unwrap();
    let out_b = ledger
        .apply_event(&k, &ressource_bytes(b), b"D2", 0, b"eff-B", 1)
        .unwrap();
    eprintln!("S2 out_a={out_a:?} out_b={out_b:?}");
    assert_eq!(out_a, LedgerOutcome::Applied);
    // Si l'identité était GLOBALE, B rendrait `IdentityConflict` (définition D1
    // enregistrée). Observé : B applique D2 sans conflit -> aucun conflit
    // d'identité INTER-ressources.
    assert_eq!(out_b, LedgerOutcome::Applied);
    assert_eq!(ledger.resource_revision(&ressource_bytes(b)).unwrap(), 1);
}

// S3 — ressource substituée : K appliquée sur A, puis on interroge/applique sur B.
#[test]
fn s3_ressource_substituee() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(3);
    let def = b"def-D".to_vec();
    ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();

    // Résolution sur A (la ressource d'origine) : appliquée.
    let resolved_a = ledger.resolve_on(&ressource_bytes(a), &k).unwrap();
    // Résolution sur B (ressource substituée) : l'identité y est-elle ?
    let resolved_b = ledger.resolve_on(&ressource_bytes(b), &k).unwrap();
    eprintln!("S3 resolved_a={resolved_a:?} resolved_b={resolved_b:?}");

    assert!(matches!(resolved_a, ResolvedState::Applied { .. }));
    assert_eq!(
        resolved_b,
        ResolvedState::NotApplied,
        "l'identité ne migre pas vers B : substitution incompatible"
    );

    // La substitution sur B crée un enregistrement NEUF, indépendant de A.
    let out_b = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    eprintln!("S3 out_b_apres_substitution={out_b:?}");
    assert_eq!(out_b, LedgerOutcome::Applied);
}

// S4 — perte de réponse : ré-application (rejeu) de K sur A vs sur B.
#[test]
fn s4_perte_reponse_rejeu_sur_a_vs_b() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(4);
    let def = b"def-D".to_vec();
    let out_a = ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    assert_eq!(out_a, LedgerOutcome::Applied);

    // Réponse perdue : rejeu identique sur A -> idempotent.
    let rejeu_a = ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    // Réponse perdue : ré-application du MÊME K sur B -> nouvel effet.
    let rejeu_b = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    eprintln!("S4 rejeu_a={rejeu_a:?} rejeu_b={rejeu_b:?}");

    assert!(
        matches!(rejeu_a, LedgerOutcome::AlreadyApplied { revision: 1, .. }),
        "rejeu sur A : idempotent"
    );
    assert_eq!(
        rejeu_b,
        LedgerOutcome::Applied,
        "même K sur B : nouvel effet (pas un rejeu) -> portée par ressource"
    );
    assert_eq!(ledger.resource_revision(&ressource_bytes(a)).unwrap(), 1);
    assert_eq!(ledger.resource_revision(&ressource_bytes(b)).unwrap(), 1);
}

// S5 — redémarrage (create/close/open) puis résolution de K.
#[test]
fn s5_redemarrage_puis_resolution() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (a, b) = {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        let a = store.create_node(&["ressource"]);
        let b = store.create_node(&["ressource"]);
        let ledger = SubstrateAtomicEventLedger::new(store.clone());
        ledger
            .apply_event(&oid(5), &ressource_bytes(a), b"def-D", 0, b"eff-A", 1)
            .unwrap();
        store.close().unwrap();
        (a, b)
    };

    // Réouverture : la portée physique de K survit-elle au redémarrage ?
    let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let ledger = SubstrateAtomicEventLedger::new(store);
    let resolved_a = ledger.resolve_on(&ressource_bytes(a), &oid(5)).unwrap();
    let resolved_b = ledger.resolve_on(&ressource_bytes(b), &oid(5)).unwrap();
    eprintln!("S5 resolved_a={resolved_a:?} resolved_b={resolved_b:?}");

    assert!(
        matches!(resolved_a, ResolvedState::Applied { revision: 1, .. }),
        "K durable sur A après redémarrage"
    );
    assert_eq!(
        resolved_b,
        ResolvedState::NotApplied,
        "K inconnue sur B après redémarrage -> portée par ressource persistante"
    );

    // Rejeu de K sur A après redémarrage : idempotent.
    let rejeu = ledger
        .apply_event(&oid(5), &ressource_bytes(a), b"def-D", 0, b"eff-A", 1)
        .unwrap();
    eprintln!("S5 rejeu_a_apres_redemarrage={rejeu:?}");
    assert!(matches!(rejeu, LedgerOutcome::AlreadyApplied { revision: 1, .. }));
}

// S6 — deux applications concurrentes de K sur A et B.
#[test]
fn s6_deux_applications_concurrentes_k_sur_a_et_b() {
    let (ledger, a, b) = deux_ressources();
    let ledger = Arc::new(ledger);
    let def = b"def-D".to_vec();
    let ra = ressource_bytes(a);
    let rb = ressource_bytes(b);
    let k = oid(6);

    let t_a = {
        let ledger = Arc::clone(&ledger);
        let def = def.clone();
        let ra = ra.clone();
        std::thread::spawn(move || ledger.apply_event(&k, &ra, &def, 0, b"eff-A", 1).unwrap())
    };
    let t_b = {
        let ledger = Arc::clone(&ledger);
        let def = def.clone();
        let rb = rb.clone();
        std::thread::spawn(move || ledger.apply_event(&k, &rb, &def, 0, b"eff-B", 1).unwrap())
    };
    let out_a = t_a.join().unwrap();
    let out_b = t_b.join().unwrap();
    eprintln!("S6 out_a={out_a:?} out_b={out_b:?}");

    assert_eq!(out_a, LedgerOutcome::Applied);
    assert_eq!(
        out_b,
        LedgerOutcome::Applied,
        "deux applications concurrentes du même K sur A et B : toutes deux acceptées"
    );
}
