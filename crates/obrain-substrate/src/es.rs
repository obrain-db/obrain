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

    /// Ouvre un fichier existant en lecture, et en écriture si `ecriture` est
    /// vrai (réécritures en place de la migration).
    fn ouvrir_donnees(&self, chemin: &Path, ecriture: bool) -> io::Result<Box<dyn FichierDonnees>>;

    /// Ouvre (ou crée, sans tronquer) un fichier de VERROU.
    fn ouvrir_verrou(&self, chemin: &Path) -> io::Result<Box<dyn FichierVerrou>>;
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
    /// Poignée de synchronisation INDÉPENDANTE du verrou du journal (un `dup` du
    /// descripteur) : elle permet de fsyncer hors du verrou d'écriture, ce qui
    /// rend possible le fsync partagé des commits concurrents. `None` si le support
    /// ne peut pas la rendre ; l'appelant retombe alors sur `synchroniser` sous verrou.
    fn poignee_synchro(&self) -> io::Result<Option<PoigneeSynchro>> {
        Ok(None)
    }
}

/// Poignée de synchronisation d'un journal, utilisable hors de son verrou.
/// L'argument est `tout` (données + métadonnées si `true`, données sinon).
pub type PoigneeSynchro = std::sync::Arc<dyn Fn(bool) -> io::Result<()> + Send + Sync>;

/// Implémentation native de [`FichierJournal`] sur un `File`.
#[derive(Debug)]
pub struct JournalNatif {
    fichier: File,
}

impl FichierJournal for JournalNatif {
    fn poignee_synchro(&self) -> io::Result<Option<PoigneeSynchro>> {
        let clone = self.fichier.try_clone()?;
        Ok(Some(std::sync::Arc::new(move |tout: bool| {
            if tout {
                clone.sync_all()
            } else {
                clone.sync_data()
            }
        })))
    }

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

/// Un fichier ouvert en LECTURE-ÉCRITURE à décalage : les réécritures en place
/// de la migration de format (patch d'un slot, d'un drapeau d'en-tête).
pub trait FichierDonnees: Send + std::fmt::Debug {
    /// Taille courante du support.
    fn taille(&self) -> io::Result<u64>;
    /// Remplit `buf` depuis `debut` (lecture exacte, `UnexpectedEof` si trop court).
    fn lire_a(&self, debut: u64, buf: &mut [u8]) -> io::Result<()>;
    /// Écrit `buf` à `debut` (écriture exacte).
    fn ecrire_a(&mut self, debut: u64, buf: &[u8]) -> io::Result<()>;
    /// Synchronise. `tout = true` : données et métadonnées.
    fn synchroniser(&mut self, tout: bool) -> io::Result<()>;
}

/// Un verrou EXCLUSIF tenu tant que l'objet vit (ou jusqu'à [`liberer`]). Sur
/// natif : `flock` via `fs2`, comme les fences du checkpoint et du dictionnaire.
/// Le navigateur le réalisera par l'exclusivité d'un `SyncAccessHandle`.
pub trait FichierVerrou: Send + std::fmt::Debug {
    /// Tente le verrou sans attendre : `Ok(true)` si pris, `Ok(false)` si un pair
    /// le tient, `Err` si le support refuse le verrouillage.
    fn essayer_exclusif(&self) -> io::Result<bool>;
    /// Prend le verrou exclusif en bloquant.
    fn exclusif(&self) -> io::Result<()>;
    /// Rend le verrou explicitement (sinon : à la fermeture).
    fn liberer(&self) -> io::Result<()>;
}

/// Implémentation native de [`FichierDonnees`] sur un `File`.
#[derive(Debug)]
pub struct DonneesNatives {
    fichier: File,
}

impl FichierDonnees for DonneesNatives {
    fn taille(&self) -> io::Result<u64> {
        Ok(self.fichier.metadata()?.len())
    }

    fn lire_a(&self, debut: u64, buf: &mut [u8]) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt as _;
            self.fichier.read_exact_at(buf, debut)
        }
        #[cfg(not(unix))]
        {
            use std::io::{Read as _, Seek as _};
            let mut f = &self.fichier;
            f.seek(io::SeekFrom::Start(debut))?;
            f.read_exact(buf)
        }
    }

    fn ecrire_a(&mut self, debut: u64, buf: &[u8]) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt as _;
            self.fichier.write_all_at(buf, debut)
        }
        #[cfg(not(unix))]
        {
            use std::io::{Seek as _, Write as _};
            self.fichier.seek(io::SeekFrom::Start(debut))?;
            self.fichier.write_all(buf)
        }
    }

    fn synchroniser(&mut self, tout: bool) -> io::Result<()> {
        if tout {
            self.fichier.sync_all()
        } else {
            self.fichier.sync_data()
        }
    }
}

/// Implémentation native de [`FichierVerrou`] : `flock` via `fs2` (unix et
/// Windows). Absente sur wasm32, où `fs2` ne compile pas.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct VerrouNatif {
    fichier: File,
}

#[cfg(not(target_arch = "wasm32"))]
impl FichierVerrou for VerrouNatif {
    fn essayer_exclusif(&self) -> io::Result<bool> {
        use fs2::FileExt as _;
        match self.fichier.try_lock_exclusive() {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn exclusif(&self) -> io::Result<()> {
        use fs2::FileExt as _;
        self.fichier.lock_exclusive()
    }

    fn liberer(&self) -> io::Result<()> {
        self.fichier.unlock()
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

    fn ouvrir_donnees(&self, chemin: &Path, ecriture: bool) -> io::Result<Box<dyn FichierDonnees>> {
        let fichier = OpenOptions::new().read(true).write(ecriture).open(chemin)?;
        Ok(Box::new(DonneesNatives { fichier }))
    }

    fn ouvrir_verrou(&self, chemin: &Path) -> io::Result<Box<dyn FichierVerrou>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Mêmes options que les fences : lecture, écriture, création, SANS
            // troncature (le contenu d'un fichier de verrou n'a aucun sens).
            let fichier = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(chemin)?;
            Ok(Box::new(VerrouNatif { fichier }))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = chemin;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "verrou de fichier non disponible sur cette cible",
            ))
        }
    }
}

static NATIF: Natif = Natif;

/// L'espace de fichiers utilisé par le substrat sur cette cible.
#[must_use]
pub fn espace() -> &'static dyn EspaceFichiers {
    &NATIF
}
