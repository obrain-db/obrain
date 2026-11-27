//! ATOMICITÉ FORK / PUBLICATION (#334/#418, frontière 1) — la fenêtre TOCTOU
//! entre la garde de domaine de `publier_si_precondition_intacte` et la bascule
//! de tête est FERMÉE par un verrou PARTAGÉ du domaine (lecteurs/écrivain).
//!
//! Avant ce correctif, la garde RELISAIT `domaine.id` SANS verrou : un fork
//! (verrou EXCLUSIF) pouvait s'intercaler entre la relecture (D0) et le commit,
//! et B émettait une confirmation sous une origine D0 déjà invalidée (fantôme).
//! Désormais la publication tient le verrou PARTAGÉ de la relecture à la bascule
//! (RAII) : soit elle commit AVANT le fork (confirmation légitime), soit elle
//! attend le fork puis relit D1 et refuse. Les publications concurrentes
//! (partagé/partagé) ne se bloquent pas entre elles.
//!
//! Le test s'appuie sur le seam REPRENABLE `point_de_reprise`
//! (`domaine:publication:verrou_partage_tenu`, feature `points-de-crash`) : B
//! SUSPEND après la prise du verrou partagé + garde, TIENT le verrou, un fork
//! concurrent BLOQUE, puis la reprise libère la main.

#![cfg(feature = "points-de-crash")]

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::{ModePublication, SubstrateError, SubstrateStore};

const TEST_ENFANT: &str = "enfant_publication_atomicite";
const ENV_ROLE: &str = "OBRAIN_PUBLICATION_ROLE";
const ENV_BASE: &str = "OBRAIN_PUBLICATION_BASE";
/// Lu par `obrain-substrate` (feature `points-de-crash`) : `<nom>:<marqueur>:<reprise>`.
const ENV_REPRISE: &str = "OBRAIN_SUBSTRATE_POINT_REPRISE";
const SEAM: &str = "domaine:publication:verrou_partage_tenu";

fn role() -> Option<String> {
    std::env::var(ENV_ROLE).ok()
}

fn base() -> PathBuf {
    PathBuf::from(std::env::var(ENV_BASE).expect("base absente dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

/// Écriture ATOMIQUE (tmp + rename) : visible par le parent via le cache de pages.
fn ecrire_fichier(dir: &Path, nom: &str, contenu: &[u8]) {
    let tmp = dir.join(format!("{nom}.tmp"));
    let fin = dir.join(nom);
    std::fs::write(&tmp, contenu).unwrap();
    std::fs::rename(&tmp, &fin).unwrap();
}

fn attendre_fichier(p: &Path, quoi: &str) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(60),
            "attente de {quoi} ({}) : délai dépassé",
            p.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    /// Lance l'enfant du test `TEST_ENFANT` avec `role` et `base`, et
    /// éventuellement une consigne de suspension reprise (`None` sinon).
    fn lancer(role: &str, base: &Path, reprise: Option<&str>) -> Self {
        let exe = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(&exe);
        cmd.args(["--exact", TEST_ENFANT, "--test-threads=1", "--nocapture"])
            .env(ENV_ROLE, role)
            .env(ENV_BASE, base)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(consigne) = reprise {
            cmd.env(ENV_REPRISE, consigne);
        }
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        Self {
            process,
            lignes: std::io::BufReader::new(sortie).lines(),
        }
    }

    fn attendre(&mut self, attendue: &str) -> String {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) if l.contains(attendue) => return l,
                Some(Ok(_)) => continue,
                autre => panic!("enfant terminé sans « {attendue} » ({autre:?})"),
            }
        }
    }

    fn terminer(&mut self) {
        let st = self.process.wait().expect("attendre l'enfant");
        assert!(st.success(), "l'enfant a échoué : {st:?}");
    }
}

