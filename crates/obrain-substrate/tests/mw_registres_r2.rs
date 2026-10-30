//! ── REGISTRES PARTAGÉS — CHECKS INTER-PROCESSUS (Phase B, pièce 3) ───────────
//!
//! Les trois registres (labels, clés de propriétés, types d'arêtes) internent des
//! NOMS en INDICES, et ces indices partent dans les données : `label_bitset`,
//! `prop_key_id`, `edge_type`. Tant que l'internement était une `HashMap` PAR
//! PROCESSUS (`store.rs`, `LabelRegistry::intern` / `EdgeTypeRegistry::intern` /
//! `PropertyKeyRegistry::intern`), l'indice était le compteur d'arrivée de CE
//! processus : deux processus qui internent des noms DIFFÉRENTS recevaient le
//! MÊME indice, et leurs bitsets désignaient alors deux libellés distincts.
//!
//! ── POURQUOI DES PROCESSUS, ET PAS DES THREADS ─────────────────────────────
//! Le défaut est PAR PROCESSUS par construction : deux threads du même processus
//! partagent le registre et n'ont jamais pu diverger. Un test multithread aurait
//! été vert AVANT la pièce — c'est le piège que la RFC a déjà rencontré sur
//! l'allocation. Le harnais est celui de `mw_interprocess_phase_b.rs` : le binaire
//! de test se relance en enfant (`current_exe` + `--exact <ce test> --nocapture`),
//! le rôle vient de `MWR_ROLE`, la synchronisation passe par stdout et par des
//! fichiers-drapeaux.
//!
//! ── UN TEST PAR FICHIER, ET POURQUOI ───────────────────────────────────────
//! Le binaire de test se relance en ENFANT (`current_exe --exact <ce test>`).
//! Plusieurs tests inter-processus dans un même fichier se marchent dessus : chaque
//! enfant relance la DÉCOUVERTE de toute la suite (`--list`), et deux enfants
//! simultanés cassent le tube de découverte du parent (« BrokenPipe »), qui voit
//! alors tourner des tests qu'il n'a pas demandés. Un fichier = un test = un enfant.
//! Les trois pièces sont `mw_registres_r1.rs` (noms différents → bits différents),
//! `mw_registres_r2.rs` (même nom → même bit) et `mw_registres_r3.rs` (survit au
//! `kill -9`).
//!
//! ── CE QUE CES TESTS MESURENT, ET RIEN D'AUTRE ─────────────────────────────
//! Ils ne mesurent pas une latence : ils mesurent si l'indice rendu à deux
//! processus DÉSIGNE LA MÊME CHOSE POUR TOUT LE MONDE. C'est une question de
//! SENS, pas de vitesse — et c'est pour cela que la vérification finale passe par
//! `get_node_labels`, c'est-à-dire par les mêmes octets qu'un lecteur humain.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_common::types::NodeId;
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
use obrain_substrate::SubstrateStore;

// ---------------------------------------------------------------------------
// Harnais inter-processus (même motif que mw_interprocess_phase_b.rs)
// ---------------------------------------------------------------------------

