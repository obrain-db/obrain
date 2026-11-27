//! FORK INTERROMPU AUX FRONTIÈRES DU JOURNAL (#334/#418) — `kill -9` RÉEL aux
//! points d'interruption déterministes de `domaine_id::forker` (feature
//! `points-de-crash`, jamais en production), puis RÉCUPÉRATION dans un NOUVEAU
//! processus.
//!
//! Les points sont nommés par leur POSITION dans la réécriture d'identité :
//!   (a) `avant_journal`          — avant la publication du journal ;
//!   (b) `apres_journal`          — après le journal, avant `domaine.id` ;
//!   (c) `entre_id_et_init`       — entre `domaine.id` et `domaine.init` ;
//!   (d) `avant_retrait_journal`  — après `domaine.init`, avant le retrait ;
//!   (e) récupération             — un NOUVEAU processus rouvre, complète le
//!                                   fork, puis un SECOND fork réussit.
//!
//! DIFFÉRENCE ASSUMÉE, dite explicitement : interruption de PROCESSUS (SIGKILL)
//! ≠ perte ÉLECTRIQUE (non simulée — pas de corruption de pages ; les écritures
//! passent par tmp+rename+fsync, la visibilité inter-processus vient du cache de
//! pages partagé).

#![cfg(feature = "points-de-crash")]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_substrate::{SubstrateStore, FICHIER_DOMAINE_FORK, FICHIER_DOMAINE_ID, FICHIER_DOMAINE_INIT};

const ENV_DIR: &str = "OBRAIN_FORK_INTERRUPT_DIR";
/// Lu par `obrain-substrate` (feature `points-de-crash`) : `<nom>:<marqueur>`.
const ENV_POINT: &str = "OBRAIN_SUBSTRATE_POINT_DE_CRASH";
const TEST_ENFANT: &str = "enfant_fork_interruption";

/// Lit le champ u128 (big-endian, octets 6..22) d'un fichier d'identité de
/// domaine (`domaine.id`, `domaine.init` ou `domaine.fork`), tous au format
/// 22 octets = entête 6 + u128 16. `None` si absent ou de longueur inattendue.
fn lire_u128(p: &Path) -> Option<u128> {
    let b = std::fs::read(p).ok()?;
    if b.len() != 22 {
        return None;
    }
    Some(u128::from_be_bytes(b[6..22].try_into().expect("16 octets")))
}

/// État sur disque de l'identité de domaine, lu SANS résolution (fichiers bruts).
#[derive(Debug)]
struct EtatDisque {
    id: Option<u128>,
    init: Option<u128>,
    journal: Option<u128>,
}

fn etat_disque(base: &Path) -> EtatDisque {
    EtatDisque {
        id: lire_u128(&base.join(FICHIER_DOMAINE_ID)),
        init: lire_u128(&base.join(FICHIER_DOMAINE_INIT)),
        journal: lire_u128(&base.join(FICHIER_DOMAINE_FORK)),
    }
}

fn tuer_9(child: &mut Child) {
    let pid = child.id();
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill -9");
    assert!(status.success(), "kill -9 a échoué");
    let st = child.wait().expect("wait enfant");
    assert_eq!(st.signal(), Some(9), "l'enfant doit MOURIR par SIGKILL (9)");
}

