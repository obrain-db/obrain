//! Contre-épreuve d'identité INTER-RESSOURCES du registre d'observation durable
//! `SubstrateAtomicEventLedger` (crates/obrain-cognitive/src/engram/observation_substrate.rs).
//!
//! QUESTION EXERCÉE (pas une prédiction) : l'`ObservationId` (clé de propriété
//! `obs:<hex>`) est-il GLOBAL ou SCOPÉ PAR RESSOURCE dans l'implémentation durable ?
//!
//! RÉPONSE (correction du défaut 52406858) : l'identité est GLOBALE. Le registre
//! durable écrit désormais `obs:<hex>` sur UN nœud d'autorité canonique
//! (`__obs_autorite__`), pas sur le nœud ressource. Le même K appliqué sur A puis
//! sur B est la MÊME observation : rejeu si la définition est identique,
//! `IdentityConflict` si elle est incompatible — jamais deux applications
//! indépendantes. C'est la parité avec le registre MÉMOIRE
//! (`InMemoryAtomicEventLedger`, `HashMap<[u8;16],_>` global).
//!
//! Les 6 scénarios historiques S1–S6 (qui caractérisaient le défaut) sont
//! préservés, avec leurs assertions retournées vers le comportement NORMATIF ;
//! les témoins normatifs `t_norm_*` les formulent explicitement.
//!
//! Classification des issues :
//!   Applied          -> « Nouvel effet autorisé » (première application de K)
//!   AlreadyApplied   -> « Rejeu » (même identité, même définition, quelle que
//!                       soit la ressource tentée)
//!   IdentityConflict -> « Conflit d'identité » (même identité, définition autre)
//!   NotApplied       -> « Substitution incompatible » (l'identité ne migre pas)
//!   Indeterminate    -> « Résultat indéterminé »

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::fmt::Write;
use std::sync::Arc;

use obrain_cognitive::engram::{
    AtomicEventLedger, LedgerOutcome, ResolvedState, SubstrateAtomicEventLedger,
};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{EtatPropriete, SubstrateStore};

/// Label RÉSERVÉ du nœud d'autorité canonique (miroir de la constante privée).
const LABEL_AUTORITE: &str = "__obs_autorite__";

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

/// Le nœud d'autorité canonique (créé paresseusement au premier `apply_event`).
fn noeud_autorite(ledger: &SubstrateAtomicEventLedger) -> Option<NodeId> {
    ledger
        .store()
        .nodes_by_label_balayage(LABEL_AUTORITE)
        .first()
        .copied()
}

/// L'enregistrement canonique `obs:<hex>` de l'identité, s'il existe sur le
/// nœud d'autorité (diagnostic physique : UNE seule autorité pour toutes les
/// ressources).
fn enregistrement_canonique(ledger: &SubstrateAtomicEventLedger, ev: &[u8; 16]) -> Option<Vec<u8>> {
    let mut cle = String::with_capacity(4 + 32);
    cle.push_str("obs:");
    for b in ev {
        let _ = write!(cle, "{b:02x}");
    }
    match ledger
        .store()
        .lire_etat_propriete(noeud_autorite(ledger)?, &cle)
        .unwrap()
    {
        EtatPropriete::Presente { valeur, .. } => valeur.as_bytes().map(|b| b.to_vec()),
        _ => None,
    }
}

// S1 — même ObservationId K, même définition, sur A puis B.
// NORMATIF : B rend AlreadyApplied (rejeu), pas une seconde application.
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
    // Identité GLOBALE : B rend AlreadyApplied (l'effet enregistré est celui de A).
    assert_eq!(
        out_b,
        LedgerOutcome::AlreadyApplied {
            state: b"eff-A".to_vec(),
            revision: 1
        }
    );
    // UN enregistrement canonique (sur le nœud autorité), pas un par ressource.
    assert!(
        enregistrement_canonique(&ledger, &k).is_some(),
        "enregistrement canonique unique"
    );
    assert_eq!(ledger.resource_revision(&ressource_bytes(a)).unwrap(), 1);
    assert_eq!(
        ledger.resource_revision(&ressource_bytes(b)).unwrap(),
        0,
        "B jamais écrite"
    );
}

// S2 — même ObservationId K, définition D1 sur A puis D2 (incompatible) sur B.
// NORMATIF : B rend IdentityConflict (D1 enregistrée).
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
    // Identité GLOBALE : conflit d'identité INTER-ressources (D1 enregistrée).
    assert_eq!(
        out_b,
        LedgerOutcome::IdentityConflict {
            recorded_definition: b"D1".to_vec()
        }
    );
    assert_eq!(
        ledger.resource_revision(&ressource_bytes(b)).unwrap(),
        0,
        "B jamais écrite"
    );
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
    // Résolution sur B (ressource substituée) : l'identité n'y est pas.
    let resolved_b = ledger.resolve_on(&ressource_bytes(b), &k).unwrap();
    eprintln!("S3 resolved_a={resolved_a:?} resolved_b={resolved_b:?}");

    assert!(matches!(resolved_a, ResolvedState::Applied { .. }));
    assert_eq!(
        resolved_b,
        ResolvedState::NotApplied,
        "l'identité ne migre pas vers B : substitution incompatible"
    );

    // La substitution sur B est un REJEU (l'identité appartient à A), pas un
    // enregistrement neuf indépendant.
    let out_b = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    eprintln!("S3 out_b_apres_substitution={out_b:?}");
    assert_eq!(
        out_b,
        LedgerOutcome::AlreadyApplied {
            state: b"eff-A".to_vec(),
            revision: 1
        }
    );
    assert_eq!(
        ledger.resource_revision(&ressource_bytes(b)).unwrap(),
        0,
        "B jamais écrite"
    );
}