fn role() -> Option<String> {
    std::env::var("MWR_ROLE").ok()
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
        let stdout = process.stdout.take().expect("stdout de l'enfant");
        Self {
            process,
            lignes: std::io::BufReader::new(stdout).lines(),
        }
    }

    /// Lit stdout jusqu'à la ligne qui COMMENCE par `prefixe`, et rend le reste.
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
        assert!(statut.success(), "l'enfant a échoué : {statut:?}");
    }

    /// `SIGKILL` : aucun `Drop`, aucun flush, aucun msync.
    fn tuer(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

// ---------------------------------------------------------------------------
// SÉRIALISATION ENTRE FICHIERS DE TEST
// ---------------------------------------------------------------------------

/// Verrou inter-Processus qui SÉRIALISE les tests multi-écrivains.
///
/// ── POURQUOI IL EXISTE ─────────────────────────────────────────────────────
/// Ces tests relancent le binaire de test en ENFANT (`current_exe --exact <test>`).
/// `cargo test --workspace` exécute les BINAIRES de test en parallèle : deux enfants
/// simultanés de deux fichiers différents se disputent alors le même tube de
/// découverte (`--list`, qui tourne AVANT que `--exact` ne filtre) et le parent voit
/// « BrokenPipe — io error when listing tests ». Mesuré : six instances de ce même
/// fichier lancées ensemble échouent cinq fois ; c'est le harnais, pas le substrat.
///
/// ── POURQUOI UN FICHIER DE VERROU, ET PAS UNE VARIABLE ─────────────────────
/// Une variable d'environnement ne franchit pas la frontière du binaire voisin. Un
/// `flock` prendrait un descripteur et bloquerait indéfiniment si un test meurt en
/// le tenant ; ici le verrou est un FICHIER (`create_new`, atomique) qui porte le
/// PID du détenteur : un verrou d'un processus MORT est REPRIS, donc un `panic` ne
/// fige pas la suite.
struct Serie {
    chemin: std::path::PathBuf,
}

impl Serie {
    /// Prend le verrou (ou attend, jusqu'à 60 s).
    fn prendre(nom: &str) -> Self {
        let chemin = std::env::temp_dir().join(format!("obrain-mw-{nom}.lock"));
        let moi = std::process::id();
        let debut = std::time::Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&chemin)
            {
                Ok(mut f) => {
                    let _ = f.write_all(format!("{moi}").as_bytes());
                    return Self { chemin };
                }
                Err(_) => {
                    // Un détenteur MORT ne doit pas bloquer : `/proc/<pid>` est la
                    // seule source qui distingue « lent » de « tué ».
                    let vivant = std::fs::read_to_string(&chemin)
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
                    if !vivant {
                        let _ = std::fs::remove_file(&chemin);
                        continue;
                    }
                    assert!(
                        debut.elapsed() < Duration::from_secs(60),
                        "verrou {} tenu plus de 60 s : un test multi-écrivains est bloqué",
                        chemin.display()
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}

impl Drop for Serie {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.chemin);
    }
}

/// ── LE RÉPERTOIRE DU TEST, GARANTI NEUF ─────────────────────────────────────
///
/// `tempfile::tempdir()` a été MESURÉ en collision ici : deux binaires de test
/// lancés dans la même milliseconde ont reçu le MÊME `/tmp/.tmpXXXXXX`, donc deux
/// enfants ont écrit dans le même store — et tout ce qui suit mesure alors
/// n'importe quoi (`persist_dict` en `NotFound` parce que l'autre test venait de
/// supprimer l'arbre). Un répertoire bâti sur (pid, horloge monotone, compteur) ne
/// peut pas se répéter dans un même processus, et l'assertion qui suit refuse de
/// CONTINUER si un autre chemin a produit le même — un échec BRUYANT vaut mieux
/// qu'un faux verdict.
static COMPTEUR_REPERTOIRE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn repertoire_unique(nom: &str) -> PathBuf {
    let n = COMPTEUR_REPERTOIRE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("obrain-{nom}-{}-{t}-{n}", std::process::id()))
}

/// Crée le store : UN nœud (pour dimensionner les zones) puis flush.
fn creer_store(chemin: &Path) {
    assert!(
        !chemin.exists(),
        "le répertoire de test {} existe DÉJÀ : deux tests ont reçu le même chemin, \
         ce run ne mesurerait rien",
        chemin.display()
    );
    let s = SubstrateStore::create(chemin).expect("créer le store");
    let _ = s.create_node(&["MWR"]);
    s.flush().expect("flush initial");
}

// ---------------------------------------------------------------------------
// Enfants
// ---------------------------------------------------------------------------

/// Enfant « deux noms distincts, deux bits » : rend les bits que le registre lui
/// a donnés pour `a` et `b` — c'est la mesure directe de l'internement.
fn enfant_bits(dir: &Path, a: &str, b: &str, partir: &Path, drapeau: &Path) -> ! {
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write");
    let _ = std::fs::write(drapeau, b"pret");
    attendre_fichier(partir, "le drapeau de départ");
    // Nœuds portant UN SEUL label : le nom est interné, et les bitsets écrits.
    let ida = s.create_node(&[a]).0;
    let idb = s.create_node(&[b]).0;
    let la = s
        .get_node_labels(NodeId(ida))
        .map(|l| l.join("|"))
        .unwrap_or_default();
    let lb = s
        .get_node_labels(NodeId(idb))
        .map(|l| l.join("|"))
        .unwrap_or_default();
    assert_eq!(la, a, "le nœud {ida} doit porter {a:?}, il porte {la:?}");
    assert_eq!(lb, b, "le nœud {idb} doit porter {b:?}, il porte {lb:?}");
    s.flush().expect("flush de l'enfant");
    annoncer(&format!("BITS {a}={ida} {b}={idb}"));
    anoncer_fin();
}

fn anoncer_fin() -> ! {
    println!("FIN");
    std::io::stdout().flush().ok();
    std::process::exit(0);
}

// ===========================================================================
// R2 — LE MÊME NOM DONNE LE MÊME BIT, DANS LES DEUX PROCESSUS (idempotent)
// ===========================================================================
//
// L'autre moitié du contrat : le partage ne doit pas créer deux indices pour un
// même nom. Deux processus internent « Partagé » en même temps ; ni l'un ni
// l'autre ne doit obtenir un bit neuf, et le dictionnaire ne doit porter ce nom
// qu'UNE fois. C'est ce que le `compare_exchange` de la table garantit : le
// perdant de la course relit la case du gagnant et rend SON indice.
#[test]
fn r2_le_meme_nom_donne_le_meme_bit_dans_les_deux_processus() {
    // Le PARENT seul prend le verrou : un enfant est DÉJÀ dans un test qui le tient,
    // l'attendre serait un interblocage (et son `role()` suffit à l'identifier).
    let _serie = role().is_none().then(|| Serie::prendre("registres"));
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWR_DIR"));
        let a = env_obligatoire("MWR_A");
        let b = env_obligatoire("MWR_B");
        let partir = PathBuf::from(env_obligatoire("MWR_PARTIR"));
        let drapeau = PathBuf::from(env_obligatoire("MWR_DRAPEAU"));
        match r.as_str() {
            "bits" => enfant_bits(&dir, &a, &b, &partir, &drapeau),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let racine = repertoire_unique("r2");
    std::fs::create_dir_all(&racine).expect("créer le répertoire du test");
    let dir = racine.join("store-r2");
    creer_store(&dir);
    let partir = racine.join("partir");
    let (da, db) = (racine.join("a.pret"), racine.join("b.pret"));

    let mut a = Enfant::lancer(
        "r2_le_meme_nom_donne_le_meme_bit_dans_les_deux_processus",
        &[
            ("MWR_ROLE", "bits".into()),
            ("MWR_DIR", dir.display().to_string()),
            ("MWR_A", "Partage".into()),
            ("MWR_B", "AutreNom".into()),
            ("MWR_PARTIR", partir.display().to_string()),
            ("MWR_DRAPEAU", da.display().to_string()),
        ],
    );
    let mut b = Enfant::lancer(
        "r2_le_meme_nom_donne_le_meme_bit_dans_les_deux_processus",
        &[
            ("MWR_ROLE", "bits".into()),
            ("MWR_DIR", dir.display().to_string()),
            ("MWR_A", "Partage".into()),
            ("MWR_B", "EncoreUn".into()),
            ("MWR_PARTIR", partir.display().to_string()),
            ("MWR_DRAPEAU", db.display().to_string()),
        ],
    );
    attendre_fichier(&da, "l'enfant A");
    attendre_fichier(&db, "l'enfant B");
    std::fs::write(&partir, b"go").expect("départ");

    let la = a.attendre_ligne("BITS ");
    let lb = b.attendre_ligne("BITS ");
    a.terminer();
    b.terminer();

    let s = SubstrateStore::open(&dir).expect("réouverture");
    // Le nom partagé est le même des deux côtés ; les deux noms propres à chaque
    // processus doivent être distincts de lui ET entre eux.
    let mut vus: Vec<(String, String)> = Vec::new();
    for (ligne, quoi) in [(&la, "A"), (&lb, "B")] {
        let (premier, second) = ligne.split_once(' ').expect("a=<id> b=<id>");
        let ida: u64 = premier.split_once('=').expect("a=<id>").1.parse().unwrap();
        let idb: u64 = second.split_once('=').expect("b=<id>").1.parse().unwrap();
        let na = s.get_node_labels(NodeId(ida)).unwrap_or_default().join("|");
        let nb = s.get_node_labels(NodeId(idb)).unwrap_or_default().join("|");
        assert_eq!(
            na, "Partage",
            "processus {quoi} : le nœud partagé porte {na:?}"
        );
        vus.push((quoi.to_string(), nb));
    }
    let propres: Vec<String> = vus.iter().map(|(_, n)| n.clone()).collect();
    assert_eq!(
        propres.len(),
        2,
        "les deux processus doivent avoir lu leur nom propre"
    );
    assert_ne!(
        propres[0], propres[1],
        "les deux processus ont lu le MÊME nom pour deux noms différents ({:?}) — \
         c'est la collision d'indices que la table partagée ferme",
        propres
    );

    // Et le dictionnaire ne porte « Partage » qu'UNE fois (idempotence) : deux
    // indices pour un même nom seraient une autre forme de divergence.
    let dict = obrain_substrate::dict::DictSnapshot::load(&dir.join("substrate.dict"))
        .expect("lire le dictionnaire persisté");
    let occurrences = dict.labels.iter().filter(|l| *l == "Partage").count();
    assert_eq!(
        occurrences, 1,
        "« Partage » apparaît {occurrences} fois dans {:?} — le même nom interné \
         deux fois donne deux bits, donc deux libellés pour un seul nom",
        dict.labels
    );
}
