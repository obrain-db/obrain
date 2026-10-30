//! Bench Phase A multi-writer — MESURE du goulot avant tout design.
//!
//! Question : que coûte réellement le commit d'écriture concurrent, et que
//! gagnerait un group commit WAL ? Le substrate ouvre par défaut en
//! `SyncMode::EveryCommit` (1 fsync par commit) ; `SyncMode::Group` existe
//! mais n'est jamais exploité sur le chemin engine.
//!
//! Bras mesurés (même volume d'écriture, stores tempfile séparés) :
//! 1. `sessions_concurrentes_everycommit`  — N threads × sessions, auto-commit
//!    (le chemin réel du runtime : chaque `session.execute` = 1 transaction +
//!    1 fsync) ;
//! 2. `writer_unique_group_commit`         — 1 writer `SyncMode::Group` +
//!    `fsync_wal()` par lot (le group commit cible).
//!
//! Lecture : le rapport commits/s et le coût par commit décident si la Phase A
//! doit porter sur le group commit (probable), le ParallelExecutor, ou les deux.
//!
//! Run : cargo bench -p obrain-engine --bench phase_a_write_goulot

use std::sync::Arc;
use std::thread;
use web_time::Instant;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use obrain_engine::ObrainDB;
use obrain_substrate::wal_io::SyncMode;

const NTHREADS: usize = 4;
const PAR_THREAD: usize = 200;

fn bench_sessions_concurrentes(c: &mut Criterion) {
    let mut group = c.benchmark_group("phase_a_write");
    group.sample_size(10);

    group.bench_function(
        BenchmarkId::new("sessions_concurrentes_everycommit", NTHREADS),
        |b| {
            b.iter(|| {
                let td = tempfile::tempdir().unwrap();
                let chemin = td.path().join("s-every");
                // open_substrate = EveryCommit (défaut du runtime réel).
                let db = Arc::new(ObrainDB::open_substrate(&chemin).unwrap());
                let depart = Instant::now();
                let mut handles = Vec::new();
                for t in 0..NTHREADS {
                    let db = Arc::clone(&db);
                    handles.push(thread::spawn(move || {
                        let s = db.session();
                        for i in 0..PAR_THREAD {
                            let etiquette = format!("Th{t}");
                            s.execute(&format!("CREATE (:{etiquette} {{id: {i}}})"))
                                .unwrap_or_else(|e| panic!("thread {t} insert {i}: {e}"));
                        }
                    }));
                }
                for h in handles {
                    h.join().unwrap();
                }
                let ecoule = depart.elapsed();
                let total = (NTHREADS * PAR_THREAD) as u64;
                assert_eq!(db.node_count() as u64, total);
                let _ = ecoule;
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("writer_unique_group_commit", NTHREADS),
        |b| {
            b.iter(|| {
                let td = tempfile::tempdir().unwrap();
                let chemin = td.path().join("s-group");
                // Group commit : fsync explicit, par rafale — la cible Phase A.
                let db = Arc::new(
                    ObrainDB::open_substrate_with_mode(&chemin, SyncMode::Group).unwrap(),
                );
                let depart = Instant::now();
                let mut handles = Vec::new();
                for t in 0..NTHREADS {
                    let db = Arc::clone(&db);
                    handles.push(thread::spawn(move || {
                        let s = db.session();
                        for i in 0..PAR_THREAD {
                            let etiquette = format!("Th{t}");
                            s.execute(&format!("CREATE (:{etiquette} {{id: {i}}})"))
                                .unwrap_or_else(|e| panic!("thread {t} insert {i}: {e}"));
                        }
                    }));
                }
                for h in handles {
                    h.join().unwrap();
                }
                // Un seul fsync groupé en fin de rafale (le group commit).
                db.flush_wal().unwrap();
                let ecoule = depart.elapsed();
                let total = (NTHREADS * PAR_THREAD) as u64;
                assert_eq!(db.node_count() as u64, total);
                let _ = ecoule;
            });
        },
    );

    // Bras décisif pour le ParallelExecutor : le SPEEDUP parallèle. Même
    // volume total (NTHREADS×PAR_THREAD), exécuté par 1 thread (séquentiel)
    // vs NTHREADS threads, en Group (fsync neutralisé). Si 4 threads ≈
    // 1 thread, un verrou global sature et l'orchestration par lot
    // (ParallelExecutor) n'apportera rien sans lever ce verrou d'abord.
    group.bench_function("seq_1_thread_group", |b| {
        b.iter(|| {
            let td = tempfile::tempdir().unwrap();
            let chemin = td.path().join("s-seq1");
            let db = ObrainDB::open_substrate_with_mode(&chemin, SyncMode::Group).unwrap();
            let depart = Instant::now();
            let s = db.session();
            let total = (NTHREADS * PAR_THREAD) as u64;
            for i in 0..total {
                s.execute(&format!("CREATE (:Seq {{id: {i}}})"))
                    .unwrap_or_else(|e| panic!("insert {i}: {e}"));
            }
            db.flush_wal().unwrap();
            assert_eq!(db.node_count() as u64, total);
            let _ = depart.elapsed();
        });
    });

    group.finish();
}

criterion_group!(benches, bench_sessions_concurrentes);
criterion_main!(benches);
