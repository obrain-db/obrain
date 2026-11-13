//! La copie d'une base désignée : ce qui permet de ne jamais l'ouvrir.
//!
//! Une copie n'est prise que si elle peut être **juste** et **supportable** :
//!
//! * **juste** — personne ne tient la base (verrou `substrate.dirlock` du
//!   moteur), et rien n'y a changé entre le début et la fin de la copie ;
//!   aucun lien symbolique ni fichier spécial ; la copie est inscriptible ;
//! * **supportable** — clone copy-on-write quand le système de fichiers le
//!   permet (`clonefile` sur APFS, `FICLONE` sur Btrfs/XFS) ; sinon copie
//!   **creuse** (les trous restent des trous), et seulement si la taille
//!   physique reste sous un plafond et laisse une marge de disque libre.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use crate::nettoyage;

/// Plafond de la taille physique d'une copie PLEINE (Gio). Un clone n'est pas
/// plafonné : il ne consomme presque rien.
pub const VARIABLE_COPIE_MAX_GO: &str = "OBRAIN_BENCH_COPIE_MAX_GO";
/// Disque libre à laisser après une copie (Gio).
pub const VARIABLE_MARGE_GO: &str = "OBRAIN_BENCH_MARGE_GO";
/// `1` : interdit le clone et force la copie pleine (mesure, essais).
pub const VARIABLE_COPIE_PLEINE: &str = "OBRAIN_BENCH_COPIE_PLEINE";

const COPIE_MAX_GO_DEFAUT: u64 = 64;
const MARGE_GO_DEFAUT: u64 = 10;
const GIO: u64 = 1 << 30;

/// Seuils d'une copie.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limites {
    /// Plafond de la taille physique d'une copie pleine, en octets.
    pub copie_max: u64,
    /// Disque libre à laisser, en octets.
    pub marge: u64,
    /// Clone interdit.
    pub forcer_pleine: bool,
}

impl Limites {
    /// Seuils lus dans l'environnement, défauts sinon.
    pub(crate) fn depuis_l_environnement() -> Self {
        let go = |cle: &str, defaut: u64| {
            std::env::var(cle)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(defaut)
        };
        Self {
            copie_max: go(VARIABLE_COPIE_MAX_GO, COPIE_MAX_GO_DEFAUT).saturating_mul(GIO),
            marge: go(VARIABLE_MARGE_GO, MARGE_GO_DEFAUT).saturating_mul(GIO),
            forcer_pleine: std::env::var_os(VARIABLE_COPIE_PLEINE).is_some_and(|v| v == "1"),
        }
    }
}

/// Comment la copie a été prise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Clone copy-on-write : blocs partagés avec l'original.
    Clone,
    /// Copie pleine et creuse : blocs neufs, trous préservés.
    Pleine,
}

/// Une copie prise. Le répertoire `racine` contient la base copiée (`chemin`)
/// et le verrou du propriétaire (`verrou`), tenu tant que la copie vit.
#[derive(Debug)]
pub(crate) struct Copie {
    pub racine: PathBuf,
    pub chemin: PathBuf,
    pub verrou: Option<File>,
    pub mode: Mode,
    pub octets_logiques: u64,
    pub octets_physiques_source: u64,
    pub octets_physiques_copie: u64,
    pub secondes: f64,
}

/// Nom du fichier, à la racine d'une copie, que son propriétaire tient
/// verrouillé tant qu'il vit (voir `nettoyage`).
pub(crate) const FICHIER_PROPRIETAIRE: &str = ".proprietaire";

