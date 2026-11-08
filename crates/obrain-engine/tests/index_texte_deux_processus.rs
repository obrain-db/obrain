//! ── L'INDEX TEXTE SUIT LES ÉCRITURES D'UN PAIR (DEUX PROCESSUS) ─────────────
//!
//! Garantie v0.0.1 : nœuds et arêtes sans propriété, en écriture partagée.
//! Un index texte persisté porte une empreinte (offset WAL + nœuds vivants).
//! Une réouverture ne charge l'index que si l'empreinte décrit encore le store.
//! Le risque propre à la multi-écriture : un processus A garde un index dont
//! l'empreinte RE-PUBLIÉE (à sa fermeture) couvre la fin partagée du WAL, alors
//! que l'index n'a pas vu les écritures d'un pair B. Le suivant chargerait un
//! index qui ignore les documents de B : résultats faux, sans erreur.
//!
//! Attendu : après écriture d'un pair, la recherche texte d'un processus
//! rouvrant la base trouve le document du pair. Qu'il soit rebâti ou chargé.
//!
//! Harnais : les écrivains et le lecteur final sont des SOUS-PROCESSUS (même
//! binaire de test, rôle par variable d'environnement), pour que chaque
//! ouverture soit une vraie ouverture de processus.

#![cfg(feature = "text-index")]

use std::path::Path;
use std::process::Command;

use obrain_engine::ObrainDB;
use tempfile::tempdir;

const ROLE: &str = "MP5_IDX_ROLE";
const DOSSIER: &str = "MP5_IDX_DIR";
const TEXTE: &str = "MP5_IDX_TEXTE";

fn role() -> Option<String> {
    std::env::var(ROLE).ok()
}

/// Lance ce même test dans un sous-processus avec le rôle donné.
fn enfant(test: &str, role: &str, dir: &Path, texte: &str) {
    let moi = std::env::current_exe().expect("current_exe");
    let statut = Command::new(moi)
        .args(["--exact", test, "--nocapture"])
        .env(ROLE, role)
        .env(DOSSIER, dir)
        .env(TEXTE, texte)
        .status()
        .expect("lancer le sous-processus");
    assert!(statut.success(), "sous-processus `{role}` en échec");
}

/// Crée un document indexé dans une session.
fn creer_doc(db: &ObrainDB, texte: &str) {
    db.session()
        .execute(&format!("CREATE (:Doc {{text: '{texte}'}})"))
        .expect("create");
}

/// Base de départ : quelques documents, index texte construit et persisté.
fn base_de_depart(dir: &Path) {
    let db = ObrainDB::open(dir).expect("open");
    creer_doc(&db, "le graphe de connaissances");
    creer_doc(&db, "le jardin potager");
    db.create_text_index("Doc", "text").expect("index texte");
    db.close().expect("close");
}

/// Le lecteur final : un processus neuf rouvre la base et cherche le terme.
fn verifier_trouve(dir: &Path, terme: &str) {
    let db = ObrainDB::open_shared_write(dir).expect("reouverture");
    let trouves = db
        .text_search("Doc", "text", terme, 10)
        .expect("recherche texte");
    assert_eq!(
        trouves.len(),
        1,
        "« {terme} » écrit par un pair doit être trouvé à la réouverture \
         (trouvés = {}, index chargés = {})",
        trouves.len(),
        db.nb_index_texte_charges()
    );
    db.close().expect("close");
}

/// Un pair écrit un document indexé dans la base partagée puis ferme.
fn ecrire_pair(dir: &Path, texte: &str) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write pair");
    creer_doc(&db, texte);
    db.close().expect("close pair");
}

#[test]
fn index_texte_ecrit_par_un_pair_est_trouve_a_la_reouverture() {
    if let Some(r) = role() {
        let dir = std::path::PathBuf::from(std::env::var(DOSSIER).expect("dossier"));
        let texte = std::env::var(TEXTE).unwrap_or_default();
        match r.as_str() {
            "ecrit" => ecrire_pair(&dir, &texte),
            "cherche" => verifier_trouve(&dir, &texte),
            autre => panic!("rôle inconnu : {autre}"),
        }
        return;
    }
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("pair-puis-reouverture");
    base_de_depart(&dir);
    enfant(
        "index_texte_ecrit_par_un_pair_est_trouve_a_la_reouverture",
        "ecrit",
        &dir,
        "beta gamma",
    );
    enfant(
        "index_texte_ecrit_par_un_pair_est_trouve_a_la_reouverture",
        "cherche",
        &dir,
        "beta",
    );
}

#[test]
fn index_ouvert_puis_ferme_apres_ecriture_d_un_pair_ne_masque_pas_le_pair() {
    // Scénario adversarial : A tient l'index ouvert, B écrit, A ferme en DERNIER.
    // La fermeture de A ne doit pas republier un index qui ignore B.
    if let Some(r) = role() {
        let dir = std::path::PathBuf::from(std::env::var(DOSSIER).expect("dossier"));
        let texte = std::env::var(TEXTE).unwrap_or_default();
        match r.as_str() {
            "ecrit" => ecrire_pair(&dir, &texte),
            "cherche" => verifier_trouve(&dir, &texte),
            autre => panic!("rôle inconnu : {autre}"),
        }
        return;
    }
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("adversarial");
    base_de_depart(&dir);

    // A : ouvert en écriture partagée, index construit et persisté.
    let a = ObrainDB::open_shared_write(&dir).expect("A open_shared_write");
    a.create_text_index("Doc", "text").expect("index A");
    assert!(
        a.nb_index_texte_persistes() >= 1,
        "A doit avoir persisté son index avant l'écriture du pair"
    );

    // B : écrit un document indexé pendant que A tient son index.
    enfant(
        "index_ouvert_puis_ferme_apres_ecriture_d_un_pair_ne_masque_pas_le_pair",
        "ecrit",
        &dir,
        "beta gamma",
    );

    // A ferme en dernier : c'est sa fermeture qui décide de l'index sur disque.
    a.close().expect("close A");

    enfant(
        "index_ouvert_puis_ferme_apres_ecriture_d_un_pair_ne_masque_pas_le_pair",
        "cherche",
        &dir,
        "beta",
    );
}
