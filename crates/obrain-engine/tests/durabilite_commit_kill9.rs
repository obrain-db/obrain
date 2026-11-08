//! D-DUR (v0.0.1, 2026-11-01) : UN COMMIT ACQUITTÉ SURVIT À UN `kill -9`.
//!
//! Ce que ce fichier prouve, et rien d'autre :
//!   1. un processus qui ouvre une base, exécute des écritures, reçoit `Ok`
//!      de `commit()` (implicite ou explicite), puis est tué par SIGKILL sans
//!      `close()` ni `checkpoint()` — à la réouverture, TOUT ce qui a été
//!      acquitté est là ;
//!   2. un processus tué AVANT le commit ne laisse rien derrière lui.
//!
//! MESURÉ sur main e11bdd2a (01/11/2026, macOS) AVANT tout correctif :
//!   - test 1 : VERT. Les records WAL sont `write()` dans le fichier dès la
//!     pose du lot ; le cache de pages du noyau survit au processus. Un
//!     `kill -9` ne perd donc pas un commit acquitté. Ce que `main` ne
//!     garantissait PAS, c'est le `fsync` : contre une coupure de courant ou
//!     un crash du noyau, un commit acquitté pouvait disparaître. C'est ce
//!     que le scellement du commit (D-DUR, `GraphStoreMut::seal_commit`)
//!     ajoute — et ce qu'aucun test en processus ne peut prouver.
//!   - test 2 : ROUGE — 50 nœuds JAMAIS commités réapparaissaient à la
//!     réouverture. Le WAL du substrat est un journal physique appliqué
//!     record par record au rejeu, sans frontière de transaction ; les
//!     marqueurs d'attente (`en_attente_noeuds`) vivent en mémoire et
//!     meurent avec le processus. C'EST le défaut d'intégrité de la v0.0.1.
//!
//! Protocole : le test parent relance SON PROPRE binaire de test sur le cas
//! enfant (`--exact`), avec le répertoire dans une variable d'environnement ;
//! l'enfant écrit, commite, pose un marqueur, puis dort ; le parent attend le
//! marqueur, tue -9, rouvre, compte.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::time::Duration;

const N: i64 = 50;
const ENV_COMMITE: &str = "OBRAIN_TEST_KILL9_COMMITE";
const ENV_SANS_COMMIT: &str = "OBRAIN_TEST_KILL9_SANS_COMMIT";
const ENV_CONCURRENT: &str = "OBRAIN_TEST_KILL9_CONCURRENT";
const THREADS_CONCURRENTS: usize = 4;
const PAR_THREAD: i64 = 50;

fn compte(db: &ObrainDB, label: &str) -> i64 {
    let s = db.session();
    let r = s
        .execute(&format!("MATCH (n:{label}) RETURN count(n)"))
        .expect("count");
    match r.rows.first().and_then(|row| row.first()) {
        Some(Value::Int64(n)) => *n,
        autre => panic!("count inattendu : {autre:?}"),
    }
}

