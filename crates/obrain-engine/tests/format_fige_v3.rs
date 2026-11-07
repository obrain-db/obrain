//! LE FORMAT ON-DISK EST FIGÉ EN v0.0.1 — et ce fichier le verrouille.
//!
//! Deux choses sont prouvées :
//!   1. une base créée par ce binaire porte `format_version = 3` dans
//!      `substrate.meta` (magic `SUBSTRT\0`), et les fichiers attendus ;
//!   2. la FIXTURE de référence `tests/golden/v3-mini.tgz` (écrite une fois
//!      par ce même test, sous `OBRAIN_GOLDEN_WRITE=1`, puis commitée) se
//!      rouvre avec ce binaire et rend exactement les comptes attendus. Une
//!      base neuve pèse ~21 Mo de zones pré-allouées et 27 Ko compressée :
//!      c'est l'archive qui est versionnée, extraite dans un tempdir par `tar`.
//!
//! Si le format change (version, layout, nom de fichier), le test 2 rougit :
//! c'est voulu. La réponse n'est jamais « régénérer la fixture en silence »,
//! c'est une migration dans `migration.rs::table()` ET une entrée de
//! CHANGELOG — puis seulement une nouvelle fixture, à côté de l'ancienne.
//!
//! La fixture est EXTRAITE dans un répertoire temporaire avant ouverture : un
//! `open` rejoue le WAL, pose un verrou et peut écrire un checkpoint — rien
//! de cela ne doit toucher l'arbre git.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::{Path, PathBuf};

const VERSION_FIGEE: u32 = 3;
const MAGIC: &[u8; 8] = b"SUBSTRT\0";

fn golden_tgz() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/v3-mini.tgz")
}

fn tar(args: &[&str], cwd: &Path) {
    let statut = std::process::Command::new("tar")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("tar introuvable");
    assert!(statut.success(), "tar {args:?} a échoué");
}

fn peupler(db: &ObrainDB) {
    let s = db.session();
    s.execute("INSERT (:Person {name: 'Alix', age: 30})")
        .unwrap();
    s.execute("INSERT (:Person {name: 'Gus', age: 25})")
        .unwrap();
    s.execute("INSERT (:City {name: 'Lyon'})").unwrap();
    s.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'}) \
         INSERT (a)-[:KNOWS {since: 2020}]->(b)",
    )
    .unwrap();
    s.execute(
        "MATCH (a:Person {name: 'Gus'}), (c:City {name: 'Lyon'}) \
         INSERT (a)-[:LIVES_IN]->(c)",
    )
    .unwrap();
}

fn verifier_contenu(db: &ObrainDB) {
    let s = db.session();
    let n = s.execute("MATCH (n) RETURN n").unwrap().row_count();
    assert_eq!(n, 3, "3 nœuds attendus");
    let e = s.execute("MATCH ()-[r]->() RETURN r").unwrap().row_count();
    assert_eq!(e, 2, "2 arêtes attendues");
    let age = s
        .execute("MATCH (p:Person {name: 'Alix'}) RETURN p.age")
        .unwrap();
    assert_eq!(age.rows[0][0], Value::Int64(30));
    let since = s
        .execute("MATCH (:Person {name: 'Alix'})-[r:KNOWS]->(:Person {name: 'Gus'}) RETURN r.since")
        .unwrap();
    assert_eq!(since.rows[0][0], Value::Int64(2020));
}

fn version_meta(dir: &Path) -> (Vec<u8>, u32) {
    let octets = std::fs::read(dir.join("substrate.meta")).expect("substrate.meta");
    let magic = octets[0..8].to_vec();
    let version = u32::from_le_bytes(octets[8..12].try_into().unwrap());
    (magic, version)
}

