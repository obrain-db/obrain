//! Espace de fichiers du substrat (P0 du RFC « persistance navigateur »).
//!
//! Toutes les opérations de fichier NON mappées du substrat passent par cette
//! interface : lecture entière, publication atomique, temporaires synchronisés,
//! synchronisation de répertoire. L'implémentation native [`Natif`] reprend mot
//! pour mot les séquences qui étaient écrites en ligne dans chaque module : aucun
//! changement de comportement, de durabilité ou de format.
//!
//! Le choix de l'implémentation est fait à la compilation par [`espace`]. Une
//! implémentation navigateur (OPFS, en worker) viendra sur cible wasm32 sans rien
//! changer aux appelants ; les zones mappées (`ZoneFile`) relèvent d'un autre
//! seam (`ZoneBacking`) et ne passent PAS par ici.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Opérations de fichier non mappées du substrat.
pub trait EspaceFichiers: Send + Sync + std::fmt::Debug {
    /// Lit tout le contenu d'un fichier.
    fn lire_tout(&self, chemin: &Path) -> io::Result<Vec<u8>>;

    /// Crée ou tronque `chemin`, écrit `octets`, puis synchronise les DONNÉES et
    /// les métadonnées du fichier (`sync_all`). Ne publie rien : c'est le temporaire
    /// d'une publication.
    fn ecrire_synchronise(&self, chemin: &Path, octets: &[u8]) -> io::Result<()>;

    /// Écrit `octets` dans `chemin` (crée ou tronque), sans synchronisation.
    fn ecrire_tout(&self, chemin: &Path, octets: &[u8]) -> io::Result<()>;

    /// Renomme `de` vers `vers` (atomique sur POSIX pour le même répertoire).
    fn renommer(&self, de: &Path, vers: &Path) -> io::Result<()>;

    /// Supprime un fichier.
    fn supprimer(&self, chemin: &Path) -> io::Result<()>;

    /// Synchronise un répertoire : rend durables les entrées qu'un `rename` y a
    /// créées. No-op hors Unix (le répertoire ne s'ouvre pas en écriture).
    fn synchroniser_repertoire(&self, repertoire: &Path) -> io::Result<()>;

    /// Le chemin existe-t-il ?
    fn existe(&self, chemin: &Path) -> bool;
}

/// Implémentation native : le système de fichiers de l'OS.
#[derive(Debug, Default, Clone, Copy)]
pub struct Natif;

impl EspaceFichiers for Natif {
    fn lire_tout(&self, chemin: &Path) -> io::Result<Vec<u8>> {
        fs::read(chemin)
    }

    fn ecrire_synchronise(&self, chemin: &Path, octets: &[u8]) -> io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(chemin)?;
        f.write_all(octets)?;
        f.sync_all()
    }

    fn ecrire_tout(&self, chemin: &Path, octets: &[u8]) -> io::Result<()> {
        fs::write(chemin, octets)
    }

    fn renommer(&self, de: &Path, vers: &Path) -> io::Result<()> {
        fs::rename(de, vers)
    }

    fn supprimer(&self, chemin: &Path) -> io::Result<()> {
        fs::remove_file(chemin)
    }

    fn synchroniser_repertoire(&self, repertoire: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            File::open(repertoire)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            let _ = repertoire;
            Ok(())
        }
    }

    fn existe(&self, chemin: &Path) -> bool {
        chemin.exists()
    }
}

static NATIF: Natif = Natif;

/// L'espace de fichiers utilisé par le substrat sur cette cible.
#[must_use]
pub fn espace() -> &'static dyn EspaceFichiers {
    &NATIF
}
