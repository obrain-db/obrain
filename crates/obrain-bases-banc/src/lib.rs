//! Désignation **explicite** des bases réelles pour les bancs et les audits.
//!
//! # Pourquoi ce crate existe
//!
//! Le 2026-11-11, un simple `cargo test -p obrain-engine --features cypher` a
//! ouvert **en lecture-écriture** les bases de travail d'un développeur
//! (`po`, `wikipedia`, `megalaw`) : une trentaine de cibles de test devinaient
//! leur chemin à partir de `$HOME` et ne se sautaient que si le dossier était
//! absent. L'une d'elles y a créé puis supprimé des nœuds et posé des
//! propriétés `_bench_*` ; une autre a été tuée en pleine ouverture.
//!
//! # La règle
//!
//! * **Par défaut, aucune cible n'ouvre de base.** Elle se saute en l'écrivant
//!   sur la sortie d'erreur. Aucun chemin n'est jamais déduit de `$HOME`.
//! * Une cible ne s'active que si une variable d'environnement **désigne** la
//!   base : `OBRAIN_BENCH_DB_PO`, `OBRAIN_BENCH_DB_WIKIPEDIA`,
//!   `OBRAIN_BENCH_DB_MEGALAW`. **Seul ce crate lit ces variables** : leur nom
//!   n'est pas exporté, et la garde du dépôt refuse toute autre lecture. Le
//!   chemin doit être absolu et contenir `substrate.meta` ; sinon la cible
//!   échoue avant toute ouverture.
//! * **La base désignée n'est jamais ouverte par le moteur — pas même en
//!   lecture seule.** Elle est copiée (clonée quand le système de fichiers le
//!   permet) et c'est la copie qu'on ouvre. La copie est refusée, nommément,
//!   si un processus tient la base, si elle change pendant la copie, si elle
//!   contient un lien symbolique, ou si une copie pleine dépasserait le
//!   plafond ou la marge de disque (voir `copie`).
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
//! # Les issues
//!
//! * [`base_lecture`] rend une [`BaseLecture`] **opaque** : ses seules issues
//!   sont [`BaseLecture::ouvrir`] (feature `moteur`) et
//!   [`BaseLecture::ouvrir_substrat`], qui ouvrent en lecture seule une copie
//!   partagée par tout le processus — une copie par base et par binaire de
//!   test, pas une par test. Elle n'est PAS partagée entre binaires : chaque
//!   binaire lancé par `cargo test` prend la sienne (gratuite en clone,
//!   bornée en copie pleine).
//! * [`base_copie`] et [`copie_de`] rendent une [`CopieBase`] privée à la
//!   cible, qui peut y écrire : son chemin est exposé parce que ce n'est
//!   jamais celui de la base désignée.
//!
//! Mode d'emploi complet : `docs/BENCHMARKS.md`, section « Bancs sur bases
//! réelles ».

mod copie;
mod empreinte;
mod nettoyage;

/// Crochets pour les tests de ce dépôt. Ce n'est pas une API : rien ici
/// n'ouvre ni ne révèle une base, mais rien n'y est stable non plus.
#[doc(hidden)]
pub mod essais {
    /// Pose une pause (ms) entre la copie et ses contrôles d'après copie,
    /// pour y glisser une course (un écrivain qui ouvre, un fichier qui
    /// change). Zéro la retire.
    pub fn pause_apres_copie(ms: u64) {
        crate::copie::PAUSE_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// Balaie maintenant les résidus de copie de `TMPDIR` dont le
    /// propriétaire est mort ; rend ceux qui ont été supprimés.
    pub fn balayer_les_residus_maintenant() -> Vec<std::path::PathBuf> {
        crate::nettoyage::balayer_maintenant()
    }
}

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(feature = "moteur")]
use obrain_engine::ObrainDB;
use obrain_substrate::SubstrateStore;

pub use copie::{VARIABLE_COPIE_MAX_GO, VARIABLE_COPIE_PLEINE, VARIABLE_MARGE_GO};
pub use empreinte::{Empreinte, EmpreinteFichier, empreinte};