// S4 — perte de réponse : ré-application (rejeu) de K sur A vs sur B.
// NORMATIF : rejeu sur A idempotent, même K sur B = rejeu (pas de nouvel effet).
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
    // Réponse perdue : ré-application du MÊME K sur B -> rejeu (pas un nouvel effet).
    let rejeu_b = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    eprintln!("S4 rejeu_a={rejeu_a:?} rejeu_b={rejeu_b:?}");

    assert!(
        matches!(rejeu_a, LedgerOutcome::AlreadyApplied { revision: 1, .. }),
        "rejeu sur A : idempotent"
    );
    assert!(
        matches!(rejeu_b, LedgerOutcome::AlreadyApplied { revision: 1, .. }),
        "même K sur B : rejeu (portée GLOBALE)"
    );
    assert_eq!(ledger.resource_revision(&ressource_bytes(a)).unwrap(), 1);
    assert_eq!(ledger.resource_revision(&ressource_bytes(b)).unwrap(), 0);
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

    // Réouverture : la portée GLOBALE de K survit-elle au redémarrage ?
    let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let ledger = SubstrateAtomicEventLedger::new(store);
    let resolved_a = ledger.resolve_on(&ressource_bytes(a), &oid(5)).unwrap();
    let resolved_b = ledger.resolve_on(&ressource_bytes(b), &oid(5)).unwrap();
    // Résolution CANONIQUE depuis l'identité SEULE (sans ressource).
    let resolved = ledger.resolve(&oid(5)).unwrap();
    eprintln!("S5 resolved_a={resolved_a:?} resolved_b={resolved_b:?} resolved={resolved:?}");

    assert!(
        matches!(resolved_a, ResolvedState::Applied { revision: 1, .. }),
        "K durable sur A après redémarrage"
    );
    assert_eq!(
        resolved_b,
        ResolvedState::NotApplied,
        "K inconnue sur B après redémarrage -> portée par ressource persistante"
    );
    assert!(
        matches!(resolved, ResolvedState::Applied { revision: 1, .. }),
        "résolution canonique depuis l'identité seule"
    );

    // Rejeu de K sur A après redémarrage : idempotent.
    let rejeu = ledger
        .apply_event(&oid(5), &ressource_bytes(a), b"def-D", 0, b"eff-A", 1)
        .unwrap();
    eprintln!("S5 rejeu_a_apres_redemarrage={rejeu:?}");
    assert!(matches!(
        rejeu,
        LedgerOutcome::AlreadyApplied { revision: 1, .. }
    ));
}

// S6 — deux applications concurrentes de K sur A et B.
// NORMATIF : une seule réservation canonique, l'autre est un rejeu.
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

    // Exactement UNE application, l'autre est un rejeu (pas de double
    // réservation). La réservation canonique sérialise l'identité.
    let n_applied = [&out_a, &out_b]
        .iter()
        .filter(|o| matches!(o, LedgerOutcome::Applied))
        .count();
    let n_rejoues = [&out_a, &out_b]
        .iter()
        .filter(|o| matches!(o, LedgerOutcome::AlreadyApplied { .. }))
        .count();
    assert_eq!(
        n_applied, 1,
        "une seule application (out_a={out_a:?} out_b={out_b:?})"
    );
    assert_eq!(n_rejoues, 1, "l'autre est un rejeu");
    // Une seule ressource porte l'effet.
    let rev_a = ledger.resource_revision(&ressource_bytes(a)).unwrap();
    let rev_b = ledger.resource_revision(&ressource_bytes(b)).unwrap();
    assert_eq!(
        rev_a + rev_b,
        1,
        "un seul effet commis (rev_a={rev_a} rev_b={rev_b})"
    );
}

// ===================================================================
// Témoins NORMATIFS (les six attendus du contrat de portée globale)
// ===================================================================

/// même K + même définition → rejeu (l'effet enregistré est rendu tel quel,
/// aucune seconde application, quelle que soit la ressource tentée).
#[test]
fn t_norm_meme_k_meme_def_rejeu_global() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(10);
    let def = b"def-D".to_vec();
    ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    let rejeu = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    assert_eq!(
        rejeu,
        LedgerOutcome::AlreadyApplied {
            state: b"eff-A".to_vec(),
            revision: 1
        }
    );
    assert_eq!(ledger.resource_revision(&ressource_bytes(b)).unwrap(), 0);
}

