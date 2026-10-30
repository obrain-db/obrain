//! AXE APPEND — **checks INTER-PROCESSUS** (T2, T3, T4, T7 multi-processus,
//! débit à N écrivains).
//!
//! ── POURQUOI DES PROCESSUS, ET PAS DES THREADS ─────────────────────────────
//! Le défaut central de l'axe APPEND ne se voit QU'entre processus : deux
//! `WalWriter` de deux processus avaient chacun leur descripteur positionné à
//! leur propre offset et leur propre compteur de LSN. Les tests de la RFC
//! (`multi_writer_wal.rs`) sont MONO-PROCESSE, multi-THREADS : ils ne pouvaient
//! pas le voir. Mesuré le 2026-10-06, sur l'état PRÉ-fix : **74 % de perte
//! silencieuse** (`mw_interprocess_probe::p1` — le second descripteur écrase le
//! premier frame par frame, avec des CRC tous valides).
//!
//! ── HARNAIS ────────────────────────────────────────────────────────────────
//! Le binaire de test se RELANCE lui-même en enfant
//! (`std::env::current_exe()` + `--exact <ce test> --nocapture`), le rôle est
//! choisi par `MW_ROLE`, la synchronisation passe par des lignes sur stdout
//! (« PRET »/« FIN »/« STATS ») et par des fichiers-drapeaux. Même motif que
//! `mw_interprocess_probe.rs` et `verrou_apres_drop.rs`.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, SyncMode, WalReader, WalWriter};

/// Enregistrements par écrivain dans T2 (× 6 processus = 9 000 frames).
const M_T2: u64 = 1500;
/// Payload maximal : le format doit encaisser des tailles VARIABLES (0 → 600 o),
/// sinon une position pourrait être devinée à partir d'un compte de frames.
const PAYLOAD_MAX: usize = 600;

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// Payload de taille variable, distinct par (tag, seq) : l'audit doit pouvoir
/// nommer CHAQUE enregistrement manquant, pas seulement en compter.
fn record(tag: u8, seq: u64, taille: usize) -> WalRecord {
    let mut value = Vec::with_capacity(taille);
    if taille > 0 {
        value.push(tag);
        value.extend(std::iter::repeat_n((seq % 251) as u8, taille - 1));
    }
    WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: seq as u32,
            key: format!("t{tag}s{seq}"),
            value_bincode: value,
        },
    }
}

/// Taille de payload déterministe dans `0..=PAYLOAD_MAX`, distincte par (tag, seq).
fn taille_pour(tag: u8, seq: u64) -> usize {
    ((tag as u64 * 31 + seq * 7) % (PAYLOAD_MAX as u64 + 1)) as usize
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

/// Résultat d'un audit de WAL.
#[derive(Debug)]
struct Audit {
    frames: Vec<(u8, u64)>,
    /// Somme des longueurs de frames lues (doit valoir la taille du fichier
    /// quand la queue est propre).
    octets_frames: u64,
    taille_fichier: u64,
    queue_incomplete: bool,
    trou: Option<(u64, u64)>,
    erreurs: usize,
    /// Offset de fin de la dernière frame VALIDE (bord de frame).
    dernier_bon: u64,
}

fn auditer(wal: &Path) -> Audit {
    let taille_fichier = std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0);
    let r = WalReader::open(wal).unwrap();
    let mut it = r.iter_from(0);
    let mut frames = Vec::new();
    let mut octets_frames = 0u64;
    let mut erreurs = 0usize;
    let mut dernier_bon = 0u64;
    while let Some(item) = it.next() {
        match item {
            Ok((rec, off, len)) => {
                dernier_bon = off + len as u64;
                octets_frames += len as u64;
                frames.push(identite(&rec));
            }
            Err(_) => {
                erreurs += 1;
                break;
            }
        }
    }
    Audit {
        frames,
        octets_frames,
        taille_fichier,
        queue_incomplete: it.queue_incomplete(),
        trou: it.trou(),
        erreurs,
        dernier_bon,
    }
}

// ---------------------------------------------------------------------------
// Harnais inter-processus
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
            debut.elapsed() < Duration::from_secs(30),
            "attente de {quoi} ({}) : délai dépassé",
            p.display()
        );
        std::thread::sleep(Duration::from_micros(200));
    }
}

struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    fn lancer(test: &str, vars: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--exact", test, "--nocapture"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
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

    /// Lit stdout jusqu'à la ligne contenant `attendue`, en accumulant les
    /// lignes `STATS ...` rencontrées (les compteurs de diagnostic de l'enfant).
    fn attendre_et_cumuler(&mut self, attendue: &str, stats: &mut HashMap<String, u64>) {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) => {
                    if let Some(reste) = l.strip_prefix("STATS ") {
                        for champ in reste.split_whitespace() {
                            if let Some((k, v)) = champ.split_once('=') {
                                *stats.entry(k.to_string()).or_insert(0) +=
                                    v.parse::<u64>().unwrap_or(0);
                            }
                        }
                    }
                    if l.contains(attendue) {
                        return;
                    }
                }
                autre => panic!("enfant terminé sans « {attendue} » ({autre:?})"),
            }
        }
    }

    fn attendre(&mut self, attendue: &str) {
        let mut ignore = HashMap::new();
        self.attendre_et_cumuler(attendue, &mut ignore);
    }

    /// Lis TOUT stdout jusqu'à la fin du processus, en accumulant les STATS.
    fn drainer(&mut self, stats: &mut HashMap<String, u64>) {
        while let Some(Ok(l)) = self.lignes.next() {
            if let Some(reste) = l.strip_prefix("STATS ") {
                for champ in reste.split_whitespace() {
                    if let Some((k, v)) = champ.split_once('=') {
                        *stats.entry(k.to_string()).or_insert(0) += v.parse::<u64>().unwrap_or(0);
                    }
                }
            }
        }
        let _ = self.process.wait();
    }

    fn tuer(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Le rôle « écrivain » : ouvre le WAL, attend le top départ, écrit M records
/// (un par append, ou par lots), annonce FIN, puis publie ses compteurs.
fn role_ecrivain() {
    let wal: PathBuf = env_obligatoire("MW_WAL").into();
    let tag: u8 = env_obligatoire("MW_TAG").parse().unwrap();
    let m: u64 = env_obligatoire("MW_M").parse().unwrap();
    let go: PathBuf = env_obligatoire("MW_GO").into();
    let mode = std::env::var("MW_MODE").unwrap_or_else(|_| "unitaire".into());
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).expect("open WAL enfant");
    if std::env::var("MW_COORD").as_deref() == Ok("verrou") {
        w.coordination(obrain_substrate::wal_io::CoordinationAppend::VerrouExact);
    }
    if mode == "course" || mode == "kill" {
        // L'enfant de T3 écrit sans discontinuer (payloads gros) : le parent le
        // tuera au milieu. Il n'attend AUCUN top départ — la fenêtre est serrée
        // par construction.
        w.forcer_surveillance_courses();
        annoncer("PRET");
        let mut seq = 0u64;
        loop {
            w.append(record(tag, seq, 64 * 1024)).expect("append");
            seq += 1;
        }
    }
    annoncer("PRET"); // descripteur OUVERT, aucune écriture encore
    attendre_fichier(&go, "le top départ");
    let debut = Instant::now();
    match mode.as_str() {
        "lot" => {
            let mut seq = 0u64;
            while seq < m {
                let n = core::cmp::min(64, m - seq);
                let lot: Vec<WalRecord> = (0..n)
                    .map(|i| record(tag, seq + i, taille_pour(tag, seq + i)))
                    .collect();
                w.append_batch(lot).expect("append_batch");
                seq += n;
            }
        }
        _ => {
            for seq in 0..m {
                w.append(record(tag, seq, taille_pour(tag, seq)))
                    .expect("append");
            }
        }
    }
    let duree = debut.elapsed();
    annoncer("FIN");
    // STATS lues par le parent : les compteurs de diagnostic de l'enfant.
    annoncer(&format!(
        "STATS recalages={} courses={} records={} micros={}",
        w.recalages(),
        w.courses_suspectees(),
        w.records_ecrits(),
        duree.as_micros()
    ));
}

