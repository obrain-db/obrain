//! Checkpoint automatique : le WAL cesse de croitre sans borne.
//!
//! CONSTAT DU 14/08 sur la base de production de l'agent : `substrate.wal` =
//! 12,8 Go, en croissance depuis toujours, avec un fsync par commit — et rien
//! qui le consomme, puisque `replay_from()` n'a aucun appelant hors tests.
//!
//! CAUSE : `checkpoint()` existe et TRONQUE le WAL, mais ses seuls appelants
//! etaient un test, le convertisseur de migration et le REPL. Confier la
//! periodicite a l'appelant, c'est donner a chacun le droit d'oublier — et
//! tous avaient oublie.
//!
//! Ces tests verifient les deux proprietes qui comptent : le WAL redescend, et
//! AUCUNE donnee ne disparait au passage.

use std::sync::Arc;

use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Taille du WAL sur disque.
fn taille_wal(dir: &std::path::Path) -> u64 {
    std::fs::metadata(dir.join("substrate.wal"))
        .map(|m| m.len())
        .unwrap_or(0)
}

#[test]
fn le_checkpoint_tronque_le_wal_sans_perdre_de_donnees() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let store = SubstrateStore::create(tmp.path()).expect("creation");
    // Seuil bas pour declencher sans ecrire des centaines de Mo. Regle SUR LA
    // BASE et non par variable d'environnement : `cargo test` execute les
    // tests en parallele dans un meme processus, et trois tests qui reglent la
    // meme variable globale se la volent. C'est exactement ce qui a fait
    // echouer la premiere version de ce fichier.
    store.definir_seuil_checkpoint(32768);

    // Assez d'ecritures pour depasser le seuil de 32 Kio.
    let mut ids = Vec::new();
    for i in 0..400u64 {
        let n = store.create_node(&["Doc"]);
        store.set_node_property(
            n,
            "titre",
            obrain_common::types::Value::String(format!("document {i}").into()),
        );
        ids.push(n);
    }

    let avant = taille_wal(tmp.path());
    assert!(
        avant > 32768,
        "le WAL doit avoir depasse le seuil pour que le test ait un sens (il fait {avant})"
    );

    store.flush().expect("flush declenche le checkpoint");

    let apres = taille_wal(tmp.path());
    assert!(
        apres < avant,
        "le WAL doit avoir ete tronque : {avant} -> {apres}"
    );

    // ── ET SURTOUT : RIEN N'A DISPARU ────────────────────────────────────
    // Un checkpoint qui tronque en perdant des donnees serait bien pire que
    // le WAL qui grossit.
    assert_eq!(
        store.nodes_by_label("Doc").len(),
        400,
        "les 400 noeuds doivent survivre au checkpoint"
    );
    let cle = obrain_common::types::PropertyKey::new("titre");
    assert_eq!(
        store.get_node_property(ids[42], &cle),
        Some(obrain_common::types::Value::String("document 42".into())),
        "les proprietes doivent survivre au checkpoint"
    );
}

/// Le seuil a zero desactive le mecanisme — un exploitant doit pouvoir le
/// couper sans recompiler.
#[test]
fn seuil_a_zero_desactive_le_checkpoint() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let store = SubstrateStore::create(tmp.path()).expect("creation");
    store.definir_seuil_checkpoint(0);
    for i in 0..300u64 {
        let n = store.create_node(&["Doc"]);
        store.set_node_property(n, "x", obrain_common::types::Value::Int64(i as i64));
    }
    let avant = taille_wal(tmp.path());
    store.flush().expect("flush");
    let apres = taille_wal(tmp.path());

    assert!(
        apres >= avant,
        "avec un seuil a zero, le WAL ne doit PAS etre tronque ({avant} -> {apres})"
    );
}

/// Ecritures concurrentes PENDANT un checkpoint : ni panique, ni interblocage,
/// ni perte.
///
/// C'est la propriete que `checkpoint()` exigeait de son appelant sans jamais
/// l'appliquer — « the caller MUST ensure no concurrent writes are in
/// flight ». L'exclusion est maintenant tenue par le substrat lui-meme.
#[test]
fn checkpoint_concurrent_aux_ecritures() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let store = Arc::new(SubstrateStore::create(tmp.path()).expect("creation"));
    store.definir_seuil_checkpoint(32768);

    std::thread::scope(|s| {
        // Deux fils qui ecrivent sans relache.
        for f in 0..2u64 {
            let store = Arc::clone(&store);
            s.spawn(move || {
                for i in 0..300u64 {
                    let n = store.create_node(&["Doc"]);
                    store.set_node_property(
                        n,
                        "k",
                        obrain_common::types::Value::Int64((f * 1000 + i) as i64),
                    );
                }
            });
        }
        // Et un fil qui checkpointe en parallele.
        let store = Arc::clone(&store);
        s.spawn(move || {
            for _ in 0..5 {
                let _ = store.checkpoint();
                std::thread::yield_now();
            }
        });
    });

    // Toutes les ecritures acceptees doivent etre la.
    assert_eq!(
        store.nodes_by_label("Doc").len(),
        600,
        "aucune ecriture ne doit disparaitre a cause d'un checkpoint concurrent"
    );
}

/// (2026-08-31) SHRINK DE LA ZONE v2 — compact ne doit PAS perdre de données.
///
/// La zone `substrate.props.v2` est pré-allouée par croissance exponentielle
/// ×1.5 (167 Mo pour 26 Mo live sur un graphe de 6,4k nœuds). `compact`
/// appelle `shrink_props_zone` qui tronque le padding zéro-init. Ce test
/// vérifie que les propriétés survivent au shrink : écriture → shrink →
/// réouverture → lecture identique.
#[test]
fn le_shrink_zone_v2_ne_perd_pas_de_proprietes() {
    use obrain_common::types::Value;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    // Créer la base, écrire des nœuds avec propriétés scalaires (elles
    // atterrissent dans la zone v2).
    let store = SubstrateStore::create(dir).expect("creation");
    let mut expected = std::collections::HashMap::new();
    for i in 0..200u64 {
        let n = store.create_node(&["Doc"]);
        let titre = format!("document {i}");
        store.set_node_property(n, "titre", Value::String(titre.clone().into()));
        expected.insert(n, titre);
    }
    store.flush().expect("flush initial");

    let props_path = dir.join("substrate.props.v2");
    let taille_avant = std::fs::metadata(&props_path).map(|m| m.len()).unwrap_or(0);
    assert!(taille_avant > 0, "la zone v2 doit exister");

    // Shrink (le geste de compact) puis réouverture.
    store.shrink_props_zone().expect("shrink");
    store.flush().expect("flush après shrink");
    drop(store);

    let taille_apres = std::fs::metadata(&props_path).map(|m| m.len()).unwrap_or(0);
    assert!(
        taille_apres <= taille_avant,
        "le shrink ne doit pas agrandir la zone (avant={taille_avant}, après={taille_apres})"
    );

    // Réouverture : toutes les propriétés doivent être intactes.
    let store = SubstrateStore::open(dir).expect("réouverture");
    for (n, titre) in &expected {
        let lu = store.get_node_property(*n, &obrain_common::types::PropertyKey::new("titre"));
        match lu {
            Some(Value::String(s)) => assert_eq!(s.as_str(), titre, "titre perdu au shrink"),
            other => panic!("titre attendu String, lu {:?}", other),
        }
    }
}
