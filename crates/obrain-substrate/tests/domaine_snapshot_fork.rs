//! COHÉRENCE SNAPSHOT / FORK (#334/#418) — un instantané ne transporte jamais
//! une paire `domaine.id`/`domaine.init` INTERMÉDIAIRE incohérente SANS son
//! contexte de récupération (`domaine.fork`), et la continuité logique est
//! préservée quand l'opération est une restauration DÉCLARÉE.
//!
//! Avant/après fork (journal absent) : la paire est COHÉRENTE, donc transportée
//! (contrat de continuité — une restauration écrase la base par la copie,
//! identité comprise). PENDANT un fork (journal `domaine.fork` présent), la paire
//! est potentiellement discordante : elle est EXCLUE de l'instantané — jamais une
//! paire incohérente sans contexte de récupération.

use std::path::Path;

use obrain_common::types::Value;
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::instantane::{exporter_avec_dict, restaurer};
use obrain_substrate::{
    SubstrateFile, SubstrateStore, FICHIER_DOMAINE_FORK, FICHIER_DOMAINE_ID, FICHIER_DOMAINE_INIT,
};

/// Identité simulée du fork « déchiré » (le vrai fork est interrompu par kill -9
/// dans `domaine_fork_interruption_kill9.rs`) : non nulle, distincte de D0.
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

fn noms_zones(store: &SubstrateStore) -> Vec<String> {
    store.avec_substrate(|sub| sub.zone_names().unwrap())
}

fn snapshot_zones(store: &SubstrateStore) -> Vec<u8> {
    let dict = store.octets_dictionnaire().unwrap();
    store
        .avec_substrate(|sub| exporter_avec_dict(sub, Some(&dict)))
        .unwrap()
}

fn restaurer_disque(image: &[u8], dir: &Path) {
    let mut sub = SubstrateFile::create(dir).unwrap();
    restaurer(&mut sub, image).unwrap();
}

fn creer_base_avec_donnee(base: &Path) -> u128 {
    let store = SubstrateStore::create(base).unwrap();
    let d0 = store.domaine_id();
    let n = store.create_node(&["ressource"]);
    store.set_node_property(n, "a", Value::Int64(1));
    store.close().unwrap();
    d0
}

/// Avant fork : la paire d'identité est transportée (cohérente), le journal ne
/// l'est jamais.
#[test]
fn zone_names_avant_fork_transporte_la_paire() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    let store = SubstrateStore::open(&base).unwrap();
    let noms = noms_zones(&store);
    assert!(noms.iter().any(|n| n == FICHIER_DOMAINE_ID), "domaine.id transporté");
    assert!(noms.iter().any(|n| n == FICHIER_DOMAINE_INIT), "domaine.init transporté");
    assert!(!noms.iter().any(|n| n == FICHIER_DOMAINE_FORK), "le journal n'est jamais transporté");
    store.close().unwrap();
}

/// Après fork : la paire (nouvelle, cohérente) est transportée.
#[test]
fn zone_names_apres_fork_transporte_la_nouvelle_paire() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    let store = SubstrateStore::open(&base).unwrap();
    let d1 = store.forker_domaine().unwrap();
    assert_ne!(d1, 0);
    let noms = noms_zones(&store);
    assert!(noms.iter().any(|n| n == FICHIER_DOMAINE_ID));
    assert!(noms.iter().any(|n| n == FICHIER_DOMAINE_INIT));
    assert!(!noms.iter().any(|n| n == FICHIER_DOMAINE_FORK));
    store.close().unwrap();
}

/// PENDANT un fork (journal présent) : la paire discordante est EXCLUE — jamais
/// transportée sans son contexte de récupération.
#[test]
fn zone_names_pendant_fork_ne_transporte_pas_la_paire_incoherente() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    simuler_fork_dechire(&base);

    // Ouverture au niveau SUBSTRAT (pas de résolution de domaine) : la paire
    // discordante n'est pas un obstacle à la capture des zones.
    let sub = SubstrateFile::open_read_only(&base).unwrap();
    let noms = sub.zone_names().unwrap();
    assert!(
        !noms.iter().any(|n| n == FICHIER_DOMAINE_ID),
        "domaine.id exclu pendant le fork (réécrit)"
    );
    assert!(
        !noms.iter().any(|n| n == FICHIER_DOMAINE_INIT),
        "domaine.init exclu pendant le fork (discordant)"
    );
    assert!(!noms.iter().any(|n| n == FICHIER_DOMAINE_FORK), "le journal reste exclu");
}

/// Restauration DÉCLARÉE avant fork : continuité logique — la base restaurée
/// relit la MÊME identité, et la donnée survit.
#[test]
fn snapshot_avant_fork_restaure_la_meme_identite() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let d0 = creer_base_avec_donnee(&base);
    let image = {
        let store = SubstrateStore::open(&base).unwrap();
        let img = snapshot_zones(&store);
        store.close().unwrap();
        img
    };
    let dir2 = td.path().join("restored");
    restaurer_disque(&image, &dir2);
    let store2 = SubstrateStore::open(&dir2).unwrap();
    assert_eq!(store2.domaine_id(), d0, "continuité : même identité restaurée");
    assert_eq!(store2.node_count(), 1, "la donnée survit à la restauration");
    store2.close().unwrap();
}

/// Restauration DÉCLARÉE après fork : la base restaurée porte la NOUVELLE identité.
#[test]
fn snapshot_apres_fork_restaure_la_nouvelle_identite() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    let (d1, image) = {
        let store = SubstrateStore::open(&base).unwrap();
        let d1 = store.forker_domaine().unwrap();
        let img = snapshot_zones(&store);
        store.close().unwrap();
        (d1, img)
    };
    let dir2 = td.path().join("restored");
    restaurer_disque(&image, &dir2);
    let store2 = SubstrateStore::open(&dir2).unwrap();
    assert_eq!(store2.domaine_id(), d1, "la nouvelle identité est restaurée");
    store2.close().unwrap();
}

/// Restauration d'un instantané pris PENDANT le fork : jamais de paire
/// incohérente — la base restaurée s'ouvre sur une identité ÉPHÉMÈRE (rien de
/// discordant, rien de matérialisé), pas sur un refus d'empreinte discordante.
#[test]
fn snapshot_pendant_fork_restaure_sans_paire_incoherente() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    creer_base_avec_donnee(&base);
    simuler_fork_dechire(&base);

    // Instantané de l'état déchiré, au niveau substrat (sans dict : seul le
    // contrat d'identité est qualifié ici).
    let image = {
        let sub = SubstrateFile::open_read_only(&base).unwrap();
        exporter_avec_dict(&sub, None).unwrap()
    };
    let dir2 = td.path().join("restored");
    restaurer_disque(&image, &dir2);

    // L'ouverture ne REFUSE PAS (pas d'empreinte discordante) : la paire n'a pas
    // été transportée, la base restaurée est éphémère.
    let store2 = SubstrateStore::open(&dir2).unwrap();
    assert_ne!(store2.domaine_id(), 0, "identité éphémère servie, jamais refusée");
    assert!(
        !dir2.join(FICHIER_DOMAINE_ID).exists() && !dir2.join(FICHIER_DOMAINE_INIT).exists(),
        "rien n'est matérialisé silencieusement"
    );
    store2.close().unwrap();
}