// ===========================================================================
// T2 — N PROCESSUS, M RECORDS CHACUN : ZÉRO PERTE
// ===========================================================================
//
// AVANT le fix : ~74 % de perte SILENCIEUSE (mesuré, sonde `p1`). Ce test
// ÉCHOUAIT donc sur l'état pré-fix, et il passe après. C'est le check de
// non-régression le plus important de l'axe.
#[test]
fn t2_six_processus_zero_perte_eof_exact() {
    let test = "t2_six_processus_zero_perte_eof_exact";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    let n: u8 = 6;
    let mut enfants = Vec::new();
    for tag in 1..=n {
        let go = td.path().join(format!("go_{tag}"));
        enfants.push((
            tag,
            go.clone(),
            Enfant::lancer(
                test,
                &[
                    ("MW_ROLE", "ecrivain".into()),
                    ("MW_MODE", "unitaire".into()),
                    ("MW_WAL", wal.display().to_string()),
                    ("MW_TAG", tag.to_string()),
                    ("MW_M", M_T2.to_string()),
                    ("MW_GO", go.display().to_string()),
                ],
            ),
        ));
    }
    // TOUS les descripteurs sont ouverts (positionnés à leur propre offset)
    // AVANT la moindre écriture : c'est exactement le décor du défaut mesuré.
    for (_, _, e) in enfants.iter_mut() {
        e.attendre("PRET");
    }

    let debut = Instant::now();
    for (_, go, _) in enfants.iter() {
        std::fs::write(go, b"go").unwrap();
    }
    let mut stats = HashMap::new();
    for (_, _, e) in enfants.iter_mut() {
        e.attendre_et_cumuler("FIN", &mut stats);
        e.drainer(&mut stats);
    }
    let ecoule = debut.elapsed();

    let audit = auditer(&wal);
    let attendus: HashSet<(u8, u64)> = (1..=n)
        .flat_map(|t| (0..M_T2).map(move |s| (t, s)))
        .collect();

    // ── AUDIT 1 : ZÉRO PERTE ────────────────────────────────────────────────
    let vus: HashSet<(u8, u64)> = audit.frames.iter().copied().collect();
    let manquants: Vec<(u8, u64)> = attendus.difference(&vus).copied().collect();
    let en_trop: Vec<(u8, u64)> = vus.difference(&attendus).copied().collect();
    assert!(
        manquants.is_empty(),
        "PERTE SILENCIEUSE : {} enregistrements sur {} n'ont jamais été relus \
         (ex. {:?}) — c'est le défaut mesuré à 74 % avant le fix",
        manquants.len(),
        attendus.len(),
        &manquants[..manquants.len().min(5)]
    );
    assert!(en_trop.is_empty(), "enregistrements fantômes : {en_trop:?}");
    assert_eq!(
        vus.len(),
        audit.frames.len(),
        "aucun enregistrement dupliqué (un écrasement produirait un doublon)"
    );

    // ── AUDIT 2 : EOF == TAILLE EXACTE ──────────────────────────────────────
    assert_eq!(audit.erreurs, 0, "aucune frame illisible");
    assert!(audit.trou.is_none(), "aucun trou");
    assert!(
        !audit.queue_incomplete,
        "queue complète : chaque append a été posé en ENTIER"
    );
    assert_eq!(
        audit.taille_fichier, audit.octets_frames,
        "EOF == somme EXACTE des frames (aucun octet parasite, aucune frame coupée)"
    );

    // ── AUDIT 3 : LE LSN EST UNE BORNE INFÉRIEURE DE LA POSITION ────────────
    //
    // Régime par défaut (`CoordinationAppend::OAppend`) : `O_APPEND` seul. Le LSN
    // est estampillé AVANT l'écriture, et le noyau ne rend que le nombre
    // d'octets — la position n'est donc connaissable qu'à la lecture de la fin,
    // et deux écrivains peuvent lire la MÊME fin avant d'écrire. Le LSN ne peut
    // donc pas être plus grand que la position (`lsn <= ancre + position`), et
    // c'est EXACTEMENT ce qui garantit qu'aucun LSN n'est réémis au checkpoint
    // suivant (l'ancre avance à `ancre + taille`, strictement au-delà de toute
    // borne inférieure). Le compteur de prédictions MESURE l'écart au cas exact.
    let r = WalReader::open(&wal).unwrap();
    let mut n_frames = 0usize;
    let mut derniere_borne = 0u64;
    let mut non_decroissant = true;
    for item in r.iter_from(0) {
        let (rec, off, _) = item.unwrap();
        assert!(rec.lsn >= ANCRE_INITIALE, "LSN au moins égal à l'ancre");
        assert!(
            rec.lsn <= ANCRE_INITIALE + off,
            "le LSN est une BORNE INFÉRIEURE de la position (lsn={}, borne={}): \
             il ne peut pas désigner une position qui n'existait pas encore",
            rec.lsn,
            ANCRE_INITIALE + off
        );
        if rec.lsn < derniere_borne {
            non_decroissant = false;
        }
        derniere_borne = derniere_borne.max(rec.lsn);
        n_frames += 1;
    }
    assert_eq!(n_frames, (n as u64 * M_T2) as usize);

    let total = n as u64 * M_T2;
    // ── LE TAUX VRAI D'INEXACTITUDE, MESURÉ PAR RELECTURE ───────────────────
    // On ne se fie PAS au compteur du writer (borne supérieure) : on relit le
    // WAL et on compte les frames dont le LSN EST la position.
    let r = WalReader::open(&wal).unwrap();
    let exacts = r
        .iter_from(0)
        .filter(|i| {
            let (rec, off, _) = i.as_ref().unwrap();
            rec.lsn == ANCRE_INITIALE + *off
        })
        .count();
    eprintln!(
        "T2 : {n} processus × {M_T2} records = {total} frames · {} o · {:.3} s · \
         {:.0} records/s cumulés · recalages={} · LSN non exacts={} ({:.2} %) · \
         ordre LSN non décroissant={non_decroissant} · compteur (borne sup.)={}",
        audit.taille_fichier,
        ecoule.as_secs_f64(),
        total as f64 / ecoule.as_secs_f64(),
        stats.get("recalages").copied().unwrap_or(0),
        (total as usize) - exacts,
        100.0 * ((total as usize) - exacts) as f64 / total as f64,
        stats.get("courses").copied().unwrap_or(0),
    );
}

