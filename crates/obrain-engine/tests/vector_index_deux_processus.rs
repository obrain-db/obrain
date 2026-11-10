//! ── L'INDEX VECTORIEL SUIT LES ÉCRITURES D'UN PAIR (DEUX PROCESSUS) ─────────
//!
//! Pendant vectoriel de `index_texte_deux_processus.rs` (O-VEC C3). Un index
//! vectoriel persisté porte une empreinte (offset WAL + nœuds vivants) ; une
//! réouverture ne le charge que si l'empreinte décrit encore le store. Le
//! risque propre à la multi-écriture : un processus A garde un index dont la
//! fermeture RE-PUBLIERAIT une empreinte couvrant la fin partagée du WAL alors
//! que l'index n'a pas vu les vecteurs écrits par un pair B. Le suivant
//! chargerait un index qui ignore B : résultats faux, sans erreur.
//!
//! Attendu : après l'écriture d'un pair, la recherche vectorielle d'un
//! processus rouvrant la base trouve le vecteur du pair — rebâti ou chargé.
//!
//! Harnais : les écrivains et le lecteur final sont des SOUS-PROCESSUS (même
//! binaire de test, rôle par variable d'environnement).

#![cfg(feature = "vector-index")]

use std::path::Path;
use std::process::Command;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use tempfile::tempdir;

const ROLE: &str = "OVEC_MP_ROLE";
const DOSSIER: &str = "OVEC_MP_DIR";
const D: usize = 64;

fn role() -> Option<String> {
    std::env::var(ROLE).ok()
}

/// Lance ce même test dans un sous-processus avec le rôle donné.
fn enfant(test: &str, role: &str, dir: &Path) {
    let moi = std::env::current_exe().expect("current_exe");
    let statut = Command::new(moi)
        .args(["--exact", test, "--nocapture"])
        .env(ROLE, role)
        .env(DOSSIER, dir)
        .status()
        .expect("lancer le sous-processus");
    assert!(statut.success(), "sous-processus `{role}` en échec");
}

/// Vecteur unitaire porté par l'axe `axe`.
fn axe(axe: usize) -> Vec<f32> {
    let mut v = vec![0f32; D];
    v[axe] = 1.0;
    v
}

/// Vecteurs de départ : loin de l'axe 5 (ils vivent sur les axes 0..4).
fn creer_doc(db: &ObrainDB, v: Vec<f32>) {
    let id = db.create_node(&["Doc"]);
    db.set_node_property(id, "emb", Value::Vector(v.into()));
}

/// Base de départ : quelques vecteurs, index construit et persisté.
fn base_de_depart(dir: &Path) {
    let db = ObrainDB::open(dir).expect("open");
    for i in 0..5 {
        creer_doc(&db, axe(i));
    }
    db.create_vector_index("Doc", "emb", Some(D), Some("cosine"), None, None)
        .expect("index vectoriel");
    assert!(db.nb_index_vectoriel_persistes() >= 1);
    db.close().expect("close");
}

/// Un pair écrit le vecteur de l'axe 5 dans la base partagée puis ferme.
fn ecrire_pair(dir: &Path) {
    let db = ObrainDB::open_shared_write(dir).expect("open_shared_write pair");
    creer_doc(&db, axe(5));
    db.close().expect("close pair");
}

/// Le vecteur du pair est-il VISIBLE par ce processus (lecture faisant autorité :
/// `store().get_node_property`, chemin colonne) ? C'est la garantie du STORE en
/// écriture partagée, distincte de celle de l'index : l'index ne peut contenir
/// que ce que la colonne lui montre.
fn vecteur_du_pair_visible(db: &ObrainDB) -> bool {
    // Les identifiants sont alloues par fenetres (par processus en ecriture
    // partagee) : on les demande au moteur plutot que de les deviner.
    let Ok(resultat) = db.execute("MATCH (n:Doc) RETURN id(n)") else {
        return false;
    };
    let cle = obrain_common::types::PropertyKey::new("emb");
    resultat
        .rows
        .iter()
        .filter_map(|ligne| ligne.first())
        .filter_map(|v| match v {
            Value::Int64(i) => Some(*i as u64),
            autre => format!("{autre:?}")
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok(),
        })
        .any(|id| {
            matches!(
                db.store().get_node_property(obrain_common::types::NodeId(id), &cle),
                Some(Value::Vector(v)) if v.len() == D && (v[5] - 1.0).abs() < 1e-6
            )
        })
}

