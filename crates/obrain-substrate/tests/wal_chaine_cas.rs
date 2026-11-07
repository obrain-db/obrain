//! ── CHAÎNE DE RÉSERVATION (CAS) DU REGISTRE D'APPEND — CE QU'ELLE GARANTIT ──
//!
//! Le registre `<wal>.reg` (voir `obrain_substrate::wal_registre`) remplace la
//! MESURE de la fin de fichier (`statx`, 802 ns, deux fois par record en WAL
//! partagé) par une RÉSERVATION atomique : un `compare_exchange` sur
//! `tete_reservee` rend l'offset d'écriture exact, un spin sur `fin_publiee`
//! attend que le prédécesseur ait publié — **zéro appel système sur le chemin
//! chaud comme sur le chemin concurrent**.
//!
//! ── CE QUE CE FICHIER PROUVE, ET POURQUOI IL FAUT DES PROCESSUS ────────────
//! Les quatre propriétés qui font qu'un WAL reste un JOURNAL :
//!
//! 1. **LSN exact par construction** — `lsn == ancre + position` sur CHAQUE frame,
//!    strictement croissant, aucun LSN réémis, sous N processus. L'ancien régime
//!    `OAppend` n'offrait qu'une BORNE INFÉRIEURE (mesuré : 17,7 % de LSN non
//!    exacts sur 6 processus × 1 500 records) ; c'est la propriété que le verrou
//!    d'append achetait autrefois, et qu'il achetait par un verrou chaud.
//! 2. **Tête bloquée** — un écrivain TUÉ entre sa réservation et son `write`
//!    laisse sa plage réservée : les suivants l'attendent (délai de récupération
//!    borné), puis la récupèrent par le chemin froid. AUCUN trou de zéros ne peut
//!    apparaître : `O_APPEND` place toujours à la fin RÉELLE.
//! 3. **Troncature étrangère** — la `generation` du registre est l'annonce d'un
//!    checkpoint (ancre publiée + fichier tronqué) : un pair se ré-ancre SANS
//!    attendre, et sans jamais estampiller un LSN sous la base du nouveau segment.
//! 4. **Composition avec le group commit** — le tampon groupé réserve sa plage par
//!    UN SEUL CAS, pas un par record.
//!
//! ── HARNAIS ────────────────────────────────────────────────────────────────
//! Le binaire de test se RELANCE lui-même en enfant (`std::env::current_exe()` +
//! `--exact <ce test> --nocapture`), le rôle est choisi par `MWC_ROLE`, la
//! synchronisation passe par des lignes sur stdout et par des fichiers-drapeaux —
//! même motif que `mw_interprocess_append.rs` et `mw_interprocess_phase_b.rs`.
//! Il FAUT des processus : deux `WalWriter` du même processus partageraient
//! l'ordonnanceur des threads et le mutex, et ne verraient donc ni la course entre
//! CAS et `write` d'un pair, ni le blocage d'une tête laissée par un processus
//! mort.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_substrate::wal::{WAL_HEADER_SIZE, WalPayload, WalRecord};
use obrain_substrate::wal_io::{
    ANCRE_INITIALE, CoordinationAppend, SyncMode, WalReader, WalWriter, lire_ancre_durable,
};
use obrain_substrate::wal_registre::RegistreAppend;

// ---------------------------------------------------------------------------
// Records et audit
// ---------------------------------------------------------------------------

