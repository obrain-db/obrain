//! H3 interprocessus : référence sérialisée + persistée AVANT publication,
//! publication en sous-processus, `kill -9` avant/après la bascule, réouverture
//! en processus indépendant, résolution SANS republier. `Indetermine` ne devient
//! jamais une absence définitive.
//!
//! TROIS INTERRUPTIONS DISTINGUÉES :
//! 1. INTERRUPTION DE PROCESSUS (testée) — `kill -9` à un point arbitraire du
//!    déroulement (avant bascule / après bascule), la base est rouverte en
//!    processus indépendant et l'état est relu/contrôlé.
//! 2. INTERRUPTION AU POINT DE SCELLEMENT (non testée ici) — un `kill -9` PENDANT
//!    le scellement WAL (`seal_commit`) est une fenêtre différente : on ne la
//!    qualifie pas dans ce banc (le marqueur `closed` est posé APRÈS `close()`,
//!    jamais pendant).
//! 3. PERTE ÉLECTRIQUE (NON revendiquée) — `ecrire_fichier` est un tmp+rename
//!    SANS fsync : la visibilité entre processus vient du cache de pages partagé,
//!    pas d'une durabilité physique. La résistance à la coupure électrique n'est
//!    pas qualifiée ici.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, ReferenceTentative, ResolutionEffet,
    ResolutionTentative, SubstrateStore,
};

const ENV_DIR: &str = "H3_RES_DIR";
const ENV_PHASE: &str = "H3_RES_PHASE";
const TEST_ENFANT: &str = "enfant_h3_resolution";
/// Marqueur POSITIF écrit À L'INTÉRIEUR de la boucle infinie `dormir_jusqua_kill` :
/// le parent l'attend avant de tuer, ce qui PROUVE que l'enfant est réellement en
/// train de tourner dans la boucle (donc `store.close()` inatteignable).
const MARQUEUR_BOUCLE: &str = "dans_la_boucle";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Avant,
    Apres,
}
impl Phase {
    fn nom(self) -> &'static str {
        match self {
            Phase::Avant => "avant",
            Phase::Apres => "apres",
        }
    }
}

/// Marqueur de phase ATOMIQUE (tmp + rename) : un kill -9 peut tomber partout.
fn poser_marqueur(dir: &Path, nom: &str) {
    let tmp = dir.join(format!("{nom}.tmp"));
    let fin = dir.join(nom);
    std::fs::write(&tmp, std::process::id().to_string()).unwrap();
    std::fs::rename(&tmp, &fin).unwrap();
}

/// Renommage ATOMIQUE (tmp+rename) : visible par le parent après kill -9 via
/// le cache de pages partagé. Ce n'est PAS un fsync — la persistance PHYSIQUE
/// (résistance à la coupure électrique) n'est PAS qualifiée ici.
fn ecrire_fichier(dir: &Path, nom: &str, contenu: &[u8]) {
    let tmp = dir.join(format!("{nom}.tmp"));
    let fin = dir.join(nom);
    std::fs::write(&tmp, contenu).unwrap();
    std::fs::rename(&tmp, &fin).unwrap();
}

