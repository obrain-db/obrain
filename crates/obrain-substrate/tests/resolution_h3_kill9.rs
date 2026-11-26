//! H3 interprocessus : référence sérialisée + persistée AVANT publication,
//! publication en sous-processus, `kill -9` avant/après la bascule, réouverture
//! en processus indépendant, résolution SANS republier. `Indetermine` ne devient
//! jamais une absence définitive.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::{
    IntegriteTentative, IssuePublication, ModePublication, ReferenceTentative, ResolutionTentative,
    SubstrateStore,
};

const ENV_DIR: &str = "H3_RES_DIR";
const ENV_PHASE: &str = "H3_RES_PHASE";
const TEST_ENFANT: &str = "enfant_h3_resolution";

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

fn ecrire_fichier(dir: &Path, nom: &str, contenu: &[u8]) {
    let tmp = dir.join(format!("{nom}.tmp"));
    let fin = dir.join(nom);
    std::fs::write(&tmp, contenu).unwrap();
    std::fs::rename(&tmp, &fin).unwrap();
}

fn dormir_jusqua_kill() -> ! {
    loop {
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
        dormir_jusqua_kill();
    }

    let issue = store.publier_si_precondition_intacte(prep).unwrap();
    assert_eq!(issue, IssuePublication::PublieIntegrale);

    poser_marqueur(&dir, Phase::Apres.nom());
    if phase == Phase::Apres.nom() {
        dormir_jusqua_kill();
    }

    store.close().unwrap();
    poser_marqueur(&dir, "closed");
    if phase == "closed" {
        dormir_jusqua_kill();
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
    let _ = child.wait();
}

fn resolution_reference(store: &SubstrateStore, dir: &Path) -> ResolutionTentative {
    let octets = std::fs::read(dir.join("reference.bin")).unwrap();
    let reference = ReferenceTentative::from_bytes(&octets).unwrap();
    store.resoudre_reference(&reference).unwrap()
}

fn node_id(dir: &Path) -> NodeId {
    let s = std::fs::read_to_string(dir.join("node_id")).unwrap();
    NodeId(s.trim().parse().unwrap())
}

/// H3 — kill -9 AVANT publication : référence persistée, tête jamais basculée.
/// Résolution Indéterminée (pas une absence définitive), état initial conservé.
#[test]
fn h3_kill9_avant_publication() {
    let (td, mut child) = lancer(Phase::Avant);
    attendre_marqueur(td.path(), Phase::Avant.nom(), &mut child);
    tuer_9(&mut child);

    let store = SubstrateStore::open(td.path().join("kb")).unwrap();
    let id = node_id(td.path());
    let res = resolution_reference(&store, td.path());
    assert_eq!(res.cible, id);
    assert_eq!(
        res.integrite,
        IntegriteTentative::Indeterminee,
        "jamais engagée → indéterminé (jamais « absence définitive »)"
    );
    assert_eq!(store.get_node_property(id, &PropertyKey::new("a")), Some(Value::Int64(1)));
    eprintln!("H3_KILL9_AVANT_OK : résolution indéterminée sans republication");
}

/// H3 — kill -9 APRÈS publication : réponse perdue, réouverture en processus
/// indépendant, résolution Engage, puis masquage → toujours Engage.
#[test]
fn h3_kill9_apres_publication() {
    let (td, mut child) = lancer(Phase::Apres);
    attendre_marqueur(td.path(), Phase::Apres.nom(), &mut child);
    tuer_9(&mut child);

    let store = SubstrateStore::open(td.path().join("kb")).unwrap();
    let id = node_id(td.path());
    let res = resolution_reference(&store, td.path());
    assert_eq!(res.cible, id);
    assert_eq!(
        res.integrite,
        IntegriteTentative::Integrale,
        "publication réelle attestée après kill -9"
    );
    assert_eq!(store.get_node_property(id, &PropertyKey::new("a")), Some(Value::Int64(10)));
    // masquage ultérieur : l'événement historique reste attesté
    store.set_node_property(id, "a", Value::Int64(99));
    let res2 = resolution_reference(&store, td.path());
    assert_eq!(res2.integrite, IntegriteTentative::Integrale, "masqué mais engagé");
    assert_eq!(store.get_node_property(id, &PropertyKey::new("a")), Some(Value::Int64(99)));
    eprintln!("H3_KILL9_APRES_OK : résolution Engage + masquage, sans republication");
}