/// Variable qui, posée à `1`, conserve la copie de travail de [`base_copie`]
/// après la cible (son chemin est imprimé) au lieu de la supprimer.
pub const VARIABLE_GARDER_COPIE: &str = "OBRAIN_BENCH_GARDER_COPIE";

/// Fichier dont la présence fait d'un répertoire une base Obrain.
const FICHIER_META: &str = "substrate.meta";

/// Préfixe des répertoires de copie sous `TMPDIR` (suivi du PID du
/// propriétaire, pour les humains ; la preuve de vie est un verrou, voir
/// `nettoyage`).
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

    /// Variable d'environnement qui désigne cette base. **Non publique** : si
    /// une cible connaissait le nom, elle pourrait lire la variable et obtenir
    /// le chemin d'ORIGINE au lieu d'une copie.
    pub(crate) const fn variable(self) -> &'static str {
        match self {
            Self::Po => "OBRAIN_BENCH_DB_PO",
            Self::Wikipedia => "OBRAIN_BENCH_DB_WIKIPEDIA",
            Self::Megalaw => "OBRAIN_BENCH_DB_MEGALAW",
        }
    }
}

/// Prépare l'environnement d'un SOUS-PROCESSUS de test : retire toute
/// désignation héritée, puis désigne `designations` (et elles seules). Écrire
/// l'environnement d'un enfant ne révèle aucun chemin : l'appelant fournit
/// les siens.
pub fn designer_pour_un_enfant(
    commande: &mut std::process::Command,
    designations: &[(Base, &Path)],
) {
    commande.env_remove(VARIABLE_GARDER_COPIE);
    for base in Base::TOUTES {
        commande.env_remove(base.variable());
    }
    for (base, chemin) in designations {
        commande.env(base.variable(), chemin);
    }
}

/// Vérifie qu'un chemin désigné est celui d'une base Obrain existante, sans
/// rien ouvrir.
///
/// * chemin relatif : il serait résolu contre le répertoire courant du
///   binaire de test (le dossier du crate), pas contre celui de l'appelant ;
/// * pas de `substrate.meta` : ce n'est pas une base. Le moteur en CRÉERAIT
///   une à cet endroit, même ouvert en lecture seule — désigner par erreur le
///   dossier parent sèmerait des `substrate.*` dans une arborescence réelle.
fn verifier_base(chemin: &Path, origine: &str) -> Result<(), String> {
    if !chemin.is_absolute() {
        return Err(format!(
            "{origine} désigne « {} » : un chemin ABSOLU est exigé (un chemin relatif serait \
             résolu contre le dossier du crate testé, pas contre le vôtre)",
            chemin.display()
        ));
    }
    if !chemin.join(FICHIER_META).is_file() {
        return Err(format!(
            "{origine} désigne « {} », qui n'est pas une base Obrain : pas de {FICHIER_META} dans \
             ce répertoire. Rien n'est ouvert.",
            chemin.display()
        ));
    }
    Ok(())
}

/// Ce que dit l'environnement d'une base.
#[derive(Debug, PartialEq, Eq)]
enum Designation {
    /// La variable n'existe pas.
    Absente,
    /// La variable existe mais elle est vide : rien n'est désigné.
    Vide,
    /// La variable désigne une base vérifiée (chemin absolu, `substrate.meta`).
    Base(PathBuf),
}

/// Résout la désignation de `base` à partir d'un environnement **fourni**
/// (`env` rend la valeur d'une variable). Fonction pure, que les tests de ce
/// crate exercent sans toucher à l'environnement du processus.
///
/// **Privée à dessein** : elle rend le chemin de la base D'ORIGINE.
///
/// # Panics
///
/// Si la variable désigne autre chose qu'une base : une désignation explicite
/// erronée est une erreur de l'appelant, pas un saut silencieux.
fn resoudre_avec(base: Base, env: impl Fn(&str) -> Option<OsString>) -> Designation {
    match env(base.variable()) {
        None => Designation::Absente,
        Some(valeur) if valeur.is_empty() => Designation::Vide,
        Some(valeur) => {
            let chemin = PathBuf::from(valeur);
            if let Err(refus) = verifier_base(&chemin, base.variable()) {
                panic!("{refus}");
            }
            Designation::Base(chemin)
        }
    }
}