/// Pause, en millisecondes, entre la copie et ses contrôles d'après copie —
/// posée par les tests de ce dépôt (`essais::pause_apres_copie`) pour y
/// glisser une course. Zéro hors essai.
pub(crate) static PAUSE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pause_d_essai() {
    let ms = PAUSE_MS.load(std::sync::atomic::Ordering::Relaxed);
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Copie `source` dans un nouveau répertoire de `TMPDIR`.
///
/// # Errors
///
/// Un message nommé, la copie étant alors entièrement supprimée : base tenue
/// par un processus, base modifiée pendant la copie, lien symbolique ou
/// fichier spécial, plafond ou marge de disque dépassés, erreur d'E/S.
pub(crate) fn copier_base(
    source: &Path,
    nom: &str,
    prefixe: &str,
    limites: Limites,
) -> Result<Copie, String> {
    nettoyage::balayer_les_residus();
    if let Some(detenteur) = detenteur(source)? {
        return Err(format!(
            "la base {} est TENUE par un processus vivant ({detenteur}) : une copie prise pendant \
             qu'il écrit serait incohérente. Arrêter ce processus, ou désigner un instantané.",
            source.display()
        ));
    }
    let avant = etat_metadonnees(source)?;
    let (logiques, physiques) = tailles(&avant);

    let racine = tempfile::Builder::new()
        .prefix(prefixe)
        .tempdir()
        .map_err(|e| format!("création du répertoire de copie : {e}"))?
        .keep();
    let resultat = (|| {
        let verrou = nettoyage::poser_verrou_proprietaire(&racine.join(FICHIER_PROPRIETAIRE))?;
        let chemin = racine.join(nom);
        let mode = choisir_mode(source, &racine, limites)?;
        if mode == Mode::Pleine {
            exiger_place(&racine, physiques, limites)?;
        } else {
            // Un clone ne coûte presque rien, mais le moteur réécrira des blocs.
            exiger_place(&racine, 0, limites)?;
        }
        let debut = Instant::now();
        copier_arborescence(source, &chemin, mode)
            .map_err(|e| format!("copie de {} : {e}", source.display()))?;
        let secondes = debut.elapsed().as_secs_f64();
        pause_d_essai();

        if let Some(detenteur) = detenteur(source)? {
            return Err(format!(
                "la base {} a été OUVERTE pendant la copie ({detenteur}) : copie abandonnée",
                source.display()
            ));
        }
        let apres = etat_metadonnees(source)?;
        if let Some(ecart) = premier_ecart(&avant, &apres) {
            return Err(format!(
                "la base {} a CHANGÉ pendant la copie ({ecart}) : copie abandonnée, elle serait \
                 incohérente. Désigner un instantané.",
                source.display()
            ));
        }
        let octets_physiques_copie = tailles(&etat_metadonnees(&chemin)?).1;
        Ok(Copie {
            racine: racine.clone(),
            chemin,
            verrou,
            mode,
            octets_logiques: logiques,
            octets_physiques_source: physiques,
            octets_physiques_copie,
            secondes,
        })
    })();
    if resultat.is_err() {
        let _ = fs::remove_dir_all(&racine);
    }
    resultat
}

// ─────────────────────── Détection d'un détenteur ──────────────────────────

/// Qui tient la base ? Le moteur pose un verrou d'enregistrement POSIX
/// (`fcntl`) sur `substrate.dirlock` pendant toute la vie d'une ouverture :
/// exclusif pour un écrivain, partagé pour un lecteur ou un écrivain partagé.
/// On demande au noyau s'il existe un verrou incompatible avec un verrou
/// exclusif — donc N'IMPORTE QUEL verrou — sans en poser, sur un descripteur
/// ouvert en LECTURE SEULE : rien n'est écrit.
///
/// Un lecteur compte aussi : une ouverture en lecture seule écrit (voir la
/// documentation du crate).
///
/// ── DEUX PIÈGES DES VERROUS `fcntl`, ET CE QUI LES FERME ──────────────────
///
/// 1. `F_GETLK` est AVEUGLE aux verrous du processus appelant : un test qui
///    tient la base et en demande une copie verrait « personne ». On
///    interroge donc avec `F_OFD_GETLK` (verrous par description de fichier
///    ouverte, Linux et macOS) : un verrou POSIX du processus appelant est
///    alors un conflit comme un autre, et le noyau en rend le pid — le nôtre.
/// 2. Un verrou `fcntl` TOMBE dès que le processus ferme N'IMPORTE QUEL
///    descripteur du fichier. Ouvrir `substrate.dirlock` pour l'interroger,
///    puis le fermer, LÂCHERAIT le verrou exclusif que le moteur de ce
///    processus tiendrait sur la base (prouvé : un second écrivain, refusé
///    avant, ouvrait après). Le descripteur d'interrogation n'est donc JAMAIS
///    fermé : un par base et par processus, gardé jusqu'à la sortie.
///
/// Sur un Unix sans `F_OFD_GETLK`, on se rabat sur `F_GETLK`, aveugle au
/// processus appelant ; la comparaison avant/après la copie reste.
#[cfg(unix)]
#[allow(unsafe_code)]
fn detenteur(source: &Path) -> Result<Option<String>, String> {
    use std::os::unix::io::AsRawFd;
    use std::sync::{Mutex, OnceLock};
    let verrou = source.join("substrate.dirlock");
    if !verrou.exists() {
        // Le moteur le crée à sa première ouverture : une base sans ce
        // fichier n'a jamais été ouverte par un moteur qui le pose. Un moteur
        // ANTÉRIEUR pourrait la tenir sans qu'on puisse le voir : on le dit, et
        // seule la comparaison avant/après la copie protège.
        eprintln!(
            "BANC : {} n'a pas de substrate.dirlock — aucun détenteur ne peut être cherché ; seule \
             la comparaison de la base avant et après la copie protège",
            source.display()
        );
        return Ok(None);
    }
    // Descripteurs d'interrogation, gardés jusqu'à la sortie du processus
    // (piège 2). Clé : le chemin canonique du fichier de verrou.
    static DESCRIPTEURS: OnceLock<Mutex<std::collections::HashMap<PathBuf, File>>> =
        OnceLock::new();
    let cle = fs::canonicalize(&verrou).unwrap_or_else(|_| verrou.clone());
    let mut table = DESCRIPTEURS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fichier = match table.entry(cle) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => e.insert(
            File::open(&verrou)
                .map_err(|err| format!("lecture de {} : {err}", verrou.display()))?,
        ),
    };
    // SAFETY: `libc::flock` est une structure POD, initialisée à zéro puis
    // remplie ; `fcntl` la lit et l'écrit pendant l'appel, sans en garder de
    // pointeur. `l_pid` vaut 0 : exigé par `F_OFD_GETLK`.
    let mut requete: libc::flock = unsafe { std::mem::zeroed() };
    requete.l_type = libc::F_WRLCK as libc::c_short;
    requete.l_whence = libc::SEEK_SET as libc::c_short;
    requete.l_start = 0;
    requete.l_len = 0;
    requete.l_pid = 0;
    let rc = unsafe { libc::fcntl(fichier.as_raw_fd(), COMMANDE_GETLK, &raw mut requete) };
    if rc == -1 {
        return Err(format!(
            "interrogation du verrou {} : {}",
            verrou.display(),
            io::Error::last_os_error()
        ));
    }
    if requete.l_type == libc::F_UNLCK as libc::c_short {
        return Ok(None);
    }
    let nature = if requete.l_type == libc::F_WRLCK as libc::c_short {
        "verrou exclusif : un écrivain"
    } else {
        "verrou partagé : un lecteur ou un écrivain partagé"
    };
    let pid = requete.l_pid;
    let qui = if pid != 0 && u32::try_from(pid).is_ok_and(|p| p == std::process::id()) {
        format!("CE processus, pid {pid} — fermer la base avant d'en demander une copie")
    } else if pid == -1 {
        "un verrou par description de fichier, pid inconnu".to_owned()
    } else {
        format!("pid {pid}")
    };
    Ok(Some(format!("{qui}, {nature}")))
}

