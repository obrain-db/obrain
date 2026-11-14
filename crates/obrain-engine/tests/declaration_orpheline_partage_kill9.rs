//! obrain#171 — DÉCLARATION DE VALEUR D'AVANT ORPHELINE, OUVERTURE PARTAGÉE.
//!
//! Scénario à TROIS processus :
//! - A (enfant, écrivain EXCLUSIF) commite `court = 'ancien'`, puis lance un
//!   `SET court = 'nouveau'` dans une transaction jamais commitée, fait un
//!   commit d'une AUTRE session (son `fsync` rend la déclaration durable),
//!   pose le marqueur `PRET`, et dort. Le parent le tue par SIGKILL.
//! - B (le parent, écrivain PARTAGÉ `open_shared_write`) constate :
//!   1. la valeur relue : doit être la valeur COMMITÉE (`ancien`) ;
//!   2. le checkpoint du substrat : doit aboutir et tronquer le WAL.
//! - C (le parent, à nouveau exclusif) : la valeur doit être `ancien`, et un
//!   checkpoint exclusif doit tronquer le WAL.
//!
//! Oracle : les VALEURS sont celles d'avant la transaction, pour B comme pour
//! C. Les effets opérationnels (refus du checkpoint, taille du WAL) sont
//! MESURÉS et affichés.
//!
//! Harnais : le binaire de test se relance en enfant (variable d'environnement),
//! comme `atomicite_proprietes_tx_kill9.rs`.
//!
//! Tests (rouge sur `main` 11877430, vert avec le correctif) :
//! - `le_pair_partage_lit_la_valeur_commitee_et_peut_checkpointer` : la valeur
//!   relue et le checkpoint du pair partagé (valeur d'avant, WAL tronqué) ;
//! - `partage_refuse_de_rejoindre_un_exclusif_vivant_du_meme_processus` ;
//! - `pair_partage_vivant_n_est_jamais_annule` : garde-fou, vert aussi sur
//!   `main` (un partagé ne journalise aucune déclaration, donc n'a rien à
//!   défaire chez un pair vivant).
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_substrate::error::SubstrateError;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ENV_ENFANT: &str = "OBRAIN_TEST_DECL_ORPHELINE_ENFANT";

fn taille_wal(base: &Path) -> u64 {
    std::fs::metadata(base.join("substrate.wal")).map_or(0, |m| m.len())
}

fn lire_court(db: &ObrainDB) -> Option<String> {
    let r = db
        .session()
        .execute("MATCH (n:Cible) RETURN n.court")
        .expect("relecture");
    assert_eq!(r.rows.len(), 1, "exactement un :Cible : {:?}", r.rows);
    match r.rows[0].first() {
        Some(Value::String(s)) => Some(s.as_str().to_string()),
        None | Some(Value::Null) => None,
        autre => panic!("valeur inattendue : {autre:?}"),
    }
}

/// Enfant A : écrivain exclusif, transaction orpheline, marqueur, puis attente
/// du SIGKILL.
#[test]
fn enfant_ecrivain_exclusif() {
    let Ok(dir) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let db = ObrainDB::open(Path::new(&dir).join("base")).expect("open exclusif de A");
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    t.execute("MATCH (n:Cible) SET n.court = 'nouveau'")
        .expect("SET dans la transaction");
    // Commit d'une AUTRE session : son fsync rend durables les trames non
    // commitées de `t`, qui le précèdent dans le WAL.
    db.session()
        .execute("INSERT (:Temoin {k: 1})")
        .expect("commit tiers");
    std::fs::write(Path::new(&dir).join("pret"), b"ok").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn enfant_tue(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("pret"));
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["enfant_ecrivain_exclusif", "--exact", "--nocapture"])
        .env(ENV_ENFANT, dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut enfant = cmd.spawn().expect("spawn enfant");
    let mut tours = 0u32;
    while !dir.join("pret").exists() {
        if let Some(s) = enfant.try_wait().expect("try_wait") {
            panic!("l'enfant est mort avant le marqueur : {s:?}");
        }
        tours += 1;
        assert!(tours < 1200, "l'enfant n'a jamais posé le marqueur");
        std::thread::sleep(Duration::from_millis(50));
    }
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();
}

