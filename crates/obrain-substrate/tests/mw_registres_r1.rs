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

/// Relit un couple `id:bitset` annoncé par un enfant.
fn couples(brut: &str) -> Vec<(u64, u64)> {
    brut.split(',')
        .filter(|s| !s.is_empty())
        .map(|c| {
            let (id, bits) = c.split_once(':').expect("id:bitset");
            (
                id.parse().expect("id entier"),
                bits.parse().expect("bitset entier"),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Enfants
// ---------------------------------------------------------------------------

/// Enfant « interne une étiquette et écrit des nœuds qui la portent ».
///
/// Il attend le drapeau `partir` AVANT d'interner : sans cette barrière, le
/// premier enfant aurait fini d'interner (et flushé) avant que le second ait
/// commencé, et le scénario « deux processus internent EN MÊME TEMPS » — le seul
/// qui produit la divergence — n'aurait pas lieu.
fn enfant_labels(dir: &Path, nom: &str, n: u64, partir: &Path, drapeau: &Path) -> ! {
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write");
    let _ = std::fs::write(drapeau, b"pret");
    attendre_fichier(partir, "le drapeau de départ");
    let mut annonces: Vec<String> = Vec::new();
    // Plusieurs tours : le bit est interné une fois, mais chaque `create_node`
    // repasse par le registre — c'est le chemin qu'un écrivain réel emprunte.
    for i in 0..n {
        let id = s.create_node(&[nom]).0;
        let bits = s
            .get_node_labels(NodeId(id))
            .map(|l| l.join("|"))
            .unwrap_or_default();
        assert_eq!(bits, nom, "le nœud {id} doit porter {nom:?}");
        annonces.push(format!("{id}:{i}"));
    }
    // Le flush PERSISTE le dictionnaire de CE processus : c'est par là que le nom de
    // l'autre écrivain était effacé (un dictionnaire écrasait l'autre).
    s.flush().expect("flush de l'enfant");
    annoncer(&format!("NOMS {nom} {}", annonces.join(",")));
    anoncer_fin();
}

/// Termine l'enfant proprement : stdout vidé, code 0.
fn anoncer_fin() -> ! {
    println!("FIN");
    std::io::stdout().flush().ok();
    std::process::exit(0);
}

// ===========================================================================
// R1 — DEUX PROCESSUS QUI INTERNENT DES NOMS DIFFÉRENTS OBTIENNENT DES BITS
//      DIFFÉRENTS (et le disque dit la même chose)
// ===========================================================================
//
// ── LE DÉFAUT MESURÉ ────────────────────────────────────────────────────────
// Chaque processus a SON `LabelRegistry` : le bit vient de `bit_to_name.len()`
// (`store.rs:271`). Les deux partent d'un registre vide (le dictionnaire initial
// n'a aucun label), donc les deux donnent le bit 0 — à deux noms DIFFÉRENTS.
// Leurs `label_bitset` désignent alors deux libellés, et le dictionnaire persisté
// par l'un des deux ne peut pas nommer l'autre (le `persist_dict` détecte la
// divergence — « REGISTRES DIVERGENTS » — et garde la version du disque : le nom
// de l'autre processus n'est plus nommé par personne).
//
// ── CE QUE LE TEST EXIGE, ET POURQUOI LE DISQUE EST LA SEULE PREUVE ─────────
// Exiger la disjonction des bitsets suffit à faire tomber le défaut, mais un
// bitset n'est qu'un entier : ce qui compte est ce qu'il DÉSIGNE. La vérification
// finale rouvre donc le store et demande `get_node_labels` pour chaque nœud des
// deux écrivains : les deux noms doivent être LISIBLES, et chaque nœud doit porter
// EXACTEMENT celui que son écrivain a demandé.
#[test]
fn r1_deux_processus_internant_des_noms_differents_obtiennent_des_bits_differents() {
    // Le PARENT seul prend le verrou : un enfant est DÉJÀ dans un test qui le tient,
    // l'attendre serait un interblocage (et son `role()` suffit à l'identifier).
    let _serie = role().is_none().then(|| Serie::prendre("registres"));
    if let Some(r) = role() {
        let dir = PathBuf::from(env_obligatoire("MWR_DIR"));
        let nom = env_obligatoire("MWR_NOM");
        let n: u64 = env_obligatoire("MWR_N").parse().unwrap();
        let partir = PathBuf::from(env_obligatoire("MWR_PARTIR"));
        let drapeau = PathBuf::from(env_obligatoire("MWR_DRAPEAU"));
        match r.as_str() {
            "labels" => enfant_labels(&dir, &nom, n, &partir, &drapeau),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let racine = repertoire_unique("r1");
    std::fs::create_dir_all(&racine).expect("créer le répertoire du test");
    let dir = racine.join("store-r1");
    creer_store(&dir);
    let partir = racine.join("partir");
    let (da, db) = (racine.join("a.pret"), racine.join("b.pret"));

    let mut a = Enfant::lancer(
        "r1_deux_processus_internant_des_noms_differents_obtiennent_des_bits_differents",
        &[
            ("MWR_ROLE", "labels".into()),
            ("MWR_DIR", dir.display().to_string()),
            ("MWR_NOM", "Alpha".into()),
            ("MWR_N", "6".into()),
            ("MWR_PARTIR", partir.display().to_string()),
            ("MWR_DRAPEAU", da.display().to_string()),
        ],
    );
    let mut b = Enfant::lancer(
        "r1_deux_processus_internant_des_noms_differents_obtiennent_des_bits_differents",
        &[
            ("MWR_ROLE", "labels".into()),
            ("MWR_DIR", dir.display().to_string()),
            ("MWR_NOM", "Beta".into()),
            ("MWR_N", "6".into()),
            ("MWR_PARTIR", partir.display().to_string()),
            ("MWR_DRAPEAU", db.display().to_string()),
        ],
    );
    // Les deux enfants sont ouverts et prêts : on les lâche ENSEMBLE.
    attendre_fichier(&da, "l'enfant A");
    attendre_fichier(&db, "l'enfant B");
    std::fs::write(&partir, b"go").expect("départ");

    let noms_a = a.attendre_ligne("NOMS ");
    let noms_b = b.attendre_ligne("NOMS ");
    a.terminer();
    b.terminer();

    let ids_a = couples(noms_a.split_once(' ').expect("NOMS <nom> <liste>").1);
    let ids_b = couples(noms_b.split_once(' ').expect("NOMS <nom> <liste>").1);
    assert_eq!(ids_a.len(), 6);
    assert_eq!(ids_b.len(), 6);

    // ── LES DEUX PROCESSUS ONT-ILS INTERNÉ AU MÊME ENDROIT ? ────────────────
    // `create_node` a rendu les bitsets ; on les relit des nœuds eux-mêmes.
    let s = SubstrateStore::open(&dir).expect("réouverture");
    let mut bits_a = std::collections::BTreeSet::new();
    let mut bits_b = std::collections::BTreeSet::new();
    for (id, _) in &ids_a {
        let l = s.get_node_labels(NodeId(*id)).unwrap_or_default();
        assert_eq!(
            l.len(),
            1,
            "le nœud {id} de A doit porter UNE étiquette, il en porte {} ({l:?})",
            l.len()
        );
        bits_a.insert(l[0].to_string());
    }
    for (id, _) in &ids_b {
        let l = s.get_node_labels(NodeId(*id)).unwrap_or_default();
        assert_eq!(
            l.len(),
            1,
            "le nœud {id} de B doit porter UNE étiquette, il en porte {} ({l:?})",
            l.len()
        );
        bits_b.insert(l[0].to_string());
    }
    assert_eq!(
        bits_a,
        ["Alpha".to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "les nœuds du processus A doivent porter « Alpha » — ils portent {bits_a:?} : \
         le dictionnaire reconstruit après réouverture ne nomme plus ce que A a écrit"
    );
    assert_eq!(
        bits_b,
        ["Beta".to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "les nœuds du processus B doivent porter « Beta » — ils portent {bits_b:?}"
    );

    // ── ET LE DICTIONNAIRE PORTE BIEN LES DEUX NOMS ─────────────────────────
    // C'est la formulation qui tombe AVANT la pièce : l'un des deux noms manque,
    // parce que les deux processus l'ont interné au MÊME bit et qu'un dictionnaire
    // ne peut nommer qu'un nom par bit.
    let dict = obrain_substrate::dict::DictSnapshot::load(&dir.join("substrate.dict"))
        .expect("lire le dictionnaire persisté");
    assert!(
        dict.labels.iter().any(|l| l == "Alpha"),
        "« Alpha » a DISPARU du dictionnaire persisté : {:?} — deux processus ont \
         interné des noms différents au même bit, et le dictionnaire n'en garde qu'un",
        dict.labels
    );
    assert!(
        dict.labels.iter().any(|l| l == "Beta"),
        "« Beta » a DISPARU du dictionnaire persisté : {:?}",
        dict.labels
    );
    assert!(
        dict.labels.iter().any(|l| l == "MWR"),
        "le label initial « MWR » doit survivre : {:?}",
        dict.labels
    );
}