/// `F_OFD_GETLK` quand le système le fournit (piège 1), `F_GETLK` sinon.
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
const COMMANDE_GETLK: libc::c_int = libc::F_OFD_GETLK;
#[cfg(all(
    unix,
    not(any(target_vendor = "apple", target_os = "linux", target_os = "android"))
))]
const COMMANDE_GETLK: libc::c_int = libc::F_GETLK;

/// Hors Unix, le moteur verrouille par `LockFileEx` ; ce helper ne l'interroge
/// pas encore : seule la comparaison avant/après protège (documenté).
#[cfg(not(unix))]
fn detenteur(_source: &Path) -> Result<Option<String>, String> {
    Ok(None)
}

// ─────────────────────── État de la source ─────────────────────────────────

/// Taille logique, taille physique et date de chaque entrée de la source.
type Etat = Vec<(PathBuf, u64, u64, Option<SystemTime>)>;

fn etat_metadonnees(racine: &Path) -> Result<Etat, String> {
    let mut etat = Vec::new();
    parcourir(racine, racine, &mut etat).map_err(|e| format!("{e}"))?;
    etat.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(etat)
}

fn parcourir(racine: &Path, dossier: &Path, etat: &mut Etat) -> io::Result<()> {
    for entree in fs::read_dir(dossier)? {
        let chemin = entree?.path();
        let meta = fs::symlink_metadata(&chemin)?;
        let relatif = chemin.strip_prefix(racine).unwrap_or(&chemin).to_path_buf();
        refuser_l_inattendu(&chemin, &meta)?;
        if meta.is_dir() {
            parcourir(racine, &chemin, etat)?;
        } else {
            etat.push((relatif, meta.len(), physique(&meta), meta.modified().ok()));
        }
    }
    Ok(())
}

