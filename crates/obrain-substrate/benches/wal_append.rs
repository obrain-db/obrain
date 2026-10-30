//! BENCH — AXE APPEND de la FIFO multi-écrivains.
//!
//! Trois bras, ceux que la RFC avait chiffrés au proto et que le fix doit
//! confirmer (ou dementir) sur le VRAI `WalWriter` :
//!
//! 1. `append_unitaire` — un `write` syscall par record, coordination `Aucune`
//!    (le chemin chaud d'un store ouvert par `SubstrateFile::open` : personne
//!    d'autre ne peut écrire, donc aucune coordination n'est due). C'est LA
//!    mesure de non-régression : le fix ne doit pas dégrader ce chemin.
//! 2. `append_unitaire_oappend` — le même, en coordination `OAppend` (WAL
//!    partagé) : le prix de la garde d'append et de la vérification
//!    post-écriture, payé seulement par les stores en écriture partagée.
//! 3. `append_batch_64|1024` — le group commit : UN `write` pour N records.
//!    C'est le levier mesuré de la RFC (`write_all` groupé par 64 = ×22 au
//!    proto sur le seul coût du syscall).
//!
//! Le débit des plus petits records est dominé par le coût FIXE du syscall,
//! c'est-à-dire exactement ce que le group commit amortit.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, CoordinationAppend, SyncMode, WalWriter};
use std::hint::black_box;

/// Record réaliste : une propriété scalaire inline (clé courte + valeur bincode),
/// soit ~90 octets encodés — la taille d'un write de graphe courant.
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

fn writer(dir: &tempfile::TempDir, nom: &str, coordination: CoordinationAppend) -> WalWriter {
    let w = WalWriter::open(dir.path().join(nom), SyncMode::Never, ANCRE_INITIALE).unwrap();
    w.coordination(coordination);
    w
}

fn bench_append_unitaire(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("wal_append_unitaire");
    groupe.throughput(Throughput::Elements(1));
    for (nom, coordination) in [
        ("seul_ecrivain", CoordinationAppend::Aucune),
        ("partage_oappend", CoordinationAppend::OAppend),
        ("partage_verrou", CoordinationAppend::VerrouExact),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(&dir, "w.wal", coordination);
        let mut seq = 0u64;
        groupe.bench_function(BenchmarkId::from_parameter(nom), |b| {
            b.iter(|| {
                seq += 1;
                black_box(w.append(black_box(record(seq))).unwrap())
            })
        });
    }
    groupe.finish();
}

fn bench_append_batch(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("wal_append_batch");
    for taille in [1usize, 8, 64, 256, 1024] {
        let dir = tempfile::tempdir().unwrap();
        let w = writer(&dir, "w.wal", CoordinationAppend::Aucune);
        let mut seq = 0u64;
        groupe.throughput(Throughput::Elements(taille as u64));
        groupe.bench_with_input(BenchmarkId::from_parameter(taille), &taille, |b, &n| {
            b.iter(|| {
                let lot: Vec<WalRecord> = (0..n)
                    .map(|i| {
                        seq += 1;
                        record(seq + i as u64)
                    })
                    .collect();
                black_box(w.append_batch(lot).unwrap())
            })
        });
    }
    groupe.finish();
}

criterion_group!(benches, bench_append_unitaire, bench_append_batch);
criterion_main!(benches);
