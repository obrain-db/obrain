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

/// Enfant « intern puis meurt en SIGKILL » : aucun flush, aucun dictionnaire.
fn enfant_intern_puis_meurt(dir: &Path, nom: &str, n: u64) -> ! {
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write");
    let mut ids = Vec::new();
    for _ in 0..n {
        ids.push(s.create_node(&[nom]).0);
    }
    let liste = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    // Le nom interné est DANS le mmap partagé, pas seulement dans la mémoire du
    // processus : c'est ce que le parent va vérifier après le SIGKILL.
    annoncer(&format!("IDS {liste}"));
    annoncer("PRET");
    loop {
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ===========================================================================
// R3 — L'INDICE SURVIT À UN `kill -9` (et ne se redistribue pas)
// ===========================================================================
//
// ── LE SCÉNARIO RÉEL ────────────────────────────────────────────────────────
// Un écrivain P est ouvert DEPUIS LONGTEMPS. Un second arrive, interne « Volatile »
// et alloue 40 nœuds, puis meurt en `SIGKILL` : aucun dictionnaire, aucun flush.
// P, qui n'a rien vu, interne ensuite « Volatile » : il doit retrouver LE MÊME
// indice que le mort — sinon le nom du mort et celui de P se partagent le bit, et
// le WAL (qui porte `LabelIntern{label_id, name}`) désigne deux choses pour un bit.
//
// ── POURQUOI CETTE PROPRIÉTÉ EST CELLE DE LA TABLE, ET PAS DU WAL ───────────
// Le WAL est par processus dans son usage (un `WalWriter` par handle) : il ne dit
// rien à P tant que P ne relit pas le journal, et P ne le relit qu'à l'ouverture.
// La table, elle, est dans un fichier mmapé : l'écriture du mort y est visible
// IMMÉDIATEMENT, sans WAL, sans flush, sans réouverture. C'est exactement le
// contrat de `BorneAllocation` (pièce 2) pour les slots, appliqué aux noms.
#[test]
fn r3_l_indice_survit_a_un_kill_9() {
    // Le PARENT seul prend le verrou : un enfant est DÉJÀ dans un test qui le tient,
    // l'attendre serait un interblocage (et son `role()` suffit à l'identifier).
    let _serie = role().is_none().then(|| Serie::prendre("registres"));
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWR_DIR"));
        let nom = env_obligatoire("MWR_NOM");
        let n: u64 = env_obligatoire("MWR_N").parse().unwrap();
        match r.as_str() {
            "meurt" => enfant_intern_puis_meurt(&dir, &nom, n),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let racine = repertoire_unique("r3");
    std::fs::create_dir_all(&racine).expect("créer le répertoire du test");
    let dir = racine.join("store-r3");
    creer_store(&dir);

    // L'écrivain LONGÉVIF : ouvert AVANT le mort, et qui ne rouvrira pas. Il ne
    // relit NI le WAL, NI le dictionnaire : tout ce qu'il saura du mort viendra du
    // mmap partagé, et de lui seul.
    let p = SubstrateStore::open_shared_write(&dir).expect("écrivain longévif");
    let mut enfant = Enfant::lancer(
        "r3_l_indice_survit_a_un_kill_9",
        &[
            ("MWR_ROLE", "meurt".into()),
            ("MWR_DIR", dir.display().to_string()),
            ("MWR_NOM", "Volatile".into()),
            ("MWR_N", "40".into()),
        ],
    );
    let ids_morts: Vec<u64> = enfant
        .attendre_ligne("IDS ")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    enfant.attendre_ligne("PRET");
    enfant.tuer(); // SIGKILL : ni Drop, ni flush, ni msync.
    assert_eq!(ids_morts.len(), 40);

    // ── CE QUE LE DISQUE PORTE APRÈS LE CRASH ─────────────────────────────
    // Le dictionnaire est celui d'AVANT (le mort n'a rien flushé) et le WAL porte
    // ses nœuds. « Volatile » n'existe donc NULLE PART sur le disque… sauf dans la
    // table mmapée, que ce processus-ci ne relira jamais.
    let dict_apres_crash = obrain_substrate::dict::DictSnapshot::load(&dir.join("substrate.dict"))
        .expect("lire le dictionnaire");
    assert!(
        !dict_apres_crash.labels.iter().any(|l| l == "Volatile"),
        "le mort ne doit RIEN avoir persisté : {:?} — le test ne mesurerait plus \
         « la table seule », mais le dictionnaire",
        dict_apres_crash.labels
    );

    // ── LA QUESTION QUI TRANCHE : LE BIT DU MORT EST-IL REDISTRIBUÉ ? ───────
    // « MWR » occupe le bit 0 (dictionnaire initial). Le mort a consommé le bit 1
    // pour « Volatile ». Un écrivain qui n'a rien vu doit donc prendre un bit AUDESSUS
    // de 1 pour un nom NEUF : si la table n'avait pas survécu, son compteur repartirait
    // du dictionnaire (1) et « Ecrivain » recevrait le bit 1 — celui du mort.
    let id_neuf = p.create_node(&["Ecrivain"]).0;
    let l_neuf = p
        .get_node_labels(NodeId(id_neuf))
        .unwrap_or_default()
        .join("|");
    assert_eq!(
        l_neuf, "Ecrivain",
        "« Ecrivain » doit être lisible après le crash, il rend {l_neuf:?}"
    );
    // Le nœud du mort, lui, doit porter « Volatile » — dans CE processus qui n'a
    // jamais vu son nom autrement que par la table.
    let l_mort = p
        .get_node_labels(NodeId(ids_morts[0]))
        .unwrap_or_default()
        .join("|");

    assert_eq!(
        l_mort, "Volatile",
        "le nœud du processus tué rend {l_mort:?} au lieu de « Volatile » : l'indice \
         que le mort avait interné n'a pas été retrouvé dans la table — un autre nom \
         a été écrit dessus, ou son bit a été redistribué"
    );
    // Et l'écrivain longévif retrouve, pour le nom du mort, EXACTEMENT le même bit
    // que celui avec lequel il vient de lire son nœud.
    assert_eq!(
        p.get_node_labels(NodeId(id_neuf)).map(|l| l[0].to_string()),
        Some("Ecrivain".to_string()),
        "relecture stable attendue"
    );

    // ── L'ÉCRIVAIN LONGÉVIF FLUSHE ────────────────────────────────────────
    // C'est CE flush qui doit NOMMER « Volatile » sur le disque, alors que ce
    // processus ne l'a jamais interné lui-même : le nom vient de la TABLE, et il
    // est entré dans le registre local au moment où la lecture du nœud du mort l'a
    // demandé. Sans cette résolution, le dictionnaire écrit ici aurait un TROU au
    // bit 1 — le nom du mort serait perdu à la prochaine ouverture, alors que ses
    // nœuds, eux, restent vivants dans la zone.
    p.flush().expect("flush de l'écrivain longévif");
    drop(p);
    // ── APRÈS RÉOUVERTURE : LES DEUX NOMS COEXISTENT ──────────────────────
    // Le dictionnaire final doit porter « MWR », « Ecrivain » ET « Volatile » (le
    // dernier arrive par le WAL, que la réouverture rejoue) : deux noms au même
    // indice est exactement ce que la pièce interdit.
    let s = SubstrateStore::open(&dir).expect("réouverture");
    let dict = obrain_substrate::dict::DictSnapshot::load(&dir.join("substrate.dict"))
        .expect("lire le dictionnaire final");
    let mut noms: Vec<&str> = dict.labels.iter().map(String::as_str).collect();
    noms.retain(|n| !n.is_empty());
    noms.sort_unstable();
    assert_eq!(
        noms,
        vec!["Ecrivain", "MWR", "Volatile"],
        "le dictionnaire final doit nommer les trois noms, chacun à son indice : {:?}",
        dict.labels
    );
    assert_eq!(
        dict.labels.len(),
        3,
        "trois noms distincts doivent occuper trois indices DISTINCTS : {:?} — un \
         indice partagé par deux noms est la collision que la table partagée ferme",
        dict.labels
    );
    // Le nom du mort est encore lisible, ET au bit que le dictionnaire lui donne.
    let id3 = s.create_node(&["Volatile"]).0;
    let l3 = s.get_node_labels(NodeId(id3)).unwrap_or_default().join("|");
    assert_eq!(
        l3, "Volatile",
        "après réouverture, « Volatile » rend {l3:?} — la table partagée n'a pas été \
         republiée depuis le dictionnaire + le WAL"
    );
    for id in &ids_morts {
        assert_eq!(
            s.get_node_labels(NodeId(*id))
                .map(|l| l.join("|"))
                .unwrap_or_default(),
            "Volatile",
            "le nœud {id} du processus tué ne porte plus « Volatile » après \
             réouverture : le WAL n'a pas reconstruit son nom"
        );
    }
}
