//! obrain#120 point 4 — UNE CRÉATION DE NŒUD AVEC PROPRIÉTÉS EST ATOMIQUE
//! SOUS `kill -9`.
//!
//! `create_node_with_props` écrivait un `NodeInsert` puis N propriétés, chacune
//! son propre record WAL, sans rien qui les lie. Un processus tué entre deux
//! revenait avec un nœud entier absent ou, pire, un nœud vivant qui porte
//! seulement une partie de ses propriétés (une « coquille ») : l'appelant devait
//! poser un marqueur de brouillon et réparer à l'ouverture.
//!
//! Le test tue un enfant (point d'injection `obrain_common::utils::panne`,
//! `abort` = état d'un SIGKILL) à chaque instant de la création d'un nœud portant
//! 6 propriétés (3 courtes, 3 de plus de 256 o : colonne blob, #123), puis
//! rouvre la base : le nœud doit exister ENTIER, ou pas du tout.
//!
//! Protocole de `blob_hors_wal_kill9.rs` : le parent relance son binaire de test
//! sur le cas enfant (`--exact`), répertoire en variable d'environnement.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;

const ENV_ENFANT: &str = "OBRAIN_TEST_CREATION_ATOMIQUE_ENFANT";
const LONG: usize = 585;
/// Propriétés du nœud : p0..p2 courtes, p3..p5 longues.
const NB_PROPS: u64 = 6;

fn valeur(i: u64) -> String {
    if i < 3 {
        format!("courte-{i}")
    } else {
        let motif = format!("[long-{i}]");
        motif.repeat(LONG / motif.len() + 1)[..LONG].to_string()
    }
}

fn props() -> Vec<(String, Value)> {
    (0..NB_PROPS)
        .map(|i| (format!("p{i}"), Value::String(valeur(i).into())))
        .collect()
}

/// Enfant : `<chemin>:<rang>:<répertoire>`. `chemin` = `session` | `db` | `tx`
/// (transaction explicite, jamais validée). Écrit un témoin complet, arme
/// l'arrêt après la propriété `rang`, crée le nœud : ne rend jamais la main.
#[test]
fn enfant_createur() {
    let Ok(spec) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let mut m = spec.splitn(3, ':');
    let chemin = m.next().unwrap().to_string();
    let rang: u64 = m.next().unwrap().parse().unwrap();
    let dir = Path::new(m.next().unwrap());
    let db = ObrainDB::open(dir.join("base")).expect("open enfant");
    let p = props();
    let refs = || p.iter().map(|(k, v)| (k.as_str(), v.clone()));
    // Témoin : un nœud complet, acquitté AVANT la panne.
    db.session().create_node_with_props(&["Temoin"], refs());
    obrain_common::utils::panne::armer_arret_creation_noeud(rang);
    match chemin.as_str() {
        "session" => {
            db.session().create_node_with_props(&["Atome"], refs());
        }
        "db" => {
            db.create_node_with_props(&["Atome"], refs());
        }
        "tx" => {
            let mut s = db.session();
            s.begin_transaction().unwrap();
            s.create_node_with_props(&["Atome"], refs());
        }
        autre => panic!("chemin inconnu {autre}"),
    }
    // Atteint seulement si `rang` dépasse le nombre de propriétés.
    std::process::exit(3);
}

/// Tue l'enfant à l'instant `rang` et rouvre : `(nœuds Atome, complets)`.
fn tuer_a_et_relire(chemin: &str, rang: u64) -> (usize, usize, ObrainDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["enfant_createur", "--exact", "--nocapture"])
        .env(
            ENV_ENFANT,
            format!("{chemin}:{rang}:{}", dir.path().display()),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");
    let statut = enfant.wait().expect("wait enfant");
    assert!(
        !statut.success() && statut.code() != Some(3),
        "l'enfant devait mourir sur le point d'injection (rang {rang}) : {statut:?}"
    );

    let db = ObrainDB::open(dir.path().join("base")).expect("réouverture");
    let r = db
        .session()
        .execute("MATCH (n:Atome) RETURN n.p0, n.p1, n.p2, n.p3, n.p4, n.p5")
        .expect("relecture");
    let presents = r.rows.len();
    let complets = r
        .rows
        .iter()
        .filter(|l| {
            l.iter()
                .enumerate()
                .all(|(i, v)| matches!(v, Value::String(s) if s.as_str() == valeur(i as u64)))
        })
        .count();
    (presents, complets, db, dir)
}

fn verifier(chemin: &str) {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let mut echecs = Vec::new();
    for rang in 0..NB_PROPS {
        let (presents, complets, db, _dir) = tuer_a_et_relire(chemin, rang);
        // Témoin : acquitté avant la panne, entier.
        let temoins = db
            .session()
            .execute("MATCH (n:Temoin) RETURN n.p0, n.p5")
            .expect("témoins")
            .rows
            .len();
        assert_eq!(temoins, 1, "le témoin acquitté doit survivre (rang {rang})");
        if presents != complets {
            echecs.push(format!(
                "tué après {rang} propriété(s) sur {NB_PROPS} : {presents} nœud(s) \
                 :Atome présent(s), {complets} complet(s) — coquille"
            ));
        }
    }
    assert!(
        echecs.is_empty(),
        "obrain#120 pt 4 ({chemin}) : la création n'est pas atomique :\n  {}",
        echecs.join("\n  ")
    );
}

#[test]
fn creation_par_session_entiere_ou_absente_apres_kill() {
    verifier("session");
}

#[test]
fn creation_par_db_entiere_ou_absente_apres_kill() {
    verifier("db");
}

/// Témoin vert : dans une transaction explicite jamais validée, le nœud est
/// déjà écarté au rejeu (TxNoeudEnAttente sans TxCommit).
#[test]
fn creation_en_transaction_non_validee_est_ecartee() {
    verifier("tx");
}