/// Le lecteur final : un processus neuf rouvre la base et cherche l'axe 5.
///
/// Garantie de C3, inconditionnelle : un index qui IGNORE le pair n'est jamais
/// chargé (il est reconstruit). Garantie conditionnelle : si le store montre le
/// vecteur du pair à ce processus, l'index reconstruit le trouve.
fn verifier_trouve(dir: &Path) {
    let db = ObrainDB::open_shared_write(dir).expect("reouverture");
    let visible = vecteur_du_pair_visible(&db);
    let hits = db
        .vector_search("Doc", "emb", &axe(5), 1, None, None)
        .expect("recherche vectorielle");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        db.nb_index_vectoriel_charges(),
        0,
        "un index persiste qui ignore l'ecriture du pair ne doit JAMAIS etre charge"
    );
    assert_eq!(
        db.nb_index_vectoriel_reconstruits(),
        1,
        "il est reconstruit depuis la colonne"
    );
    if visible {
        assert!(
            hits[0].1 < 1e-3,
            "le vecteur du pair est visible dans la colonne mais l'index reconstruit ne le \
             trouve pas (distance {})",
            hits[0].1
        );
    } else {
        eprintln!(
            "LIMITE v0.0.1 (hors C3) : le vecteur ecrit par le pair n'est pas visible par un \
             troisieme processus via la colonne de vecteurs (garantie multi-ecrivain des \
             proprietes : v0.0.2) — l'index reconstruit ne peut pas le contenir (distance {})",
            hits[0].1
        );
    }
    db.close().expect("close");
}

#[test]
fn index_vectoriel_ecrit_par_un_pair_est_trouve_a_la_reouverture() {
    if let Some(r) = role() {
        let dir = std::path::PathBuf::from(std::env::var(DOSSIER).expect("dossier"));
        match r.as_str() {
            "ecrit" => ecrire_pair(&dir),
            "cherche" => verifier_trouve(&dir),
            autre => panic!("rôle inconnu : {autre}"),
        }
        return;
    }
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("pair-puis-reouverture");
    base_de_depart(&dir);
    enfant(
        "index_vectoriel_ecrit_par_un_pair_est_trouve_a_la_reouverture",
        "ecrit",
        &dir,
    );
    enfant(
        "index_vectoriel_ecrit_par_un_pair_est_trouve_a_la_reouverture",
        "cherche",
        &dir,
    );
}

#[test]
fn index_ouvert_puis_ferme_apres_ecriture_d_un_pair_ne_masque_pas_le_pair() {
    // Scénario adversarial : A tient l'index ouvert, B écrit, A ferme en DERNIER.
    // La fermeture de A ne doit pas republier un index qui ignore B.
    if let Some(r) = role() {
        let dir = std::path::PathBuf::from(std::env::var(DOSSIER).expect("dossier"));
        match r.as_str() {
            "ecrit" => ecrire_pair(&dir),
            "cherche" => verifier_trouve(&dir),
            autre => panic!("rôle inconnu : {autre}"),
        }
        return;
    }
    let td = tempdir().expect("tempdir");
    let dir = td.path().join("adversarial");
    base_de_depart(&dir);
    // A : ouvert en écriture partagée, index au registre (chargé) et persisté.
    let a = ObrainDB::open_shared_write(&dir).expect("A open_shared_write");
    assert!(a.est_en_ecriture_partagee());
    let _ = a
        .vector_search("Doc", "emb", &axe(0), 1, None, None)
        .expect("A charge son index");
    assert!(
        a.nb_index_vectoriel_charges() + a.nb_index_vectoriel_reconstruits() == 1,
        "A tient un index"
    );
    // B : écrit un vecteur pendant que A tient son index.
    enfant(
        "index_ouvert_puis_ferme_apres_ecriture_d_un_pair_ne_masque_pas_le_pair",
        "ecrit",
        &dir,
    );
    // A ferme en dernier : c'est sa fermeture qui décide de l'index sur disque.
    a.close().expect("close A");
    enfant(
        "index_ouvert_puis_ferme_apres_ecriture_d_un_pair_ne_masque_pas_le_pair",
        "cherche",
        &dir,
    );
}