/// Boucle infinie DIVERGENTE : tourne jusqu'au `kill -9`. Écrit un MARQUEUR
/// POSITIF À L'INTÉRIEUR de la boucle (atomique, tmp+rename) : le parent attend
/// CE marqueur avant de tuer, ce qui PROUVE que l'enfant est réellement EN train
/// de tourner ici — donc que `store.close()` est INATTEIGNABLE — et pas
/// simplement qu'il a dépassé la phase. Ce n'est PAS un fsync : la résistance à
/// la coupure électrique n'est pas qualifiée ici.
fn dormir_jusqua_kill(dir: &Path) -> ! {
    loop {
        ecrire_fichier(dir, MARQUEUR_BOUCLE, b"1");
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// ENFANT — construit, SÉRIALISE la référence avant publication, avance jusqu'à
/// la phase demandée, dort jusqu'au kill. No-op sans ENV_DIR.
#[test]
fn enfant_h3_resolution() {
    let Some(dir) = std::env::var_os(ENV_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let phase = std::env::var(ENV_PHASE).unwrap_or_default();

    let store = SubstrateStore::create(dir.join("kb")).unwrap();
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    store.set_node_property(id, "b", Value::Int64(2));
    ecrire_fichier(&dir, "node_id", id.0.to_string().as_bytes());

    let pre = store.capturer_precondition(id, &["a", "b"]).unwrap();
    // ESTAMPILLES INITIALES EXACTES (auteur + horloge) des valeurs d'origine,
    // capturées AVANT publication : après kill -9 avant bascule, le parent doit
    // les retrouver À L'IDENTIQUE (l'état initial est CONSERVÉ, pas seulement
    // « différent de la tentative »).
    let stamp_a = pre.estampille_capturee("a").expect("a présente");
    let stamp_b = pre.estampille_capturee("b").expect("b présente");
    ecrire_fichier(
        &dir,
        "estampilles_initiales",
        format!(
            "{} {} {} {}",
            stamp_a.auteur, stamp_a.horloge, stamp_b.auteur, stamp_b.horloge
        )
        .as_bytes(),
    );
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10)), ("b", Value::Int64(20))],
            &pre,
            ModePublication::Strict,
        )
        .unwrap();
    let reference = prep.reference_tentative();
    // sérialisation + persistance AVANT publication
    ecrire_fichier(&dir, "reference.bin", &reference.to_bytes());

    poser_marqueur(&dir, Phase::Avant.nom());
    if phase == Phase::Avant.nom() {
        dormir_jusqua_kill(&dir);
    }

    let issue = store.publier_si_precondition_intacte(prep).unwrap();
    assert_eq!(issue, IssuePublication::PublieIntegrale);

    poser_marqueur(&dir, Phase::Apres.nom());
    if phase == Phase::Apres.nom() {
        dormir_jusqua_kill(&dir);
    }

    store.close().unwrap();
    poser_marqueur(&dir, "closed");
    if phase == "closed" {
        dormir_jusqua_kill(&dir);
    }
}

fn lancer(phase: Phase) -> (tempfile::TempDir, Child) {
    let td = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let child = Command::new(&exe)
        .args([TEST_ENFANT, "--exact", "--test-threads=1", "--nocapture"])
        .env(ENV_DIR, td.path())
        .env(ENV_PHASE, phase.nom())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");
    (td, child)
}

