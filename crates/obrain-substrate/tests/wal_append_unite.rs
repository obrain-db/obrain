//! AXE APPEND de la FIFO multi-écrivains — **checks mono-processus**.
//!
//! Chaque test porte le numéro du check qu'il instancie (T6, T7, garde de
//! taille, ENOSPC, queue à réparer) ou le nom du cas qu'il fige (réconciliation
//! d'ouverture, trou de zéros). Les checks INTER-PROCESSUS (T2, T3, T4) vivent
//! dans `mw_interprocess_append.rs`, et l'écriture courte réelle (quota disque
//! atteint) dans `wal_ecriture_courte.rs` — ce dernier a son propre binaire de
//! test parce qu'il borne `RLIMIT_FSIZE` du processus entier.
//!
//! ── CE QUE CES TESTS DOIVENT PROUVER ────────────────────────────────────────
//! Ils ÉCHOUENT tous, ou ne compilent pas, sur l'état PRÉ-fix (voir la preuve
//! rouge→vert du rapport) : c'est leur raison d'être. Un test qui passerait
//! avant et après ne garderait rien.

use std::path::Path;

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{
    ANCRE_INITIALE, Reconciliation, WalReader, WalWriter, lire_ancre_durable,
};
use obrain_substrate::{SubstrateError, SyncMode};

fn record(tag: u8, seq: u64, taille_payload: usize) -> WalRecord {
    // Payload de taille VARIABLE et de contenu distinct par (tag, seq) : un
    // audit ne doit pas pouvoir confondre deux enregistrements, et la taille
    // variable interdit de calculer une position à partir d'un compte.
    let mut value = Vec::with_capacity(taille_payload);
    value.push(tag);
    value.extend(std::iter::repeat_n(
        (seq % 251) as u8,
        taille_payload.saturating_sub(1),
    ));
    WalRecord {
        lsn: 0, // écrasé par l'append (ancre + position)
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: seq as u32,
            key: format!("t{tag}s{seq}"),
            value_bincode: value,
        },
    }
}

fn identite(rec: &WalRecord) -> (u8, u64) {
    match &rec.payload {
        WalPayload::PropSetInline { key, .. } => {
            let k = key.strip_prefix('t').expect("clé t<tag>s<seq>");
            let (t, s) = k.split_once('s').expect("clé t<tag>s<seq>");
            (t.parse().unwrap(), s.parse().unwrap())
        }
        autre => panic!("payload inattendu : {autre:?}"),
    }
}

/// Identités des records lus, queue incomplète, trou éventuel (voir `lus`).
type Lecture = (Vec<(u8, u64)>, bool, Option<(u64, u64)>);

fn lus(wal: &Path) -> Lecture {
    let r = WalReader::open(wal).unwrap();
    let mut it = r.iter_from(0);
    let mut v = Vec::new();
    for item in it.by_ref() {
        match item {
            Ok((rec, _, _)) => v.push(identite(&rec)),
            Err(_) => break,
        }
    }
    (v, it.queue_incomplete(), it.trou())
}

