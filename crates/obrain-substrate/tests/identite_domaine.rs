//! ── IDENTITÉ DE DOMAINE — premier incrément natif (#334) ─────────────────
//!
//! Témoin d'intégration du `domaine_id` persistant (`domaine.id`), exercé
//! UNIQUEMENT via l'API publique (`SubstrateStore::create/open`, `domaine_id()`,
//! `forker_domaine()`), comme les autres contrats d'identité.
//!
//! Ce que l'on vérifie :
//!   * création          — deux stores → deux domaines distincts ;
//!   * réouverture       — même store fermé puis rouvert → même domaine ;
//!   * déplacement       — copier le RÉPERTOIRE entier ailleurs puis rouvrir →
//!     même domaine (le domaine suit les FICHIERS, pas le chemin) ;
//!   * fork déclaré      — `forker_domaine()` → nouveau domaine, distinct ;
//!   * corruption        — `domaine.id` tronqué / version inconnue → erreur
//!     explicite à l'ouverture (jamais de réinitialisation silencieuse) ;
//!   * copie NON déclarée — deux copies intégrales partagent le MÊME
//!     `domaine_id` : c'est la LIMITE ASSUMÉE, la distinction exige le fork.

use obrain_common::types::Value;
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::{SubstrateError, SubstrateStore};

/// Copie byte-à-byte du CONTENU d'un répertoire de base (tous les fichiers de
/// premier niveau, y compris `domaine.id`, `writer.id`, `substrate.meta`, zones
/// et WAL) — c'est exactement ce que fait un `cp -a`.
fn copier_repertoire(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entree in std::fs::read_dir(src).unwrap() {
        let e = entree.unwrap();
        let chemin = e.path();
        if chemin.is_file() {
            std::fs::copy(&chemin, dst.join(e.file_name())).unwrap();
        }
    }
}

/// Crée une base, rend son `domaine_id`, ferme.
fn creer_et_fermer(chemin: &std::path::Path) -> u128 {
    let store = SubstrateStore::create(chemin).unwrap();
    let id = store.domaine_id();
    store.close().unwrap();
    id
}

/// Deux bases créées indépendamment reçoivent deux domaines distincts.
#[test]
fn creation_deux_bases_deux_domaines_distincts() {
    let td = tempfile::tempdir().unwrap();
    let a = creer_et_fermer(&td.path().join("a"));
    let b = creer_et_fermer(&td.path().join("b"));
    assert_ne!(a, b, "deux bases neuves doivent avoir des domaines distincts");
    assert_ne!(a, 0, "un domaine neuf n'est jamais nul");
}

/// Même base fermée puis rouverte : même domaine (l'identité est persistée).
#[test]
fn reouverture_meme_base_meme_domaine() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let premier = creer_et_fermer(&chemin);
    let store = SubstrateStore::open(&chemin).unwrap();
    assert_eq!(store.domaine_id(), premier, "la réouverture doit relire le même domaine");
    store.close().unwrap();
}

/// Déplacer = copier le répertoire ENTIER ailleurs : le domaine suit les
/// fichiers, pas le chemin.
#[test]
fn deplacement_repertoire_conserve_le_domaine() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path().join("a");
    let dst = td.path().join("b");
    let id = creer_et_fermer(&src);
    copier_repertoire(&src, &dst);
    let store = SubstrateStore::open(&dst).unwrap();
    assert_eq!(store.domaine_id(), id, "le domaine suit les fichiers, pas le chemin");
    store.close().unwrap();
}

/// Fork DÉCLARÉ : `forker_domaine()` régénère l'identité, distincte de l'actuelle,
/// et la persiste (une réouverture relit le NOUVEAU domaine).
#[test]
fn fork_declare_regenerer_un_domaine_distinct() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let avant = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let avant = store.domaine_id();
        let nouveau = store.forker_domaine().unwrap();
        assert_ne!(nouveau, avant, "le fork déclaré doit régénérer le domaine");
        store.close().unwrap();
        avant
    };
    let store = SubstrateStore::open(&chemin).unwrap();
    assert_ne!(
        store.domaine_id(),
        avant,
        "après fork, la réouverture doit relire le NOUVEAU domaine"
    );
    store.close().unwrap();
}

