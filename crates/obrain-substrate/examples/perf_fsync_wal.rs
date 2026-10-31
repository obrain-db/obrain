//! ── SONDE : COMBIEN COÛTE VRAIMENT LE `fsync` DU WAL (2026-10-06) ──────────
//!
//! POURQUOI CETTE SONDE EXISTE. Le group commit armé ralentissait la
//! ré-indexation incrémentale (×0,42-0,60). L'instrumentation a montré que
//! TOUT l'écart était le `fsync` du veilleur : 557 poses × 1,66 ms = 925 ms sur
//! un mur de 2,33 s. Reste à savoir si ces 1,66 ms sont le prix du MÉDIA (auquel
//! cas aucune correction logicielle ne peut les enlever) ou le prix d'une
//! INTERFÉRENCE avec les écritures du store (auquel cas il y a un correctif).
//!
//! Usage :
//!   cargo run --release -p obrain-substrate --example perf_fsync_wal -- <mode> <n>
//!   mode = seul   : WAL seul, une pose toutes les 2 ms, aucun autre trafic
//!          charge : idem + un fil qui écrit 8 Mio/s dans un AUTRE fichier (le
//!                   trafic que le store fait pendant l'ingestion)
//!
//! Sortie : p50/p90/p99 du `fdatasync` en µs, et le débit de poses tenu.

use obrain_substrate::SyncMode;
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, WalWriter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "seul".into());
    let n: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);

    let dir = std::path::PathBuf::from("/tmp/perf-fsync-wal");
    let _ = std::fs::create_dir_all(&dir);
    let wal_path = dir.join(format!("sonde-{mode}.wal"));
    let _ = std::fs::remove_file(&wal_path);
    let _ = std::fs::remove_file(dir.join(format!("sonde-{mode}.wal.lsn")));

    // `SyncMode::Group` : l'écriture ne fsynce pas d'elle-même, c'est NOUS qui
    // fsyncons à la main — exactement ce que fait `poser_le_tampon(Durable)`.
    let w = Arc::new(WalWriter::open(&wal_path, SyncMode::Group, ANCRE_INITIALE).unwrap());

    let stop = Arc::new(AtomicBool::new(false));
    let charge = if mode == "charge" {
        let stop = Arc::clone(&stop);
        let f = std::fs::File::create(dir.join("charge.bin")).unwrap();
        Some(std::thread::spawn(move || {
            use std::io::Write;
            let mut f = f;
            let buf = vec![0xABu8; 256 * 1024];
            let mut i = 0u64;
            while !stop.load(Ordering::Relaxed) {
                // ~8 Mio/s, soit le trafic d'un store qui ingère des propriétés.
                for _ in 0..2 {
                    let debut = i.to_le_bytes();
                    let _ = f.write_all(&debut);
                    let _ = f.write_all(&buf);
                }
                i += 1;
                std::thread::sleep(Duration::from_millis(60));
            }
        }))
    } else {
        None
    };

    std::thread::sleep(Duration::from_millis(200));
    let mut latences: Vec<u64> = Vec::with_capacity(n);
    let t0 = Instant::now();
    let mut poses = 0u64;
    for seq in 0..n {
        // On pousse un record, puis on POSE + fsync : c'est la pose du veilleur.
        let rec = WalRecord {
            lsn: 0,
            timestamp: 1_700_000_000,
            flags: 0,
            payload: WalPayload::PropSetInline {
                node_id: seq as u32,
                key: "qualified_name".to_string(),
                value_bincode: vec![0x5Au8; 40],
            },
        };
        let _ = w.append(rec);
        let t = Instant::now();
        if w.commit_point().is_err() {
            break;
        }
        latences.push(t.elapsed().as_nanos() as u64);
        poses += 1;
        // Cadence de la needle de TEMPS : le veilleur se réveille toutes les 2 ms.
        std::thread::sleep(Duration::from_millis(2));
    }
    let mur = t0.elapsed();
    stop.store(true, Ordering::Relaxed);
    if let Some(h) = charge {
        let _ = h.join();
    }

    latences.sort_unstable();
    let q = |p: f64| -> f64 {
        if latences.is_empty() {
            return 0.0;
        }
        latences[((latences.len() as f64 - 1.0) * p) as usize] as f64 / 1000.0
    };
    println!(
        "mode={mode} poses={poses} mur={:.3}s  p50={:.0}µs p90={:.0}µs p99={:.0}µs max={:.0}µs \
         · écrits={} octets",
        mur.as_secs_f64(),
        q(0.50),
        q(0.90),
        q(0.99),
        *latences.last().unwrap_or(&0) as f64 / 1000.0,
        w.records_ecrits(),
    );
}
