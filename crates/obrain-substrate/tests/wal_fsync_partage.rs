//! ── FSYNC PARTAGÉ DU COMMIT CONCURRENT (`SyncMode::EveryCommit`) ─────────────
//!
//! Constat mesuré : chaque `commit()` posait son propre `fsync` sous le verrou
//! `inner`, donc N commits concurrents coûtaient N fsync séquentiels.
//!
//! Contrats testés ici :
//!
//! 1. **Un commit n'est rendu qu'après un `fsync` qui couvre ses octets.**
//!    À chaque retour d'`append`, `octets_durables() >= position + len`.
//! 2. **Les commits concurrents partagent leur `fsync`.** Avec 4 threads, le
//!    nombre de fsync est strictement inférieur au nombre de commits. (ROUGE
//!    sur l'état d'avant : un fsync par commit.)
//! 3. **Cohérence des compteurs** : au repos, chaque commit est couvert, le
//!    nombre de fsync ne dépasse pas le nombre de commits, et le nombre de
//!    frames acquittées est exactement celui attendu.
use obrain_substrate::SyncMode;
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, CoordinationAppend, WalWriter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const THREADS: usize = 4;
const COMMITS_PAR_THREAD: usize = 200;

fn commit_nu() -> WalRecord {
    WalRecord {
        lsn: 0,
        timestamp: 1_700_000_000,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::NoOp,
    }
}

/// Un WAL à écrivain exclusif (le cas de `open_substrate`, `Aucune`).
fn wal_exclusif(dir: &std::path::Path) -> Arc<WalWriter> {
    let w = WalWriter::open(dir.join("w.wal"), SyncMode::EveryCommit, ANCRE_INITIALE).unwrap();
    w.coordination(CoordinationAppend::Aucune);
    Arc::new(w)
}

#[test]
fn commit_rendu_seulement_une_fois_ses_octets_fsynces() {
    let td = tempfile::tempdir().unwrap();
    let wal = wal_exclusif(td.path());
    let acquittes = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let wal = Arc::clone(&wal);
        let acquittes = Arc::clone(&acquittes);
        handles.push(std::thread::spawn(move || {
            for _ in 0..COMMITS_PAR_THREAD {
                let (position, len) = wal.append_encoded(&commit_nu()).unwrap();
                let fin = position + len as u64;
                // Invariant d'acquittement : au retour, la fin de NOTRE frame
                // est couverte par un fsync terminé.
                assert!(
                    wal.octets_durables() >= fin,
                    "commit rendu avant son fsync : fin={fin}, durable={}",
                    wal.octets_durables()
                );
                acquittes.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let total = (THREADS * COMMITS_PAR_THREAD) as u64;
    assert_eq!(
        acquittes.load(Ordering::Relaxed),
        total,
        "commits acquittés"
    );
    let fsyncs = wal.fsyncs();
    eprintln!(
        "fsync-partage: {fsyncs} fsync pour {total} commits (ratio {:.3})",
        fsyncs as f64 / total as f64
    );
    assert!(fsyncs >= 1, "aucun fsync pour {total} commits");
    assert!(
        fsyncs <= total,
        "plus de fsync ({fsyncs}) que de commits ({total})"
    );
    assert!(
        fsyncs < total,
        "les commits concurrents ne partagent pas leur fsync : {fsyncs} fsync pour {total} commits"
    );
}