// ===========================================================================
// T2b — LE RÉGIME EXACT : `VerrouExact` rend le LSN ÉGAL À LA POSITION
// ===========================================================================
//
// Le verrou d'append inter-processus (`flock` sur le WAL) autour de la paire
// [lire la fin, écrire] supprime la fenêtre : le LSN devient EXACTEMENT la
// position, et le compteur de prédictions tombe à zéro. Le prix est MESURÉ ici
// (et dans le bench de débit) : le verrou coûte plus cher que le `write` qu'il
// protège, et le débit cumulé BAISSE quand N monte.
#[test]
fn t2b_lsn_exact_par_verrou_d_append() {
    let test = "t2b_lsn_exact_par_verrou_d_append";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    let n: u8 = 4;
    let mut enfants = Vec::new();
    for tag in 1..=n {
        let go = td.path().join(format!("go_{tag}"));
        enfants.push((
            go.clone(),
            Enfant::lancer(
                test,
                &[
                    ("MW_ROLE", "ecrivain".into()),
                    ("MW_MODE", "unitaire".into()),
                    ("MW_COORD", "verrou".into()),
                    ("MW_WAL", wal.display().to_string()),
                    ("MW_TAG", tag.to_string()),
                    ("MW_M", M_T2.to_string()),
                    ("MW_GO", go.display().to_string()),
                ],
            ),
        ));
    }
    for (_, e) in enfants.iter_mut() {
        e.attendre("PRET");
    }
    let debut = Instant::now();
    for (go, _) in enfants.iter() {
        std::fs::write(go, b"go").unwrap();
    }
    let mut stats = HashMap::new();
    for (_, e) in enfants.iter_mut() {
        e.attendre_et_cumuler("FIN", &mut stats);
        e.drainer(&mut stats);
    }
    let ecoule = debut.elapsed();

    let audit = auditer(&wal);
    let attendus: HashSet<(u8, u64)> = (1..=n)
        .flat_map(|t| (0..M_T2).map(move |s| (t, s)))
        .collect();
    let vus: HashSet<(u8, u64)> = audit.frames.iter().copied().collect();
    assert_eq!(vus, attendus, "zéro perte, même sous verrou");
    assert_eq!(audit.taille_fichier, audit.octets_frames, "EOF exact");

    // ── LE LSN EST EXACTEMENT LA POSITION, SUR CHAQUE FRAME ────────────────
    let r = WalReader::open(&wal).unwrap();
    let mut dernier = 0u64;
    let mut n_frames = 0usize;
    for item in r.iter_from(0) {
        let (rec, off, _) = item.unwrap();
        assert_eq!(
            rec.lsn,
            ANCRE_INITIALE + off,
            "sous verrou d'append, lsn == ancre + position — SANS exception"
        );
        assert!(rec.lsn > dernier, "et donc strictement croissants");
        dernier = rec.lsn;
        n_frames += 1;
    }
    let total = n as u64 * M_T2;
    assert_eq!(n_frames, total as usize);
    assert_eq!(
        stats.get("courses").copied().unwrap_or(u64::MAX),
        0,
        "le verrou doit rendre le compteur de prédictions NUL — c'est lui qui \
         PROUVE que la fenêtre est fermée"
    );
    eprintln!(
        "T2b (VerrouExact) : {n} processus × {M_T2} records = {total} frames · \
         {:.3} s · {:.0} records/s cumulés · LSN non exacts (compteur)={}",
        ecoule.as_secs_f64(),
        total as f64 / ecoule.as_secs_f64(),
        stats.get("courses").copied().unwrap_or(0),
    );
}

