//! ── BANC : LE CHEMIN RÉEL `Writer::commit()`, AVANT / APRÈS LA NEEDLE ───────
//!
//! POURQUOI UN TEST DE DÉBIT ET PAS UNE BENCH CRITERION. Le poste mesuré est un
//! `fsync` : ~2,65 ms pièce sur ce disque, soit 644 rec/s sur le chemin de
//! production (mesuré le 2026-10-06 par le banc à 12 agents). Un `criterion`
//! échantillonné (100 tirages) prendrait des dizaines de minutes pour ce seul
//! point, et son warmup se ferait sur un état qui n'a plus rien à voir avec le
//! régime établi. Ici on mesure TROIS bras sur le VRAI `Writer` (le même objet que
//! le store), en une passe, et on imprime des chiffres datés.
//!
//! LES TROIS BRAS, ET CE QU'ILS ISOLENT :
//!
//! 1. `commit()` group commit NON ARMÉ — le chemin d'avant, tel qu'il tourne
//!    aujourd'hui : un `fsync` par commit. C'est la référence (644 rec/s).
//! 2. `commit()` group commit ARMÉ — le chemin après la pièce 1 : un `fsync` par
//!    NEEDLE (4 096 records), donc ~4 096 records par 2,65 ms.
//! 3. `commit_durable()` group commit ARMÉ — la durabilité par commit RESTE
//!    disponible, mais elle paie l'encodage des pièces 2-3 (zéro allocation, CRC
//!    en une passe) au lieu de l'ancien. Comparer 1 et 3 isole donc les pièces 2
//!    et 3 du gain de la pièce 1.
//!
//! Les seuils d'assertion sont LARGES (×10) : ce test n'est pas un bench de
//! précision, c'est un garde-fou contre la disparition du gain (une needle
//! débranchée, un `fsync` remis par commit, un verrou ajouté au push).

use obrain_substrate::wal_io::SEUIL_RECORDS;
use obrain_substrate::{SubstrateFile, SyncMode, Writer};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn ouvrir(nom: &str) -> (tempfile::TempDir, Writer) {
    let td = tempfile::tempdir().unwrap();
    let chemin: PathBuf = td.path().join(nom);
    let sub = SubstrateFile::create(&chemin).unwrap();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
    (td, w)
}

/// Débit d'un bras : `n` commits, et la durée observée.
fn bras(nom: &str, n: u64, mut commit: impl FnMut()) -> (f64, Duration) {
    // Échauffement : la première écriture paie l'ouverture du fichier, sa
    // croissance et le cache froid. On ne mesure QUE le régime établi.
    for _ in 0..8 {
        commit();
    }
    let debut = Instant::now();
    for _ in 0..n {
        commit();
        black_box(());
    }
    let ecoule = debut.elapsed();
    let rec_s = n as f64 / ecoule.as_secs_f64();
    eprintln!(
        "  {nom:<44} {n:>7} commits en {:>9.2} ms → {rec_s:>12.0} rec/s ({:>7.0} ns/commit)",
        ecoule.as_secs_f64() * 1000.0,
        ecoule.as_nanos() as f64 / n as f64
    );
    (rec_s, ecoule)
}

#[test]
fn debit_du_chemin_reel_writer_commit() {
    // ── BRAS 1 : LE CHEMIN D'AVANT (group commit non armé) ──────────────────
    let (_t1, w1) = ouvrir("kb");
    eprintln!(
        "BANC Writer::commit — machine : {} cœurs disponibles",
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
    let n_avant = 256u64;
    let (avant, _d1) = bras("1. AVANT  : commit() sans group commit", n_avant, || {
        w1.commit().unwrap();
    });
    let fsyncs_avant = w1.wal().fsyncs();
    // 1 fsync PAR COMMIT : c'est la règle `EveryCommit => commit_dans_le_lot`, et
    // c'est elle qui fixe les 644 rec/s. Si ce compte tombait, le bras « avant »
    // ne serait plus le chemin d'avant.
    assert_eq!(
        fsyncs_avant,
        n_avant + 8,
        "hors group commit, l'EveryCommit doit fsyncer une fois par commit"
    );

    // ── BRAS 2 : LE CHEMIN APRÈS LA PIÈCE 1 (needle) ────────────────────────
    let (_t2, w2) = ouvrir("kb");
    w2.activer_group_commit_wal();
    let n_apres = 8 * SEUIL_RECORDS as u64;
    let (apres, _d2) = bras("2. APRÈS  : commit() + group commit", n_apres, || {
        w2.commit().unwrap();
    });
    let fsyncs_apres = w2.wal().fsyncs();
    // Le compte exact est `n / SEUIL_RECORDS`, PLUS les flush que la needle de
    // TEMPS (2 ms, le seuil de production) a pu ajouter en scindant un lot — le
    // veilleur tourne pendant la mesure, c'est le régime réel. Ce qui compte ici
    // est l'ordre de grandeur : 8 fsync pour 32 768 commits, pas 32 768.
    let fsyncs_attendus = n_apres / SEUIL_RECORDS as u64;
    assert!(
        fsyncs_apres >= fsyncs_attendus && fsyncs_apres <= fsyncs_attendus + 16,
        "le fsync doit être indexé sur la NEEDLE : {n_apres} commits = \
         ~{fsyncs_attendus} fsync, pas {n_apres}. Obtenu : {fsyncs_apres}"
    );

    // ── BRAS 3 : DURABILITÉ PAR COMMIT CONSERVÉE (pièces 2-3 seules) ────────
    let (_t3, w3) = ouvrir("kb");
    w3.activer_group_commit_wal();
    let (durable, _d3) = bras(
        "3. AVANT+ : commit_durable() (pièces 2-3 seules)",
        n_avant,
        || {
            w3.commit_durable().unwrap();
        },
    );
    assert_eq!(w3.wal().fsyncs(), n_avant + 8);

    let x_needle = apres / avant;
    let x_code = durable / avant;
    eprintln!(
        "\nBANC — CHEMIN RÉEL `Writer::commit()`, mesuré le {} :\n  \
         AVANT (1 fsync/commit)         : {:>12.0} rec/s\n  \
         APRÈS (needle 4096)            : {:>12.0} rec/s  → ×{x_needle:.0}  \
         ({} fsync pour {n_apres} commits)\n  \
         APRÈS durable par commit       : {:>12.0} rec/s  → ×{x_code:.2}  \
         (pièces 2-3 seules, 1 fsync/commit)\n  \
         coût unitaire : AVANT {:.0} ns/commit · APRÈS {:.0} ns/commit",
        "2026-10-06",
        avant,
        apres,
        fsyncs_apres,
        durable,
        (n_avant as f64 / avant) * 1e9,
        (n_apres as f64 / apres) * 1e9,
    );

    // ── SEUILS (larges, volontairement : on garde le GAIN, pas des %) ───────
    assert!(
        x_needle > 10.0,
        "la needle ne rend que ×{x_needle:.1} : le fsync est-il retombé sur le lot ?"
    );
    // PAS de plancher en rec/s : il dépend du disque et de la charge du runner
    // (184 031 rec/s mesurés sur n3 chargé, rouge pour un seuil à 200 000 alors
    // que le gain était de ×131). Le gain relatif et la rareté des fsync
    // ci-dessous disent la même chose sans dépendre de la machine ; le débit est
    // imprimé plus haut pour qui veut le lire.
    assert!(
        fsyncs_apres * 100 < n_apres,
        "les fsync doivent être RARES : {fsyncs_apres} pour {n_apres} commits"
    );
}
