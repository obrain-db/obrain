//! Désignation **explicite** des bases réelles pour les bancs et les audits.
//!
//! # Pourquoi ce crate existe
//!
//! Le 2026-11-11, un simple `cargo test -p obrain-engine --features cypher` a
//! ouvert **en lecture-écriture** les bases de travail d'un développeur sous
//! `~/.obrain/db/` (`po`, `wikipedia`, `megalaw`) : une trentaine de cibles de
//! test devinaient ce chemin à partir de `$HOME` et ne se sautaient que si le
//! dossier était absent. L'une d'elles y a créé puis supprimé des nœuds et
//! posé des propriétés `_bench_*` ; une autre a été tuée en pleine ouverture.
//!
//! # La règle
//!
//! * **Par défaut, aucune cible n'ouvre de base.** Elle se saute en l'écrivant
//!   sur la sortie d'erreur. Aucun chemin n'est jamais déduit de `$HOME`.
//! * Une cible ne s'active que si une variable d'environnement **désigne** la
//!   base : `OBRAIN_BENCH_DB_PO`, `OBRAIN_BENCH_DB_WIKIPEDIA`,
//!   `OBRAIN_BENCH_DB_MEGALAW` (voir [`Base::variable`]). Le chemin doit être
//!   absolu et contenir `substrate.meta` ; sinon la cible échoue avant toute
//!   ouverture.
//! * **La base désignée n'est jamais ouverte par le moteur — pas même en
//!   lecture seule.** Elle est copiée (clonée quand le système de fichiers le
//!   permet), fichier par fichier, et c'est la copie qu'on ouvre.
//!
//! # Pourquoi même les lecteurs copient
//!
//! « Lecture seule » ne veut pas dire, aujourd'hui, « n'écrit pas sur le
//! disque ». `SubstrateFile::open_read_only` passe par le même `open_inner`
//! que l'écriture partagée, et le drapeau `read_only` du store n'est posé
//! qu'APRÈS sa construction. Une ouverture en lecture seule peut donc rejouer
//! le WAL dans les zones, reconstruire des colonnes, vider des temporaires,
//! créer les fichiers absents et republier `substrate.alloc` ; sur un dossier
//! sans `substrate.meta`, `ObrainDB::open_read_only` CRÉE une base. Ce que le
//! mode garantit réellement : la session refuse les mutations demandées par
//! l'appelant. C'est un défaut du moteur, reproduit par
//! `crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs`
//! (`defaut_moteur_…`, sous `--cfg defauts_connus`). Tant qu'il n'est pas
//! corrigé, la seule protection réelle d'une base est de ne pas l'ouvrir.
//!
//! # Les deux issues
//!
//! * [`base_lecture`] rend une [`BaseLecture`] **opaque** : ses seules issues
//!   sont [`BaseLecture::ouvrir`] et [`BaseLecture::ouvrir_substrat`], qui
//!   ouvrent en lecture seule une copie partagée par tout le processus (une
//!   copie par base et par binaire de test, pas une par test).
//! * [`base_copie`] rend une [`CopieBase`] privée à la cible, qui peut y
//!   écrire : son chemin est exposé parce que ce n'est jamais celui de la base
//!   désignée.
//!
//! Mode d'emploi complet : `docs/BENCHMARKS.md`, section « Bancs sur bases
//! réelles ».

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use obrain_engine::ObrainDB;
use obrain_substrate::SubstrateStore;

/// Variable qui, posée à `1`, conserve la copie de travail de [`base_copie`]
/// après la cible (son chemin est imprimé) au lieu de la supprimer.
pub const VARIABLE_GARDER_COPIE: &str = "OBRAIN_BENCH_GARDER_COPIE";

/// Fichier dont la présence fait d'un répertoire une base Obrain.
const FICHIER_META: &str = "substrate.meta";

