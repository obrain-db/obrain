//! UNE BASE HÉRITÉE N'EST PAS OUVERTE EN ÉCRITURE PARTAGÉE — refus nommé.
//!
//! Garantie v0.0.1 (périmètre restreint) : la multi-écriture ne couvre que les
//! nœuds et arêtes SANS propriété. Une base dont un sidecar hérité
//! (`substrate.props`, `substrate.edge_props`, `substrate.extedges`) porte des
//! données n'a pas ce périmètre : ses propriétés vivent dans un état LOCAL au
//! processus, persisté au flush depuis la DashMap de ce processus. Ouverte en
//! `open_shared_write`, elle produirait des écritures qui écrasent ou perdent
//! les propriétés du pair, en silence.
//!
//! Attendu : `open_shared_write` refuse avec `SubstrateError::BaseHeritee`
//! (qui nomme le sidecar et dit quoi faire), et ne modifie RIEN sur disque.
//! Une base neuve reste ouvrable en écriture partagée, et une base héritée
//! reste ouvrable en exclusif (c'est le chemin de migration).
//!
//! Fixture : la base est créée par un SOUS-PROCESSUS avec
//! `OBRAIN_PROPS_V2=0`, qui force le régime hérité (propriétés dans
//! `substrate.props`). L'env est lu à l'ouverture, donc on ne touche pas à
//! l'environnement du processus de test.

use obrain_common::types::Value;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{SubstrateError, SubstrateStore};
use std::collections::BTreeMap;
use std::path::Path;

const ROLE: &str = "MP5_BASE_HERITEE_ROLE";
const DOSSIER: &str = "MP5_BASE_HERITEE_DIR";

/// Fabrique une base héritée : un nœud porteur de propriété, flushé. Appelée
/// uniquement dans le sous-processus (rôle `fabrique`).
fn fabriquer_base_heritee(dir: &Path) {
    let store = SubstrateStore::create(dir).expect("creation de la base hérité");
    let id = store.create_node(&["Doc"]);
    store.set_node_property(id, "text", Value::from("une propriété héritée"));
    store.flush().expect("flush");
    drop(store);
}

/// Lance ce même test dans un sous-processus qui fabrique la base héritée.
fn fabriquer_dans_un_enfant(dir: &Path) {
    let moi = std::env::current_exe().expect("current_exe");
    let statut = std::process::Command::new(moi)
        .args([
            "--exact",
            "base_heritee_open_shared_write_refuse_nommee_sans_modifier",
            "--nocapture",
        ])
        .env(ROLE, "fabrique")
        .env(DOSSIER, dir)
        .env("OBRAIN_PROPS_V2", "0")
        .status()
        .expect("lancer le sous-processus");
    assert!(
        statut.success(),
        "le sous-processus de fabrication a échoué"
    );
}

/// Photographie complète du répertoire : nom -> octets. Sert à prouver qu'un
/// refus ne modifie rien.
fn photographier(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut carte = BTreeMap::new();
    for entree in std::fs::read_dir(dir).expect("lecture du répertoire") {
        let entree = entree.expect("entrée");
        if entree.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let nom = entree.file_name().to_string_lossy().into_owned();
            carte.insert(nom, std::fs::read(entree.path()).expect("lecture"));
        }
    }
    carte
}

#[test]
fn base_heritee_open_shared_write_refuse_nommee_sans_modifier() {
    if std::env::var(ROLE).as_deref() == Ok("fabrique") {
        let dir = std::path::PathBuf::from(std::env::var(DOSSIER).expect("dossier"));
        fabriquer_base_heritee(&dir);
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("heritee");
    std::fs::create_dir_all(&dir).expect("mkdir");
    fabriquer_dans_un_enfant(&dir);

    // Précondition de la fixture : le sidecar hérité porte bien la donnée.
    let taille_props = std::fs::metadata(dir.join("substrate.props"))
        .expect("substrate.props doit exister")
        .len();
    assert!(
        taille_props > 0,
        "fixture invalide : substrate.props est vide, le régime hérité n'a rien persisté"
    );

    let avant = photographier(&dir);
    let resultat = SubstrateStore::open_shared_write(&dir);
    let apres = photographier(&dir);

    match resultat {
        Err(SubstrateError::BaseHeritee { sidecar, .. }) => {
            assert_eq!(
                sidecar, "substrate.props",
                "le refus doit nommer le sidecar"
            );
        }
        Err(autre) => panic!("refus attendu BaseHeritee, reçu : {autre}"),
        Ok(_) => panic!(
            "open_shared_write a ACCEPTÉ une base à sidecar hérité non vide : \
             ses propriétés seraient écrasées par le premier pair"
        ),
    }
    assert_eq!(
        avant, apres,
        "un refus d'ouverture ne doit modifier aucun fichier du répertoire"
    );
}

#[test]
fn base_heritee_reste_ouvrable_en_exclusif() {
    // Le refus ne doit pas être une fermeture : le chemin de migration (ouverture
    // exclusive) reste ouvert sur la même base.
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("heritee-exclusive");
    std::fs::create_dir_all(&dir).expect("mkdir");
    fabriquer_dans_un_enfant(&dir);
    let store = SubstrateStore::open(&dir).expect("ouverture exclusive d'une base héritée");
    drop(store);
}

#[test]
fn base_neuve_open_shared_write_reste_ouverte() {
    // Garde de non-régression : le refus ne vise que les sidecars non vides.
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("neuve");
    std::fs::create_dir_all(&dir).expect("mkdir");
    {
        let store = SubstrateStore::create(&dir).expect("creation");
        store.flush().expect("flush");
    }
    let store = SubstrateStore::open_shared_write(&dir)
        .expect("une base neuve (sidecars vides) doit s'ouvrir en écriture partagée");
    drop(store);
}
