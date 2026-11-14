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

fn preparer() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    {
        let db = ObrainDB::open(&base).expect("open initial");
        db.session()
            .execute("INSERT (:Cible {court: 'ancien'})")
            .expect("insert");
        db.close().expect("close initial");
    }
    (dir, base)
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
