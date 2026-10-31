//! CONTRAT J5 (C6) — contre-epreuve BUG-PROPS-TRUNCATE.
//!
//! Le bug connu (couche props WIP d'une AUTRE lignee) : a l'open, les
//! colonnes blob sont TRONQUEES au layout sequentiel. Cette lignee ne doit
//! PAS avoir ce comportement — c'est un point de contrat de la bascule v3 :
//! ouvrir puis fermer un store, sans rien ecrire, ne change NI la taille
//! des fichiers porteurs de proprietes (props, blobcol, strings, dict) NI
//! le contenu relu.
//!
//! Le banc : un store v3 neuf, des props INLINE (courtes) et des props
//! routees BLOBCOL (payload > BLOB_COLUMN_THRESHOLD_BYTES), un cycle de
//! stabilisation (le premier open/close peut fusionner du WAL), puis TROIS
//! cycles open/close purs — chaque cycle relit tout et compare, et les
//! tailles disque sont strictement identiques d'un cycle a l'autre.
//!
//!   cargo test -p obrain-substrate --test contrat_j5_props_truncate

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{BLOB_COLUMN_THRESHOLD_BYTES, SubstrateStore};
use std::collections::BTreeMap;
use std::path::Path;

/// Nombre de noeuds du banc.
const N: u64 = 20;

/// Tailles des fichiers PORTEURS DE PROPRIETES du repertoire — le perimetre
/// exact du contrat de stabilite. (checkpoint/wal/meta bougent legitimement
/// a l'open/close ; props, blobcol, strings et dict, JAMAIS.)
fn tailles_props(dir: &Path) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let nom = e.file_name().to_string_lossy().into_owned();
        let porteur = nom.starts_with("substrate.props")
            || nom.starts_with("substrate.blobcol.")
            || nom == "substrate.strings"
            || nom == "substrate.dict";
        if porteur {
            m.insert(nom, e.metadata().unwrap().len());
        }
    }
    m
}

/// Relit toutes les props du banc et les rend sous forme comparable.
fn relire(store: &SubstrateStore) -> Vec<(u64, Option<Value>, Option<Value>)> {
    (1..=N)
        .map(|i| {
            let id = NodeId::new(i);
            (
                i,
                store.get_node_property(id, &PropertyKey::new("courte")),
                store.get_node_property(id, &PropertyKey::new("grosse")),
            )
        })
        .collect()
}

#[test]
fn open_close_ne_tronque_ni_ne_perd_les_props() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();

    // Payload largement au-dessus du seuil de routage blobcol.
    let gros = "x".repeat(BLOB_COLUMN_THRESHOLD_BYTES * 40);

    // --- Le banc : N noeuds, une prop inline + une prop blobcol chacun.
    {
        let store = SubstrateStore::create(dir).expect("create");
        for i in 1..=N {
            let n = store.create_node(&["Doc"]);
            store.set_node_property(n, "courte", Value::String(format!("v{i}").into()));
            store.set_node_property(n, "grosse", Value::String(format!("{gros}-{i}").into()));
        }
        store.flush().expect("flush");
    }

    // --- Stabilisation : un premier open/close (fusion WAL eventuelle).
    let attendu = {
        let store = SubstrateStore::open(dir).expect("open stabilisation");
        relire(&store)
    };
    // Garde d'instrument : le banc doit REELLEMENT exercer la route blobcol.
    let t0 = tailles_props(dir);
    assert!(
        t0.keys().any(|n| n.starts_with("substrate.blobcol.node.")),
        "le banc n'a produit aucune colonne blob — le test ne prouve rien : {t0:?}"
    );
    // ... et les valeurs doivent etre la.
    for (i, courte, grosse) in &attendu {
        assert!(courte.is_some(), "noeud {i} : prop inline absente");
        assert!(grosse.is_some(), "noeud {i} : prop blobcol absente");
    }

    // --- TROIS cycles open/close purs : tailles figees, contenu identique.
    for cycle in 1..=3 {
        let store = SubstrateStore::open(dir).unwrap_or_else(|e| {
            panic!("cycle {cycle} : open a echoue : {e}");
        });
        let relu = relire(&store);
        assert_eq!(relu, attendu, "cycle {cycle} : les props relues ont change");
        drop(store);
        let t = tailles_props(dir);
        assert_eq!(
            t, t0,
            "cycle {cycle} : une zone porteuse de props a change de taille \
             (BUG-PROPS-TRUNCATE ou grossissement parasite)"
        );
    }
}
