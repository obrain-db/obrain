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
