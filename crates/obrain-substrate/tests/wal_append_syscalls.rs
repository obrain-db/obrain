//! ── PREUVE PAR COMPTAGE DE SYSCALLS : UN APPEND PARTAGÉ COÛTE-T-IL 3 APPELS ? ─
//!
//! ── POURQUOI CE BINÔME DE MESURE EXISTE, ET PAS SEULEMENT UN BANC ───────────
//! Le banc criterion donne des NANOSECONDES, pas des CAUSES. Les deux chiffres
//! qui expliquent la latence de l'append partagé sont des NOMBRES D'APPELS
//! SYSTÈME : `write` (l'écriture, incompressible), et `statx` (la relecture de la
//! taille réelle — `File::metadata()`, pas `fstat`, 802 ns mesurés contre 571).
//! Un test qui n'imprimerait que des ns laisserait croire à un effet de cache ;
//! `strace -c` nomme le poste.
//!
//! ── MODE D'EMPLOI (les trois tests sont `#[ignore]` : ce sont des sondes) ───
//! ```text
//! cargo test -p obrain-substrate --release --test wal_append_syscalls --no-run
//! BIN=$(ls -t target/release/deps/wal_append_syscalls-* | grep -v '\.d$' | head -1)
//! strace -c -f -e trace=write,statx,newfstatat,fstat,flock,openat,fsync,fdatasync \
//!        "$BIN" --ignored --nocapture --test-threads=1
//! ```
//!
//! ── CE QU'IL FAUT Y LIRE : AVANT / APRÈS LA CHAÎNE DE RÉSERVATION ──────────
//! Mesuré le 2026-10-06 sur le VRAI `WalWriter`.
//!
//! ── AVANT (commit `cebd0ab5`, 4 000 appends par sonde) ─────────────────────
//! | régime                        | `write` | `statx` | `flock` | ns/record (banc) |
//! |-------------------------------|---------|---------|---------|------------------|
//! | `Aucune` (mono-écrivain prouvé)| 1,007   | 0       | 0       | 995              |
//! | `OAppend` (WAL partagé)        | 1,007   | **2,003** | 0     | 2 774            |
//! | `VerrouExact`                  | 1,007   | **2,003** | **2,0** | 4 005          |
//!
//! ── APRÈS (chaîne de réservation dans `substrate.wal.reg`) ─────────────────
//! | régime                        | `write` | `statx` | `flock` | ns/record (banc) |
//! |-------------------------------|---------|---------|---------|------------------|
//! | `Aucune` (mono-écrivain prouvé)| 1,007   | 0       | 0       | 998 (inchangé)   |
//! | `OAppend` (WAL partagé)        | 1,007   | **0**   | 0       | **1 054**        |
//! | `VerrouExact`                  | 1,007   | **0**   | 0       | **1 075**        |
//!
//! Les `statx` qui restent dans le rapport (`13` au total, contre `8 010` avant)
//! sont ceux de l'OUVERTURE (`.metadata()`, `.lsn` du side-car) : aucun n'est payé
//! par un append. Le `flock` unique est celui de la CRÉATION du registre (chemin
//! froid, une fois par vie de base). Voir `tests/wal_chaine_cas.rs` pour les
//! garanties (LSN exact, tête bloquée, troncature annoncée, composition avec le
//! group commit).

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, CoordinationAppend, SyncMode, WalWriter};
use std::hint::black_box;

/// Records par sonde. 2 000 suffisent à rendre les comptes de syscalls lisibles
/// (les rapports `strace -c` sont proportionnels) et gardent la sortie courte.
const N: u64 = 2_000;

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

/// Le corps commun des trois sondes : échauffe, mesure, imprime.
fn sonde(nom: &str, coordination: CoordinationAppend) {
    let dir = tempfile::tempdir().unwrap();
    let w = WalWriter::open(dir.path().join("w.wal"), SyncMode::Never, ANCRE_INITIALE).unwrap();
    w.coordination(coordination);
    // Échauffement : croissance du fichier, premier `statx`, cache froid. On ne
    // mesure que le régime ÉTABLI (comme le banc criterion).
    for i in 0..2_000u64 {
        w.append(record(i)).unwrap();
    }
    let debut = std::time::Instant::now();
    for i in 0..N {
        black_box(w.append(black_box(record(i))).unwrap());
    }
    let ecoule = debut.elapsed();
    let ns = ecoule.as_nanos() as f64 / N as f64;
    eprintln!(
        "SONDE {nom} : {N} appends en {:.3} ms → {ns:.0} ns/record · offset final {} o \
         · recalages={} · courses={}",
        ecoule.as_secs_f64() * 1000.0,
        w.offset(),
        w.recalages(),
        w.courses_suspectees(),
    );
}

#[test]
#[ignore = "sonde de mesure : à lancer sous strace -c (voir l'en-tête)"]
fn sonde_aucune() {
    sonde("aucune (mono-écrivain prouvé)", CoordinationAppend::Aucune);
}

#[test]
#[ignore = "sonde de mesure : à lancer sous strace -c (voir l'en-tête)"]
fn sonde_oappend() {
    sonde("oappend (WAL partagé)", CoordinationAppend::OAppend);
}

#[test]
#[ignore = "sonde de mesure : à lancer sous strace -c (voir l'en-tête)"]
fn sonde_verrou() {
    sonde(
        "verrou (exactitude demandée)",
        CoordinationAppend::VerrouExact,
    );
}