/// Crée `dir/base` avec un `:Cible {court: 'ancien'}` commité.
fn preparer_dans(dir: &Path) -> PathBuf {
    let base = dir.join("base");
    let db = ObrainDB::open(&base).expect("open initial");
    db.session()
        .execute("INSERT (:Cible {court: 'ancien'})")
        .expect("insert");
    db.close().expect("close initial");
    base
}

fn preparer() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = preparer_dans(dir.path());
    (dir, base)
}

/// Nombre de trames `TxAnnulee` du WAL : une par transaction orpheline fermée.
fn compter_annulations(base: &Path) -> usize {
    let reader = obrain_substrate::wal_io::WalReader::open(base.join("substrate.wal"))
        .expect("lecture du WAL");
    reader
        .iter_from(0)
        .filter_map(Result::ok)
        .filter(|(rec, _, _)| matches!(rec.payload, obrain_substrate::WalPayload::TxAnnulee { .. }))
        .count()
}

/// Les deux effets, mesurés et affichés pour le rapport.
#[test]
fn le_pair_partage_lit_la_valeur_commitee_et_peut_checkpointer() {
    let (dir, base) = preparer();
    enfant_tue(dir.path());

    let avant = taille_wal(&base);
    let db = ObrainDB::open_shared_write(&base).expect("open_shared_write du pair");
    let court_partage = lire_court(&db);
    let store = db.substrate_handle().expect("store substrat");
    let resultat = store.checkpoint();
    let apres = taille_wal(&base);
    eprintln!("MESURE partage : court lu = {court_partage:?}");
    eprintln!("MESURE partage : checkpoint = {resultat:?}");
    eprintln!("MESURE partage : WAL {avant} -> {apres} octets");
    drop(store);
    db.close().expect("close du pair");

    // Oracle 1 : la valeur relue par un pair est la valeur COMMITÉE.
    assert_eq!(
        court_partage.as_deref(),
        Some("ancien"),
        "le pair partagé relit une valeur jamais commitée"
    );
    // Oracle 2 : le checkpoint du pair aboutit (la déclaration est orpheline
    // d'un processus mort, elle ne doit pas bloquer la troncature).
    assert!(
        !matches!(resultat, Err(SubstrateError::CheckpointDiffere { .. })),
        "checkpoint refusé par une déclaration orpheline : {resultat:?}"
    );
    assert!(resultat.is_ok(), "checkpoint en erreur : {resultat:?}");
    // Oracle 3 : la troncature a eu lieu (le WAL ne grossit plus).
    assert!(
        apres < avant,
        "le WAL n'a pas été tronqué : {avant} -> {apres} octets"
    );
}

/// Après la réouverture exclusive, la valeur est restaurée et le checkpoint
/// exclusif tronque le WAL (témoin : ce chemin marche sur main).
#[test]
fn la_reouverture_exclusive_restaure_et_tronque() {
    let (dir, base) = preparer();
    enfant_tue(dir.path());
    let db = ObrainDB::open(&base).expect("open exclusif du parent");
    assert_eq!(lire_court(&db).as_deref(), Some("ancien"));
    db.close().expect("close");
    let db = ObrainDB::open(&base).expect("reouverture");
    assert_eq!(lire_court(&db).as_deref(), Some("ancien"));
    db.close().expect("close");
}

const ENV_PAIR: &str = "OBRAIN_TEST_PAIR_VIVANT_ENFANT";

/// Enfant : un SECOND pair partagé, vivant, qui ouvre, commite, ferme.
#[test]
fn enfant_pair_partage_vivant() {
    let Ok(dir) = std::env::var(ENV_PAIR) else {
        return;
    };
    let base = Path::new(&dir).join("base");
    let db = ObrainDB::open_shared_write(&base).expect("open_shared_write de l'enfant");
    db.session()
        .execute("INSERT (:Temoin {k: 2})")
        .expect("commit de l'enfant");
    db.close().expect("close de l'enfant");
    std::fs::write(Path::new(&dir).join("fini"), b"ok").unwrap();
}