/// `domaine.id` tronqué : l'ouverture refuse (InvalidData), sans réinitialiser.
#[test]
fn domaine_id_tronque_refuse_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    creer_et_fermer(&chemin);
    std::fs::write(
        chemin.join(obrain_substrate::FICHIER_DOMAINE_ID),
        b"OBRD1\n\x00\x01", // 8 octets au lieu de 22
    )
    .unwrap();
    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("l'ouverture d'un domaine.id corrompu aurait dû échouer"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "fichier tronqué : refus explicite attendu (InvalidData), obtenu {err:?}"
    );
    // Aucune réinitialisation silencieuse : le fichier reste corrompu.
    let brut = std::fs::read(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID)).unwrap();
    assert_eq!(brut.len(), 8, "le fichier corrompu ne doit pas être réécrit");
}

/// `domaine.id` avec une version inconnue : l'ouverture refuse, sans régénérer.
#[test]
fn domaine_id_version_inconnue_refuse_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    creer_et_fermer(&chemin);
    let mut buf = b"OBRD9\n".to_vec();
    buf.extend_from_slice(&[0u8; 16]);
    std::fs::write(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID), buf).unwrap();
    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("l'ouverture d'un domaine.id corrompu aurait dû échouer"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "version inconnue : refus explicite attendu (InvalidData), obtenu {err:?}"
    );
}

/// LIMITE ASSUMÉE (documentée) : une copie intégrale NON déclarée (simple
/// `cp -a`) partage le MÊME `domaine_id`. Structurellement, « déplacer » et
/// « copier » produisent des répertoires byte-identiques ; seule l'intention de
/// l'opérateur les distingue. La distinction exige le fork DÉCLARÉ
/// (`forker_domaine`) — c'est précisément ce que le test `fork_declare_*`
/// ci-dessus éprouve.
#[test]
fn copie_integrale_non_declaree_partage_le_domaine() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path().join("a");
    let dst = td.path().join("b");
    let id = creer_et_fermer(&src);
    copier_repertoire(&src, &dst);
    let store = SubstrateStore::open(&dst).unwrap();
    // C'est la limite assumée : même identifiant, car rien ne distingue la
    // copie d'un déplacement. Le consommateur DOIT forker explicitement.
    assert_eq!(store.domaine_id(), id);
    store.close().unwrap();
}

/// TÉMOIN DU DISCRIMINANT (#334) : une base identifiée qui a publié une
/// tentative (nœud + propriété) puis PERDU son `domaine.id` doit REFUSER la
/// réouverture — jamais recevoir une identité neuve, qui ferait de la
/// tentative passée une étrangère à sa propre base.
#[test]
fn base_avec_donnees_sans_domaine_id_refuse_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");

    // 1. Base identifiée + tentative publiée (nœud + propriété), puis fermée.
    let id_origine = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let id_origine = store.domaine_id();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "etat", Value::Int64(7));
        store.close().unwrap();
        id_origine
    };
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
    let _ = id_origine;

    // 2. Le fichier d'identité est perdu (suppression).
    std::fs::remove_file(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID)).unwrap();

    // 3. Réouverture → ERREUR explicite (pas une identité neuve).
    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("une base avec données sans domaine.id aurait dû refuser l'ouverture"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "refus explicite attendu (InvalidData), obtenu {err:?}"
    );

    // 4. Aucune réattribution silencieuse : `domaine.id` reste ABSENT. La
    //    tentative passée n'est jamais rattachée à une identité neuve inventée
    //    à la volée — elle reste attachée à l'identité perdue, qu'un humain
    //    devra restaurer.
    assert!(
        !chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists(),
        "l'ouverture refusée ne doit pas matérialiser une nouvelle identité"
    );
}

