//! D'où l'indexeur LIT — un disque, un VFS, une archive.
//!
//! L'indexeur lisait le disque en deux endroits seulement :
//! `std::fs::read_dir` dans [`crate::discovery`] et `std::fs::read_to_string`
//! dans [`crate::index`]. Tant que c'était vrai, il ne pouvait indexer qu'un
//! arbre réel : un VFS — les arbres montés par `this-vfs`, une archive, un
//! dépôt chargé en mémoire — n'avait aucun moyen de lui être présenté.
//!
//! [`Source`] est ce point d'entrée unique. Deux implémentations sont
//! fournies :
//!
//! - [`Disque`] : le système de fichiers, comportement historique inchangé ;
//! - [`Memoire`] : un arbre en mémoire, construit à partir d'un disque
//!   ([`Memoire::depuis_dossier`]) ou pièce par pièce ([`Memoire::ajouter`]).
//!
//! Un consommateur qui possède son propre arbre — c'est le cas de `this-vfs`
//! avec son `trait Vfs`, dont les opérations sont asynchrones — n'a pas besoin
//! d'un adaptateur ici : il matérialise son arbre dans [`Memoire`] (ou
//! implémente [`Source`] lui-même) et appelle
//! [`crate::index::index_repository_depuis`]. Le contrat reste synchrone parce
//! que l'indexation est un travail par lots, qui ne rend pas la main avant
//! d'avoir fini.
//!
//! Ce que la source ne dit PAS, et qui reste au disque : git. [`Source::racine_disque`]
//! rend la racine réelle quand il y en a une, et `None` sinon — auquel cas
//! l'indexeur n'écrit aucun nœud `Branch`, puisqu'un VFS n'a pas d'historique
//! git à tresser.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Une entrée de répertoire, réduite à ce dont l'indexeur a besoin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entree {
    /// Chemin complet, tel que construit depuis la racine demandée.
    pub chemin: PathBuf,
    /// Dernier segment — c'est sur lui que [`crate::discovery`] décide.
    pub nom: String,
    pub est_repertoire: bool,
    pub est_fichier: bool,
    /// Taille en octets (0 pour un répertoire). Sert au budget
    /// `max_file_bytes`, décidé sans lire le fichier.
    pub taille: u64,
}

impl Entree {
    fn nouveau(chemin: PathBuf, est_repertoire: bool, taille: u64) -> Self {
        let nom = chemin
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            chemin,
            nom,
            est_repertoire,
            est_fichier: !est_repertoire,
            taille,
        }
    }
}

/// Ce qui empêche une source de répondre. Volontairement pauvre : l'indexeur
/// n'agit sur aucune erreur de lecture, il passe au fichier suivant.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("lecture impossible de {chemin} : {source}")]
    Io {
        chemin: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Autre(String),
}

impl SourceError {
    fn io(chemin: &Path, source: std::io::Error) -> Self {
        Self::Io {
            chemin: chemin.to_path_buf(),
            source,
        }
    }
}

/// Un arbre que l'indexeur peut parcourir et lire.
///
/// `Send + Sync` : `index_repository_depuis` lit les fichiers depuis les
/// workers rayon, donc la source est partagée entre threads.
pub trait Source: Send + Sync {
    /// Les entrées d'un répertoire. Une erreur est tolérée par l'appelant :
    /// le répertoire est ignoré, ce qui est le comportement historique sur un
    /// disque (permissions, lien mort, course avec une suppression).
    fn lister(&self, repertoire: &Path) -> Result<Vec<Entree>, SourceError>;

    /// Le contenu d'un fichier, tel quel.
    fn lire(&self, chemin: &Path) -> Result<Vec<u8>, SourceError>;

    /// La racine sur disque, quand la source EST un disque.
    ///
    /// Sert de seule autorisation pour les traitements qui ne peuvent pas
    /// s'appliquer à un arbre virtuel : capture du contexte git, historique
    /// git. `None` ⇒ aucun nœud `Branch`, aucune arête d'historique.
    fn racine_disque(&self) -> Option<&Path> {
        None
    }
}

/// Le système de fichiers. C'est la source par défaut, et le comportement
/// d'avant l'existence de ce module.
///
/// Elle porte sa racine parce que c'est la seule à pouvoir autoriser les
/// traitements git : [`Source::racine_disque`] doit répondre `Some` ici, et
/// `None` sur un arbre virtuel.
#[derive(Debug, Clone)]
pub struct Disque {
    racine: PathBuf,
}

impl Disque {
    pub fn new(racine: impl Into<PathBuf>) -> Self {
        Self {
            racine: racine.into(),
        }
    }
}

impl Source for Disque {
    fn lister(&self, repertoire: &Path) -> Result<Vec<Entree>, SourceError> {
        let lecteur = std::fs::read_dir(repertoire).map_err(|e| SourceError::io(repertoire, e))?;
        let mut entrees = Vec::new();
        for entree in lecteur.flatten() {
            let chemin = entree.path();
            // `file_type` peut échouer (lien cassé) : l'entrée est alors ni
            // répertoire ni fichier, donc ignorée plus haut.
            let type_ = match entree.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let est_repertoire = type_.is_dir();
            if !est_repertoire && !type_.is_file() {
                continue;
            }
            // La taille est demandée ici, pas au moment du budget : un `stat`
            // par entrée, fait une fois.
            let taille = if est_repertoire {
                0
            } else {
                entree.metadata().map(|m| m.len()).unwrap_or(0)
            };
            entrees.push(Entree::nouveau(chemin, est_repertoire, taille));
        }
        Ok(entrees)
    }

