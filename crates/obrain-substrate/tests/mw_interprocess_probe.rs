//! SONDE INTER-PROCESSUS — ce que le WAL fait réellement quand DEUX PROCESSUS
//! partagent le même store (le cas que la RFC multi-writer déclare hors portée,
//! `docs/rfc/substrate/multi-writer.md` §6, et que le filet I1-I12 ne couvre
//! NULLE PART : I5 `multi_writer_wal.rs` est MONO-PROCESSE, multi-THREADS).
//!
//! Harnais : le binaire de test se RELANCE lui-même en enfant
//! (`std::env::current_exe()` + `--exact <ce test> --nocapture`), le rôle est
//! choisi par variable d'environnement, la synchronisation passe par des lignes
//! sur stdout (« PRET »/« FIN ») et par des fichiers-drapeaux. C'est le motif
//! déjà éprouvé de `verrou_apres_drop.rs::un_refus_de_verrou_nomme_le_detenteur`.
//!
//! ── CES SONDES ONT DOCUMENTÉ UN DÉFAUT ; ELLES GARDENT MAINTENANT LE FIX ───
//!
//! Écrites AVANT l'axe APPEND (2026-10-06), elles mesuraient trois défauts et se
//! contentaient de les IMPRIMER (une porte `OBRAIN_MW_GATE` permettait de les
//! exiger). Deux de ces défauts n'existent plus — les sondes ont donc changé de
//! camp : elles EXIGENT désormais la garantie, sans porte, parce qu'une garantie
//! acquise qui n'est plus testée est une garantie perdue.
//!
//! 1. ~~`WalWriter::open` n'ouvre PAS en `O_APPEND`~~ → **CORRIGÉ** : le fichier
//!    est ouvert `.append(true)`. Deux processus ne peuvent plus s'écraser : la
//!    sonde exige maintenant la SURVIE INTÉGRALE des frames du second écrivain
//!    (elle mesurait leur disparition).
//! 2. ~~Le LSN est un compteur PAR INSTANCE~~ → **CORRIGÉ** : le LSN est dérivé
//!    de la position (`lsn = ancre + position`), l'ancre vivant dans le side-car
//!    `substrate.wal.lsn`. La sonde exige l'UNICITÉ des LSN entre processus.
//! 3. `open_shared_write` pose `LOCK_SH` (`file.rs:1081`) : deux processus
//!    shared-write coexistent toujours sans se refuser — l'unicité du WRITER
//!    reste une convention d'appelant. C'est ce fait-là qui justifie que le WAL
//!    se coordonne lui-même (`CoordinationAppend`), au lieu de compter sur le
//!    verrou de répertoire.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{SyncMode, WalReader, WalWriter};
use obrain_substrate::SubstrateFile;

/// Records écrits par chaque processus dans les sondes de perte.
const M: u64 = 2000;

/// ── LA PORTE EST-ELLE ARMÉE ? ──────────────────────────────────────────────
/// Les sondes 1 et 2 encodent une garantie que le code N'A PAS ENCORE : elles
/// mesurent et IMPRIMENT toujours, mais n'exigent la garantie que si la porte
/// est armée (`OBRAIN_MW_GATE=1`) — même contrat que `porte_armee()` de
/// `multi_writer_invariants.rs:410-413` (un test qui échoue selon l'état du
/// chantier n'apprend rien tant que le chantier n'est pas engagé ; ce qui doit
/// rester vrai, c'est que le DÉFAUT SE DIT).
// ---------------------------------------------------------------------------
// Harnais
// ---------------------------------------------------------------------------

