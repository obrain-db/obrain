//! Suppression des copies.
//!
//! * À la sortie normale du processus : `atexit` supprime les copies inscrites.
//! * Après une sortie brutale (Ctrl-C, `kill -9`, panne) : le lancement suivant
//!   supprime les copies dont le propriétaire est mort.
//!
//! « Le propriétaire est mort » se prouve par un VERROU, pas par un PID : un
//! PID peut être recyclé, ou appartenir à un autre espace de noms (conteneur).
//! Le propriétaire tient un `flock` exclusif sur `<copie>/.proprietaire` tant
//! qu'il vit ; le noyau le rend quand le processus meurt, de quelque façon que
//! ce soit. Un balayeur qui obtient ce verrou sait donc que personne ne tient
//! plus la copie. Le PID reste dans le nom du répertoire, pour les humains.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once, OnceLock};

use crate::copie::FICHIER_PROPRIETAIRE;
use crate::{PREFIXE_COPIE, PREFIXE_COPIE_GARDEE};

fn registre() -> &'static Mutex<Vec<PathBuf>> {
    static REGISTRE: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    REGISTRE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Inscrit `racine` pour suppression à la sortie du processus.
pub(crate) fn a_la_sortie(racine: PathBuf) {
    registre()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(racine);
    armer();
}

/// Retire `racine` du registre (elle vient d'être supprimée).
pub(crate) fn oublier(racine: &Path) {
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
        // SAFETY: `supprimer_tout` est une fonction `extern "C"` sans argument,
        // valide pour toute la vie du processus ; elle ne panique pas.
        let _ = unsafe { libc::atexit(supprimer_tout) };
    });
}

/// Hors Unix : pas d'`atexit` ni de `flock` ici — les copies de lecture
/// restent dans `TMPDIR` (documenté dans docs/BENCHMARKS.md).
#[cfg(not(unix))]
fn armer() {}

/// Crée `chemin` et y pose un verrou exclusif, tenu tant que le `File` rendu
/// vit.
///
/// # Errors
///
/// Création du fichier ou pose du verrou impossible.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn poser_verrou_proprietaire(chemin: &Path) -> Result<Option<File>, String> {
    use std::os::unix::io::AsRawFd;
    let fichier =
        File::create(chemin).map_err(|e| format!("verrou de copie {} : {e}", chemin.display()))?;
    // SAFETY: descripteur valide pendant l'appel.
    if unsafe { libc::flock(fichier.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "verrou de copie {} : {}",
            chemin.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(Some(fichier))
}

#[cfg(not(unix))]
pub(crate) fn poser_verrou_proprietaire(_chemin: &Path) -> Result<Option<File>, String> {
    Ok(None)
}

/// Le propriétaire de la copie `racine` est-il mort ? Vrai seulement si son
/// verrou a pu être pris (et il est aussitôt rendu, à la fermeture). Un
/// fichier de verrou absent ne prouve rien : la copie est laissée.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn proprietaire_mort(racine: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    let Ok(fichier) = File::open(racine.join(FICHIER_PROPRIETAIRE)) else {
        return false;
    };
    // SAFETY: descripteur valide pendant l'appel.
    unsafe { libc::flock(fichier.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(not(unix))]
pub(crate) fn proprietaire_mort(_racine: &Path) -> bool {
    false
}

/// Supprime de `TMPDIR` les copies `obrain-banc-…` dont le propriétaire est
/// mort. Une fois par processus.
pub(crate) fn balayer_les_residus() {
    static FAIT: Once = Once::new();
    FAIT.call_once(|| {
        let Ok(entrees) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        for entree in entrees.flatten() {
            let nom = entree.file_name();
            let Some(nom) = nom.to_str() else { continue };
            if !nom.starts_with(PREFIXE_COPIE) || nom.starts_with(PREFIXE_COPIE_GARDEE) {
                continue;
            }
            if proprietaire_mort(&entree.path()) {
                eprintln!(
                    "BANC : résidu d'un processus interrompu supprimé : {}",
                    entree.path().display()
                );
                let _ = std::fs::remove_dir_all(entree.path());
            }
        }
    });
}
