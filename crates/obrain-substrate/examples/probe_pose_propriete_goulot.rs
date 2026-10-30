//! `probe_pose_propriete_goulot` — décompose le coût unitaire d'écriture d'une
//! propriété scalaire dans le MODE MÉMOIRE (RAM), pour isoler le goulot restant
//! après que l'ouverture soit passée de ~6,5 ms (fichier) à ~50 µs (RAM).
//!
//! PROTOCOLE (mesure séquentielle, un seul process, médiane de N répétitions) :
//!   * Phase A — `create_node` : le coût de l'écriture « brute » d'un nœud.
//!   * Phase B — `set_node_property` avec UNE SEULE clé réutilisée (clé déjà
//!     internée après le premier appel) : le régime établi de l'écriture.
//!   * Phase C — `set_node_property` avec une clé NEUVE à chaque appel : même
//!     chemin + l'internement journalisé (`KeyIntern` au WAL + registre).
//!   * Phase D — `get_node_property` (lecture unitaire), pour le rapport écrit/lect.
//!   * Phase E — `get_node` (hydratation complète).
//!
//! Le GAP B→C isole le coût MARGINAL de l'internement ; B/A isole le surcoût de
//! l'écriture de propriété par rapport à l'écriture de nœud.

use std::hint::black_box;
use std::time::Instant;

use obrain_common::types::Value;
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

const N: usize = 2_000;
const REPS: usize = 51;

fn median_ns(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() {
    println!("probe_pose_propriete_goulot — HEAD {}", env_head());
    println!("mode MÉMOIRE (open_memory), N={N} nœuds, {REPS} répétitions (médiane)");
    println!();

    let store = SubstrateStore::open_memory().expect("store en mémoire");
    let ids: Vec<_> = (0..N).map(|_| store.create_node(&["Banc"])).collect();
    for (i, id) in ids.iter().enumerate() {
        store.set_node_property(*id, "rang", Value::Int64(i as i64));
    }

    // Phase D — lecture unitaire (clé "rang", écrite au montage ; nœuds à 1 prop).
    let k = obrain_common::types::PropertyKey::new("rang");
    let d = time_median(
        || {
            let t = Instant::now();
            let mut i = 0usize;
            for _ in 0..N {
                let id = ids[i % ids.len()];
                i += 1;
                black_box(store.get_node_property(id, &k));
            }
            t.elapsed().as_nanos()
        },
        REPS,
    ) / N as u128;

    // Phase E — hydratation complète (nœuds à 1 prop, AVANT toute pollution).
    let e = time_median(
        || {
            let t = Instant::now();
            let mut i = 0usize;
            for _ in 0..N {
                let id = ids[i % ids.len()];
                i += 1;
                black_box(store.get_node(id).is_some());
            }
            t.elapsed().as_nanos()
        },
        REPS,
    ) / N as u128;

    // Phase A — création de nœud (boucle de N par répétition).
    let a_total = time_median(
        || {
            let t = Instant::now();
            for _ in 0..N {
                black_box(store.create_node(&["Banc"]));
            }
            t.elapsed().as_nanos()
        },
        REPS,
    );
    let a = a_total / N as u128;

    // Phase B — écriture, clé fixe "compteur".
    let b = time_median(
        || {
            let t = Instant::now();
            let mut i = 0usize;
            for _ in 0..N {
                let id = ids[i % ids.len()];
                i += 1;
                store.set_node_property(id, "compteur", Value::Int64(i as i64));
                black_box(id);
            }
            t.elapsed().as_nanos()
        },
        REPS,
    ) / N as u128;

    // Phase C — écriture, clé NEUVE à chaque appel (internement journalisé).
    // Un compteur MONOTONE (partagé entre répétitions) garantit une clé jamais
    // vue : chaque appel paie réellement l'insert au registre + `KeyIntern` WAL.
    let mut seq = 0usize;
    let c = time_median(
        || {
            let t = Instant::now();
            for _ in 0..N {
                let id = ids[seq % ids.len()];
                seq += 1;
                store.set_node_property(id, &format!("cle_{seq}"), Value::Int64(seq as i64));
                black_box(id);
            }
            t.elapsed().as_nanos()
        },
        REPS,
    ) / N as u128;

    println!("{:<28} {:>10}", "bras", "ns/op");
    println!("{:<28} {:>10}", "A create_node", a);
    println!("{:<28} {:>10}", "B set_prop (clé fixe)", b);
    println!("{:<28} {:>10}", "C set_prop (clé neuve)", c);
    println!("{:<28} {:>10}", "D get_property (lecture)", d);
    println!("{:<28} {:>10}", "E get_node (hydratation)", e);
    println!();
    println!("surcoût écriture vs création      B/A  = {:.2}×", b as f64 / a as f64);
    println!("coût MARGINAL de l'internement     C−B  = {} ns", c.saturating_sub(b));
    println!("écriture vs lecture               B/D  = {:.2}×", b as f64 / d as f64);
}

fn env_head() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "?".into())
}

fn time_median<F: FnMut() -> u128>(mut f: F, reps: usize) -> u128 {
    let mut v = Vec::with_capacity(reps);
    for _ in 0..reps {
        v.push(f());
    }
    median_ns(v)
}