/// Préfixe des répertoires de copie sous `TMPDIR`. Suivi du PID du processus
/// propriétaire, pour qu'un lancement ultérieur reconnaisse et supprime les
/// résidus d'un processus tué (Ctrl-C, `kill -9`).
const PREFIXE_COPIE: &str = "obrain-banc-";
/// Préfixe des copies conservées à la demande : jamais balayées.
const PREFIXE_COPIE_GARDEE: &str = "obrain-banc-gardee-";

/// Les bases réelles que les bancs et audits du dépôt savent exploiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    /// Le graphe de code de Project Orchestrator.
    Po,
    /// Le corpus Wikipédia.
    Wikipedia,
    /// Le corpus juridique `MegaLaw`.
    Megalaw,
}

impl Base {
    /// Toutes les bases connues, dans l'ordre habituel des bancs.
    pub const TOUTES: [Self; 3] = [Self::Po, Self::Wikipedia, Self::Megalaw];

    /// Nom court de la base (`po`, `wikipedia`, `megalaw`).
    #[must_use]
    pub const fn nom(self) -> &'static str {
        match self {
            Self::Po => "po",
            Self::Wikipedia => "wikipedia",
            Self::Megalaw => "megalaw",
        }
    }

    /// Variable d'environnement qui désigne cette base. C'est le **seul**
    /// moyen d'activer une cible : aucun chemin par défaut n'existe.
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::Po => "OBRAIN_BENCH_DB_PO",
            Self::Wikipedia => "OBRAIN_BENCH_DB_WIKIPEDIA",
            Self::Megalaw => "OBRAIN_BENCH_DB_MEGALAW",
        }
    }
}

/// Vérifie qu'un chemin désigné est celui d'une base Obrain existante, sans
/// rien ouvrir.
///
/// # Panics
///
/// * chemin relatif : il serait résolu contre le répertoire courant du
///   binaire de test (le dossier du crate), pas contre celui de l'appelant ;
/// * pas de `substrate.meta` : ce n'est pas une base. Le moteur en CRÉERAIT
///   une à cet endroit, même ouvert en lecture seule — désigner par erreur le
///   dossier parent sèmerait des `substrate.*` dans une arborescence réelle.
fn exiger_base(chemin: &Path, origine: &str) {
    assert!(
        chemin.is_absolute(),
        "{origine} désigne « {} » : un chemin ABSOLU est exigé (un chemin relatif serait résolu \
         contre le dossier du crate testé, pas contre le vôtre)",
        chemin.display()
    );
    assert!(
        chemin.join(FICHIER_META).is_file(),
        "{origine} désigne « {} », qui n'est pas une base Obrain : pas de {FICHIER_META} dans ce \
         répertoire. Rien n'est ouvert.",
        chemin.display()
    );
}

/// Résout la désignation de `base` à partir d'un environnement **fourni**
/// (`env` rend la valeur d'une variable). Fonction pure : c'est elle que les
/// tests exercent, sans toucher à l'environnement du processus.
///
/// Rend `None` quand la variable est absente ou vide — jamais un chemin
/// deviné, quel que soit `HOME`.
///
/// # Panics
///
/// Si la variable désigne autre chose qu'une base : chemin relatif, ou
/// répertoire sans `substrate.meta`. Une désignation explicite erronée est une
/// erreur de l'appelant, pas un saut silencieux.
pub fn resoudre_avec(base: Base, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let valeur = env(base.variable()).filter(|v| !v.is_empty())?;
    let chemin = PathBuf::from(valeur);
    exiger_base(&chemin, base.variable());
    Some(chemin)
}

/// Nom de la cible en cours, pour les annonces : libtest nomme le fil
/// d'exécution d'après le test.
fn cible() -> String {
    std::thread::current().name().unwrap_or("?").to_owned()
}