/// Payload de taille VARIABLE et distinct par (tag, seq) : l'audit doit pouvoir
/// nommer chaque enregistrement manquant, et aucune position ne doit être
/// devinable à partir d'un compte de frames.
fn record(tag: u8, seq: u64) -> WalRecord {
    let taille = 8 + ((tag as usize * 31 + seq as usize * 7) % 200);
    let mut value = Vec::with_capacity(taille);
    value.push(tag);
    value.extend(std::iter::repeat_n((seq % 251) as u8, taille - 1));
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

fn identite(rec: &WalRecord) -> (u8, u64) {
    match &rec.payload {
        WalPayload::PropSetInline { key, .. } => {
            let k = key.strip_prefix('t').expect("clé t<tag>s<seq>");
            let (t, s) = k.split_once('s').expect("clé t<tag>s<seq>");
            (t.parse().unwrap(), s.parse().unwrap())
        }
        // Le marqueur de checkpoint d'un pair n'est pas un record de charge : son
        // identité est son `at_lsn`, et il est taggé 0 (aucun écrivain de ce test
        // n'utilise le tag 0). Il compte comme UNE frame dans les audits de
        // contiguïté et de LSN, ce qui est exactement ce qu'on veut.
        WalPayload::Checkpoint { at_lsn } => (0, *at_lsn),
        autre => panic!("payload inattendu : {autre:?}"),
    }
}

/// Une frame relue : identité, offset, longueur, LSN.
struct Frame {
    identite: (u8, u64),
    offset: u64,
    longueur: u64,
    lsn: u64,
}

/// Relit tout le WAL. Panique sur une frame illisible : ce fichier de test audite
/// des écritures SAINES (les frontières de crash sont le sujet de `t3_kill_*`).
fn auditer(wal: &Path) -> Vec<Frame> {
    let reader = WalReader::open(wal).expect("ouvrir le WAL");
    let mut it = reader.iter_from(0);
    let mut frames = Vec::new();
    while let Some(item) = it.next() {
        match item {
            Ok((rec, off, len)) => frames.push(Frame {
                identite: identite(&rec),
                offset: off,
                longueur: len as u64,
                lsn: rec.lsn,
            }),
            Err(e) => panic!("frame illisible dans le WAL audité : {e}"),
        }
    }
    assert!(
        !it.queue_incomplete(),
        "queue déchirée : chaque append de ce test doit être posé en ENTIER"
    );
    // ── LE TROU EST STRUCTUREL, PAS UNE PLAGE DE ZÉROS QUELCONQUE ───────────
    // Un payload LÉGITIME contient de longues plages de zéros (un `Vec<u8>` de
    // valeurs nulles) : chercher « des zéros dans le fichier » signalerait des
    // frames saines. Le TROU, lui, se reconnaît à sa STRUCTURE — une plage de zéros
    // qui commence là où une frame devrait commencer — et c'est exactement ce que
    // le lecteur publie (`WalIter::trou`), en plus de la contiguïté vérifiée
    // ci-dessous (taille du fichier == somme des longueurs de frames).
    assert!(
        it.trou().is_none(),
        "TROU de zéros dans le WAL : {:?}",
        it.trou()
    );
    frames
}

/// Le contrôle qui fonde tout le chantier : les frames sont CONTIGUËS depuis 0, et
/// chacune porte `lsn == ancre + position`.
fn verifier_contiguite_et_lsn_exact(wal: &Path, ancre: u64) -> Vec<Frame> {
    let frames = auditer(wal);
    let mut fin = 0u64;
    let mut lsn_precedent: Option<u64> = None;
    let mut ecarts: Vec<String> = Vec::new();
    for f in &frames {
        if f.offset != fin {
            ecarts.push(format!(
                "CONTIGUÏTÉ : frame {} à {} au lieu de {fin}",
                fmt_id(f.identite),
                f.offset
            ));
        }
        if f.lsn != ancre + f.offset {
            ecarts.push(format!(
                "LSN : frame {} à {} porte {} au lieu de {}",
                fmt_id(f.identite),
                f.offset,
                f.lsn,
                ancre + f.offset
            ));
        }
        if let Some(prec) = lsn_precedent
            && f.lsn <= prec
        {
            ecarts.push(format!(
                "CROISSANCE : frame {} porte {} après {prec}",
                fmt_id(f.identite),
                f.lsn
            ));
        }
        lsn_precedent = Some(f.lsn);
        fin = f.offset + f.longueur;
    }
    assert!(
        ecarts.is_empty(),
        "{} écart(s) sur {} frames — les 5 premiers : {:#?}",
        ecarts.len(),
        frames.len(),
        &ecarts[..ecarts.len().min(5)]
    );
    assert_eq!(
        std::fs::metadata(wal).unwrap().len(),
        fin,
        "la taille du fichier doit être la somme EXACTE des frames (aucun octet parasite)"
    );
    frames
}

/// Identité lisible d'une frame (messages d'échec).
fn fmt_id(id: (u8, u64)) -> String {
    format!("t{}s{}", id.0, id.1)
}

// ---------------------------------------------------------------------------
// Harnais inter-processus
// ---------------------------------------------------------------------------

fn role() -> Option<String> {
    std::env::var("MWC_ROLE").ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
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

    fn attendre(&mut self, attendue: &str) {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) => {
                    if l.contains(attendue) {
                        return;
                    }
                }
                autre => panic!("enfant terminé sans « {attendue} » ({autre:?})"),
            }
        }
    }

    /// La ligne suivante de l'enfant (les STATS qu'il publie après FIN).
    fn ligne_suivante(&mut self) -> String {
        match self.lignes.next() {
            Some(Ok(l)) => l,
            autre => panic!("enfant sans ligne de STATS ({autre:?})"),
        }
    }

    fn terminer(&mut self) {
        while let Some(Ok(_)) = self.lignes.next() {}
        let _ = self.process.wait();
    }

    fn tuer(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Ouvre le registre d'un WAL en LECTURE (le parent observe la chaîne sans
/// écrire) : `None` si le fichier n'existe pas encore.
///
/// ── SANS RECALER (MP-1, 2026-11-07) ─────────────────────────────────────────
/// `RegistreAppend::ouvrir` est un acte d'ÉCRIVAIN : il peut recaler les têtes sur
/// la taille mesurée. Un observateur qui l'appelait PUBLIAIT donc, à la place
/// d'un enfant entre son `write` et sa publication, la plage de celui-ci — et
/// l'enfant comptait ensuite une course sur un LSN exact. L'observateur ouvre
/// désormais SANS toucher à la chaîne ; la taille rendue reste la mesure prise
/// AVANT l'ouverture.
fn registre_du_wal(wal: &Path) -> Option<(RegistreAppend, u64)> {
    let taille = std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0);
    RegistreAppend::ouvrir_sans_recaler(wal)
        .ok()
        .map(|r| (r, taille))
}

// ---------------------------------------------------------------------------
// Rôles des enfants
// ---------------------------------------------------------------------------

