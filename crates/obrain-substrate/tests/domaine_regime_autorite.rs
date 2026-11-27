//! RÉGIME D'AUTORITÉ DU DOMAINE (#334/#418, frontière 3) — distinguer une
//! identité ÉPHÉMÈRE (servie en réouverture sans fichiers, invalidée à la
//! prochaine réouverture) d'une identité DURABLE (persistée), SANS dupliquer
//! l'autorité transactionnelle.
//!
//! `SubstrateStore::domaine_est_ephemere()` est un accesseur READ-ONLY : il rend
//! `true` si l'identité servie par `domaine_id()` ne survivra pas à une
//! réouverture, `false` si elle est persistée. Pure observabilité : aucune
//! écriture, aucune nouvelle autorité, la sémantique de `domaine_id()` est
//! inchangée. Complète `domaine_ephemere_cycle.rs` (qu'il ne modifie pas).

use std::path::Path;

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::{
    FICHIER_DOMAINE_ID, FICHIER_DOMAINE_INIT, ModePublication, SubstrateStore,
};

fn creer_base_avec_donnee(base: &Path) -> (u128, NodeId) {
    let store = SubstrateStore::create(base).unwrap();
    let d0 = store.domaine_id();
    let n = store.create_node(&["ressource"]);
    store.set_node_property(n, "a", Value::Int64(7));
    store.close().unwrap();
    (d0, n)
}

fn retirer_les_deux_fichiers(base: &Path) {
    std::fs::remove_file(base.join(FICHIER_DOMAINE_ID)).unwrap();
    std::fs::remove_file(base.join(FICHIER_DOMAINE_INIT)).unwrap();
}

fn aucun_fichier_d_identite(base: &Path) -> bool {
    !base.join(FICHIER_DOMAINE_ID).exists() && !base.join(FICHIER_DOMAINE_INIT).exists()
}

/// `create` matérialise une identité persistée : régime DURABLE.
#[test]
fn apres_create_l_identite_est_durable() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let store = SubstrateStore::create(&base).unwrap();
    assert!(
        !store.domaine_est_ephemere(),
        "create matérialise une identité durable (domaine.id + domaine.init)"
    );
    assert!(base.join(FICHIER_DOMAINE_ID).exists());
    assert!(base.join(FICHIER_DOMAINE_INIT).exists());
    store.close().unwrap();
}

/// Après perte des DEUX fichiers d'identité, la réouverture sert une identité
/// ÉPHÉMÈRE : `domaine_est_ephemere() == true`, rien n'est persisté.
#[test]
fn apres_perte_des_deux_fichiers_l_identite_est_ephemere() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let (d0, _n) = creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);

    let store = SubstrateStore::open(&base).unwrap();
    assert!(
        store.domaine_est_ephemere(),
        "sans fichiers d'identité, le régime courant est éphémère"
    );
    assert_ne!(store.domaine_id(), d0, "l'éphémère ne ressuscite pas l'identité perdue");
    store.close().unwrap();
    assert!(aucun_fichier_d_identite(&base), "toujours rien d'écrit après fermeture");
}

/// `migrer_legacy_domaine()` matérialise une identité persistée : le régime
/// bascule d'ÉPHÉMÈRE à DURABLE, observé par le MÊME store puis à la réouverture.
#[test]
fn apres_migrer_legacy_l_identite_est_durable() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);

    let store = SubstrateStore::open(&base).unwrap();
    assert!(store.domaine_est_ephemere(), "avant migration : éphémère");
    let stable = store.migrer_legacy_domaine().unwrap();
    assert_ne!(stable, 0);
    assert!(
        !store.domaine_est_ephemere(),
        "la migration légacy matérialise une identité durable"
    );
    store.close().unwrap();

    // Réouverture : le régime durable est relu.
    let store2 = SubstrateStore::open(&base).unwrap();
    assert_eq!(store2.domaine_id(), stable);
    assert!(!store2.domaine_est_ephemere(), "identité durable relue");
    store2.close().unwrap();
}

/// Le consommateur peut interroger le régime AVANT de publier — et publier sous
/// un régime éphémère ne matérialise RIEN (observabilité pure, pas une autorité).
#[test]
fn le_consommateur_interroge_le_regime_avant_de_publier() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let (_d0, n) = creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);

    let store = SubstrateStore::open(&base).unwrap();
    assert!(store.domaine_est_ephemere(), "le régime est lisible avant publication");

    // Préparer/publier une transformation : le régime reste éphémère (rien n'est
    // matérialisé par la publication), le consommateur le constate.
    let pre = store.capturer_precondition(n, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(n, &[("a", Value::Int64(8))], &pre, ModePublication::Strict)
        .unwrap();
    assert!(
        store.domaine_est_ephemere(),
        "interroger le régime avant publication ne le change pas"
    );
    store.publier_si_precondition_intacte(prep).unwrap();
    assert!(
        store.domaine_est_ephemere(),
        "publier sous un régime éphémère ne matérialise aucune identité"
    );
    store.close().unwrap();
    assert!(aucun_fichier_d_identite(&base), "aucune identité n'a été persistée");
}

/// Store MÉMOIRE : toujours éphémère — aucun répertoire, identité tirée en
/// mémoire à chaque ouverture (in-process, aucune invalidation par un pair).
#[test]
fn store_memoire_est_toujours_ephemere() {
    let store = SubstrateStore::open_memory().unwrap();
    assert!(
        store.domaine_est_ephemere(),
        "un store mémoire n'a aucune identité persistée : régime éphémère"
    );
}
