//! PREUVE ROUGE→VERT de l'axe APPEND — T2 dans sa forme la PLUS DURE.
//!
//! ── POURQUOI CE FICHIER EXISTE, ALORS QUE `t2` EXISTE DÉJÀ ────────────────
//! `mw_interprocess_append.rs` mesure T2 avec l'API COMPLÈTE de l'axe APPEND
//! (ancre, `reparer_queue`, compteurs de diagnostic) : sur l'état PRÉ-fix, il ne
//! COMPILE pas — ce qui prouve qu'il garde le fix, mais ne mesure pas le défaut.
//! Ce fichier-ci n'utilise QUE le sous-ensemble d'API commun aux deux états
//! (`WalWriter::open`, `append`, `WalReader`). Il compile donc AVANT comme APRÈS,
//! et c'est lui qui porte la mesure :
//!
//!   · AVANT le fix : N processus écrivent chacun M frames ; le second descripteur
//!     ÉCRASE le premier à partir de l'offset 0 (pas d'`O_APPEND`), frame par
//!     frame, avec des CRC tous valides. Mesuré : ~74 % de perte SILENCIEUSE.
//!   · APRÈS : `O_APPEND` place chaque écriture à la fin, atomiquement. Zéro
//!     perte, EOF == taille exacte des frames.
//!
//! C'est le check de non-régression de l'axe : il ne peut pas passer par
//! accident, et il ne peut pas cesser d'échouer sans qu'`O_APPEND` disparaisse.
//!
//! ── CE QU'IL N'EXIGE PAS ──────────────────────────────────────────────────
//! Aucune propriété de LSN : le LSN n'existe pas dans le sous-ensemble commun
//! (`next_lsn` par processus avant, `ancre + position` après). La non-perte est
//! la garantie que LES DEUX états revendiquent — c'est donc la seule qui puisse
//! être mesurée sur les deux.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{SyncMode, WalReader, WalWriter};

/// Enregistrements par processus. 2 000 comme la sonde `p1` : assez pour que
/// l'écrasement soit massif, assez court pour que le test reste rapide.
const M: u64 = 2000;

fn record(tag: u8, seq: u64) -> WalRecord {
    WalRecord {
        lsn: 0, // écrasé par l'append, dans les deux états
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

struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    fn lancer(test: &str, tag: u8, wal: &Path, go: &Path) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--exact", test, "--nocapture"])
            .env("MW2_TAG", tag.to_string())
            .env("MW2_WAL", wal.display().to_string())
            .env("MW2_GO", go.display().to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        Self {
            process,
            lignes: std::io::BufReader::new(sortie).lines(),
        }
    }

    fn attendre(&mut self, attendue: &str) {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) if l.contains(attendue) => return,
                Some(Ok(_)) => continue,
                autre => panic!("enfant terminé sans « {attendue} » ({autre:?})"),
            }
        }
    }

    fn terminer(&mut self) {
        let _ = self.process.wait();
    }
}

fn attendre_fichier(p: &Path) {
    let debut = std::time::Instant::now();
    while !p.exists() {
        assert!(debut.elapsed() < Duration::from_secs(30), "attente du top départ");
        std::thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn t2_zero_perte_inter_processus() {
    let test = "t2_zero_perte_inter_processus";
    if let Ok(tag) = std::env::var("MW2_TAG") {
        let tag: u8 = tag.parse().unwrap();
        let wal: PathBuf = std::env::var("MW2_WAL").unwrap().into();
        let go: PathBuf = std::env::var("MW2_GO").unwrap().into();
        let w = WalWriter::open(&wal, SyncMode::Never, 1).expect("open");
        // TOUS les descripteurs sont ouverts AVANT la moindre écriture : c'est le
        // décor exact du défaut (chaque descripteur croit être à l'offset 0).
        println!("PRET");
        std::io::stdout().flush().ok();
        attendre_fichier(&go);
        for i in 0..M {
            w.append(record(tag, i)).expect("append");
        }
        println!("FIN");
        std::io::stdout().flush().ok();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    let n: u8 = 6;
    let mut enfants = Vec::new();
    for tag in 1..=n {
        let go = td.path().join(format!("go_{tag}"));
        enfants.push((go.clone(), Enfant::lancer(test, tag, &wal, &go)));
    }
    for (_, e) in enfants.iter_mut() {
        e.attendre("PRET");
    }
    for (go, _) in enfants.iter() {
        std::fs::write(go, b"go").unwrap();
    }
    for (_, e) in enfants.iter_mut() {
        e.attendre("FIN");
        e.terminer();
    }

    let taille = std::fs::metadata(&wal).unwrap().len();
    let r = WalReader::open(&wal).unwrap();
    let mut it = r.iter_from(0);
    let mut vus = std::collections::HashSet::new();
    let mut octets = 0u64;
    let mut lues = 0usize;
    while let Some(item) = it.next() {
        match item {
            Ok((rec, _, len)) => {
                vus.insert((tag_de(&rec), seq_de(&rec)));
                octets += len as u64;
                lues += 1;
            }
            Err(_) => break,
        }
    }
    let attendus: std::collections::HashSet<(u8, u64)> = (1..=n)
        .flat_map(|t| (0..M).map(move |s| (t, s)))
        .collect();
    let perdues = attendus.difference(&vus).count();
    eprintln!(
        "T2 (forme dure) : {n} processus × {M} frames · attendues={} · lues={lues} · \
         PERDUES={perdues} ({:.1} %) · taille={taille} o · octets des frames lues={octets} o",
        attendus.len(),
        100.0 * perdues as f64 / attendus.len() as f64,
    );

    assert_eq!(
        perdues, 0,
        "PERTE SILENCIEUSE de {perdues} frames sur {} ({:.1} %) : deux processus \
         écrivaient au MÊME offset de descripteur — l'append n'est pas atomique \
         au niveau du noyau (pas d'O_APPEND). C'est LE défaut que l'axe APPEND \
         corrige ; le mesurer est le seul moyen de savoir qu'il est parti.",
        attendus.len(),
        100.0 * perdues as f64 / attendus.len() as f64,
    );
    // (`queue_incomplete` existe dans les deux états ; la détection de TROU,
    // elle, n'existe qu'APRÈS le fix — elle est donc vérifiée par
    // `mw_interprocess_append.rs` et `wal_append_unite.rs`, pas ici.)
    assert!(!it.queue_incomplete(), "aucune queue déchirée");
    assert_eq!(
        octets, taille,
        "EOF == taille EXACTE des frames : aucune frame coupée, aucun octet parasite"
    );
}
