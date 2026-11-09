//! ── PHASE B — CHECKS INTER-PROCESSUS DES DEUX PIÈCES QUI RESTAIENT
//! MONO-ÉCRIVAIN ─────────────────────────────────────────────────────────────
//!
//! L'axe APPEND (commit 672370aa) a rendu le WAL sain pour N écrivains. Deux
//! choses restaient pourtant mono-écrivain DE FAIT, et ce sont elles qui font
//! qu'un store reste « un seul écrivain » :
//!
//! 1. **CHECKPOINT** — `WalWriter::truncate()` est une paire [avancer l'ancre,
//!    tronquer] qui n'est PAS atomique. Deux checkpointeurs concurrents avancent
//!    chacun l'ancre de SA mesure, et une frame posée dans la fenêtre est
//!    emportée sans que personne ne la réécrive.
//! 2. **ALLOCATION** — `next_node_id` / `next_edge_id` sont des atomiques PAR
//!    PROCESSUS, chargés du dict. Deux écrivains partis du même dict allouent les
//!    mêmes slots, et le second écrase le `NodeRecord` du premier DANS LA ZONE
//!    mmap — CRC valides, aucune trace.
//!
//! ── POURQUOI DES PROCESSUS, ET PAS DES THREADS ─────────────────────────────
//! Le défaut d'allocation ne se voit QU'entre processus pour une raison
//! structurelle : les atomiques sont par processus. Deux threads du MÊME
//! processus se partagent `next_node_id` et n'ont jamais pu se recouvrir. Un test
//! multithread aurait donc été vert AVANT le fix — c'est exactement le piège dans
//! lequel la RFC était tombée (aucun de ses tests n'était inter-processus).
//!
//! ── HARNAIS ────────────────────────────────────────────────────────────────
//! Le binaire de test se RELANCE lui-même en enfant
//! (`std::env::current_exe()` + `--exact <ce test> --nocapture`), le rôle est
//! choisi par `MWB_ROLE`, la synchronisation passe par des lignes sur stdout et
//! par des fichiers-drapeaux. Même motif que `mw_interprocess_append.rs`,
//! `mw_interprocess_probe.rs` et `verrou_apres_drop.rs`.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{
    ANCRE_INITIALE, CoordinationAppend, SyncMode, WalReader, WalWriter,
};

// ---------------------------------------------------------------------------
// Harnais inter-processus
// ---------------------------------------------------------------------------

