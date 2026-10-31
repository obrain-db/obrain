//! # `replay_bench` — coût du rejeu WAL + relevé du filigrane à l'ouverture.
//!
//! Aspect disjoint : rejeu / reconstruction après crash. Mesure SÉPARÉMENT les
//! deux composantes du `replay_from` de `SubstrateStore::open` :
//!
//! 1. **décode+applique** — coût O(fenêtre WAL) : lire, décoder (CRC) et
//!    ré-appliquer chaque record au mmap de zone.
//! 2. **audit post-rejeu des ancres** — balayage linéaire O(slots totaux) de la
//!    zone nodes (comptage coquilles / ancres pendantes), exécuté à CHAQUE
//!    ouverture où le WAL n'est pas vide, quelle que soit la taille de la
//!    fenêtre rejouée.
//!
//! Le rapport des deux répond à la question : sur une grande base, qu'est-ce
//! qui domine l'ouverture après crash — le rejeu lui-même, ou l'audit ?
//!
//! Usage :
//! ```text
//! cargo run --release -p obrain-substrate --example replay_bench -- /tmp/obrain-bench-replay.obrain
//! ```
//! L'argument est un répertoire de travail DISJOINT (chaque agent son store).

use std::path::{Path, PathBuf};
use web_time::Instant;

use obrain_substrate::record::{PackedScarUtilAff, U48, f32_to_q1_15};
use obrain_substrate::wal_io::SyncMode;
use obrain_substrate::{NodeRecord, SubstrateFile, Writer, checkpoint, replay_from};

fn node(i: u32) -> NodeRecord {
    NodeRecord {
        label_bitset: 1u64 << (i % 64),
        first_edge_off: U48::default(),
        first_prop_off: U48::default(),
        community_id: i % 8,
        energy: f32_to_q1_15(0.5),
        scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
        centrality_cached: 0,
        flags: 0,
        tenant_id: 0,
        _reserve: 0,
    }
}

/// Bras 1 : décode+applique sur une fenêtre WAL de N records, zone de N slots.
/// Les deux grandeurs croissent ensemble : c'est le coût « rejouer N records ».
fn bras_rejeu_brut(racine: &Path, n: u32) -> f64 {
    let dir = racine.join(format!("brut-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sub_path = dir.join("kb");
    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::Group).unwrap();
        for i in 0..n {
            w.write_node(i, node(i)).unwrap();
        }
        w.commit().unwrap();
        w.fsync_wal().unwrap();
        drop(w);
    }
    // Simule le crash : on ne flushe PAS les zones, le WAL fait foi.
    let sub = SubstrateFile::open(&sub_path).unwrap();
    let t0 = Instant::now();
    let stats = replay_from(&sub, 0).unwrap();
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "  [rejeu-brut] n={n:>9}  appliques={:>9}  {:.3} s  ({:.1} M rec/s)",
        stats.applied,
        dt,
        n as f64 / dt / 1e6
    );
    dt
}

/// Bras 2 : audit post-rejeu isolé. Zone nodes de M slots (flush + checkpoint),
/// puis UN SEUL record de plus dans le WAL. Le rejeu n'applique que ~1 record
/// mais l'audit balaie les M slots : c'est le coût O(slots) pur.
fn bras_audit_isole(racine: &Path, m: u32) -> f64 {
    let dir = racine.join(format!("audit-{m}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sub_path = dir.join("kb");
    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::Group).unwrap();
        for i in 0..m {
            w.write_node(i, node(i)).unwrap();
        }
        w.commit().unwrap();
        let wal = w.wal();
        drop(w);
        // Checkpoint : zone durable, WAL tronqué au marqueur.
        let mut sub2 = SubstrateFile::open(&sub_path).unwrap();
        checkpoint(&mut sub2, &wal).unwrap();
        drop(sub2);
    }
    // Un seul record post-checkpoint, puis « crash » sans flush des zones.
    {
        let sub = SubstrateFile::open(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::Group).unwrap();
        w.write_node(m, node(m)).unwrap();
        w.commit().unwrap();
        w.fsync_wal().unwrap();
        drop(w);
    }
    let sub = SubstrateFile::open(&sub_path).unwrap();
    let t0 = Instant::now();
    let stats = replay_from(&sub, 0).unwrap();
    let dt = t0.elapsed().as_secs_f64();
    let slots = stats.stopped_at; // approx
    println!(
        "  [audit-isole] m={m:>9}  appliques={:>6}  {:.3} s  (~{:.2} GB/s si linéaire sur {} slots de 40 o)",
        stats.applied,
        dt,
        (m as f64 * NodeRecord::SIZE as f64) / dt / 1e9,
        m
    );
    let _ = slots;
    dt
}

fn main() {
    let racine: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-bench-replay.obrain"));
    std::fs::create_dir_all(&racine).unwrap();
    println!("== replay_bench — rejeu WAL / reconstruction (store {racine:?}) ==");
    println!("NodeRecord::SIZE = {} o", NodeRecord::SIZE);
    println!();

    println!("-- Bras 1 : décode+applique (fenêtre WAL = zone) --");
    for &n in &[10_000u32, 100_000, 1_000_000, 5_000_000] {
        bras_rejeu_brut(&racine, n);
    }
    println!();

    println!("-- Bras 2 : audit post-rejeu isolé (WAL ~1 record, zone = M slots) --");
    for &m in &[100_000u32, 1_000_000, 5_000_000] {
        bras_audit_isole(&racine, m);
    }
    println!();

    println!("== fin ==");
}
