//! D-DUR (v0.0.1, 2026-11-01) : UN COMMIT ACQUITTÉ SURVIT À UN `kill -9`.
//!
//! Ce que ce fichier prouve, et rien d'autre :
//!   1. un processus qui ouvre une base, exécute des écritures, reçoit `Ok`
//!      de `commit()` (implicite ou explicite), puis est tué par SIGKILL sans
//!      `close()` ni `checkpoint()` — à la réouverture, TOUT ce qui a été
//!      acquitté est là ;
//!   2. un processus tué AVANT le commit ne laisse rien derrière lui ;
//!   3. (2026-11-08, DUR-ORACLE) au retour de CHAQUE `commit()` acquitté, la
//!      ligne durable du WAL (`ObrainDB::wal_octets_durables`, l'offset de fin
//!      couvert par un `fsync` terminé) couvre la fin de la frame du commit ;
//!   4. (2026-11-08) une COUPURE SIMULÉE — le WAL tronqué à la ligne durable
//!      relevée après le dernier ack, tout ce qui n'était pas fsyncé perdu —
//!      laisse exactement les commits acquittés.
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
//! MESURÉ le 08/11/2026 (branche perf/fsync-partage, macOS), fsync NEUTRALISÉ
//! dans `wal_io.rs` (`should_fsync => false`, chef de `attendre_durable`
//! rendu sans rien synchroniser) : les tests 1 et 2 et le cas concurrent
//! restaient VERTS (6/6). Un `kill -9` tue le processus, pas le noyau : ces
//! tests prouvent l'ordre write→ack et le rejeu, PAS la durabilité face à une
//! coupure. D'où l'oracle (3) — ROUGE sous le même patch (`durable=0`) — et la
//! coupure simulée (4), qui est la seule façon, en processus, de perdre ce qui
//! n'a pas été fsyncé.
//!
//! Protocole : le test parent relance SON PROPRE binaire de test sur le cas
//! enfant (`--exact`), avec le répertoire dans une variable d'environnement ;
//! l'enfant écrit, commite, vérifie l'oracle après chaque ack (une violation
//! est écrite dans `violation`, jamais avalée), pose un marqueur, puis dort ;
//! le parent attend le marqueur, tue -9, rouvre, compte.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::time::Duration;

const N: i64 = 50;
const ENV_COMMITE: &str = "OBRAIN_TEST_KILL9_COMMITE";
const ENV_SANS_COMMIT: &str = "OBRAIN_TEST_KILL9_SANS_COMMIT";
const ENV_CONCURRENT: &str = "OBRAIN_TEST_KILL9_CONCURRENT";
const ENV_COUPURE: &str = "OBRAIN_TEST_COUPURE_SIMULEE";
const THREADS_CONCURRENTS: usize = 4;
const PAR_THREAD: i64 = 50;
/// Commits acquittés par l'enfant de la coupure simulée (un par statement).
const K_COUPURE: i64 = 20;
/// Écritures JAMAIS commitées posées après le dernier ack (elles sont `write()`
/// dans le WAL mais au-delà de la ligne durable : une coupure les perd).
const M_FANTOMES: i64 = 10;

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

// ── L'oracle de durabilité ─────────────────────────────────────────────────
//
// Un `kill -9` laisse le cache de pages intact : la réouverture ne distingue
// pas un commit `write()` d'un commit `fsync()`. La seule mesure possible en
// processus est la ligne durable du WAL, publiée par l'écrivain APRÈS le
// retour du fsync qui la couvre. L'enfant étant seul à écrire, la fin du
// fichier `substrate.wal` au retour de `commit()` EST la fin de la frame du
// commit : la ligne durable doit la couvrir.
//
// Une violation est ÉCRITE (fichier `violation`) plutôt que `panic!` : le
// parent la lit juste après le marqueur, et le verdict est immédiat au lieu
// d'un délai d'attente de 60 s sur un marqueur jamais posé.