fn role() -> Option<String> {
    std::env::var("MW_ROLE").ok()
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn attendre_fichier(p: &Path, quoi: &str) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(20),
            "attente de {quoi} ({}) : délai dépassé",
            p.display()
        );
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// Un enfant + son flux de stdout, tenu OUVERT pour toute la vie de l'enfant
/// (la barrière « PRET » puis « FIN » se lit sur le MÊME descripteur).
struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    /// Lance CE binaire en enfant, sur CE test uniquement, avec un rôle.
    fn lancer(test: &str, role_enfant: &str, vars: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--exact", test, "--nocapture"])
            .env("MW_ROLE", role_enfant)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        Self {
            process,
            lignes: std::io::BufReader::new(sortie).lines(),
        }
    }

    /// Barrière : bloque jusqu'à la ligne attendue sur stdout de l'enfant.
    fn attendre(&mut self, attendue: &str) {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) if l.contains(attendue) => return,
                Some(Ok(_)) => continue,
                autre => panic!("enfant terminé sans « {attendue} » ({autre:?})"),
            }
        }
    }

    /// Lit stdout jusqu'au verdict préfixé (ligne contenant `EXCLUSIF_`).
    fn capter_verdict(&mut self, dans: &mut String) {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) if l.contains("EXCLUSIF_") => {
                    *dans = l;
                    return;
                }
                Some(Ok(_)) => continue,
                autre => panic!("enfant tiers sans verdict EXCLUSIF_ ({autre:?})"),
            }
        }
    }

    fn terminer(&mut self) {
        let _ = self.process.wait();
    }

    fn tuer(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Record distinct par (tag, seq) — le tag est dans les 8 bits hauts du bitset.
fn record(tag: u8, seq: u64) -> WalRecord {
    WalRecord {
        lsn: 0, // écrasé par WalWriter::append
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::NodeInsert {
            node_id: seq as u32,
            label_bitset: ((tag as u64) << 56) | seq,
        },
    }
}

fn tag_de(rec: &WalRecord) -> u8 {
    match &rec.payload {
        WalPayload::NodeInsert { label_bitset, .. } => (label_bitset >> 56) as u8,
        _ => 0,
    }
}

fn seq_de(rec: &WalRecord) -> u64 {
    match &rec.payload {
        WalPayload::NodeInsert { label_bitset, .. } => label_bitset & 0x00FF_FFFF_FFFF_FFFF,
        _ => 0,
    }
}

// ===========================================================================
// SONDE 1 — deux processus, deux descripteurs, un seul WAL : ÉCRASEMENT
// ===========================================================================
//
// Les deux enfants ouvrent le WAL (vide) AVANT toute écriture : leurs deux
// descripteurs sont donc positionnés à l'offset 0. On laisse B écrire en
// entier, PUIS on libère A. Si l'append était atomique au niveau du noyau
// (`O_APPEND`), les 2×M frames seraient là. Sinon A réécrit par-dessus B.
#[test]
fn p1_deux_processus_ecrivent_au_meme_offset() {
    let test = "p1_deux_processus_ecrivent_au_meme_offset";
    if role().as_deref() == Some("p1") {
        let wal: PathBuf = env_obligatoire("MW_WAL").into();
        let tag: u8 = env_obligatoire("MW_TAG").parse().unwrap();
        let go: PathBuf = env_obligatoire("MW_GO").into();
        let w = WalWriter::open(&wal, SyncMode::Never, 1).expect("open WAL enfant");
        annoncer("PRET"); // descripteur positionné à l'offset 0, fichier vide
        attendre_fichier(&go, "le top départ");
        for i in 0..M {
            w.append(record(tag, i)).expect("append");
        }
        annoncer("FIN");
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();
    let go_a = td.path().join("go_a");
    let go_b = td.path().join("go_b");

    let mut a = Enfant::lancer(
        test,
        "p1",
        &[
            ("MW_WAL", wal.display().to_string()),
            ("MW_TAG", "1".into()),
            ("MW_GO", go_a.display().to_string()),
        ],
    );
    let mut b = Enfant::lancer(
        test,
        "p1",
        &[
            ("MW_WAL", wal.display().to_string()),
            ("MW_TAG", "2".into()),
            ("MW_GO", go_b.display().to_string()),
        ],
    );
    a.attendre("PRET");
    b.attendre("PRET");

    // B écrit tout ; puis A écrit tout. Aucun verrou, aucun O_APPEND.
    std::fs::write(&go_b, b"go").unwrap();
    b.attendre("FIN");
    b.terminer();
    let taille_apres_b = std::fs::metadata(&wal).unwrap().len();

    std::fs::write(&go_a, b"go").unwrap();
    a.attendre("FIN");
    a.terminer();

    let octets = std::fs::read(&wal).unwrap();
    let lecteur = WalReader::open(&wal).unwrap();
    let frames: Vec<WalRecord> = lecteur
        .iter_from(0)
        .map(|it| it.expect("toutes les frames restantes ont un CRC valide").0)
        .collect();
    let de_a = frames.iter().filter(|r| tag_de(r) == 1).count();
    let de_b = frames.iter().filter(|r| tag_de(r) == 2).count();
    // Une frame de cette sonde = taille du fichier / nombre de frames quand
    // toutes les écritures ont survécu (payload de taille constante).
    let frame_octets = if frames.is_empty() {
        0
    } else {
        taille_apres_b / M
    };
    let attendu_final = 2 * M * frame_octets;
    eprintln!(
        "SONDE 1 : frame={frame_octets} o · frames lues={} (A={de_a}, B={de_b}) · \
         attendu par processus={M} · taille après B={taille_apres_b} o · \
         taille finale={} o (attendu si append atomique : {attendu_final} o)",
        frames.len(),
        octets.len()
    );
    eprintln!(
        "  → les frames de B sont TOUTES décodables (CRC valide) : aucune corruption \
         détectable — {} frames de B ont été écrasées par A au même offset.",
        M as usize - de_b
    );

    // ── EXIGENCE (sans porte) : LES FRAMES DE B SURVIVENT ───────────────────
    // Avant le fix, cette sonde mesurait `de_b == 0` : les M frames du second
    // écrivain étaient écrasées par le premier, une par une, au même offset.
    // `O_APPEND` fait placer chaque écriture à la fin du fichier par le noyau :
    // les 2×M frames doivent être là, intégralement.
    assert_eq!(
        de_a, M as usize,
        "les {M} frames du processus A doivent TOUTES survivre (O_APPEND)"
    );
    assert_eq!(
        de_b, M as usize,
        "les {M} frames du processus B doivent TOUTES survivre — avant le fix, \
         elles disparaissaient (écrasement au même offset de descripteur)"
    );
    assert_eq!(
        frames.len(),
        2 * M as usize,
        "le WAL doit contenir exactement les 2×M frames des deux processus"
    );
}

// ===========================================================================
// SONDE 2 — le LSN est PAR INSTANCE : deux processus émettent LSN 1
// ===========================================================================
//
// Étape 1 : A ouvre (LSN semé à 1), écrit 1 record (LSN 1), et attend.
// Étape 2 : B ouvre (il voit le fichier non vide, se positionne à la fin —
// là, pas d'écrasement) et écrit SON record : son compteur repart de 1 aussi.
// La relecture montre donc DEUX frames de LSN 1 dans le même WAL.
#[test]
fn p2_lsn_par_instance_se_duplique_entre_processus() {
    let test = "p2_lsn_par_instance_se_duplique_entre_processus";
    if role().as_deref() == Some("p2a") {
        let wal: PathBuf = env_obligatoire("MW_WAL").into();
        let go: PathBuf = env_obligatoire("MW_GO").into();
        let w = WalWriter::open(&wal, SyncMode::Never, 1).expect("open A");
        w.append(record(1, 0)).expect("append A");
        annoncer("PRET"); // A a écrit sa frame, B peut ouvrir
        attendre_fichier(&go, "la fin de B");
        annoncer("FIN");
        return;
    }
    if role().as_deref() == Some("p2b") {
        let wal: PathBuf = env_obligatoire("MW_WAL").into();
        let w = WalWriter::open(&wal, SyncMode::Never, 1).expect("open B");
        // B est un PROCESSUS DIFFÉRENT : il ne sait rien du compteur de A.
        eprintln!("SONDE 2 : B ouvre avec next_lsn={}", w.peek_lsn());
        w.append(record(2, 0)).expect("append B");
        annoncer("FIN");
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();
    let go_a = td.path().join("go_a");

    let mut a = Enfant::lancer(
        test,
        "p2a",
        &[
            ("MW_WAL", wal.display().to_string()),
            ("MW_GO", go_a.display().to_string()),
        ],
    );
    a.attendre("PRET"); // A a écrit 1 frame (LSN 1)

    let mut b = Enfant::lancer(
        test,
        "p2b",
        &[("MW_WAL", wal.display().to_string())],
    );
    b.attendre("FIN");
    b.terminer();

    std::fs::write(&go_a, b"go").unwrap();
    a.attendre("FIN");
    a.terminer();

    let lecteur = WalReader::open(&wal).unwrap();
    let lsn: Vec<u64> = lecteur
        .iter_from(0)
        .map(|it| it.expect("frame décodable").0.lsn)
        .collect();
    eprintln!(
        "SONDE 2 : {} frames écrites par 2 processus, LSN lus = {lsn:?}",
        lsn.len()
    );

    // ── EXIGENCE (sans porte) : LE LSN EST UN IDENTIFIANT GLOBAL ────────────
    // Avant le fix, les deux processus émettaient tous deux le LSN 1 (compteur
    // par instance, semé sur `last_wal_offset`), donc `uniques.len() == 1`. Le
    // LSN est maintenant `ancre + position`, et l'ancre est partagée par le
    // side-car : deux frames ne peuvent pas porter le même LSN.
    let uniques: std::collections::HashSet<u64> = lsn.iter().copied().collect();
    assert_eq!(
        uniques.len(),
        lsn.len(),
        "les LSN de deux PROCESSUS doivent être distincts (ancre partagée) — \
         mesuré : {lsn:?}"
    );
    assert_eq!(
        lsn.len(),
        2,
        "chaque processus a écrit exactement une frame"
    );
}

// ===========================================================================
// SONDE 3 — `open_shared_write` : deux PROCESSUS écrivaient le même store
// ===========================================================================
//
// L'enfant tient un shared-write ; le parent (processus 2) ouvre le MÊME store
// en shared-write : doit-il être refusé ? Non — le verrou est `LOCK_SH`
// (`file.rs:1081`), donc partagé. On vérifie aussi que l'exclusif, lui, est
// bien refusé depuis l'autre processus (I4 « partiel » de la RFC §4).
#[test]
fn p3_deux_processus_shared_write_coexistent() {
    let test = "p3_deux_processus_shared_write_coexistent";
    if role().as_deref() == Some("p3_shared") {
        let chemin: PathBuf = env_obligatoire("MW_STORE").into();
        let _gardien = SubstrateFile::open_shared_write(&chemin).expect("shared-write enfant");
        annoncer("PRET");
        std::thread::sleep(Duration::from_secs(10));
        return;
    }
    if role().as_deref() == Some("p3_excl") {
        let chemin: PathBuf = env_obligatoire("MW_STORE").into();
        match SubstrateFile::open(&chemin) {
            Ok(_) => annoncer("EXCLUSIF_ACCEPTE"),
            Err(e) => annoncer(&format!("EXCLUSIF_REFUSE:{e}")),
        }
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    drop(SubstrateFile::create(&chemin).expect("créer le store"));

    let mut gardien = Enfant::lancer(
        test,
        "p3_shared",
        &[("MW_STORE", chemin.display().to_string())],
    );
    gardien.attendre("PRET");

    // Le PARENT est le second processus écrivain sur le même store.
    let second = SubstrateFile::open_shared_write(&chemin);
    let excl = SubstrateFile::open(&chemin);

    let mut demandeur = Enfant::lancer(
        test,
        "p3_excl",
        &[("MW_STORE", chemin.display().to_string())],
    );
    // Le verdict de l'enfant tiers : « EXCLUSIF_ACCEPTE » ou « EXCLUSIF_REFUSE:… ».
    let mut verdict = String::new();
    demandeur.capter_verdict(&mut verdict);
    demandeur.terminer();

    eprintln!(
        "SONDE 3 : 2ᵉ shared-write (parent, autre processus) = {} · \
         ouvertures exclusives refusées : parent={} · enfant tiers={}",
        if second.is_ok() { "ACCEPTÉ" } else { "REFUSÉ" },
        excl.is_err(),
        verdict.contains("EXCLUSIF_REFUSE")
    );

    assert!(
        second.is_ok(),
        "MESURE : deux PROCESSUS peuvent tenir un shared-write sur le même store \
         (`LOCK_SH`, `file.rs:1081`) — l'unicité du writer est une convention \
         d'appelant, pas une garantie du verrou (RFC §6)"
    );
    assert!(
        excl.is_err(),
        "un writer EXCLUSIF pendant un shared-write vivant doit être refusé (I4)"
    );
    gardien.tuer();
}