/// ENFANT — rôle « forkeur » (A) : ouvre, annonce, attend que le parent ait
/// constaté la suspension de B, puis fork (EXCLUSIF). Si B tient le verrou
/// partagé, ce fork BLOQUE ; il n'aboutit qu'à la libération.
fn enfant_forkeur(base: &Path, dir: &Path) {
    let store = SubstrateStore::open_shared_write(base).unwrap();
    let _cache_initial = store.domaine_id();
    annoncer("PRET_A");
    let nouveau = store.forker_domaine().unwrap();
    ecrire_fichier(dir, "d1", &nouveau.to_be_bytes());
    store.close().unwrap();
    annoncer("FORK_OK");
}

/// ENFANT — rôle « publieur » (B) : ouvre (cache D0), prépare une transformation
/// sous D0, puis publie. Le seam `SEAM` (armé par l'environnement) SUSPEND après
/// la prise du verrou partagé + garde, le verrou TENU. À la reprise, la
/// publication commit (confirmation légitime sous D0), puis le verrou est rendu.
fn enfant_publieur(base: &Path, dir: &Path) {
    let store = SubstrateStore::open_shared_write(base).unwrap();
    let domaine_cache = store.domaine_id();
    annoncer(&format!("PRET_B {domaine_cache:#x}"));

    let id = NodeId(
        std::fs::read_to_string(dir.join("node_id"))
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    let pre = store.capturer_precondition(id, &["a"]).unwrap();
    let prep = store
        .preparer_transformation(id, &[("a", Value::Int64(2))], &pre, ModePublication::Strict)
        .unwrap();
    annoncer("PREPARE_B");

    // Attend le signal du parent avant de publier : le parent fork d'abord (chemin
    // inverse), ou libère immédiatement (exclusion mutuelle, où le seam suspend).
    attendre_fichier(&dir.join("go_b"), "GO_B");

    // Publication : le seam suspend ici (le verrou partagé est déjà tenu).
    let verdict = match store.publier_si_precondition_intacte(prep) {
        Ok(issue) => format!("publie {issue:?}"),
        Err(SubstrateError::DomaineForke {
            prepare_sous,
            courant,
        }) => format!("refuse_domaine_forke {prepare_sous:#x} {courant:#x}"),
        Err(autre) => format!("refuse_autre {autre:?}"),
    };
    ecrire_fichier(dir, "verdict_b", verdict.as_bytes());
    annoncer("PUBLI_B");
    let _ = store.close();
}

#[test]
fn enfant_publication_atomicite() {
    let Some(r) = role() else { return };
    let b = base();
    let dir = b.parent().unwrap().to_path_buf();
    match r.as_str() {
        "forkeur" => enfant_forkeur(&b, &dir),
        "publieur" => enfant_publieur(&b, &dir),
        autre => panic!("rôle inconnu {autre}"),
    }
}

/// FRONT 1 — exclusion mutuelle : B, suspendu APRÈS la prise du verrou partagé
/// (garde passée), BLOQUE le fork exclusif de A. B reprend, commit sous D0
/// (confirmation LÉGITIME), libère ; A fork ensuite. Jamais un fantôme.
#[test]
fn publication_et_fork_sont_mutuellement_exclusifs() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let dir = td.path();

    let (id, d0) = {
        let store = SubstrateStore::create(&base).unwrap();
        let d0 = store.domaine_id();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "a", Value::Int64(1));
        store.close().unwrap();
        (n, d0)
    };
    ecrire_fichier(dir, "node_id", id.0.to_string().as_bytes());

    let marqueur = dir.join("b_hold");
    let reprise = dir.join("reprise_b");
    let consigne = format!("{SEAM}:{}:{}", marqueur.display(), reprise.display());

    // B (publieur) : armé pour suspendre au seam, verrou partagé tenu.
    let mut b = Enfant::lancer("publieur", &base, Some(&consigne));
    let pret_b = b.attendre("PRET_B");
    assert!(
        pret_b.contains(&format!("{d0:#x}")),
        "B doit annoncer l'identité initiale {d0:#x}, obtenu {pret_b}"
    );
    b.attendre("PREPARE_B");
    // Libère B : il publie, prend le verrou partagé, passe la garde, puis
    // SUSPEND au seam (le verrou est TENU).
    std::fs::write(dir.join("go_b"), b"go").unwrap();
    // B a atteint le seam : verrou partagé tenu, garde passée, avant commit.
    attendre_fichier(&marqueur, "B_hold");

    // A (forkeur) : tente le fork EXCLUSIF — doit BLOQUER tant que B tient.
    let mut a = Enfant::lancer("forkeur", &base, None);
    a.attendre("PRET_A");
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        !dir.join("d1").exists(),
        "A ne doit PAS aboutir tant que B tient le verrou partagé (fork bloqué)"
    );

    // Reprise de B : commit, libération du verrou partagé.
    std::fs::write(&reprise, b"go").unwrap();
    b.attendre("PUBLI_B");
    a.attendre("FORK_OK");
    b.terminer();
    a.terminer();

    // B a publié LÉGITIMEMENT (sous D0, AVANT le fork) — jamais un fantôme.
    let verdict = std::fs::read_to_string(dir.join("verdict_b")).unwrap();
    assert!(
        verdict.starts_with("publie PublieIntegrale"),
        "B devait publier intégralement sous D0, obtenu {verdict}"
    );
    let d1 = u128::from_be_bytes(
        std::fs::read(dir.join("d1"))
            .unwrap()
            .try_into()
            .expect("16 octets"),
    );
    assert_ne!(d1, d0, "le fork de A régénère l'identité");

    // Réouverture au domaine courant (D1) : la confirmation de B (émise sous D0)
    // est un COMMIT RÉEL, pas un fantôme — la donnée est là, le domaine a avancé.
    {
        let store = SubstrateStore::open(&base).unwrap();
        assert_eq!(store.domaine_id(), d1, "le domaine courant est celui du fork");
        assert_eq!(
            store.get_node_property(id, &PropertyKey::new("a")),
            Some(Value::Int64(2)),
            "la publication de B, antérieure au fork, est un commit réel"
        );
        store.close().unwrap();
    }
}