fn taille_wal(base: &Path) -> u64 {
    std::fs::metadata(base.join("substrate.wal"))
        .expect("substrate.wal")
        .len()
}

fn signaler_violation(dir: &Path, message: String) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("violation"))
        .unwrap();
    writeln!(f, "{message}").unwrap();
}

/// Après un `commit()` acquitté par un écrivain SEUL : la ligne durable couvre
/// la fin du WAL, donc la fin de la frame du commit. Rend la ligne durable.
fn oracle_apres_ack(db: &ObrainDB, base: &Path, dir: &Path, quoi: &str) -> u64 {
    let durable = db.wal_octets_durables().expect("base persistante");
    let fin = taille_wal(base);
    if durable < fin {
        signaler_violation(
            dir,
            format!(
                "{quoi} : commit acquitté AVANT son fsync — ligne durable={durable} < fin du \
                 WAL={fin} (fsyncs={})",
                db.wal_fsyncs().unwrap_or(0)
            ),
        );
    }
    durable
}

/// Après un `commit()` acquitté par un fil parmi d'autres : la fin de NOTRE
/// frame n'est pas connue, mais aucun fsync commencé avant notre ack précédent
/// ne pouvait la couvrir — le compteur doit donc avoir avancé.
fn oracle_concurrent_apres_ack(db: &ObrainDB, dir: &Path, fsyncs_avant: u64, quoi: &str) -> u64 {
    let fsyncs = db.wal_fsyncs().expect("base persistante");
    if fsyncs <= fsyncs_avant {
        signaler_violation(
            dir,
            format!(
                "{quoi} : commit acquitté sans fsync nouveau — fsyncs={fsyncs}, déjà {fsyncs_avant} \
                 à l'ack précédent"
            ),
        );
    }
    fsyncs
}

fn assert_aucune_violation(dir: &Path) {
    if let Ok(texte) = std::fs::read_to_string(dir.join("violation")) {
        panic!("oracle de durabilité violé dans l'enfant :\n{texte}");
    }
}

/// Écriture atomique (fichier temporaire + rename) : le parent ne lit jamais
/// un fichier à moitié écrit.
fn ecrire_atomique(chemin: &Path, contenu: &str) {
    let tmp = chemin.with_extension("tmp");
    std::fs::write(&tmp, contenu).unwrap();
    std::fs::rename(&tmp, chemin).unwrap();
}

// ── Cas enfants : ne font rien hors de la variable d'environnement ─────────

