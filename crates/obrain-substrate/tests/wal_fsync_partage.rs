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
//! 4. **(2026-11-08, DUR-ORACLE) Le même oracle au niveau du store.** Après
//!    chaque `Writer::commit()` acquitté d'un `SubstrateStore` ouvert en
//!    `EveryCommit`, `SubstrateStore::wal_octets_durables()` couvre la fin du
//!    fichier WAL (l'écrivain est seul : c'est la fin de la frame du commit).
//!    MESURÉ ROUGE (`durable=0`) avec le fsync neutralisé dans `wal_io.rs` —
//!    alors que les tests `kill -9` du moteur restent verts sans fsync, le
//!    cache de pages survivant au processus.
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, CoordinationAppend, WalWriter};
use obrain_substrate::{SubstrateStore, SyncMode};
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

#[test]
fn chaque_commit_du_store_est_couvert_par_la_ligne_durable() {
    let td = tempfile::tempdir().unwrap();
    let store = SubstrateStore::create_with_mode(td.path(), SyncMode::EveryCommit).unwrap();
    let wal = td.path().join("substrate.wal");
    let mut fsyncs_avant = store.wal_fsyncs();
    for k in 0..50u32 {
        store.writer().commit().unwrap();
        // L'écrivain est seul : la fin du fichier EST la fin de la frame du
        // commit que l'on vient d'acquitter.
        let fin = std::fs::metadata(&wal).unwrap().len();
        let durable = store.wal_octets_durables();
        assert!(
            durable >= fin,
            "commit {k} acquitté avant son fsync : fin de frame={fin}, ligne durable={durable}"
        );
        let fsyncs = store.wal_fsyncs();
        assert!(
            fsyncs > fsyncs_avant,
            "commit {k} acquitté sans fsync nouveau ({fsyncs}, déjà {fsyncs_avant})"
        );
        fsyncs_avant = fsyncs;
    }
}