/// FRONT 1 — chemin inverse : A fork d'abord (verrou exclusif libéré), B publie
/// ensuite sous son ancienne origine → `DomaineForke`, rien n'est écrit. Le
/// verrou partagé n'a pas aboli le refus : il n'a fait que borner la fenêtre.
#[test]
fn publication_apres_fork_complet_est_refusee() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let dir = td.path();

    let (id, d0) = {
        let store = SubstrateStore::create(&base).unwrap();
        let d0 = store.domaine_id();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "a", Value::Int64(1));
        store.close().unwrap();
        (n, d0)
    };
    ecrire_fichier(dir, "node_id", id.0.to_string().as_bytes());

    // B ouvre D'ABORD (cache D0), prépare, puis attend le signal. Le fork vers
    // D1 a lieu APRÈS (processus parent, verrou exclusif libéré) : B publie sous
    // une origine déjà invalidée.
    let mut b = Enfant::lancer("publieur", &base, None);
    let pret_b = b.attendre("PRET_B");
    assert!(pret_b.contains(&format!("{d0:#x}")), "B cache encore D0 : {pret_b}");
    b.attendre("PREPARE_B");

    // Fork en écriture PARTAGÉE (même verrou de répertoire que B, qui reste
    // ouvert) : le verrou EXCLUSIF du domaine est pris et libéré avant que B ne
    // publie — pas de conflit de verrou de répertoire.
    let d1 = {
        let store = SubstrateStore::open_shared_write(&base).unwrap();
        let d1 = store.forker_domaine().unwrap();
        store.close().unwrap();
        d1
    };
    std::fs::write(dir.join("go_b"), b"go").unwrap();

    b.attendre("PUBLI_B");
    b.terminer();

    let verdict = std::fs::read_to_string(dir.join("verdict_b")).unwrap();
    assert!(
        verdict.starts_with("refuse_domaine_forke"),
        "B devait être refusé (DomaineForke), obtenu {verdict}"
    );
    {
        let store = SubstrateStore::open(&base).unwrap();
        assert_eq!(store.domaine_id(), d1);
        assert_eq!(
            store.get_node_property(id, &PropertyKey::new("a")),
            Some(Value::Int64(1)),
            "la publication refusée ne laisse aucune trace"
        );
        store.close().unwrap();
    }
}