    fn lire(&self, chemin: &Path) -> Result<Vec<u8>, SourceError> {
        std::fs::read(chemin).map_err(|e| SourceError::io(chemin, e))
    }

    fn racine_disque(&self) -> Option<&Path> {
        Some(&self.racine)
    }
}

/// Un arbre en mémoire : chemins → octets. Les répertoires ne sont pas
/// stockés, ils se déduisent des chemins de fichiers.
///
/// C'est la forme que prend un VFS quand on le présente à l'indexeur : on
/// énumère ses entrées une fois, et l'indexation travaille ensuite sans
/// rappeler le backend.
#[derive(Debug, Clone, Default)]
pub struct Memoire {
    fichiers: BTreeMap<PathBuf, Vec<u8>>,
}

impl Memoire {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute (ou remplace) un fichier. Le chemin est pris tel quel : c'est
    /// lui que l'indexeur verra, donc c'est lui qui décide des noms.
    pub fn ajouter(&mut self, chemin: impl Into<PathBuf>, contenu: impl Into<Vec<u8>>) {
        self.fichiers.insert(chemin.into(), contenu.into());
    }

    /// Nombre de fichiers en mémoire.
    pub fn len(&self) -> usize {
        self.fichiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fichiers.is_empty()
    }

    /// Charge un arbre réel en mémoire.
    ///
    /// `fichiers` porte les chemins à charger — typiquement ce que
    /// [`crate::discovery::discover`] vient de retenir. Les chemins sont
    /// conservés tels quels, de sorte qu'une indexation en mémoire produise
    /// exactement les mêmes FQN que l'indexation sur disque.
    pub fn depuis_dossier(
        fichiers: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, SourceError> {
        let mut memoire = Self::new();
        for chemin in fichiers {
            let octets = std::fs::read(&chemin).map_err(|e| SourceError::io(&chemin, e))?;
            memoire.ajouter(chemin, octets);
        }
        Ok(memoire)
    }
}

impl Source for Memoire {
    fn lister(&self, repertoire: &Path) -> Result<Vec<Entree>, SourceError> {
        let mut entrees = BTreeMap::<PathBuf, Entree>::new();
        let mut repertoires = BTreeSet::<PathBuf>::new();

        for chemin in self.fichiers.keys() {
            if chemin.parent() == Some(repertoire) {
                entrees.insert(
                    chemin.clone(),
                    Entree::nouveau(chemin.clone(), false, self.fichiers[chemin].len() as u64),
                );
                continue;
            }
            // Sous-répertoire : on ne rend que le PREMIER segment sous
            // `repertoire`, comme le ferait `read_dir`.
            if let Ok(relatif) = chemin.strip_prefix(repertoire) {
                if let Some(premier) = relatif.components().next() {
                    let sous = repertoire.join(premier);
                    if sous != *chemin {
                        repertoires.insert(sous);
                    }
                }
            }
        }
        for sous in repertoires {
            entrees.insert(sous.clone(), Entree::nouveau(sous, true, 0));
        }
        Ok(entrees.into_values().collect())
    }

    fn lire(&self, chemin: &Path) -> Result<Vec<u8>, SourceError> {
        self.fichiers.get(chemin).cloned().ok_or_else(|| {
            SourceError::Autre(format!("absent de la source : {}", chemin.display()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memoire_liste_comme_un_disque() {
        let mut m = Memoire::new();
        m.ajouter("/r/a.rs", "fn a() {}");
        m.ajouter("/r/sous/b.rs", "fn b() {}");
        m.ajouter("/r/sous/profond/c.rs", "fn c() {}");

        let racine = m.lister(Path::new("/r")).unwrap();
        let noms: Vec<&str> = racine.iter().map(|e| e.nom.as_str()).collect();
        assert_eq!(noms, vec!["a.rs", "sous"]);
        assert!(racine.iter().any(|e| e.nom == "sous" && e.est_repertoire));

        let sous = m.lister(Path::new("/r/sous")).unwrap();
        let noms: Vec<&str> = sous.iter().map(|e| e.nom.as_str()).collect();
        assert_eq!(noms, vec!["b.rs", "profond"]);
    }

    #[test]
    fn memoire_donne_la_taille_sans_lire() {
        let mut m = Memoire::new();
        m.ajouter("/r/a.rs", vec![b'x'; 1234]);
        let entrees = m.lister(Path::new("/r")).unwrap();
        assert_eq!(entrees.len(), 1);
        assert_eq!(entrees[0].taille, 1234);
    }

    #[test]
    fn memoire_refuse_un_chemin_absent() {
        let m = Memoire::new();
        assert!(m.lire(Path::new("/r/absent.rs")).is_err());
    }

    #[test]
    fn le_disque_autorise_git_la_memoire_non() {
        assert_eq!(Disque::new("/r").racine_disque(), Some(Path::new("/r")));
        assert!(Memoire::new().racine_disque().is_none());
    }
}
