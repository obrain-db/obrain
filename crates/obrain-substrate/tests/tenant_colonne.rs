//! COLONNE TENANT — le contrat du jalon 2 (RFC principal-natif-v2 §4-J2).
//!
//! La rupture de format : `NodeRecord` 32 → 40 octets (colonne `tenant_id`
//! + reserve, en QUEUE de record — aucun offset v2 ne bouge), page
//! d'allocation 20 480 o = 512 records exactement, migration 2→3 par
//! reecriture de la zone nodes vers un temporaire + rename atomique.
//! Chaque noeud existant recoit `tenant_id = 0` — public DECLARE
//! (`meta_flags::MONO_TENANT_PUBLIC` pose dans le meta, §5.2). La
//! sentinelle A5 (`u32::MAX` = invisible) est RESERVEE : cette migration ne
//! la pose jamais. AUCUN filtrage par tenant ici — c'est le jalon 3 ; la
//! semantique du jalon 2 est : le substrat SAIT a qui appartient chaque
//! noeud, il ne decide encore rien.
//!
//! Les fixtures « anciennes » sont fabriquees par DECONVERSION byte-a-byte
//! d'un store du binaire courant (40 o → 32 o, retrait de la colonne,
//! slot 0 remis a zero, version reculee) : c'est l'inverse exact de la
//! migration 2→3, donc le meme dump doit ressortir de la chaine.
//!
//!   cargo test -p obrain-substrate --test tenant_colonne

use obrain_common::types::Value;
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use obrain_substrate::meta::{SUBSTRATE_FORMAT_VERSION, meta_flags};
use obrain_substrate::migration::{
    Migration, ecrire_version_meta, lire_version_meta, migrer_repertoire,
    migrer_repertoire_avec_table, reecrire_zone_nodes_2_vers_3, zone_nodes_est_v3,
};
use obrain_substrate::record::{
    NODE_ALLOC_PAGE_BYTES, NODES_PER_PAGE, NODES_ZONE_V3_MAGIC, NodeRecord, node_flags, tenant,
};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Sondes disque et fixtures
// ---------------------------------------------------------------------------

const STRIDE_V2: usize = 32;

fn zone_nodes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("substrate.nodes")).expect("zone nodes")
}

/// Lit le `tenant_id` brut du record `slot` sur disque (stride 40, octets
/// 32..36 du record).
fn tenant_sur_disque(dir: &Path, slot: u32) -> u32 {
    let bytes = zone_nodes(dir);
    let off = slot as usize * NodeRecord::SIZE + 32;
    u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
}

fn flags_sur_disque(dir: &Path, slot: u32) -> u16 {
    let bytes = zone_nodes(dir);
    let off = slot as usize * NodeRecord::SIZE + 30;
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

/// Drapeaux du header meta (offset 12).
fn drapeaux_meta(dir: &Path) -> u32 {
    let bytes = std::fs::read(dir.join("substrate.meta")).expect("meta");
    u32::from_le_bytes(bytes[12..16].try_into().unwrap())
}

fn ecrire_drapeaux_meta(dir: &Path, flags: u32) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("substrate.meta"))
        .unwrap();
    f.seek(SeekFrom::Start(12)).unwrap();
    f.write_all(&flags.to_le_bytes()).unwrap();
    f.sync_all().unwrap();
}

/// Cree `n` noeuds etiquetes `Locataire`, chacun dans SA communaute — le
/// store est troue par construction (pages d'allocation clairsemees), le
/// scenario le plus revelateur pour une reecriture de zone.
fn store_troue(dir: &Path, n: u32) -> SubstrateStore {
    let store = SubstrateStore::create(dir).expect("create");
    for c in 1..=n {
        store.create_node_in_community(&["Locataire"], c);
    }
    store
}