#[test]
fn une_base_neuve_porte_le_format_3_et_ses_fichiers() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("base");
    {
        let db = ObrainDB::open(&dir).unwrap();
        peupler(&db);
        db.close().unwrap();
    }
    let (magic, version) = version_meta(&dir);
    assert_eq!(&magic[..], MAGIC, "magic de substrate.meta");
    assert_eq!(version, VERSION_FIGEE, "format_version de substrate.meta");
    for f in [
        "substrate.meta",
        "substrate.nodes",
        "substrate.edges",
        "substrate.wal",
    ] {
        assert!(dir.join(f).is_file(), "fichier attendu absent : {f}");
    }
    let db = ObrainDB::open(&dir).unwrap();
    verifier_contenu(&db);
}

#[test]
fn la_fixture_de_reference_v3_se_rouvre_et_rend_les_memes_comptes() {
    let tgz = golden_tgz();
    if std::env::var_os("OBRAIN_GOLDEN_WRITE").is_some() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("v3-mini");
        let db = ObrainDB::open(&dir).unwrap();
        peupler(&db);
        db.close().unwrap();
        // Les verrous n'ont rien à faire dans une archive.
        let _ = std::fs::remove_file(dir.join("substrate.dirlock"));
        let _ = std::fs::remove_file(dir.join("substrate.dict.lock"));
        std::fs::create_dir_all(tgz.parent().unwrap()).unwrap();
        tar(&["czf", tgz.to_str().unwrap(), "v3-mini"], td.path());
        eprintln!("fixture écrite dans {}", tgz.display());
    }
    assert!(
        tgz.is_file(),
        "fixture absente : lancer une fois avec OBRAIN_GOLDEN_WRITE=1 puis la commiter ({})",
        tgz.display()
    );

    let td = tempfile::tempdir().unwrap();
    tar(&["xzf", tgz.to_str().unwrap()], td.path());
    let copie = td.path().join("v3-mini");
    let (magic, version) = version_meta(&copie);
    assert_eq!(&magic[..], MAGIC);
    assert_eq!(
        version, VERSION_FIGEE,
        "la fixture n'est plus au format figé : migration + CHANGELOG, pas de régénération silencieuse"
    );
    let db = ObrainDB::open(&copie).expect("la fixture v3 doit s'ouvrir avec ce binaire");
    verifier_contenu(&db);
}

// ───────────────────────────────────────────────────────────────────────────
// UN OPCODE WAL INCONNU REFUSE L'OUVERTURE — jamais lu comme vide ni partiel.
//
// Une frame complète, à CRC valide, dont le `kind` est inconnu de ce binaire
// vient d'un binaire PLUS RÉCENT (le CRC couvre l'octet `kind`). Le moteur
// doit refuser d'ouvrir, nommer l'opcode, et ne rien écrire — ni rejouer un
// préfixe que le checkpoint suivant figerait en tronquant le WAL.
// Pendant substrat : `obrain-substrate/tests/wal_opcode_inconnu.rs`.
// ───────────────────────────────────────────────────────────────────────────

/// Opcode qu'aucun binaire v0.0.x ne connaît : 0x7E, hors de toute plage
/// attribuée (0x59 est RÉSERVÉ par le chantier MP-4 pour `TxOuverte`).
const OPCODE_FUTUR: u8 = 0x7E;

/// Frame au framing de `wal.rs` (28 o d'en-tête : longueur u32 LE, kind u8,
/// flags u8, réservé u16 = 0, lsn u64 LE, timestamp i64 LE, crc32 u32 LE sur
/// les 24 premiers octets puis le payload).
fn frame_opcode_futur() -> Vec<u8> {
    let payload: &[u8] = b"payload d'un binaire plus recent";
    let mut entete = Vec::with_capacity(24);
    entete.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    entete.push(OPCODE_FUTUR);
    entete.push(0);
    entete.extend_from_slice(&[0, 0]);
    entete.extend_from_slice(&1_000_000u64.to_le_bytes());
    entete.extend_from_slice(&1_700_000_000_000_000i64.to_le_bytes());
    let mut h = crc32fast::Hasher::new();
    h.update(&entete);
    h.update(payload);
    let crc = h.finalize();
    let mut frame = entete;
    frame.extend_from_slice(&crc.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn ajouter_au_wal(dir: &Path, octets: &[u8]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("substrate.wal"))
        .expect("substrate.wal");
    f.write_all(octets).expect("append");
    f.sync_all().expect("sync");
}

fn golden_opcode_futur_tgz() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/v3-mini-opcode-futur.tgz")
}