/// Écrivain : ouvre le WAL, annonce PRET, attend le top départ, écrit M records,
/// puis publie ses compteurs de chaîne.
fn role_ecrivain() {
    let wal: PathBuf = env_obligatoire("MWC_WAL").into();
    let tag: u8 = env_obligatoire("MWC_TAG").parse().unwrap();
    let m: u64 = env_obligatoire("MWC_M").parse().unwrap();
    let go: PathBuf = env_obligatoire("MWC_GO").into();
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).expect("open WAL enfant");
    w.coordination(match std::env::var("MWC_COORD").as_deref() {
        Ok("verrou") => CoordinationAppend::VerrouExact,
        _ => CoordinationAppend::OAppend,
    });
    // RÉGIME TESTÉ : aucun écrivain n'est préempté plus de 5 s. Un écrivain vivant
    // mais préempté plus de `DELAI_TETE_BLOQUEE` (25 ms par défaut) est pris pour
    // mort et ré-ancré : LSN inexact, décrit dans docs/LIMITES-v0.0.1.md. Cette
    // limite est mesurée par `t2b`/`t2c` (mesures.yml), pas par ce test. Sans ce
    // réglage, le test échoue sur tout runner chargé, pour une raison documentée.
    w.forcer_delai_tete_bloquee(std::time::Duration::from_secs(5));
    annoncer("PRET");
    attendre_fichier(&go, "le top départ");
    for seq in 0..m {
        w.append(record(tag, seq)).expect("append enfant");
    }
    annoncer("FIN");
    annoncer(&format!(
        "STATS reprises={} attentes={} tetes={} generations={} records={}",
        w.reprises_de_reservation(),
        w.attentes_de_publication(),
        w.tetes_reancrees(),
        w.generations_vues(),
        w.records_ecrits(),
    ));
}

/// Écrivain qui RÉSERVE puis MEURT (rôle de la tête bloquée) : la pause du seam
/// ouvre la fenêtre [CAS, write], et le parent tue le processus dedans.
fn role_reserve_puis_meurt() -> ! {
    let wal: PathBuf = env_obligatoire("MWC_WAL").into();
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).expect("open WAL enfant");
    w.coordination(CoordinationAppend::OAppend);
    // 30 s : le parent n'a pas besoin de se presser, et un `append` ne doit
    // JAMAIS rendre la main avant qu'il ait tué le processus.
    w.forcer_pause_apres_reservation(Duration::from_secs(30));
    annoncer("PRET");
    // La réservation est prise ICI, puis on s'endort SOUS le verrou : la plage est
    // réservée dans le registre et aucun octet n'existe dans le fichier.
    let _ = w.append(record(9, 0));
    annoncer("JAMAIS_ATTEINT");
    std::process::exit(0);
}

/// Checkpointeur d'un pair : tronque le WAL (ce qui annonce `generation`) puis
/// repose son marqueur — exactement la séquence de `checkpoint::checkpoint`.
fn role_tronqueur() -> ! {
    let wal: PathBuf = env_obligatoire("MWC_WAL").into();
    let go: PathBuf = env_obligatoire("MWC_GO").into();
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).expect("open WAL pair");
    w.coordination(CoordinationAppend::OAppend);
    annoncer("PRET");
    attendre_fichier(&go, "le top départ");
    let ancre = w.truncate().expect("troncature du pair");
    let at_lsn = w.peek_lsn();
    w.append(WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_CHECKPOINT | WalRecord::FLAG_COMMIT,
        payload: WalPayload::Checkpoint { at_lsn },
    })
    .expect("marqueur de checkpoint");
    w.sceller_segment().expect("sceller le segment");
    annoncer(&format!("TRONQUE ancre={ancre}"));
    std::process::exit(0);
}

// ===========================================================================
// 1 — N PROCESSUS : LSN EXACT, ORDRE PHYSIQUE == ORDRE DES LSN
// ===========================================================================
//
// C'est LE test du chantier. Il vérifie, sur CHAQUE frame, `lsn == ancre +
// position`, la stricte croissance des LSN, la contiguïté physique et la
// non-réémission — dans les DEUX régimes partagés (`OAppend` par défaut, et
// `VerrouExact`, qui n'est plus qu'un nom : l'exactitude vient de la réservation,
// plus d'un `flock`).
#[test]
fn t1_quatre_processus_lsn_exact_et_ordre_physique() {
    let test = "t1_quatre_processus_lsn_exact_et_ordre_physique";
    if role().as_deref() == Some("ecrivain") {
        role_ecrivain();
        return;
    }

    for coordination in ["oappend", "verrou"] {
        let td = tempfile::tempdir().unwrap();
        let wal = td.path().join("substrate.wal");
        std::fs::write(&wal, b"").unwrap();

        let n: u8 = 4;
        let m: u64 = 1500;
        let mut enfants = Vec::new();
        for tag in 1..=n {
            let go = td.path().join(format!("go_{tag}"));
            enfants.push((
                Tagged(tag),
                go.clone(),
                Enfant::lancer(
                    test,
                    &[
                        ("MWC_ROLE", "ecrivain".into()),
                        ("MWC_WAL", wal.display().to_string()),
                        ("MWC_TAG", tag.to_string()),
                        ("MWC_M", m.to_string()),
                        ("MWC_COORD", coordination.to_string()),
                        ("MWC_GO", go.display().to_string()),
                    ],
                ),
            ));
        }
        for (_, _, e) in enfants.iter_mut() {
            e.attendre("PRET");
        }
        let debut = Instant::now();
        for (_, go, _) in enfants.iter() {
            std::fs::write(go, b"go").unwrap();
        }
        for (tag, _, e) in enfants.iter_mut() {
            e.attendre("FIN");
            let stats = e.ligne_suivante();
            eprintln!("  enfant {tag:?} : {stats}");
            e.terminer();
        }
        let ecoule = debut.elapsed();

        // ── L'AUDIT ──────────────────────────────────────────────────────────
        let frames = verifier_contiguite_et_lsn_exact(&wal, ANCRE_INITIALE);
        assert_eq!(
            frames.len(),
            (n as u64 * m) as usize,
            "aucune frame perdue, aucune dupliquée"
        );
        let attendus: std::collections::HashSet<(u8, u64)> =
            (1..=n).flat_map(|t| (0..m).map(move |s| (t, s))).collect();
        let vus: std::collections::HashSet<(u8, u64)> = frames.iter().map(|f| f.identite).collect();
        assert_eq!(vus, attendus, "zéro perte, zéro fantôme");

        // ── L'ORDRE INTERNE DE CHAQUE ÉCRIVAIN EST PRÉSERVÉ ──────────────────
        // Le journal doit raconter les écritures de chaque processus dans SON
        // ordre : une inversion signalerait qu'une frame a été posée ailleurs que
        // dans la plage réservée.
        let mut derniere: std::collections::HashMap<u8, u64> = std::collections::HashMap::new();
        for f in &frames {
            let (tag, seq) = f.identite;
            if let Some(prev) = derniere.get(&tag) {
                assert!(
                    seq > *prev,
                    "la sous-suite du tag {tag} doit être STRICTEMENT croissante \
                     (vu {seq} après {prev})"
                );
            }
            derniere.insert(tag, seq);
        }
        let total = n as u64 * m;
        eprintln!(
            "T1 ({coordination}) : {n} processus × {m} records = {total} frames · \
             {:.3} s · {:.0} records/s cumulés · LSN exacts = 100 % (0 écart) · \
             aucun LSN réémis · aucun trou de zéros",
            ecoule.as_secs_f64(),
            total as f64 / ecoule.as_secs_f64()
        );
    }
}