/// DECONVERSION 40 → 32 : fabrique un store v2 byte-exact a partir d'un
/// store du binaire courant. Retire `tenant_id`/`_reserve` de chaque
/// record (en verifiant qu'ils valaient 0 — la fixture vient d'etre
/// ecrite par des writers publics), remet le slot 0 (sentinelle de
/// stride) a zero comme dans tout store v2 reel, recule la version a 2 et
/// efface le drapeau MONO_TENANT_PUBLIC.
fn fabriquer_v2(dir: &Path) {
    let v3 = zone_nodes(dir);
    let n = v3.len() / NodeRecord::SIZE;
    let mut v2 = Vec::with_capacity(n * STRIDE_V2);
    for slot in 0..n {
        let base = slot * NodeRecord::SIZE;
        if slot == 0 {
            assert_eq!(
                u64::from_le_bytes(v3[0..8].try_into().unwrap()),
                NODES_ZONE_V3_MAGIC,
                "fixture inattendue : pas de sentinelle de stride au slot 0"
            );
            v2.extend_from_slice(&[0u8; STRIDE_V2]);
            continue;
        }
        assert_eq!(
            &v3[base + 32..base + 40],
            &[0u8; 8],
            "fixture inattendue : tenant/_reserve non nuls au slot {slot}"
        );
        v2.extend_from_slice(&v3[base..base + STRIDE_V2]);
    }
    std::fs::write(dir.join("substrate.nodes"), &v2).expect("zone 32 o");
    ecrire_version_meta(dir, 2).expect("recul version");
    ecrire_drapeaux_meta(dir, drapeaux_meta(dir) & !meta_flags::MONO_TENANT_PUBLIC);
    assert_eq!(lire_version_meta(dir).unwrap(), 2);
    assert!(!zone_nodes_est_v3(dir).unwrap(), "deconversion ratee");
}

/// Fabrique un store v1 : deconversion v2 PUIS retrait du bit ALLOCATED de
/// tous les records non nuls (l'inverse de la migration 1→2), version 1.
fn fabriquer_v1(dir: &Path) {
    fabriquer_v2(dir);
    let chemin = dir.join("substrate.nodes");
    let mut bytes = std::fs::read(&chemin).unwrap();
    let zero = [0u8; STRIDE_V2];
    for base in (0..bytes.len()).step_by(STRIDE_V2) {
        let fin = base + STRIDE_V2;
        if fin > bytes.len() || bytes[base..fin] == zero {
            continue;
        }
        let off = base + 30;
        let mut flags = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
        flags &= !node_flags::ALLOCATED;
        bytes[off..off + 2].copy_from_slice(&flags.to_le_bytes());
    }
    std::fs::write(&chemin, &bytes).unwrap();
    ecrire_version_meta(dir, 1).expect("recul version");
}

// ---------------------------------------------------------------------------
// 1. Le layout : 40 octets, 512 par page d'allocation, zero slack
// ---------------------------------------------------------------------------

#[test]
fn layout_40_octets_512_par_page_zero_slack() {
    assert_eq!(NodeRecord::SIZE, 40);
    assert_eq!(core::mem::size_of::<NodeRecord>(), 40);
    assert_eq!(core::mem::align_of::<NodeRecord>(), 8);
    assert_eq!(NODES_PER_PAGE, 512);
    assert_eq!(NODE_ALLOC_PAGE_BYTES, 20480);
    assert_eq!(
        NODES_PER_PAGE as usize * NodeRecord::SIZE,
        NODE_ALLOC_PAGE_BYTES
    );
    assert_eq!(NODE_ALLOC_PAGE_BYTES % 4096, 0, "5 pages OS exactement");
    // La colonne s'ajoute en QUEUE : les offsets v2 n'ont pas bouge.
    assert_eq!(core::mem::offset_of!(NodeRecord, flags), 30);
    assert_eq!(core::mem::offset_of!(NodeRecord, tenant_id), 32);
    // Les valeurs reservees du contrat A5.
    assert_eq!(tenant::PUBLIC, 0);
    assert_eq!(tenant::SENTINELLE_NON_MIGRE, u32::MAX);
}

// ---------------------------------------------------------------------------
// 2. Plomberie : les writers posent le tenant (0 = public), les readers
//    le rendent — et personne ne filtre (jalon 3, pas ici)
// ---------------------------------------------------------------------------