/// Même témoin, en LECTURE SEULE : un lecteur d'une base avec données sans
/// `domaine.id` refuse aussi — il ne régénère pas une identité éphémère qui
/// divergerait de celle des écrivains.
#[test]
fn base_avec_donnees_sans_domaine_id_refuse_la_lecture_seule() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    {
        let store = SubstrateStore::create(&chemin).unwrap();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "etat", Value::Int64(7));
        store.close().unwrap();
    }
    std::fs::remove_file(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID)).unwrap();

    let err = match SubstrateStore::open_read_only(&chemin) {
        Ok(_) => panic!("un lecteur d'une base avec données sans domaine.id aurait dû refuser"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "refus explicite attendu (InvalidData), obtenu {err:?}"
    );
    assert!(!chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
}

/// Création initiale : `domaine.id` ET `domaine.init` sont posés, et l'empreinte
/// du marqueur est l'identité COMPLÈTE du domaine (16 octets big-endian).
#[test]
fn creation_initiale_pose_le_marqueur() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let id = creer_et_fermer(&chemin);
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
    let brut = std::fs::read(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT)).unwrap();
    assert_eq!(&brut[..6], b"OBRI1\n", "le marqueur porte le magic OBRI + version 1");
    assert_eq!(
        &brut[6..],
        &id.to_be_bytes()[..16],
        "l'empreinte est l'identité COMPLÈTE sur 128 bits du domaine_id"
    );
}

/// LE CAS CRITIQUE (#334) : une base identifiée VIDE (aucune donnée écrite) qui
/// perd `domaine.id` — mais garde son marqueur `domaine.init` — REFUSE la
/// réouverture. L'ancienne heuristique d'occupation l'aurait ré-identifiée en
/// silence (base vide) ; le marqueur durable l'interdit.
#[test]
fn base_identifiee_vide_qui_perd_domaine_id_refuse() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let id_origine = creer_et_fermer(&chemin);
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT).exists());

    // Perte : SEUL domaine.id est supprimé (le marqueur reste).
    std::fs::remove_file(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID)).unwrap();

    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("une base identifiée vide sans domaine.id aurait dû refuser l'ouverture"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "refus explicite attendu (InvalidData), obtenu {err:?}"
    );
    // Jamais de réattribution silencieuse : domaine.id reste ABSENT, et la
    // tentative passée n'est jamais rattachée à une identité inventée.
    assert!(!chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT).exists());
    let _ = id_origine;
}

/// MIGRATION legacy-domaine : une base identifiée AVANT le marqueur (`domaine.id`
/// présent, `domaine.init` absent) qui porte des données est MIGRÉE à
/// l'ouverture — le marqueur est écrit (empreinte du domaine EXISTANT), jamais
/// une identité neuve, et les données sont conservées.
#[test]
fn migration_base_legacy_domaine_non_vide_conserve_donnees() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (id, _n) = {
        let store = SubstrateStore::create(&chemin).unwrap();
        let id = store.domaine_id();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "etat", Value::Int64(7));
        store.close().unwrap();
        (id, n)
    };
    // Simule « identifiée avant le marqueur » : SEUL domaine.init est supprimé.
    std::fs::remove_file(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT)).unwrap();

    let store = SubstrateStore::open(&chemin).unwrap();
    assert_eq!(
        store.domaine_id(),
        id,
        "la migration conserve l'identité existante (jamais de nouvelle identité)"
    );
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT).exists(), "le marqueur est écrit");
    assert_eq!(store.node_count(), 1, "les données survivent à la migration");
    store.close().unwrap();
}