/// Résout `base` dans l'environnement du processus et **annonce** le saut
/// quand elle n'est pas désignée.
fn resoudre_ou_annoncer(base: Base) -> Option<PathBuf> {
    let chemin = resoudre_avec(base, |cle| std::env::var_os(cle));
    if chemin.is_none() {
        eprintln!(
            "SAUT [{}] : {} n'est pas définie — la base réelle « {} » n'est PAS ouverte \
             (aucun chemin n'est deviné sous $HOME ; voir docs/BENCHMARKS.md, \
             « Bancs sur bases réelles »)",
            cible(),
            base.variable(),
            base.nom()
        );
    }
    chemin
}

// ───────────────────────────── Lecteurs ─────────────────────────────────────

/// Base désignée, pour une cible qui ne fait que **lire**.
///
/// Type **opaque** : il ne rend aucun chemin. Ses seules issues sont
/// [`ouvrir`](Self::ouvrir) et [`ouvrir_substrat`](Self::ouvrir_substrat),
/// qui ouvrent **en lecture seule** une **copie** de la base — jamais la base
/// désignée. Une cible ne peut donc ni ouvrir l'original, ni l'ouvrir en
/// écriture, même par inadvertance.
#[derive(Debug, Clone)]
pub struct BaseLecture {
    base: Base,
    copie: PathBuf,
}

impl BaseLecture {
    /// Ouvre la copie en lecture seule avec le moteur.
    ///
    /// # Panics
    ///
    /// Si l'ouverture échoue.
    #[must_use]
    pub fn ouvrir(&self) -> ObrainDB {
        ObrainDB::open_read_only(&self.copie).unwrap_or_else(|e| {
            panic!(
                "ouverture en lecture seule de la copie de « {} » ({}) : {e}",
                self.base.nom(),
                self.copie.display()
            )
        })
    }

    /// Ouvre la copie en lecture seule au niveau du substrat.
    ///
    /// # Panics
    ///
    /// Si l'ouverture échoue.
    #[must_use]
    pub fn ouvrir_substrat(&self) -> SubstrateStore {
        SubstrateStore::open_read_only(&self.copie).unwrap_or_else(|e| {
            panic!(
                "ouverture en lecture seule (substrat) de la copie de « {} » ({}) : {e}",
                self.base.nom(),
                self.copie.display()
            )
        })
    }
}

/// Copies de lecture du processus : une par base, prise à la première
/// demande, partagée par tous les tests du binaire, supprimée à la sortie.
fn copies_de_lecture() -> &'static Mutex<HashMap<Base, PathBuf>> {
    static COPIES: OnceLock<Mutex<HashMap<Base, PathBuf>>> = OnceLock::new();
    COPIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Base désignée pour une cible qui ne fait que **lire** — ou `None`, après
/// l'avoir annoncé sur la sortie d'erreur, quand la variable de la base n'est
/// pas définie : la cible se saute.
///
/// La base désignée est copiée **une fois par processus** (donc une fois par
/// binaire de test, quel que soit le nombre de tests qui la demandent) ; la
/// copie est supprimée à la sortie du processus.
///
/// # Panics
///
/// Si la variable désigne autre chose qu'une base, ou si la copie échoue.
#[must_use]
pub fn base_lecture(base: Base) -> Option<BaseLecture> {
    let source = resoudre_ou_annoncer(base)?;
    // Le verrou est tenu pendant la copie : deux tests du même binaire qui
    // demandent la même base attendent la même copie au lieu d'en prendre deux.
    let mut copies = copies_de_lecture()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let copie = copies.entry(base).or_insert_with(|| {
        let (racine, chemin) = copier_base(&source, base.nom(), false);
        nettoyage::a_la_sortie(racine);
        eprintln!(
            "BANC [{}] : base « {} » = COPIE {} de {} (ouverte en lecture seule, partagée par \
             ce binaire, supprimée à sa sortie)",
            cible(),
            base.nom(),
            chemin.display(),
            source.display()
        );
        chemin
    });
    Some(BaseLecture {
        base,
        copie: copie.clone(),
    })
}

// ───────────────────────────── Écrivains ────────────────────────────────────