#[test]
fn writers_posent_tenant_public_et_readers_le_rendent() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path()).unwrap();
    let a = store.create_node(&["Alpha"]);
    let b = store.create_node_in_community(&["Beta"], 7);
    store.set_node_property(a, "qualified_name", Value::String("alpha:1".into()));
    drop(store);

    let store = SubstrateStore::open(td.path()).unwrap();
    // Le reader rend le NodeRecord complet, tenant compris.
    for id in [a, b] {
        let rec = store
            .writer()
            .read_node(id.0 as u32)
            .unwrap()
            .expect("record lu");
        assert!(rec.is_live());
        assert_eq!(rec.tenant_id, tenant::PUBLIC, "writer -> tenant 0 = public");
        assert_eq!(rec._reserve, 0);
    }
    // Et sur le disque nu, aux octets 32..36.
    assert_eq!(tenant_sur_disque(td.path(), a.0 as u32), tenant::PUBLIC);
    assert_eq!(tenant_sur_disque(td.path(), b.0 as u32), tenant::PUBLIC);
    // AUCUN filtrage : tout est lisible comme avant.
    assert_eq!(store.node_ids().len(), 2);
    assert_eq!(store.node_count(), 2);
}

/// La sentinelle de stride du slot 0 est presente des la creation, et
/// n'est JAMAIS un noeud (flags nuls : ni vivante, ni comptee).
#[test]
fn sentinelle_slot_zero_invisible_des_la_creation() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path()).unwrap();
    assert!(zone_nodes_est_v3(td.path()).unwrap());
    store.create_node(&["Seul"]);
    assert_eq!(
        store.node_ids().len(),
        1,
        "la sentinelle n'est pas un noeud"
    );
    assert_eq!(store.node_count(), 1);
    drop(store);
    let store = SubstrateStore::open(td.path()).unwrap();
    assert_eq!(store.node_ids().len(), 1);
    assert_eq!(flags_sur_disque(td.path(), 0), 0, "slot 0 jamais ALLOCATED");
}

// ---------------------------------------------------------------------------
// 3. Migration 2→3 : tenant 0 partout, public declare, jamais la
//    sentinelle A5
// ---------------------------------------------------------------------------

#[test]
fn migration_2_vers_3_pose_tenant_zero_partout_et_declare_public() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 10);
    let ids_avant = store.node_ids();
    let victime = ids_avant[3];
    store.delete_node(victime);
    let ids_apres_suppression = store.node_ids();
    drop(store);
    fabriquer_v2(td.path());

    // L'ouverture EXCLUSIVE migre (machinerie jalon 0, etape 2→3).
    let store = SubstrateStore::open(td.path()).expect("open migrant");
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        SUBSTRATE_FORMAT_VERSION
    );
    assert_eq!(store.node_ids(), ids_apres_suppression, "dump identique");
    assert_eq!(store.nodes_by_label("Locataire").len(), 9);
    drop(store);

    // Sur disque : zone re-strideee, marquee v3, tenant 0 sur TOUT slot —
    // vivants, tombstone, trous — et JAMAIS u32::MAX (A5 reservee).
    assert!(zone_nodes_est_v3(td.path()).unwrap());
    let bytes = zone_nodes(td.path());
    let n = bytes.len() / NodeRecord::SIZE;
    assert!(n > 1);
    for slot in 1..n {
        let off = slot * NodeRecord::SIZE + 32;
        let t = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        assert_eq!(
            t,
            tenant::PUBLIC,
            "slot {slot} : tenant != 0 apres migration en bloc"
        );
    }
    // Le tombstone a survecu tel quel.
    let f = flags_sur_disque(td.path(), victime.0 as u32);
    assert_ne!(f & node_flags::TOMBSTONED, 0);
    assert_ne!(f & node_flags::ALLOCATED, 0);
    // Le public est DECLARE, pas subi.
    assert_ne!(
        drapeaux_meta(td.path()) & meta_flags::MONO_TENANT_PUBLIC,
        0,
        "la migration en bloc doit poser MONO_TENANT_PUBLIC (RFC §5.2)"
    );
}

#[test]
fn migration_2_vers_3_idempotente_octet_pour_octet() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 5);
    drop(store);
    fabriquer_v2(td.path());

    migrer_repertoire(td.path()).expect("premiere migration");
    let zone_1 = zone_nodes(td.path());
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        SUBSTRATE_FORMAT_VERSION
    );

    migrer_repertoire(td.path()).expect("seconde migration = no-op");
    let zone_2 = zone_nodes(td.path());
    assert_eq!(zone_1, zone_2, "no-op octet pour octet");
}

// ---------------------------------------------------------------------------
// 4. La chaine complete 1→2→3 en UNE ouverture
// ---------------------------------------------------------------------------