// ===========================================================================
// T6 — `offset()` COHÉRENT AVEC L'ANCRE DE DURABILITÉ (MONO-PROCESSUS)
// ===========================================================================
//
// GARDE-FOU. `offset()` est comparé à des valeurs PERSISTÉES : baselines de
// flush (`store.rs`), ancres d'index d'égalité et d'ordre
// (`props_eq_index.rs::etendre_apres_fenetre` reçoit `ancre_wal`). Le fix de
// l'axe APPEND ne doit RIEN changer à sa valeur — bit-identique à la taille du
// fichier, à chaque instant, en mono-écrivain. Ce qui change, c'est le LSN :
// il est désormais `ancre + position`, et l'ancre AVANCE à chaque troncature
// pour qu'aucun LSN ne soit jamais réémis.
#[test]
fn t6_offset_bit_identique_et_ancre_de_durabilite() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();

    // (1) `offset()` == taille du fichier, à chaque pas, et l'ancre ne bouge pas
    //     tant qu'aucune troncature n'a eu lieu.
    let mut attendu = 0u64;
    for i in 0..50u64 {
        let (pos, len) = w.append(record(1, i, 16 + (i as usize % 97))).unwrap();
        assert_eq!(pos, attendu, "l'offset rendu est la position d'écriture");
        attendu += len as u64;
        assert_eq!(w.offset(), attendu, "offset() == taille du fichier");
        assert_eq!(
            std::fs::metadata(&wal).unwrap().len(),
            attendu,
            "la taille RÉELLE du fichier est l'arbitre"
        );
        assert_eq!(
            w.ancre(),
            ANCRE_INITIALE,
            "pas de troncature → ancre stable"
        );
    }

    // (2) Chaque frame porte `lsn = ancre + position`.
    let r = WalReader::open(&wal).unwrap();
    for item in r.iter_from(0) {
        let (rec, off, _) = item.unwrap();
        assert_eq!(rec.lsn, ANCRE_INITIALE + off, "lsn = ancre + position");
    }
    drop(r);

    // (3) Troncature : l'ancre avance de la taille du segment abandonné, et le
    //     premier LSN du nouveau segment est STRICTEMENT au-delà de tous ceux du
    //     précédent. C'est l'invariant qui interdit de RÉÉMETTRE un LSN.
    let taille_segment = w.offset();
    let derniers_lsn_avant: Vec<u64> = WalReader::open(&wal)
        .unwrap()
        .iter_from(0)
        .map(|i| i.unwrap().0.lsn)
        .collect();
    let max_avant = *derniers_lsn_avant.iter().max().unwrap();

    let nouvelle_ancre = w.truncate().unwrap();
    assert_eq!(nouvelle_ancre, ANCRE_INITIALE + taille_segment);
    assert_eq!(w.offset(), 0);
    assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0);
    assert_eq!(w.peek_lsn(), nouvelle_ancre, "le prochain LSN est l'ancre");

    for i in 0..10u64 {
        w.append(record(2, i, 24)).unwrap();
    }
    let apres: Vec<u64> = WalReader::open(&wal)
        .unwrap()
        .iter_from(0)
        .map(|i| i.unwrap().0.lsn)
        .collect();
    assert_eq!(apres.len(), 10);
    assert!(
        apres.iter().all(|l| *l > max_avant),
        "aucun LSN du nouveau segment ne recouvre le précédent \
         (avant max={max_avant}, après min={})",
        apres[0]
    );
    // Et l'ancre durable publiée dit la même chose que l'objet vivant.
    let a = lire_ancre_durable(&wal).expect("side-car publié");
    assert_eq!(a.ancre, nouvelle_ancre);
}

// ===========================================================================
// T7 — GROUP COMMIT : `append_batch`, 64 RECORDS, UN SEUL `write`
// ===========================================================================
#[test]
fn t7_group_commit_un_seul_write_et_ordre_preserve() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();

    // Un premier record seul, pour que le lot ne commence PAS à l'offset 0 (un
    // lot qui commence à 0 cacherait les erreurs de base de position).
    let (_, l0) = w.append(record(9, 0, 32)).unwrap();
    let base = l0 as u64;

    let lot: Vec<WalRecord> = (1..=64u64)
        .map(|s| record(7, s, 40 + (s as usize % 13)))
        .collect();
    let (pos, len) = w.append_batch(lot).unwrap();
    assert_eq!(pos, base, "le lot commence à la position courante");

    // ── LA PREUVE DU « UN SEUL write » ──────────────────────────────────────
    // La taille du fichier a augmenté d'EXACTEMENT la taille encodée du lot :
    // aucun octet parasite, et rien n'a été écrit deux fois.
    assert_eq!(
        std::fs::metadata(&wal).unwrap().len(),
        base + len as u64,
        "un lot = un seul write, de la taille exacte du lot"
    );
    assert_eq!(w.offset(), base + len as u64);

    // Ordre préservé + LSN dérivés de la position (le lot est, sur le disque,
    // indiscernable de 64 appends successifs).
    drop(w);
    let r = WalReader::open(&wal).unwrap();
    let mut it = r.iter_from(0);
    let mut vus = Vec::new();
    for item in it.by_ref() {
        let (rec, off, _) = item.unwrap();
        assert_eq!(rec.lsn, ANCRE_INITIALE + off, "lsn = ancre + position");
        vus.push(identite(&rec));
    }
    assert_eq!(vus.len(), 65, "le record seul + les 64 du lot");
    assert_eq!(vus[0], (9, 0));
    for (i, (tag, seq)) in vus[1..].iter().enumerate() {
        assert_eq!(*tag, 7);
        assert_eq!(*seq, i as u64 + 1, "l'ordre du LOT est l'ordre physique");
    }
    assert!(!it.queue_incomplete());
}

