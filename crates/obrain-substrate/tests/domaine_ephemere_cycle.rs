//! IDENTITÉ ÉPHÉMÈRE (#334/#418) — délimitation stricte du régime de
//! réouverture : une base dont les DEUX fichiers d'identité sont absents reçoit
//! une identité ÉPHÉMÈRE (jamais refusée, JAMAIS écrite), jusqu'à un
//! `migrer_legacy_domaine` explicite. Contrats :
//!   (1) une identité éphémère n'est JAMAIS présentée comme autorité durable ;
//!   (2) une base PRÉCÉDEMMENT identifiée ayant perdu ses DEUX fichiers ne
//!       reçoit PAS silencieusement une nouvelle origine ;
//!   (3) les constructeurs `Creation` (`create`, `create_with_mode`) ne peuvent
//!       PAS contourner la protection sur une base existante non vide ;
//!   (4) la migration legacy reste EXPLICITE (pas d'auto-réattribution).

use std::path::Path;

use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::{
    FICHIER_DOMAINE_ID, FICHIER_DOMAINE_INIT, IntegriteTentative, ModePublication,
    ReferenceTentative, SubstrateError, SubstrateStore, wal_io::SyncMode,
};

/// Crée une base identifiée portant un nœud avec propriété, ferme, rend le domaine.
fn creer_base_avec_donnee(base: &Path) -> (u128, obrain_common::types::NodeId) {
    let store = SubstrateStore::create(base).unwrap();
    let d0 = store.domaine_id();
    let n = store.create_node(&["ressource"]);
    store.set_node_property(n, "a", Value::Int64(7));
    store.close().unwrap();
    (d0, n)
}

/// Supprime les DEUX fichiers d'identité (simule une perte intégrale).
fn retirer_les_deux_fichiers(base: &Path) {
    std::fs::remove_file(base.join(FICHIER_DOMAINE_ID)).unwrap();
    std::fs::remove_file(base.join(FICHIER_DOMAINE_INIT)).unwrap();
}

fn aucun_fichier_d_identite(base: &Path) -> bool {
    !base.join(FICHIER_DOMAINE_ID).exists() && !base.join(FICHIER_DOMAINE_INIT).exists()
}

/// (1) Cycle COMPLET : ouverture sans fichiers d'identité → lecture → écriture
/// → préparation transactionnelle → publication → export OBRT → réouverture.
/// L'identité éphémère n'est JAMAIS une autorité durable : deux réouvertures
/// servent deux éphémères DIFFÉRENTS, aucun fichier n'est écrit, et la référence
/// émise sous le premier éphémère est ÉTRANGÈRE au second.
#[test]
fn cycle_complet_ephemere_n_est_jamais_une_autorite_durable() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let (d0, n) = creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);
    assert!(aucun_fichier_d_identite(&base));

    // Réouverture : identité ÉPHÉMÈRE (jamais refusée), rien d'écrit.
    let (e1, obrt) = {
        let store = SubstrateStore::open(&base).unwrap();
        let e1 = store.domaine_id();
        assert_ne!(e1, 0, "éphémère jamais nul");
        assert_ne!(e1, d0, "l'éphémère ne ressuscite pas l'identité perdue");
        // Lecture : la donnée de la base legacy est servie.
        assert_eq!(store.node_count(), 1);
        assert_eq!(store.get_node_property(n, &PropertyKey::new("a")), Some(Value::Int64(7)));
        // Écriture : une écriture ordinaire sous l'éphémère est possible.
        store.set_node_property(n, "a", Value::Int64(8));
        // Préparation transactionnelle → la référence porte l'éphémère e1.
        let pre = store.capturer_precondition(n, &["a"]).unwrap();
        let prep = store
            .preparer_transformation(n, &[("a", Value::Int64(9))], &pre, ModePublication::Strict)
            .unwrap();
        let reference = prep.reference_tentative();
        assert_eq!(reference.domaine(), e1, "la référence porte l'éphémère courant");
        // Publication réelle.
        let issue = store.publier_si_precondition_intacte(prep).unwrap();
        assert!(matches!(issue, obrain_substrate::IssuePublication::PublieIntegrale));
        // Export OBRT : la référence sérialisée.
        let obrt = reference.to_bytes();
        assert_eq!(&obrt[..4], b"OBRT", "le codec v2 porte le magic OBRT");
        store.close().unwrap();
        (e1, obrt)
    };

    // Jamais durable : aucun fichier d'identité n'a été matérialisé.
    assert!(aucun_fichier_d_identite(&base), "l'éphémère n'est jamais persisté");

    // Réouverture : un AUTRE éphémère, et la référence du premier est étrangère.
    let reference = ReferenceTentative::from_bytes(&obrt).unwrap();
    {
        let store = SubstrateStore::open(&base).unwrap();
        let e2 = store.domaine_id();
        assert_ne!(e2, e1, "deux réouvertures éphémères servent deux identités distinctes");
        // La donnée a survécu (écrite sous l'éphémère précédent, dans le WAL).
        assert_eq!(store.get_node_property(n, &PropertyKey::new("a")), Some(Value::Int64(9)));
        // La référence émise sous e1 est ÉTRANGÈRE au domaine courant e2 :
        // résolution intégralement indéterminée, jamais une confirmation.
        let res = store.resoudre_reference(&reference).unwrap();
        assert_eq!(
            res.integrite(),
            IntegriteTentative::Indeterminee,
            "une référence d'un éphémère passé ne peut pas être confirmée par un autre"
        );
        store.close().unwrap();
    }
    assert!(aucun_fichier_d_identite(&base));
}