// ===========================================================================
// T7 (inter-processus) — GROUP COMMIT SOUS CONCURRENCE
// ===========================================================================
#[test]
fn t7_deux_processus_en_lots_zero_perte_et_ordre_interne_preserve() {
    let test = "t7_deux_processus_en_lots_zero_perte_et_ordre_interne_preserve";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    let m = 64 * 20; // 20 lots de 64
    let mut enfants = Vec::new();
    for tag in 1..=2u8 {
        let go = td.path().join(format!("go_{tag}"));
        enfants.push((
            tag,
            go.clone(),
            Enfant::lancer(
                test,
                &[
                    ("MW_ROLE", "ecrivain".into()),
                    ("MW_MODE", "lot".into()),
                    ("MW_WAL", wal.display().to_string()),
                    ("MW_TAG", tag.to_string()),
                    ("MW_M", m.to_string()),
                    ("MW_GO", go.display().to_string()),
                ],
            ),
        ));
    }
    for (_, _, e) in enfants.iter_mut() {
        e.attendre("PRET");
    }
    for (_, go, _) in enfants.iter() {
        std::fs::write(go, b"go").unwrap();
    }
    let mut stats = HashMap::new();
    for (_, _, e) in enfants.iter_mut() {
        e.attendre_et_cumuler("FIN", &mut stats);
        e.drainer(&mut stats);
    }

    let audit = auditer(&wal);
    let attendus: HashSet<(u8, u64)> = (1..=2u8)
        .flat_map(|t| (0..m).map(move |s| (t, s)))
        .collect();
    let vus: HashSet<(u8, u64)> = audit.frames.iter().copied().collect();
    assert!(audit.erreurs == 0 && audit.trou.is_none() && !audit.queue_incomplete);
    assert_eq!(
        audit.taille_fichier, audit.octets_frames,
        "EOF == somme exacte"
    );
    assert_eq!(vus.len(), audit.frames.len(), "aucun doublon");
    assert_eq!(vus, attendus, "zéro perte, lot par lot");

    // ── L'ORDRE INTERNE DE CHAQUE LOT EST PRÉSERVÉ ──────────────────────────
    // Un lot est indiscernable, sur le disque, de 64 appends successifs. Deux
    // propriétés, toutes deux nécessaires :
    //   (a) la sous-suite d'un tag est GLOBALEMENT croissante (les lots d'un
    //       même écrivain s'écrivent dans l'ordre) — une seule inversion
    //       suffirait à prouver qu'un lot a été entrelacé avec lui-même ;
    //   (b) au moins (64-1) × 20 paires consécutives par tag : chaque lot est
    //       CONTIGU. C'est un MINORANT : deux lots du même tag peuvent se
    //       suivre sans intercalaire, ce qui ajoute des paires légitimes.
    let mut derniere: HashMap<u8, u64> = HashMap::new();
    let mut paires_contigues = 0usize;
    for (tag, seq) in audit.frames.iter() {
        if let Some(prev) = derniere.get(tag) {
            assert!(
                *seq > *prev,
                "la sous-suite du tag {tag} doit être STRICTEMENT croissante \
                 (vu {seq} après {prev}) : un lot a été entrelacé avec lui-même"
            );
            if *seq == prev + 1 {
                paires_contigues += 1;
            }
        }
        derniere.insert(*tag, *seq);
    }
    let minimum_contigu = (64 - 1) * 20 * 2;
    assert!(
        paires_contigues >= minimum_contigu,
        "chaque lot doit apparaître CONTIGU : {paires_contigues} paires \
         consécutives, minimum attendu {minimum_contigu}"
    );
}