/// ── UN LOT REFUSÉ N'ÉCRIT RIEN (et laisse la queue intacte) ────────────────
///
/// L'échec est provoqué par la BORNE DE TAILLE : une frame qu'un seul `write`
/// ne peut pas poser d'un bloc doit être refusée À L'ENCODAGE, jamais confiée à
/// `write_all` (qui la découperait et fabriquerait une frame HYBRIDE). Le lot
/// est encodé ENTIÈREMENT avant le `write` : la frame fautive le fait donc
/// échouer sans qu'un seul octet ne parte.
#[test]
fn t7_lot_refuse_entierement_quand_une_frame_depasse_la_borne() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    let (_, l0) = w.append(record(1, 0, 16)).unwrap();
    let taille_avant = l0 as u64;
    assert_eq!(std::fs::metadata(&wal).unwrap().len(), taille_avant);

    // Une frame de ~2 Mio face à une borne de 1 Mio : le refus est réel, sur un
    // encodage réel — seule la BORNE est abaissée (sinon il faudrait allouer
    // 1 Gio pour l'exercer, et le chemin de refus serait le même).
    w.forcer_borne_frame(1 << 20);
    let lot = vec![
        record(2, 1, 1024),
        record(2, 2, 2 << 20), // ← dépasse la borne
        record(2, 3, 1024),
    ];
    let e = w.append_batch(lot).unwrap_err();
    match e {
        SubstrateError::WalFrameTropGrande { taille, max } => {
            assert!(taille > max);
            assert_eq!(max, 1 << 20);
        }
        autre => panic!("attendu WalFrameTropGrande, obtenu {autre:?}"),
    }
    assert_eq!(
        std::fs::metadata(&wal).unwrap().len(),
        taille_avant,
        "RIEN n'a été écrit : les 3 records du lot sont perdus ENSEMBLE, jamais à moitié"
    );
    assert_eq!(w.offset(), taille_avant);

    // Le WAL reste utilisable, et les octets déjà écrits sont intacts.
    w.forcer_borne_frame(obrain_substrate::MAX_FRAME_BYTES);
    w.append(record(3, 1, 16)).unwrap();
    let (v, queue, trou) = lus(&wal);
    assert_eq!(v, vec![(1, 0), (3, 1)]);
    assert!(!queue);
    assert!(trou.is_none());
}

