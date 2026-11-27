//! FORK BIPROCESSUS RÉEL (#334/#418) — le contrat le plus dur de l'identité de
//! domaine : deux processus OS INDÉPENDANTS (jamais deux threads), deux handles
//! ouverts sur la même base, identité initiale commune.
//!
//! Processus A déclare un fork (nouvelle identité persistée). Processus B, qui
//! garde l'ANCIENNE identité en cache (`domaine_id` lu à l'ouverture, jamais
//! rafraîchi), tente une publication (commit d'une transformation
//! conditionnelle) sous l'ancienne origine.
//!
//! CONTRAT : B ne doit PAS pouvoir produire une confirmation sous une origine
//! invalidée — refus net (`SubstrateError::DomaineForke`), pas de commit
//! fantôme. La référence émise sous l'ancien domaine reste indéterminée pour
//! tout résolveur au domaine courant.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::{
    IntegriteTentative, ModePublication, ReferenceTentative, SubstrateError, SubstrateStore,
};

const TEST_ENFANT: &str = "enfant_fork_biprocessus";
const ENV_ROLE: &str = "OBRAIN_FORK_ROLE";
const ENV_BASE: &str = "OBRAIN_FORK_BASE";

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

/// Écriture ATOMIQUE (tmp + rename) : visible par le parent après un `kill -9`
/// via le cache de pages partagé. Ce n'est PAS un fsync.
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
    fn lancer(test: &str, role: &str, base: &Path) -> Self {
        let exe = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(&exe);
        cmd.args(["--exact", test, "--test-threads=1", "--nocapture"])
            .env(ENV_ROLE, role)
            .env(ENV_BASE, base)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
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

/// ENFANT — rôle « forkeur » (processus A) : ouvre, annonce, attend le GO, fork,
/// persiste le nouveau domaine, annonce.
fn enfant_forkeur(base: &Path, dir: &Path) {
    let store = SubstrateStore::open_shared_write(base).unwrap();
    let _cache_initial = store.domaine_id();
    annoncer("PRET_A");
    attendre_fichier(&dir.join("go_a"), "GO_A");
    let nouveau = store.forker_domaine().unwrap();
    ecrire_fichier(dir, "d1", &nouveau.to_be_bytes());
    store.close().unwrap();
    annoncer("FORK_OK");
}

/// ENFANT — rôle « publieur » (processus B) : ouvre (cache l'ANCIENNE identité),
/// prépare une transformation sous cette origine, attend le fork du pair, tente
/// de publier. Consigne le verdict dans un fichier.
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
    // Référence AVANT publication (elle porte le domaine capturé à la préparation).
    let reference = prep.reference_tentative();
    ecrire_fichier(dir, "reference.bin", &reference.to_bytes());
    annoncer("PREPARE_B");

    // Attend que le pair ait fork (le domaine persistant a changé).
    attendre_fichier(&dir.join("d1"), "D1");

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
fn enfant_fork_biprocessus() {
    let Some(r) = role() else { return };
    let b = base();
    // Le répertoire de coordination est le parent de la base.
    let dir = b.parent().unwrap().to_path_buf();
    match r.as_str() {
        "forkeur" => enfant_forkeur(&b, &dir),
        "publieur" => enfant_publieur(&b, &dir),
        autre => panic!("rôle inconnu {autre}"),
    }
}

/// FRONT 1 — le contrat central : B, qui garde l'ancienne identité en cache,
/// ne peut pas publier sous une origine invalidée par le fork de A.
#[test]
fn b_ne_publie_pas_sous_une_origine_invalidee_par_le_fork() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let dir = td.path();

    // Base identifiée + nœud porteur d'une propriété.
    let (id, d0) = {
        let store = SubstrateStore::create(&base).unwrap();
        let d0 = store.domaine_id();
        let n = store.create_node(&["ressource"]);
        store.set_node_property(n, "a", Value::Int64(1));
        store.close().unwrap();
        (n, d0)
    };
    ecrire_fichier(dir, "node_id", id.0.to_string().as_bytes());

    // A (forkeur) et B (publieur) : deux processus OS indépendants, deux handles
    // shared-write sur la même base, identité initiale commune.
    let mut a = Enfant::lancer(TEST_ENFANT, "forkeur", &base);
    let mut b = Enfant::lancer(TEST_ENFANT, "publieur", &base);

    a.attendre("PRET_A");
    let pret_b = b.attendre("PRET_B");
    // B annonce son identité en cache : c'est D0 (l'identité initiale commune).
    assert!(
        pret_b.contains(&format!("{d0:#x}")),
        "B doit annoncer l'identité initiale commune {d0:#x}, obtenu {pret_b}"
    );

    // B a préparé sa transformation AVANT le fork (le domaine capturé est D0).
    b.attendre("PREPARE_B");

    // On déclenche le fork de A ; B attend déjà la nouvelle identité.
    std::fs::write(dir.join("go_a"), b"go").unwrap();
    a.attendre("FORK_OK");
    b.attendre("PUBLI_B");
    a.terminer();
    b.terminer();

    // Verdict de B : refus net, jamais une publication.
    let verdict = std::fs::read_to_string(dir.join("verdict_b")).unwrap();
    assert!(
        verdict.starts_with("refuse_domaine_forke"),
        "B a pu publier (ou a été refusé pour une autre raison) : {verdict}"
    );
    let d1 = u128::from_be_bytes(
        std::fs::read(dir.join("d1"))
            .unwrap()
            .try_into()
            .expect("16 octets"),
    );

    // PAS DE COMMIT FANTÔME : réouverture au domaine courant (D1) → la valeur
    // d'origine est intacte, la propriété n'a PAS été écrite à 2.
    {
        let store = SubstrateStore::open(&base).unwrap();
        assert_eq!(store.domaine_id(), d1, "le domaine courant est celui du fork");
        assert_eq!(
            store.get_node_property(id, &PropertyKey::new("a")),
            Some(Value::Int64(1)),
            "la publication refusée ne doit laisser aucune trace"
        );
        // La référence émise sous D0 est ÉTRANGÈRE au domaine courant : résolution
        // intégralement indéterminée, sans consulter le contenu.
        let octets = std::fs::read(dir.join("reference.bin")).unwrap();
        let reference = ReferenceTentative::from_bytes(&octets).unwrap();
        assert_eq!(reference.domaine(), d0, "la référence porte l'ancien domaine");
        let res = store.resoudre_reference(&reference).unwrap();
        assert_eq!(
            res.integrite(),
            IntegriteTentative::Indeterminee,
            "référence d'un domaine invalidé : indéterminée, jamais engagée"
        );
        store.close().unwrap();
    }
}
