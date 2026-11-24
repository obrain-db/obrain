//! (2026-11-24) EXPÉRIENCE COMMUNE — trancher la contradiction WAL.
//!
//! Deux mesures se contredisent sur la durabilité des synapses :
//!   1. banc d'injection (dgx, b8ee6f87) : « WAL tronqué avant réouverture →
//!      0 synapse » ⇒ « les synapses ne tiennent que par le WAL » ;
//!   2. test d'invariant : « WAL perdu, colonne intacte → poids toujours lu ».
//!
//! Cette expérience part du MÊME état (étoile hub—2 feuilles, 2 synapses),
//! applique la MÊME frontière de durabilité (drop sans close() : les octets
//! vivent dans les pages sales du mmap), puis distingue explicitement :
//!   (a) les DONNÉES DE COLONNE — `scan_typed_edge_weights(SYNAPSE)` lit les
//!       EdgeRecords de la zone Edges (le poids `weight_u16`, durable WAL+mmap) ;
//!   (b) l'INCIDENCE — `synapse_edges_of(node)` lit la zone nommée
//!       `substrate.synapse_incidence` + `substrate.synapse_postings` ;
//!   (c) la LECTURE PUBLIQUE — `len()`, `list_synapses(node)`,
//!       `for_each_synapse(node)`.
//!
//! Run: cargo test --release -p obrain-cognitive --test wal_synapse_durabilite
//!        --features synapse,substrate -- --nocapture
#![cfg(all(feature = "synapse", feature = "substrate"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use obrain_cognitive::synapse::{SynapseConfig, SynapseStore};
use obrain_common::types::NodeId;
use obrain_substrate::SubstrateStore;

fn liste_fichiers(dir: &Path) -> Vec<(String, u64)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            v.push((name, len));
        }
    }
    v.sort();
    v
}

/// Étoile hub—{a,b} construite, 2 synapses, puis drop SANS close().
fn construire(td: &tempfile::TempDir) -> (NodeId, NodeId, NodeId) {
    let chemin = td.path().join("kb");
    let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create"));
    let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
    let hub = gs.create_node(&["n"]);
    let a = gs.create_node(&["n"]);
    let b = gs.create_node(&["n"]);
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
    store.reinforce(hub, a, 0.35);
    store.reinforce(hub, b, 0.35);
    (hub, a, b)
}

/// (a) données de colonne, (b) incidence, (c) lecture publique — en trois
/// lignes claires.
fn trois_colonnes(chemin: &Path, hub: NodeId) {
    let substrate = Arc::new(SubstrateStore::open(chemin).expect("open"));
    let (a_col, a_inc) = {
        let col = substrate.scan_typed_edge_weights("SYNAPSE");
        let inc = substrate.synapse_edges_of(hub);
        (
            col.map(|v| v.len()).unwrap_or_else(|e| {
                println!("  (a) ERREUR colonne: {e}");
                999_999
            }),
            inc.map(|v| v.len()).unwrap_or_else(|e| {
                println!("  (b) ERREUR incidence: {e}");
                999_999
            }),
        )
    };
    let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
    let mut c3 = 0usize;
    store.for_each_synapse(hub, |_, _| c3 += 1);
    println!(
        "  (a) colonne scan_typed_edge_weights = {a_col}   (b) incidence synapse_edges_of = {a_inc}   (c) len={} list={} for_each={c3}",
        store.len(),
        store.list_synapses(hub).len(),
    );
}

#[test]
fn wal_tronque_vs_colonne_vs_incidence() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");
    let (hub, _a, _b) = construire(&td);

    println!("== fichiers après construction (drop sans close) ==");
    for (n, l) in liste_fichiers(&chemin) {
        println!("   {n}  {l} octets");
    }

    // ── Scénario CONTROL : réouverture directe ─────────────────────────
    println!("== CONTROL (aucune injection) ==");
    trois_colonnes(&chemin, hub);

    // ── Scénario WAL TRONQUÉ : on vide substrate.wal avant réouverture ─
    let wal: PathBuf = chemin.join("substrate.wal");
    let avant = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    std::fs::write(&wal, b"").expect("tronquer wal");
    let apres = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    println!("== WAL TRONQUÉ : substrate.wal {avant} -> {apres} octets ==");
    trois_colonnes(&chemin, hub);

    // ── Scénario INCIDENCE TRONQUÉE : on vide les deux zones nommées ──
    for f in ["substrate.synapse_incidence", "substrate.synapse_postings"] {
        let p: PathBuf = chemin.join(f);
        let av = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        std::fs::write(&p, b"").expect("tronquer incidence");
        let ap = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        println!("== INCIDENCE TRONQUÉE : {f} {av} -> {ap} octets ==");
    }
    trois_colonnes(&chemin, hub);
}