// ===========================================================================
// ENOSPC — 0 OCTET (PROPRE) vs ÉCRITURE COURTE (DÉCHIRURE)
// ===========================================================================
//
// Le disque plein se présente sous DEUX formes opposées, et les confondre ferait
// tronquer un fichier sain :
//   · le noyau refuse la frame ENTIÈRE (0 octet transféré) : la queue est
//     intacte, il n'y a RIEN à réparer ;
//   · le noyau accepte une PARTIE de la frame : elle est DÉCHIRÉE, et elle doit
//     être annulée par troncature — sinon le rejeu s'arrêterait dessus et tout
//     ce qui suivrait serait invisible.
//
// `/dev/full` rend le premier cas pour de vrai (write → ENOSPC, 0 octet) ; le
// second est couvert par `wal_ecriture_courte.rs` (RLIMIT_FSIZE), qui a besoin
// d'un binaire de test à lui.
#[test]
fn enospc_zero_octet_le_refus_est_propre_et_ne_repare_rien() {
    if !Path::new("/dev/full").exists() {
        eprintln!("  /dev/full absent (non-Linux ?) — check ENOSPC 0 octet NON exécuté");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Le side-car d'ancre vit dans le répertoire temporaire (inscriptible) : le
    // WAL, lui, est `/dev/full` — c'est le seul montage qui donne un vrai ENOSPC
    // sans droits particuliers.
    let ancre = dir.path().join("w.wal.lsn");
    let w = WalWriter::open_avec_ancre("/dev/full", &ancre, SyncMode::Never, ANCRE_INITIALE)
        .expect("ouvrir /dev/full");

    let e = w.append(record(1, 0, 64)).unwrap_err();
    match &e {
        SubstrateError::Io(io) => {
            // Le noyau a refusé la frame ENTIÈRE. `StorageFull` sur Linux,
            // éventuellement `WriteZero` ailleurs : les deux disent « 0 octet ».
            assert!(
                matches!(
                    io.kind(),
                    std::io::ErrorKind::StorageFull | std::io::ErrorKind::WriteZero
                ),
                "0 octet écrit doit se dire comme tel, obtenu {:?}",
                io.kind()
            );
        }
        SubstrateError::WalEcritureCourte { .. } => panic!(
            "0 octet n'est PAS une écriture courte : rien n'est déchiré, \
             et le confondre ferait tronquer un fichier sain"
        ),
        autre => panic!("attendu Io(StorageFull), obtenu {autre:?}"),
    }
    assert!(
        w.queue_a_repairer().is_none(),
        "0 octet = queue INTACTE : aucune réparation n'est due"
    );
}

/// ── REFUS TANT QUE LA QUEUE N'EST PAS RÉPARÉE ─────────────────────────────
///
/// Après une écriture courte dont l'annulation a échoué, la queue reste
/// déchirée : écrire à sa suite produirait des frames qu'AUCUN lecteur ne
/// verrait (le rejeu s'arrête sur la déchirure). Le refus doit donc être TOTAL,
/// NOMMÉ, et levé par la réparation — jamais par un contournement.
#[test]
fn queue_dechiree_tout_append_est_refuse_jusqu_a_reparation() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    let (_, l0) = w.append(record(1, 0, 16)).unwrap();
    let taille = l0 as u64;

    w.forcer_queue_a_repairer("écriture courte non annulable (injection de test)");
    let e = w.append(record(1, 1, 16)).unwrap_err();
    match &e {
        SubstrateError::WalQueueAPreparer { raison } => {
            assert!(raison.contains("écriture courte non annulable"));
        }
        autre => panic!("attendu WalQueueAPreparer, obtenu {autre:?}"),
    }
    // Le refus est TOTAL : rien n'a été écrit, et le refus se LIT dans le
    // message (l'opérateur doit savoir quoi appeler).
    assert_eq!(std::fs::metadata(&wal).unwrap().len(), taille);
    let texte = format!("{e}");
    assert!(
        texte.contains("reparer_queue"),
        "le refus doit nommer le remède : {texte}"
    );

    assert!(
        w.reparer_queue().unwrap().is_none(),
        "queue saine, rien à faire"
    );
    assert!(w.queue_a_repairer().is_none(), "le drapeau est levé");
    w.append(record(1, 2, 16)).unwrap();
    let (v, queue, trou) = lus(&wal);
    assert_eq!(v, vec![(1, 0), (1, 2)]);
    assert!(!queue);
    assert!(trou.is_none());
}

// ===========================================================================
// RÉCONCILIATION D'OUVERTURE — LES TROIS CAS NOMMÉS
// ===========================================================================

/// **(1) Ancre adoptée** — le side-car décrit le fichier : aucune écriture, et
/// les LSN reprennent exactement là où ils s'étaient arrêtés.
#[test]
fn cas_1_ancre_adoptee_aucun_lsn_reemis() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let (ancre, lsn_max, taille) = {
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 0..20u64 {
            w.append(record(1, i, 32)).unwrap();
        }
        let lsn_max = WalReader::open(&wal)
            .unwrap()
            .iter_from(0)
            .map(|i| i.unwrap().0.lsn)
            .max()
            .unwrap();
        (w.ancre(), lsn_max, w.offset())
    };

    // Réouverture : le side-car existe et décrit ce fichier.
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    assert_eq!(
        w.reconciliation(),
        Some(Reconciliation::AncreAdoptee { ancre })
    );
    assert_eq!(w.offset(), taille, "l'offset est repris tel quel");
    let (_, _) = w.append(record(2, 0, 32)).unwrap();
    drop(w);

    let r = WalReader::open(&wal).unwrap();
    let lsns: Vec<u64> = r.iter_from(0).map(|i| i.unwrap().0.lsn).collect();
    assert_eq!(lsns.len(), 21);
    assert!(
        lsns[20] > lsn_max,
        "le premier LSN après réouverture est AU-DELÀ du plus grand déjà écrit \
         (max avant = {lsn_max}, écrit après = {})",
        lsns[20]
    );
    let mut uniques = lsns.clone();
    uniques.sort_unstable();
    uniques.dedup();
    assert_eq!(uniques.len(), lsns.len(), "aucun LSN réémis");
}