/// Copie de travail **privée** d'une base désignée : tout ce qu'une cible y
/// écrit reste dans un répertoire temporaire, supprimé quand la valeur est
/// lâchée (sauf [`VARIABLE_GARDER_COPIE`]).
#[derive(Debug)]
pub struct CopieBase {
    chemin: PathBuf,
    racine: Option<PathBuf>,
}

impl CopieBase {
    /// Chemin de la copie. Exposé parce que ce n'est jamais celui de la base
    /// désignée : la cible peut l'ouvrir en écriture.
    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }
}

impl AsRef<Path> for CopieBase {
    fn as_ref(&self) -> &Path {
        &self.chemin
    }
}

impl Drop for CopieBase {
    fn drop(&mut self) {
        if let Some(racine) = self.racine.take() {
            let _ = fs::remove_dir_all(&racine);
            nettoyage::oublier(&racine);
        }
    }
}

/// Copie privée de la base désignée, pour une cible qui **écrit** (ou qui
/// mesure l'ouverture en écriture). La base désignée n'est que lue, fichier
/// par fichier ; elle n'est jamais ouverte par le moteur.
///
/// Rend `None`, après l'avoir annoncé, quand la variable n'est pas définie.
/// La copie n'est cohérente que si rien n'écrit dans la base pendant qu'elle
/// est prise : désigner un instantané, ou arrêter le service qui la tient.
///
/// # Panics
///
/// Si la variable désigne autre chose qu'une base, ou si la copie échoue
/// (espace disque, droits).
#[must_use]
pub fn base_copie(base: Base) -> Option<CopieBase> {
    let source = resoudre_ou_annoncer(base)?;
    Some(copie_privee(&source, base.nom()))
}

/// Copie privée d'une base désignée **par son chemin** — pour les cibles qui
/// ont leur propre variable (`OBRAIN_TEST_STORE_REEL_DIR`…) et qui écrivent.
///
/// # Panics
///
/// Si `source` n'est pas un chemin absolu vers une base (`substrate.meta`),
/// ou si la copie échoue.
#[must_use]
pub fn copie_de(source: &Path, origine: &str) -> CopieBase {
    exiger_base(source, origine);
    copie_privee(source, "base")
}

fn copie_privee(source: &Path, nom: &str) -> CopieBase {
    let garder = std::env::var_os(VARIABLE_GARDER_COPIE).is_some_and(|v| v == "1");
    let (racine, chemin) = copier_base(source, nom, garder);
    eprintln!(
        "BANC [{}] : base « {nom} » = COPIE privée {} de {} ({})",
        cible(),
        chemin.display(),
        source.display(),
        if garder {
            "conservée après la cible : à supprimer à la main"
        } else {
            "supprimée après la cible"
        }
    );
    let racine = if garder {
        None
    } else {
        // Filet si la cible sort sans lâcher la valeur (`process::exit`).
        nettoyage::a_la_sortie(racine.clone());
        Some(racine)
    };
    CopieBase { chemin, racine }
}

/// Copie `source` dans un nouveau répertoire de `TMPDIR`. Rend
/// `(racine à supprimer, chemin de la base copiée)`.
fn copier_base(source: &Path, nom: &str, garder: bool) -> (PathBuf, PathBuf) {
    nettoyage::balayer_les_residus();
    let prefixe = if garder {
        format!("{PREFIXE_COPIE_GARDEE}{nom}-")
    } else {
        format!("{PREFIXE_COPIE}{}-{nom}-", std::process::id())
    };
    // `keep` : la durée de vie est gérée ici (lâcher, sortie du processus,
    // balayage des résidus), pas par le destructeur de `TempDir`.
    let racine = tempfile::Builder::new()
        .prefix(&prefixe)
        .tempdir()
        .expect("création du répertoire temporaire de la copie")
        .keep();
    let chemin = racine.join(nom);
    let debut = std::time::Instant::now();
    let octets = copier_arborescence(source, &chemin).unwrap_or_else(|e| {
        let _ = fs::remove_dir_all(&racine);
        panic!(
            "copie de {} vers {} : {e}",
            source.display(),
            chemin.display()
        )
    });
    eprintln!(
        "BANC [{}] : copie de {} — {octets} octets en {:.2} s",
        cible(),
        source.display(),
        debut.elapsed().as_secs_f64()
    );
    (racine, chemin)
}