/// Une base ne contient que des répertoires et des fichiers ordinaires. Un lien
/// symbolique pourrait boucler, pointer hors de la base, ou être cassé : il est
/// refusé, nommé, plutôt que suivi.
fn refuser_l_inattendu(chemin: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let genre = meta.file_type();
    if genre.is_symlink() {
        return Err(io::Error::other(format!(
            "lien symbolique refusé dans une base : {} (une base ne contient que des fichiers \
             ordinaires ; copier la cible à la main si c'est voulu)",
            chemin.display()
        )));
    }
    if !genre.is_dir() && !genre.is_file() {
        return Err(io::Error::other(format!(
            "fichier spécial refusé dans une base : {}",
            chemin.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn physique(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn physique(meta: &fs::Metadata) -> u64 {
    meta.len()
}

fn tailles(etat: &Etat) -> (u64, u64) {
    etat.iter()
        .fold((0, 0), |(l, p), (_, lo, ph, _)| (l + lo, p + ph))
}

fn premier_ecart(avant: &Etat, apres: &Etat) -> Option<String> {
    if avant.len() != apres.len() {
        return Some(format!(
            "{} fichiers avant, {} après",
            avant.len(),
            apres.len()
        ));
    }
    avant
        .iter()
        .zip(apres)
        .find(|(a, b)| a.0 != b.0 || a.1 != b.1 || a.3 != b.3)
        .map(|(a, _)| format!("{} modifié", a.0.display()))
}

// ─────────────────────── Place et plafond ──────────────────────────────────

fn exiger_place(dossier: &Path, besoin: u64, limites: Limites) -> Result<(), String> {
    if besoin > limites.copie_max {
        return Err(format!(
            "copie PLEINE de {} Gio refusée : au-delà du plafond de {} Gio ({VARIABLE_COPIE_MAX_GO}). {} \
             Relever le plafond, ou placer TMPDIR sur le volume de la base.",
            besoin.div_ceil(GIO),
            limites.copie_max / GIO,
            if limites.forcer_pleine {
                "Le clone est interdit par OBRAIN_BENCH_COPIE_PLEINE."
            } else {
                "Le clone n'est pas disponible ici (autre volume, ou système de fichiers sans copy-on-write)."
            }
        ));
    }
    let Some(libre) = disque_libre(dossier)? else {
        return Ok(());
    };
    if libre < besoin.saturating_add(limites.marge) {
        return Err(format!(
            "place insuffisante dans {} : {} Gio libres, la copie demande {} Gio et doit laisser \
             {} Gio de marge ({VARIABLE_MARGE_GO})",
            dossier.display(),
            libre / GIO,
            besoin.div_ceil(GIO),
            limites.marge / GIO
        ));
    }
    Ok(())
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn disque_libre(dossier: &Path) -> Result<Option<u64>, String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dossier.as_os_str().as_bytes())
        .map_err(|e| format!("chemin invalide {} : {e}", dossier.display()))?;
    // SAFETY: `statvfs` est un POD rempli par l'appel ; `c` est une chaîne C
    // valide pendant l'appel.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &raw mut s) } != 0 {
        return Err(format!(
            "statvfs {} : {}",
            dossier.display(),
            io::Error::last_os_error()
        ));
    }
    #[allow(clippy::useless_conversion)]
    let libre = u64::from(s.f_bavail).saturating_mul(u64::from(s.f_frsize));
    Ok(Some(libre))
}

#[cfg(not(unix))]
fn disque_libre(_dossier: &Path) -> Result<Option<u64>, String> {
    Ok(None)
}

// ─────────────────────── Clone ou copie creuse ─────────────────────────────

/// Clone si possible — sonde sur `substrate.meta`, le plus petit fichier
/// d'une base —, copie pleine sinon.
fn choisir_mode(source: &Path, racine: &Path, limites: Limites) -> Result<Mode, String> {
    if limites.forcer_pleine {
        return Ok(Mode::Pleine);
    }
    let sonde = racine.join(".sonde-clone");
    let mode = if cloner(&source.join("substrate.meta"), &sonde).is_ok() {
        Mode::Clone
    } else {
        Mode::Pleine
    };
    let _ = fs::remove_file(&sonde);
    Ok(mode)
}

fn copier_arborescence(source: &Path, destination: &Path, mode: Mode) -> io::Result<()> {
    fs::create_dir(destination)?;
    for entree in fs::read_dir(source)? {
        let entree = entree?;
        let depuis = entree.path();
        let vers = destination.join(entree.file_name());
        let meta = fs::symlink_metadata(&depuis)?;
        refuser_l_inattendu(&depuis, &meta)?;
        if meta.is_dir() {
            copier_arborescence(&depuis, &vers, mode)?;
        } else {
            let clone = mode == Mode::Clone && cloner(&depuis, &vers).is_ok();
            if !clone {
                let _ = fs::remove_file(&vers);
                copier_creux(&depuis, &vers)?;
            }
        }
        rendre_inscriptible(&vers, meta.is_dir())?;
    }
    rendre_inscriptible(destination, true)
}

/// Une base en `0444` (instantané en lecture seule) donnerait une copie en
/// `0444`, que le moteur ne pourrait pas ouvrir : la copie appartient au banc,
/// elle est rendue inscriptible par son propriétaire.
#[cfg(unix)]
fn rendre_inscriptible(chemin: &Path, dossier: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut droits = fs::metadata(chemin)?.permissions();
    droits.set_mode(droits.mode() | if dossier { 0o700 } else { 0o600 });
    fs::set_permissions(chemin, droits)
}

#[cfg(not(unix))]
fn rendre_inscriptible(chemin: &Path, _dossier: bool) -> io::Result<()> {
    let mut droits = fs::metadata(chemin)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    droits.set_readonly(false);
    fs::set_permissions(chemin, droits)
}

/// Clone copy-on-write de `depuis` vers `vers` (qui ne doit pas exister).
#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
fn cloner(depuis: &Path, vers: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    /// `CLONE_NOFOLLOW` de `<sys/clonefile.h>` (absent du crate `libc`).
    const CLONE_NOFOLLOW: u32 = 0x0001;
    let d = std::ffi::CString::new(depuis.as_os_str().as_bytes())?;
    let v = std::ffi::CString::new(vers.as_os_str().as_bytes())?;
    // SAFETY: deux chaînes C valides pendant l'appel.
    if unsafe { libc::clonefile(d.as_ptr(), v.as_ptr(), CLONE_NOFOLLOW) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[allow(unsafe_code)]
fn cloner(depuis: &Path, vers: &Path) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let source = File::open(depuis)?;
    let cible = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(vers)?;
    // SAFETY: deux descripteurs valides pendant l'appel.
    let rc = unsafe { libc::ioctl(cible.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) };
    if rc == 0 {
        Ok(())
    } else {
        let e = io::Error::last_os_error();
        drop(cible);
        let _ = fs::remove_file(vers);
        Err(e)
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn cloner(_depuis: &Path, _vers: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "clone indisponible",
    ))
}

/// Copie pleine qui PRÉSERVE LES TROUS : un bloc entièrement nul n'est pas
/// écrit, on avance le curseur ; la longueur est fixée à la fin. Les zones
/// pré-allouées du moteur (des mégaoctets de zéros par fichier) ne coûtent
/// donc pas de disque dans la copie.
fn copier_creux(depuis: &Path, vers: &Path) -> io::Result<()> {
    const BLOC: usize = 1 << 20;
    let mut source = File::open(depuis)?;
    let mut cible = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(vers)?;
    let mut tampon = vec![0u8; BLOC];
    let mut total = 0u64;
    loop {
        let lus = lire_plein(&mut source, &mut tampon)?;
        if lus == 0 {
            break;
        }
        if tampon[..lus].iter().all(|o| *o == 0) {
            cible.seek(SeekFrom::Current(i64::try_from(lus).unwrap_or(i64::MAX)))?;
        } else {
            cible.write_all(&tampon[..lus])?;
        }
        total += lus as u64;
    }
    cible.set_len(total)?;
    cible.sync_all()
}

fn lire_plein(source: &mut File, tampon: &mut [u8]) -> io::Result<usize> {
    let mut lus = 0;
    while lus < tampon.len() {
        match source.read(&mut tampon[lus..])? {
            0 => break,
            n => lus += n,
        }
    }
    Ok(lus)
}

/// Rapport d'une copie, pour la sortie d'erreur.
pub(crate) fn decrire(copie: &Copie, source: &Path) -> String {
    let mio = |o: u64| o as f64 / f64::from(1u32 << 20);
    format!(
        "copie de {} en {:.2} s — {} ; {:.1} Mio logiques, {:.1} Mio physiques à la source, \
         {:.1} Mio physiques dans la copie{}",
        source.display(),
        copie.secondes,
        match copie.mode {
            Mode::Clone => "CLONE copy-on-write",
            Mode::Pleine => "copie PLEINE (creuse)",
        },
        mio(copie.octets_logiques),
        mio(copie.octets_physiques_source),
        mio(copie.octets_physiques_copie),
        if copie.mode == Mode::Clone {
            " (blocs partagés avec la source : rien de neuf sur le disque)"
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cloner` rend `Ok` avec un clone fidèle, ou `Err` sans laisser de
    /// fichier partiel — selon ce que le système de fichiers permet (APFS :
    /// oui ; ext4, tmpfs : non ; Btrfs, XFS avec reflink : oui).
    #[test]
    fn cloner_reussit_fidelement_ou_echoue_proprement() {
        let tmp = tempfile::tempdir().unwrap();
        let depuis = tmp.path().join("source");
        let vers = tmp.path().join("clone");
        fs::write(&depuis, b"contenu a cloner").unwrap();
        match cloner(&depuis, &vers) {
            Ok(()) => assert_eq!(fs::read(&vers).unwrap(), b"contenu a cloner"),
            Err(e) => {
                eprintln!("clone indisponible ici : {e}");
                assert!(!vers.exists(), "un fichier partiel est resté après l'échec");
            }
        }
        // APFS sait cloner : on l'exige là où on sait que c'est vrai.
        if cfg!(target_vendor = "apple") {
            assert!(vers.is_file());
        }
    }
}