#[test]
fn enfant_ecrit_commite_et_dort() {
    let Ok(dir) = std::env::var(ENV_COMMITE) else {
        return;
    };
    let dir = Path::new(&dir);
    let base = dir.join("base");
    let db = ObrainDB::open(&base).expect("open enfant");
    // Moitié en auto-commit (un statement = une transaction implicite),
    // moitié dans une transaction explicite : les deux chemins passent par
    // `commit_inner`, les deux doivent être durables.
    let s = db.session();
    for i in 0..(N / 2) {
        s.execute(&format!("INSERT (:Durable {{k: {i}}})"))
            .expect("insert auto-commit");
        oracle_apres_ack(&db, &base, dir, &format!("auto-commit k={i}"));
    }
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    for i in (N / 2)..N {
        t.execute(&format!("INSERT (:Durable {{k: {i}}})"))
            .expect("insert dans la transaction");
    }
    t.commit().expect("commit explicite");
    oracle_apres_ack(&db, &base, dir, "commit explicite");
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
    let base = dir.join("base");
    let db = std::sync::Arc::new(ObrainDB::open(&base).expect("open enfant"));
    // Auto-commit depuis plusieurs sessions : leurs fsync sont partagés. Chaque
    // `execute` rend la main seulement une fois son commit durable.
    let mut fils = Vec::new();
    for t in 0..THREADS_CONCURRENTS {
        let db = std::sync::Arc::clone(&db);
        let dir = dir.to_path_buf();
        fils.push(std::thread::spawn(move || {
            let s = db.session();
            let mut fsyncs = db.wal_fsyncs().expect("base persistante");
            for i in 0..PAR_THREAD {
                s.execute(&format!("INSERT (:Concurrent {{k: {i}}})"))
                    .expect("insert concurrent");
                fsyncs = oracle_concurrent_apres_ack(&db, &dir, fsyncs, &format!("fil {t} k={i}"));
            }
        }));
    }
    for f in fils {
        f.join().unwrap();
    }
    // Plus personne n'écrit : tout ce qui a été acquitté est sous la ligne.
    oracle_apres_ack(&db, &base, dir, "après le dernier commit concurrent");
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

/// Enfant de la coupure simulée : K commits acquittés, et après CHAQUE ack la
/// ligne durable est publiée au parent dans `durable.txt` (une ligne
/// « k octets » par ack, fichier réécrit atomiquement) ; puis M écritures dans
/// une transaction jamais commitée, qui gonflent le WAL au-delà de la ligne.
#[test]
fn enfant_coupure_commits_acquittes_puis_fantomes() {
    let Ok(dir) = std::env::var(ENV_COUPURE) else {
        return;
    };
    let dir = Path::new(&dir);
    let base = dir.join("base");
    let db = ObrainDB::open(&base).expect("open enfant");
    let s = db.session();
    let mut lignes = String::new();
    for k in 1..=K_COUPURE {
        s.execute(&format!("INSERT (:Durable {{k: {k}}})"))
            .expect("insert auto-commit");
        let durable = oracle_apres_ack(&db, &base, dir, &format!("coupure k={k}"));
        eprintln!("ACK {k} durable={durable}");
        {
            use std::fmt::Write as _;
            writeln!(lignes, "{k} {durable}").unwrap();
        }
        ecrire_atomique(&dir.join("durable.txt"), &lignes);
    }
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    for i in 0..M_FANTOMES {
        t.execute(&format!("INSERT (:Fantome {{k: {i}}})"))
            .expect("insert jamais commité");
    }
    std::fs::write(dir.join("pret"), b"ok").unwrap();
    dormir_pour_toujours();
}

// ── Les vrais tests ────────────────────────────────────────────────────────

#[test]
fn un_commit_acquitte_survit_a_un_kill_9() {
    let td = tempfile::tempdir().unwrap();
    let mut enfant = lancer_enfant("enfant_ecrit_commite_et_dort", ENV_COMMITE, td.path());
    attendre(&td.path().join("commite"));
    assert_aucune_violation(td.path());
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
    assert_aucune_violation(td.path());
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

/// Lit `durable.txt` : les lignes « k octets » dans l'ordre des acks.
fn lire_durable(dir: &Path) -> Vec<(i64, u64)> {
    std::fs::read_to_string(dir.join("durable.txt"))
        .expect("durable.txt")
        .lines()
        .map(|l| {
            let mut it = l.split_whitespace();
            (
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

fn tronquer(wal: &Path, a: u64) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(wal)
        .unwrap()
        .set_len(a)
        .unwrap();
}

/// Copie plate d'un répertoire de base (les fichiers `substrate.*`), après la
/// mort de l'écrivain : l'image que laisserait le cache de pages.
fn copier_repertoire(de: &Path, vers: &Path) {
    std::fs::create_dir_all(vers).unwrap();
    for entree in std::fs::read_dir(de).unwrap() {
        let entree = entree.unwrap();
        if entree.file_type().unwrap().is_file() {
            std::fs::copy(entree.path(), vers.join(entree.file_name())).unwrap();
        }
    }
}

/// COUPURE SIMULÉE. Un `kill -9` laisse le cache de pages ; une coupure de
/// courant ne laisse que ce qui a été fsyncé. On la simule en tronquant le
/// WAL à la ligne durable publiée par l'enfant après son dernier ack : tout
/// ce qui était `write()` au-delà (les écritures jamais commitées) disparaît,
/// comme il disparaîtrait d'un disque coupé.
///
/// Ce qui est prouvé : les K commits acquittés sont TOUS sous la ligne (ni
/// moins — c'est la durabilité), et les écritures jamais commitées ne
/// réapparaissent pas (ni plus). La troncature tombe sur une frontière de
/// frame (la ligne durable est toujours une fin de frame) : le rejeu ne voit
/// pas de queue déchirée, seulement un WAL plus court.
#[test]
fn une_coupure_simulee_ne_garde_que_les_commits_durables() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("base");
    let mut enfant = lancer_enfant(
        "enfant_coupure_commits_acquittes_puis_fantomes",
        ENV_COUPURE,
        td.path(),
    );
    attendre(&td.path().join("pret"));
    assert_aucune_violation(td.path());
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    let acks = lire_durable(td.path());
    assert_eq!(acks.len() as i64, K_COUPURE, "un ack par commit");
    let (dernier_k, ligne_durable) = *acks.last().unwrap();
    assert_eq!(dernier_k, K_COUPURE);
    let taille_avant = taille_wal(&base);
    eprintln!(
        "coupure simulée : WAL de {taille_avant} octets, ligne durable {ligne_durable} \
         ({} octets jamais fsyncés perdus)",
        taille_avant - ligne_durable
    );
    assert!(
        taille_avant > ligne_durable,
        "les écritures jamais commitées devraient dépasser la ligne durable"
    );

    // Seconde coupure, plus tôt : sur une COPIE de la base, le WAL est tronqué
    // à la ligne durable relevée après l'ack du commit K/2. Les K/2 premiers
    // commits doivent revenir (durabilité).
    //
    // ── CE QUE CETTE TRONCATURE NE SIMULE PAS (MESURÉ le 08/11/2026) ──────
    // Les K/2 commits suivants reviennent AUSSI (20 présents pour 10
    // acquittés avant la ligne) : leurs frames WAL sont tronquées, mais leurs
    // slots sont dans les zones mmap, que le cache de pages a gardées après
    // le kill. Tronquer le WAL ne rejoue donc la coupure que pour le WAL, pas
    // pour les zones — « ni plus » n'est prouvable ici que pour ce qui n'a
    // JAMAIS été scellé (voir la coupure finale, `Fantome`). Une coupure
    // réelle qui garderait des pages de zones sans leurs frames WAL n'est
    // couverte par aucun test de ce fichier.
    let base_mi = td.path().join("base_mi");
    copier_repertoire(&base, &base_mi);
    let (k_mi, ligne_mi) = acks[(K_COUPURE / 2 - 1) as usize];
    assert_eq!(k_mi, K_COUPURE / 2);
    tronquer(&base_mi.join("substrate.wal"), ligne_mi);
    let db_mi = ObrainDB::open(&base_mi).expect("réouverture après coupure à mi-chemin");
    let presents_mi = compte(&db_mi, "Durable");
    eprintln!("coupure après l'ack {k_mi} (ligne {ligne_mi}) : {presents_mi} commits présents");
    assert!(
        presents_mi >= k_mi,
        "coupure après l'ack {k_mi} (ligne {ligne_mi}) : {presents_mi} commits présents, des \
         commits acquittés avant la coupure manquent"
    );
    assert_eq!(compte(&db_mi, "Fantome"), 0);
    drop(db_mi);

    // La coupure après le dernier ack : tout ce qui n'est pas fsyncé est perdu.
    tronquer(&base.join("substrate.wal"), ligne_durable);

    let db = ObrainDB::open(&base).expect("réouverture après coupure simulée");
    assert_eq!(
        compte(&db, "Durable"),
        K_COUPURE,
        "des commits acquittés manquent après troncature à la ligne durable : \
         un ack a précédé son fsync"
    );
    assert_eq!(
        compte(&db, "Fantome"),
        0,
        "des écritures jamais commitées sont réapparues après la coupure simulée"
    );
}
