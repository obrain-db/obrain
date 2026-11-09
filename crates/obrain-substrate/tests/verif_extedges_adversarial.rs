//! Verification ADVERSARIALE de la durabilite des aretes inter-bases (V1) :
//! rejeu double, insert+delete dans la meme fenetre WAL, sidecar corrompu,
//! filigrane d'ids depuis le sidecar seul.
//!
//! ```bash
//! cargo test -p obrain-substrate --test verif_extedges_adversarial
//! ```

use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

/// Rejeu DOUBLE : deux reouvertures successives rendent exactement les
/// memes ids (idempotence), et le filigrane interdit toute collision.
#[test]
fn p4_double_rejeu_idempotent_ids_et_filigrane() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a;
    let mut attendu: Vec<u64>;
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        a = store.create_node(&["Note"]);
        attendu = vec![
            store
                .create_external_edge(a, "T", None, "b", "k1")
                .expect("e1")
                .0,
            store
                .create_external_edge(a, "T", None, "b", "k2")
                .expect("e2")
                .0,
        ];
        attendu.sort_unstable();
        // PAS de flush : la fenetre WAL seule porte tout.
        drop(store);
    }
    {
        let store = SubstrateStore::open(tmp.path()).expect("reouverture 1");
        let mut v: Vec<u64> = store
            .external_edges_from(a)
            .iter()
            .map(|e| e.id.0)
            .collect();
        v.sort_unstable();
        assert_eq!(v, attendu, "premier rejeu : les deux aretes, memes ids");
        assert_eq!(store.external_edge_count(), 2);
        drop(store);
    }
    {
        let store = SubstrateStore::open(tmp.path()).expect("reouverture 2");
        let mut v: Vec<u64> = store
            .external_edges_from(a)
            .iter()
            .map(|e| e.id.0)
            .collect();
        v.sort_unstable();
        assert_eq!(
            v, attendu,
            "second rejeu : memes ids, aucune duplication (idempotence)"
        );
        assert_eq!(store.external_edge_count(), 2);
        let neuf = store
            .create_external_edge(a, "T", None, "b", "k3")
            .expect("neuf");
        assert!(
            !attendu.contains(&neuf.0),
            "le filigrane doit interdire la collision d'ids apres double rejeu"
        );
    }
}

/// Insert PUIS delete dans la meme fenetre WAL (aucun flush) : au rejeu
/// l'arete ne doit pas exister, et son id ne doit pas etre realloue.
#[test]
fn p4_insert_puis_delete_dans_la_meme_fenetre_wal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a;
    let ephemere: u64;
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        a = store.create_node(&["Note"]);
        let e = store
            .create_external_edge(a, "T", None, "b", "k1")
            .expect("e1");
        ephemere = e.0;
        assert!(store.delete_external_edge(e));
        drop(store);
    }
    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.external_edge_count(),
        0,
        "l'arete inseree puis supprimee dans la fenetre WAL ne doit pas revenir"
    );
    assert_eq!(store.external_inbound_refs("b", "k1").len(), 0);
    let neuf = store
        .create_external_edge(a, "T", None, "b", "k2")
        .expect("neuf");
    assert_ne!(
        neuf.0, ephemere,
        "l'id de l'arete supprimee ne doit pas etre realloue (filigrane via Delete)"
    );
}

/// Snapshot porte l'arete, puis DETACH DELETE (noeud + externes) seulement
/// dans la fenetre WAL, kill : rien ne doit ressusciter.
#[test]
fn p4_detach_delete_apres_flush_kill_l_arete_ne_revient_pas() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a;
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        store.definir_seuil_checkpoint(0);
        a = store.create_node(&["Note"]);
        store
            .create_external_edge(a, "DOCUMENTS", None, "cible", "k1")
            .expect("e1");
        store.flush().expect("flush"); // le sidecar porte l'arete
        store.delete_node_edges(a); // fenetre WAL seule
        assert!(store.delete_node(a));
        drop(store);
    }
    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(
        store.external_edge_count(),
        0,
        "l'arete du snapshot supprimee ensuite (fenetre WAL) ne doit pas ressusciter"
    );
    assert_eq!(store.external_inbound_refs("cible", "k1").len(), 0);
    assert!(
        store.external_edges_from(a).is_empty(),
        "résultat attendu vide"
    );
}

