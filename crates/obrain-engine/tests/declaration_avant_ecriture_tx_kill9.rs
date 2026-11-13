//! EN TRANSACTION EXPLICITE, LA DÉCLARATION PRÉCÈDE L'ÉCRITURE (fenêtre kill -9).
//!
//! `create_node_versioned` écrivait le `NodeInsertV3` (`creer_noeud(.., false)`
//! → `Writer::write_node`), PUIS déclarait le nœud en vol (`TxNoeudEnAttente`)
//! dans un second append (`declare_en_vol`). Même ordre pour les arêtes
//! (`create_edge_versioned` : `EdgeInsert`, puis `TxAreteEnAttente`). Le rejeu
//! (`replay_from_avec`) n'écarte que ce qui a été déclaré : un processus tué
//! entre les deux appends rouvrait sur un nœud (ou une arête) PUBLIÉ d'une
//! transaction jamais validée.
//!
//! Le test tue un enfant au point d'injection `panne::PointTx` (juste après
//! l'écriture de l'entité, `abort` = état d'un SIGKILL) puis rouvre : l'entité
//! de la transaction non validée doit être absente ; le témoin acquitté avant
//! la panne, présent.
//!
//! Protocole de `creation_noeud_atomique_kill9.rs` : le parent relance son
//! binaire de test sur le cas enfant (`--exact`), spec en variable d'environnement.
#![cfg(feature = "wal")]

use obrain_common::utils::panne::{PointTx, armer_arret_tx};
use obrain_engine::ObrainDB;
use std::path::Path;

const ENV_ENFANT: &str = "OBRAIN_TEST_DECL_AVANT_ECRITURE_ENFANT";

/// Enfant : `<cas>:<répertoire>`, `cas` = `noeud` | `arete`. Ne rend jamais la main.
#[test]
fn enfant_tx() {
    let Ok(spec) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let (cas, dir) = spec.split_once(':').unwrap();
    let db = ObrainDB::open(Path::new(dir).join("base")).expect("open enfant");
    // Témoins acquittés (auto-commit) AVANT la panne ; pour le cas arête, ce
    // sont aussi les extrémités.
    let a = db.session().create_node(&["Temoin"]);
    let b = db.session().create_node(&["Temoin"]);
    let mut s = db.session();
    s.begin_transaction().unwrap();
    match cas {
        "noeud" => {
            armer_arret_tx(PointTx::NoeudEcrit);
            s.create_node(&["Fantome"]);
        }
        "arete" => {
            armer_arret_tx(PointTx::AreteEcrite);
            s.create_edge(a, b, "FANTOME");
        }
        autre => panic!("cas inconnu {autre}"),
    }
    // Atteint seulement si le point d'injection n'est pas sur le chemin.
    std::process::exit(3);
}

fn tuer_et_rouvrir(cas: &str) -> (ObrainDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let statut = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["enfant_tx", "--exact", "--nocapture"])
        .env(ENV_ENFANT, format!("{cas}:{}", dir.path().display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn enfant");
    assert!(
        !statut.success() && statut.code() != Some(3),
        "l'enfant devait mourir sur le point d'injection ({cas}) : {statut:?}"
    );
    let db = ObrainDB::open(dir.path().join("base")).expect("réouverture");
    (db, dir)
}

fn compter(db: &ObrainDB, requete: &str) -> usize {
    db.session().execute(requete).expect(requete).rows.len()
}

#[test]
fn noeud_de_tx_non_validee_absent_apres_kill_entre_insert_et_declaration() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let (db, _dir) = tuer_et_rouvrir("noeud");
    assert_eq!(
        compter(&db, "MATCH (n:Temoin) RETURN n"),
        2,
        "témoins acquittés"
    );
    assert_eq!(
        compter(&db, "MATCH (n:Fantome) RETURN n"),
        0,
        "nœud d'une transaction jamais validée publié au rejeu : le NodeInsert \
         a atteint le WAL sans sa déclaration TxNoeudEnAttente"
    );
}

#[test]
fn arete_de_tx_non_validee_absente_apres_kill_entre_insert_et_declaration() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let (db, _dir) = tuer_et_rouvrir("arete");
    assert_eq!(
        compter(&db, "MATCH (n:Temoin) RETURN n"),
        2,
        "témoins acquittés"
    );
    assert_eq!(
        compter(&db, "MATCH ()-[r:FANTOME]->() RETURN r"),
        0,
        "arête d'une transaction jamais validée publiée au rejeu : l'EdgeInsert \
         a atteint le WAL sans sa déclaration TxAreteEnAttente"
    );
}