/// **(2) Ré-ancrage en avant** — un WAL qui porte des frames mais AUCUNE ancre
/// qui les décrive (side-car perdu, base écrite par une version antérieure). On
/// ré-ancre AU-DELÀ du plus grand LSN présent : jamais de réémission.
#[test]
fn cas_2_reancrage_en_avant_quand_le_side_car_manque() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let lsn_max = {
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for i in 0..5u64 {
            w.append(record(1, i, 48)).unwrap();
        }
        WalReader::open(&wal)
            .unwrap()
            .iter_from(0)
            .map(|i| i.unwrap().0.lsn)
            .max()
            .unwrap()
    };
    // Le side-car disparaît (base restaurée seule, ou écrite avant l'ancre).
    std::fs::remove_file(obrain_substrate::chemin_ancre(&wal)).unwrap();

    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    match w.reconciliation() {
        Some(Reconciliation::ReancrageEnAvant {
            ancre_avant,
            ancre_apres,
            lsn_max_fichier,
        }) => {
            assert_eq!(ancre_avant, ANCRE_INITIALE);
            assert_eq!(
                lsn_max_fichier, lsn_max,
                "le fichier a été MESURÉ, pas deviné"
            );
            assert_eq!(ancre_apres, lsn_max + 1);
            assert_eq!(w.ancre(), ancre_apres);
        }
        autre => panic!("attendu ReancrageEnAvant, obtenu {autre:?}"),
    }
    w.append(record(2, 0, 48)).unwrap();
    let lsns: Vec<u64> = WalReader::open(&wal)
        .unwrap()
        .iter_from(0)
        .map(|i| i.unwrap().0.lsn)
        .collect();
    assert_eq!(lsns.len(), 6);
    let mut uniques = lsns.clone();
    uniques.sort_unstable();
    uniques.dedup();
    assert_eq!(uniques.len(), 6, "aucun LSN réémis après ré-ancrage");
}

/// **(3) Troncature étrangère** — le plancher du side-car décrit un segment plus
/// long que le fichier : quelqu'un a tronqué hors de notre dos. Le cas est
/// NOMMÉ, et le plancher est republié à la taille réelle.
#[test]
fn cas_3_troncature_etrangere_nommee_a_l_ouverture() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    for i in 0..8u64 {
        w.append(record(1, i, 32)).unwrap();
    }
    // Le scellement du segment (ce que fait le checkpoint après son marqueur)
    // publie le PLANCHER : c'est lui qui rend la troncature détectable.
    w.sceller_segment().unwrap();
    let ancre_avant = w.ancre();
    drop(w);

    // Troncature ÉTRANGÈRE : une main extérieure coupe le fichier en deux.
    let taille = std::fs::metadata(&wal).unwrap().len();
    let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
    f.set_len(taille / 2).unwrap();
    drop(f);

    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    match w.reconciliation() {
        Some(Reconciliation::TroncatureEtrangere {
            ancre,
            plancher_attendu,
            taille_reelle,
        }) => {
            assert_eq!(ancre, ancre_avant, "l'ancre est CONSERVÉE");
            assert_eq!(plancher_attendu, taille);
            assert_eq!(taille_reelle, taille / 2);
        }
        autre => panic!("attendu TroncatureEtrangere, obtenu {autre:?}"),
    }
    assert_eq!(w.offset(), taille / 2, "l'offset est la taille RÉELLE");
    // Le plancher a été republié à la taille réelle : le défaut ne se répète pas
    // à chaque ouverture (il est dit UNE fois, puis il est acté).
    let a = lire_ancre_durable(&wal).unwrap();
    assert_eq!(a.plancher, taille / 2);
    // Et le WAL reste utilisable : l'append suit la fin réelle, sans trou.
    let (pos, _) = w.append(record(3, 0, 32)).unwrap();
    assert_eq!(pos, taille / 2);
}

// ===========================================================================
// LECTEUR QUI REFUSE LES TROUS — le fait doit être NOMMÉ
// ===========================================================================

