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
//!
//! ## Pourquoi le journal porte l'INTENTION avant l'écriture
//!
//! Un journal qui n'écrit que les acquittements ne sait pas distinguer les deux
//! issues d'un `kill -9` reçu entre le retour de `durable()` et la ligne de
//! journal :
//!
//! 1. la valeur acquittée a été **perdue** (le défaut de obrain#172) ;
//! 2. l'écriture **suivante**, non acquittée, a atterri (atterrissage normal
//!    d'une transaction dont le commit a été franchi — ce qu'exige
//!    l'atomicité : ce qui est durable est ce qui est commité).
//!
//! Les deux se lisent « la valeur durable n'est pas le dernier acquittement ».
//! Sans l'intention, elles sont indiscernables : c'est ce qui a fait échouer la
//! première version de ce test en CI (Linux, run 43645097620) alors que la
//! valeur durable était `court-…`, c'est-à-dire l'écriture juste après
//! l'acquittement. L'intention écrite **avant** de muter sépare les deux cas :
//! si aucune intention `court-…` n'est au journal, la valeur durable doit être
//! le blob acquitté, et rien d'autre ; si elle y est, l'une ou l'autre est
//! légitime, et jamais une valeur plus ancienne.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

const ENV_ENFANT: &str = "OBRAIN_TEST_BLOB_DEUX_PROPS_ENFANT";
const TOURS: u64 = 8;
/// Nombre de lignes de journal produites par une itération de l'enfant (deux
/// intentions, deux acquittements).
const LIGNES_PAR_ITERATION: usize = 4;

const INTENTION: &str = "intention";
const ACQUITTE: &str = "acquitte";

fn durable(db: &ObrainDB) {
    db.flush_wal().expect("flush_wal");
    db.commit_wal().expect("commit_wal");
}

/// Consigne `quoi<TAB>path<TAB>hash` en **un seul** `write` : une ligne
/// tronquée par le `kill -9` reste ainsi reconnaissable (elle est ignorée à la
/// relecture au lieu de faire échouer l'analyse).
fn consigner(log: &mut std::fs::File, quoi: &str, path: &str, hash: &str) {
    let ligne = format!("{quoi}\t{path}\t{hash}\n");
    log.write_all(ligne.as_bytes()).expect("journal");
}

/// Étiquette courte d'une valeur, pour les messages d'échec.
fn abrege(v: &str) -> String {
    if v.len() <= 24 {
        return v.to_string();
    }
    format!("{}…{}", &v[..6], &v[v.len() - 8..])
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
        .open(dir.join("journal"))
        .unwrap();
    let pid = std::process::id();
    let mut i = 0u64;
    loop {
        i += 1;
        let path = format!("src/{}/{pid}-{i}.rs", "d".repeat(300));
        let long = format!("{}{pid}-{i}", "h".repeat(300));
        let id = db.create_node(&["File"]);

        consigner(&mut log, INTENTION, &path, &long);
        db.set_node_property(id, "brouillon", Value::from(true));
        db.set_node_property(id, "path", Value::String(path.clone().into()));
        db.set_node_property(id, "hash", Value::String(long.clone().into()));
        durable(&db);
        consigner(&mut log, ACQUITTE, &path, &long);

        db.remove_node_property(id, "brouillon");
        durable(&db);

        let court = format!("court-{pid}-{i}");
        consigner(&mut log, INTENTION, &path, &court);
        db.set_node_property(id, "path", Value::String(path.clone().into()));
        db.set_node_property(id, "hash", Value::String(court.clone().into()));
        durable(&db);
        consigner(&mut log, ACQUITTE, &path, &court);
    }
}