/// même K + définition incompatible → IdentityConflict (la définition enregistrée
/// est restituée), aucune application.
#[test]
fn t_norm_meme_k_def_incompatible_conflit() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(11);
    ledger
        .apply_event(&k, &ressource_bytes(a), b"D1", 0, b"eff-A", 1)
        .unwrap();
    let conflit = ledger
        .apply_event(&k, &ressource_bytes(b), b"D2", 0, b"eff-B", 1)
        .unwrap();
    assert_eq!(
        conflit,
        LedgerOutcome::IdentityConflict {
            recorded_definition: b"D1".to_vec()
        }
    );
    assert_eq!(ledger.resource_revision(&ressource_bytes(b)).unwrap(), 0);
}

/// ressource non autorisée → refus : K autorise UNE ressource (celle de sa
/// première application) ; l'appliquer à une AUTRE ressource est refusé (rejeu
/// sans effet, la ressource reste intacte).
#[test]
fn t_norm_ressource_non_autorisee_refus() {
    let (ledger, a, b) = deux_ressources();
    let k = oid(12);
    let def = b"def-D".to_vec();
    ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    // B n'est pas la ressource autorisée par K.
    let refus = ledger
        .apply_event(&k, &ressource_bytes(b), &def, 0, b"eff-B", 1)
        .unwrap();
    assert!(matches!(refus, LedgerOutcome::AlreadyApplied { .. }));
    assert_eq!(
        ledger.resource_revision(&ressource_bytes(b)).unwrap(),
        0,
        "B (non autorisée) jamais écrite"
    );
    // L'identité canonique pointe vers A, pas vers B.
    assert!(matches!(
        ledger.resolve_on(&ressource_bytes(a), &k).unwrap(),
        ResolvedState::Applied { .. }
    ));
    assert_eq!(
        ledger.resolve_on(&ressource_bytes(b), &k).unwrap(),
        ResolvedState::NotApplied
    );
}

/// perte de réponse → pas de réapplication : ré-émettre la MÊME tentative est
/// idempotent (révision inchangée).
#[test]
fn t_norm_perte_reponse_pas_de_reapplication() {
    let (ledger, a, _b) = deux_ressources();
    let k = oid(13);
    let def = b"def-D".to_vec();
    let first = ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    assert_eq!(first, LedgerOutcome::Applied);
    let resend = ledger
        .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
        .unwrap();
    assert!(matches!(
        resend,
        LedgerOutcome::AlreadyApplied { revision: 1, .. }
    ));
    assert_eq!(
        ledger.resource_revision(&ressource_bytes(a)).unwrap(),
        1,
        "aucune réapplication"
    );
}

/// redémarrage → résolution canonique : après réouverture, `resolve(K)` (sans
/// ressource) restitue définition + état + révision depuis l'identité seule.
#[test]
fn t_norm_redemarrage_resolution_canonique() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let k = oid(14);
    let def = b"def-D".to_vec();
    {
        let store = Arc::new(SubstrateStore::create(&chemin).unwrap());
        let a = store.create_node(&["ressource"]);
        let ledger = SubstrateAtomicEventLedger::new(store.clone());
        ledger
            .apply_event(&k, &ressource_bytes(a), &def, 0, b"eff-A", 1)
            .unwrap();
        store.close().unwrap();
    }
    let store = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let ledger = SubstrateAtomicEventLedger::new(store);
    match ledger.resolve(&k).unwrap() {
        ResolvedState::Applied {
            definition,
            state,
            revision,
        } => {
            assert_eq!(definition, def);
            assert_eq!(state, b"eff-A".to_vec());
            assert_eq!(revision, 1);
        }
        other => panic!("attendu Applied, reçu {other:?}"),
    }
}

/// concurrence → pas de double réservation : deux applications concurrentes du
/// même K sur A et B donnent exactement UNE application + UN rejeu.
#[test]
fn t_norm_concurrence_pas_de_double_reservation() {
    let (ledger, a, b) = deux_ressources();
    let ledger = Arc::new(ledger);
    let def = b"def-D".to_vec();
    let ra = ressource_bytes(a);
    let rb = ressource_bytes(b);
    let k = oid(15);
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
    let n_applied = [&out_a, &out_b]
        .iter()
        .filter(|o| matches!(o, LedgerOutcome::Applied))
        .count();
    let n_rejoues = [&out_a, &out_b]
        .iter()
        .filter(|o| matches!(o, LedgerOutcome::AlreadyApplied { .. }))
        .count();
    assert_eq!(
        n_applied, 1,
        "une seule réservation (out_a={out_a:?} out_b={out_b:?})"
    );
    assert_eq!(n_rejoues, 1);
    // Un seul enregistrement canonique physique pour K.
    assert!(
        enregistrement_canonique(&ledger, &k).is_some(),
        "enregistrement canonique unique"
    );
}