/// (2) Une base PRÉCÉDEMMENT identifiée qui perd ses DEUX fichiers ne reçoit
/// PAS silencieusement une nouvelle origine : réouverture = éphémère (rien
/// d'écrit), la donnée survit.
#[test]
fn perte_des_deux_fichiers_est_ephemere_pas_une_reidentification() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let (_d0, n) = creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);

    let store = SubstrateStore::open(&base).unwrap();
    assert!(aucun_fichier_d_identite(&base), "réouverture sans matérialisation");
    assert_eq!(store.domaine_id() != 0, true, "une identité éphémère est servie");
    assert_eq!(store.node_count(), 1, "la donnée de la base perdue survit");
    assert_eq!(store.get_node_property(n, &PropertyKey::new("a")), Some(Value::Int64(7)));
    store.close().unwrap();
    assert!(aucun_fichier_d_identite(&base), "toujours rien d'écrit après fermeture");
}

/// (3) Les constructeurs `Creation` (`create`, `create_with_mode`) REFUSENT une
/// base existante non vide (le `substrate.meta` est déjà là) : ils ne peuvent
/// donc pas contourner la protection en ré-identifiant silencieusement.
#[test]
fn constructeurs_creation_refusent_une_base_existante_non_vide() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);

    // `create` doit échouer (AlreadyExists), sans rien matérialiser.
    let err = match SubstrateStore::create(&base) {
        Ok(_) => panic!("create sur une base existante non vide aurait dû échouer"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "create doit refuser (AlreadyExists), obtenu {err:?}"
    );
    assert!(aucun_fichier_d_identite(&base), "aucune ré-identification silencieuse");

    // `create_with_mode` (même famille Creation) : même refus.
    let err = match SubstrateStore::create_with_mode(&base, SyncMode::EveryCommit) {
        Ok(_) => panic!("create_with_mode sur une base existante aurait dû échouer"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "create_with_mode doit refuser (AlreadyExists), obtenu {err:?}"
    );
    assert!(aucun_fichier_d_identite(&base));
}

/// (4) La migration legacy reste EXPLICITE : la réouverture n'auto-matérialise
/// jamais ; seul `migrer_legacy_domaine()` établit une identité persistée stable.
#[test]
fn migration_legacy_reste_explicite() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    retirer_les_deux_fichiers(&base);

    // Réouverture ×2 : jamais d'auto-matérialisation.
    for _ in 0..2 {
        let store = SubstrateStore::open(&base).unwrap();
        assert!(aucun_fichier_d_identite(&base), "la réouverture ne matérialise jamais");
        store.close().unwrap();
    }

    // L'acte explicite matérialise une identité persistée stable.
    let stable = {
        let store = SubstrateStore::open(&base).unwrap();
        let stable = store.migrer_legacy_domaine().unwrap();
        assert_ne!(stable, 0);
        store.close().unwrap();
        stable
    };
    assert!(base.join(FICHIER_DOMAINE_ID).exists(), "l'identité est persistée");
    assert!(base.join(FICHIER_DOMAINE_INIT).exists(), "le marqueur est posé");

    // Les réouvertures suivantes relisent l'identité STABLE.
    let store = SubstrateStore::open(&base).unwrap();
    assert_eq!(store.domaine_id(), stable, "identité stable relue");
    assert_eq!(store.node_count(), 1, "la donnée a survécu à la migration");
    store.close().unwrap();
}