/// Nom de la cible en cours, pour les annonces : libtest nomme le fil
/// d'exécution d'après le test.
fn cible() -> String {
    std::thread::current().name().unwrap_or("?").to_owned()
}

/// Résout `base` dans l'environnement du processus et **annonce** le saut
/// quand elle n'est pas désignée.
fn resoudre_ou_annoncer(base: Base) -> Option<PathBuf> {
    let etat = match resoudre_avec(base, |cle| std::env::var_os(cle)) {
        Designation::Base(chemin) => return Some(chemin),
        Designation::Absente => "n'est pas définie",
        Designation::Vide => "est définie mais vide",
    };
    eprintln!(
        "SAUT [{}] : {} {etat} — la base réelle « {} » n'est PAS ouverte \
         (aucun chemin n'est deviné sous $HOME ; voir docs/BENCHMARKS.md, \
         « Bancs sur bases réelles »)",
        cible(),
        base.variable(),
        base.nom()
    );
    None
}

// ───────────────────────────── Lecteurs ─────────────────────────────────────

/// Base désignée, pour une cible qui ne fait que **lire**.
///
/// Type **opaque** : il ne rend aucun chemin. Ses seules issues sont
/// [`ouvrir`](Self::ouvrir) et [`ouvrir_substrat`](Self::ouvrir_substrat),
/// qui ouvrent **en lecture seule** une **copie** de la base — jamais la base
/// désignée. Une cible ne peut donc ni ouvrir l'original, ni l'ouvrir en
/// écriture, même par inadvertance.
///
/// Plusieurs lecteurs peuvent ouvrir la même copie en même temps (huit
/// ouvertures simultanées d'une copie de base sale, 150 fois de suite, voient
/// toutes la même base : `bases_reelles_non_ouvertes.rs`).
#[derive(Debug, Clone)]
pub struct BaseLecture {
    base: Base,
    copie: PathBuf,
}

