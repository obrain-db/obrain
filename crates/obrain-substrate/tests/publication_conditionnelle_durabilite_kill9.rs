//! Durabilité par sous-processus (`kill -9`) de la primitive de publication
//! conditionnelle (`capturer_precondition` / `preparer_transformation` /
//! `publier_si_precondition_intacte`).
//!
//! MÉTHODE — réutilise le motif existant de `simplex_durability.rs` (et
//! `mp8_fenetre_rejeu.rs`) : le binaire de test se relance lui-même en enfant
//! (`std::process::Command` sur `current_exe`, filtre `--exact`). L'enfant
//! atteint un point DÉTERMINISTE du cycle de publication, pose un marqueur de
//! phase (fichier sentinelle écrit ATOMIQUEMENT par rename — un `kill -9` peut
//! tomber n'importe où, le parent ne doit jamais lire un fichier tronqué), puis
//! s'endort ; le parent attend le marqueur, tue l'enfant par `kill -9`, rouvre
//! le store et vérifie l'état canonique.
//!
//! POINTS TUÉS (les trois états où le processus peut mourir en plein cycle) :
//!   (a) `apres_preparation` — `preparer_transformation` a rendu Ok : pages
//!       privées posées dans la PropsZone (mmap), tête canonique NON basculée ;
//!   (b) `apres_bascule`    — `publier_si_precondition_intacte` a rendu
//!       `PublieIntegrale` : tête basculée (`NodePropHeadUpdate` au WAL),
//!       mais AUCUN `flush()`/`close()` ;
//!   (c) `apres_close`      — `close()` propre (flush + fsync) rendu, puis kill.
//!
//! CE QU'UN `kill -9` DÉMONTRE, ET CE QU'IL NE DÉMONTRE PAS (honnêteté) :
//!   * DÉMONTRÉ — la bascule de tête survit au rejeu du WAL : le POINTEUR est
//!     journalisé (`NodePropHeadUpdate`, WAL d'abord puis mmap), et son rejeu
//!     est idempotent. Une préparation abandonnée (tête non basculée) ne fuit
//!     AUCUN effet canonique. La chaîne de propriétés reste lisible (aucune
//!     erreur de décodage), l'estampille est cohérente.
//!   * NON DÉMONTRÉ — la perte électrique. Un `SIGKILL` tue le processus mais
//!     laisse le cache de pages de l'OS intact : les pages mmap SALE de la
//!     PropsZone survivent à la mort du processus (elles appartiennent au
//!     fichier, pas au processus). Le CONTENU privé n'est durable qu'au
//!     `flush()`/`checkpoint`/`close()`. Le test (b) passe donc sous `kill -9`
//!     sans prouver qu'il passerait sous coupure de courant (il faut un fsync
//!     forcé + barrière matérielle, hors de portée d'un test en sous-processus).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::store::{EtatPropriete, IssuePublication, ModePublication, SubstrateStore};

/// Nom du test enfant (relancé avec `--exact`).
const TEST_ENFANT: &str = "enfant_publication_durabilite";
/// Variables d'environnement lues par l'enfant.
const ENV_DIR: &str = "PUB_DUR_DIR";
const ENV_PHASE: &str = "PUB_DUR_PHASE";

/// Point du cycle où l'enfant s'arrête et se fait tuer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::enum_variant_names)]
enum Phase {
    ApresPreparation,
    ApresBascule,
    ApresClose,
}

impl Phase {
    /// Nom du marqueur de phase (et valeur de `PUB_DUR_PHASE`).
    fn nom(self) -> &'static str {
        match self {
            Phase::ApresPreparation => "prepared",
            Phase::ApresBascule => "published",
            Phase::ApresClose => "closed",
        }
    }
}

/// Écrit un fichier sentinelle ATOMIQUEMENT (tmp + rename) : un `kill -9` peut
/// tomber n'importe où, le parent ne doit jamais lire un fichier tronqué.
fn poser_marqueur(dir: &Path, nom: &str) {
    let tmp = dir.join(format!("{nom}.tmp"));
    let fin = dir.join(nom);
    std::fs::write(&tmp, std::process::id().to_string()).unwrap();
    std::fs::rename(&tmp, &fin).unwrap();
}

/// Écrit un contenu arbitraire, atomiquement (même contrat que le marqueur).
fn ecrire_fichier(dir: &Path, nom: &str, contenu: &str) {
    let tmp = dir.join(format!("{nom}.tmp"));
    let fin = dir.join(nom);
    std::fs::write(&tmp, contenu).unwrap();
    std::fs::rename(&tmp, &fin).unwrap();
}