// ===========================================================================
// T3 — kill -9 PENDANT L'APPEND : UN DÉCHIREMENT N'EXISTE QU'À LA QUEUE
// ===========================================================================
//
// Le décor : un enfant écrit sans discontinuer des frames de 64 Kio ; le parent
// le tue en SIGKILL après un délai tiré dans une fenêtre serrée, à plusieurs
// reprises. L'invariant vérifié est CELUI DE LA FIFO : jamais de frame valide
// APRÈS une déchirure (donc jamais « un autre écrivain a suivi une frame
// coupée »), et tout ce qui précède la déchirure est lisible.
#[test]
fn t3_kill_pendant_l_append_le_dechirement_n_existe_qu_a_la_queue() {
    let test = "t3_kill_pendant_l_append_le_dechirement_n_existe_qu_a_la_queue";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    // Une frame = 28 o d'en-tête + payload bincode d'un Vec de 64 Kio + la clé.
    let frame_max = 28 + 64 * 1024 + 64;
    let mut rounds_avec_dechirure = 0usize;
    let rounds = 10usize;
    for round in 0..rounds {
        let tag = (round as u8) + 1;
        let go = td.path().join("jamais_ecrit");
        let mut enfant = Enfant::lancer(
            test,
            &[
                ("MW_ROLE", "ecrivain".into()),
                ("MW_MODE", "kill".into()),
                ("MW_WAL", wal.display().to_string()),
                ("MW_TAG", tag.to_string()),
                ("MW_M", "0".into()),
                ("MW_GO", go.display().to_string()),
            ],
        );
        enfant.attendre("PRET");
        // Fenêtre serrée : quelques dizaines de microsecondes (l'enfant écrit
        // des frames de 64 Kio, donc chaque append dure bien plus longtemps
        // qu'un write de 40 octets — la fenêtre est RÉELLE).
        std::thread::sleep(Duration::from_millis(2 + (round as u64 % 5)));
        enfant.tuer();

        let audit = auditer(&wal);
        // (a) Aucune frame valide APRÈS le bord de la dernière frame lue.
        assert_eq!(
            audit.dernier_bon, audit.octets_frames,
            "les frames lues sont CONTIGUËS depuis l'offset 0 (pas de trou)"
        );
        // (b) La déchirure, si elle existe, est PLUS COURTE qu'une frame : elle
        //     ne peut pas contenir une frame entière suivie d'autre chose.
        let reste = audit.taille_fichier - audit.dernier_bon;
        assert!(
            reste < frame_max,
            "round {round} : {reste} octets après le dernier bord de frame — \
             une déchirure ne peut pas dépasser la frame qu'elle a commencée"
        );
        // (c) JAMAIS d'autre écrivain après une déchirure : un lecteur qui
        //     repart du bord ne trouve AUCUNE frame complète.
        if reste > 0 {
            rounds_avec_dechirure += 1;
            assert!(
                audit.queue_incomplete,
                "round {round} : {reste} octets en trop DOIVENT se dire comme queue \
                 incomplète (frontière de crash), pas passer pour une fin de fichier"
            );
            let brut = std::fs::read(&wal).unwrap();
            let queue = &brut[audit.dernier_bon as usize..];
            assert!(
                obrain_substrate::wal::WalRecord::decode(queue).is_err() || queue.len() < 28,
                "round {round} : la queue déchirée ne doit pas décoder comme une frame"
            );
        } else {
            assert!(
                !audit.queue_incomplete,
                "round {round} : queue exactement au bord ⇒ AUCUNE déchirure à signaler"
            );
        }
        // (d) Zéro perte : tout ce qui a été écrit ENTIER est lisible.
        assert!(
            !audit.frames.is_empty(),
            "round {round} : rien n'a été écrit"
        );

        // ── (e) LA LEÇON DU ROUND PRÉCÉDENT : ON RÉPARE AVANT DE REPARTIR ────
        //
        // DÉFAUT MESURÉ PAR CE TEST (2026-10-06). Si on écrit à la suite d'une
        // queue déchirée, les frames neuves tombent APRÈS la déchirure — et le
        // rejeu s'arrête SUR la déchirure : elles sont donc INVISIBLES, pour
        // toujours, sans que rien ne le dise. Le test l'a vu de la pire façon :
        // au round suivant, 630 784 octets (≈ 10 frames de 64 Kio) séparaient le
        // dernier bord de frame lisible de la fin du fichier.
        //
        // Le remède existe et il est le même que T4 : `reparer_queue()` ramène le
        // fichier à un bord de frame, et les appends suivants sont LUS. C'est
        // cette discipline que le round suivant exerce ici.
        if audit.queue_incomplete {
            let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
            let rep = w
                .reparer_queue()
                .unwrap()
                .expect("la déchirure doit être réparable");
            assert_eq!(rep.apres, audit.dernier_bon);
            assert_eq!(w.offset(), rep.apres);
            drop(w);
        }
    }

    eprintln!(
        "T3 : {rounds} rounds de kill -9 · déchirures observées = \
         {rounds_avec_dechirure} · (un seul `write` sous O_APPEND ⇒ la frame est \
         posée d'un bloc : la déchirure ne peut venir que du noyau, et JAMAIS d'un \
         `write_all` rejoué)"
    );
}