impl BaseLecture {
    /// Ouvre la copie en lecture seule avec le moteur (feature `moteur`).
    ///
    /// # Panics
    ///
    /// Si l'ouverture échoue.
    #[cfg(feature = "moteur")]
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

/// Résultat mémorisé d'une copie de lecture : la copie, ou le refus — qu'on
/// ne retente pas à chaque test.
type CopieLecture = Arc<OnceLock<Result<PathBuf, String>>>;

/// Une cellule par base : le verrou global n'est tenu que le temps de la
/// trouver, la copie se fait sous la cellule de SA base (deux bases se copient
/// en parallèle ; deux demandes de la même base attendent la même copie).
fn cellule_de_lecture(base: Base) -> CopieLecture {
    static CELLULES: OnceLock<Mutex<HashMap<Base, CopieLecture>>> = OnceLock::new();
    CELLULES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(base)
        .or_default()
        .clone()
}

/// Verrous de propriétaire des copies de lecture : tenus jusqu'à la sortie du
/// processus.
fn verrous_de_lecture() -> &'static Mutex<Vec<File>> {
    static VERROUS: OnceLock<Mutex<Vec<File>>> = OnceLock::new();
    VERROUS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Base désignée pour une cible qui ne fait que **lire** — ou `None`, après
/// l'avoir annoncé sur la sortie d'erreur, quand la variable de la base n'est
/// pas définie : la cible se saute.
///
/// La base désignée est copiée **une fois par processus** (donc une fois par
/// binaire de test, quel que soit le nombre de tests qui la demandent) ; la
/// copie est supprimée à la sortie du processus. Un refus de copie est
/// mémorisé : les tests suivants du binaire échouent avec le même message,
/// sans retenter.
///
/// # Panics
///
/// Si la variable désigne autre chose qu'une base, ou si la copie est refusée
/// (base tenue par un processus ou modifiée pendant la copie, lien
/// symbolique, plafond ou marge de disque, erreur d'E/S).
#[must_use]
pub fn base_lecture(base: Base) -> Option<BaseLecture> {
    let source = resoudre_ou_annoncer(base)?;
    let cellule = cellule_de_lecture(base);
    let resultat = cellule.get_or_init(|| {
        let prefixe = format!("{PREFIXE_COPIE}{}-{}-", std::process::id(), base.nom());
        let copie = copie::copier_base(
            &source,
            base.nom(),
            &prefixe,
            copie::Limites::depuis_l_environnement(),
        )?;
        eprintln!("BANC [{}] : {}", cible(), copie::decrire(&copie, &source));
        eprintln!(
            "BANC [{}] : base « {} » = copie {} (ouverte en lecture seule, partagée par ce \
             binaire, supprimée à sa sortie)",
            cible(),
            base.nom(),
            copie.chemin.display()
        );
        nettoyage::a_la_sortie(copie.racine.clone());
        if let Some(verrou) = copie.verrou {
            verrous_de_lecture()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(verrou);
        }
        Ok(copie.chemin)
    });
    match resultat {
        Ok(copie) => Some(BaseLecture {
            base,
            copie: copie.clone(),
        }),
        Err(refus) => panic!(
            "copie de lecture de « {} » refusée (refus mémorisé pour tout ce binaire) : {refus}",
            base.nom()
        ),
    }
}

// ───────────────────────────── Écrivains ────────────────────────────────────

/// Copie de travail **privée** d'une base désignée : tout ce qu'une cible y
/// écrit reste dans un répertoire temporaire, supprimé quand la valeur est
/// lâchée (sauf [`VARIABLE_GARDER_COPIE`]).
#[derive(Debug)]
pub struct CopieBase {
    chemin: PathBuf,
    racine: Option<PathBuf>,
    _verrou: Option<File>,
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
            let _ = std::fs::remove_dir_all(&racine);
            nettoyage::oublier(&racine);
        }
    }
}

/// Copie privée de la base désignée, pour une cible qui **écrit** (ou qui
/// mesure l'ouverture en écriture). La base désignée n'est que lue, fichier
/// par fichier ; elle n'est jamais ouverte par le moteur.
///
/// Rend `None`, après l'avoir annoncé, quand la variable n'est pas définie.
///
/// # Panics
///
/// Si la variable désigne autre chose qu'une base, ou si la copie est refusée
/// (voir [`base_lecture`]).
#[must_use]
pub fn base_copie(base: Base) -> Option<CopieBase> {
    let source = resoudre_ou_annoncer(base)?;
    Some(copie_de(&source, base.variable()))
}

/// Copie privée d'une base désignée **par son chemin** — pour les cibles qui
/// ont leur propre variable (`OBRAIN_TEST_STORE_REEL_DIR`…) ou leurs
/// arguments.
///
/// # Panics
///
/// Si [`essayer_copie_de`] refuse.
#[must_use]
pub fn copie_de(source: &Path, origine: &str) -> CopieBase {
    essayer_copie_de(source, origine).unwrap_or_else(|refus| panic!("{refus}"))
}

/// Comme [`copie_de`], mais rend le refus au lieu de paniquer — pour un outil
/// qui veut sauter une base et continuer.
///
/// La copie n'est prise que si personne ne tient la base (verrou
/// `substrate.dirlock` du moteur, sous Unix) et qu'elle n'a pas changé entre le
/// début et la fin de la copie ; elle est refusée nommément sinon.
///
/// # Errors
///
/// Un message nommé : chemin relatif ou sans `substrate.meta`, base tenue ou
/// modifiée pendant la copie, lien symbolique, plafond ou marge de disque,
/// erreur d'E/S.
pub fn essayer_copie_de(source: &Path, origine: &str) -> Result<CopieBase, String> {
    verifier_base(source, origine)?;
    let garder = std::env::var_os(VARIABLE_GARDER_COPIE).is_some_and(|v| v == "1");
    let prefixe = if garder {
        format!("{PREFIXE_COPIE_GARDEE}{}-", std::process::id())
    } else {
        format!("{PREFIXE_COPIE}{}-base-", std::process::id())
    };
    let copie = copie::copier_base(
        source,
        "base",
        &prefixe,
        copie::Limites::depuis_l_environnement(),
    )?;
    eprintln!("BANC [{}] : {}", cible(), copie::decrire(&copie, source));
    eprintln!(
        "BANC [{}] : copie privée {} ({})",
        cible(),
        copie.chemin.display(),
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
        nettoyage::a_la_sortie(copie.racine.clone());
        Some(copie.racine)
    };
    Ok(CopieBase {
        chemin: copie.chemin,
        racine,
        _verrou: copie.verrou,
    })
}

