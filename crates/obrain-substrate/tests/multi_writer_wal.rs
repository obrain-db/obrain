//! I5 — append WAL sous threads concurrents (RFC docs/rfc/substrate/multi-writer.md §4).
//!
//! ── CE QUE CE FILET EXIGE A CHANGÉ LE 2026-10-06 (axe APPEND) ───────────────
//! L'invariant gelé était : « N threads qui appendent en parallèle produisent des
//! LSN STRICTEMENT CONTIGUS (1..=N*M, sans trou ni doublon) ». Il ne tient plus,
//! et ce n'est PAS une régression : le LSN n'est plus un numéro de séquence, il
//! est DÉRIVÉ DE LA POSITION PHYSIQUE (`lsn = ancre + position`, `wal_io.rs`).
//! Deux frames consécutives ont donc des LSN séparés par la taille de la
//! première, pas par 1.
//!
//! Ce que le filet exige désormais, et qui est STRICTEMENT PLUS FORT que
//! l'ancien invariant ensembliste :
//!   · zéro perte (inchangé) ;
//!   · l'ORDRE PHYSIQUE **est** l'ORDRE LSN (croissant le long du fichier) —
//!     propriété que l'ancien `fetch_add` hors mutex ne garantissait PAS, et que
//!     la RFC §4.1 documentait comme un défaut (« l'ordre physique des records
//!     dans le WAL ≠ ordre des LSN ») ;
//!   · `lsn == ancre + offset`, vérifié frame par frame ;
//!   · unicité des LSN (conséquence directe de l'unicité des offsets).

use std::sync::Arc;
use std::thread;

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, SyncMode, WalReader, WalWriter};

fn record_pour(thread_id: u64, seq: u64) -> WalRecord {
    // Payload distinct par (thread, seq) — la relecture doit tous les retrouver.
    WalRecord {
        lsn: 0, // assigné par WalWriter::append (ancre + position)
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::NodeInsert {
            node_id: (thread_id * 1_000_000 + seq) as u32,
            label_bitset: (thread_id << 32) | seq,
        },
    }
}

/// Relit le WAL et rend `(nombre de frames, LSN strictement croissants,
/// `lsn == ancre + offset` pour chacune, LSN uniques)`.
fn auditer(wal_path: &std::path::Path) -> (usize, bool, bool, bool) {
    let reader = WalReader::open(wal_path).unwrap();
    let mut dernier = 0u64;
    let mut croissant = true;
    let mut derive = true;
    let mut vus = std::collections::HashSet::new();
    let mut uniques = true;
    let mut n = 0usize;
    for item in reader.iter_from(0) {
        let (rec, off, _len) = item.expect("record décodable");
        if rec.lsn <= dernier {
            croissant = false;
        }
        // L'ancre est celle du segment : 1 pour un WAL neuf, et elle ne bouge
        // pas tant qu'aucun checkpoint n'a tronqué.
        if rec.lsn != ANCRE_INITIALE + off {
            derive = false;
        }
        if !vus.insert(rec.lsn) {
            uniques = false;
        }
        dernier = rec.lsn;
        n += 1;
    }
    (n, croissant, derive, uniques)
}

#[test]
fn append_multithread_ordre_physique_egal_ordre_lsn_zero_perte() {
    let dir = tempfile::tempdir().unwrap();
    let wal_path = dir.path().join("w-multi.wal");

    let writer = Arc::new(WalWriter::open(&wal_path, SyncMode::Never, ANCRE_INITIALE).unwrap());

    let nb_threads = 8;
    let par_thread = 200;
    let mut handles = Vec::new();
    for t in 0..nb_threads {
        let w = Arc::clone(&writer);
        handles.push(thread::spawn(move || {
            for i in 0..par_thread {
                w.append(record_pour(t as u64, i as u64))
                    .expect("append concurrent");
            }
        }));
    }
    for h in handles {
        h.join().expect("thread joint");
    }

    let total = (nb_threads * par_thread) as u64;
    let taille = writer.offset();
    assert_eq!(
        writer.peek_lsn(),
        ANCRE_INITIALE + taille,
        "le prochain LSN est `ancre + offset` — plus un compteur de records"
    );

    let (n, croissant, derive, uniques) = auditer(&wal_path);
    assert_eq!(n, total as usize, "zéro perte : tous les appends sont relus");
    assert!(
        croissant,
        "l'ORDRE PHYSIQUE est l'ORDRE LSN : les LSN croissent le long du fichier"
    );
    assert!(derive, "chaque frame porte `lsn = ancre + position`");
    assert!(uniques, "aucun LSN émis deux fois (positions uniques)");
}

#[test]
fn append_multithread_contre_reader_concurrent() {
    // I10 au niveau WAL : un lecteur qui itère PENDANT que des threads
    // appendent ne voit jamais de record corrompu ni de LSN décroissant dans
    // son parcours séquentiel (chaque record est complet ou absent).
    let dir = tempfile::tempdir().unwrap();
    let wal_path = dir.path().join("w-live.wal");

    let writer = Arc::new(WalWriter::open(&wal_path, SyncMode::Never, ANCRE_INITIALE).unwrap());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let w = Arc::clone(&writer);
    let s = Arc::clone(&stop);
    let producteur = thread::spawn(move || {
        let mut i = 0u64;
        while !s.load(std::sync::atomic::Ordering::Relaxed) {
            w.append(record_pour(0, i)).expect("append live");
            i += 1;
        }
    });

    // Le lecteur relit le fichier à plusieurs reprises pendant l'écriture :
    // chaque record COMPLET doit se décoder sans erreur (CRC valide) — une
    // écriture en vol peut laisser une queue partielle, que l'itérateur arrête
    // proprement (comportement crash-boundary), mais jamais de record corrompu
    // au milieu.
    for _ in 0..10 {
        let reader = WalReader::open(&wal_path).unwrap();
        for item in reader.iter_from(0) {
            let (rec, _, _) = item.expect("aucun record corrompu visible");
            assert!(rec.lsn >= 1, "LSN valide");
        }
        thread::sleep(std::time::Duration::from_millis(2));
    }

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    producteur.join().expect("producteur joint");

    // Après arrêt du producteur : relecture complète, ordre physique == ordre
    // LSN (croissant, sans doublon) — zéro perte des records committés.
    let (n, croissant, derive, uniques) = auditer(&wal_path);
    assert!(n > 0);
    assert!(croissant, "ensemble croissant après arrêt");
    assert!(derive, "lsn = ancre + position");
    assert!(uniques, "aucun doublon");
}
