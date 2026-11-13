//! INDEX DE LABELS (obrain#120, point 5) APRÈS `kill -9`.
//!
//! Sur le modèle de `durabilite_commit_kill9.rs` : le parent relance SON
//! binaire de test sur le cas enfant ; l'enfant ouvre une base, exécute une
//! suite déterministe de commits acquittés qui changent des labels (création
//! multi-labels, `SET n:L`, `REMOVE n:L`, `DELETE`, une transaction annulée, un
//! label introduit par `SET` seulement), INTERROGE les labels en cours de route
//! (l'index est donc construit et nourri par les crochets dans l'enfant), écrit
//! l'état attendu, ouvre une transaction jamais commitée, pose un marqueur et
//! dort. Le parent le tue par SIGKILL, rouvre, et exige :
//!
//!   1. pour CHAQUE label (y compris celui de la transaction jamais commitée),
//!      `nodes_by_label` (index) == `nodes_by_label_balayage` (zone) ;
//!   2. pour les labels des seuls commits acquittés, l'ensemble des `k` est
//!      celui que l'enfant a acquitté.
//!
//!   cargo test -p obrain-engine --test index_labels_kill9
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStore;
use obrain_engine::ObrainDB;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

const ENV: &str = "OBRAIN_TEST_INDEX_LABELS_KILL9";
const N: i64 = 120;
/// Labels touchés UNIQUEMENT par des commits acquittés. `Neuf` n'apparaît que
/// par `SET` (jamais à la création).
const ACQUITTES: [&str; 4] = ["A", "B", "C", "Neuf"];
/// Touché seulement par la transaction jamais commitée.
const FANTOME: &str = "Fantome";

fn ks(db: &ObrainDB, label: &str) -> BTreeSet<i64> {
    let s = db.session();
    let r = s
        .execute(&format!("MATCH (n:{label}) RETURN n.k"))
        .expect("lecture");
    r.rows
        .iter()
        .map(|row| match row.first() {
            Some(Value::Int64(k)) => *k,
            autre => panic!("k inattendu : {autre:?}"),
        })
        .collect()
}

fn dormir_pour_toujours() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Générateur déterministe (LCG) : la suite est la même à chaque exécution.
struct Lcg(u64);
impl Lcg {
    fn suivant(&mut self, borne: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % borne
    }
}

#[test]
fn enfant_labels_commite_et_dort() {
    let Ok(dir) = std::env::var(ENV) else {
        return;
    };
    let dir = Path::new(&dir);
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    let s = db.session();
    // Modèle de l'état acquitté : k → labels, pour les nœuds vivants.
    let mut modele: BTreeMap<i64, BTreeSet<&str>> = BTreeMap::new();
    for k in 0..N {
        let labels: Vec<&str> = match k % 3 {
            0 => vec!["A"],
            1 => vec!["A", "B"],
            _ => vec!["B", "C"],
        };
        s.execute(&format!("INSERT (:{} {{k: {k}}})", labels.join(":")))
            .expect("insert");
        modele.insert(k, labels.into_iter().collect());
    }
    // L'index est construit ICI, avant les mutations : la suite passe donc par
    // les crochets d'écriture, pas par une reconstruction.
    assert_eq!(ks(&db, "A").len(), 80);
    let mut lcg = Lcg(0x5eed);
    for tour in 0..300 {
        let k = lcg.suivant(N as u64) as i64;
        let label = ACQUITTES[lcg.suivant(ACQUITTES.len() as u64) as usize];
        let Some(labels) = modele.get_mut(&k) else {
            continue;
        };
        match lcg.suivant(10) {
            0..=4 => {
                s.execute(&format!("MATCH (n {{k: {k}}}) SET n:{label}"))
                    .expect("set");
                labels.insert(label);
            }
            5..=8 => {
                s.execute(&format!("MATCH (n {{k: {k}}}) REMOVE n:{label}"))
                    .expect("remove");
                labels.remove(label);
            }
            _ => {
                s.execute(&format!("MATCH (n {{k: {k}}}) DETACH DELETE n"))
                    .expect("delete");
                modele.remove(&k);
            }
        }
        if tour % 37 == 0 {
            // Lecture en cours de route : fusion des ajouts hors ordre,
            // compaction éventuelle.
            let _ = ks(&db, label);
        }
    }
    // Une transaction ANNULÉE : rien d'elle ne doit rester.
    {
        let mut t = db.session();
        t.begin_transaction().expect("begin");
        t.execute("MATCH (n:A) SET n:C")
            .expect("set en transaction");
        t.execute("MATCH (n:B) REMOVE n:B")
            .expect("remove en transaction");
        t.rollback().expect("rollback");
    }
    let mut attendu = String::new();
    for l in ACQUITTES {
        let v: Vec<String> = modele
            .iter()
            .filter(|(_, ls)| ls.contains(l))
            .map(|(k, _)| k.to_string())
            .collect();
        attendu.push_str(l);
        attendu.push(' ');
        attendu.push_str(&v.join(","));
        attendu.push('\n');
    }
    // Le modèle et la base vivante concordent avant le kill.
    for l in ACQUITTES {
        let attendu_l: BTreeSet<i64> = modele
            .iter()
            .filter(|(_, ls)| ls.contains(l))
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(ks(&db, l), attendu_l, "enfant, avant kill : {l}");
    }
    let tmp = dir.join("attendu.tmp");
    std::fs::write(&tmp, attendu).unwrap();
    std::fs::rename(&tmp, dir.join("attendu")).unwrap();
    // Transaction JAMAIS commitée : son label peut revenir ou non au rejeu
    // (hors du périmètre de ce test), mais l'index doit égaler le balayage.
    let mut t = db.session();
    t.begin_transaction().expect("begin");
    t.execute(&format!("MATCH (n:A) SET n:{FANTOME}"))
        .expect("set fantôme");
    std::fs::write(dir.join("pret"), b"ok").unwrap();
    dormir_pour_toujours();
}

#[test]
fn index_de_labels_egal_au_balayage_apres_kill_9() {
    let td = tempfile::tempdir().unwrap();
    let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["enfant_labels_commite_et_dort", "--exact", "--nocapture"])
        .env(ENV, td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn enfant");
    let pret = td.path().join("pret");
    let mut tours = 0u32;
    while !pret.exists() {
        if let Some(statut) = enfant.try_wait().unwrap() {
            panic!("l'enfant est mort avant le marqueur : {statut:?}");
        }
        tours += 1;
        assert!(tours < 2400, "l'enfant n'a jamais posé {}", pret.display());
        std::thread::sleep(Duration::from_millis(50));
    }
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    let db = ObrainDB::open(td.path().join("base")).expect("réouverture après kill");
    let store = db.substrate_handle().expect("base substrat");
    for l in ACQUITTES.iter().copied().chain([FANTOME]) {
        let index = store.nodes_by_label(l);
        let balayage = store.nodes_by_label_balayage(l);
        assert_eq!(index, balayage, "après kill -9 : index ≠ balayage pour {l}");
    }
    let d = store.diagnostic_index_labels();
    assert!(d.actif && d.construit, "anti-vacuité : {d:?}");

    let attendu = std::fs::read_to_string(td.path().join("attendu")).unwrap();
    for ligne in attendu.lines() {
        let (l, liste) = ligne.split_once(' ').unwrap();
        let k_attendus: BTreeSet<i64> = liste
            .split(',')
            .filter(|x| !x.is_empty())
            .map(|x| x.parse().unwrap())
            .collect();
        assert_eq!(
            ks(&db, l),
            k_attendus,
            "après kill -9 : le label {l} ne rend pas les commits acquittés"
        );
    }
}