// ===========================================================================
// T4 — RÉPARATION DE QUEUE ENTRE PROCESSUS, PUIS REPRISE PROPRE
// ===========================================================================
//
// Un écrivain meurt ; un AUTRE processus répare la queue et reprend. Les 100
// appends suivants doivent être LUS — ce qui était faux avant : le rejeu
// s'arrête sur la déchirure, et tout ce qui suit était invisible.
#[test]
fn t4_reparation_de_queue_par_un_autre_processus_puis_100_appends() {
    let test = "t4_reparation_de_queue_par_un_autre_processus_puis_100_appends";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    // (1) L'écrivain meurt en plein travail.
    let go = td.path().join("jamais_ecrit");
    let mut enfant = Enfant::lancer(
        test,
        &[
            ("MW_ROLE", "ecrivain".into()),
            ("MW_MODE", "kill".into()),
            ("MW_WAL", wal.display().to_string()),
            ("MW_TAG", "1".into()),
            ("MW_M", "0".into()),
            ("MW_GO", go.display().to_string()),
        ],
    );
    enfant.attendre("PRET");
    std::thread::sleep(Duration::from_millis(3));
    enfant.tuer();

    let avant = auditer(&wal);
    assert!(
        avant.frames.len() >= 1,
        "l'écrivain mort a laissé des frames"
    );
    assert_eq!(avant.octets_frames, avant.dernier_bon);

    // (2) On FABRIQUE la déchirure si le noyau n'en a pas laissé : un `kill -9`
    //     pendant un `write` ne coupe pas la frame (le noyau termine l'écriture),
    //     donc attendre une déchirure naturelle rendrait le test vacillant. La
    //     déchirure simulée est EXACTEMENT ce que laisse un disque plein ou une
    //     coupure d'alimentation : une frame commencée, jamais finie.
    let tronque = avant.dernier_bon < avant.taille_fichier;
    eprintln!(
        "T4 : déchirure laissée par le kill = {tronque} (un `write` unique sous \
         O_APPEND pose la frame d'un bloc : le kill ne la coupe pas)"
    );
    if !tronque {
        // ── UNE DÉCHIRURE, CE N'EST PAS « DES OCTETS QUELCONQUES » ───────────
        // C'est le DÉBUT d'une frame RÉELLE coupée en plein milieu : un en-tête
        // valide qui réclame plus d'octets qu'il n'en existe. Des octets
        // arbitraires (0xAB répété) portent des octets RÉSERVÉS non nuls et sont
        // donc classés « corruption », pas « frontière de crash » — la
        // distinction est exactement celle que le format permet de faire, et le
        // test doit fabriquer le bon cas.
        let entete = record(9, 0, 64 * 1024)
            .encode()
            .expect("encoder une frame réelle");
        let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.set_len(avant.taille_fichier + 40).unwrap();
        drop(f);
        let mut octets = std::fs::read(&wal).unwrap();
        let n = octets.len();
        octets[n - 40..].copy_from_slice(&entete[..40]); // 40 o d'une frame de 64 Kio
        std::fs::write(&wal, &octets).unwrap();
    }
    let dechire = auditer(&wal);
    assert!(
        dechire.queue_incomplete,
        "la queue doit être DÉCHIRÉE (frontière de crash) avant réparation"
    );

    // (3) Le PARENT (processus distinct du mort) répare et reprend.
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    let rep = w
        .reparer_queue()
        .unwrap()
        .expect("une queue à réparer, forcément");
    assert_eq!(rep.avant, dechire.taille_fichier);
    assert_eq!(
        rep.apres, dechire.dernier_bon,
        "retour au dernier bord de frame"
    );
    assert_eq!(w.offset(), rep.apres);

    for i in 0..100u64 {
        w.append(record(2, i, taille_pour(2, i))).unwrap();
    }
    let attendu_fin = w.offset();
    drop(w);

    // (4) Tout est lisible : les frames du mort + les 100 du vivant.
    let apres = auditer(&wal);
    assert_eq!(
        apres.erreurs, 0,
        "aucune erreur de décodage après réparation"
    );
    assert!(!apres.queue_incomplete, "plus aucune déchirure");
    assert!(apres.trou.is_none(), "et aucun trou");
    assert_eq!(
        apres.taille_fichier, apres.octets_frames,
        "EOF == somme exacte"
    );
    assert_eq!(apres.taille_fichier, attendu_fin);
    let du_mort = apres.frames.iter().filter(|(t, _)| *t == 1).count();
    let du_vivant = apres.frames.iter().filter(|(t, _)| *t == 2).count();
    assert_eq!(du_vivant, 100, "les 100 appends suivants sont LUS");
    assert_eq!(
        du_mort,
        dechire.frames.iter().filter(|(t, _)| *t == 1).count(),
        "aucune frame saine du processus mort n'a été perdue par la réparation"
    );
    // Ordre physique == ordre LSN de bout en bout.
    let r = WalReader::open(&wal).unwrap();
    let mut dernier = 0u64;
    for item in r.iter_from(0) {
        let (rec, off, _) = item.unwrap();
        assert_eq!(rec.lsn, ANCRE_INITIALE + off);
        assert!(rec.lsn > dernier);
        dernier = rec.lsn;
    }
}