#[cfg(test)]
mod tests {
    //! La résolution, exercée ici parce qu'elle est privée : elle rend le
    //! chemin de la base d'origine, qu'aucune cible ne doit pouvoir obtenir.

    use super::{Base, Designation, empreinte, resoudre_avec};
    use crate::copie::{self, Limites, Mode};
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    /// Fabrique `<racine>/.obrain/db/{po,wikipedia,megalaw}`, chacune avec un
    /// `substrate.meta` : la disposition exacte que les anciennes cibles
    /// devinaient. (Fichiers factices : ces tests ne passent pas par le moteur.)
    fn faux_home(racine: &Path) {
        for base in Base::TOUTES {
            let dossier = racine.join(".obrain/db").join(base.nom());
            fs::create_dir_all(&dossier).unwrap();
            fs::write(dossier.join("substrate.meta"), b"en-tete").unwrap();
            fs::write(dossier.join("substrate.nodes"), b"donnees de l'utilisateur").unwrap();
        }
    }

    fn environnement(paires: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let table: HashMap<String, OsString> = paires
            .iter()
            .map(|(cle, valeur)| ((*cle).to_owned(), valeur.as_os_str().to_owned()))
            .collect();
        move |cle| table.get(cle).cloned()
    }

    #[test]
    fn home_seul_ne_designe_aucune_base() {
        let tmp = tempfile::tempdir().unwrap();
        faux_home(tmp.path());
        let avant = empreinte(tmp.path()).unwrap();

        let env = environnement(&[("HOME", tmp.path()), ("USERPROFILE", tmp.path())]);
        for base in Base::TOUTES {
            assert_eq!(
                resoudre_avec(base, &env),
                Designation::Absente,
                "la base « {} » a été résolue sans {} : un chemin a été deviné sous HOME",
                base.nom(),
                base.variable()
            );
        }

        assert_eq!(
            avant,
            empreinte(tmp.path()).unwrap(),
            "la résolution a touché aux fichiers"
        );
    }

    #[test]
    fn une_variable_vide_ne_designe_rien_et_le_dit() {
        let env = environnement(&[(Base::Po.variable(), Path::new(""))]);
        assert_eq!(resoudre_avec(Base::Po, &env), Designation::Vide);
    }

    #[test]
    fn la_variable_designe_sa_base_et_elle_seule() {
        let tmp = tempfile::tempdir().unwrap();
        faux_home(tmp.path());
        let po = tmp.path().join(".obrain/db/po");
        let env = environnement(&[(Base::Po.variable(), &po), ("HOME", tmp.path())]);

        assert_eq!(resoudre_avec(Base::Po, &env), Designation::Base(po));
        assert_eq!(resoudre_avec(Base::Wikipedia, &env), Designation::Absente);
        assert_eq!(resoudre_avec(Base::Megalaw, &env), Designation::Absente);
    }

    #[test]
    #[should_panic(expected = "pas de substrate.meta")]
    fn un_chemin_inexistant_echoue_bruyamment() {
        let env = environnement(&[(Base::Po.variable(), Path::new("/chemin/qui/n/existe/pas"))]);
        let _ = resoudre_avec(Base::Po, &env);
    }

