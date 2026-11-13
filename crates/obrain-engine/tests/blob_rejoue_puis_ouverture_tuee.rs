//! UNE VALEUR LONGUE REJOUÉE DEPUIS LE WAL, PUIS FERMÉE PROPREMENT, DISPARAÎT
//! QUAND UN PROCESSUS SUIVANT OUVRE LA BASE ET MEURT.
//!
//! Séquence, trois processus, rien d'autre que l'API Rust :
//!   1. l'écrivain A crée un nœud `Doc` (une valeur courte, une valeur de
//!      800 o, donc en colonne blob), une arête vers un autre nœud, puis
//!      `flush_wal()` ; il est tué (`kill -9`) ;
//!   2. le parent rouvre (rejeu WAL : la valeur longue revient), relit,
//!      `close()` (flush complet) ;
//!   3. le processus B ouvre la base, n'écrit RIEN, est tué ;
//!   4. le parent rouvre : la valeur longue doit être là.
//!
//! CAUSE (obrain#167) : le dictionnaire que le `close()` du parent écrit porte
//! des compteurs d'arêtes sans les histogrammes de labels (forme « v5 ») ; B,
//! en l'ouvrant, détecte ce v5 et faisait son auto-flush v5→v6 AU MILIEU de
//! l'ouverture, AVANT d'hydrater les registres de colonnes blob : le
//! dictionnaire réécrit n'avait plus la colonne, la valeur devenait illisible.
//! L'auto-flush est désormais différé à la fin de l'ouverture.
//!
//! Mesuré avant correctif (e5648dbf, 8d7e3b09) : perdue dès que l'arête
//! existe ; sans arête (pas d'histogramme à reconstruire), ou quand A fait
//! `commit_wal()`, rien n'est perdu.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const ENV_DIR: &str = "OBRAIN_TEST_REJEU_TUE_DIR";
const ENV_ROLE: &str = "OBRAIN_TEST_REJEU_TUE_ROLE";
const LONGUE: usize = 800;

fn valeur_longue() -> String {
    "é-long ".repeat(LONGUE / 8)
}

fn enfant(dir: &Path, role: &str) -> std::process::Child {
    let _ = std::fs::remove_file(dir.join("pret"));
    let journal = std::fs::File::create(dir.join(format!("journal-{role}"))).unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args([
            "une_valeur_longue_rejouee_puis_fermee_survit_a_une_ouverture_tuee",
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

/// (longueur de la courte, longueur de la longue) du nœud `Doc`, puis `close()`.
fn relire(base: &Path) -> Option<(Option<usize>, Option<usize>)> {
    let db = ObrainDB::open(base).unwrap();
    let ns = db.store().nodes_by_label("Doc");
    let r = ns.first().and_then(|n| db.get_node(*n)).map(|n| {
        (
            n.get_property("courte")
                .and_then(|v| v.as_str().map(str::len)),
            n.get_property("longue")
                .and_then(|v| v.as_str().map(str::len)),
        )
    });
    db.close().unwrap();
    r
}

#[test]
fn une_valeur_longue_rejouee_puis_fermee_survit_a_une_ouverture_tuee() {
    if let Ok(dir) = std::env::var(ENV_DIR) {
        let dir = std::path::PathBuf::from(dir);
        let role = std::env::var(ENV_ROLE).unwrap();
        let db = ObrainDB::open(dir.join("base")).unwrap();
        if role.starts_with('A') {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(n, "courte", Value::from("c"));
            db.set_node_property(n, "longue", Value::from(valeur_longue()));
            if role == "A-arete" {
                let m = db.create_node(&["Autre"]);
                db.create_edge(m, n, "A");
            }
            db.flush_wal().unwrap();
        }
        std::fs::write(dir.join("pret.tmp"), "x").unwrap();
        std::fs::rename(dir.join("pret.tmp"), dir.join("pret")).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    let attendu = Some((Some(1), Some(valeur_longue().len())));
    let mut pertes = Vec::new();
    for a in ["A-sans-arete", "A-arete"] {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base");
        let mut c = enfant(dir.path(), a);
        attendre(dir.path());
        c.kill().unwrap();
        c.wait().unwrap();
        let apres_a = relire(&base);
        assert_eq!(apres_a, attendu, "{a} : la valeur acquittée revient du WAL");
        let mut c = enfant(dir.path(), "B");
        attendre(dir.path());
        c.kill().unwrap();
        c.wait().unwrap();
        let apres_b = relire(&base);
        if apres_b != attendu {
            pertes.push(format!("{a} : {apres_b:?}"));
        }
    }
    assert!(
        pertes.is_empty(),
        "une valeur longue acquittée, rejouée puis fermée, perdue après une ouverture tuée : {pertes:#?}"
    );
}
