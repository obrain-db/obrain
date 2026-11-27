//! CONTRAT SNAPSHOT / FORK — CONTINUITÉ D'ORIGINE (#334/#418, frontière 2).
//!
//! Une restauration DÉCLARÉE CONTINUE exige une identité cohérente : si l'image
//! ne porte PAS la paire `domaine.id` + `domaine.init` (instantané pris PENDANT
//! un fork — paire exclue par `file::zone_names`), elle REFUSE
//! (`SubstrateError::ContinuiteIndisponible`) au lieu de produire silencieusement
//! une base ÉPHÉMÈRE. La restauration NON-continue (`restaurer`) reste
//! permissive — le distingue est LISIBLE par le nom et par le verdict.
//!
//! L'export (`exporter_avec_dict`) tient le verrou PARTAGÉ du domaine de
//! l'énumération des zones à la fin de la copie : un fork (exclusif) ne peut pas
//! déchirer la paire au milieu — la preuve de cohérence n'est pas la seule
//! exclusion de fichiers, c'est l'exclusion mutuelle du verrou.

use std::path::Path;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::instantane::{
    exporter_avec_dict, porte_paire_identite, restaurer, restaurer_continue,
};
use obrain_substrate::{
    FICHIER_DOMAINE_FORK, FICHIER_DOMAINE_ID, FICHIER_DOMAINE_INIT, IntegriteTentative,
    IssuePublication, ModePublication, SubstrateError, SubstrateFile, SubstrateStore,
};

/// Identité simulée du fork « déchiré » (même approche que
/// `domaine_snapshot_fork.rs`).
const D1_SIMULE: u128 = 0x3344_5566_7788_99AA_BBCC_DDEE_FF01;

fn ecrire_fichier_domaine(p: &Path, magic: &[u8], id: u128) {
    let mut buf = Vec::with_capacity(22);
    buf.extend_from_slice(magic);
    buf.extend_from_slice(&id.to_be_bytes());
    std::fs::write(p, buf).unwrap();
}

/// Simule un fork interrompu ENTRE `domaine.id` et `domaine.init` :
/// journal = D1, `domaine.id` = D1, `domaine.init` = D0 (empreinte discordante).
fn simuler_fork_dechire(base: &Path) {
    ecrire_fichier_domaine(&base.join(FICHIER_DOMAINE_FORK), b"OBRF1\n", D1_SIMULE);
    ecrire_fichier_domaine(&base.join(FICHIER_DOMAINE_ID), b"OBRD1\n", D1_SIMULE);
    // domaine.init : laissé tel quel (empreinte de D0) — discordant.
}

fn creer_base_avec_donnee(base: &Path) -> (u128, NodeId) {
    let store = SubstrateStore::create(base).unwrap();
    let d0 = store.domaine_id();
    let n = store.create_node(&["ressource"]);
    store.set_node_property(n, "a", Value::Int64(1));
    store.close().unwrap();
    (d0, n)
}

/// Image brute des zones + dictionnaire (le flux que `restaurer_continue` prend).
fn snapshot_zones(store: &SubstrateStore) -> Vec<u8> {
    let dict = store.octets_dictionnaire().unwrap();
    store
        .avec_substrate(|sub| exporter_avec_dict(sub, Some(&dict)))
        .unwrap()
}

/// L'image porte la paire d'identité, mais pas le journal de fork (jamais
/// transporté) : continuité DISPONIBLE.
#[test]
fn porte_paire_identite_avant_fork() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    let image = {
        let store = SubstrateStore::open(&base).unwrap();
        let img = snapshot_zones(&store);
        store.close().unwrap();
        img
    };
    assert!(
        porte_paire_identite(&image).unwrap(),
        "la paire est transportée hors fork"
    );
}

/// PENDANT un fork (journal présent) : la paire est exclue — continuité
/// INDISPONIBLE, détectable SANS restaurer.
#[test]
fn porte_paire_identite_pendant_fork_est_fausse() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    simuler_fork_dechire(&base);
    let image = {
        let sub = SubstrateFile::open_read_only(&base).unwrap();
        exporter_avec_dict(&sub, None).unwrap()
    };
    assert!(
        !porte_paire_identite(&image).unwrap(),
        "la paire est exclue pendant le fork"
    );
}