#[test]
fn une_valeur_blob_acquittee_survit_a_des_kill_9_successifs() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let journal = dir.path().join("journal");
    let compte = || std::fs::read_to_string(&journal).map_or(0, |s| s.lines().count());
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
            if compte() >= avant + 2 * LIGNES_PAR_ITERATION {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            compte() >= avant + 2 * LIGNES_PAR_ITERATION,
            "tour {tour}: aucun acquittement"
        );
        std::thread::sleep(Duration::from_micros(1_500 * (tour % 9 + 1)));
        // Un enfant déjà mort (échec d'ouverture du magasin, panique) rendrait
        // le `kill` inopérant et le tour suivant mesurerait autre chose que ce
        // qu'il annonce : on le nomme.
        if let Ok(Some(statut)) = enfant.try_wait() {
            panic!("tour {tour}: l'enfant écrivain s'est arrêté seul ({statut:?})");
        }
        enfant.kill().expect("kill -9");
        let _ = enfant.wait();

        // Relecture du journal : dernière intention et dernier acquittement
        // connus pour chaque chemin. Une ligne sans ses trois champs est une
        // ligne tronquée par le kill : ignorée.
        let mut intentions: HashMap<String, String> = HashMap::new();
        let mut acquittes: HashMap<String, String> = HashMap::new();
        let texte = std::fs::read_to_string(&journal).expect("journal");
        for ligne in texte.lines() {
            let mut champs = ligne.splitn(3, '\t');
            let (Some(quoi), Some(p), Some(h)) = (champs.next(), champs.next(), champs.next())
            else {
                continue;
            };
            let cible = match quoi {
                INTENTION => &mut intentions,
                ACQUITTE => &mut acquittes,
                _ => continue,
            };
            cible.insert(p.to_string(), h.to_string());
        }

        let db = ObrainDB::open(&base).expect("réouverture");
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

        // Aucun nœud durable ne peut porter un chemin que l'enfant n'a jamais
        // eu l'intention d'écrire : ce serait une contamination.
        for p in relus.keys() {
            assert!(
                intentions.contains_key(p) || acquittes.contains_key(p),
                "tour {tour}: nœud durable au chemin jamais écrit {}",
                abrege(p)
            );
        }

        for p in acquittes.keys().chain(intentions.keys()) {
            let lu = relus.get(p).and_then(|o| o.as_deref());
            let ack = acquittes.get(p).map(String::as_str);
            let intention = intentions.get(p).map(String::as_str);
            match (ack, intention) {
                // Écriture acquittée : sa valeur, ou celle de l'écriture
                // suivante si elle a été entamée. Jamais une plus ancienne,
                // jamais rien.
                (Some(a), Some(i)) => {
                    assert!(
                        lu == Some(a) || lu == Some(i),
                        "tour {tour}: {} → la valeur durable {} n'est ni l'acquittée {} ni \
                         l'écriture entamée {}",
                        abrege(p),
                        lu.map_or_else(|| "rien".to_string(), abrege),
                        abrege(a),
                        abrege(i)
                    );
                }
                // Intention écrite, acquittement jamais consigné : la valeur
                // peut être durable (commit franchi) ou absente (rien de
                // commité). Elle ne peut pas être autre chose.
                (None, Some(i)) => {
                    assert!(
                        lu.is_none() || lu == Some(i),
                        "tour {tour}: {} → écriture jamais acquittée laissant {} au lieu de \
                         {} ou rien",
                        abrege(p),
                        lu.map_or_else(|| "rien".to_string(), abrege),
                        abrege(i)
                    );
                }
                // Acquittement sans intention : impossible dans l'enfant (l'
                // intention précède toujours l'écriture), donc la valeur
                // acquittée doit être là, exactement.
                (Some(a), None) => assert_eq!(
                    lu,
                    Some(a),
                    "tour {tour}: valeur acquittée {} absente ou remplacée par {}",
                    abrege(a),
                    lu.map_or_else(|| "rien".to_string(), abrege)
                ),
                (None, None) => unreachable!("boucle sur les clés des deux journaux"),
            }
        }
        drop(db);
    }
}