#[test]
fn chaine_complete_v1_vers_v3_en_une_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let store = store_troue(td.path(), 10);
    let ids_avant = store.node_ids();
    drop(store);
    fabriquer_v1(td.path());

    // Une seule ouverture : 1→2 (benediction ALLOCATED, sur du 32 o) puis
    // 2→3 (reecriture 40 o, tenant 0) — dans la meme chaine.
    let store = SubstrateStore::open(td.path()).expect("open migrant v1");
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        SUBSTRATE_FORMAT_VERSION
    );
    assert_eq!(
        store.node_ids(),
        ids_avant,
        "10 crees -> 10 lisibles, jamais 5120"
    );
    assert_eq!(store.node_count(), 10);
    for id in &ids_avant {
        assert_eq!(tenant_sur_disque(td.path(), id.0 as u32), tenant::PUBLIC);
    }
    assert_ne!(drapeaux_meta(td.path()) & meta_flags::MONO_TENANT_PUBLIC, 0);
}

// ---------------------------------------------------------------------------
// 5. props, aretes, labels : la migration ne touche QUE la zone nodes
// ---------------------------------------------------------------------------

#[test]
fn props_aretes_et_wal_intacts_a_travers_la_migration() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create(td.path()).unwrap();
    let a = store.create_node(&["Doc"]);
    let b = store.create_node(&["Doc"]);
    store.set_node_property(a, "qualified_name", Value::String("doc:alpha".into()));
    store.set_node_property(b, "qualified_name", Value::String("doc:beta".into()));
    let _e = store.create_edge(a, b, "REL");
    drop(store);
    fabriquer_v2(td.path());

    // La migration (appel direct, sans ouverture) ne touche ni le WAL, ni
    // les zones props/edges/strings.
    let wal_avant = std::fs::read(td.path().join("substrate.wal")).unwrap();
    let edges_avant = std::fs::read(td.path().join("substrate.edges")).unwrap();
    migrer_repertoire(td.path()).expect("migration directe");
    let wal_apres = std::fs::read(td.path().join("substrate.wal")).unwrap();
    let edges_apres = std::fs::read(td.path().join("substrate.edges")).unwrap();
    assert_eq!(
        wal_avant, wal_apres,
        "la migration ne draine ni ne tronque le WAL"
    );
    assert_eq!(edges_avant, edges_apres, "zone edges intacte");

    // Et tout se relit : props par id (les ids n'ont pas bouge), aretes,
    // labels.
    let store = SubstrateStore::open(td.path()).expect("reopen");
    let pk = obrain_common::types::PropertyKey::new("qualified_name");
    assert_eq!(
        store.get_node_property(a, &pk),
        Some(Value::String("doc:alpha".into())),
        "qualified_name lisible apres migration (props referencees par id)"
    );
    assert_eq!(
        store.get_node_property(b, &pk),
        Some(Value::String("doc:beta".into()))
    );
    let sortants = store.edges_from(a, obrain_core::graph::Direction::Outgoing);
    assert_eq!(sortants.len(), 1);
    assert_eq!(sortants[0].0, b);
    assert_eq!(store.nodes_by_label("Doc").len(), 2);
}

// ---------------------------------------------------------------------------
// 5 bis. Migration d'un store REEL (copie) — porte manuelle
// ---------------------------------------------------------------------------

