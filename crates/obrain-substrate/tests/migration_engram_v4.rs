//! Migration V3→V4 des poids d'engramme : classification des 4 cas.
//!
//! Le bump de `SUBSTRATE_FORMAT_VERSION` 3→4 rend le format V4 obligatoire.
//! L'étape `migration_3_vers_4` classe les poids V3 SANS rien réécrire et
//! REFUSE le seul cas corrompu. Ces bancs fabriquent un store V3 (en reculant
//! la version meta d'un store neuf) puis rejouent la migration.

use obrain_substrate::error::SubstrateError;
use obrain_substrate::meta::SUBSTRATE_FORMAT_VERSION;
use obrain_substrate::migration::{ecrire_version_meta, lire_version_meta, migrer_repertoire};
use obrain_substrate::wal_io::SyncMode;
use obrain_substrate::{SubstrateFile, Writer};

/// Crée un store, applique `f` (qui écrit des engrammes), le FERME, puis
/// recule la version meta à 3. Rend le répertoire prêt à migrer.
fn store_v3<F: FnOnce(&Writer)>(dir: &std::path::Path, f: F) {
    let sub = SubstrateFile::create(dir).unwrap();
    let w = Writer::new(sub, SyncMode::Never).unwrap();
    f(&w);
    w.msync_zones().unwrap();
    w.wal().fsync().unwrap();
    drop(w);
    ecrire_version_meta(dir, 3).unwrap();
    assert_eq!(lire_version_meta(dir).unwrap(), 3);
}

/// V3 pondéré à provenance VÉRIFIABLE (lié, empreinte cohérente) et V3 sans
/// poids (legacy légitime) : la migration ne réécrit RIEN et bumpe la version.
#[test]
fn migre_v3_verifiable_et_sans_poids() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("kb");
    store_v3(&dir, |w| {
        // VÉRIFIABLE : membres + poids LIÉS.
        w.set_engram_state(7, vec![1, 2, 3], vec![0.2, 0.3, 0.5])
            .unwrap();
        // SANS POIDS : membres seuls.
        w.set_engram_members(8, vec![4, 5]).unwrap();
    });

    migrer_repertoire(&dir).expect("migration 3→4");
    assert_eq!(lire_version_meta(&dir).unwrap(), SUBSTRATE_FORMAT_VERSION);

    let sub = SubstrateFile::open(&dir).unwrap();
    let w = Writer::new(sub, SyncMode::Never).unwrap();
    assert_eq!(
        w.read_engram_state(7).unwrap(),
        Some((vec![1, 2, 3], Some(vec![0.2, 0.3, 0.5]))),
        "poids vérifiables préservés"
    );
    assert_eq!(
        w.read_engram_state(8).unwrap(),
        Some((vec![4, 5], None)),
        "sans poids = absence légitime"
    );
}

/// V3 pondéré AMBIGU (non lié, positionnel) : PRÉSERVÉ (données intactes),
/// jamais certifié — la lecture V4 refuse de réassocier par position.
#[test]
fn migre_v3_ambigu_preserve_pas_certifie() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("kb");
    store_v3(&dir, |w| {
        // AMBIGU : membres + poids NON LIÉS (legacy positionnel 0x76).
        w.set_engram_members(7, vec![1, 2, 3]).unwrap();
        w.set_engram_weights(7, vec![0.2, 0.3, 0.5]).unwrap();
    });

    migrer_repertoire(&dir).expect("migration 3→4 (ambigu préservé)");
    assert_eq!(lire_version_meta(&dir).unwrap(), SUBSTRATE_FORMAT_VERSION);

    let sub = SubstrateFile::open(&dir).unwrap();
    let w = Writer::new(sub, SyncMode::Never).unwrap();
    assert!(
        matches!(
            w.read_engram_state(7),
            Err(SubstrateError::EngramIncoherent { .. })
        ),
        "poids ambigus non certifiés : lecture refusée, données préservées"
    );
}

/// V3 CORROMPU (empreinte en contradiction avec les membres) : la migration
/// REFUSE, et la version meta ne bumpe PAS.
#[test]
fn migre_v3_refuse_les_poids_corrompus() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("kb");
    store_v3(&dir, |w| {
        // LIÉ, puis mutation des membres SEULE → empreinte en contradiction.
        w.set_engram_state(7, vec![1, 2, 3], vec![0.2, 0.3, 0.5])
            .unwrap();
        w.set_engram_members(7, vec![1, 2, 4]).unwrap();
    });

    match migrer_repertoire(&dir) {
        Err(SubstrateError::EngramIncoherent { engram_id: 7, .. }) => {}
        Err(autre) => panic!("refus nommé attendu, obtenu {autre:?}"),
        Ok(()) => panic!("la migration devait REFUSER des poids corrompus"),
    }
    assert_eq!(
        lire_version_meta(&dir).unwrap(),
        3,
        "un refus ne bumpe pas la version"
    );
}
