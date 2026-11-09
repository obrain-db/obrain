//! Aretes inter-bases (V1) — durabilite reelle : on tue sans flush, on
//! rouvre, on compte. Modele : `wal_recovery_reelle.rs`.
//!
//! Ces tests couvrent le contrat de la spec << aretes inter-bases natives >>
//! paragraphe 1 : l'arete externe vit dans le store SOURCE (une seule WAL), elle
//! survit au kill-sans-flush comme une arete normale, sa suppression ne
//! RESSUSCITE pas au rejeu, et DETACH DELETE l'emporte avec son noeud.

use obrain_common::types::{EdgeId, NodeId};
use obrain_core::graph::{EXTERNAL_EDGE_ID_BIT, GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Toutes les aretes externes du store, triees par id — pour comparer des
/// etats complets et pas seulement des comptes.
fn toutes_les_externes(store: &SubstrateStore) -> Vec<(u64, u64, String, String, String)> {
    let mut out = Vec::new();
    for nid in store.node_ids() {
        for e in store.external_edges_from(nid) {
            out.push((
                e.id.0,
                e.from.0,
                e.edge_type.clone(),
                e.to_store.clone(),
                e.to_key.clone(),
            ));
        }
    }
    out.sort();
    out
}

#[test]
fn les_aretes_externes_survivent_au_kill_sans_flush() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (a, b);
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        a = store.create_node(&["Note"]);
        b = store.create_node(&["Note"]);
        let e1 = store
            .create_external_edge(a, "DOCUMENTS", None, "agent/system", "note_6b608fa4")
            .expect("backend substrate doit supporter les aretes externes");
        assert_ne!(e1.0 & EXTERNAL_EDGE_ID_BIT, 0, "bit haut pose");
        store
            .create_external_edge(b, "REFERS_TO", None, "task_7problems", "42")
            .expect("creation");
        // Champ hote RESERVE multi-host : encode des aujourd'hui, resolu
        // jamais (V5). Il doit survivre au rejeu comme le reste.
        store
            .create_external_edge(a, "MIRRORS", Some("dgx-2"), "agent/system", "note_x")
            .expect("creation");
        // PAS de flush : c'est tout l'objet du test.
        drop(store);
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.external_edge_count(),
        3,
        "les 3 aretes externes doivent revenir"
    );

    let de_a = store.external_edges_from(a);
    assert_eq!(de_a.len(), 2);
    let doc = de_a
        .iter()
        .find(|e| e.edge_type == "DOCUMENTS")
        .expect("DOCUMENTS");
    assert_eq!(doc.to_store, "agent/system");
    assert_eq!(doc.to_key, "note_6b608fa4");
    assert_eq!(doc.to_host, None);
    let mir = de_a
        .iter()
        .find(|e| e.edge_type == "MIRRORS")
        .expect("MIRRORS");
    assert_eq!(
        mir.to_host.as_deref(),
        Some("dgx-2"),
        "l'hote encode doit survivre"
    );

    // Registre inverse reconstruit : qui pointe vers agent/system:note_6b608fa4 ?
    let inbound = store.external_inbound_refs("agent/system", "note_6b608fa4");
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].0, a);
}

#[test]
fn une_suppression_apres_flush_ne_ressuscite_pas_au_rejeu() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let garde: EdgeId;
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        let a = store.create_node(&["Note"]);
        let e1 = store
            .create_external_edge(a, "DOCUMENTS", None, "b1", "k1")
            .expect("creation");
        garde = store
            .create_external_edge(a, "DOCUMENTS", None, "b2", "k2")
            .expect("creation");
        // Le snapshot sidecar contient les DEUX aretes...
        store.flush().expect("flush");
        // ... puis la suppression n'existe que dans la fenetre WAL.
        assert!(store.delete_external_edge(e1));
        drop(store);
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    let restantes = toutes_les_externes(&store);
    assert_eq!(
        restantes.len(),
        1,
        "l'arete supprimee apres flush ne doit PAS ressusciter au rejeu"
    );
    assert_eq!(restantes[0].0, garde.0);
    assert!(
        store.external_inbound_refs("b1", "k1").is_empty(),
        "le registre inverse ne doit plus porter la ref supprimee"
    );
}

#[test]
fn detach_delete_emporte_les_externes_sortantes_et_survit_au_kill() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (a, b);
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        a = store.create_node(&["Note"]);
        b = store.create_node(&["Note"]);
        store
            .create_external_edge(a, "DOCUMENTS", None, "cible", "k1")
            .expect("creation");
        store
            .create_external_edge(a, "REFERS_TO", None, "cible", "k2")
            .expect("creation");
        store
            .create_external_edge(b, "DOCUMENTS", None, "cible", "k3")
            .expect("creation");
        // DETACH DELETE de a : ses externes SORTANTES partent avec lui
        // (elles vivent chez lui — spec paragraphe 4) ; celles de b restent.
        store.delete_node_edges(a);
        assert!(store.delete_node(a));
        assert_eq!(store.external_edges_from(a).len(), 0);
        assert_eq!(store.external_edge_count(), 1);
        drop(store);
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.external_edge_count(),
        1,
        "apres kill-sans-flush, seule l'arete de b doit revenir"
    );
    assert_eq!(store.external_edges_from(a).len(), 0);
    assert_eq!(store.external_edges_from(b).len(), 1);
}

#[test]
fn le_filigrane_d_ids_est_restaure_au_rejeu() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let deja: Vec<u64>;
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        let a = store.create_node(&["Note"]);
        deja = (0..3)
            .map(|i| {
                store
                    .create_external_edge(a, "T", None, "b", &format!("k{i}"))
                    .expect("creation")
                    .0
            })
            .collect();
        drop(store);
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    let a = store.node_ids()[0];
    let neuf = store
        .create_external_edge(a, "T", None, "b", "k_neuf")
        .expect("creation");
    assert!(
        !deja.contains(&neuf.0),
        "un id d'arete externe ne doit JAMAIS etre realloue apres rejeu \
         (filigrane fetch_max) — collision = ecrasement silencieux"
    );
}

#[test]
fn le_sidecar_seul_suffit_apres_flush_et_checkpoint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        let a = store.create_node(&["Note"]);
        store
            .create_external_edge(a, "DOCUMENTS", None, "cible", "k1")
            .expect("creation");
        store.flush().expect("flush");
        // Checkpoint = troncature du WAL : la fenetre de rejeu disparait,
        // seul le sidecar porte encore le registre.
        store.checkpoint().expect("checkpoint");
        drop(store);
    }

    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.external_edge_count(),
        1,
        "apres checkpoint (WAL tronque), le sidecar doit suffire"
    );
}

#[test]
fn delete_edge_route_le_bit_haut_vers_le_registre_externe() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = SubstrateStore::create(tmp.path()).expect("creation");
    let a = store.create_node(&["Note"]);
    let b = store.create_node(&["Note"]);
    let interne = store.create_edge(a, b, "KNOWS");
    let externe = store
        .create_external_edge(a, "DOCUMENTS", None, "cible", "k")
        .expect("creation");
    // delete_edge sur un id a bit haut supprime l'externe, pas l'interne.
    assert!(store.delete_edge(externe));
    assert_eq!(store.external_edge_count(), 0);
    assert!(
        store.get_edge(interne).is_some(),
        "l'arete interne ne bouge pas"
    );
    // Idempotence.
    assert!(!store.delete_edge(externe));
    // Un id externe ne resout jamais un get_edge interne.
    assert!(store.get_edge(EdgeId(externe.0)).is_none());
    let _ = NodeId(0);
}
