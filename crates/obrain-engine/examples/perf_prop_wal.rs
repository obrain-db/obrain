//! Sonde datée — coût d'une écriture de propriété substrate (chemin v2).
//!
//! Contexte : RFC `docs/rfc/substrate/double-journalisation-wal-props.md`
//! (option C — ne plus appender `PropSetInline` quand la zone v2 a pris
//! l'écriture). Avant/après sur le MÊME binaire de la sonde :
//!
//! ```text
//! cargo run --release -p obrain-engine --example perf_prop_wal
//! ```
//!
//! Deux phases, toutes deux sur un store substrate réel (tempfile, v2 par
//! défaut sur base neuve — `OBRAIN_PROPS_V2` absent) :
//!
//!   - **neuve** : 1ʳᵉ propriété d'un nœud frais — tête de chaîne
//!     (`NodePropHeadUpdate` WAL-first) + append zone + append `PropSetInline`.
//!   - **chaude** : écrasement de la MÊME clé sur le même nœud — tête
//!     inchangée, donc le seul journal valeur = append zone + `PropSetInline` :
//!     c'est le signal le plus propre du retrait conditionnel (option C).
//!
//! Mesures datées : voir ledger-ameliorations-2026-09-04.md (ligne du chantier).
//! Établi le 2026-09-06 — lecture seule du chemin, aucun effet sur le code.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use web_time::Instant;

fn stats(label: &str, mut ns: Vec<u128>) {
    ns.sort_unstable();
    let n = ns.len();
    let sum: u128 = ns.iter().sum();
    let mean_us = sum as f64 / n.max(1) as f64 / 1000.0;
    let p50 = ns[n / 2] as f64 / 1000.0;
    let p95 = ns[(n as f64 * 0.95) as usize] as f64 / 1000.0;
    println!(
        "  {label:<28} n={n:<5} moy={mean_us:>7.2} µs  p50={p50:>6.2} µs  p95={p95:>6.2} µs"
    );
}

fn main() {
    const N_NEUVES: usize = 3_000;
    const N_CHAUDES: usize = 3_000;
    const RODAGE: usize = 200;

    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate(dir.path()).expect("open substrate");
    let store = db.graph_store_mut();

    // État stable : quelques nœuds + flush (zone allouée, sentinelle posée).
    for _ in 0..64 {
        store.create_node(&["P"]);
    }
    db.checkpoint().expect("flush/checkpoint initial");

    // ── Phase « neuve » : 1ʳᵉ prop d'un nœud frais ────────────────────────
    let nodes: Vec<_> = (0..N_NEUVES).map(|_| store.create_node(&["P"])).collect();
    for &n in nodes.iter().take(RODAGE) {
        store.set_node_property(n, "v", Value::Int64(0));
    }
    let mut neuves = Vec::with_capacity(N_NEUVES);
    for (i, &n) in nodes.iter().enumerate() {
        let t = Instant::now();
        store.set_node_property(n, "v", Value::Int64(i as i64));
        neuves.push(t.elapsed().as_nanos());
    }

    // ── Phase « chaude » : même clé, même nœud (tête inchangée) ───────────
    let hot = nodes[0];
    for _ in 0..RODAGE {
        store.set_node_property(hot, "v", Value::Int64(0));
    }
    let mut chaudes = Vec::with_capacity(N_CHAUDES);
    for i in 0..N_CHAUDES {
        let t = Instant::now();
        store.set_node_property(hot, "v", Value::Int64(i as i64));
        chaudes.push(t.elapsed().as_nanos());
    }

    println!("=== sonde perf_prop_wal (substrate v2, tempfile) ===");
    println!("base : {}", dir.path().display());
    stats("prop page neuve (1ʳᵉ prop)", neuves);
    stats("prop page chaude (même clé)", chaudes);
}