fn dormir_pour_toujours() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn attendre(marqueur: &Path) {
    let mut tours = 0u32;
    while !marqueur.exists() {
        tours += 1;
        assert!(
            tours < 1200,
            "l'enfant n'a jamais posé {}",
            marqueur.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn lancer_enfant(cas: &str, env: &str, dir: &Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args([cas, "--exact", "--nocapture"])
        .env(env, dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant")
}

// ── Cas enfants : ne font rien hors de la variable d'environnement ─────────

#[test]
fn enfant_ecrit_commite_et_dort() {
    let Ok(dir) = std::env::var(ENV_COMMITE) else {
        return;
    };
    let dir = Path::new(&dir);
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    // Moitié en auto-commit (un statement = une transaction implicite),
    // moitié dans une transaction explicite : les deux chemins passent par
    // `commit_inner`, les deux doivent être durables.
    let s = db.session();
    for i in 0..(N / 2) {
        s.execute(&format!("INSERT (:Durable {{k: {i}}})"))
            .expect("insert auto-commit");
    }
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    for i in (N / 2)..N {
        t.execute(&format!("INSERT (:Durable {{k: {i}}})"))
            .expect("insert dans la transaction");
    }
    t.commit().expect("commit explicite");
    // Le marqueur n'est posé QU'APRÈS les `Ok` des commits : tout ce qui
    // précède est acquitté au sens du contrat.
    std::fs::write(dir.join("commite"), b"ok").unwrap();
    dormir_pour_toujours();
}

#[test]
fn enfant_commits_concurrents_et_dort() {
    let Ok(dir) = std::env::var(ENV_CONCURRENT) else {
        return;
    };
    let dir = Path::new(&dir);
    let db = std::sync::Arc::new(ObrainDB::open(dir.join("base")).expect("open enfant"));
    // Auto-commit depuis plusieurs sessions : leurs fsync sont partagés. Chaque
    // `execute` rend la main seulement une fois son commit durable.
    let mut fils = Vec::new();
    for _ in 0..THREADS_CONCURRENTS {
        let db = std::sync::Arc::clone(&db);
        fils.push(std::thread::spawn(move || {
            let s = db.session();
            for i in 0..PAR_THREAD {
                s.execute(&format!("INSERT (:Concurrent {{k: {i}}})"))
                    .expect("insert concurrent");
            }
        }));
    }
    for f in fils {
        f.join().unwrap();
    }
    std::fs::write(dir.join("commite_concurrent"), b"ok").unwrap();
    dormir_pour_toujours();
}

#[test]
fn enfant_ecrit_sans_commiter_et_dort() {
    let Ok(dir) = std::env::var(ENV_SANS_COMMIT) else {
        return;
    };
    let dir = Path::new(&dir);
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    for i in 0..N {
        t.execute(&format!("INSERT (:Fantome {{k: {i}}})"))
            .expect("insert dans la transaction");
    }
    std::fs::write(dir.join("ecrit"), b"ok").unwrap();
    dormir_pour_toujours();
}

// ── Les deux vrais tests ───────────────────────────────────────────────────

#[test]
fn un_commit_acquitte_survit_a_un_kill_9() {
    let td = tempfile::tempdir().unwrap();
    let mut enfant = lancer_enfant("enfant_ecrit_commite_et_dort", ENV_COMMITE, td.path());
    attendre(&td.path().join("commite"));
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    let db = ObrainDB::open(td.path().join("base")).expect("réouverture après kill");
    assert_eq!(
        compte(&db, "Durable"),
        N,
        "des commits acquittés ont disparu après kill -9 : contrat D-DUR violé"
    );
}

#[test]
fn commits_concurrents_acquittes_survivent_a_un_kill_9() {
    let td = tempfile::tempdir().unwrap();
    let mut enfant = lancer_enfant(
        "enfant_commits_concurrents_et_dort",
        ENV_CONCURRENT,
        td.path(),
    );
    attendre(&td.path().join("commite_concurrent"));
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    let db = ObrainDB::open(td.path().join("base")).expect("réouverture après kill");
    assert_eq!(
        compte(&db, "Concurrent"),
        THREADS_CONCURRENTS as i64 * PAR_THREAD,
        "des commits concurrents acquittés ont disparu après kill -9"
    );
}

#[test]
fn un_commit_jamais_acquitte_ne_laisse_rien() {
    let td = tempfile::tempdir().unwrap();
    let mut enfant = lancer_enfant(
        "enfant_ecrit_sans_commiter_et_dort",
        ENV_SANS_COMMIT,
        td.path(),
    );
    attendre(&td.path().join("ecrit"));
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    let db = ObrainDB::open(td.path().join("base")).expect("réouverture après kill");
    assert_eq!(
        compte(&db, "Fantome"),
        0,
        "des écritures jamais commitées sont réapparues après kill -9"
    );
}
