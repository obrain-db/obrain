//! TROUS / ALLOCATED — le contrat du jalon 1 (RFC principal-natif-v2 §4-J1,
//! plan A : ALLOCATED seul, bump 1→2, sans changement de layout).
//!
//! Le bug prouvé : l'allocateur par communauté aligne chaque nouvelle
//! communauté sur une frontière de page (128 slots), laissant des créneaux
//! JAMAIS ÉCRITS que la zone mmap rend comme `NodeRecord::default()`
//! non-tombstone — 10 noeuds créés, 132+ « lisibles » (audit AGREGATS-O1).
//!
//! Le correctif : `node_flags::ALLOCATED` (bit 6), posé par TOUT writer de
//! noeud, jamais effacé (un tombstone reste ALLOCATED|TOMBSTONED). Un slot
//! sans ALLOCATED est un trou — point final, plus d'heuristique 8-champs.
//! La migration 1→2 bénit les stores v1 existants : elle pose ALLOCATED sur
//! tout record non-défaut (l'heuristique sert UNE dernière fois, là et
//! nulle part ailleurs), puis la meta passe à 2 (point de commit, jalon 0).
//!
//!   cargo test -p obrain-substrate --test trous_allocated

use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use obrain_substrate::meta::SUBSTRATE_FORMAT_VERSION;
use obrain_substrate::migration::{
    Migration, ecrire_version_meta, lire_version_meta, migrer_repertoire,
    migrer_repertoire_avec_table, poser_allocated_zone_nodes,
};
use obrain_substrate::record::{NodeRecord, node_flags};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Fixtures et sondes disque
// ---------------------------------------------------------------------------

/// Cree `n` noeuds etiquetes `Troue`, chacun dans SA communaute (1..=n) :
/// chaque allocation ouvre une page de 128 slots → le store est troue par
/// construction (le scenario mesure par AGREGATS).
fn store_troue(dir: &Path, n: u32) -> SubstrateStore {
    let store = SubstrateStore::create(dir).expect("create");
    for c in 1..=n {
        store.create_node_in_community(&["Troue"], c);
    }
    store
}

/// Lit les flags bruts du record `slot` dans `substrate.nodes` (sur disque).
fn flags_sur_disque(dir: &Path, slot: u32) -> u16 {
    let bytes = std::fs::read(dir.join("substrate.nodes")).expect("zone nodes");
    let off = slot as usize * NodeRecord::SIZE + 30; // flags: u16, offset 30
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

/// Fabrique un store v1 : efface le bit ALLOCATED de tous les records
/// non-nuls de la zone nodes, puis recule la version meta a 1. C'est
/// l'inverse exact de la migration 1→2 (sur des fixtures sans noeud
/// « nu » de communaute 0, seul cas byte-pour-byte ambigu en v1).
fn fabriquer_v1(dir: &Path) {
    let chemin = dir.join("substrate.nodes");
    let mut bytes = std::fs::read(&chemin).expect("zone nodes");
    let zero = [0u8; NodeRecord::SIZE];
    for base in (0..bytes.len()).step_by(NodeRecord::SIZE) {
        let fin = base + NodeRecord::SIZE;
        if fin > bytes.len() || bytes[base..fin] == zero {
            continue;
        }
        let off = base + 30;
        let mut flags = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
        flags &= !node_flags::ALLOCATED;
        bytes[off..off + 2].copy_from_slice(&flags.to_le_bytes());
    }
    std::fs::write(&chemin, &bytes).expect("reecriture zone nodes");
    ecrire_version_meta(dir, 1).expect("recul version");
    assert_eq!(lire_version_meta(dir).unwrap(), 1);
}

// ---------------------------------------------------------------------------
// 1. Le test des trous : N crees → N lisibles, N comptes
// ---------------------------------------------------------------------------

#[test]
fn dix_crees_dix_lisibles_dix_comptes() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 10);

    // La fixture troue vraiment : 10 communautes × pages de 128 slots.
    assert!(
        store.slot_high_water() > 1000,
        "fixture sans trous — high water {} trop bas",
        store.slot_high_water()
    );

    assert_eq!(store.node_ids().len(), 10, "10 crees -> 10 lisibles");
    assert_eq!(store.nodes_by_label("Troue").len(), 10);
    assert_eq!(store.node_count(), 10, "10 crees -> 10 comptes");
    drop(store);

    // Reouverture : les compteurs reconstruits depuis les zones comptent
    // les ALLOCATED vivants, pas les slots.
    let store = SubstrateStore::open(td.path()).expect("reopen");
    assert_eq!(store.node_ids().len(), 10);
    assert_eq!(store.nodes_by_label("Troue").len(), 10);
    assert_eq!(store.node_count(), 10);
    assert_eq!(
        store.node_ids().len(),
        store.node_count(),
        "critere RFC : node_ids().len() == node_count() sur store troue"
    );
}

// ---------------------------------------------------------------------------
// 2. Le cas jadis ambigu : noeud nu en communaute 0 ≠ trou
// ---------------------------------------------------------------------------