/// Preuve sur donnees reelles (critere RFC §3) : pointer
/// `OBRAIN_TEST_STORE_REEL_DIR` sur la COPIE d'un store de production
/// (jamais l'original), relever les comptes AVANT avec le binaire qui sait
/// le lire, puis lancer ce test : il ouvre (donc migre 1→2→3 ou 2→3),
/// verifie version 3, tenant 0 sur tout vivant, drapeau public pose, et
/// imprime les comptes a comparer au releve d'avant.
#[test]
fn migration_store_reel_par_copie() {
    let Ok(dir) = std::env::var("OBRAIN_TEST_STORE_REEL_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let store = SubstrateStore::open(&dir).expect("open migrant du store reel");
    assert_eq!(lire_version_meta(&dir).unwrap(), SUBSTRATE_FORMAT_VERSION);
    assert!(zone_nodes_est_v3(&dir).unwrap());
    assert_ne!(drapeaux_meta(&dir) & meta_flags::MONO_TENANT_PUBLIC, 0);
    let ids = store.node_ids();
    assert!(!ids.is_empty(), "store reel vide ?");
    for id in &ids {
        assert_eq!(
            tenant_sur_disque(&dir, id.0 as u32),
            tenant::PUBLIC,
            "noeud reel {id:?} sans tenant public"
        );
    }
    eprintln!(
        "store reel migre : {} noeuds vivants, {} comptes, {} aretes, {} Chantier, version {}",
        ids.len(),
        store.node_count(),
        store.edge_count(),
        store.nodes_by_label("Chantier").len(),
        lire_version_meta(&dir).unwrap()
    );
}

// ---------------------------------------------------------------------------
// 6. KILL -9 en pleine reecriture 2→3, puis reprise par l'ouverture
// ---------------------------------------------------------------------------

/// Etape lente pour l'enfant : la vraie reecriture, ralentie (200 ms par
/// tranche de 4096 slots). Le parent kill -9 pendant que le tmp grossit.
fn etape_lente_2v3(dir: &Path) -> Result<(), obrain_substrate::error::SubstrateError> {
    reecrire_zone_nodes_2_vers_3(dir, Some(std::time::Duration::from_millis(200)))
}

#[test]
fn enfant_migration_2v3_lente() {
    let Ok(dir) = std::env::var("OBRAIN_TEST_ENFANT_MIGRATION_2V3") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let table = vec![Migration {
        de: 2,
        vers: SUBSTRATE_FORMAT_VERSION,
        etape: etape_lente_2v3,
    }];
    let _ = migrer_repertoire_avec_table(&dir, &table);
}

#[test]
fn kill_9_pendant_reecriture_2v3_puis_reprise_par_ouverture() {
    let td = tempfile::tempdir().unwrap();
    // 40 communautes x 512 slots ≈ 20k slots : ~5 tranches de 200 ms —
    // une fenetre de kill large et observable.
    let store = store_troue(td.path(), 40);
    let ids_avant = store.node_ids();
    drop(store);
    fabriquer_v2(td.path());
    let zone_v2 = zone_nodes(td.path());

    let exe = std::env::current_exe().unwrap();
    let mut enfant = std::process::Command::new(exe)
        .args(["enfant_migration_2v3_lente", "--exact", "--nocapture"])
        .env("OBRAIN_TEST_ENFANT_MIGRATION_2V3", td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");

    // Attendre que la reecriture soit REELLEMENT en cours : pas juste le
    // tmp cree, mais au moins UNE tranche de 4096 records flushee — le
    // kill frappe un fichier temporaire a moitie construit, pas un fichier
    // vide.
    let tmp = td.path().join("substrate.nodes.v3.tmp");
    let seuil = (4096 * NodeRecord::SIZE) as u64;
    let mut attentes = 0u32;
    loop {
        let taille = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        if taille >= seuil {
            break;
        }
        attentes += 1;
        assert!(
            attentes < 1200,
            "l'enfant n'a jamais atteint la premiere tranche (tmp = {} o)",
            std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0)
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();
    // Le debris est bien un tmp PARTIEL (la zone finale aurait n_slots*40).
    let taille_tmp = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    let attendu_final = (zone_v2.len() / 32) * NodeRecord::SIZE;
    assert!(
        (taille_tmp as usize) < attendu_final,
        "le tmp est deja complet ({taille_tmp} o >= {attendu_final}) — le kill n'a rien prouve"
    );

    // Tue en pleine reecriture : version TOUJOURS 2 (point de commit non
    // atteint), zone nodes 32 o INTACTE octet pour octet — le tmp dechire
    // est le seul debris.
    assert_eq!(lire_version_meta(td.path()).unwrap(), 2, "jamais mixte");
    assert_eq!(zone_nodes(td.path()), zone_v2, "l'ancienne zone fait foi");
    assert!(!zone_nodes_est_v3(td.path()).unwrap());

    // Reprise par l'OUVERTURE normale : reconstruction du tmp de zero,
    // bascule, bump — dump identique, tenant 0 partout.
    let store = SubstrateStore::open(td.path()).expect("reprise par open");
    assert_eq!(
        lire_version_meta(td.path()).unwrap(),
        SUBSTRATE_FORMAT_VERSION
    );
    assert_eq!(
        store.node_ids(),
        ids_avant,
        "40 crees -> 40 lisibles apres reprise"
    );
    assert_eq!(store.node_count(), 40);
    for id in &ids_avant {
        assert_eq!(tenant_sur_disque(td.path(), id.0 as u32), tenant::PUBLIC);
    }
}
