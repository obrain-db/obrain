//! obrain#164 x obrain#167 — LE SCÉNARIO COMBINÉ : une valeur longue acquittée
//! (colonne blob), une transaction EN VOL sur les mêmes propriétés, un `kill -9`,
//! puis une OUVERTURE TUÉE (le scénario de #167).
//!
//! Séquence, trois processus (parent, A, B), API Rust seule :
//!   1. l'écrivain A crée un nœud `Doc` avec `courte` (c), `longue` (800 o, en
//!      colonne blob), avec ou sans arête vers un autre nœud, et l'acquitte par
//!      `flush_wal()` ; puis, en transaction EXPLICITE, réécrit `courte` et
//!      `longue`. Cas `A-en-vol-*` : A est tué (`kill -9`) AVANT le commit.
//!      Cas `A-commit-*` (témoin) : la transaction est commitée avant le kill ;
//!   2. le parent rouvre (restauration des valeurs d'avant, #164), relit, `close()` ;
//!   3. le processus B ouvre la base, n'écrit RIEN, est tué (ouverture tuée) ;
//!   4. le parent rouvre et relit.
//!
//! La transaction de A reste VIVANTE jusqu'au kill. Sa destruction avant (rollback
//! au drop) annulerait ses écritures et masquerait le défaut : c'est ce qui rendait
//! la version précédente de ce test verte sans le correctif.
//!
//! Attendu (cas `A-en-vol-*`) : les valeurs d'AVANT la transaction, à chaque
//! relecture. Témoin (`A-commit-*`) : les valeurs de la transaction commitée.
//!
//! Rouge sans #164 (mesuré) : l'étape 2 sert les valeurs de la transaction en vol.
//! Rouge sans #167 (mesuré, cas `avec-arete`) : la valeur longue disparaît à
//! l'étape 4 (flush de l'ouverture de B, fait avant que les colonnes blob soient
//! hydratées).
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const ENV_DIR: &str = "OBRAIN_TEST_RejeuOuvTuee_DIR";
const ENV_ROLE: &str = "OBRAIN_TEST_RejeuOuvTuee_ROLE";
const LONGUE: usize = 800;

/// Valeur longue d'AVANT la transaction.
fn longue_avant() -> String {
    "avant-é ".repeat(LONGUE / 8)
}

/// Valeur longue de la transaction (même longueur, autre contenu).
fn longue_tx() -> String {
    "tx-é ".repeat(LONGUE / 5)
}

fn enfant(dir: &Path, role: &str) -> std::process::Child {
    let _ = std::fs::remove_file(dir.join("pret"));
    let journal = std::fs::File::create(dir.join(format!("journal-{role}"))).unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args([
            "rejeu_en_vol_puis_ouverture_tuee",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ENV_DIR, dir)
        .env(ENV_ROLE, role)
        .stdout(Stdio::null())
        .stderr(journal)
        .spawn()
        .unwrap()
}

fn attendre(dir: &Path) {
    for _ in 0..1200 {
        if dir.join("pret").exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("l'enfant n'a jamais posé son marqueur");
}

/// Pose le marqueur `pret` puis dort jusqu'au `kill -9` (le processus garde
/// ce qu'il a ouvert, transaction comprise).
fn poser_et_dormir(dir: &Path) -> ! {
    std::fs::write(dir.join("pret.tmp"), "x").unwrap();
    std::fs::rename(dir.join("pret.tmp"), dir.join("pret")).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// (courte, longue) du nœud `Doc`, puis `close()`.
fn relire(base: &Path) -> Option<(Option<String>, Option<String>)> {
    let db = ObrainDB::open(base).unwrap();
    let ns = db.store().nodes_by_label("Doc");
    let r = ns.first().and_then(|n| db.get_node(*n)).map(|n| {
        (
            n.get_property("courte")
                .and_then(|v| v.as_str().map(str::to_string)),
            n.get_property("longue")
                .and_then(|v| v.as_str().map(str::to_string)),
        )
    });
    db.close().unwrap();
    r
}

#[test]
fn rejeu_en_vol_puis_ouverture_tuee() {
    if let Ok(dir) = std::env::var(ENV_DIR) {
        let dir = std::path::PathBuf::from(dir);
        let role = std::env::var(ENV_ROLE).unwrap();
        let db = ObrainDB::open(dir.join("base")).unwrap();
        if role.starts_with('A') {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(n, "courte", Value::from("c"));
            db.set_node_property(n, "longue", Value::from(longue_avant()));
            if role.contains("avec-arete") {
                let m = db.create_node(&["Autre"]);
                db.create_edge(m, n, "A");
            }
            db.flush_wal().unwrap();

            // La transaction reste VIVANTE jusqu'au kill : sa destruction avant
            // le kill annulerait ses écritures (rollback au drop) et masquerait
            // le défaut. Le marqueur est posé à l'intérieur de ce bloc.
            let mut t = db.session();
            t.begin_transaction().unwrap();
            t.execute(&format!(
                "MATCH (n:Doc) SET n.courte = 'tx', n.longue = '{}'",
                longue_tx()
            ))
            .unwrap();
            // Garde : la transaction voit ses propres écritures. Sinon « avant »
            // serait vrai trivialement et le test ne prouverait rien.
            let vu = t
                .execute("MATCH (n:Doc) RETURN n.courte, n.longue")
                .unwrap()
                .rows
                .first()
                .cloned();
            assert_eq!(
                vu,
                Some(vec![Value::from("tx"), Value::from(longue_tx())]),
                "la transaction ne voit pas son SET"
            );
            if role.starts_with("A-commit") {
                t.commit().unwrap();
            }
            poser_et_dormir(&dir);
        }
        // `B` n'écrit rien : il ouvre (et donc relance le rejeu) puis meurt.
        poser_et_dormir(&dir);
    }

    let avant = (Some("c".to_string()), Some(longue_avant()));
    let apres = (Some("tx".to_string()), Some(longue_tx()));
    // Cas `en-vol-*` : la transaction est tuée avant commit. `commit-*` est le
    // témoin : la même séquence, commitée, doit laisser les valeurs de la tx.
    let cas = [
        ("A-en-vol-sans-arete", avant.clone()),
        ("A-en-vol-avec-arete", avant.clone()),
        ("A-commit-avec-arete", apres.clone()),
    ];
    let mut echecs = Vec::new();
    for (role, attendu) in cas {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        let mut c = enfant(dir.path(), role);
        attendre(dir.path());
        c.kill().unwrap();
        c.wait().unwrap();

        // Le parent rouvre (restauration), relit, ferme.
        let apres_a = relire(&base);
        if apres_a != Some(attendu.clone()) {
            echecs.push(format!(
                "{role} après A tué, réouverture du parent : {apres_a:?}"
            ));
        }

        // B ouvre la base et est tué pendant qu'il la tient ouverte.
        let mut c = enfant(dir.path(), "B");
        attendre(dir.path());
        c.kill().unwrap();
        c.wait().unwrap();

        let apres_b = relire(&base);
        if apres_b != Some(attendu.clone()) {
            echecs.push(format!("{role} après ouverture tuée de B : {apres_b:?}"));
        }
    }
    assert!(
        echecs.is_empty(),
        "obrain#164 x #167 : valeur acquittée ou restaurée incorrecte : {echecs:#?}"
    );
}