// ===========================================================================
// DÉBIT À N ÉCRIVAINS — la preuve que la FIFO SCALE
// ===========================================================================
//
// « Ne pas perdre » n'est pas « tenir la charge » : un verrou global ne perdrait
// rien non plus. On mesure donc le débit CUMULÉ de N PROCESSUS qui appendent en
// parallèle, à M constant par processus.
#[test]
fn debit_a_n_ecrivains_la_fifo_scale() {
    let test = "debit_a_n_ecrivains_la_fifo_scale";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    let td = tempfile::tempdir().unwrap();
    let m: u64 = 20_000;
    let mut mesures: Vec<(u8, u64, f64)> = Vec::new();

    for n in [1u8, 2, 4, 8] {
        let wal = td.path().join(format!("w{n}.wal"));
        std::fs::write(&wal, b"").unwrap();
        let mut enfants = Vec::new();
        for tag in 1..=n {
            let go = td.path().join(format!("go_{n}_{tag}"));
            enfants.push((
                go.clone(),
                Enfant::lancer(
                    test,
                    &[
                        ("MW_ROLE", "ecrivain".into()),
                        ("MW_MODE", "unitaire".into()),
                        ("MW_WAL", wal.display().to_string()),
                        ("MW_TAG", tag.to_string()),
                        ("MW_M", m.to_string()),
                        ("MW_GO", go.display().to_string()),
                    ],
                ),
            ));
        }
        for (_, e) in enfants.iter_mut() {
            e.attendre("PRET");
        }
        let debut = Instant::now();
        for (go, _) in enfants.iter() {
            std::fs::write(go, b"go").unwrap();
        }
        let mut stats = HashMap::new();
        for (_, e) in enfants.iter_mut() {
            e.attendre_et_cumuler("FIN", &mut stats);
            e.drainer(&mut stats);
        }
        let ecoule = debut.elapsed().as_secs_f64();

        // Zéro perte à CHAQUE point de mesure (sinon le débit ne veut rien dire).
        let audit = auditer(&wal);
        let total = n as u64 * m;
        assert_eq!(audit.frames.len() as u64, total, "N={n} : zéro perte");
        assert_eq!(
            audit.taille_fichier, audit.octets_frames,
            "N={n} : EOF exact"
        );
        assert!(!audit.queue_incomplete);
        assert_eq!(audit.erreurs, 0);
        mesures.push((n, total, total as f64 / ecoule));
    }

    for (n, total, debit) in &mesures {
        eprintln!("DÉBIT : N={n:2} · {total} records · {debit:>12.0} records/s cumulés");
    }
    let d1 = mesures[0].2;
    let d8 = mesures[3].2;
    eprintln!(
        "DÉBIT : facteur de scale N=1 → N=8 = {:.2}× (le noyau sérialise le write \
         sous O_APPEND : on attend une croissance, pas une multiplication par 8)",
        d8 / d1
    );
    assert!(
        d8 > d1,
        "8 écrivains doivent faire MIEUX qu'un seul (mesuré : {d8:.0} vs {d1:.0} \
         records/s) — sinon l'append ne scale pas du tout"
    );
}