fn attendre_marqueur(dir: &Path, nom: &str, child: &mut Child) {
    let debut = Instant::now();
    let chemin = dir.join(nom);
    loop {
        if chemin.exists() {
            return;
        }
        if let Some(st) = child.try_wait().expect("try_wait") {
            panic!("enfant terminé ({st}) avant le marqueur « {nom} »");
        }
        assert!(
            debut.elapsed() < Duration::from_secs(60),
            "marqueur « {nom} » non apparu en 60 s"
        );
        std::thread::sleep(Duration::from_millis(10));
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
    assert_eq!(
        st.signal(),
        Some(9),
        "l'enfant doit MOURIR par SIGKILL (9), pas terminer proprement"
    );
}

fn reference_et_resolution(store: &SubstrateStore, dir: &Path) -> (ReferenceTentative, ResolutionTentative) {
    let octets = std::fs::read(dir.join("reference.bin")).unwrap();
    let reference = ReferenceTentative::from_bytes(&octets).unwrap();
    let res = store.resoudre_reference(&reference).unwrap();
    (reference, res)
}

fn node_id(dir: &Path) -> NodeId {
    let s = std::fs::read_to_string(dir.join("node_id")).unwrap();
    NodeId(s.trim().parse().unwrap())
}

/// Estampilles initiales EXACTES (auteur, horloge) des valeurs d'origine,
/// relues depuis ce que l'enfant a persisté AVANT publication.
fn estampilles_initiales(dir: &Path) -> ((u64, u64), (u64, u64)) {
    let s = std::fs::read_to_string(dir.join("estampilles_initiales")).unwrap();
    let mut it = s.split_whitespace().map(|t| t.parse::<u64>().unwrap());
    let a = (it.next().unwrap(), it.next().unwrap());
    let b = (it.next().unwrap(), it.next().unwrap());
    (a, b)
}

/// H3 — kill -9 AVANT publication : référence persistée, tête jamais basculée.
/// Résolution Indéterminée (pas une absence définitive), état initial conservé.
#[test]
fn h3_kill9_avant_publication() {
    let (td, mut child) = lancer(Phase::Avant);
    // MARQUEUR POSITIF : l'enfant est DANS la boucle infinie (close() inatteignable),
    // pas simplement passé devant elle. Le marqueur de phase garantit déjà que la
    // référence et le node_id sont persistés ; le marqueur de boucle prouve que
    // l'enfant n'a PAS pu franchir close().
    attendre_marqueur(td.path(), Phase::Avant.nom(), &mut child);
    attendre_marqueur(td.path(), MARQUEUR_BOUCLE, &mut child);
    tuer_9(&mut child);

    let store = SubstrateStore::open(td.path().join("kb")).unwrap();
    let id = node_id(td.path());
    let (reference, res) = reference_et_resolution(&store, td.path());
    assert_eq!(res.cible(), id);
    assert_eq!(res.auteur_resolvant(), reference.auteur(), "autorité résolvante = auteur de la référence");
    assert_eq!(
        res.integrite(),
        IntegriteTentative::Indeterminee,
        "jamais engagée → indéterminé (jamais « absence définitive »)"
    );
    assert_eq!(store.get_node_property(id, &PropertyKey::new("a")), Some(Value::Int64(1)));
    assert_eq!(store.get_node_property(id, &PropertyKey::new("b")), Some(Value::Int64(2)));
    // IDENTITÉ EXACTE : l'estampille initiale (auteur + horloge) est CONSERVÉE
    // À L'IDENTIQUE après kill -9 avant bascule — pas seulement « différente de
    // la tentative ».
    let (estamp_a, estamp_b) = estampilles_initiales(td.path());
    for i in 0..res.nb_effets() {
        let e = res.effet(i);
        assert_eq!(e.etat(), ResolutionEffet::Indetermine, "chaque clé est indéterminée");
        let attendu = if e.cle() == "a" {
            estamp_a
        } else if e.cle() == "b" {
            estamp_b
        } else {
            panic!("clé inattendue {}", e.cle());
        };
        assert_eq!(
            e.estampille_courante(),
            Some(attendu),
            "l'estampille initiale de {} est CONSERVÉE à l'identique (auteur + horloge)",
            e.cle()
        );
        // la tentative n'a jamais remplacé l'état initial
        assert_ne!(
            e.estampille_courante(),
            Some((reference.auteur(), reference.horloge())),
            "l'estampille de la tentative n'est PAS la courante (jamais engagée)"
        );
    }
    eprintln!("H3_KILL9_AVANT_OK : résolution indéterminée sans republication, estampille initiale conservée");
}

/// H3 — kill -9 APRÈS publication : réponse perdue, réouverture en processus
/// indépendant, résolution Engage, puis masquage → toujours Engage.
#[test]
fn h3_kill9_apres_publication() {
    let (td, mut child) = lancer(Phase::Apres);
    // MARQUEUR POSITIF : l'enfant est DANS la boucle infinie APRÈS publication
    // (close() inatteignable) — la réponse est perdue par le kill, pas par close().
    attendre_marqueur(td.path(), Phase::Apres.nom(), &mut child);
    attendre_marqueur(td.path(), MARQUEUR_BOUCLE, &mut child);
    tuer_9(&mut child);

    let store = SubstrateStore::open(td.path().join("kb")).unwrap();
    let id = node_id(td.path());
    let (reference, res) = reference_et_resolution(&store, td.path());
    assert_eq!(res.cible(), id);
    assert_eq!(res.auteur_resolvant(), reference.auteur(), "autorité résolvante = auteur de la référence");
    assert_eq!(
        res.integrite(),
        IntegriteTentative::Integrale,
        "publication réelle attestée après kill -9"
    );
    assert_eq!(store.get_node_property(id, &PropertyKey::new("a")), Some(Value::Int64(10)));
    assert_eq!(store.get_node_property(id, &PropertyKey::new("b")), Some(Value::Int64(20)));
    for i in 0..res.nb_effets() {
        let e = res.effet(i);
        assert_eq!(e.etat(), ResolutionEffet::Engage, "chaque clé est engagée");
        assert_eq!(
            e.estampille_courante(),
            Some((reference.auteur(), reference.horloge())),
            "IDENTITÉ exacte de l'estampille (auteur + horloge), pas seulement son existence"
        );
    }
    // masquage ultérieur : l'événement historique reste attesté
    store.set_node_property(id, "a", Value::Int64(99));
    let (_reference, res2) = reference_et_resolution(&store, td.path());
    assert_eq!(res2.integrite(), IntegriteTentative::Integrale, "masqué mais engagé");
    assert_eq!(store.get_node_property(id, &PropertyKey::new("a")), Some(Value::Int64(99)));
    eprintln!("H3_KILL9_APRES_OK : résolution Engage + masquage, sans republication");
}