/// SCÉNARIO COMPLET — avant fork : continuité OUI, identité restaurée, référence
/// historique résolue (pas étrangère au domaine courant).
#[test]
fn restauration_continue_avant_fork_preserve_identite_et_resout_la_reference() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let (d0, n) = creer_base_avec_donnee(&base);

    // Publier sous D0 et émettre une référence AVANT publication.
    let (reference, image) = {
        let store = SubstrateStore::open(&base).unwrap();
        let pre = store.capturer_precondition(n, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(n, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        assert_eq!(
            reference.domaine(),
            d0,
            "la référence porte le domaine d'origine"
        );
        let issue = store.publier_si_precondition_intacte(prep).unwrap();
        assert!(matches!(issue, IssuePublication::PublieIntegrale));
        let img = snapshot_zones(&store);
        store.close().unwrap();
        (reference, img)
    };
    assert!(porte_paire_identite(&image).unwrap());

    // Restauration CONTINUE : identité restaurée (continuité).
    let dir2 = td.path().join("restored");
    {
        let mut sub = SubstrateFile::create(&dir2).unwrap();
        restaurer_continue(&mut sub, &image).unwrap();
    }
    let store2 = SubstrateStore::open(&dir2).unwrap();
    assert_eq!(
        store2.domaine_id(),
        d0,
        "continuité : même identité restaurée"
    );
    assert!(
        !store2.domaine_est_ephemere(),
        "l'identité restaurée est durable"
    );
    assert_eq!(
        store2.get_node_property(n, &PropertyKey::new("a")),
        Some(Value::Int64(2)),
        "la donnée publiée survit à la restauration"
    );
    // La référence émise sous D0 se résout contre le store restauré (même domaine) :
    // l'intégrité est INTÉGRALE, jamais l'indétermination « domaine étranger ».
    let res = store2.resoudre_reference(&reference).unwrap();
    assert_eq!(
        res.integrite(),
        IntegriteTentative::Integrale,
        "référence historique résolue, pas étrangère"
    );
    store2.close().unwrap();
}

/// SCÉNARIO COMPLET — pendant fork : continuité INDISPONIBLE → refus explicite de
/// la restauration continue. La restauration NON-continue reste permissive
/// (base éphémère), et le distingue est LISIBLE.
#[test]
fn restauration_continue_pendant_fork_est_refusee() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    simuler_fork_dechire(&base);

    let image = {
        let sub = SubstrateFile::open_read_only(&base).unwrap();
        exporter_avec_dict(&sub, None).unwrap()
    };
    assert!(!porte_paire_identite(&image).unwrap());

    // Restauration CONTINUE : refus AVANT toute écriture de zone.
    let dir2 = td.path().join("restored_continue");
    let err = {
        let mut sub = SubstrateFile::create(&dir2).unwrap();
        restaurer_continue(&mut sub, &image).unwrap_err()
    };
    assert!(
        matches!(err, SubstrateError::ContinuiteIndisponible),
        "refus explicite « continuité indisponible », obtenu {err:?}"
    );

    // Restauration NON-continue : permissive, et la base restaurée est
    // ÉPHÉMÈRE — lisiblement (pas une paire discordante, pas une réattribution).
    let dir3 = td.path().join("restored_brut");
    {
        let mut sub = SubstrateFile::create(&dir3).unwrap();
        restaurer(&mut sub, &image).unwrap();
    }
    let store3 = SubstrateStore::open(&dir3).unwrap();
    assert!(
        store3.domaine_est_ephemere(),
        "la restauration brute ouvre une base ÉPHÉMÈRE (continuité perdue, LISIBLE)"
    );
    assert!(
        !dir3.join(FICHIER_DOMAINE_ID).exists() && !dir3.join(FICHIER_DOMAINE_INIT).exists(),
        "rien n'est matérialisé silencieusement"
    );
    store3.close().unwrap();
}
