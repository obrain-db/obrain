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
