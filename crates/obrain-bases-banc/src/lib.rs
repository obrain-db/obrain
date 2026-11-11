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
//!   `OBRAIN_BENCH_DB_MEGALAW` (voir [`Base::variable`]).
//! * Une cible qui ne fait que **lire** passe par [`base_lecture`] et ouvre le
//!   chemin rendu **en lecture seule** (`ObrainDB::open_read_only`,
//!   `SubstrateStore::open_read_only`).
//! * Une cible qui **écrit** — ou qui mesure l'ouverture en écriture — passe
//!   par [`base_copie`] : elle travaille sur une copie dans un répertoire
//!   temporaire, jamais sur la base désignée.
//!
//! Mode d'emploi complet : `docs/BENCHMARKS.md`, section « Bancs sur bases
//! réelles ».

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Variable qui, posée à `1`, conserve la copie de travail de [`base_copie`]
/// après la cible (son chemin est imprimé) au lieu de la supprimer.
pub const VARIABLE_GARDER_COPIE: &str = "OBRAIN_BENCH_GARDER_COPIE";

/// Les bases réelles que les bancs et audits du dépôt savent exploiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Résout la désignation de `base` à partir d'un environnement **fourni**
/// (`env` rend la valeur d'une variable). Fonction pure : c'est elle que les
/// tests exercent, sans toucher à l'environnement du processus.
///
/// Rend `None` quand la variable est absente ou vide — jamais un chemin
/// deviné, quel que soit `HOME`.
///
/// # Panics
///
/// Si la variable désigne un chemin qui n'est pas un répertoire : une
/// désignation explicite erronée est une erreur de l'appelant, pas un saut
/// silencieux.
pub fn resoudre_avec(base: Base, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let valeur = env(base.variable()).filter(|v| !v.is_empty())?;
    let chemin = PathBuf::from(valeur);
    assert!(
        chemin.is_dir(),
        "{} désigne « {} », qui n'est pas un répertoire de base Obrain",
        base.variable(),
        chemin.display()
    );
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

/// Chemin de la base désignée, pour une cible qui ne fait que **lire**.
///
/// L'appelant **doit** ouvrir ce chemin en lecture seule
/// (`ObrainDB::open_read_only`, `SubstrateStore::open_read_only`) : les
/// fichiers de données restent alors identiques à l'octet ; seul
/// `substrate.alloc`, la borne d'allocation partagée que le moteur republie à
/// chaque ouverture, peut changer. Rend
/// `None`, après l'avoir annoncé sur la sortie d'erreur, quand la variable
/// de la base n'est pas définie : la cible se saute.
///
/// # Panics
///
/// Si la variable désigne un chemin qui n'est pas un répertoire.
#[must_use]
pub fn base_lecture(base: Base) -> Option<PathBuf> {
    let chemin = resoudre_ou_annoncer(base)?;
    eprintln!(
        "BANC [{}] : base « {} » = {} (lecture seule)",
        cible(),
        base.nom(),
        chemin.display()
    );
    Some(chemin)
}

/// Copie de travail d'une base désignée : tout ce qu'une cible y écrit reste
/// dans un répertoire temporaire, supprimé quand la valeur est lâchée (sauf
/// [`VARIABLE_GARDER_COPIE`]).
#[derive(Debug)]
pub struct CopieBase {
    chemin: PathBuf,
    _temporaire: Option<tempfile::TempDir>,
}

impl CopieBase {
    /// Chemin de la copie — le seul que la cible a le droit d'ouvrir.
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

/// Copie de la base désignée dans un répertoire temporaire, pour une cible
/// qui **écrit** (ou qui mesure l'ouverture en écriture). La base désignée
/// n'est que lue, fichier par fichier ; elle n'est jamais ouverte par le
/// moteur.
///
/// Rend `None`, après l'avoir annoncé, quand la variable n'est pas définie.
/// La copie n'est cohérente que si rien n'écrit dans la base pendant qu'elle
/// est prise : désigner un instantané, ou arrêter le service qui la tient.
///
/// # Panics
///
/// Si la variable désigne un chemin qui n'est pas un répertoire, ou si la
/// copie échoue (espace disque, droits).
#[must_use]
pub fn base_copie(base: Base) -> Option<CopieBase> {
    let source = resoudre_ou_annoncer(base)?;
    let temporaire = tempfile::Builder::new()
        .prefix(&format!("obrain-banc-{}-", base.nom()))
        .tempdir()
        .expect("création du répertoire temporaire de la copie");
    let chemin = temporaire.path().join(base.nom());
    copier_arborescence(&source, &chemin).unwrap_or_else(|e| {
        panic!(
            "copie de {} vers {} : {e}",
            source.display(),
            chemin.display()
        )
    });
    let garder = std::env::var_os(VARIABLE_GARDER_COPIE).is_some_and(|v| v == "1");
    eprintln!(
        "BANC [{}] : base « {} » = COPIE {} de {} ({})",
        cible(),
        base.nom(),
        chemin.display(),
        source.display(),
        if garder {
            "conservée après la cible"
        } else {
            "supprimée après la cible"
        }
    );
    let temporaire = if garder {
        // Le répertoire survit : c'est à l'appelant de le supprimer.
        let _ = temporaire.keep();
        None
    } else {
        Some(temporaire)
    };
    Some(CopieBase {
        chemin,
        _temporaire: temporaire,
    })
}

/// Copie récursive de `source` vers `destination` (créée). Les liens
/// symboliques sont suivis : la copie ne pointe jamais vers l'original.
///
/// # Errors
///
/// Toute erreur d'entrée-sortie rencontrée pendant la copie.
pub fn copier_arborescence(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entree in fs::read_dir(source)? {
        let entree = entree?;
        let depuis = entree.path();
        let vers = destination.join(entree.file_name());
        if fs::metadata(&depuis)?.is_dir() {
            copier_arborescence(&depuis, &vers)?;
        } else {
            fs::copy(&depuis, &vers)?;
        }
    }
    Ok(())
}

/// État observable d'un fichier : taille, date de modification, condensé du
/// contenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmpreinteFichier {
    /// Taille en octets (`0` pour un répertoire).
    pub taille: u64,
    /// Date de dernière modification (`None` pour un répertoire, dont la
    /// date bouge à la moindre création de fichier — que la liste des
    /// chemins détecte déjà).
    pub modifie: Option<SystemTime>,
    /// Condensé FNV-1a 64 bits du contenu (`0` pour un répertoire).
    pub condense: u64,
}

/// Empreinte d'une arborescence : chaque chemin relatif et son état. Deux
/// empreintes égales prouvent que rien n'a été créé, supprimé ni modifié.
pub type Empreinte = BTreeMap<PathBuf, EmpreinteFichier>;

/// Calcule l'[`Empreinte`] de `racine` (récursive).
///
/// # Errors
///
/// Toute erreur d'entrée-sortie rencontrée pendant le parcours.
pub fn empreinte(racine: &Path) -> io::Result<Empreinte> {
    let mut etat = Empreinte::new();
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
                    modifie: None,
                    condense: 0,
                },
            );
            empreinte_dans(racine, &chemin, etat)?;
        } else {
            etat.insert(
                relatif,
                EmpreinteFichier {
                    taille: meta.len(),
                    modifie: Some(meta.modified()?),
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