/// Un pair partagé VIVANT, en pleine transaction, n'est jamais touché : un
/// second pair partagé ouvre, écrit, ferme. La transaction du premier aboutit
/// et son commit est visible à la réouverture exclusive.
#[test]
fn pair_partage_vivant_n_est_jamais_annule() {
    let (dir, base) = preparer();
    let vivant = ObrainDB::open_shared_write(&base).expect("pair vivant");
    let mut t = vivant.session();
    t.begin_transaction().expect("begin du pair vivant");
    t.execute("MATCH (n:Cible) SET n.court = 'vivant'")
        .expect("SET en vol du pair vivant");

    let _ = std::fs::remove_file(dir.path().join("fini"));
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["enfant_pair_partage_vivant", "--exact", "--nocapture"])
        .env(ENV_PAIR, dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut enfant = cmd.spawn().expect("spawn enfant");
    let status = enfant.wait().expect("attente enfant");
    assert!(status.success(), "le second pair a échoué : {status:?}");
    assert!(
        dir.path().join("fini").exists(),
        "marqueur de l'enfant absent"
    );

    // Oracle : la transaction vivante voit encore sa propre écriture.
    let en_vol = t
        .execute("MATCH (n:Cible) RETURN n.court")
        .expect("relecture en vol");
    assert_eq!(
        en_vol.rows[0].first(),
        Some(&Value::String("vivant".into())),
        "la transaction vivante a été annulée par un pair"
    );
    t.commit().expect("commit du pair vivant");
    drop(t);
    vivant.close().expect("close du pair vivant");
    drop(vivant);

    let db = ObrainDB::open(&base).expect("reouverture exclusive");
    assert_eq!(lire_court(&db).as_deref(), Some("vivant"));
    db.close().expect("close");
}

/// Un écrivain partagé ne rejoint pas, dans le même processus, un écrivain
/// exclusif vivant (sinon il lirait ses écritures en vol, et pourrait
/// restaurer ses déclarations). Le refus est la règle symétrique de
/// l'exclusif contre une part partagée.
#[test]
fn partage_refuse_de_rejoindre_un_exclusif_vivant_du_meme_processus() {
    let (_dir, base) = preparer();
    let excl = ObrainDB::open(&base).expect("exclusif");
    let refus = ObrainDB::open_shared_write(&base);
    assert!(
        refus.is_err(),
        "un écrivain partagé a rejoint un exclusif vivant du même processus"
    );
    excl.close().expect("close");
}

const ENV_FIXTURE_ECRIT: &str = "OBRAIN_TEST_FIXTURE_ECRITURE";
const ENV_FIXTURE_LIT: &str = "OBRAIN_TEST_FIXTURE_LECTURE";

/// Rétro-compatibilité, côté ÉCRITURE : produit, avec le binaire courant, deux
/// bases à rejouer par un AUTRE binaire. Inactif sans la variable d'environnement.
/// `orpheline/base` : un exclusif tué pendant un SET ; `propre/base` : un SET commité
/// puis fermé proprement.
#[test]
fn fixture_ecrit_par_le_binaire_courant() {
    let Ok(racine) = std::env::var(ENV_FIXTURE_ECRIT) else {
        return;
    };
    let racine = PathBuf::from(racine);
    let orph = racine.join("orpheline");
    std::fs::create_dir_all(&orph).unwrap();
    preparer_dans(&orph);
    enfant_tue(&orph);

    let propre = racine.join("propre");
    std::fs::create_dir_all(&propre).unwrap();
    let db = ObrainDB::open(propre.join("base")).expect("open propre");
    db.session()
        .execute("INSERT (:Cible {court: 'ancien'})")
        .expect("insert");
    db.session()
        .execute("MATCH (n:Cible) SET n.court = 'nouveau'")
        .expect("set commité");
    db.close().expect("close propre");
}