/// Copie récursive de `source` vers `destination` (créée) ; rend le nombre
/// d'octets logiques copiés. Les liens symboliques sont suivis : la copie ne
/// pointe jamais vers l'original.
///
/// `std::fs::copy` clone le fichier quand le système de fichiers le permet
/// (`fclonefileat` sur APFS ; `copy_file_range`, qui partage les blocs sur
/// Btrfs et XFS avec reflink) : la copie est alors quasi instantanée et ne
/// consomme de disque que pour les blocs réécrits ensuite. Ailleurs — et dès
/// que `TMPDIR` n'est pas sur le même volume que la base — c'est une copie
/// pleine.
///
/// # Errors
///
/// Toute erreur d'entrée-sortie rencontrée pendant la copie.
pub fn copier_arborescence(source: &Path, destination: &Path) -> io::Result<u64> {
    fs::create_dir_all(destination)?;
    let mut octets = 0;
    for entree in fs::read_dir(source)? {
        let entree = entree?;
        let depuis = entree.path();
        let vers = destination.join(entree.file_name());
        if fs::metadata(&depuis)?.is_dir() {
            octets += copier_arborescence(&depuis, &vers)?;
        } else {
            octets += fs::copy(&depuis, &vers)?;
        }
    }
    Ok(octets)
}

// ───────────────────────────── Nettoyage ────────────────────────────────────

/// Suppression des copies : à la sortie normale du processus (`atexit`), et,
/// pour les résidus d'un processus tué (Ctrl-C, `kill -9`, panne), au
/// lancement suivant — le nom du répertoire porte le PID de son propriétaire.
mod nettoyage {
    use super::{PREFIXE_COPIE, PREFIXE_COPIE_GARDEE};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, Once, OnceLock};

    fn registre() -> &'static Mutex<Vec<PathBuf>> {
        static REGISTRE: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
        REGISTRE.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Inscrit `racine` pour suppression à la sortie du processus.
    pub(super) fn a_la_sortie(racine: PathBuf) {
        registre()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(racine);
        armer();
    }

    /// Retire `racine` du registre (elle vient d'être supprimée).
    pub(super) fn oublier(racine: &Path) {
        registre()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|r| r != racine);
    }

    #[cfg(unix)]
    extern "C" fn supprimer_tout() {
        // `try_lock` : on est dans un gestionnaire de sortie, on ne bloque pas.
        if let Ok(racines) = registre().try_lock() {
            for racine in racines.iter() {
                let _ = std::fs::remove_dir_all(racine);
            }
        }
    }

    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn armer() {
        static ARME: Once = Once::new();
        ARME.call_once(|| {
            // SAFETY: `supprimer_tout` est une fonction `extern "C"` sans
            // argument, valide pour toute la vie du processus ; elle ne
            // panique pas (erreurs ignorées, `try_lock`).
            let _ = unsafe { libc::atexit(supprimer_tout) };
        });
    }

    #[cfg(not(unix))]
    fn armer() {
        // Pas d'`atexit` ici : les copies de lecture restent dans TMPDIR
        // (documenté dans docs/BENCHMARKS.md).
        static ARME: Once = Once::new();
        ARME.call_once(|| {});
    }

    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn processus_vivant(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return true;
        };
        // SAFETY: `kill` avec le signal 0 n'envoie rien : il teste seulement
        // l'existence du processus.
        let resultat = unsafe { libc::kill(pid, 0) };
        // Échec autre que « aucun processus » (droits…) : on le tient pour vivant.
        resultat == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    #[cfg(not(unix))]
    fn processus_vivant(_pid: u32) -> bool {
        true
    }

    /// Supprime de `TMPDIR` les copies `obrain-banc-<pid>-…` dont le
    /// propriétaire n'existe plus. Une fois par processus.
    pub(super) fn balayer_les_residus() {
        static FAIT: Once = Once::new();
        FAIT.call_once(|| {
            let Ok(entrees) = std::fs::read_dir(std::env::temp_dir()) else {
                return;
            };
            for entree in entrees.flatten() {
                let nom = entree.file_name();
                let Some(nom) = nom.to_str() else { continue };
                if nom.starts_with(PREFIXE_COPIE_GARDEE) {
                    continue;
                }
                let Some(pid) = nom
                    .strip_prefix(PREFIXE_COPIE)
                    .and_then(|reste| reste.split('-').next())
                    .and_then(|pid| pid.parse::<u32>().ok())
                else {
                    continue;
                };
                if !processus_vivant(pid) {
                    eprintln!(
                        "BANC : résidu d'un processus interrompu supprimé : {}",
                        entree.path().display()
                    );
                    let _ = std::fs::remove_dir_all(entree.path());
                }
            }
        });
    }
}