/// MIGRATION legacy jamais identifiée : une base avec données SANS AUCUN fichier
/// RÉOUVERTURE d'une base legacy jamais identifiée (les DEUX fichiers absents) :
/// identité ÉPHÉMÈRE (jamais refusée, jamais écrite), données conservées — deux
/// fichiers absents ne prouvent pas qu'elle n'a jamais été identifiée. Puis
/// MIGRATION legacy EXPLICITE (`migrer_legacy_domaine`) : matérialise l'identité
/// persistée, relue stable aux réouvertures suivantes.
#[test]
fn reouverture_ephemere_puis_migration_explicite_conserve_donnees() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    {
        let store = SubstrateStore::create(&chemin).unwrap();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "etat", Value::Int64(7));
        store.close().unwrap();
    }
    // Les DEUX fichiers d'identité sont absents (legacy jamais identifié).
    std::fs::remove_file(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID)).unwrap();
    std::fs::remove_file(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT)).unwrap();

    // Réouverture : identité ÉPHÉMÈRE, rien n'est écrit.
    {
        let store = SubstrateStore::open(&chemin).unwrap();
        assert_ne!(store.domaine_id(), 0, "une identité éphémère est servie, jamais refusée");
        assert!(
            !chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists(),
            "la réouverture ne doit PAS réattribuer d'identité persistée à une base legacy"
        );
        assert_eq!(store.node_count(), 1, "les données d'une base legacy survivent");
        store.close().unwrap();
    }

    // Migration legacy EXPLICITE : matérialise l'identité persistée.
    {
        let store = SubstrateStore::open(&chemin).unwrap();
        let stable = store.migrer_legacy_domaine().unwrap();
        assert_ne!(stable, 0);
        assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
        assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT).exists());
        assert_eq!(store.node_count(), 1);
        store.close().unwrap();
    }

    // Les réouvertures suivantes relisent l'identité stable.
    let store = SubstrateStore::open(&chemin).unwrap();
    assert!(chemin.join(obrain_substrate::FICHIER_DOMAINE_ID).exists());
    assert_eq!(store.node_count(), 1);
    store.close().unwrap();
}

/// `domaine.init` dont l'empreinte ne correspond pas à `domaine.id` : refus
/// explicite (corruption ou fork interrompu), jamais de réattribution.
#[test]
fn empreinte_discordante_refuse_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    creer_et_fermer(&chemin);
    // Retourne un octet de l'empreinte (octets 6..22) : magic intact, empreinte
    // discordante.
    let mut brut = std::fs::read(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT)).unwrap();
    brut[6] ^= 0xff;
    std::fs::write(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT), brut).unwrap();

    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("une empreinte discordante aurait dû refuser l'ouverture"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "empreinte discordante : refus attendu (InvalidData), obtenu {err:?}"
    );
}

/// `domaine.init` tronqué : refus explicite, sans réinitialisation.
#[test]
fn domaine_init_tronque_refuse_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    creer_et_fermer(&chemin);
    std::fs::write(
        chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT),
        b"OBRI1\n\x00\x01", // 8 octets au lieu de 22
    )
    .unwrap();
    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("un domaine.init tronqué aurait dû refuser l'ouverture"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "domaine.init tronqué : refus attendu (InvalidData), obtenu {err:?}"
    );
}

/// `domaine.init` avec une version inconnue : refus explicite.
#[test]
fn domaine_init_version_inconnue_refuse_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    creer_et_fermer(&chemin);
    let mut buf = b"OBRI9\n".to_vec();
    buf.extend_from_slice(&[0u8; 16]);
    std::fs::write(chemin.join(obrain_substrate::FICHIER_DOMAINE_INIT), buf).unwrap();
    let err = match SubstrateStore::open(&chemin) {
        Ok(_) => panic!("un domaine.init de version inconnue aurait dû refuser l'ouverture"),
        Err(e) => e,
    };
    assert!(
        matches!(&err, SubstrateError::Io(e) if e.kind() == std::io::ErrorKind::InvalidData),
        "domaine.init version inconnue : refus attendu (InvalidData), obtenu {err:?}"
    );
}