#[test]
fn un_opcode_wal_inconnu_refuse_l_ouverture_par_le_moteur_et_laisse_le_wal_intact() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("base");
    {
        let db = ObrainDB::open(&dir).unwrap();
        peupler(&db);
        db.close().unwrap();
    }
    let wal = dir.join("substrate.wal");
    let taille_avant = std::fs::metadata(&wal).unwrap().len();
    ajouter_au_wal(&dir, &frame_opcode_futur());
    let wal_forge = std::fs::read(&wal).unwrap();

    let err = match ObrainDB::open(&dir) {
        Ok(_) => {
            panic!("un opcode WAL inconnu doit REFUSER l'ouverture, pas ouvrir une base partielle")
        }
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("0x7E") && err.contains(&taille_avant.to_string()),
        "l'erreur doit nommer l'opcode et l'offset : {err}"
    );
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        wal_forge,
        "le WAL doit rester intact"
    );

    // Sans la frame future, la même base se rouvre entière : rien n'a été
    // perdu ni tronqué par la tentative refusée.
    let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
    f.set_len(taille_avant).unwrap();
    f.sync_all().unwrap();
    let db = ObrainDB::open(&dir).expect("sans la frame future, la base s'ouvre");
    verifier_contenu(&db);
}

/// Fixture `tests/golden/v3-mini-opcode-futur.tgz` : la base v3-mini, close,
/// puis une frame d'opcode 0x7E (CRC valide) ajoutée à `substrate.wal`. Écrite
/// une fois sous `OBRAIN_GOLDEN_WRITE=1`, commitée, et rouverte par ce binaire :
/// le refus doit nommer 0x7E et laisser l'archive extraite intacte.
#[test]
fn la_fixture_v3_avec_un_opcode_futur_est_refusee_en_le_nommant() {
    let tgz = golden_opcode_futur_tgz();
    if std::env::var_os("OBRAIN_GOLDEN_WRITE").is_some() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("v3-mini-opcode-futur");
        let db = ObrainDB::open(&dir).unwrap();
        peupler(&db);
        db.close().unwrap();
        let _ = std::fs::remove_file(dir.join("substrate.dirlock"));
        let _ = std::fs::remove_file(dir.join("substrate.dict.lock"));
        ajouter_au_wal(&dir, &frame_opcode_futur());
        std::fs::create_dir_all(tgz.parent().unwrap()).unwrap();
        tar(
            &["czf", tgz.to_str().unwrap(), "v3-mini-opcode-futur"],
            td.path(),
        );
        eprintln!("fixture écrite dans {}", tgz.display());
    }
    assert!(
        tgz.is_file(),
        "fixture absente : lancer une fois avec OBRAIN_GOLDEN_WRITE=1 puis la commiter ({})",
        tgz.display()
    );

    let td = tempfile::tempdir().unwrap();
    tar(&["xzf", tgz.to_str().unwrap()], td.path());
    let copie = td.path().join("v3-mini-opcode-futur");
    let (magic, version) = version_meta(&copie);
    assert_eq!(&magic[..], MAGIC);
    assert_eq!(
        version, VERSION_FIGEE,
        "la fixture est au format figé v3 : seul le WAL vient du futur"
    );
    let wal = copie.join("substrate.wal");
    let wal_avant = std::fs::read(&wal).unwrap();

    let err = match ObrainDB::open(&copie) {
        Ok(_) => panic!("la fixture à opcode futur doit être REFUSÉE par ce binaire"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("0x7E"),
        "le refus doit nommer l'opcode : {err}"
    );
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        wal_avant,
        "le WAL de la fixture doit rester intact"
    );
}