// ───────────────────────────── Empreinte ────────────────────────────────────

/// État observable d'une entrée : taille, date de modification, condensé du
/// contenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmpreinteFichier {
    /// Taille en octets (`0` pour un répertoire).
    pub taille: u64,
    /// Date de dernière modification. Pour un répertoire, elle bouge dès
    /// qu'une entrée y est créée, supprimée ou renommée : c'est elle qui
    /// trahit un fichier créé PUIS supprimé (verrou, temporaire), que la
    /// seule liste des chemins ne verrait pas.
    pub modifie: SystemTime,
    /// Condensé FNV-1a 64 bits du contenu (`0` pour un répertoire).
    pub condense: u64,
}

/// Empreinte d'une arborescence : chaque chemin relatif (la racine comprise,
/// sous le chemin vide) et son état. Deux empreintes égales prouvent que rien
/// n'a été créé, supprimé, renommé ni modifié — même transitoirement.
pub type Empreinte = BTreeMap<PathBuf, EmpreinteFichier>;

/// Calcule l'[`Empreinte`] de `racine` (récursive).
///
/// # Errors
///
/// Toute erreur d'entrée-sortie rencontrée pendant le parcours.
pub fn empreinte(racine: &Path) -> io::Result<Empreinte> {
    let mut etat = Empreinte::new();
    etat.insert(
        PathBuf::new(),
        EmpreinteFichier {
            taille: 0,
            modifie: fs::metadata(racine)?.modified()?,
            condense: 0,
        },
    );
    empreinte_dans(racine, racine, &mut etat)?;
    Ok(etat)
}

fn empreinte_dans(racine: &Path, dossier: &Path, etat: &mut Empreinte) -> io::Result<()> {
    for entree in fs::read_dir(dossier)? {
        let chemin = entree?.path();
        let relatif = chemin
            .strip_prefix(racine)
            .expect("chemin sous la racine")
            .to_path_buf();
        let meta = fs::metadata(&chemin)?;
        if meta.is_dir() {
            etat.insert(
                relatif,
                EmpreinteFichier {
                    taille: 0,
                    modifie: meta.modified()?,
                    condense: 0,
                },
            );
            empreinte_dans(racine, &chemin, etat)?;
        } else {
            etat.insert(
                relatif,
                EmpreinteFichier {
                    taille: meta.len(),
                    modifie: meta.modified()?,
                    condense: condense_fichier(&chemin)?,
                },
            );
        }
    }
    Ok(())
}

fn condense_fichier(chemin: &Path) -> io::Result<u64> {
    let mut fichier = fs::File::open(chemin)?;
    let mut tampon = vec![0u8; 1 << 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let lus = fichier.read(&mut tampon)?;
        if lus == 0 {
            return Ok(h);
        }
        for octet in &tampon[..lus] {
            h ^= u64::from(*octet);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