    /// Le piège : désigner le PARENT des bases. Le moteur y créerait une base
    /// vide, même ouvert en lecture seule. Refus avant toute ouverture, et
    /// rien n'est créé.
    #[test]
    fn le_dossier_parent_des_bases_est_refuse_sans_rien_creer() {
        let tmp = tempfile::tempdir().unwrap();
        faux_home(tmp.path());
        let parent = tmp.path().join(".obrain/db");
        let avant = empreinte(tmp.path()).unwrap();

        let env = environnement(&[(Base::Po.variable(), &parent)]);
        let refus = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            resoudre_avec(Base::Po, &env)
        }));
        assert!(
            refus.is_err(),
            "le parent des bases a été accepté comme base"
        );
        assert_eq!(avant, empreinte(tmp.path()).unwrap());
    }

    #[test]
    #[should_panic(expected = "chemin ABSOLU")]
    fn un_chemin_relatif_est_refuse() {
        let env = environnement(&[(Base::Po.variable(), Path::new("bases/po"))]);
        let _ = resoudre_avec(Base::Po, &env);
    }

    // ─────────────────────── Le moteur de copie ────────────────────────────

    /// Une fausse base de fichiers factices.
    fn fausse_base(racine: &Path) -> std::path::PathBuf {
        let base = racine.join("source");
        fs::create_dir_all(base.join("graphs/annexe")).unwrap();
        fs::write(base.join("substrate.meta"), b"en-tete").unwrap();
        fs::write(base.join("substrate.nodes"), vec![7u8; 4096]).unwrap();
        fs::write(base.join("graphs/annexe/substrate.meta"), b"annexe").unwrap();
        base
    }

    fn limites() -> Limites {
        Limites {
            copie_max: u64::MAX,
            marge: 0,
            forcer_pleine: false,
        }
    }

    fn copier(source: &Path, l: Limites) -> Result<copie::Copie, String> {
        copie::copier_base(source, "base", "obrain-banc-essai-", l)
    }

    #[test]
    fn la_copie_reproduit_la_base_et_appartient_au_banc() {
        let tmp = tempfile::tempdir().unwrap();
        let source = fausse_base(tmp.path());
        for forcer_pleine in [false, true] {
            let c = copier(
                &source,
                Limites {
                    forcer_pleine,
                    ..limites()
                },
            )
            .unwrap();
            assert_eq!(
                fs::read(c.chemin.join("substrate.nodes")).unwrap(),
                vec![7u8; 4096]
            );
            assert_eq!(
                fs::read(c.chemin.join("graphs/annexe/substrate.meta")).unwrap(),
                b"annexe"
            );
            if forcer_pleine {
                assert_eq!(c.mode, Mode::Pleine);
            } else if cfg!(target_vendor = "apple") {
                assert_eq!(c.mode, Mode::Clone, "APFS sait cloner");
            }
            fs::remove_dir_all(&c.racine).unwrap();
        }
    }

    /// Un lien qui boucle ferait déborder la pile d'une copie qui suit les
    /// liens ; un lien cassé la ferait paniquer. Les deux sont refusés,
    /// nommément, et rien ne reste dans TMPDIR.
    #[cfg(unix)]
    #[test]
    fn un_lien_symbolique_est_refuse_nommement() {
        for (nom, cible) in [("boucle", "."), ("casse", "/nulle/part/obrain")] {
            let tmp = tempfile::tempdir().unwrap();
            let source = fausse_base(tmp.path());
            std::os::unix::fs::symlink(cible, source.join("graphs").join(nom)).unwrap();
            let refus = copier(&source, limites()).unwrap_err();
            assert!(
                refus.contains("lien symbolique refusé") && refus.contains(nom),
                "{refus}"
            );
        }
    }

    /// Une base en 0444 (instantané en lecture seule) doit donner une copie
    /// que le moteur peut ouvrir en écriture.
    #[cfg(unix)]
    #[test]
    fn une_base_en_lecture_seule_donne_une_copie_inscriptible() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let source = fausse_base(tmp.path());
        for f in [
            "substrate.meta",
            "substrate.nodes",
            "graphs/annexe/substrate.meta",
        ] {
            fs::set_permissions(source.join(f), fs::Permissions::from_mode(0o444)).unwrap();
        }
        for forcer_pleine in [false, true] {
            let c = copier(
                &source,
                Limites {
                    forcer_pleine,
                    ..limites()
                },
            )
            .unwrap();
            for f in [
                "substrate.meta",
                "substrate.nodes",
                "graphs/annexe/substrate.meta",
            ] {
                fs::OpenOptions::new()
                    .write(true)
                    .open(c.chemin.join(f))
                    .unwrap_or_else(|e| panic!("{f} non inscriptible dans la copie : {e}"));
            }
            fs::remove_dir_all(&c.racine).unwrap();
        }
    }

    #[test]
    fn une_copie_pleine_au_dela_du_plafond_est_refusee() {
        let tmp = tempfile::tempdir().unwrap();
        let source = fausse_base(tmp.path());
        let refus = copier(
            &source,
            Limites {
                copie_max: 0,
                forcer_pleine: true,
                ..limites()
            },
        )
        .unwrap_err();
        assert!(refus.contains("au-delà du plafond"), "{refus}");
    }

    #[cfg(unix)]
    #[test]
    fn une_copie_sans_marge_de_disque_est_refusee() {
        let tmp = tempfile::tempdir().unwrap();
        let source = fausse_base(tmp.path());
        for forcer_pleine in [false, true] {
            let refus = copier(
                &source,
                Limites {
                    marge: u64::MAX / 4,
                    forcer_pleine,
                    ..limites()
                },
            )
            .unwrap_err();
            assert!(refus.contains("place insuffisante"), "{refus}");
        }
    }

    /// Un fichier de 64 Mio presque entièrement creux reste creux dans une
    /// copie pleine.
    #[cfg(unix)]
    #[test]
    fn la_copie_pleine_preserve_les_trous() {
        use std::io::{Seek, SeekFrom, Write};
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let source = fausse_base(tmp.path());
        let mut f = fs::File::create(source.join("substrate.edges")).unwrap();
        f.write_all(b"debut").unwrap();
        f.seek(SeekFrom::Start(40 << 20)).unwrap();
        f.write_all(b"milieu").unwrap();
        f.set_len(64 << 20).unwrap();
        drop(f);
        let c = copier(
            &source,
            Limites {
                forcer_pleine: true,
                ..limites()
            },
        )
        .unwrap();
        let meta = fs::metadata(c.chemin.join("substrate.edges")).unwrap();
        assert_eq!(meta.len(), 64 << 20);
        assert!(
            meta.blocks() * 512 < 8 << 20,
            "la copie a rempli les trous : {} octets physiques",
            meta.blocks() * 512
        );
        let octets = fs::read(c.chemin.join("substrate.edges")).unwrap();
        assert_eq!(&octets[..5], b"debut");
        assert_eq!(&octets[40 << 20..(40 << 20) + 6], b"milieu");
        fs::remove_dir_all(&c.racine).unwrap();
    }
}

#[cfg(test)]
mod ordre_de_fermeture {
    //! Le banc `zero_copy` garde une base et le garde de ses fichiers dans
    //! une structure : c'est l'ordre de déclaration des champs qui fait
    //! fermer la base AVANT de supprimer ses fichiers. Ce test fixe la
    //! garantie du langage sur laquelle il s'appuie.

    use std::cell::RefCell;
    use std::rc::Rc;

    struct Trace(Rc<RefCell<Vec<&'static str>>>, &'static str);
    impl Drop for Trace {
        fn drop(&mut self) {
            self.0.borrow_mut().push(self.1);
        }
    }

    struct Ouvert {
        _base: Trace,
        _garde: Trace,
    }

    #[test]
    fn les_champs_sont_laches_dans_l_ordre_de_leur_declaration() {
        let journal = Rc::new(RefCell::new(Vec::new()));
        drop(Ouvert {
            _base: Trace(journal.clone(), "base fermée"),
            _garde: Trace(journal.clone(), "fichiers supprimés"),
        });
        assert_eq!(*journal.borrow(), ["base fermée", "fichiers supprimés"]);
    }
}