/// Un TROU (zéros suivis de données) n'est ni une fin de fichier pré-allouée ni
/// une frontière de crash. Le lecteur doit le NOMMER, avec ses bornes — et la
/// lecture REPREND après lui (voir l'arbitrage de réunification plus bas).
#[test]
fn un_trou_de_zeros_est_nomme_et_la_reparation_refuse_de_trancher() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    for i in 0..5u64 {
        w.append(record(1, i, 48)).unwrap();
    }
    let bornes: Vec<(u64, usize)> = WalReader::open(&wal)
        .unwrap()
        .iter_from(0)
        .map(|i| {
            let (_, off, len) = i.unwrap();
            (off, len)
        })
        .collect();
    drop(w);

    // On efface la TROISIÈME frame (zéros), en laissant les deux dernières.
    let (off3, len3) = bornes[2];
    let mut octets = std::fs::read(&wal).unwrap();
    for b in &mut octets[off3 as usize..off3 as usize + len3] {
        *b = 0;
    }
    std::fs::write(&wal, &octets).unwrap();

    let r = WalReader::open(&wal).unwrap();
    let mut it = r.iter_from(0);
    let mut vus = 0usize;
    let mut erreur = None;
    for item in it.by_ref() {
        match item {
            Ok((rec, _, _)) => {
                assert_eq!(identite(&rec).0, 1);
                vus += 1;
            }
            Err(e) => {
                erreur = Some(e);
                break;
            }
        }
    }
    // ── ARBITRAGE DE RÉUNIFICATION (2026-10-06) ─────────────────────────────
    // Ce test et `wal_io::tests::un_wal_deja_troue_se_relit_en_entier` (lignée
    // servie) étaient DIRECTEMENT CONTRADICTOIRES : celui-ci exigeait que le
    // lecteur S'ARRÊTE sur le trou en rendant `WalTrou`, celui-là exige qu'il
    // REPRENNE et relise les frames d'après. Aucune implémentation ne peut
    // satisfaire les deux — il a fallu trancher.
    //
    // Tranché en faveur de la REPRISE (lignée servie) : c'est le comportement
    // DÉPLOYÉ, il est adossé à un incident réellement mesuré (7 964 288 octets
    // nuls sur runtime-dev), et des stores DÉJÀ troués sur disque en dépendent —
    // s'arrêter au trou, c'est cesser de rejouer du contenu qu'on sait lire.
    //
    // Tout ce que ce test affirmait et qui NE contredit PAS la reprise est
    // conservé : le trou reste NOMMÉ à ses bornes exactes (`it.trou()`), et il
    // n'est toujours pas une frontière de crash.
    assert_eq!(
        vus, 4,
        "les deux frames AVANT le trou, ET les deux APRÈS (la lecture reprend)"
    );
    assert!(
        erreur.is_none(),
        "la reprise après un trou ne rend PAS d'erreur : les frames d'après \
         sont lues (obtenu {erreur:?})"
    );
    assert_eq!(
        it.trou(),
        Some((off3, len3 as u64)),
        "le trou reste NOMMÉ, à ses bornes exactes (apport de la pièce 6)"
    );
    assert!(
        !it.queue_incomplete(),
        "un trou n'est PAS une frontière de crash : les confondre ferait \
         passer une perte de contenu pour une panne ordinaire"
    );

    // ── LA RÉPARATION NE TRANCHE TOUJOURS PAS SUR UN TROU ───────────────────
    // L'ISSUE DE SÛRETÉ est identique dans les deux lignées, et c'est elle qui
    // compte : tronquer ramènerait au bord avant le trou et effacerait les deux
    // frames VALIDES qui le suivent — une perte silencieuse, exactement ce que
    // ce dépôt refuse. Seul le CANAL change : la lecture reprenant après le
    // trou, la queue est PROPRE, donc il n'y a rien à trancher et la réparation
    // rend `Ok(None)` — au lieu du `Err(WalTrou)` de la pièce 6, qui décrivait
    // un arrêt de lecture qui n'a plus lieu. Le fichier, lui, n'est pas touché.
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    let r = w.reparer_queue();
    assert!(
        r.is_ok() && r.as_ref().unwrap().is_none(),
        "la réparation d'un trou doit REFUSER de trancher, et ne rien réparer"
    );
    assert_eq!(
        std::fs::metadata(&wal).unwrap().len(),
        octets.len() as u64,
        "le fichier n'a pas été touché : les frames après le trou sont intactes"
    );
}
