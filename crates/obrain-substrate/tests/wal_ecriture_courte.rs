//! ENOSPC — LE CAS **ÉCRITURE COURTE** : la frame est DÉCHIRÉE, et elle est annulée.
//!
//! ── POURQUOI CE FICHIER DE TEST EST SÉPARÉ ─────────────────────────────────
//! Le quota disque se simule avec `RLIMIT_FSIZE`, qui borne le processus ENTIER :
//! tous les fichiers écrits par ce binaire (y compris ceux du harnais de test)
//! tombent sous la limite. Un binaire dédié, à UN SEUL test, est le seul moyen de
//! garantir qu'aucun autre test ne subit la limite en parallèle.
//!
//! ── CE QU'ON VÉRIFIE, ET POURQUOI LES DEUX CAS SONT OPPOSÉS ────────────────
//! Un quota atteint ne produit pas toujours la même chose :
//!   · si le noyau refuse la frame ENTIÈRE, rien n'est déchiré — la queue est
//!     intacte et il n'y a RIEN à réparer (`wal_append_unite.rs` le vérifie sur
//!     `/dev/full`, qui rend un ENOSPC franc) ;
//!   · si le noyau en accepte une PARTIE (ce que fait `RLIMIT_FSIZE` : il écrit
//!     jusqu'à la limite puis rend un compte PARTIEL), la frame est DÉCHIRÉE.
//!     Sans annulation, le rejeu s'arrêterait sur elle — et tout ce qui suivrait
//!     serait invisible à jamais, sans que rien ne le dise.
//!
//! C'est ce second cas, le plus dangereux, que ce test provoque pour de vrai.

#![cfg(unix)]

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, WalReader, WalWriter};
use obrain_substrate::{SubstrateError, SyncMode};

fn record(seq: u64, taille: usize) -> WalRecord {
    let mut value = Vec::with_capacity(taille);
    value.push(1);
    value.extend(std::iter::repeat_n(0x5A, taille.saturating_sub(1)));
    WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: seq as u32,
            key: format!("t1s{seq}"),
            value_bincode: value,
        },
    }
}

struct Quota {
    ancien: libc::rlimit,
}

impl Quota {
    /// Plafonne la taille de TOUT fichier écrit par ce processus à `octets`.
    fn poser(octets: u64) -> Self {
        unsafe {
            // SIGXFSZ tue le processus par défaut : on l'ignore pour que
            // l'écriture rende un compte PARTIEL (c'est le comportement POSIX
            // documenté), au lieu de mourir avant de pouvoir vérifier.
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let mut ancien = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut ancien), 0);
            let nouveau = libc::rlimit {
                rlim_cur: octets,
                rlim_max: ancien.rlim_max,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &nouveau), 0);
            Self { ancien }
        }
    }
}

impl Drop for Quota {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &self.ancien), 0);
            libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
        }
    }
}

#[test]
fn ecriture_courte_le_quota_dechire_la_frame_et_l_append_l_annule() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    for i in 0..3u64 {
        w.append(record(i, 32)).unwrap();
    }
    let taille_saine = w.offset();
    assert_eq!(std::fs::metadata(&wal).unwrap().len(), taille_saine);

    // Quota : 24 octets de marge seulement, alors que le lot à écrire en fait
    // plus de 300. Le noyau en posera une PARTIE, puis rendra le compte.
    let quota = Quota::poser(taille_saine + 24);

    let lot: Vec<WalRecord> = (0..3u64).map(|i| record(100 + i, 96)).collect();
    let e = w.append_batch(lot).unwrap_err();

    // La licence du quota est rendue TOUT DE SUITE : le reste du test (et le
    // ménage de `tempfile`) doit pouvoir écrire normalement.
    drop(quota);

    match &e {
        SubstrateError::WalEcritureCourte {
            ecrits,
            demandes,
            position,
        } => {
            assert!(
                *ecrits > 0,
                "une écriture COURTE a bien transféré des octets"
            );
            assert!(*ecrits < *demandes, "mais pas la frame entière");
            assert_eq!(*position, taille_saine, "elle a commencé à la fin saine");
        }
        autre => panic!(
            "un quota atteint EN PLEINE frame doit se dire « écriture courte » \
             (frame déchirée), pas {autre:?} — la confondre avec un refus net \
             ferait laisser une frame hybride dans le WAL"
        ),
    }

    // ── LA FRAME PARTIELLE A ÉTÉ RETIRÉE : LA QUEUE EST PROPRE ─────────────
    assert_eq!(
        std::fs::metadata(&wal).unwrap().len(),
        taille_saine,
        "la frame déchirée a été ANNULÉE par troncature"
    );
    assert_eq!(w.offset(), taille_saine);
    assert!(w.queue_a_repairer().is_none(), "rien ne reste à réparer");

    let r = WalReader::open(&wal).unwrap();
    let mut it = r.iter_from(0);
    let mut n = 0usize;
    for item in it.by_ref() {
        item.expect("aucune frame illisible après annulation");
        n += 1;
    }
    assert_eq!(n, 3, "les trois frames saines sont intactes");
    assert!(
        !it.queue_incomplete(),
        "la queue doit être un BORD de frame, pas une déchirure"
    );
    assert!(it.trou().is_none());

    // ── ET LE WAL REPART PROPREMENT (le quota est rendu) ───────────────────
    for i in 0..10u64 {
        w.append(record(200 + i, 48)).unwrap();
    }
    let r = WalReader::open(&wal).unwrap();
    let total = r.iter_from(0).filter(|i| i.is_ok()).count();
    assert_eq!(total, 13, "3 saines + 10 nouvelles, contiguës");
    assert_eq!(
        std::fs::metadata(&wal).unwrap().len(),
        w.offset(),
        "EOF == offset, aucune trace de l'écriture courte"
    );
}