/// Rétro-compatibilité, côté LECTURE : rejoue, avec le binaire courant, les
/// bases produites par `fixture_ecrit_par_le_binaire_courant` (copies de travail
/// fournies par le harnais). Attendu : `propre` = `nouveau` (commit) et
/// `orpheline` = `ancien`. Inactif sans la variable d'environnement.
#[test]
fn fixture_relue_par_le_binaire_courant() {
    let Ok(racine) = std::env::var(ENV_FIXTURE_LIT) else {
        return;
    };
    let racine = PathBuf::from(racine);

    let db = ObrainDB::open_shared_write(racine.join("propre/base")).expect("open propre");
    let propre = lire_court(&db);
    db.close().expect("close propre");
    drop(db);

    let base = racine.join("orpheline/base");
    let db = ObrainDB::open_shared_write(&base).expect("open orpheline");
    let orpheline = lire_court(&db);
    db.close().expect("close orpheline");
    drop(db);
    let db = ObrainDB::open(&base).expect("reouverture exclusive");
    let apres = lire_court(&db);
    db.close().expect("close");

    eprintln!("RESULTAT propre={propre:?} orpheline={orpheline:?} apres_exclusif={apres:?}");
    assert_eq!(propre.as_deref(), Some("nouveau"), "base propre");
    assert_eq!(
        orpheline.as_deref(),
        Some("ancien"),
        "base orpheline (partagé)"
    );
    assert_eq!(
        apres.as_deref(),
        Some("ancien"),
        "base orpheline (exclusif)"
    );
}

const ENV_COURSE: &str = "OBRAIN_TEST_COURSE_ENFANT";
const NB_COURSE: usize = 6;

/// Enfant de la course : attend le signal, ouvre en partagé, ferme.
#[test]
fn enfant_ouvre_partage_au_signal() {
    let Ok(dir) = std::env::var(ENV_COURSE) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let depart = std::time::Instant::now();
    while !dir.join("go").exists() {
        assert!(depart.elapsed() < Duration::from_secs(30), "pas de signal");
        std::thread::sleep(Duration::from_millis(1));
    }
    let db = ObrainDB::open_shared_write(dir.join("base")).expect("open partagé de l'enfant");
    db.close().expect("close de l'enfant");
    drop(db);
    std::fs::write(dir.join(format!("fini-{}", std::process::id())), b"ok").unwrap();
}

/// Course : six écrivains partagés ouvrent ENSEMBLE une base dont une
/// déclaration orpheline attend. Une seule restauration doit avoir lieu : une
/// seule trame `TxAnnulee` dans le WAL, et la valeur d'avant. Sans le verrou de
/// restauration, deux partagés peuvent restaurer et clore chacun la même orpheline.
#[test]
fn course_entre_partages_restaure_une_seule_fois() {
    let (dir, base) = preparer();
    enfant_tue(dir.path());
    let mut enfants = Vec::new();
    for _ in 0..NB_COURSE {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["enfant_ouvre_partage_au_signal", "--exact", "--nocapture"])
            .env(ENV_COURSE, dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        enfants.push(cmd.spawn().expect("spawn enfant de course"));
    }
    std::thread::sleep(Duration::from_millis(1500));
    std::fs::write(dir.path().join("go"), b"1").unwrap();
    for mut e in enfants {
        let statut = e.wait().expect("attente enfant");
        assert!(
            statut.success(),
            "un partagé de la course a échoué : {statut:?}"
        );
    }
    let fini = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.file_name().to_string_lossy().starts_with("fini-"))
        })
        .count();
    assert_eq!(fini, NB_COURSE, "tous les partagés doivent avoir fini");

    let annulations = compter_annulations(&base);
    eprintln!("MESURE course : TxAnnulee = {annulations} sur {NB_COURSE} partagés");
    assert_eq!(
        annulations, 1,
        "l'orpheline a été restaurée et close {annulations} fois au lieu d'une"
    );
    let db = ObrainDB::open(&base).expect("reouverture exclusive");
    assert_eq!(lire_court(&db).as_deref(), Some("ancien"));
    db.close().expect("close");
}