/// Petit porteur pour lire le tag dans le journal du test.
struct Tagged(u8);
impl std::fmt::Debug for Tagged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tag{}", self.0)
    }
}

// ===========================================================================
// 2 — UN ÉCRIVAIN TUÉ ENTRE SA RÉSERVATION ET SON `write` BLOQUE LA TÊTE
// ===========================================================================
//
// Le décor : un enfant RÉSERVE une plage (30 s de pause dans la fenêtre), le
// parent le tue en SIGKILL. La plage reste réservée dans `tete_reservee` alors
// qu'aucun octet n'existe — c'est la tête bloquée. Un écrivain suivant :
//   · ATTEND (délai de récupération borné, réglé à 150 ms par le seam) ;
//   · puis se RÉ-ANCRE sur la taille RÉELLE et écrit là — donc sa frame est
//     contiguë, son LSN est exact, et la plage du mort est devenue un trou DANS
//     LES LSN, jamais un trou de zéros dans le fichier.
#[test]
fn t2_ecrivain_tue_apres_reservation_bloque_la_tete_puis_recuperation_froide() {
    let test = "t2_ecrivain_tue_apres_reservation_bloque_la_tete_puis_recuperation_froide";
    if role().as_deref() == Some("reserve_puis_meurt") {
        role_reserve_puis_meurt();
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    // Deux frames saines d'abord : la récupération doit repartir APRÈS elles.
    let temoin = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    temoin.coordination(CoordinationAppend::OAppend);
    for seq in 0..2u64 {
        temoin.append(record(1, seq)).unwrap();
    }
    let taille_saine = temoin.offset();
    assert_eq!(std::fs::metadata(&wal).unwrap().len(), taille_saine);

    let mut enfant = Enfant::lancer(
        test,
        &[
            ("MWC_ROLE", "reserve_puis_meurt".into()),
            ("MWC_WAL", wal.display().to_string()),
        ],
    );
    enfant.attendre("PRET");

    // ── ON ATTEND QUE LA RÉSERVATION SOIT VISIBLE DANS LE REGISTRE ──────────
    // C'est l'observation DIRECTE de l'état « réservé, rien d'écrit » : la tête
    // réservée dépasse la taille réelle, et la fin publiée, elle, ne bouge pas.
    let limite = Instant::now() + Duration::from_secs(10);
    let mut observe = None;
    while Instant::now() < limite {
        if let Some((reg, taille)) = registre_du_wal(&wal)
            && reg
                .tete_reservee()
                .load(std::sync::atomic::Ordering::Acquire)
                > taille
        {
            observe = Some((
                reg.tete_reservee()
                    .load(std::sync::atomic::Ordering::Acquire),
                reg.fin_publiee().load(std::sync::atomic::Ordering::Acquire),
                taille,
            ));
            break;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    let (tete_reservee, fin_publiee, taille) =
        observe.expect("l'enfant devait avoir RÉSERVÉ sa plage (tête > taille réelle)");
    assert_eq!(
        fin_publiee, taille_saine,
        "rien n'a été PUBLIÉ par l'enfant"
    );
    assert!(
        tete_reservee > taille,
        "la tête réservée doit dépasser le fichier — c'est la définition du blocage"
    );
    enfant.tuer();
    let taille_apres_kill = std::fs::metadata(&wal).unwrap().len();
    assert_eq!(
        taille_apres_kill, taille_saine,
        "un écrivain tué entre sa réservation et son write n'a écrit AUCUN octet"
    );

    // ── LE SUIVANT EST RÉCUPÉRÉ PAR LE CHEMIN FROID ─────────────────────────
    let suivant = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    suivant.coordination(CoordinationAppend::OAppend);
    // 150 ms au lieu des 25 ms de production : le chemin exercé est le même, et le
    // test prouve qu'il a ATTENDU (mesure ci-dessous) sans attendre pour rien.
    suivant.forcer_delai_tete_bloquee(Duration::from_millis(150));
    let debut = Instant::now();
    let (pos, len) = suivant
        .append(record(2, 0))
        .expect("append de récupération");
    let attente = debut.elapsed();
    assert_eq!(
        pos, taille_saine,
        "la frame récupérée doit être posée à la fin RÉELLE (aucun trou de zéros \
         fabriqué pour remplir la plage du mort)"
    );
    assert!(
        attente >= Duration::from_millis(140),
        "le délai de récupération doit avoir été PAYÉ (mesuré {attente:?}) — sinon \
         la tête n'était pas bloquée et le test ne prouve rien"
    );
    assert!(
        suivant.tetes_reancrees() >= 1,
        "la récupération doit être COMPTÉE (têtes ré-ancrées = {})",
        suivant.tetes_reancrees()
    );
    assert!(
        taille_apres_kill + len as u64 == std::fs::metadata(&wal).unwrap().len(),
        "le fichier ne doit avoir grandi QUE de la frame récupérée"
    );

    // ── L'AUDIT : CONTIGU, EXACT, SANS ZÉROS ────────────────────────────────
    let frames = verifier_contiguite_et_lsn_exact(&wal, ANCRE_INITIALE);
    assert_eq!(
        frames.iter().map(|f| f.identite).collect::<Vec<_>>(),
        vec![(1, 0), (1, 1), (2, 0)],
        "les deux frames témoins, puis celle de la récupération — la plage du \
         mort n'a laissé AUCUN octet"
    );
    if let Some((reg, _)) = registre_du_wal(&wal) {
        assert_eq!(
            reg.fin_publiee().load(std::sync::atomic::Ordering::Acquire),
            taille_saine + len as u64,
            "la fin publiée doit décrire exactement ce qui existe"
        );
    }
    eprintln!(
        "T2 : tête bloquée à {tete_reservee} (fichier à {taille_saine}) · récupérée en \
         {attente:?} · frame posée à {pos} · aucun zéro, LSN exact, contiguïté intacte"
    );
}

// ===========================================================================
// 3 — TRONCATURE ÉTRANGÈRE : LA GÉNÉRATION ANNONCE, LE PAIR SE RÉ-ANCRE
// ===========================================================================
//
// Un checkpointeur d'un AUTRE processus avance l'ancre et tronque le WAL. Un
// écrivain qui a encore son descripteur ouvert doit :
//   · le SAVOIR sans `statx` (la `generation` du registre est l'annonce) ;
//   · ne PAS attendre le délai de récupération (un pair qui a tronqué ne va pas
//     finir d'écrire une frame que le fichier ne contient plus) ;
//   · estampiller ses frames suivantes AVEC LA NOUVELLE ANCRE — un LSN sous la
//     base du segment courant serait un LSN déjà émis.
#[test]
fn t3_troncature_etrangere_annoncee_le_pair_se_reancre_sans_attendre() {
    let test = "t3_troncature_etrangere_annoncee_le_pair_se_reancre_sans_attendre";
    if role().as_deref() == Some("tronqueur") {
        role_tronqueur();
    }

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    let ecrivain = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    ecrivain.coordination(CoordinationAppend::OAppend);
    for seq in 0..8u64 {
        ecrivain.append(record(1, seq)).unwrap();
    }
    let ancre_avant = ecrivain.ancre();
    let frames_avant = ecrivain.records_ecrits();

    let go = td.path().join("go");
    let mut pair = Enfant::lancer(
        test,
        &[
            ("MWC_ROLE", "tronqueur".into()),
            ("MWC_WAL", wal.display().to_string()),
            ("MWC_GO", go.display().to_string()),
        ],
    );
    pair.attendre("PRET");
    let debut = Instant::now();
    std::fs::write(&go, b"go").unwrap();
    pair.attendre("TRONQUE");
    pair.terminer();
    let duree_pair = debut.elapsed();

    // ── LE PAIR A BIEN TRONQUÉ, ET L'ANCRE A AVANCÉ ─────────────────────────
    let ancre_apres = lire_ancre_durable(&wal)
        .expect("l'ancre durable doit être publiée")
        .ancre;
    assert!(
        ancre_apres > ancre_avant,
        "la troncature du pair doit avoir AVANCÉ l'ancre ({ancre_avant} → {ancre_apres})"
    );
    let taille_apres_pair = std::fs::metadata(&wal).unwrap().len();
    assert!(
        taille_apres_pair < frames_avant * 200,
        "le fichier doit avoir été tronqué (taille {taille_apres_pair})"
    );

    // ── L'ÉCRIVAIN D'À CÔTÉ ÉCRIT : LA GÉNÉRATION DOIT L'AVOIR PRÉVENU ──────
    // Le délai de récupération est ÉCARTÉ (5 s) : si l'append rend la main vite,
    // c'est bien l'annonce de génération qui a dispensé d'attendre — et non un
    // hasard de timing.
    ecrivain.forcer_delai_tete_bloquee(Duration::from_secs(5));
    let debut = Instant::now();
    let (pos, len) = ecrivain
        .append(record(2, 0))
        .expect("append après troncature");
    let duree = debut.elapsed();
    assert!(
        duree < Duration::from_secs(1),
        "l'append doit se ré-ancrer SANS attendre le délai de récupération \
         (mesuré {duree:?}) — c'est l'annonce de génération qui le dispense"
    );
    assert_eq!(
        pos, taille_apres_pair,
        "la frame doit suivre le marqueur du pair, à la fin RÉELLE"
    );
    assert!(
        ecrivain.generations_vues() >= 1,
        "l'écrivain doit avoir VU la troncature du pair par la génération \
         (compteur = {})",
        ecrivain.generations_vues()
    );

    // ── AUDIT : AUCUN LSN SOUS LA BASE DU NOUVEAU SEGMENT ───────────────────
    let frames = verifier_contiguite_et_lsn_exact(&wal, ancre_apres);
    assert!(
        frames.iter().all(|f| f.lsn >= ancre_apres),
        "aucune frame ne peut porter un LSN antérieur à la base du segment courant"
    );
    let dernier = frames.last().unwrap();
    assert_eq!(
        (dernier.identite, dernier.offset, dernier.longueur),
        ((2u8, 0u64), pos, len as u64),
        "la frame de l'écrivain est la dernière, à l'offset qu'il a rendu"
    );
    eprintln!(
        "T3 : pair tronqué en {duree_pair:?} (ancre {ancre_avant} → {ancre_apres}) · \
         pair écrasé par l'écrivain : ré-ancrage en {duree:?} SANS attente · {} frames \
         survivantes, LSN exacts depuis la nouvelle base",
        frames.len()
    );
}

// ===========================================================================
// 4 — LE CHEMIN CHAUD NE PAIE AUCUNE MESURE (un seul écrivain partagé)
// ===========================================================================
//
// C'est la mesure du chantier, faite SANS `strace` : un écrivain SEUL en
// coordination partagée (`OAppend`, le régime du banc) doit écrire N records sans
// jamais demander la taille du fichier au noyau, sans jamais attendre un pair, et
// sans jamais récupérer de tête. Trois compteurs à zéro le prouvent, et l'audit
// prouve que l'exactitude ne vient pas d'un relâchement.
#[test]
fn t4_le_chemin_chaud_ne_paie_aucune_mesure() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    w.coordination(CoordinationAppend::OAppend);
    // Le registre doit être EN SERVICE : sans lui, le test ne mesurerait que le
    // chemin de repli (deux `statx`), qui est justement celui d'avant le chantier.
    assert!(
        w.registre_en_service() || {
            w.append(record(1, 0)).unwrap();
            w.registre_en_service()
        },
        "le registre d'append doit être créé au premier append partagé"
    );

    let n = 5_000u64;
    for seq in 0..n {
        w.append(record(1, seq)).unwrap();
    }

    assert_eq!(
        w.attentes_de_publication(),
        0,
        "un écrivain SEUL ne doit JAMAIS attendre de publication (spin sur \
         `fin_publiee` : la chaîne est quiescente à chaque fois)"
    );
    assert_eq!(
        w.tetes_reancrees(),
        0,
        "aucune tête ne doit être ré-ancrée : rien n'est bloqué, rien n'est hors chaîne"
    );
    assert_eq!(
        w.reprises_de_reservation(),
        0,
        "aucun CAS ne doit échouer : personne d'autre ne réserve"
    );
    assert_eq!(
        w.courses_suspectees(),
        0,
        "aucun LSN ne doit être suspect : l'exactitude est structurelle"
    );
    assert_eq!(
        w.offset(),
        std::fs::metadata(&wal).unwrap().len(),
        "`offset()` reste bit-identique à la taille du fichier (contrat T6)"
    );
    verifier_contiguite_et_lsn_exact(&wal, ANCRE_INITIALE);
    eprintln!(
        "T4 : {n} appends en coordination PARTAGÉE · 0 attente, 0 ré-ancrage, \
         0 reprise de CAS, 0 LSN suspect — soit 1 CAS + 1 `write` par record, \
         sans aucune mesure de taille"
    );
}

// ===========================================================================
// 5 — COMPOSITION AVEC LE GROUP COMMIT : LE TAMPON RÉSERVE SA PLAGE PAR UN CAS
// ===========================================================================
//
// Le group commit accumule des records dans un tampon et le pose d'un seul
// `write`. La chaîne doit se composer avec lui SANS dégénérer : la réservation
// porte sur la PLAGE ENTIÈRE du tampon (un CAS), pas sur chaque record — sinon
// l'apport du group commit serait annulé par un `fetch_add`/CAS par record.
#[test]
fn t5_le_tampon_groupe_reserve_sa_plage_par_un_seul_cas() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("substrate.wal");
    let w = std::sync::Arc::new(WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap());
    w.coordination(CoordinationAppend::OAppend);
    // La needle de TEMPS est écartée : les poses observées ne peuvent venir que
    // des seuils de records/octets, donc leur compte est exact et reproductible.
    w.forcer_seuil_temps(Duration::from_secs(3600));
    w.activer_group_commit();

    let n = obrain_substrate::wal_io::SEUIL_RECORDS as u64;
    for seq in 0..n {
        w.append(record(1, seq)).unwrap();
    }
    assert_eq!(w.poses(), 1, "4096 records = UN `write` (la needle)");
    assert_eq!(
        w.attentes_de_publication(),
        0,
        "écrivain seul : la chaîne est quiescente, la pose ne paie aucune attente"
    );

    // Un PAIR écrit maintenant : la prochaine pose de tampon trouvera la chaîne
    // avancée (et devra donc réserver APRÈS lui, puis attendre qu'il publie).
    let pair = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    pair.coordination(CoordinationAppend::OAppend);
    let taille_avant_pair = std::fs::metadata(&wal).unwrap().len();
    let (pos_pair, len_pair) = pair.append(record(2, 0)).unwrap();
    assert_eq!(
        pos_pair, taille_avant_pair,
        "le pair écrit à la fin réelle laissée par le tampon"
    );
    assert_eq!(
        std::fs::metadata(&wal).unwrap().len(),
        taille_avant_pair + len_pair as u64
    );

    let attentes_avant = w.attentes_de_publication();
    for seq in 0..n {
        w.append(record(3, seq)).unwrap();
    }
    let attentes_apres = w.attentes_de_publication();
    assert_eq!(w.poses(), 2, "deux poses pour deux tampons pleins");
    // ── LA MESURE QUI COMPTE : PAR PLAGE, JAMAIS PAR RECORD ─────────────────
    // 8 192 records ont été écrits en DEUX plages réservées. Tout ce que la chaîne
    // a pu coûter — attentes de publication et reprises de CAS — doit donc se
    // compter en unités, pas en milliers : c'est exactement ce qui distingue la
    // composition (une réservation par tampon) d'une régression (une par record).
    assert!(
        attentes_apres + w.reprises_de_reservation() <= 8,
        "la chaîne est payée par PLAGE, pas par record : {} attente(s) + {} reprise(s) \
         de CAS pour {} records",
        attentes_apres,
        w.reprises_de_reservation(),
        2 * n
    );
    assert!(len_pair > 0);

    // ── AUDIT : LE TAMPON EST INDISCERNABLE DE N APPENDS ────────────────────
    let reprises = w.reprises_de_reservation();
    let attentes = w.attentes_de_publication();
    drop(pair);
    drop(w);
    let frames = verifier_contiguite_et_lsn_exact(&wal, ANCRE_INITIALE);
    assert_eq!(frames.len(), (2 * n + 1) as usize);
    eprintln!(
        "T5 : group commit composé avec la chaîne · 2 poses de {n} records · \
         {} attente(s) de publication et {} reprise(s) de CAS pour {} records — \
         soit par PLAGE, jamais par record · frames contiguës et LSN exacts ({attentes} attentes au total)",
        attentes_apres - attentes_avant,
        reprises,
        2 * n
    );
}

// ===========================================================================
// 6 — REPLI SANS REGISTRE : LE CHEMIN D'AVANT, À LA LETTRE
// ===========================================================================
//
// Le registre est une optimisation de COÛT, jamais une condition de correction :
// quand il ne peut pas exister (`/dev/full`, quota disque, répertoire en lecture
// seule, wasm32), l'append doit reprendre le chemin d'AVANT — mesure `statx` sous
// le verrou, vérification après — et rendre EXACTEMENT les mêmes erreurs.
#[test]
fn t6_sans_registre_le_chemin_d_avant_est_repris() {
    if !Path::new("/dev/full").exists() {
        eprintln!("/dev/full absent (non-Linux ?) — check de repli NON exécuté");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ancre = dir.path().join("w.wal.lsn");
    let w = WalWriter::open_avec_ancre("/dev/full", &ancre, SyncMode::Never, ANCRE_INITIALE)
        .expect("ouvrir /dev/full");
    w.coordination(CoordinationAppend::OAppend);

    let e = w.append(record(1, 0)).unwrap_err();
    match &e {
        obrain_substrate::SubstrateError::Io(io) => assert!(
            matches!(
                io.kind(),
                std::io::ErrorKind::StorageFull | std::io::ErrorKind::WriteZero
            ),
            "0 octet écrit doit se dire comme tel, obtenu {:?}",
            io.kind()
        ),
        autre => panic!("attendu Io(StorageFull|WriteZero), obtenu {autre:?}"),
    }
    assert!(
        !w.registre_en_service(),
        "le registre ne peut pas exister sur `/dev/full` : le repli doit être PRIS, \
         et le dire (`registre_en_service() == false`)"
    );
    assert!(
        w.queue_a_repairer().is_none(),
        "0 octet = queue INTACTE : aucune réparation n'est due"
    );
    eprintln!(
        "T6 : sans registre, l'append reprend le chemin d'avant (mesure sous verrou) \
         et rend la MÊME erreur — le registre est une optimisation, pas une condition"
    );
}

// ===========================================================================
// T8 — LA CRÉATION (OU L'OUVERTURE) DU REGISTRE NE DEVANCE PAS UNE PUBLICATION
// ===========================================================================
//
// ── LE DÉFAUT (MP-1, 2026-11-07) ───────────────────────────────────────────
// `t2b` (`mw_interprocess_append`) échouait 2 fois sur 40 sous charge sur son
// assertion `courses == 0`, avec TOUS les LSN exacts : le compteur montait donc
// dans un cas où la position était juste. Cause déduite par lecture, et que ce
// test rend DÉTERMINISTE :
//
// 1. A réserve [0, l_A), écrit (le fichier fait l_A), et est préempté AVANT de
//    publier `fin_publiee = l_A` (ici : le seam `forcer_pause_apres_ecriture`) ;
// 2. B ouvre le registre (`RegistreAppend::ouvrir`, ce que fait aussi son
//    premier append, paresseusement) avec la taille MESURÉE `l_A` : l'ouverture
//    faisait `fin_publiee.fetch_max(l_A)` INCONDITIONNELLEMENT, et PUBLIAIT donc
//    la plage de A à sa place ;
// 3. B réserve [l_A, …), voit `fin_publiee == l_A`, écrit (à la bonne place :
//    les octets de A existent bien), publie ;
// 4. A reprend : `fin_publiee != 0` → « un pair a publié avant nous » →
//    `courses_suspectees += 1`, alors que sa frame est EXACTEMENT à 0.
//
// Le LSN est juste dans les deux cas : c'est le COMPTEUR qui mentait, et avec lui
// `surveiller_les_courses` (un `statx` par append jusqu'à la fin de vie de A).
//
// ── CE QUE LE TEST EXIGE ──────────────────────────────────────────────────
// Deux écrivains du MÊME processus suffisent ici (contrairement aux autres tests
// de ce fichier) : la fenêtre est ouverte par le seam, pas par l'ordonnanceur,
// et l'ouverture du registre est faite explicitement par le test — comme la fait
// `registre_du_wal`, qui prétend « lire » le registre.
#[test]
fn la_creation_du_registre_ne_devance_pas_une_publication() {
    use std::sync::Arc;

    // Pause de A entre son `write` et sa publication. Elle doit durer bien plus
    // longtemps que le temps de lancement du thread de A sur un runner chargé :
    // les vérifications ci-dessous sont déclenchées par la CONDITION (A a écrit),
    // jamais par un délai fixe. Un `sleep(50 ms)` supposait que A avait déjà écrit,
    // ce qui est faux sur le runner partagé (échec CI du 07/11/2026 : « A doit avoir
    // écrit avant sa pause », taille == 0).
    const PAUSE_A_MS: u64 = 3_000;

    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    std::fs::write(&wal, b"").unwrap();

    // A : une pause de 300 ms entre son `write` et sa publication.
    let a = Arc::new(WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).expect("open A"));
    a.coordination(CoordinationAppend::OAppend);
    a.forcer_pause_apres_ecriture(Duration::from_millis(PAUSE_A_MS));
    let a_fil = Arc::clone(&a);
    let fil_a = std::thread::spawn(move || a_fil.append(record(1, 0)).expect("append A"));

    // On attend que A ait ÉCRIT sa frame (taille > 0) ; il ne l'a pas encore publiée
    // (sa pause court). Attente bornée à 10 s, sondée toutes les 2 ms.
    let debut_attente = std::time::Instant::now();
    let taille = loop {
        let t = std::fs::metadata(&wal).unwrap().len();
        if t > 0 {
            break t;
        }
        assert!(
            debut_attente.elapsed() < Duration::from_secs(10),
            "A n'a pas écrit sa frame en 10 s"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(
        taille > 0,
        "A doit avoir écrit avant sa pause (seam mal placé ?)"
    );
    let (reg, _) = registre_du_wal(&wal).expect("A a créé le registre à son premier append");
    assert_eq!(
        reg.fin_publiee().load(std::sync::atomic::Ordering::Acquire),
        0,
        "A n'a pas encore publié : ouvrir le registre ne doit PAS publier à sa place"
    );
    assert_eq!(
        reg.tete_reservee()
            .load(std::sync::atomic::Ordering::Acquire),
        taille,
        "la réservation de A est intacte"
    );

    // B : un second écrivain, dont le premier append ouvre (paresseusement) le
    // registre avec la taille mesurée, puis réserve au-dessus de A et attend sa
    // publication (A est VIVANT : la preuve de vie doit faire attendre, pas
    // ré-ancrer).
    let b = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).expect("open B");
    b.coordination(CoordinationAppend::OAppend);
    let off_b = b.append(record(2, 0)).expect("append B");
    let off_a = fil_a.join().expect("fil A");

    assert_eq!(off_a.0, 0, "A est à 0");
    assert_eq!(off_b.0, taille, "B est juste après A");
    assert_eq!(
        a.courses_suspectees(),
        0,
        "A : sa frame est à sa position, le compteur de LSN suspects doit rester NUL"
    );
    assert_eq!(b.courses_suspectees(), 0, "B : idem");
    assert_eq!(
        a.tetes_reancrees() + b.tetes_reancrees(),
        0,
        "aucune chaîne ré-ancrée"
    );

    let frames = verifier_contiguite_et_lsn_exact(&wal, ANCRE_INITIALE);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].identite, (1, 0));
    assert_eq!(frames[1].identite, (2, 0));
    eprintln!(
        "T8 : A (pause {} ms après write) puis B (ouverture du registre + append) : \
         courses A={} B={}, devancées A={} B={}, LSN exacts et contigus",
        PAUSE_A_MS,
        a.courses_suspectees(),
        b.courses_suspectees(),
        a.publications_devancees(),
        b.publications_devancees(),
    );
}

/// Garde-fou : le format de frame n'a pas bougé (constante utilisée par l'audit).
#[test]
fn t0_la_taille_d_entete_est_inchangee() {
    assert_eq!(WAL_HEADER_SIZE, 28);
}
