//! (2026-11-24) RÉGRESSION — le nom de type d'arête « SYNAPSE » doit survivre
//! à une panne AVANT flush, même quand le WAL de données est tronqué.
//!
//! Constat mesuré (banc d'injection dgx, b8ee6f87 + integ/cognitive-lot-1) :
//! après troncature du WAL de données, `for_each_synapse` rend 2 arêtes (l'index
//! d'incidence `substrate.synapse_incidence`/`postings` survit) mais
//! `list_synapses`/`len`/`snapshot` rendent 0 : ces trois chemins résolvent le
//! NOM « SYNAPSE » en id via `EdgeTypeRegistry::id_for`, et ce dictionnaire
//! nom→id n'est persisté (`substrate.dict`) qu'à `flush()`. L'enregistrement
//! `EdgeTypeIntern` du WAL le journalise à l'internement, mais une troncature
//! du WAL (perte des écritures non durables, panne avant flush) l'emporte.
//!
//! Ce test est la forme « assertion » de l'expérience `wal_synapse_durabilite.rs`.
//!
//! Run: cargo test -p obrain-cognitive --test recovery_synapse \
//!        --features synapse,substrate -- --nocapture
#![cfg(all(feature = "synapse", feature = "substrate"))]

use std::path::PathBuf;
use std::sync::Arc;

use obrain_cognitive::synapse::{SynapseConfig, SynapseStore};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;

/// Étoile hub—{a,b} construite, 2 synapses, puis `drop` SANS `close()` ni
/// `flush()` — la frontière de durabilité de la panne : les octets vivent dans
/// les pages sales du mmap (ils survivent à la mort du processus, pas à une
/// perte des écritures non durables).
fn construire(td: &tempfile::TempDir) -> (NodeId, NodeId, NodeId) {
    let chemin = td.path().join("kb");
    let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create"));
    let gs: Arc<dyn GraphStoreMut> = substrate.clone();
    let hub = gs.create_node(&["n"]);
    let a = gs.create_node(&["n"]);
    let b = gs.create_node(&["n"]);
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
    store.reinforce(hub, a, 0.35);
    store.reinforce(hub, b, 0.35);
    (hub, a, b)
}

#[test]
fn le_nom_synapse_survit_a_la_troncature_du_wal_avant_flush() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");
    let (hub, _a, _b) = construire(&td);

    // ── Panne avant flush : le WAL de données est tronqué ─────────────────
    // On imite la perte des écritures NON DURABLES (pas un kill -9 : un kill
    // préserve le cache de pages du noyau). Les zones mmap (nœuds, arêtes,
    // incidence) restent, mais `substrate.wal` — donc l'EdgeTypeIntern qui
    // journalisait le nom « SYNAPSE » — disparaît.
    let wal: PathBuf = chemin.join("substrate.wal");
    let avant = std::fs::metadata(&wal).map_or(0, |m| m.len());
    assert!(
        avant > 0,
        "le WAL doit porter des écritures avant la troncature"
    );
    std::fs::write(&wal, b"").expect("tronquer le WAL de données");

    // ── Réouverture ───────────────────────────────────────────────────────
    let substrate = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn GraphStoreMut> = substrate.clone();
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);

    // Les trois chemins de lecture PAR NOM doivent voir les 2 synapses —
    // c'est le défaut : ils résolvent « SYNAPSE » via le dictionnaire nom→id.
    assert_eq!(
        store.len(),
        2,
        "len() (scan_typed_edge_weights par nom) doit voir les 2 synapses"
    );
    assert_eq!(
        store.snapshot().len(),
        2,
        "snapshot() (scan_typed_edge_weights par nom) doit voir les 2 synapses"
    );
    assert_eq!(
        store.list_synapses(hub).len(),
        2,
        "list_synapses(hub) (adjacence par nom de type) doit voir les 2 synapses"
    );

    // Le chemin d'incidence (for_each_synapse) sert de témoin : il lit la zone
    // nommée, sans passer par le nom de type, et voyait déjà 2 avant le correctif.
    let mut emis = 0usize;
    store.for_each_synapse(hub, |_, _| emis += 1);
    assert_eq!(emis, 2, "for_each_synapse(hub) doit émettre 2 arêtes");
}

/// (2026-11-24) L'index d'incidence des synapses survit à `checkpoint()`.
///
/// `create_synapse_slot` écrit l'incidence (répertoire + postings) par un append
/// mmap DIRECT, sans enregistrement WAL. Avant le correctif, `checkpoint()` (qui
/// tronque le WAL) ne fsyncait PAS ces deux side-cars nommés : les arêtes SYNAPSE
/// étaient durables (zones fsyncées) mais l'incidence ne l'était pas — une
/// coupure de courant après checkpoint perdait donc `for_each_synapse` (lecture
/// incidence) tout en gardant `len()`/`snapshot()` (lecture colonne d'arêtes).
/// La coupure de courant (perte du cache de pages) n'est pas simulable en
/// processus ; ce test fige l'invariant observable : un checkpoint + réouverture
/// préserve les DEUX chemins de lecture.
#[test]
fn le_checkpoint_preserve_l_incidence_des_synapses() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");
    let hub;
    {
        let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn GraphStoreMut> = substrate.clone();
        let h = gs.create_node(&["n"]);
        let a = gs.create_node(&["n"]);
        let b = gs.create_node(&["n"]);
        hub = h;
        let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate.clone());
        store.reinforce(h, a, 0.35);
        store.reinforce(h, b, 0.35);
        // Checkpoint = flush (zones + dict) + troncature du WAL.
        substrate.checkpoint().expect("checkpoint");
        // Drop SANS close : le checkpoint a déjà rendu tout durable.
    }

    let substrate = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn GraphStoreMut> = substrate.clone();
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);

    // (a) Le chemin d'incidence (for_each_synapse) — celui que le checkpoint
    // ne fsyncait pas.
    let mut emis = 0usize;
    store.for_each_synapse(hub, |_, _| emis += 1);
    assert_eq!(
        emis, 2,
        "for_each_synapse(hub) doit retrouver 2 arêtes après checkpoint"
    );

    // (b) Les lectures par nom, pour symétrie.
    assert_eq!(store.len(), 2, "len() après checkpoint");
    assert_eq!(store.snapshot().len(), 2, "snapshot() après checkpoint");
    assert_eq!(
        store.list_synapses(hub).len(),
        2,
        "list_synapses(hub) après checkpoint"
    );
}
