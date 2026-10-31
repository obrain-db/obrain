//! LATENCE DU CHEMIN CHAUD — le fix ne doit PAS dégrader l'append mono-écrivain.
//!
//! ── CE QUE CE TEST MESURE, ET POURQUOI IL EST PERMANENT ────────────────────
//! Le goulot structurel du chemin d'écriture est le `write` SYSCALL (~0,94 µs
//! par record, mesuré le 2026-09-06). L'axe APPEND ajoute au chemin chaud :
//!   · la lecture de la fin sous le mutex (`fstat`), qui remplace le compteur
//!     `fetch_add` par processus — c'est ce qui rend le LSN connaissable ;
//!   · un `write` UNIQUE au lieu de `write_all` (aucun coût : souvent MOINS de
//!     syscalls, puisqu'un `write_all` peut boucler).
//! Si ce test se met à mesurer des dizaines de µs par record, c'est que quelque
//! chose de structurel a été ajouté au chemin chaud (un fsync par record, un
//! verrou inter-processus, un balayage du fichier…).
//!
//! ── IL N'UTILISE QUE L'API COMMUNE AUX ÉTATS PRÉ/POST-FIX ──────────────────
//! `WalWriter::open` + `append` : le même corps compile et tourne avant ET après,
//! ce qui permet de comparer les deux régimes dans les mêmes conditions — c'est
//! la mesure « avant/après » de la non-régression.

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{SyncMode, WalWriter};
use std::hint::black_box;

fn record(seq: u64) -> WalRecord {
    WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: seq as u32,
            key: format!("k{}", seq % 64),
            value_bincode: (seq as i64).to_le_bytes().to_vec(),
        },
    }
}

#[test]
fn latence_append_mono_ecrivain() {
    let dir = tempfile::tempdir().unwrap();
    let w = WalWriter::open(dir.path().join("w.wal"), SyncMode::Never, 1).unwrap();

    // Échauffement : la première écriture paie l'allocation, la croissance du
    // fichier et le cache froid. On ne mesure QUE le régime établi.
    for i in 0..20_000u64 {
        w.append(record(i)).unwrap();
    }

    let n = 200_000u64;
    let debut = std::time::Instant::now();
    for i in 0..n {
        black_box(w.append(black_box(record(i))).unwrap());
    }
    let ecoule = debut.elapsed();
    let par_record_ns = ecoule.as_nanos() as f64 / n as f64;

    eprintln!(
        "LATENCE append mono-écrivain : {n} records en {:.1} ms → {:.0} ns/record \
         ({:.2} M records/s) · offset final {} o",
        ecoule.as_secs_f64() * 1000.0,
        par_record_ns,
        n as f64 / ecoule.as_secs_f64() / 1e6,
        w.offset(),
    );

    // Borne LARGE, volontairement : ce test n'est pas un bench (il ne prétend pas
    // mesurer des %, il refuse une RÉGRESSION CATASTROPHIQUE). Le chiffre daté,
    // lui, vit dans `benches/wal_append.rs` et dans le rapport.
    assert!(
        par_record_ns < 20_000.0,
        "l'append mono-écrivain est à {par_record_ns:.0} ns/record : quelque chose \
         de structurel s'est ajouté au chemin chaud (fsync par record ? verrou \
         inter-processus ? balayage ?)"
    );
}
