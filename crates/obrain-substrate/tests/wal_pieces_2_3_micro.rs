//! ── MESURE DES PIÈCES 2 ET 3 (2026-10-06) — LE CPU, PAS LE FSYNC ───────────
//!
//! POURQUOI CES MESURES VIVENT À PART DU BANC `Writer::commit`. Sur le chemin de
//! production, le poste dominant est le `fsync` (~1,6 ms/commit ici) : y mesurer
//! une économie de CPU de quelques dizaines de nanosecondes est impossible — le
//! résultat est ×1,00, quel que soit le gain (mesuré : bras 3 du banc de débit).
//! Les pièces 2 et 3 se mesurent donc LÀ OÙ ELLES AGISSENT : sur le coût CPU par
//! record de l'ENCODAGE et des ALLOCATIONS, qui est le plafond des chemins
//! CPU-bornés (`SyncMode::Never`, WAL en mémoire, gros lots, ré-estampillage).
//!
//! Trois postes, mesurés en isolation sur la même machine et dans le même
//! binaire — c'est ce qui les rend comparables entre eux :
//!
//! 1. **pièce 2 (allocations)** — construire `WalPayload::PropSetInline` (une
//!    `String` + un `Vec<u8>` par record) puis encoder, contre encoder un
//!    `PropSetInlineRef` emprunté. Mêmes octets, deux coûts.
//! 2. **pièce 2 (le `Vec<WalRecord>` du lot)** — `into_iter().collect::<Vec<_>>()`
//!    pour un lot d'UN record, ce que faisait `Writer::commit()` avant.
//! 3. **pièce 3 (CRC)** — hacher 24 o puis le payload (deux `update`) contre une
//!    passe contiguë, sur des frames des trois tailles réelles.
//!
//! Les seuils d'assertion sont LARGES : ce test garde les pièces en place, il ne
//! prétend pas mesurer un pourcentage.

use obrain_substrate::wal::{PropSetInlineRef, WalPayload, WalRecord};
use std::hint::black_box;
use std::time::Instant;

/// Durée moyenne par itération, en nanosecondes, après échauffement.
fn ns_par_iter(iterations: u64, mut f: impl FnMut(u64)) -> f64 {
    for i in 0..iterations.min(2_000) {
        f(i);
    }
    let debut = Instant::now();
    for i in 0..iterations {
        f(i);
    }
    debut.elapsed().as_nanos() as f64 / iterations as f64
}

const VALEUR: [u8; 8] = 0x0102_0304_0506_0708u64.to_le_bytes();

#[test]
#[ignore = "mesure de performance : 207,5 ns contre un budget de 200 ns sur un runner partagé ; à lancer en --ignored ou dans un job dédié"]
fn mesure_piece_2_allocations_du_payload() {
    let n = 200_000u64;
    let mut buf = Vec::with_capacity(256);

    // ── AVANT : payer la String ET le Vec, puis encoder (forme POSSÉDÉE) ────
    let ancien = ns_par_iter(n, |i| {
        let rec = WalRecord {
            lsn: 0,
            timestamp: 1_700_000_000,
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::PropSetInline {
                node_id: i as u32,
                key: format!("k{}", i % 64),
                value_bincode: VALEUR.to_vec(),
            },
        };
        buf.clear();
        rec.encode_append_into(&mut buf).unwrap();
        black_box(buf.len());
    });

    // ── APRÈS : encoder le payload EMPRUNTÉ (aucune allocation) ─────────────
    let cle = "k42";
    let nouveau = ns_par_iter(n, |i| {
        buf.clear();
        PropSetInlineRef {
            node_id: i as u32,
            key: cle,
            value_bincode: &VALEUR,
        }
        .encode_append_with_lsn_into(&mut buf, i, 1_700_000_000, WalRecord::FLAG_COMMIT)
        .unwrap();
        black_box(buf.len());
    });

    let gain = ancien - nouveau;
    eprintln!(
        "PIÈCE 2 (payload) : possédé {ancien:>6.1} ns/rec → emprunté {nouveau:>6.1} ns/rec \
         (−{gain:.1} ns/rec, ×{:.2})",
        ancien / nouveau
    );
    assert!(
        nouveau < ancien,
        "le payload emprunté doit être MOINS cher que le possédé ({nouveau:.1} vs {ancien:.1} ns)"
    );
}

#[test]
#[ignore = "mesure de performance : 207,5 ns contre un budget de 200 ns sur un runner partagé ; à lancer en --ignored ou dans un job dédié"]
fn mesure_piece_2_vec_du_lot_unitaire() {
    let n = 200_000u64;
    // Ce que faisait `Writer::commit()` : `append_batch(once(rec))` →
    // `into_iter().collect::<Vec<WalRecord>>()`. Une allocation de tas + un
    // déplacement de `WalRecord` par ENREGISTREMENT, pour un lot d'UN élément.
    let rec = WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::NoOp,
    };
    let collect = ns_par_iter(n, |_| {
        let v: Vec<WalRecord> = core::iter::once(rec.clone()).collect();
        black_box(v.len());
    });
    eprintln!(
        "PIÈCE 2 (Vec du lot de 1) : {collect:>6.1} ns/rec supprimés — c'est le poste que \
         `append_encoded(&rec)` ne paie plus"
    );
    assert!(
        collect < 200.0,
        "un collect d'un élément doit rester sous 200 ns"
    );
}

#[test]
#[ignore = "mesure de performance : 207,5 ns contre un budget de 200 ns sur un runner partagé ; à lancer en --ignored ou dans un job dédié"]
fn mesure_piece_3_crc() {
    // Les trois tailles qui comptent : un record de propriété courant (~90 o avec
    // l'en-tête), une petite page (~1 Kio), un gros payload (~64 Kio).
    for taille_payload in [62usize, 1000, 65_000] {
        let n = if taille_payload > 10_000 {
            20_000
        } else {
            200_000
        } as u64;
        let mut frame = vec![0u8; 24 + taille_payload];
        for (i, b) in frame.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let deux = ns_par_iter(n, |_| {
            let mut h = crc32fast::Hasher::new();
            h.update(&frame[..24]);
            h.update(&frame[24..]);
            black_box(h.finalize());
        });
        let une = ns_par_iter(n, |_| {
            let mut h = crc32fast::Hasher::new();
            h.update(black_box(&frame[..]));
            black_box(h.finalize());
        });
        eprintln!(
            "PIÈCE 3 (CRC, payload {taille_payload:>6} o) : 2 tranches {deux:>6.1} ns → \
             une passe {une:>6.1} ns (−{:.1} ns, ×{:.2})",
            deux - une,
            deux / une
        );
        // La valeur est IDENTIQUE : c'est ce qui autorise le remplacement (test
        // unitaire `crc_une_passe_egale_le_crc_en_deux_tranches` pour la preuve).
        let mut h2 = crc32fast::Hasher::new();
        h2.update(&frame[..24]);
        h2.update(&frame[24..]);
        let mut h1 = crc32fast::Hasher::new();
        h1.update(&frame[..]);
        assert_eq!(h1.finalize(), h2.finalize());
    }
}