fn role() -> Option<String> {
    std::env::var("MWB_ROLE").ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

/// Attend l'existence d'un fichier-drapeau (borne : 30 s, comme le harnais de
/// l'axe append).
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

    /// Lit stdout jusqu'à la ligne qui COMMENCE par `prefixe`, et rend le reste
    /// de cette ligne.
    fn attendre_ligne(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                autre => panic!("enfant terminé sans « {prefixe} » ({autre:?})"),
            }
        }
    }

    fn terminer(mut self) {
        let statut = self.process.wait().expect("attendre l'enfant");
        assert!(
            statut.success(),
            "l'enfant a échoué : {statut:?} (voir son stdout ci-dessus)"
        );
    }

    /// `SIGKILL` : c'est le scénario « crash sans flush » — aucun `Drop`,
    /// aucun flush de dictionnaire, aucun msync. Le noyau rend le `flock` du
    /// répertoire au passage, ce qui est le comportement réel d'un `kill -9`.
    fn tuer(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn creer_store(chemin: &Path) {
    let s = SubstrateStore::create(chemin).expect("créer le store");
    // Un nœud pour que la zone Nodes EXISTE et soit dimensionnée (la croissance
    // exponentielle pré-alloue ~1 Mio, soit ~26 000 slots) : les enfants qui
    // suivent n'ont alors AUCUNE croissance de zone à faire, ce qui isole la
    // mesure sur ce qu'on veut prouver (les ids) plutôt que sur le remappage.
    let _ = s.create_node(&["MW"]);
    s.flush().expect("flush initial");
}

// ---------------------------------------------------------------------------
// Enfants
// ---------------------------------------------------------------------------

/// Enfant « écrivain » : alloue `MWB_N` nœuds sur le store partagé et annonce
/// ses ids.
fn enfant_ecrivain(dir: &Path, n: u64) -> ! {
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write écrivain");
    let mut ids = Vec::with_capacity(n as usize);
    for _ in 0..n {
        ids.push(s.create_node(&["MW"]).0);
    }
    let liste = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    annoncer(&format!("IDS {liste}"));
    std::process::exit(0);
}

/// Enfant « rafale » : append EN CONTINU (jusqu'au fichier-drapeau d'arrêt) des
/// trames à clé unique, et écrit dans un fichier la liste EXACTE des indices pour
/// lesquels `append` a rendu `Ok`. C'est le certificat de non-perte : « Ok » est
/// la promesse que le substrat fait à son appelant, et rien ne doit pouvoir
/// l'annuler après coup.
fn enfant_rafale(wal: &Path, sortie: &Path, arret: &Path, drapeau: &Path) -> ! {
    let w = WalWriter::open(wal, SyncMode::Never, ANCRE_INITIALE).expect("ouvrir le WAL");
    w.coordination(CoordinationAppend::OAppend);
    let mut ok: Vec<u64> = Vec::new();
    let _ = std::fs::write(drapeau, b"go");
    let debut = Instant::now();
    let mut i: u64 = 0;
    // Borne de sécurité (30 s) : une rafale ne doit jamais bloquer la suite de
    // tests si le parent meurt avant d'écrire le drapeau d'arrêt.
    while !arret.exists() && debut.elapsed() < Duration::from_secs(30) {
        let rec = WalRecord {
            lsn: 0,
            timestamp: 0,
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::PropSetInline {
                node_id: i as u32,
                key: format!("rafale-{i}"),
                value_bincode: vec![0u8; 8],
            },
        };
        if w.append(rec).is_ok() {
            ok.push(i);
        }
        i += 1;
    }
    let corps = ok.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    std::fs::write(sortie, corps).expect("écrire le certificat");
    annoncer(&format!("OK {}", ok.len()));
    std::process::exit(0);
}

/// Enfant « écrivain qui meurt » : alloue, ANNONCE, puis se met en attente d'un
/// `SIGKILL`. Aucun flush : ni dictionnaire, ni borne, ni msync.
fn enfant_ecrivain_puis_meurt(dir: &Path, n: u64) -> ! {
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write écrivain");
    let mut ids = Vec::with_capacity(n as usize);
    for _ in 0..n {
        ids.push(s.create_node(&["MW"]).0);
    }
    let liste = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    annoncer(&format!("IDS {liste}"));
    annoncer("PRET");
    // Boucle d'attente : le parent va nous SIGKILL.
    loop {
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Utilitaires d'audit
// ---------------------------------------------------------------------------

/// Relit un WAL et rend `(clés trouvées, trou?, erreurs, lsn_min, lsn_max)`.
fn auditer_wal(wal: &Path) -> (Vec<String>, Option<(u64, u64)>, usize, u64, u64) {
    let r = WalReader::open(wal).expect("ouvrir le WAL");
    let mut cles = Vec::new();
    let mut erreurs = 0usize;
    let (mut min, mut max) = (u64::MAX, 0u64);
    let mut it = r.iter_from(0);
    for item in it.by_ref() {
        match item {
            Ok((rec, _, _)) => {
                min = min.min(rec.lsn);
                max = max.max(rec.lsn);
                if let WalPayload::PropSetInline { key, .. } = rec.payload {
                    cles.push(key);
                }
            }
            Err(_) => {
                erreurs += 1;
                break;
            }
        }
    }
    (cles, it.trou(), erreurs, min, max)
}

fn ids_depuis(csv: &str) -> Vec<u64> {
    csv.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("id entier"))
        .collect()
}

// ===========================================================================
// T1 — DEUX PROCESSUS ALLOUENT DES IDS DISTINCTS (pièce 2)
// ===========================================================================
//
// ── CE QUE CE TEST MESURE, ET POURQUOI IL EST INTER-PROCESSUS ───────────────
// Deux processus ouvrent le MÊME store en écriture partagée et allouent chacun
// 150 nœuds. Les deux ensembles d'ids doivent être DISJOINTS. Avant la pièce 2,
// les deux partaient du même dictionnaire : les deux ensembles étaient
// IDENTIQUES (1..=150) et le second écrasait le NodeRecord du premier dans la
// zone mmap — perte silencieuse, CRC valides.
#[test]
fn t1_deux_processus_allouent_des_ids_distincts() {
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWB_DIR"));
        let n: u64 = env_obligatoire("MWB_N").parse().unwrap();
        match r.as_str() {
            "ecrivain" => enfant_ecrivain(&dir, n),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store-t1");
    creer_store(&dir);

    let a = Enfant::lancer(
        "t1_deux_processus_allouent_des_ids_distincts",
        &[
            ("MWB_ROLE", "ecrivain".into()),
            ("MWB_DIR", dir.display().to_string()),
            ("MWB_N", "150".into()),
        ],
    );
    let b = Enfant::lancer(
        "t1_deux_processus_allouent_des_ids_distincts",
        &[
            ("MWB_ROLE", "ecrivain".into()),
            ("MWB_DIR", dir.display().to_string()),
            ("MWB_N", "150".into()),
        ],
    );
    let mut a = a;
    let mut b = b;
    let ids_a = ids_depuis(&a.attendre_ligne("IDS "));
    let ids_b = ids_depuis(&b.attendre_ligne("IDS "));
    a.terminer();
    b.terminer();

    assert_eq!(ids_a.len(), 150);
    assert_eq!(ids_b.len(), 150);

    let mut union: Vec<u64> = ids_a.iter().chain(ids_b.iter()).copied().collect();
    union.sort_unstable();
    let avant = union.len();
    union.dedup();
    assert_eq!(
        union.len(),
        avant,
        "DEUX ÉCRIVAINS ONT REÇU LE MÊME SLOT : {} ids pour {} allocations — \
         l'allocation n'est pas partagée entre processus (ensemble A = {:?}, \
         ensemble B = {:?})",
        union.len(),
        2 * 150,
        &ids_a[..ids_a.len().min(8)],
        &ids_b[..ids_b.len().min(8)]
    );
    assert!(
        !ids_a.contains(&0) && !ids_b.contains(&0),
        "le slot 0 est la sentinelle nulle : il ne doit jamais être alloué"
    );

    // Et les slots alloués sont RÉELLEMENT distincts sur le disque : un nœud
    // vivant à chacun des 300 slots (aucun n'a été écrasé par un pair).
    let s = SubstrateStore::open(&dir).expect("réouverture exclusive");
    let mut vivants = 0usize;
    for id in &union {
        if s.get_node(obrain_common::types::NodeId(*id)).is_some() {
            vivants += 1;
        }
    }
    assert_eq!(
        vivants, 300,
        "les 300 slots alloués doivent porter un nœud VIVANT après réouverture \
         (un slot écrasé par un pair rendrait un nœud manquant)"
    );
}

// ===========================================================================
// T3 — L'ALLOCATION SURVIT À UN `kill -9` (pièce 2)
// ===========================================================================
//
// ── LE SCÉNARIO RÉEL ────────────────────────────────────────────────────────
// Un écrivain P est ouvert DEPUIS LONGTEMPS. Un second écrivain arrive, alloue
// 40 nœuds, et meurt en `SIGKILL` (aucun flush : ni dictionnaire, ni borne).
// P, qui n'a rien vu, alloue à son tour : il ne doit PAS réattribuer un slot du
// mort.
//
// Avant la pièce 2 : P tenait son compteur du dictionnaire chargé à SON
// ouverture, et le scan du WAL ne tourne qu'à l'ouverture — P rendait donc 1,
// c'est-à-dire le slot du nœud n°1 du mort. Après : la borne partagée a été
// avancée par le mort, et P la lit au moment d'allouer — sans relecture du
// disque, sans WAL, sans flush.
#[test]
fn t3_l_allocation_survit_a_un_kill_9() {
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWB_DIR"));
        let n: u64 = env_obligatoire("MWB_N").parse().unwrap();
        match r.as_str() {
            "meurt" => enfant_ecrivain_puis_meurt(&dir, n),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store-t3");
    creer_store(&dir);

    // L'écrivain LONGÉVIF : ouvert AVANT le mort, et qui ne rouvrira pas.
    let p = SubstrateStore::open_shared_write(&dir).expect("écrivain longévif");

    let mut enfant = Enfant::lancer(
        "t3_l_allocation_survit_a_un_kill_9",
        &[
            ("MWB_ROLE", "meurt".into()),
            ("MWB_DIR", dir.display().to_string()),
            ("MWB_N", "40".into()),
        ],
    );
    let ids_morts = ids_depuis(&enfant.attendre_ligne("IDS "));
    enfant.attendre_ligne("PRET");
    enfant.tuer(); // SIGKILL : ni Drop, ni flush, ni msync.

    let max_mort = *ids_morts.iter().max().expect("le mort a alloué");
    assert_eq!(ids_morts.len(), 40);

    // ── L'ÉCRIVAIN LONGÉVIF ALLOUE SANS AVOIR RIEN REVU ────────────────────
    let nouvel_id = p.create_node(&["MW"]).0;
    assert!(
        nouvel_id > max_mort,
        "l'écrivain longévif a réattribué le slot {nouvel_id}, DÉJÀ occupé par un \
         nœud du processus tué (max du mort = {max_mort}) — le filigrane n'a pas \
         survécu au `kill -9`, ou l'allocation n'est pas partagée"
    );

    // Et après réouverture, le filigrane tient toujours : le prochain slot est
    // encore au-delà de tout ce qui a été servi.
    drop(p);
    let s = SubstrateStore::open(&dir).expect("réouverture");
    let apres = s.create_node(&["MW"]).0;
    assert!(
        apres > nouvel_id,
        "après réouverture, le filigrane est reparti en arrière ({apres} ≤ \
         {nouvel_id}) : les slots du processus tué seraient réattribuables"
    );
}

// ===========================================================================
// T4a — LE CHECKPOINT D'UN PAIR NE FAIT PAS RECULER LE FILIGRANE PERSISTÉ
// ===========================================================================
//
// Un pair P (longévif, rien alloué depuis son ouverture) checkpointe pendant
// qu'un second processus a alloué 40 nœuds et est mort. Le checkpoint de P
// PERSISTE le dictionnaire. Avant la pièce 2, P y écrivait SON filigrane
// périmé : la borne du disque RECULAIT, et le prochain `create_node` après
// réouverture réattribuait les slots du mort — l'écrasement silencieux,
// réintroduit par la porte de la persistance.
#[test]
fn t4a_le_checkpoint_d_un_pair_ne_fait_pas_reculer_le_filigrane_persiste() {
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWB_DIR"));
        let n: u64 = env_obligatoire("MWB_N").parse().unwrap();
        match r.as_str() {
            "meurt" => enfant_ecrivain_puis_meurt(&dir, n),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store-t4a");
    creer_store(&dir);

    let p = SubstrateStore::open_shared_write(&dir).expect("écrivain longévif");
    let mut enfant = Enfant::lancer(
        "t4a_le_checkpoint_d_un_pair_ne_fait_pas_reculer_le_filigrane_persiste",
        &[
            ("MWB_ROLE", "meurt".into()),
            ("MWB_DIR", dir.display().to_string()),
            ("MWB_N", "40".into()),
        ],
    );
    let ids_morts = ids_depuis(&enfant.attendre_ligne("IDS "));
    enfant.attendre_ligne("PRET");
    enfant.tuer();
    let max_mort = *ids_morts.iter().max().unwrap();

    // P checkpointe : flush (qui PERSISTE le dictionnaire) + troncature du WAL.
    p.checkpoint().expect("checkpoint du pair longévif");
    drop(p);

    let s = SubstrateStore::open(&dir).expect("réouverture");
    let nouvel_id = s.create_node(&["MW"]).0;
    assert!(
        nouvel_id > max_mort,
        "après le checkpoint d'un pair, le filigrane persisté est reparti en \
         arrière : le nouveau nœud a reçu le slot {nouvel_id}, déjà occupé par un \
         nœud du processus tué (max = {max_mort}) — `persist_dict` a écrasé le \
         dictionnaire au lieu de le FUSIONNER"
    );
}

// ===========================================================================
// T4b — LE FILIGRANE EST RÉPARÉ DEPUIS LES ZONES (pièce 2)
// ===========================================================================
//
// ── POURQUOI CE TEST RETIRE LE DICT, LA BORNE ET LE WAL ─────────────────────
// C'est le SEUL scénario que le relevé des zones couvre et que rien d'autre ne
// couvre, et il est réel : les zones sont la seule copie qui survit quand le
// dictionnaire n'a jamais été écrit (crash avant flush) et que le WAL a été
// tronqué par le checkpoint d'un pair. Le retrait est donc explicite et daté
// plutôt que provoqué par une course — c'est un état de disque, pas une fenêtre.
//
// Avant la pièce 2 : plus de dictionnaire ⇒ `next_node_id = 1` ⇒ le premier
// `create_node` RÉÉCRIT le nœud du slot 1 (perte silencieuse d'un nœud vivant).
// Après : le filigrane est relevé des zones (plus grand slot ALLOUÉ et non
// tombstoné, +1) et le nouveau nœud reçoit un slot libre.
#[test]
fn t4b_le_filigrane_est_repare_des_zones() {
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWB_DIR"));
        let n: u64 = env_obligatoire("MWB_N").parse().unwrap();
        match r.as_str() {
            // Écrit 40 nœuds, msync les zones (comme le fait un checkpoint),
            // puis MEURT sans écrire le dictionnaire ni la borne.
            "zones_seules" => {
                let s = SubstrateStore::open_shared_write(&dir).expect("écrivain partagé");
                let mut ids = Vec::new();
                for _ in 0..n {
                    ids.push(s.create_node(&["MW"]).0);
                }
                // Les OCTETS des zones doivent être à l'abri : c'est la copie que
                // le relevé va lire. Un crash réel les laisserait dans le cache
                // du noyau, donc visibles de tout processus — on le rend explicite.
                s.writer().msync_zones().expect("msync des zones");
                let liste = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
                annoncer(&format!("IDS {liste}"));
                annoncer("PRET");
                loop {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("store-t4b");
    creer_store(&dir);

    let mut enfant = Enfant::lancer(
        "t4b_le_filigrane_est_repare_des_zones",
        &[
            ("MWB_ROLE", "zones_seules".into()),
            ("MWB_DIR", dir.display().to_string()),
            ("MWB_N", "40".into()),
        ],
    );
    let ids = ids_depuis(&enfant.attendre_ligne("IDS "));
    enfant.attendre_ligne("PRET");
    enfant.tuer();
    let max_mort = *ids.iter().max().unwrap();

    // ── L'ÉTAT DE DISQUE « LES ZONES SEULES » ──────────────────────────────
    // (voir le commentaire d'en-tête : ce n'est pas une course, c'est un état.)
    let dict = dir.join("substrate.dict");
    if dict.exists() {
        std::fs::remove_file(&dict).expect("retirer le dictionnaire");
    }
    let borne = dir.join("substrate.alloc");
    if borne.exists() {
        std::fs::remove_file(&borne).expect("retirer la borne partagée");
    }
    let wal = dir.join("substrate.wal");
    if wal.exists() {
        std::fs::remove_file(&wal).expect("retirer le WAL");
    }
    for f in ["substrate.wal.lsn", "substrate.wal.chk"] {
        let p = dir.join(f);
        if p.exists() {
            let _ = std::fs::remove_file(p);
        }
    }

    let s = SubstrateStore::open(&dir).expect("réouverture sur les zones seules");
    let nouvel_id = s.create_node(&["MW"]).0;
    assert!(
        nouvel_id > max_mort,
        "le filigrane n'a PAS été relevé des zones : le nouveau nœud a reçu le \
         slot {nouvel_id}, déjà occupé (max alloué = {max_mort}) — les zones \
         portent pourtant les {max_mort} nœuds vivants"
    );
}

// ===========================================================================
// T6 — DEUX CHECKPOINTS DE DEUX PROCESSUS NE FONT PAS RECULER L'ANCRE
// ===========================================================================
//
// ── LE DÉFAUT MESURÉ ────────────────────────────────────────────────────────
// `WalWriter::truncate()` calculait `nouvelle_ancre = NOTRE_ancre + taille`. Or
// « notre ancre » est celle de CE handle : chargée à l'OUVERTURE et rafraîchie
// seulement quand une anomalie y force. Le second processus d'un store
// multi-écrivains — ouvert depuis un moment, c'est le cas NORMAL — porte donc
// une ancre ANTÉRIEURE au dernier checkpoint d'un pair, et son `truncate`
// republie une valeur PLUS PETITE. L'ancre RECULE, et les LSN du nouveau segment
// recouvrent ceux du segment abandonné : exactement ce que l'ancre existe pour
// empêcher.
//
// ── POURQUOI DEUX HANDLES DANS LE MÊME PROCESSUS SUFFISENT ICI ──────────────
// Ce que le défaut exige, ce n'est pas deux PROCESSUS : c'est deux ÉTATS
// D'ANCRE différents sur le même WAL. Deux `WalWriter` ouverts à deux instants
// distincts les portent exactement (c'est aussi ce que fait `open_shared_write`,
// un handle par processus). Un seul `WalWriter` ne peut PAS reproduire le
// défaut : il rafraîchit son ancre à chaque troncature. Le reste des tests de ce
// fichier est inter-processus parce que le défaut d'ALLOCATION, lui, est
// strictement par processus.
#[test]
fn t6_deux_checkpoints_ne_font_pas_reculer_l_ancre() {
    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");

    // (1) Un premier handle écrit 100 trames.
    let a = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    for _ in 0..100u64 {
        a.append(WalRecord {
            lsn: 0,
            timestamp: 0,
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::NoOp,
        })
        .unwrap();
    }
    // (2) Un SECOND handle est ouvert ICI — AVANT le premier checkpoint. Son ancre
    //     est donc celle du segment naissant, et elle ne bougera plus tant qu'il
    //     n'allouera pas : c'est l'ancre périmée du défaut.
    let b = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();

    // (3) Le premier checkpointe selon la séquence EXACTE de `checkpoint` :
    //     ancre publiée AVANT troncature, marqueur, scellement du plancher.
    let ancre1 = {
        let n = a.truncate().expect("premier checkpoint");
        let at_lsn = a.peek_lsn();
        a.append(WalRecord {
            lsn: 0,
            timestamp: 0,
            flags: WalRecord::FLAG_CHECKPOINT | WalRecord::FLAG_COMMIT,
            payload: WalPayload::Checkpoint { at_lsn },
        })
        .unwrap();
        a.sceller_segment().unwrap();
        n
    };
    assert!(
        ancre1 > ANCRE_INITIALE,
        "le premier checkpoint doit avoir avancé l'ancre (sinon le test ne          mesure rien) : {ancre1}"
    );

    // (4) Le nouveau segment reçoit des trames — sans quoi le second checkpoint
    //     n'aurait qu'un marqueur à abandonner et l'écart serait minuscule.
    for _ in 0..5u64 {
        a.append(WalRecord {
            lsn: 0,
            timestamp: 0,
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::NoOp,
        })
        .unwrap();
    }

    // (5) Le SECOND handle checkpointe — avec son ancre PÉRIMÉE.
    let ancre2 = b.truncate().expect("second checkpoint");

    assert!(
        ancre2 >= ancre1,
        "L'ANCRE A RECULÉ : {ancre1} → {ancre2} après le checkpoint d'un second \
         écrivain. Les LSN du nouveau segment (base {ancre2}) recouvrent donc \
         ceux du segment abandonné (base {ancre1}) — deux enregistrements \
         distincts peuvent porter le même LSN, et `at_lsn` désigne une époque \
         révolue"
    );
    // Et le side-car publié dit la même chose que l'objet vivant : la régression
    // ne doit pas non plus être seulement évitée en mémoire.
    let durable = obrain_substrate::wal_io::lire_ancre_durable(&wal).expect("ancre publiée");
    assert!(
        durable.ancre >= ancre1,
        "l'ancre PUBLIÉE est retombée à {} alors qu'elle valait {ancre1}",
        durable.ancre
    );
}

// ===========================================================================
// T7 — CHECKPOINT CONCURRENT : CE QUI EST OBSERVABLE DE L'EXTÉRIEUR
// ===========================================================================
//
// ── CE QUE CE TEST PROUVE, ET CE QU'IL NE PROUVE PAS (MESURÉ, PAS SUPPOSÉ) ──
// Il n'essaie PAS de détecter la perte d'une frame, et il faut le dire : un
// checkpoint tronque à zéro, donc les frames qu'il emporte forment toujours un
// PRÉFIXE — la perte d'une frame ne crée jamais de TROU dans les indices
// survivants, et un audit extérieur ne peut pas distinguer « emportée par le
// checkpoint » de « emportée par le checkpoint SANS avoir été réécrite ». Cette
// distinction n'est connaissable que par l'écrivain qui a posé la frame (c'est
// tout le mécanisme `taille_apres < fin` → reprise de la pièce 1) : elle est
// prouvée DÉTERMINISTEMENT par `t2_...` (seam), pas ici.
//
// Ce que T7 prouve, et qui est le contrat observable d'un WAL multi-écrivains
// sous checkpoint concurrent :
//   · aucune frame ILLISIBLE (CRC) n'est laissée derrière un checkpoint ;
//   · aucun TROU de zéros n'apparaît ;
//   · l'ensemble des frames survivantes est un SUFFIXE CONTIGU (aucune frame
//     intercalée, aucun trou d'indice) — c'est-à-dire que le WAL reste un
//     journal SÉQUENTIEL, ce qu'aucun des deux défauts d'origine ne garantissait.
//
// ── LA COURSE EST VOULUE ────────────────────────────────────────────────────
// Le parent tronque 80 fois PENDANT que l'enfant append en rafale : c'est la
// configuration réelle d'un store où un processus checkpointe pendant qu'un autre
// écrit.
#[test]
fn t7_checkpoint_concurrent_ne_laisse_ni_trou_ni_frame_illisible() {
    if let Some(r) = role() {
        let wal = PathBuf::from(env_obligatoire("MWB_WAL"));
        let sortie = PathBuf::from(env_obligatoire("MWB_SORTIE"));
        let arret = PathBuf::from(env_obligatoire("MWB_ARRET"));
        let drapeau = PathBuf::from(env_obligatoire("MWB_DRAPEAU"));
        match r.as_str() {
            "rafale" => enfant_rafale(&wal, &sortie, &arret, &drapeau),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    let sortie = td.path().join("certificat-ok");
    let arret = td.path().join("arret");
    let drapeau = td.path().join("rafale-partie");

    let mut enfant = Enfant::lancer(
        "t7_checkpoint_concurrent_ne_laisse_ni_trou_ni_frame_illisible",
        &[
            ("MWB_ROLE", "rafale".into()),
            ("MWB_WAL", wal.display().to_string()),
            ("MWB_SORTIE", sortie.display().to_string()),
            ("MWB_ARRET", arret.display().to_string()),
            ("MWB_DRAPEAU", drapeau.display().to_string()),
        ],
    );
    attendre_fichier(&drapeau, "la rafale");
    // Laisse la rafale remplir le WAL AVANT la première troncature : sans cela,
    // les premières troncatures tomberaient sur un fichier presque vide et le
    // test serait vacuux.
    std::thread::sleep(Duration::from_millis(30));

    // ── LE CHECKPOINTEUR CONCURRENT ────────────────────────────────────────
    // Il fait exactement ce qu'un checkpointeur ÉLU fait au WAL : avancer
    // l'ancre, tronquer, reposer le marqueur, sceller le plancher — 80 fois.
    let pair = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    pair.coordination(CoordinationAppend::OAppend);
    for _ in 0..80 {
        let _ = pair.truncate();
        let at_lsn = pair.peek_lsn();
        let _ = pair.append(WalRecord {
            lsn: 0,
            timestamp: 0,
            flags: WalRecord::FLAG_CHECKPOINT | WalRecord::FLAG_COMMIT,
            payload: WalPayload::Checkpoint { at_lsn },
        });
        let _ = pair.sceller_segment();
        std::thread::sleep(Duration::from_micros(200));
    }
    // La rafale continue APRÈS la dernière troncature : c'est elle qui donne le
    // suffixe contigu auditable.
    std::thread::sleep(Duration::from_millis(40));
    std::fs::write(&arret, b"stop").expect("signaler l'arrêt");

    let n_ok: u64 = enfant.attendre_ligne("OK ").parse().expect("compte Ok");
    enfant.terminer();

    let (cles, trou, erreurs, _, _) = auditer_wal(&wal);
    assert!(trou.is_none(), "TROU de zéros dans le WAL : {trou:?}");
    assert_eq!(erreurs, 0, "frame illisible dans le WAL");

    let certificat = std::fs::read_to_string(&sortie).expect("certificat de la rafale");
    let ok: Vec<u64> = certificat
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("indice"))
        .collect();
    assert_eq!(ok.len() as u64, n_ok);
    assert!(ok.len() > 500, "rafale trop courte : {} appends", ok.len());

    let mut presents: Vec<u64> = cles
        .iter()
        .filter_map(|k| k.strip_prefix("rafale-"))
        .filter_map(|i| i.parse::<u64>().ok())
        .collect();
    presents.sort_unstable();
    assert!(
        presents.len() > 50,
        "trop peu de frames survivantes ({}) : le test ne mesurerait rien",
        presents.len()
    );
    // Le début du suffixe doit être STRICTEMENT au-dessus de 0 : cela prouve que
    // la première troncature a bien emporté un préfixe (le scénario a eu lieu).
    assert!(
        presents[0] > 0,
        "aucune frame n'a été tronquée : le scénario du checkpoint concurrent \
         n'a pas eu lieu, le test ne prouve rien"
    );
    let mut uniques: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let doublons: Vec<u64> = presents
        .iter()
        .copied()
        .filter(|i| !uniques.insert(*i))
        .collect();
    let attendu = presents[presents.len() - 1] - presents[0] + 1;
    let trous = attendu.saturating_sub(uniques.len() as u64);
    // ── AUCUNE FRAME EN DOUBLE ────────────────────────────────────────────
    // Défaut TROUVÉ par ce test à sa première exécution : la détection « frame
    // emportée » (`taille_apres < fin`) recouvre DEUX situations — la frame
    // détruite, et la frame intacte mais posée PLUS BAS que prévu (un pair a
    // tronqué entre notre lecture de la fin et notre `write`). Confondre les
    // deux faisait RÉÉCRIRE une frame qui n'avait jamais disparu, donc une frame
    // en double dans le journal. Ce n'est pas une perte (le rejeu est idempotent)
    // mais ce n'est plus le journal qu'on croit.
    assert!(
        doublons.is_empty(),
        "{} frame(s) écrite(s) DEUX FOIS (ex. {:?}) : la reprise a réécrit une \
         frame qui avait survécu — un journal qui écrit deux fois le même \
         enregistrement n'est plus le journal qu'on croit",
        doublons.len(),
        &doublons[..doublons.len().min(8)]
    );
    assert_eq!(
        trous,
        0,
        "le WAL n'est PLUS un journal SÉQUENTIEL : le suffixe survivant \
         {}..={} porte {trous} trou(s) d'indice ({} appends ont rendu Ok, {} \
         frames survivent) — une frame a été intercalée hors de l'ordre du journal",
        presents[0],
        presents[presents.len() - 1],
        n_ok,
        presents.len()
    );
}