#[test]
fn noeud_nu_communaute_zero_discernable_dun_trou() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path()).unwrap();
    let nu = store.create_node_in_community(&[], 0);
    store.create_node_in_community(&["Temoin"], 5);
    assert_eq!(
        store.node_ids().len(),
        2,
        "le noeud nu (labels vides, communaute 0) est un noeud, pas un trou"
    );
    drop(store);

    // Sur disque : son record ne porte QUE le bit ALLOCATED — c'etait le
    // cas byte-pour-byte identique a un trou avant le jalon 1.
    let flags = flags_sur_disque(td.path(), nu.0 as u32);
    assert_eq!(flags & node_flags::ALLOCATED, node_flags::ALLOCATED);

    let store = SubstrateStore::open(td.path()).unwrap();
    assert_eq!(store.node_ids().len(), 2, "discernable apres reouverture");
    assert_eq!(store.node_count(), 2);
}

// ---------------------------------------------------------------------------
// 3. Un tombstone reste ALLOCATED (jamais efface)
// ---------------------------------------------------------------------------

#[test]
fn tombstone_reste_alloue_et_invisible() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 3);
    let victime = store.node_ids()[1];
    store.delete_node(victime);
    assert_eq!(store.node_ids().len(), 2);
    drop(store);

    let flags = flags_sur_disque(td.path(), victime.0 as u32);
    assert_ne!(flags & node_flags::ALLOCATED, 0, "tombstone reste ALLOCATED");
    assert_ne!(flags & node_flags::TOMBSTONED, 0, "et TOMBSTONED");

    let store = SubstrateStore::open(td.path()).unwrap();
    assert_eq!(store.node_ids().len(), 2);
    assert_eq!(store.node_count(), 2);
}

// ---------------------------------------------------------------------------
// 4. Migration 1→2 par l'ouverture : compte juste, dump identique
// ---------------------------------------------------------------------------

#[test]
fn store_v1_migre_a_l_ouverture_compte_juste() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 10);
    let ids_avant = store.node_ids();
    drop(store);

    fabriquer_v1(td.path());

    // L'ouverture EXCLUSIVE migre (jalon 0) via la premiere entree reelle
    // de la table : 1→2 pose ALLOCATED sur tout record non-defaut.
    let store = SubstrateStore::open(td.path()).expect("open migrant");
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert_eq!(store.node_ids(), ids_avant, "dump identique avant/apres");
    assert_eq!(store.nodes_by_label("Troue").len(), 10);
    assert_eq!(store.node_count(), 10);
}

#[test]
fn migration_1_vers_2_idempotente_octet_pour_octet() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 5);
    drop(store);
    fabriquer_v1(td.path());

    migrer_repertoire(td.path()).expect("premiere migration");
    let zone_apres_1 = std::fs::read(td.path().join("substrate.nodes")).unwrap();
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);

    migrer_repertoire(td.path()).expect("seconde migration = no-op");
    let zone_apres_2 = std::fs::read(td.path().join("substrate.nodes")).unwrap();
    assert_eq!(zone_apres_1, zone_apres_2, "no-op octet pour octet");

    let store = SubstrateStore::open(td.path()).unwrap();
    assert_eq!(store.node_ids().len(), 5);
}

// ---------------------------------------------------------------------------
// 5. KILL -9 pendant la migration 1→2, puis reprise par l'ouverture
// ---------------------------------------------------------------------------

/// Etape lente pour l'enfant : pose ALLOCATED sur la PREMIERE MOITIE des
/// slots (la vraie logique de migration, bornee), signale, puis dort.
fn etape_lente_1v2(dir: &Path) -> Result<(), obrain_substrate::error::SubstrateError> {
    let taille = std::fs::metadata(dir.join("substrate.nodes"))?.len();
    let moitie = (taille as usize / NodeRecord::SIZE / 2) as u32;
    poser_allocated_zone_nodes(dir, Some(moitie))?;
    std::fs::write(dir.join("enfant_pret"), b"pret")?;
    for _ in 0..3000 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

#[test]
fn enfant_migration_1v2_lente() {
    let Ok(dir) = std::env::var("OBRAIN_TEST_ENFANT_MIGRATION_1V2") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let table = vec![Migration {
        de: 1,
        vers: SUBSTRATE_FORMAT_VERSION,
        etape: etape_lente_1v2,
    }];
    let _ = migrer_repertoire_avec_table(&dir, &table);
}

#[test]
fn kill_9_pendant_migration_1v2_puis_reprise_par_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 20);
    let ids_avant = store.node_ids();
    drop(store);
    fabriquer_v1(td.path());

    let exe = std::env::current_exe().unwrap();
    let mut enfant = std::process::Command::new(exe)
        .args(["enfant_migration_1v2_lente", "--exact", "--nocapture"])
        .env("OBRAIN_TEST_ENFANT_MIGRATION_1V2", td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");

    let pret = td.path().join("enfant_pret");
    let mut attentes = 0u32;
    while !pret.exists() {
        attentes += 1;
        assert!(attentes < 600, "l'enfant n'a jamais atteint la mi-etape");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    // Mi-migration tuee : version TOUJOURS 1 (le point de commit n'est pas
    // atteint), une partie des records deja benie — etat rejouable.
    assert_eq!(lire_version_meta(td.path()).unwrap(), 1, "jamais mixte");

    // Reprise par l'OUVERTURE normale : la vraie table (1→2) rejoue
    // l'etape — idempotente sur les records deja benis — et bump en dernier.
    let store = SubstrateStore::open(td.path()).expect("reprise par open");
    assert_eq!(lire_version_meta(td.path()).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert_eq!(store.node_ids(), ids_avant, "20 crees -> 20 lisibles, jamais 2560");
    assert_eq!(store.node_count(), 20);
}