fn attendre_marqueur(marqueur: &Path, child: &mut Child) {
    let debut = Instant::now();
    loop {
        if marqueur.exists() {
            return;
        }
        if let Some(st) = child.try_wait().expect("try_wait") {
            panic!("enfant terminé ({st}) avant le marqueur — le point n'a pas tiré");
        }
        assert!(
            debut.elapsed() < Duration::from_secs(60),
            "marqueur non apparu en 60 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// ENFANT — ouvre la base en écriture puis déclenche un fork : le point de crash
/// armé (variable d'environnement) tire au milieu du fork, pose le marqueur et
/// dort jusqu'au `kill -9`. No-op sans `ENV_DIR`.
#[test]
fn enfant_fork_interruption() {
    let Some(dir) = std::env::var_os(ENV_DIR) else {
        return;
    };
    let base = PathBuf::from(dir).join("kb");
    let store = SubstrateStore::open(&base).expect("ouvrir la base");
    // La consigne de crash est déjà dans l'environnement (posée par le parent).
    // `forker_domaine` atteint le point armé, pose le marqueur, dort.
    let _ = store.forker_domaine();
    // Inatteignable si le point a tiré.
    store.close().unwrap();
    std::process::exit(0);
}

/// Crée la base (domaine `d0`), lance l'enfant sur `point`, le tue -9 au point,
/// relit l'état disque brut, rouvre (récupération) et rend l'état final.
fn derouler(point: &str) -> (tempfile::TempDir, u128, EtatDisque, u128, EtatDisque) {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let d0 = {
        let store = SubstrateStore::create(&base).unwrap();
        let d0 = store.domaine_id();
        store.close().unwrap();
        d0
    };
    let marqueur = td.path().join("marqueur");
    let exe = std::env::current_exe().expect("current_exe");
    let consigne = format!("{point}:{}", marqueur.display());
    let mut child = Command::new(&exe)
        .args([TEST_ENFANT, "--exact", "--test-threads=1", "--nocapture"])
        .env(ENV_DIR, td.path())
        .env(ENV_POINT, consigne)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("lancer l'enfant");
    attendre_marqueur(&marqueur, &mut child);
    tuer_9(&mut child);
    let avant = etat_disque(&base);
    let recupere = {
        let store = SubstrateStore::open(&base).expect("réouverture (récupération)");
        let d = store.domaine_id();
        store.close().unwrap();
        d
    };
    let apres = etat_disque(&base);
    (td, d0, avant, recupere, apres)
}

/// (a) Avant la publication du journal : le fork n'a AUCUNE trace persistée.
/// La réouverture rend l'identité INITIALE — le fork est perdu, sans corruption.
#[test]
fn a_fork_interrompu_avant_journal_rend_l_identite_initiale() {
    let (_td, d0, avant, recupere, apres) = derouler("domaine:fork:avant_journal");
    assert_eq!(avant.journal, None, "aucun journal avant sa publication");
    assert_eq!(avant.id, Some(d0), "domaine.id intact");
    assert_eq!(avant.init, Some(d0), "domaine.init intact (empreinte == identité)");
    assert_eq!(recupere, d0, "fork jamais déclaré : identité initiale conservée");
    assert_eq!(apres.journal, None);
    assert_eq!(apres.id, Some(d0));
}

/// (b) Après le journal, avant `domaine.id` : le journal rend le fork
/// RÉCUPÉRABLE — la réouverture complète le fork vers la NOUVELLE identité.
#[test]
fn b_fork_interrompu_apres_journal_est_recupere() {
    let (_td, d0, avant, recupere, apres) = derouler("domaine:fork:apres_journal");
    let d1 = avant.journal.expect("le journal est posé");
    assert_ne!(d1, d0, "le journal porte une identité NEUVE");
    assert_eq!(avant.id, Some(d0), "domaine.id pas encore réécrit");
    assert_eq!(avant.init, Some(d0));
    assert_eq!(recupere, d1, "la réouverture doit converger vers le journal");
    assert_eq!(apres.journal, None, "journal retiré après récupération");
    assert_eq!(apres.id, Some(d1));
    assert_eq!(apres.init, Some(d1), "marqueur réécrit à l'empreinte du nouveau domaine");
}

/// (c) Entre `domaine.id` et `domaine.init` : paire DISCORDANTE sur disque,
/// mais le journal la rend récupérable — la réouverture réécrit le marqueur.
#[test]
fn c_fork_interrompu_entre_id_et_init_est_recupere() {
    let (_td, d0, avant, recupere, apres) = derouler("domaine:fork:entre_id_et_init");
    let d1 = avant.journal.expect("le journal est posé");
    assert_eq!(avant.id, Some(d1), "domaine.id RÉÉCRIT");
    assert_eq!(avant.init, Some(d0), "marqueur ENCORE ancien (paire discordante)");
    assert_ne!(avant.id, avant.init, "la fenêtre (c) est bien une paire discordante");
    assert_eq!(recupere, d1);
    assert_eq!(apres.journal, None);
    assert_eq!(apres.id, Some(d1));
    assert_eq!(apres.init, Some(d1));
}

/// (d) Avant le retrait du journal : identité déjà cohérente, seul le journal
/// reste — la réouverture ne fait que le retirer (idempotent).
#[test]
fn d_fork_interrompu_avant_retrait_journal_est_recupere() {
    let (_td, _d0, avant, recupere, apres) = derouler("domaine:fork:avant_retrait_journal");
    let d1 = avant.journal.expect("le journal est encore là");
    assert_eq!(avant.id, Some(d1));
    assert_eq!(avant.init, Some(d1), "paire déjà cohérente");
    assert_eq!(recupere, d1);
    assert_eq!(apres.journal, None);
    assert_eq!(apres.id, Some(d1));
    assert_eq!(apres.init, Some(d1));
}

/// (e) Après récupération : la base est STABLE et un SECOND fork réussit
/// proprement (nouvelle identité distincte, relue aux réouvertures suivantes).
#[test]
fn e_apres_recuperation_un_second_fork_reussit() {
    let (td, d0, avant, recupere, _apres) = derouler("domaine:fork:entre_id_et_init");
    let d1 = avant.journal.expect("journal");
    assert_eq!(recupere, d1);
    let base = td.path().join("kb");
    let d2 = {
        let store = SubstrateStore::open(&base).unwrap();
        let d2 = store.forker_domaine().unwrap();
        store.close().unwrap();
        d2
    };
    assert_ne!(d2, d0, "le second fork régénère l'identité");
    assert_ne!(d2, d1, "le second fork diffère du premier fork récupéré");
    let relu = {
        let store = SubstrateStore::open(&base).unwrap();
        let d = store.domaine_id();
        store.close().unwrap();
        d
    };
    assert_eq!(relu, d2, "la réouverture relit la seconde identité");
    let final_etat = etat_disque(&base);
    assert_eq!(final_etat.journal, None, "aucun journal résiduel");
    assert_eq!(final_etat.id, Some(d2));
    assert_eq!(final_etat.init, Some(d2));
}