/// Sidecar TRONQUE : l'ouverture ne doit jamais etre fatale, et le
/// registre doit rester fonctionnel (nouvelles ecritures possibles).
#[test]
fn p4_sidecar_tronque_jamais_fatal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        let a = store.create_node(&["Note"]);
        store
            .create_external_edge(a, "T", None, "b", "k1")
            .expect("e1");
        store
            .create_external_edge(a, "T", None, "b", "k2")
            .expect("e2");
        store.flush().expect("flush");
        store.checkpoint().expect("checkpoint"); // WAL tronque : sidecar seul
        drop(store);
    }
    let chemin = tmp.path().join("substrate.extedges");
    let octets = std::fs::read(&chemin).expect("lire sidecar");
    assert!(!octets.is_empty(), "résultat attendu non vide");
    std::fs::write(&chemin, &octets[..octets.len() / 2]).expect("tronquer");

    let store = SubstrateStore::open(tmp.path()).expect("ouverture JAMAIS fatale");
    // Perte assumee (le WAL est tronque par le checkpoint) mais le store vit.
    let n = store.create_node(&["X"]);
    let e = store
        .create_external_edge(n, "T", None, "b", "k_neuf")
        .expect("le registre doit refonctionner apres un sidecar illisible");
    assert!(store.delete_external_edge(e));
}

/// Sidecar remplace par du bruit : ouverture non fatale.
#[test]
fn p4_sidecar_bruit_jamais_fatal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        let a = store.create_node(&["Note"]);
        store
            .create_external_edge(a, "T", None, "b", "k1")
            .expect("e1");
        store.flush().expect("flush");
        drop(store);
    }
    let chemin = tmp.path().join("substrate.extedges");
    let bruit: Vec<u8> = (0..4096u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    std::fs::write(&chemin, &bruit).expect("ecrire bruit");
    let store = SubstrateStore::open(tmp.path()).expect("ouverture JAMAIS fatale");
    // La fenetre WAL (non checkpointee ici) re-fusionne ce qu'elle couvre :
    // l'arete peut revenir par le WAL — l'essentiel est : pas de panique,
    // et un etat coherent.
    let count = store.external_edge_count();
    let par_noeud: u64 = store
        .node_ids()
        .iter()
        .map(|n| store.external_edges_from(*n).len() as u64)
        .sum();
    assert_eq!(count, par_noeud, "etat coherent apres sidecar corrompu");
}

/// Filigrane restaure depuis le SIDECAR SEUL (WAL tronque par le
/// checkpoint) : une nouvelle arete ne collisionne jamais.
#[test]
fn p4_filigrane_depuis_le_sidecar_seul() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut deja: Vec<u64> = Vec::new();
    {
        let store = SubstrateStore::create(tmp.path()).expect("creation");
        let a = store.create_node(&["Note"]);
        for i in 0..3 {
            deja.push(
                store
                    .create_external_edge(a, "T", None, "b", &format!("k{i}"))
                    .expect("creation")
                    .0,
            );
        }
        store.flush().expect("flush");
        store.checkpoint().expect("checkpoint");
        drop(store);
    }
    let store = SubstrateStore::open(tmp.path()).expect("reouverture");
    assert_eq!(store.external_edge_count(), 3);
    let a = store.node_ids()[0];
    let neuf = store
        .create_external_edge(a, "T", None, "b", "k_neuf")
        .expect("creation");
    assert!(
        !deja.contains(&neuf.0),
        "filigrane du sidecar seul : jamais de collision d'ids"
    );
}
