//! obrain#172 — une valeur de plus de 256 o ACQUITTÉE (après `flush_wal` +
//! `commit_wal`, le flush complet) survit à un `kill -9`, y compris sur un
//! nœud à DEUX propriétés blob dont l'une est ensuite remplacée par une valeur
//! courte (blob → inline), et sur plusieurs tués successifs.
//!
//! Reprend la séquence du backend PO (`ecrire_fichiers`) : création du nœud,
//! `path` (344 o) et `hash` (336 o) en blob, flush complet, acquittement ;
//! puis mise à jour de `hash` par une valeur courte, flush complet,
//! acquittement. Le parent tue -9 l'enfant à des instants variés, rouvre, et
//! vérifie que chaque `path` porte le dernier `hash` acquitté.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const ENV_ENFANT: &str = "OBRAIN_TEST_BLOB_DEUX_PROPS_ENFANT";
const TOURS: u64 = 8;

fn durable(db: &ObrainDB) {
    db.flush_wal().expect("flush_wal");
    db.commit_wal().expect("commit_wal");
}

#[test]
fn enfant_ecrivain_fichiers() {
    let Ok(dir) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let dir = Path::new(&dir);
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("acquittes"))
        .unwrap();
    use std::io::Write as _;
    let pid = std::process::id();
    let mut i = 0u64;
    loop {
        i += 1;
        let path = format!("src/{}/{pid}-{i}.rs", "d".repeat(300));
        let long = format!("{}{pid}-{i}", "h".repeat(300));
        let id = db.create_node(&["File"]);
        db.set_node_property(id, "brouillon", Value::from(true));
        db.set_node_property(id, "path", Value::String(path.clone().into()));
        db.set_node_property(id, "hash", Value::String(long.clone().into()));
        durable(&db);
        db.remove_node_property(id, "brouillon");
        durable(&db);
        writeln!(log, "{path}\t{long}").unwrap();

        let court = format!("court-{pid}-{i}");
        db.set_node_property(id, "path", Value::String(path.clone().into()));
        db.set_node_property(id, "hash", Value::String(court.clone().into()));
        durable(&db);
        writeln!(log, "{path}\t{court}").unwrap();
    }
}

#[test]
fn une_valeur_blob_acquittee_survit_a_des_kill_9_successifs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let acquittes = dir.path().join("acquittes");
    let compte = || std::fs::read_to_string(&acquittes).map_or(0, |s| s.lines().count());
    for tour in 0..TOURS {
        let avant = compte();
        let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["enfant_ecrivain_fichiers", "--exact", "--nocapture"])
            .env(ENV_ENFANT, dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn enfant");
        for _ in 0..2400 {
            if compte() >= avant + 6 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(compte() >= avant + 6, "tour {tour}: aucun acquittement");
        std::thread::sleep(Duration::from_micros(1_500 * (tour % 9 + 1)));
        enfant.kill().expect("kill -9");
        let _ = enfant.wait();

        let db = ObrainDB::open(&base).expect("réouverture");
        let mut derniers: HashMap<String, String> = HashMap::new();
        for ligne in std::fs::read_to_string(&acquittes).unwrap().lines() {
            let (p, h) = ligne.split_once('\t').unwrap();
            derniers.insert(p.to_string(), h.to_string());
        }
        let r = db
            .session()
            .execute("MATCH (f:File) RETURN f.path, f.hash")
            .expect("relecture");
        let mut relus: HashMap<String, Option<String>> = HashMap::new();
        for l in &r.rows {
            if let Some(Value::String(p)) = l.first() {
                let h = match l.get(1) {
                    Some(Value::String(h)) => Some(h.to_string()),
                    _ => None,
                };
                relus.insert(p.to_string(), h);
            }
        }
        for (p, h) in &derniers {
            let lu = relus.get(p);
            assert_eq!(
                lu.map(|o| o.as_deref()),
                Some(Some(h.as_str())),
                "tour {tour}: dernier hash acquitté de {}…{} perdu",
                &p[..8],
                &p[p.len().saturating_sub(12)..]
            );
        }
        drop(db);
    }
}
