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

    /// Ouvre (ou crée) un journal en ajout : le WAL.
    fn ouvrir_journal(&self, chemin: &Path) -> io::Result<Box<dyn FichierJournal>>;
}

/// Un journal ouvert en AJOUT (le WAL) : écrire à la fin, lire à décalage,
/// tronquer, synchroniser. Reproduit la sémantique de `File` ouvert avec
/// `append(true)` : chaque écriture va à la fin, atomiquement.
pub trait FichierJournal: Send + std::fmt::Debug {
    /// Ajoute `buf` à la fin (un seul appel système, nombre d'octets rendu).
    fn ajouter(&mut self, buf: &[u8]) -> io::Result<usize>;
    /// Taille courante, mesurée sur le support.
    fn taille(&self) -> io::Result<u64>;
    /// Tronque ou raccourcit à `longueur` (jamais un allongement par l'appelant).
    fn tronquer(&mut self, longueur: u64) -> io::Result<()>;
    /// Remplit `buf` depuis `debut` (lecture exacte à décalage, sans curseur).
    fn lire_a(&self, debut: u64, buf: &mut [u8]) -> io::Result<()>;
    /// Ramène le curseur au début (sans effet sur les ajouts).
    fn rembobiner(&mut self) -> io::Result<()>;
    /// Synchronise. `tout = true` : données et métadonnées ; `false` : données.
    fn synchroniser(&mut self, tout: bool) -> io::Result<()>;
}

/// Implémentation native de [`FichierJournal`] sur un `File`.
#[derive(Debug)]
pub struct JournalNatif {
    fichier: File,
}

impl FichierJournal for JournalNatif {
    fn ajouter(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.fichier.write(buf)
    }

    fn taille(&self) -> io::Result<u64> {
        Ok(self.fichier.metadata()?.len())
    }

    fn tronquer(&mut self, longueur: u64) -> io::Result<()> {
        self.fichier.set_len(longueur)
    }

    fn lire_a(&self, debut: u64, buf: &mut [u8]) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt as _;
            self.fichier.read_exact_at(buf, debut)
        }
        #[cfg(not(unix))]
        {
            let _ = (debut, buf);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "lecture à décalage non disponible sur cette cible",
            ))
        }
    }

    fn rembobiner(&mut self) -> io::Result<()> {
        use std::io::Seek as _;
        self.fichier.seek(io::SeekFrom::Start(0)).map(|_| ())
    }

    fn synchroniser(&mut self, tout: bool) -> io::Result<()> {
        if tout {
            self.fichier.sync_all()
        } else {
            self.fichier.sync_data()
        }
    }
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

    fn ouvrir_journal(&self, chemin: &Path) -> io::Result<Box<dyn FichierJournal>> {
        // Mêmes options que le WAL : lecture, ajout, création. Le texte exact
        // compte : `append` est ce qui rend chaque écriture atomique vis-à-vis des
        // autres processus (voir `wal_io`).
        let fichier = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(chemin)?;
        Ok(Box::new(JournalNatif { fichier }))
    }
}

static NATIF: Natif = Natif;

/// L'espace de fichiers utilisé par le substrat sur cette cible.
#[must_use]
pub fn espace() -> &'static dyn EspaceFichiers {
    &NATIF
}