/// Estampille complète `(auteur, horloge)` d'une propriété présente.
fn stamp(store: &SubstrateStore, id: NodeId, key: &str) -> (u64, u64) {
    match store.lire_etat_propriete(id, key).unwrap() {
        EtatPropriete::Presente { estampille, .. } => (estampille.auteur, estampille.horloge),
        autre => panic!("{key} attendue présente, lu {autre:?}"),
    }
}

fn ecrire_stamp(dir: &Path, nom: &str, s: (u64, u64)) {
    ecrire_fichier(dir, nom, &format!("{} {}", s.0, s.1));
}

fn lire_stamp(dir: &Path, nom: &str) -> (u64, u64) {
    let s = std::fs::read_to_string(dir.join(nom)).unwrap();
    let mut it = s.split_whitespace();
    let auteur: u64 = it.next().unwrap().parse().unwrap();
    let horloge: u64 = it.next().unwrap().parse().unwrap();
    (auteur, horloge)
}

/// Bloque le fil jusqu'au `kill -9` (jamais rendu : l'enfant est tué).
fn dormir_jusqua_kill() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// ENFANT — construit le store, avance jusqu'à la phase demandée, pose les
/// marqueurs, puis dort jusqu'au kill. No-op sans `PUB_DUR_DIR` (le harnais de
/// test complet la voit comme un test trivialement vert).
#[test]
fn enfant_publication_durabilite() {
    let Some(dir) = std::env::var_os(ENV_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let phase = std::env::var(ENV_PHASE).unwrap_or_default();

    let store = SubstrateStore::create(dir.join("kb")).unwrap();
    let id = store.create_node(&["ressource"]);
    store.set_node_property(id, "a", Value::Int64(1));
    store.set_node_property(id, "b", Value::Int64(2));

    ecrire_fichier(&dir, "node_id", &id.0.to_string());

    let pre = store.capturer_precondition(id, &["a", "b"]).unwrap();
    let prep = store
        .preparer_transformation(
            id,
            &[("a", Value::Int64(10)), ("b", Value::Int64(20))],
            &pre,
            ModePublication::Lww,
        )
        .unwrap();

    // Estampilles DE BASE (a=1, b=2) — tête non encore basculée.
    ecrire_stamp(&dir, "stamp_a_pre", stamp(&store, id, "a"));
    ecrire_stamp(&dir, "stamp_b_pre", stamp(&store, id, "b"));

    poser_marqueur(&dir, Phase::ApresPreparation.nom());
    if phase == Phase::ApresPreparation.nom() {
        dormir_jusqua_kill();
    }

    let issue = store.publier_si_precondition_intacte(prep).unwrap();
    assert_eq!(issue, IssuePublication::PublieIntegrale);

    // Estampilles PUBLIÉES (a=10, b=20) — tête basculée.
    ecrire_stamp(&dir, "stamp_a_pub", stamp(&store, id, "a"));
    ecrire_stamp(&dir, "stamp_b_pub", stamp(&store, id, "b"));

    poser_marqueur(&dir, Phase::ApresBascule.nom());
    if phase == Phase::ApresBascule.nom() {
        dormir_jusqua_kill();
    }

    store.close().unwrap();
    poser_marqueur(&dir, Phase::ApresClose.nom());
    if phase == Phase::ApresClose.nom() {
        dormir_jusqua_kill();
    }
    // Phase inconnue : sortie propre (baseline possible).
}

fn lancer(test: &str, phase: Phase) -> (tempfile::TempDir, Child) {
    let td = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let child = Command::new(&exe)
        .args([test, "--exact", "--test-threads=1", "--nocapture"])
        .env(ENV_DIR, td.path())
        .env(ENV_PHASE, phase.nom())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn de l'enfant");
    (td, child)
}

/// Attend le marqueur de phase ; panique si l'enfant meurt avant de le poser.
fn attendre_marqueur(dir: &Path, nom: &str, child: &mut Child) {
    let debut = Instant::now();
    let chemin = dir.join(nom);
    loop {
        if chemin.exists() {
            return;
        }
        if let Some(st) = child.try_wait().expect("try_wait") {
            panic!("l'enfant a terminé (status {st}) avant le marqueur « {nom} »");
        }
        assert!(
            debut.elapsed() < Duration::from_secs(60),
            "marqueur « {nom} » non apparu en 60 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `kill -9` explicite (SIGKILL), puis récolte du zombie.
fn tuer_9(child: &mut Child) {
    let pid = child.id();
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill -9");
    assert!(status.success(), "kill -9 a échoué");
    let _ = child.wait();
}

/// Vérifie qu'une propriété est présente, avec la bonne valeur ET la bonne
/// estampille complète (auteur + horloge) — c'est la preuve que ni la valeur
/// ni son identité d'écriture n'ont été corrompues par le crash.
fn verifier(
    store: &SubstrateStore,
    id: NodeId,
    key: &str,
    valeur: Value,
    stamp_attendue: (u64, u64),
) {
    // Chemin de lecture ponctuel (celui du moteur).
    assert_eq!(
        store.get_node_property(id, &PropertyKey::new(key)),
        Some(valeur.clone()),
        "{key} : valeur canonique via get_node_property"
    );
    // Chemin estampillé : valeur + identité d'écriture.
    match store
        .lire_etat_propriete(id, key)
        .expect("lecture sans corruption")
    {
        EtatPropriete::Presente {
            estampille,
            valeur: v,
        } => {
            assert_eq!(v, valeur, "{key} : valeur via lire_etat_propriete");
            assert_eq!(
                (estampille.auteur, estampille.horloge),
                stamp_attendue,
                "{key} : estampille (auteur, horloge) après réouverture"
            );
        }
        autre => panic!("{key} attendue présente (estampillée), lu {autre:?}"),
    }
}

/// Scénario commun : l'enfant meurt à la phase demandée, le parent rouvre et
/// vérifie l'état canonique, les estampilles et la cohérence de projection.
fn scenario(phase: Phase) {
    let (td, mut child) = lancer(TEST_ENFANT, phase);
    attendre_marqueur(td.path(), phase.nom(), &mut child);
    tuer_9(&mut child);

    // Réouverture après crash : toute corruption ferait échouer `open` ici.
    let store = SubstrateStore::open(td.path().join("kb")).expect("réouverture après kill -9");

    let id = NodeId(
        std::fs::read_to_string(td.path().join("node_id"))
            .unwrap()
            .trim()
            .parse()
            .expect("node_id"),
    );
    // Le nœud est vivant (le record n'a pas été corrompu).
    assert!(
        store.get_node(id).is_some(),
        "nœud {id} absent après réouverture"
    );

    let (pre_a, pre_b) = (
        lire_stamp(td.path(), "stamp_a_pre"),
        lire_stamp(td.path(), "stamp_b_pre"),
    );

    match phase {
        // (a) tête NON basculée : seules les valeurs de base sont canoniques.
        Phase::ApresPreparation => {
            verifier(&store, id, "a", Value::Int64(1), pre_a);
            verifier(&store, id, "b", Value::Int64(2), pre_b);
            // La bascule n'a jamais eu lieu : le marqueur « published » manque,
            // et la valeur publiée (a=10) est introuvable par la projection.
            assert!(
                !td.path().join(Phase::ApresBascule.nom()).exists(),
                "la bascule de tête ne devait pas avoir eu lieu"
            );
            assert!(
                store
                    .find_nodes_by_property("a", &Value::Int64(10))
                    .is_empty(),
                "une préparation abandonnée ne doit rendre AUCUN effet canonique"
            );
            assert!(
                store
                    .find_nodes_by_property("a", &Value::Int64(1))
                    .contains(&id),
                "la projection doit retrouver la valeur de base"
            );
        }
        // (b)/(c) tête basculée : les valeurs publiées sont canoniques.
        Phase::ApresBascule | Phase::ApresClose => {
            let (pub_a, pub_b) = (
                lire_stamp(td.path(), "stamp_a_pub"),
                lire_stamp(td.path(), "stamp_b_pub"),
            );
            verifier(&store, id, "a", Value::Int64(10), pub_a);
            verifier(&store, id, "b", Value::Int64(20), pub_b);
            assert!(
                store
                    .find_nodes_by_property("a", &Value::Int64(10))
                    .contains(&id),
                "la projection doit retrouver la valeur publiée"
            );
        }
    }
}

/// (a) `kill -9` APRÈS préparation : pages privées posées, tête non basculée.
#[test]
fn kill9_apres_preparation_tete_non_basculee() {
    scenario(Phase::ApresPreparation);
}

/// (b) `kill -9` APRÈS bascule de tête, sans flush/close.
#[test]
fn kill9_apres_bascule_de_tete_sans_close() {
    scenario(Phase::ApresBascule);
}

/// (c) `kill -9` APRÈS `close()` propre (confirmation durable).
#[test]
fn kill9_apres_close_propre() {
    scenario(Phase::ApresClose);
}
