//! `SubstrateFile` — directory-based on-disk representation of a substrate.
//!
//! A substrate is a **directory** containing a fixed set of files, each mapping
//! one logical zone (see `docs/rfc/substrate/format-spec.md` §1):
//!
//! ```text
//!  <path>/
//!    substrate.meta          # 4 KiB — magic, version, counters
//!    substrate.nodes         # array of NodeRecord (32 B each)
//!    substrate.edges         # array of EdgeRecord (32 B each)
//!    substrate.props         # array of PropertyPage (4 KiB each)
//!    substrate.strings       # array of HeapPage (4 KiB each)
//!    substrate.tier0         # 16 B per node
//!    substrate.tier1         # 64 B per node
//!    substrate.tier2         # 768 B per node
//!    substrate.hilbert       # u32 × node_count
//!    substrate.community     # u32 × node_count
//!    substrate.wal           # append-only WAL (see wal-spec.md)
//!    substrate.checkpoint    # latest checkpoint marker
//! ```
//!
//! Every file except `substrate.wal` and `substrate.checkpoint` is memory-mapped
//! directly. Writers use a write-through path: mutate the mmap, then fsync via
//! `msync`. Durability is owned by the WAL.
//!
//! ## Safety
//!
//! `memmap2::Mmap::map()` and friends are `unsafe`: the OS guarantees memory
//! safety only if the backing file is not concurrently mutated by another
//! process or truncated in a way that shrinks the mapped region. We enforce
//! this by holding an exclusive file lock on `substrate.dirlock` for the
//! lifetime of the `SubstrateFile` — un verrou de PROCESSUS (`fcntl(F_SETLK)`
//! sur unix, voir `verrouiller_fichier`), so other processes cannot open the
//! same directory.
//!
//! (2026-08-21) Ce paragraphe a decrit pendant longtemps un verrou qui
//! N'EXISTAIT PAS : `fs2` n'etait meme pas une dependance de ce crate, et le
//! commentaire SAFETY d'`open_inner` disait le contraire de celui-ci — « the
//! directory is not locked against other processes yet ». La promesse etait en
//! tete de fichier, son dementi 433 lignes plus bas, et c'est cette promesse qui
//! justifiait le `#![allow(unsafe_code)]`. Le verrou est desormais reellement
//! pose ; les deux commentaires disent la meme chose.
//!
//! Within this process, we never truncate a mapped file — growth is always
//! append + remap. The `#[allow(unsafe_code)]` block is therefore scoped to
//! the `mmap_*` helpers, which document the invariants they require.

#![allow(unsafe_code)]

use crate::error::{SubstrateError, SubstrateResult};
use crate::meta::{META_FILE_SIZE, MetaHeader, SUBSTRATE_FORMAT_VERSION};
#[cfg(not(target_arch = "wasm32"))]
use memmap2::Advice;
use memmap2::{Mmap, MmapMut, MmapOptions};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

// ═══════════════════════════════════════════════════════════════════════════
//  VERROU DE RÉPERTOIRE — inter-processus dehors, partagé dedans
// ═══════════════════════════════════════════════════════════════════════════

/// Part du verrou d'un répertoire de substrat (exclusif ou partagé).
///
/// Le verrou OS (`flock` via `fs2`) vit aussi longtemps que le fichier qui le
/// porte ; ce fichier vit aussi longtemps que la dernière part. Un processus
/// tué rend tout : le noyau ferme les descripteurs, le `flock` part avec eux,
/// et aucun fichier de verrou résiduel n'est à nettoyer.
#[derive(Debug)]
pub(crate) struct VerrouRepertoire {
    chemin: PathBuf,
    /// `true` = verrou exclusif (écriture autorisée) ; `false` = verrou
    /// partagé (lecture seule). Le mode pilote aussi le partage intra-processus
    /// via le registre : une part partagée existante ne peut PAS être réutilisée
    /// par une demande exclusive (elle autoriserait un écrivain là où des
    /// lecteurs partagent déjà le répertoire).
    exclusif: bool,
    /// Porte le verrou. Jamais lu — c'est sa DURÉE DE VIE qui compte. `Option`
    /// pour que `Drop` puisse le fermer EXPLICITEMENT sous le mutex du registre
    /// (voir `Drop for VerrouRepertoire`) ; `None` seulement après ce `take`.
    fichier: Option<File>,
}

/// Verrou OS inter-processus — NATIFS SEULEMENT.
///
/// ── POURQUOI `fcntl(F_SETLK)` ET PLUS `flock` SUR UNIX (2026-10-31) ────────
///
/// `flock` porte sur la DESCRIPTION DE FICHIER OUVERTE. Or une description est
/// COPIÉE dans tout enfant engendré par ce processus (`fork`/`clone`, y compris
/// le `posix_spawn` de `std::process::Command`) et n'en disparaît qu'à son
/// `execve` (`O_CLOEXEC`). Pendant cette fenêtre, fermer NOTRE descripteur ne
/// rend PAS le verrou : l'enfant le tient encore. Un `open` qui suit
/// immédiatement un `drop` reçoit alors `EWOULDBLOCK` — sans détenteur
/// nommable, l'enfant ayant déjà exécuté quand `detecter_detenteur` lit `/proc`.
///
/// C'est ce que la CI voyait sur `simplex_durability` : trois tests « drop puis
/// réouverture » tombaient au hasard parce qu'un test voisin du même binaire
/// engendre des processus (`kill_9_pendant_ecriture_reouverture_saine`).
/// Mesuré le 2026-10-31 (n3, Linux) avec un programme minimal — un thread
/// verrouille/ferme/reverrouille, un autre engendre des enfants en boucle :
/// en `flock`, refus dès le premier reverrouillage ; en `fcntl`, 0 refus sur
/// 1,5 million de cycles et 8 000 enfants.
///
/// Un verrou `fcntl` (POSIX record lock) appartient au PROCESSUS : jamais
/// hérité par un enfant, rendu par le noyau quand le processus meurt (comme
/// avant). Sa contrepartie, absorbée par le registre : il tombe dès que le
/// processus ferme N'IMPORTE QUEL descripteur du fichier. D'où deux règles,
/// toutes deux tenues ici : un seul descripteur par répertoire et par
/// processus (`acquerir_verrou_repertoire`), fermé SOUS le mutex du registre
/// (`Drop for VerrouRepertoire`) pour qu'aucune nouvelle part ne pose son
/// verrou avant que l'ancien descripteur soit clos.
///
/// Sur wasm32 (navigateur/edge workers), il n'y a ni multi-process ni verrou
/// de fichier : le store est mono-instance en mémoire, et les ouvertures de
/// fichiers échouent de toute façon sans filesystem. Le no-op est donc
/// sémantiquement correct. Hors unix et wasm32 (Windows), `fs2` (`LockFileEx`).
#[cfg(unix)]
pub(crate) fn verrouiller_fichier(fichier: &File, exclusif: bool) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `libc::flock` est un POD, rempli ici avant usage ; `F_SETLK` lit
    // la structure pendant l'appel et n'en conserve aucun pointeur.
    let mut verrou: libc::flock = unsafe { std::mem::zeroed() };
    verrou.l_type = if exclusif {
        libc::F_WRLCK
    } else {
        libc::F_RDLCK
    } as libc::c_short;
    verrou.l_whence = libc::SEEK_SET as libc::c_short;
    verrou.l_start = 0;
    verrou.l_len = 0; // 0 = jusqu'à la fin du fichier, quelle que soit sa taille
    let rc = unsafe {
        libc::fcntl(
            fichier.as_raw_fd(),
            libc::F_SETLK,
            &verrou as *const libc::flock,
        )
    };
    if rc == -1 {
        // `EACCES` ou `EAGAIN` selon le système : un seul contrat pour
        // l'appelant, `WouldBlock` (celui de `acquerir_verrou_repertoire`).
        let e = std::io::Error::last_os_error();
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            e.to_string(),
        ));
    }
    Ok(())
}

#[cfg(all(not(unix), not(target_arch = "wasm32")))]
pub(crate) fn verrouiller_fichier(fichier: &File, exclusif: bool) -> std::io::Result<()> {
    use fs2::FileExt;
    // fs2 0.4 : les variantes try_* rendent des types d'erreur différents
    // (`Error` vs `std::fs::TryLockError`) — on normalise chaque branche
    // en `io::Error::WouldBlock` (le contrat de `acquerir_verrou_repertoire`).
    if exclusif {
        fichier
            .try_lock_exclusive()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e.to_string()))
    } else {
        fichier
            .try_lock_shared()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e.to_string()))
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn verrouiller_fichier(_fichier: &File, _exclusif: bool) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn deverrouiller_fichier(fichier: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: voir `verrouiller_fichier`.
    let mut verrou: libc::flock = unsafe { std::mem::zeroed() };
    verrou.l_type = libc::F_UNLCK as libc::c_short;
    verrou.l_whence = libc::SEEK_SET as libc::c_short;
    let rc = unsafe {
        libc::fcntl(
            fichier.as_raw_fd(),
            libc::F_SETLK,
            &verrou as *const libc::flock,
        )
    };
    if rc == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(not(unix), not(target_arch = "wasm32")))]
pub(crate) fn deverrouiller_fichier(fichier: &File) -> std::io::Result<()> {
    use fs2::FileExt;
    fichier.unlock()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn deverrouiller_fichier(_fichier: &File) -> std::io::Result<()> {
    Ok(())
}

impl Drop for VerrouRepertoire {
    fn drop(&mut self) {
        let mut table = registre_verrous()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        table.remove(&self.chemin);
        // Fermeture SOUS le mutex, pas après : un verrou de processus (`fcntl`)
        // tombe à la fermeture de N'IMPORTE QUEL descripteur du fichier. Si une
        // nouvelle part posait son verrou entre notre retrait de la table et
        // la fermeture de ce descripteur, cette fermeture lui rendrait le sien
        // — un écrivain qui se croit exclusif sans l'être. Avec le registre
        // tenu jusqu'ici, la nouvelle part n'ouvre son descripteur qu'après.
        drop(self.fichier.take());
    }
}

/// Cherche, dans `/proc`, les processus qui tiennent un descripteur sur ce
/// fichier — et le DIT dans l'erreur de verrou.
///
/// Rend une chaîne vide si rien n'est trouvé (ou si `/proc` n'est pas lisible,
/// ou sur une plateforme sans `/proc`) : ce diagnostic ne doit jamais faire
/// échouer l'ouverture, il ne fait que rendre un refus instruisible.
///
/// Forme rendue : ` (detenteur : pid 1234 (obrain))` — le nom du binaire en
/// clair, sans espace ni guillemet, pour rester lisible dans un journal.
#[cfg(target_os = "linux")]
fn detecter_detenteur(fichier: &Path) -> String {
    let cible = std::fs::canonicalize(fichier).unwrap_or_else(|_| fichier.to_path_buf());
    let Ok(procfs) = std::fs::read_dir("/proc") else {
        return String::new();
    };
    let mut trouves: Vec<String> = Vec::new();
    for entree in procfs.flatten() {
        let nom = entree.file_name();
        let Some(pid) = nom
            .to_str()
            .filter(|p| p.chars().all(|c| c.is_ascii_digit()))
        else {
            continue;
        };
        if pid == crate::wal_io::pid_portable().to_string() {
            continue; // nous-mêmes : c'est le cas « déjà ouvert ici », traité ailleurs
        }
        let fd_dir = entree.path().join("fd");
        let Ok(fds) = std::fs::read_dir(&fd_dir) else {
            continue;
        };
        let tient = fds.flatten().any(|fd| {
            std::fs::read_link(fd.path())
                .map(|c| c == cible)
                .unwrap_or(false)
        });
        if !tient {
            continue;
        }
        let commande = std::fs::read_to_string(entree.path().join("comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "?".to_string());
        trouves.push(format!("pid {pid} ({commande})"));
        if trouves.len() >= 4 {
            break; // au-delà, c'est du bruit : on nomme les premiers et on s'arrête
        }
    }
    if trouves.is_empty() {
        String::new()
    } else {
        format!(" (detenteur : {})", trouves.join(", "))
    }
}

/// macOS n'a pas de `/proc` : on demande à `lsof` (présent sur toute
/// installation, `/usr/sbin/lsof`) les PID qui tiennent le fichier. Chemin
/// d'ERREUR seulement — un refus de verrou est déjà un échec, le coût d'un
/// processus de plus n'y change rien, et nommer le détenteur est ce qui rend
/// le refus instruisible (mesuré le 01/11/2026 : `verrou_apres_drop` rouge
/// sur macOS parce que ce stub rendait une chaîne vide).
#[cfg(target_os = "macos")]
fn detecter_detenteur(fichier: &Path) -> String {
    let sortie = match std::process::Command::new("/usr/sbin/lsof")
        .args(["-F", "pc", "--"])
        .arg(fichier)
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(_) => return String::new(),
    };
    // `-F pc` : une ligne `p<pid>` puis une ligne `c<commande>` par processus
    // (`-t` est incompatible avec `-F`, mesuré avec lsof 4.91).
    let texte = String::from_utf8_lossy(&sortie.stdout);
    let moi = crate::wal_io::pid_portable().to_string();
    let mut trouves: Vec<String> = Vec::new();
    let mut pid: Option<String> = None;
    for ligne in texte.lines() {
        if let Some(p) = ligne.strip_prefix('p') {
            pid = (p != moi).then(|| p.to_string());
        } else if let (Some(p), Some(c)) = (pid.take(), ligne.strip_prefix('c')) {
            trouves.push(format!("pid {p} ({c})"));
            if trouves.len() >= 4 {
                break;
            }
        }
    }
    if trouves.is_empty() {
        String::new()
    } else {
        format!(" (detenteur : {})", trouves.join(", "))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detecter_detenteur(_fichier: &Path) -> String {
    String::new()
}

type TableVerrous = std::sync::Mutex<HashMap<PathBuf, std::sync::Weak<VerrouRepertoire>>>;

fn registre_verrous() -> &'static TableVerrous {
    static TABLE: std::sync::OnceLock<TableVerrous> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Acquiert — ou PARTAGE — le verrou du répertoire `chemin`.
///
/// ── POURQUOI UN REGISTRE, ET PAS UN `flock` NU ──
///
/// La section Safety de ce module a longtemps promis un verrou exclusif qui
/// n'existait pas : `fs2` n'était même pas une dépendance du crate, et le
/// commentaire SAFETY d'`open_inner` disait le contraire — « the directory is
/// not locked against other processes yet ». C'est pourtant cette promesse qui
/// justifiait le `allow(unsafe_code)` : memmap2 ne garantit la sûreté mémoire
/// que si le fichier n'est pas muté concurremment par un AUTRE processus.
///
/// Un `flock` nu a été posé le 2026-08-21. Il cassait la production, et le
/// test l'a montré tout de suite. `flock` porte sur la DESCRIPTION DE FICHIER
/// OUVERTE, pas sur le processus : deux ouvertures du même répertoire dans le
/// MÊME processus se refusaient l'une l'autre. Or le pool d'`obrain-tmn` en
/// fait, délibérément — il ouvre HORS du verrou de sa table pour ne pas
/// sérialiser les ouvertures, puis résout la collision en jetant le perdant
/// (« course bénigne », `obrain_db_pool.rs:714`). Le test de non-régression de
/// la famine du 2026-07-23 tombait immédiatement.
///
/// D'où ce registre : la première ouverture d'un répertoire dans ce processus
/// prend le `flock` ; les suivantes PARTAGENT la même part. Les autres
/// processus restent dehors — ce qui est le seul objet de l'invariant.
///
/// (2026-08-31) MODE PARTAGÉ READ-ONLY : `exclusif=false` pose un `flock`
/// partagé (`LOCK_SH`) au lieu d'exclusif — plusieurs processus peuvent lire
/// le même store simultanément (le contrat de `Config::read_only`, qui était
/// promis mais non tenu). Le registre refuse de réutiliser une part partagée
/// pour une demande exclusive : un écrivain ne doit jamais coexister avec des
/// lecteurs partageant déjà le répertoire dans CE processus.
///
/// Le chemin est CANONISÉ avant indexation : deux écritures différentes du
/// même répertoire (lien symbolique, `..`, chemin relatif) doivent tomber sur
/// la même entrée, sinon le registre croirait à deux répertoires distincts et
/// le second `flock` échouerait sur le premier.
///
/// ── CE QUE LE VERROU GARANTIT, EXACTEMENT ──────────────────────────────────
/// C'est un verrou CONSULTATIF de PROCESSUS sur `substrate.dirlock` :
///
/// * exclusif (`open`) contre partagé (`open_read_only`, `open_shared_write`) :
///   exclusion mutuelle ENTRE PROCESSUS. Un lecteur seul ne s'ouvre pas tant
///   qu'un écrivain exclusif vit, et inversement — l'écrivain exclusif étant le
///   seul à tronquer une zone ou à migrer un format, c'est ce qui rend la
///   projection d'un lecteur sûre ;
/// * partagé contre partagé : AUCUNE exclusion. Un lecteur seul coexiste avec
///   un ou plusieurs écrivains partagés VIVANTS et avec d'autres lecteurs. Le
///   verrou partagé ne garantit donc PAS l'absence d'écrivain ;
/// * dans un même processus, les ouvertures PARTAGENT la part : rien n'y est
///   exclu, sauf une demande exclusive sur une part partagée.
///
/// `creer = false` (lecture seule) : le fichier de verrou est ouvert SANS droit
/// d'écriture et n'est PAS créé. Une base qui ne l'a pas (antérieure au verrou
/// dédié) ne peut pas être protégée d'un écrivain exclusif : l'ouverture en
/// lecture seule est refusée, en disant quoi faire.
pub(crate) fn acquerir_verrou_repertoire(
    chemin: &Path,
    verrou_path: &Path,
    exclusif: bool,
    creer: bool,
) -> SubstrateResult<std::sync::Arc<VerrouRepertoire>> {
    let cle = std::fs::canonicalize(chemin).unwrap_or_else(|_| chemin.to_path_buf());
    let mut table = registre_verrous()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(faible) = table.get(&cle) {
        if let Some(part) = faible.upgrade() {
            // Même processus : on partage — SAUF demande exclusive sur une
            // part partagée existante (un écrivain à côté de lecteurs = viol).
            if !exclusif || part.exclusif {
                return Ok(part);
            }
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!(
                    "substrate {} is already open read-only in this process — cannot \
                     upgrade to an exclusive (write) lock. Close the read-only holders \
                     first.",
                    chemin.display()
                ),
            )));
        }
        table.remove(&cle); // part expirée entre son Drop et son retrait
    }

    // `.create(true)` : les stores antérieurs à ce verrou dédié n'ont pas encore
    // `substrate.dirlock` — on le crée au premier accès (fichier vide, contenu
    // jamais lu : seule sa durée de vie compte, pour porter le `flock`).
    let fichier = if creer {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(verrou_path)
            .map_err(SubstrateError::Io)?
    } else {
        debug_assert!(!exclusif, "une ouverture sans création est un lecteur");
        match File::open(verrou_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SubstrateError::LectureSeuleRefusee {
                    raison: format!(
                        "{} n'a pas de fichier de verrou `{}` (base antérieure au verrou \
                         dédié) et une ouverture en lecture seule ne le crée pas ; ouvrez \
                         la base une fois en écriture, puis rouvrez en lecture seule",
                        chemin.display(),
                        zone::DIR_LOCK
                    ),
                });
            }
            Err(e) => return Err(SubstrateError::Io(e)),
        }
    };
    let res = verrouiller_fichier(&fichier, exclusif);
    res.map_err(|e| {
        SubstrateError::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!(
                "substrate {} is already open by another process ({} lock on {} \
                 refused: {e}). Close the other holder first{}.{}",
                chemin.display(),
                if exclusif { "exclusive" } else { "shared" },
                verrou_path.display(),
                if exclusif {
                    " — note: read-only opens use a shared lock and can coexist"
                } else {
                    ""
                },
                // ── QUI TIENT LE VERROU, NOMMÉ DANS L'ERREUR ────────────────
                // Un refus de verrou sans détenteur nommé est un échec qu'on ne
                // peut pas instruire : deux exécutions d'une suite complète sont
                // tombées ici (`simplex_durability`) sans qu'on puisse dire QUI
                // tenait le `flock` — ni même si c'était un autre processus ou
                // une trace du précédent. Le système sait répondre (`/proc`) :
                // on le lui demande au moment du refus, et seulement là.
                detecter_detenteur(verrou_path)
            ),
        ))
    })?;

    let part = std::sync::Arc::new(VerrouRepertoire {
        chemin: cle.clone(),
        exclusif,
        fichier: Some(fichier),
    });
    table.insert(cle, std::sync::Arc::downgrade(&part));
    Ok(part)
}

/// Threshold below which `advise_huge_pages` refuses to apply the THP hint:
/// mappings smaller than this are too small to benefit from a 2 MB huge
/// page (they would waste TLB entries and fragment the address space).
/// 8 MB covers typical zone files on bootstrap while still letting growing
/// zones acquire huge-page backing as soon as they cross this threshold.
const HUGE_PAGE_MIN_BYTES: u64 = 8 * 1024 * 1024;

/// Cross-platform huge-page / Transparent-Huge-Pages hint for a mmap region.
///
/// * **Linux**: issues `madvise(MADV_HUGEPAGE)`. The kernel tries to back
///   the region with 2 MB pages where possible. Requires
///   `CONFIG_TRANSPARENT_HUGEPAGE=y`. If THP is disabled the call returns
///   `EINVAL` and we silently ignore it — the hint is advisory.
/// * **macOS / Windows / other**: no-op. Darwin's superpages require
///   allocation-time flags (`VM_FLAGS_SUPERPAGE_SIZE_2MB`) that memmap2
///   does not expose, and Windows large pages require a privileged token.
///   Both platforms fall back to default 4-16 KB pages without error.
///
/// The hint lives on the VMA, not the file descriptor, so this helper must
/// be invoked after every `map_mut` / remap. Zones smaller than
/// `HUGE_PAGE_MIN_BYTES` are skipped — they can't fill a single huge page.
///
/// ## Pilotage : `OBRAIN_TMN_HUGEPAGE` (défaut ON)
///
/// Poser `MADV_HUGEPAGE` inconditionnellement sur toute zone ≥ 8 Mio, re-posé à
/// chaque remap, fait exploser `FilePmdMapped` (les gros mappings des bases
/// live sont backés par des huge pages 2 Mo) : le reclaim/refault du noyau
/// travaille alors par blocs de 2 Mo au lieu de 4 Ko, ce qui AMPLIFIE le thrash
/// quand le processus vit collé à son plafond mémoire. Les huge pages restent
/// pourtant bénéfiques aux **balayages séquentiels** (hilbert/tiers, scan
/// complet) via le TLB. Ce compromis est donc rendu PILOTABLE au lieu d'être
/// gravé :
///
/// * absente / toute valeur autre que `0` ⇒ **ON** — le comportement d'origine,
///   INCHANGÉ par défaut ; une valeur inattendue retombe sur ON (le côté SÛR :
///   jamais de régression perf séquentielle silencieuse) ;
/// * `0` ⇒ **OFF** — aucune zone ne reçoit `MADV_HUGEPAGE`, on reste sur des
///   pages 4 Ko (idiome `OBRAIN_PROPS_INDEX_ORDONNE`, `map_or(true, v != "0")`).
///
/// Le défaut ON garantit qu'aucun déploiement ne change de comportement sans un
/// geste explicite : le gain (FilePmdMapped ↓ quand OFF) se MESURE au banc
/// (ON vs OFF) avec porte de non-régression (recall/query en ms) AVANT de
/// figer OFF. Suites possibles si le flag global s'avère trop grossier :
/// (b) conditionner par TYPE de zone (huge pages seulement sur les zones à accès
/// séquentiel — hilbert/veccol scanné en entier — pas sur props.v2 lu par clé) ;
/// (c) `MADV_NOHUGEPAGE` explicite sur les zones dispersées.
/// Vrai si le NOM de fichier est une zone props v2 (`substrate.props.v2` ou
/// `substrate.props.heap.v2`) : écrites par petits appends, lues par clé —
/// le THP n'y apporte rien et coûte à l'écriture (RFC multi-writer Levier 2,
/// 2026-09-06). Sans suffixe de séparateur : le nom est comparé tel quel.
fn est_zone_props_v2_nom(nom: &str) -> bool {
    nom == crate::props_zone::PROPS_V2_FILENAME || nom == crate::props_zone::PROPS_HEAP_V2_FILENAME
}

fn advise_huge_pages(map: &MmapMut, len: u64) {
    if !should_advise_huge_pages(len, huge_pages_enabled()) {
        // Sous le seuil, ou THP désactivé via OBRAIN_TMN_HUGEPAGE : on laisse le
        // mapping sur des pages 4 Ko (le comportement pré-hint). `let _ = map`
        // garde la liaison utilisée sur toutes les plateformes.
        let _ = map;
        return;
    }
    #[cfg(target_os = "linux")]
    {
        // Ignore the result: THP is a hint. Kernels without CONFIG_TRANSPARENT_HUGEPAGE
        // return EINVAL; we treat that as "no huge pages, back with regular 4K" which is
        // exactly the pre-hint behaviour.
        let _ = map.advise(Advice::HugePage);
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Keep the binding to silence the unused-var warning without cfg-annotating
        // the call site.
        let _ = map;
    }
}

/// Décision PURE : faut-il poser `MADV_HUGEPAGE` sur un mapping de `len` octets,
/// étant donné le flag `enabled` ? Les deux seuls leviers sont le seuil de
/// taille (`HUGE_PAGE_MIN_BYTES`) et le flag `OBRAIN_TMN_HUGEPAGE`. Isoler cette
/// décision de tout effet de bord la rend directement falsifiable au test.
fn should_advise_huge_pages(len: u64, enabled: bool) -> bool {
    enabled && len >= HUGE_PAGE_MIN_BYTES
}

/// Lit le flag `OBRAIN_TMN_HUGEPAGE`. Défaut **ON** : absente ou toute valeur
/// autre que `"0"` conserve le comportement d'origine ; une valeur inattendue
/// retombe donc sur ON, le côté SÛR (jamais de régression perf séquentielle
/// silencieuse). Seul `"0"` coupe les huge pages. Même idiome que
/// `OBRAIN_PROPS_INDEX_ORDONNE` (`store.rs`), défaut ON assumé.
///
/// NB : comme les autres flags du crate, il est lu au moment de l'advise (donc
/// à chaque `open_or_create`/`grow_to`+remap), pas via un cache à chaud ; en
/// pratique son effet se constate au PROCHAIN BOOT du service — on ne redémarre
/// pas le service vivant pour le mesurer.
fn huge_pages_enabled() -> bool {
    std::env::var("OBRAIN_TMN_HUGEPAGE").map_or(true, |v| v != "0")
}

/// Canonical names for the per-zone files inside a substrate directory.
pub mod zone {
    pub const META: &str = "substrate.meta";
    pub const NODES: &str = "substrate.nodes";
    pub const EDGES: &str = "substrate.edges";
    pub const PROPS: &str = "substrate.props";
    pub const STRINGS: &str = "substrate.strings";
    pub const TIER0: &str = "substrate.tier0";
    pub const TIER1: &str = "substrate.tier1";
    pub const TIER2: &str = "substrate.tier2";

    /// Nom sûr pour un fichier de zone dérivé d'un identifiant libre (label,
    /// propriété) : alphanumériques ASCII, `_` et `-` tels quels, tout le
    /// reste en `%xx` (octets UTF-8). Injectif : deux identifiants distincts
    /// donnent deux noms distincts, donc deux index ne se marchent pas dessus.
    pub fn nom_sur(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02x}"));
            }
        }
        out
    }

    /// Zone de palier NOMMÉE d'un index vectoriel `(label, propriété)` :
    /// `substrate.tier<niveau>.<label>.<propriété>`, `niveau` ∈ {0, 1, 2}.
    /// Les zones non nommées [`TIER0`]..[`TIER2`] restent celles de l'index
    /// unique historique de la base (`SubstrateStore::load_tier_index`,
    /// `examples/rebuild_tiers.rs`).
    pub fn tier_nomme(niveau: u8, label: &str, propriete: &str) -> String {
        format!(
            "substrate.tier{niveau}.{}.{}",
            nom_sur(label),
            nom_sur(propriete)
        )
    }

    pub const HILBERT: &str = "substrate.hilbert";
    pub const COMMUNITY: &str = "substrate.community";
    /// Side-column Z-order (Morton) — clé logique u32/slot sur le même plan
    /// (centrality × degree) que la colonne Hilbert (superposition « ordre Z »).
    pub const ZORDER: &str = "substrate.zorder";
    /// Grille de tri persistée `(order, max_degree)` du dernier bulk sort —
    /// interprétation des colonnes Hilbert/Zorder (RFC zorder-superposition).
    /// Fichier 12 o, hint : absent ⇒ pas d'accélération zruns (le compactage
    /// efface HILBERT_SORTED et ne met donc pas ce fichier à jour).
    pub const ZGRID: &str = "substrate.zgrid";
    pub const ENGRAM_MEMBERS: &str = "substrate.engram_members";
    pub const ENGRAM_BITSET: &str = "substrate.engram_bitset";
    /// Répertoire des simplexes — records fixes 32 B (T4, décision 2d461b89).
    pub const SIMPLEX_DIR: &str = "substrate.simplex_dir";
    /// Incidence sommet→simplexes — répertoire plat 16 B par slot de nœud.
    pub const SIMPLEX_INCIDENCE: &str = "substrate.simplex_incidence";
    pub const WAL: &str = "substrate.wal";
    pub const CHECKPOINT: &str = "substrate.checkpoint";
    /// Verrou de répertoire inter-processus. Fichier dédié STABLE, jamais
    /// renommé ni tronqué : le `flock` (exclusif = écriture, partagé = lecture)
    /// y est posé pour la vie du processus. C'est le point d'ancrage du verrou
    /// d'écrivain — SANS lui, poser le verrou sur `substrate.meta` le faisait
    /// porter sur un inode que `checkpoint::atomically_write_meta` REMPLACE
    /// (tmp + rename) à chaque checkpoint : après le rename, l'ancien détenteur
    /// tenait son `flock` sur un inode « deleted » tandis qu'un nouvel écrivain
    /// prenait un `flock` frais sur le nouvel inode → deux écrivains coexistants
    /// → WAL corrompu → messages perdus au replay (invariant mono-écrivain violé).
    pub const DIR_LOCK: &str = "substrate.dirlock";

    pub const ALL_MMAPPED: &[&str] = &[
        NODES,
        EDGES,
        PROPS,
        STRINGS,
        TIER0,
        TIER1,
        TIER2,
        HILBERT,
        COMMUNITY,
        ZORDER,
        ENGRAM_MEMBERS,
        ENGRAM_BITSET,
        SIMPLEX_DIR,
        SIMPLEX_INCIDENCE,
    ];
}

/// Enumeration of zones exposed as typed handles.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    Nodes,
    Edges,
    Props,
    Strings,
    Tier0,
    Tier1,
    Tier2,
    Hilbert,
    Community,
    /// Z-order (Morton) side-column — u32/slot, clé logique par préfixe sur
    /// le même plan (centrality × degree) que `Zone::Hilbert`. Remplie dans
    /// les mêmes passes (bulk sort, compactage) ; sémantique « hint »
    /// identique à Hilbert (cf. RFC zorder-superposition).
    Zorder,
    /// Engram-membership side-table (T7). Maps `engram_id: u16 → Vec<u32>`
    /// node slot IDs via an in-place directory + append-only payload.
    EngramMembers,
    /// Per-node 64-bit engram signature (T7 Step 1). One u64 per node slot,
    /// used as a Bloom-filter-like candidate prune for Hopfield recall.
    EngramBitset,
    /// Répertoire des simplexes (T4, décision 2d461b89 option B) :
    /// `SimplexRecord` fixes 32 B, `simplex_id` = index. Les membres et
    /// propriétés des hyperarêtes vivent dans le side-car nommé
    /// `substrate.simplex_heap` (voir `crate::simplex_zone`).
    SimplexDir,
    /// Incidence sommet→simplexes (T4) : répertoire plat 16 B par slot de
    /// nœud (tête/queue U48 + compteur), pages de postings 4 KiB dans le
    /// side-car nommé `substrate.simplex_postings` (voir
    /// `crate::simplex_incidence`). Lecture en O(degré simplicial).
    SimplexIncidence,
}

impl Zone {
    pub fn filename(self) -> &'static str {
        match self {
            Zone::Nodes => zone::NODES,
            Zone::Edges => zone::EDGES,
            Zone::Props => zone::PROPS,
            Zone::Strings => zone::STRINGS,
            Zone::Tier0 => zone::TIER0,
            Zone::Tier1 => zone::TIER1,
            Zone::Tier2 => zone::TIER2,
            Zone::Hilbert => zone::HILBERT,
            Zone::Community => zone::COMMUNITY,
            Zone::Zorder => zone::ZORDER,
            Zone::EngramMembers => zone::ENGRAM_MEMBERS,
            Zone::EngramBitset => zone::ENGRAM_BITSET,
            Zone::SimplexDir => zone::SIMPLEX_DIR,
            Zone::SimplexIncidence => zone::SIMPLEX_INCIDENCE,
        }
    }
}

// ---------------------------------------------------------------------------
// Mode MÉMOIRE — le substrat sans système de fichiers
// ---------------------------------------------------------------------------

/// Nom de répertoire SYNTHÉTIQUE du mode mémoire.
///
/// Il n'existe aucun répertoire de ce nom : toute E/S qui s'y risquerait échoue
/// proprement (`NotFound`), plutôt que d'écrire par accident dans le dépôt.
/// Sur wasm32, les E/S échouent de toute façon sans trapper.
const MEMORY_DIR: &str = "<memoire:obrain>";

/// Une zone qui vit en RAM, PARTAGÉE par toutes les poignées qui l'ouvrent.
///
/// POURQUOI CE TYPE EXISTE. `wasm32-unknown-unknown` n'a pas de système de
/// fichiers : `std::fs` y échoue et `std::env::temp_dir()` y panique. Le « mode
/// mémoire » du moteur était pourtant un substrat FICHIER dans un répertoire
/// temporaire (`SubstrateFile::open_tempfile`), donc aucune base ne pouvait
/// naître dans un navigateur — mesuré : `new Database()` trappait.
///
/// INVARIANT DE PARTAGE — le cœur de la décision. Deux poignées ouvertes sur la
/// MÊME zone voient les MÊMES octets : c'est ce que le mmap assure gratuitement
/// (cache de pages), et le code s'y appuie déjà — `blob_column` garde `idx`/
/// `dat`, `degree_column` garde `zf`, le `Writer` a un cache de zones. Des
/// tampons possédés par poignée divergeraient EN SILENCE : corruption, pas
/// erreur. D'où l'`Arc` ici et le registre de [`RamDir`].
#[derive(Debug)]
pub(crate) struct RamZone {
    /// Octets de la zone. `UnsafeCell` parce que les vues sont rendues depuis
    /// `&self`, comme `as_slice` sur un mmap, alors que plusieurs poignées
    /// partagent la même allocation.
    data: std::cell::UnsafeCell<Vec<u8>>,
}

// SAFETY: contrat identique à celui d'un mmap partagé entre poignées, que ce
// code pratique déjà — la sérialisation vient des verrous du substrat (un seul
// écrivain), et `wasm32-unknown-unknown` n'a aucun thread OS. Le mode mémoire
// n'est construit par le moteur QUE sur wasm ; sur natif il n'est atteint que
// par les tests unitaires qui en font leur objet.
unsafe impl Send for RamZone {}
unsafe impl Sync for RamZone {}

impl RamZone {
    fn with_len(len: usize) -> Self {
        Self {
            data: std::cell::UnsafeCell::new(vec![0u8; len]),
        }
    }

    fn len_u64(&self) -> u64 {
        // SAFETY: lire la longueur pendant qu'une autre poignée écrit serait
        // déjà un viol du contrat du substrat (voir `as_slice_mut`).
        unsafe { (*self.data.get()).len() as u64 }
    }

    /// Vue lecture.
    ///
    /// # Safety
    /// Même contrat qu'un mmap : l'appelant garantit qu'aucune vue MUTABLE
    /// n'est vivante sur cette zone en même temps.
    unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: délégué à l'appelant (cf. `# Safety`).
        unsafe { &*self.data.get() }
    }

    /// Vue écriture.
    ///
    /// # Safety
    /// Même contrat qu'un mmap : aucune autre vue (lecture ou écriture) ne doit
    /// être vivante sur cette zone pendant la durée de celle-ci.
    // Accès mmap : le contrat est dans `# Safety`, comme pour `memmap2`. Rendre
    // un `&mut` depuis un `&self` est le geste même d une vue écriture sur une
    // zone mappée ; l interdiction est ici explicite plutôt que silencieuse.
    #[allow(clippy::mut_from_ref)]
    unsafe fn as_slice_mut(&self) -> &mut [u8] {
        // SAFETY: délégué à l'appelant (cf. `# Safety`).
        unsafe { &mut *self.data.get() }
    }

    /// Croissance (ou rétrécissement) zéro-rempli — l'équivalent de
    /// `set_len` + remap.
    ///
    /// # Safety
    /// La réallocation DÉPLACE les octets : aucune vue ne doit être vivante.
    /// C'est exactement ce qu'exige déjà `ZoneFile::grow_to` en mode mmap, où le
    /// remap change l'adresse — le contrat n'est donc pas assoupli.
    unsafe fn resize(&self, new_len: u64) {
        // SAFETY: délégué à l'appelant (cf. `# Safety`).
        let v = unsafe { &mut *self.data.get() };
        v.resize(new_len as usize, 0);
    }
}

/// Le « répertoire » d'un substrat en mémoire : le registre des zones.
///
/// Il tient le rôle du répertoire du mode fichier, et surtout il rend à chaque
/// appel la MÊME `Arc<RamZone>` pour un même nom — sans quoi l'invariant de
/// partage décrit sur [`RamZone`] serait perdu.
#[derive(Debug, Default)]
struct RamDir {
    zones: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<RamZone>>>,
}

impl RamDir {
    /// Rend la zone `nom`, en la créant à `len_initial` si elle n'existe pas
    /// encore. Une zone déjà présente n'est JAMAIS redimensionnée ici : sa
    /// longueur appartient à ses poignées.
    fn zone(&self, nom: &str, len_initial: usize) -> std::sync::Arc<RamZone> {
        let mut garde = self
            .zones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        garde
            .entry(nom.to_string())
            .or_insert_with(|| std::sync::Arc::new(RamZone::with_len(len_initial)))
            .clone()
    }

    /// Noms des zones enregistrées — l'équivalent, en mémoire, d'un `read_dir`.
    /// Sert à l'instantané (`crate::instantane`), qui doit emporter TOUTES les
    /// zones, y compris les colonnes latérales créées à la volée.
    fn noms(&self) -> Vec<String> {
        let garde = self
            .zones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        garde.keys().cloned().collect()
    }

    /// Rend la zone `nom` SEULEMENT si elle existe déjà (ouverture en lecture).
    fn zone_existante(&self, nom: &str) -> Option<std::sync::Arc<RamZone>> {
        let garde = self
            .zones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        garde.get(nom).cloned()
    }
}

/// De quoi une `ZoneFile` est faite : un fichier mmapé (mode natif), ou des
/// octets en mémoire (mode sans système de fichiers).
#[derive(Debug)]
enum ZoneBacking {
    /// Fichier mmapé — comportement natif, strictement inchangé.
    Mmap { file: File, map: Option<MmapMut> },
    /// Zone en mémoire, partagée par le registre de [`RamDir`].
    Ram(std::sync::Arc<RamZone>),
    /// Zone d'une ouverture en LECTURE SEULE : fichier ouvert sans droit
    /// d'écriture, projeté en privé (copy-on-write), partagée par le registre
    /// de [`RegistrePrive`]. Rien de ce qui s'y écrit n'atteint le disque.
    Privee(std::sync::Arc<ZonePrivee>),
}

// ═══════════════════════════════════════════════════════════════════════════
//  ZONES D'UNE OUVERTURE EN LECTURE SEULE — privées, jamais écrites sur disque
// ═══════════════════════════════════════════════════════════════════════════

/// Au-delà de cette taille, une zone ADOSSÉE À UN FICHIER n'est pas recopiée en
/// mémoire pour grandir : l'ouverture en lecture seule est refusée (voir
/// [`ZonePrivee::grandir`]).
const PLAFOND_COPIE_PRIVEE: usize = 1 << 30;

/// Réserve d'adresses minimale d'une zone née en mémoire : des pages anonymes
/// jamais touchées ne coûtent ni RAM ni disque, et tant que la zone grandit
/// dans sa réserve elle ne DÉMÉNAGE pas.
const RESERVE_MEMOIRE_MIN: usize = 16 << 20;

/// Les octets d'une [`ZonePrivee`].
#[derive(Debug)]
enum OctetsPrives {
    /// Fichier absent ou vide : aucune projection.
    Vide,
    /// Projection PRIVÉE (`MAP_PRIVATE`, copy-on-write) d'un fichier ouvert en
    /// lecture seule. Une écriture ne modifie que la copie de la page dans CE
    /// processus ; le fichier et les autres processus ne la voient jamais.
    Fichier(MmapMut),
    /// Octets nés en mémoire (fichier absent, ou zone qui a dû grandir au-delà
    /// de son fichier) : pages anonymes, réservées plus large que `len`.
    Memoire(MmapMut),
}

#[derive(Debug)]
struct EtatPrive {
    octets: OctetsPrives,
    /// Longueur LOGIQUE de la zone (≤ taille de la projection).
    len: usize,
    /// Projections remplacées (croissance, reprojection). Elles ne sont
    /// détruites qu'avec la zone : une vue rendue par une AUTRE poignée avant le
    /// remplacement reste donc valide. Leur nombre est logarithmique (les
    /// fichiers grandissent ×1,5, les réserves doublent).
    retraites: Vec<MmapMut>,
}

/// Une zone vue par une ouverture en LECTURE SEULE.
///
/// LE CONTRAT, ET QUI LE TIENT. Ouvrir une base en lecture seule ne doit
/// modifier aucun octet d'aucun fichier. Ce n'est pas une discipline demandée
/// au code appelant : le fichier est ouvert SANS droit d'écriture, et il est
/// projeté en PRIVÉ. Le rejeu du WAL, la reconstruction d'une colonne de
/// degrés, le restampage d'une chaîne — tout ce que l'ouverture écrivait
/// autrefois dans les zones — écrit désormais dans des pages copiées à la
/// première écriture, propres à ce processus. `set_len`, `pwrite` et `fsync`
/// ne sont jamais appelés ; s'ils l'étaient, le descripteur les refuserait.
///
/// CE QUE VOIT LE LECTEUR. Les pages qu'il n'a pas écrites restent celles du
/// cache de pages : si un écrivain partagé vivant les modifie, le lecteur le
/// voit (comme avant). Une page qu'il a écrite (rejeu) est figée à sa version.
///
/// PARTAGE ENTRE POIGNÉES. Le code s'appuie sur le fait que deux poignées de
/// la même zone voient les mêmes octets (le rejeu écrit par les siennes, le
/// `Writer` relit par les siennes). Un `MAP_SHARED` l'offrait gratuitement ;
/// deux projections privées divergeraient. D'où le [`RegistrePrive`] : une
/// seule `ZonePrivee` par fichier et par ouverture, partagée par `Arc`.
pub(crate) struct ZonePrivee {
    chemin: PathBuf,
    /// Descripteur en LECTURE SEULE ; `None` si le fichier n'existait pas à
    /// l'ouverture (il n'est pas créé : la zone vit alors en mémoire).
    fichier: Option<File>,
    etat: std::cell::UnsafeCell<EtatPrive>,
}

// SAFETY: même contrat que `RamZone` et qu'un mmap partagé entre poignées — la
// sérialisation des écritures vient des verrous du substrat.
unsafe impl Send for ZonePrivee {}
unsafe impl Sync for ZonePrivee {}

impl std::fmt::Debug for ZonePrivee {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZonePrivee")
            .field("chemin", &self.chemin)
            .field("fichier", &self.fichier.is_some())
            .finish_non_exhaustive()
    }
}

impl ZonePrivee {
    /// Ouvre `chemin` SANS droit d'écriture et SANS le créer.
    fn ouvrir(chemin: &Path) -> SubstrateResult<Self> {
        let fichier = match File::open(chemin) {
            Ok(f) => Some(f),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(SubstrateError::Io(e)),
        };
        let (octets, len) = match &fichier {
            Some(f) => {
                let len = f.metadata()?.len() as usize;
                if len == 0 {
                    (OctetsPrives::Vide, 0)
                } else {
                    // SAFETY: projection PRIVÉE d'un fichier en lecture seule ;
                    // le verrou partagé du répertoire exclut l'écrivain exclusif
                    // (le seul à pouvoir tronquer une zone).
                    let m = unsafe { MmapOptions::new().len(len).map_copy(f)? };
                    (OctetsPrives::Fichier(m), len)
                }
            }
            None => (OctetsPrives::Vide, 0),
        };
        Ok(Self {
            chemin: chemin.to_path_buf(),
            fichier,
            etat: std::cell::UnsafeCell::new(EtatPrive {
                octets,
                len,
                retraites: Vec::new(),
            }),
        })
    }

    fn len(&self) -> usize {
        // SAFETY: lecture d'un entier ; aucune croissance concurrente (contrat).
        unsafe { (*self.etat.get()).len }
    }

    fn longueur_disque(&self) -> u64 {
        match &self.fichier {
            Some(f) => f
                .metadata()
                .map(|m| m.len())
                .unwrap_or_else(|_| self.len() as u64),
            None => self.len() as u64,
        }
    }

    /// # Safety
    /// Aucune vue MUTABLE vivante sur la zone (contrat d'un mmap partagé).
    unsafe fn as_slice(&self) -> &[u8] {
        // SAFETY: délégué à l'appelant.
        let e = unsafe { &*self.etat.get() };
        match &e.octets {
            OctetsPrives::Vide => &[],
            OctetsPrives::Fichier(m) | OctetsPrives::Memoire(m) => &m[..e.len],
        }
    }

    /// # Safety
    /// Aucune autre vue vivante sur la zone pendant la durée de celle-ci.
    #[allow(clippy::mut_from_ref)]
    unsafe fn as_slice_mut(&self) -> &mut [u8] {
        // SAFETY: délégué à l'appelant.
        let e = unsafe { &mut *self.etat.get() };
        let len = e.len;
        match &mut e.octets {
            OctetsPrives::Vide => &mut [],
            OctetsPrives::Fichier(m) | OctetsPrives::Memoire(m) => &mut m[..len],
        }
    }

    /// La projection du FICHIER, si la zone en est encore une (conseils noyau).
    ///
    /// # Safety
    /// Aucune vue mutable vivante.
    unsafe fn projection_fichier(&self) -> Option<&MmapMut> {
        // SAFETY: délégué à l'appelant.
        match unsafe { &(*self.etat.get()).octets } {
            OctetsPrives::Fichier(m) => Some(m),
            _ => None,
        }
    }

    /// Fait grandir la zone EN MÉMOIRE (zéro-remplie), sans toucher au fichier.
    ///
    /// * zone née en mémoire, croissance dans la réserve : un entier change ;
    /// * réserve épuisée : nouvelle réserve double, recopie, ancienne retirée ;
    /// * zone adossée à un fichier : elle doit quitter son fichier, donc être
    ///   recopiée. C'est le cas RARE où le rejeu étend une zone au-delà de ce
    ///   que le disque porte (écrivain tué entre l'append WAL et l'allongement
    ///   du fichier). Au-delà de [`PLAFOND_COPIE_PRIVEE`] on REFUSE plutôt que
    ///   de recopier des Go en mémoire anonyme : un écrivain doit consolider.
    ///
    /// # Safety
    /// Aucune vue rendue par la poignée APPELANTE ne survit (elle tient
    /// `&mut self`). Les vues d'autres poignées restent valides : l'ancienne
    /// projection est retirée, pas détruite.
    unsafe fn grandir(&self, new_len: usize) -> SubstrateResult<()> {
        // SAFETY: délégué à l'appelant.
        unsafe { self.grandir_avec_plafond(new_len, PLAFOND_COPIE_PRIVEE) }
    }

    /// [`Self::grandir`] avec un plafond explicite (testable sans fichier de 1 Gio).
    ///
    /// # Safety
    /// Même contrat que [`Self::grandir`].
    unsafe fn grandir_avec_plafond(&self, new_len: usize, plafond: usize) -> SubstrateResult<()> {
        // SAFETY: délégué à l'appelant.
        let e = unsafe { &mut *self.etat.get() };
        if new_len <= e.len {
            return Ok(());
        }
        if let OctetsPrives::Memoire(m) = &e.octets
            && new_len <= m.len()
        {
            e.len = new_len;
            return Ok(());
        }
        if matches!(e.octets, OctetsPrives::Fichier(_)) && e.len > plafond {
            return Err(SubstrateError::LectureSeuleRefusee {
                raison: format!(
                    "la zone {} ({} octets) devrait grandir à {} octets pour rejouer le \
                     WAL, ce qu'une ouverture en lecture seule ne peut faire qu'en la \
                     recopiant en mémoire ; ouvrez la base une fois en écriture \
                     (`obrain data compact` ou toute ouverture inscriptible) pour \
                     consolider, puis rouvrez en lecture seule",
                    self.chemin.display(),
                    e.len,
                    new_len
                ),
            });
        }
        let reserve = new_len
            .saturating_mul(2)
            .max(RESERVE_MEMOIRE_MIN)
            .next_multiple_of(4096);
        let mut neuf = MmapOptions::new().len(reserve).map_anon()?;
        let ancien = std::mem::replace(&mut e.octets, OctetsPrives::Vide);
        match ancien {
            OctetsPrives::Vide => {}
            OctetsPrives::Fichier(m) | OctetsPrives::Memoire(m) => {
                neuf[..e.len].copy_from_slice(&m[..e.len]);
                e.retraites.push(m);
            }
        }
        e.octets = OctetsPrives::Memoire(neuf);
        e.len = new_len;
        Ok(())
    }

    /// Reprojette EN PRIVÉ un fichier qu'un écrivain a allongé. Rend `true` si
    /// la zone a grandi.
    ///
    /// La nouvelle projection est NEUVE : les pages que ce lecteur avait écrites
    /// (rejeu de l'ouverture) redeviennent celles du fichier. C'est correct : un
    /// fichier qui grandit a un écrivain vivant, et cet écrivain a lui-même posé
    /// dans les zones tout ce que son WAL portait (écriture directe, ou rejeu à
    /// sa propre ouverture). Une zone qui a quitté son fichier ([`Self::grandir`])
    /// n'est plus reprojetée.
    ///
    /// # Safety
    /// Même contrat que [`Self::grandir`].
    unsafe fn rafraichir(&self) -> SubstrateResult<bool> {
        // SAFETY: délégué à l'appelant.
        let e = unsafe { &mut *self.etat.get() };
        let Some(f) = &self.fichier else {
            return Ok(false);
        };
        if matches!(e.octets, OctetsPrives::Memoire(_)) {
            return Ok(false);
        }
        let disque = f.metadata()?.len() as usize;
        if disque <= e.len {
            return Ok(false);
        }
        // SAFETY: voir `ouvrir`.
        let m = unsafe { MmapOptions::new().len(disque).map_copy(f)? };
        if let OctetsPrives::Fichier(ancien) =
            std::mem::replace(&mut e.octets, OctetsPrives::Fichier(m))
        {
            e.retraites.push(ancien);
        }
        e.len = disque;
        Ok(true)
    }
}

/// Le registre des zones d'une ouverture en lecture seule : UNE [`ZonePrivee`]
/// par fichier, rendue à toutes les poignées (voir « PARTAGE ENTRE POIGNÉES »).
///
/// Les entrées sont FORTES et vivent autant que l'ouverture : une zone écrite
/// en privé par le rejeu puis relâchée doit être retrouvée telle quelle par la
/// poignée suivante.
#[derive(Debug, Default)]
struct RegistrePrive {
    zones: std::sync::Mutex<HashMap<String, std::sync::Arc<ZonePrivee>>>,
}

impl RegistrePrive {
    fn zone(&self, dir: &Path, nom: &str) -> SubstrateResult<std::sync::Arc<ZonePrivee>> {
        let mut garde = self
            .zones
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(z) = garde.get(nom) {
            return Ok(z.clone());
        }
        let z = std::sync::Arc::new(ZonePrivee::ouvrir(&dir.join(nom))?);
        garde.insert(nom.to_string(), z.clone());
        Ok(z)
    }
}

/// A read/write memory-mapped file for one zone.
#[derive(Debug)]
pub struct ZoneFile {
    path: PathBuf,
    /// Current logical length (may exceed mmap length if the file was just grown).
    len: u64,
    /// Le support des octets : fichier mmapé ou mémoire.
    backing: ZoneBacking,
    /// Number of times `grow_to` actually performed a resize+remap. Exposed for
    /// tests and observability; should grow at O(log N) under exponential
    /// pre-alloc policy.
    remap_count: u64,
    /// Projection en lecture seule (ouverte par `open_read_only_zone`) : une
    /// reprojection doit garder ce mode.
    lecture_seule: bool,
}

impl ZoneFile {
    /// Ouvre une zone existante en lecture seule (mmap read-only). Utilisé
    /// par le bridge RO partagé (zones journal) — aucune écriture possible.
    pub(crate) fn open_read_only_zone(path: &Path) -> SubstrateResult<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let map = if len == 0 {
            None
        } else {
            // SAFETY: lecture seule sur un fichier existant (mmap MAP_SHARED,
            // mapping immuable — le support mmap est Option<MmapMut>, mais un
            // Mmap immuable est casté en MmapMut sans droit d'écriture réel ;
            // la lecture seule (as_slice) est sûre, grow_to échouera (le RO
            // n'écrit jamais).
            let m: Mmap = unsafe { MmapOptions::new().len(len as usize).map(&file)? };
            Some(unsafe { std::mem::transmute::<Mmap, MmapMut>(m) })
        };
        Ok(ZoneFile {
            path: path.to_path_buf(),
            len,
            backing: ZoneBacking::Mmap { file, map },
            remap_count: 0,
            lecture_seule: true,
        })
    }

    /// Open or create the file at `path`. If the file is non-empty, memory-map it
    /// read/write; if empty, the mmap is deferred until the first grow.
    pub(crate) fn open_or_create(path: &Path) -> SubstrateResult<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // Open-or-create semantics: existing contents must be kept.
            .truncate(false)
            .open(path)?;
        let len = file.metadata()?.len();
        let map = if len == 0 {
            None
        } else {
            // SAFETY: we just opened `file` with read+write, the substrate
            // directory is exclusively locked (see SubstrateFile::open), and we
            // never truncate while a mmap is live.
            let m = unsafe { MmapOptions::new().len(len as usize).map_mut(&file)? };
            let nom = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !est_zone_props_v2_nom(nom) {
                advise_huge_pages(&m, len);
            }
            Some(m)
        };
        Ok(Self {
            path: path.to_path_buf(),
            len,
            backing: ZoneBacking::Mmap { file, map },
            remap_count: 0,
            lecture_seule: false,
        })
    }

    /// Ouvre une zone EN MÉMOIRE : aucune E/S, aucun mmap.
    ///
    /// `nom` sert de chemin LOGIQUE (diagnostics, décision THP
    /// `est_zone_props_v2_nom`) et n'est jamais ouvert comme fichier.
    pub(crate) fn open_ram(nom: &str, zone: std::sync::Arc<RamZone>) -> Self {
        let len = zone.len_u64();
        Self {
            path: PathBuf::from(MEMORY_DIR).join(nom),
            len,
            backing: ZoneBacking::Ram(zone),
            remap_count: 0,
            // Une zone EN MÉMOIRE n'a pas de projection en lecture seule :
            // `open_ram` est le mode « sans système de fichiers ».
            lecture_seule: false,
        }
    }

    /// Poignée sur une zone d'une ouverture en LECTURE SEULE (voir
    /// [`ZonePrivee`]) : aucune E/S d'écriture, jamais.
    pub(crate) fn open_privee(zone: std::sync::Arc<ZonePrivee>) -> Self {
        Self {
            path: zone.chemin.clone(),
            len: zone.len() as u64,
            backing: ZoneBacking::Privee(zone),
            remap_count: 0,
            lecture_seule: true,
        }
    }

    /// Number of resize+remap operations performed on this zone since it was
    /// opened. Useful for observability and for verifying the grow policy is
    /// O(log N) in stress tests.
    pub fn remap_count(&self) -> u64 {
        self.remap_count
    }

    /// Taille ACTUELLE du fichier sur disque (un autre processus peut l'avoir
    /// agrandi depuis notre projection).
    pub fn longueur_disque(&self) -> u64 {
        match &self.backing {
            ZoneBacking::Mmap { file, .. } => file.metadata().map(|m| m.len()).unwrap_or(self.len),
            // En mémoire il n'y a pas de « disque plus récent que nous » : la
            // `RamZone` EST l'état, et elle est partagée par le registre de
            // `RamDir` — `self.len` en est donc déjà la taille vraie.
            ZoneBacking::Ram(_) => self.len,
            ZoneBacking::Privee(z) => z.longueur_disque(),
        }
    }

    /// (2026-09-16) REPROJETER UNE ZONE QU'UN AUTRE PROCESSUS A AGRANDIE.
    ///
    /// Mesuré sur runtime-dev : les lecteurs au long cours (viz, harness)
    /// gardaient la projection de leur ouverture ; l'écrivain, lui, agrandit
    /// le fichier. Une référence vers une page plus récente tombait « hors
    /// limites » et la propriété disparaissait de la vue (12 269 fois pour le
    /// seul viz 8432), alors que la donnée était bien sur disque.
    ///
    /// Ne fait QUE projeter plus grand — jamais `set_len` : entre la lecture de
    /// la taille et l'appel, l'écrivain peut avoir allongé encore le fichier,
    /// et un `set_len` le tronquerait. Le mode (lecture seule ou non) est
    /// conservé. Rend `true` si la projection a grandi.
    pub fn rafraichir(&mut self) -> SubstrateResult<bool> {
        match &mut self.backing {
            ZoneBacking::Mmap { file, map } => {
                let disque = file.metadata()?.len();
                if disque <= self.len {
                    return Ok(false);
                }
                let m = if self.lecture_seule {
                    // SAFETY: même contrat que `open_read_only_zone` — projection
                    // immuable, jamais écrite.
                    let m: Mmap = unsafe { MmapOptions::new().len(disque as usize).map(&*file)? };
                    unsafe { std::mem::transmute::<Mmap, MmapMut>(m) }
                } else {
                    // SAFETY: fichier ouvert en lecture-écriture, on ne tronque jamais.
                    unsafe { MmapOptions::new().len(disque as usize).map_mut(&*file)? }
                };
                *map = Some(m);
                self.len = disque;
                self.remap_count += 1;
                Ok(true)
            }
            // En mémoire, il n'y a pas de pair qui agrandirait le support :
            // rien à reprojeter. (Le mode RAM est celui du wasm, sans E/S.)
            ZoneBacking::Ram(_) => Ok(false),
            // Lecture seule : on reprojette EN PRIVÉ la taille que le fichier a
            // prise chez l'écrivain — sans jamais écrire (voir `ZonePrivee`).
            ZoneBacking::Privee(z) => {
                // SAFETY: `&mut self` — aucune vue rendue par cette poignée ne
                // survit ; l'ancienne projection est RETIRÉE, pas détruite, donc
                // une vue tenue par une autre poignée reste valide.
                let grandi = unsafe { z.rafraichir()? };
                if grandi {
                    self.len = z.len() as u64;
                    self.remap_count += 1;
                }
                Ok(grandi)
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        match &self.backing {
            // La longueur d'une zone privée est PARTAGÉE entre ses poignées :
            // une croissance faite par l'une (rejeu) est vue par les autres.
            ZoneBacking::Privee(z) => z.len() as u64,
            _ => self.len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Borrow the mapped bytes (read-only view).
    pub fn as_slice(&self) -> &[u8] {
        match &self.backing {
            ZoneBacking::Mmap { map, .. } => match map.as_ref() {
                Some(m) => &m[..],
                None => &[],
            },
            ZoneBacking::Ram(z) => {
                if self.len == 0 {
                    &[]
                } else {
                    // SAFETY: aucune vue mutable vivante — le substrat sérialise
                    // les écritures (contrat identique au mmap, cf. `RamZone`).
                    unsafe { z.as_slice() }
                }
            }
            // SAFETY: même contrat qu'un mmap partagé entre poignées — aucune
            // vue mutable vivante pendant une lecture (le substrat sérialise).
            ZoneBacking::Privee(z) => unsafe { z.as_slice() },
        }
    }

    /// Borrow the mapped bytes mutably.
    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        match &mut self.backing {
            ZoneBacking::Mmap { map, .. } => match map.as_mut() {
                Some(m) => &mut m[..],
                None => &mut [],
            },
            ZoneBacking::Ram(z) => {
                if self.len == 0 {
                    &mut []
                } else {
                    // SAFETY: `&mut self` garantit l'unicité de CETTE poignée ;
                    // le substrat interdit une vue concurrente sur une autre
                    // poignée de la même zone (contrat identique au mmap).
                    unsafe { z.as_slice_mut() }
                }
            }
            // SAFETY: `&mut self` garantit l'unicité de CETTE poignée ; les
            // octets sont PRIVÉS (copy-on-write ou mémoire) : rien n'atteint le
            // fichier, quoi qu'on écrive ici.
            ZoneBacking::Privee(z) => unsafe { z.as_slice_mut() },
        }
    }

    /// Grow the file to `new_len` bytes (zero-padded) and remap.
    ///
    /// This is the low-level primitive; higher-level pre-allocation policies
    /// (exponential ×1.5) live in `SubstrateFile`.
    pub fn grow_to(&mut self, new_len: u64) -> SubstrateResult<()> {
        let actuelle = self.len();
        if new_len < actuelle {
            return Err(SubstrateError::WalBadFrame(format!(
                "grow_to: refuses to shrink ({} → {})",
                actuelle, new_len
            )));
        }
        if new_len == actuelle {
            return Ok(());
        }
        // Drop the previous map before resizing (some platforms disallow resize
        // while a mapping is live).
        match &mut self.backing {
            ZoneBacking::Mmap { file, map } => {
                map.take();
                // ── NE JAMAIS RÉTRÉCIR UN FICHIER QUE D'AUTRES ONT FAIT GRANDIR ───
                //
                // (Phase B, 2026-10-…) `set_len(new_len)` RÉTRÉCIT quand la cible
                // est plus petite que la taille RÉELLE du fichier. En mono-écrivain
                // cela ne pouvait pas arriver (`self.len` était la taille du
                // fichier, et `new_len > self.len`). En écriture partagée, un PAIR
                // peut avoir fait grandir le fichier au-delà de NOTRE mapping : le
                // slot que la borne partagée vient de nous donner peut alors être
                // au-delà de notre `self.len`, et la croissance calculée par
                // `ensure_room` (`max(self.len × 1,5, besoin + marge)`) peut rester
                // SOUS la taille réelle. `set_len` aurait tronqué le fichier du
                // pair — effaçant ses nœuds, ses arêtes et ses colonnes — et
                // remappé par-dessus. C'est un effacement silencieux d'un autre
                // écrivain, exactement la famille de défauts que cette phase ferme.
                //
                // On adopte donc la taille RÉELLE quand elle dépasse la cible : on
                // mappe plus, jamais moins. Mono-écrivain : `reelle == self.len`, et
                // comme `new_len > self.len`, `cible == new_len` — comportement
                // INCHANGÉ (les tests de politique de croissance le verrouillent).
                let reelle = file.metadata()?.len();
                let cible = new_len.max(reelle);
                file.set_len(cible)?;
                // SAFETY: same invariants as `open_or_create`.
                let m = unsafe { MmapOptions::new().len(cible as usize).map_mut(&*file)? };
                // `madvise(MADV_HUGEPAGE)` must be re-applied after every remap — the
                // hint lives on the VMA, not on the file. A zone that crossed the
                // HUGE_PAGE_MIN_BYTES threshold during this grow will now be eligible
                // for THP backing.
                //
                // (2026-09-06, RFC multi-writer Levier 2) EXCEPTION props v2 : les
                // zones `substrate.props.v2` / `substrate.props.heap.v2` sont écrites
                // par PETITS appends (4-64 B par prop) et lues par chaîne (clé), pas
                // par balayage séquentiel — le THP n'y apporte rien et coûte à
                // l'écriture : chaque append dans une page THP fraîche déclenche une
                // allocation/split 2 Mio (mesuré : prop page neuve 8,09 µs THP ON vs
                // 6,89 µs OFF, −15 %). C'est la suite (b) documentée du pilotage
                // `OBRAIN_TMN_HUGEPAGE` : conditionner par type de zone.
                let est_zone_props_v2 = {
                    let nom = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    est_zone_props_v2_nom(nom)
                };
                if !est_zone_props_v2 {
                    advise_huge_pages(&m, cible);
                }
                *map = Some(m);
                self.len = cible;
            }
            ZoneBacking::Ram(z) => {
                // SAFETY: `grow_to` prend `&mut self` — aucune vue rendue par
                // CETTE poignée ne survit à l'appel. Le contrat du substrat
                // interdit de même une vue vivante sur une AUTRE poignée de la
                // même zone : c'est déjà l'exigence d'un remap mmap, qui change
                // l'adresse. Rien n'est donc assoupli en mode mémoire.
                unsafe { z.resize(new_len) };
                self.len = new_len;
            }
            ZoneBacking::Privee(z) => {
                // SAFETY: `&mut self` ; croissance EN MÉMOIRE seulement — le
                // fichier n'est ni allongé ni touché (descripteur sans droit
                // d'écriture). Voir `ZonePrivee::grandir`.
                unsafe { z.grandir(new_len as usize)? };
                self.len = new_len;
            }
        }
        self.remap_count += 1;
        Ok(())
    }

    /// Shrink the file to `new_len` bytes (truncate) and remap.
    ///
    /// The inverse of [`grow_to`](Self::grow_to), added for the zone shrink
    /// at `compact` time (chantier « meilleure en tout », 2026-08-31). The
    /// caller guarantees `new_len` stays at or above the last LIVE slot —
    /// the pages above it are zero-init padding produced by the exponential
    /// ×1.5 pre-allocation, so truncating them reclaims disk without touching
    /// live data. Same mmap-drop-then-resize discipline as `grow_to`.
    pub fn shrink_to(&mut self, new_len: u64) -> SubstrateResult<()> {
        if matches!(self.backing, ZoneBacking::Privee(_)) {
            return Err(SubstrateError::LectureSeule {
                operation: format!("rétrécir la zone {}", self.path.display()),
            });
        }
        if new_len > self.len {
            return Err(SubstrateError::WalBadFrame(format!(
                "shrink_to: refuses to grow ({} → {}) — use grow_to",
                self.len, new_len
            )));
        }
        if new_len == self.len {
            return Ok(());
        }
        match &mut self.backing {
            ZoneBacking::Mmap { file, map } => {
                map.take();
                file.set_len(new_len)?;
                // SAFETY: same invariants as `open_or_create`.
                let m = unsafe { MmapOptions::new().len(new_len as usize).map_mut(&*file)? };
                advise_huge_pages(&m, new_len);
                *map = Some(m);
            }
            ZoneBacking::Ram(z) => {
                // SAFETY: même raisonnement que `grow_to` (cf. commentaire).
                unsafe { z.resize(new_len) };
            }
            ZoneBacking::Privee(_) => unreachable!("refusé en tête de fonction"),
        }
        self.len = new_len;
        self.remap_count += 1;
        Ok(())
    }

    /// Flush dirty pages to disk (`msync`). Call after write-through mutations
    /// when durability is required *beyond* the WAL (for example at checkpoint).
    /// (T27) Écrit `octets` à `offset` par l'APPEL SYSTÈME, sans passer par la
    /// projection. Une écriture en lot de plusieurs Go à travers la projection
    /// paie un défaut de page par page de 4 Ko à la première écriture (mesuré :
    /// 7,5 M défauts mineurs sur l'indexation du noyau Linux) ; `pwrite` de
    /// blocs de dizaines de Mo n'en paie aucun. La projection partagée voit le
    /// résultat par le cache de pages, et `flush()` le rend durable (msync +
    /// fsync) comme le reste. L'appelant doit avoir agrandi la zone au
    /// préalable (`grow_to`) : on n'écrit jamais au-delà de `len`.
    pub fn ecrire_a(&self, offset: u64, octets: &[u8]) -> SubstrateResult<()> {
        let fin = offset + octets.len() as u64;
        if fin > self.len() {
            return Err(SubstrateError::WalBadFrame(format!(
                "ecrire_a : {fin} au-delà de la zone ({})",
                self.len()
            )));
        }
        match &self.backing {
            ZoneBacking::Mmap { file, .. } => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    use std::os::unix::fs::FileExt;
                    file.write_all_at(octets, offset)?;
                }
                // ── wasm32 : LA BRANCHE DOIT COMPILER, PAS FORCÉMENT SERVIR ──
                // `std::os::unix` n'existe pas sur wasm32 ; la lignée servie
                // importait `FileExt` sans garde, ce qui cassait le build wasm
                // (`verify-wasm.sh`, ajouté par le multi-écrivain, est la porte
                // qui l'attrape). Sur wasm le support est TOUJOURS `Ram`
                // (`open_ram`, pas de système de fichiers) : cette branche n'est
                // jamais atteinte, mais elle doit compiler — et si elle l'était,
                // elle le dit au lieu de ne rien faire en silence.
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = file;
                    return Err(SubstrateError::WalBadFrame(
                        "ecrire_a : aucune écriture fichier sur wasm32 (support RAM)".to_string(),
                    ));
                }
            }
            // En mémoire, il n'y a pas d'appel système à économiser : on copie
            // directement, avec le MÊME contrat (jamais au-delà de `self.len`).
            ZoneBacking::Ram(z) => {
                // SAFETY: `ecrire_a` borne déjà l'écriture à `self.len` (garde
                // ci-dessus) et le contrat du substrat interdit une vue vivante
                // sur la même zone pendant une écriture (cf. `grow_to`).
                let mem = unsafe { z.as_slice_mut() };
                let (debut, arrondi) = (offset as usize, fin as usize);
                if arrondi > mem.len() {
                    return Err(SubstrateError::WalBadFrame(format!(
                        "ecrire_a : {fin} au-delà de la zone mémoire ({})",
                        mem.len()
                    )));
                }
                mem[debut..arrondi].copy_from_slice(octets);
            }
            // Lecture seule : JAMAIS d'appel système d'écriture — la copie va
            // dans la projection privée, bornée par la garde ci-dessus.
            ZoneBacking::Privee(z) => {
                // SAFETY: contrat du substrat (aucune vue vivante sur la zone
                // pendant une écriture) ; borne vérifiée contre `self.len()`.
                let mem = unsafe { z.as_slice_mut() };
                mem[offset as usize..fin as usize].copy_from_slice(octets);
            }
        }
        Ok(())
    }

    pub fn msync(&self) -> SubstrateResult<()> {
        #[cfg(test)]
        {
            let vise = crate::writer::ECHEC_MSYNC.with(|c| {
                c.borrow()
                    .as_deref()
                    .is_some_and(|suffixe| self.path.to_string_lossy().ends_with(suffixe))
            });
            if vise {
                crate::writer::ECHEC_MSYNC.with(|c| *c.borrow_mut() = None);
                return Err(SubstrateError::Io(std::io::Error::other(
                    "EIO simule (failpoint de test) : msync refuse",
                )));
            }
        }
        match &self.backing {
            ZoneBacking::Mmap { map, .. } => {
                if let Some(m) = map.as_ref() {
                    m.flush()?;
                }
            }
            // En mémoire, il n'y a rien à « pousser » : les octets SONT l'état.
            // En lecture seule non plus : la projection est privée, et rien ne
            // doit — ni ne peut — rejoindre le fichier.
            ZoneBacking::Ram(_) | ZoneBacking::Privee(_) => {}
        }
        Ok(())
    }

    /// Fsync the underlying file descriptor.
    pub fn fsync(&self) -> SubstrateResult<()> {
        if let ZoneBacking::Mmap { file, .. } = &self.backing {
            file.sync_all()?;
        }
        Ok(())
    }

    /// Evict pages backing this zone from the resident set via
    /// `madvise(MADV_DONTNEED)` (Unix only — on non-Unix this is a no-op).
    ///
    /// Primary use: the T11 Step 5 prefetch bench, which needs a cold
    /// page cache to measure the `WILLNEED` effect. Production code
    /// does not call this — `DONTNEED` is a destructive hint (subsequent
    /// reads miss the cache and fault in from disk).
    ///
    /// Marked `unsafe` because `DONTNEED` semantics on Linux include
    /// *discarding dirty pages* in anonymous mappings. For a
    /// file-backed mmap the behavior is benign (pages are re-read from
    /// the file on next access), but we surface the hazard so tests
    /// and developers explicitly opt in.
    ///
    /// # Safety
    /// The caller must ensure no other thread is reading this zone
    /// concurrently via an existing `&[u8]` reference, since the pages
    /// backing that slice may be evicted and re-faulted between reads.
    pub unsafe fn advise_dontneed(&self) -> SubstrateResult<()> {
        if let ZoneBacking::Mmap { map, .. } = &self.backing
            && let Some(m) = map.as_ref()
        {
            // SAFETY: delegated to caller — file-backed mapping, caller
            // attests no live &[u8] outstanding.
            // (2026-09-01) madvise n'existe pas sur wasm32 (stub memmap2) :
            // hint de perf no-op, sémantique inchangée.
            #[cfg(not(target_arch = "wasm32"))]
            unsafe {
                m.unchecked_advise(memmap2::UncheckedAdvice::DontNeed)?
            };
            #[cfg(target_arch = "wasm32")]
            let _ = m;
        }
        Ok(())
    }

    /// Advise the kernel that the byte range `[offset, offset + len)` of
    /// this zone will be accessed soon (`madvise(MADV_WILLNEED)` on Unix,
    /// `PrefetchVirtualMemory` on Windows, no-op elsewhere).
    ///
    /// This is a *hint* — it does not block and does not guarantee the
    /// range will be resident. It asks the kernel to schedule readahead
    /// for the given pages before the application touches them, which on
    /// cold-cache access patterns can remove most of the minor+major
    /// page-fault latency from the hot path (T11 Step 5).
    ///
    /// **Bounds.** `offset + len` is clamped to `self.len` so callers can
    /// safely advise a range that overruns the mapped region (typical
    /// when `last_slot + 1` rounds past the high-water mark on the last
    /// partial page).
    ///
    /// **No-op cases:**
    /// * Zone is unmapped (no mmap — empty file, ou zone en memoire) →
    ///   returns `Ok(())` immediately.
    /// * `len == 0` after clamping → `Ok(())` (the kernel call itself is
    ///   a no-op for zero-length ranges on most platforms, and skipping
    ///   avoids an unnecessary syscall).
    ///
    /// **Errors.** An I/O error from `madvise` is surfaced as
    /// `SubstrateError::Io`. Callers treat the result as best-effort: a
    /// prefetch failure must not abort the activation pipeline.
    pub fn advise_willneed(&self, offset: usize, len: usize) -> SubstrateResult<()> {
        let m = match &self.backing {
            ZoneBacking::Mmap { map, .. } => match map.as_ref() {
                Some(m) => m,
                None => return Ok(()),
            },
            // Lecture seule : la pré-lecture est un conseil au noyau, pas une
            // écriture — elle vaut aussi pour une projection privée de fichier.
            // SAFETY: lecture de l'état partagé, aucune vue mutable vivante.
            ZoneBacking::Privee(z) => match unsafe { z.projection_fichier() } {
                Some(m) => m,
                None => return Ok(()),
            },
            // Mode mémoire : pas de noyau à qui demander une pré-lecture.
            ZoneBacking::Ram(_) => return Ok(()),
        };
        let total = (self.len() as usize).min(m.len());
        if total == 0 || offset >= total {
            return Ok(());
        }
        let clamped = len.min(total - offset);
        if clamped == 0 {
            return Ok(());
        }
        // (2026-09-01) madvise n'existe pas sur wasm32 (stub memmap2) :
        // hint de prefetch no-op.
        #[cfg(not(target_arch = "wasm32"))]
        m.advise_range(Advice::WillNeed, offset, clamped)?;
        #[cfg(target_arch = "wasm32")]
        let _ = (m, offset, clamped);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Top-level SubstrateFile: the directory handle.
// ---------------------------------------------------------------------------

/// Le support de l'en-tête meta : fichier mmapé (mode natif) ou mémoire.
#[derive(Debug)]
enum MetaBacking {
    Mmap { file: File, map: MmapMut },
    Ram(std::sync::Arc<RamZone>),
}

/// A live substrate on disk. Owns the directory handle, the meta file mmap,
/// and lazy handles to the per-zone mmap files.
#[derive(Debug)]
pub struct SubstrateFile {
    path: PathBuf,
    /// L'en-tête : mmapé séparément en mode fichier (il est écrit à chaque
    /// commit), en mémoire en mode sans système de fichiers.
    meta: MetaBacking,
    /// Backing temp directory, when the substrate was created via
    /// [`SubstrateFile::open_tempfile`]. Dropped at the end of the substrate's
    /// lifetime, which unlinks the directory.
    _temp: Option<tempfile_guard::TempDirGuard>,
    /// (2026-08-21) Part du verrou exclusif du répertoire. Partagée entre
    /// toutes les ouvertures du MÊME répertoire dans CE processus ; le verrou
    /// OS n'est rendu que lorsque la dernière part disparaît.
    ///
    /// `None` en mode mémoire : il n'y a ni répertoire à verrouiller, ni second
    /// processus à en exclure (wasm32 n'a pas de multi-processus).
    _verrou: Option<std::sync::Arc<VerrouRepertoire>>,
    /// L'ouverture a-t-elle pris un verrou PARTAGÉ (`open_shared_write`,
    /// `open_read_only`) plutôt qu'EXCLUSIF ?
    ///
    /// ── POURQUOI LE WAL A BESOIN DE LE SAVOIR (axe APPEND, 2026-10-06) ──────
    /// Un verrou exclusif de répertoire prouve qu'AUCUN autre processus ne peut
    /// écrire ce store : la coordination d'append inter-processus est alors
    /// inutile, et le WAL garde son chemin chaud (lire la fin + écrire). Un
    /// verrou PARTAGÉ, lui, autorise plusieurs `open_shared_write`
    /// simultanés (constat mesuré de la sonde `p3`) : il faut alors sérialiser
    /// la paire [lire la fin, écrire] entre processus, sans quoi deux écrivains
    /// peuvent lire la MÊME fin et estampiller le MÊME LSN.
    partage: bool,
    /// Registre des zones, en mode mémoire uniquement. `None` = mode fichier.
    /// C'est LUI qui garantit que deux poignées de la même zone partagent les
    /// mêmes octets (cf. [`RamZone`]).
    ram: Option<std::sync::Arc<RamDir>>,
    /// Registre des zones PRIVÉES, pour une ouverture en LECTURE SEULE
    /// uniquement (`open_read_only`). `Some` = aucune écriture n'atteint le
    /// disque par cette poignée : toutes les zones passent par [`ZonePrivee`].
    prive: Option<std::sync::Arc<RegistrePrive>>,
    /// (RFC zorder-superposition, chantier 3) Époque des MUTATIONS de
    /// features (centrality_cached, arêtes/degrés, communautés, tombstones).
    /// Incrémentée par les écritures writer (write_node/write_edge/
    /// update_centrality/update_community) — PAS par le bulk sort, le
    /// compactage ni le replay (ce sont des opérations de layout). La voie
    /// « layout trié » (zruns) n'est prouvablement saine que si cette époque
    /// n'a pas bougé depuis le dernier tri (snapshot pris par le store).
    pub(crate) feature_epoch: std::sync::atomic::AtomicU64,
}

impl SubstrateFile {
    /// Open an existing substrate at `path`. Fails if the directory does not
    /// exist or the meta file has an invalid magic / unsupported version.
    pub fn open(path: impl AsRef<Path>) -> SubstrateResult<Self> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("substrate directory does not exist: {}", path.display()),
            )));
        }
        Self::open_inner(path.to_path_buf(), None, true)
    }

    /// Open an existing substrate in SHARED-WRITE mode : verrou PARTAGÉ
    /// (`LOCK_SH`), coexiste avec les lecteurs RO et avec d'autres écrivains
    /// du même répertoire. L'écriture est permise par append WAL : plusieurs
    /// processus écrivent, leurs appends sont sérialisés par `O_APPEND` et le
    /// registre partagé `substrate.wal.reg` (réservation de position). Les
    /// lecteurs rejouent le WAL à leur ouverture.
    ///
    /// Périmètre de la garantie v0.0.1 : seuls nœuds et arêtes SANS propriété
    /// sont garantis en écriture partagée. Une base dont un sidecar du régime
    /// hérité (`substrate.props`, `substrate.edge_props`, `substrate.extedges`)
    /// est non vide est refusée avec [`SubstrateError::BaseHeritee`], sans rien
    /// modifier, car ses propriétés sont tenues par l'état local d'un processus.
    pub fn open_shared_write(path: impl AsRef<Path>) -> SubstrateResult<Self> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("substrate directory does not exist: {}", path.display()),
            )));
        }
        // ── BASE HÉRITÉE : REFUS AVANT TOUTE OUVERTURE ───────────────────────
        // Un sidecar non vide du régime hérité porte des propriétés ou des arêtes
        // externes tenus par l'état LOCAL d'un processus. En écriture partagée,
        // le premier flush d'un pair les écraserait en silence. On refuse ici,
        // avant d'ouvrir zones, WAL ou index, pour ne rien modifier.
        if let Some(sidecar) = crate::store::premier_sidecar_herite_non_vide(path) {
            return Err(SubstrateError::BaseHeritee {
                sidecar: sidecar.to_string(),
            });
        }
        Self::open_inner(path.to_path_buf(), None, false)
    }

    /// Ouvre un substrat existant en LECTURE SEULE.
    ///
    /// LE CONTRAT : cette ouverture, tout ce qu'on lit par elle et sa fermeture
    /// ne modifient AUCUN octet d'AUCUN fichier de la base, n'en créent aucun
    /// et n'en suppriment aucun — quel que soit l'état où un écrivain a laissé
    /// la base, et qu'un écrivain partagé soit vivant ou non.
    ///
    /// COMMENT IL EST TENU : `substrate.meta`, `substrate.dirlock` et chaque
    /// zone sont ouverts SANS droit d'écriture ; meta et zones sont projetés en
    /// PRIVÉ (copy-on-write, voir `ZonePrivee`). Ce que l'ouverture doit
    /// « écrire » pour offrir une vue cohérente (rejeu du WAL, degrés au CRC
    /// périmé) reste dans la mémoire de ce processus. Un fichier absent n'est
    /// pas créé : sa zone vit en mémoire, vide.
    ///
    /// LE VERROU : partagé. Il exclut l'écrivain EXCLUSIF d'un autre processus,
    /// PAS un écrivain partagé — voir `acquerir_verrou_repertoire`.
    ///
    /// CE QUI EST REFUSÉ ([`SubstrateError::LectureSeuleRefusee`]) plutôt que
    /// servi en écrivant : une base d'un format antérieur (la migration est un
    /// acte d'écrivain), une base sans `substrate.dirlock`.
    pub fn open_read_only(path: impl AsRef<Path>) -> SubstrateResult<Self> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("substrate directory does not exist: {}", path.display()),
            )));
        }
        Self::open_inner_lecture_seule(path.to_path_buf())
    }

    fn open_inner_lecture_seule(path: PathBuf) -> SubstrateResult<Self> {
        let meta_path = path.join(zone::META);
        // SANS droit d'écriture, et sans création : un répertoire qui n'est pas
        // une base échoue ici, avant que quoi que ce soit n'y soit posé.
        let meta_file = File::open(&meta_path).map_err(|e| {
            SubstrateError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "{} n'est pas une base obrain ({} : {e}) — une ouverture en lecture \
                     seule ne crée rien",
                    path.display(),
                    zone::META
                ),
            ))
        })?;
        let file_len = meta_file.metadata()?.len();
        if file_len < META_FILE_SIZE as u64 {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("meta file too short: {} (< {})", file_len, META_FILE_SIZE),
            )));
        }
        let verrou_path = path.join(zone::DIR_LOCK);
        let verrou = acquerir_verrou_repertoire(&path, &verrou_path, false, false)?;
        // SAFETY: projection PRIVÉE d'un fichier ouvert en lecture seule ; le
        // verrou partagé exclut l'écrivain exclusif. `substrate.meta` n'est
        // jamais tronqué (un checkpoint le REMPLACE par renommage : cette
        // projection garde alors l'ancien inode, comme avant).
        let meta_map = unsafe {
            MmapOptions::new()
                .len(META_FILE_SIZE)
                .map_copy(&meta_file)?
        };
        let sf = Self {
            path,
            meta: MetaBacking::Mmap {
                file: meta_file,
                map: meta_map,
            },
            _temp: None,
            partage: true,
            _verrou: Some(verrou),
            ram: None,
            prive: Some(std::sync::Arc::new(RegistrePrive::default())),
            feature_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        let header = sf.meta_header();
        if header.is_valid_magic() && header.format_version < SUBSTRATE_FORMAT_VERSION {
            return Err(SubstrateError::LectureSeuleRefusee {
                raison: format!(
                    "{} est au format {} et ce binaire lit le format {} : la migration \
                     est une écriture, qu'une ouverture en lecture seule ne fait pas. \
                     Ouvrez la base une fois en écriture (elle migre à l'ouverture), puis \
                     rouvrez en lecture seule",
                    sf.path.display(),
                    header.format_version,
                    SUBSTRATE_FORMAT_VERSION
                ),
            });
        }
        sf.validate_header()?;
        Ok(sf)
    }

    /// Cette poignée est-elle une ouverture en LECTURE SEULE
    /// ([`Self::open_read_only`]) ? Si oui, rien de ce qu'elle fait n'atteint le
    /// disque.
    pub fn est_lecture_seule(&self) -> bool {
        self.prive.is_some()
    }

    /// Refuse `operation` par [`SubstrateError::LectureSeule`] si cette poignée
    /// est en lecture seule. À appeler en tête de toute écriture qui ne passe
    /// PAS par une zone (publication de fichier, suppression, marqueur).
    pub fn exiger_ecriture(&self, operation: &str) -> SubstrateResult<()> {
        if self.est_lecture_seule() {
            return Err(SubstrateError::LectureSeule {
                operation: operation.to_string(),
            });
        }
        Ok(())
    }

    /// Create a fresh substrate at `path`. The directory is created if absent;
    /// fails if it exists and already contains a substrate.meta.
    pub fn create(path: impl AsRef<Path>) -> SubstrateResult<Self> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path)?;
        let meta_path = path.join(zone::META);
        if meta_path.exists() {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("substrate already initialized: {}", meta_path.display()),
            )));
        }
        // Pre-write a blank 4 KiB meta file.
        let header = MetaHeader::new_blank();
        let mut buf = vec![0u8; META_FILE_SIZE];
        buf[..core::mem::size_of::<MetaHeader>()].copy_from_slice(bytemuck::bytes_of(&header));
        std::fs::write(&meta_path, &buf)?;

        // Pre-create empty zone files so later opens don't need to special-case
        // "missing file". The WAL file is append-only and lives outside the
        // ZoneFile abstraction.
        for name in zone::ALL_MMAPPED {
            let zp = path.join(name);
            if !zp.exists() {
                // Zero-byte file — will be grown on first write.
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&zp)?;
            }
        }
        // (jalon 2) La zone nodes v3 se decrit elle-meme : son slot 0
        // (reserve, les compteurs demarrent a 1, flags nuls donc jamais
        // vivant) porte le marqueur de stride NODES_ZONE_V3_MAGIC. Grave
        // des la CREATION pour que les etapes de migration sachent lire
        // une zone dont la version meta aurait ete reculee (fixtures de
        // banc) ou pas encore bumpee (kill entre rename et bump).
        std::fs::write(
            path.join(zone::NODES),
            bytemuck::bytes_of(&crate::record::NodeRecord::sentinelle_zone_v3()),
        )?;
        // Empty WAL file.
        let wal_path = path.join(zone::WAL);
        if !wal_path.exists() {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&wal_path)?;
        }

        Self::open_inner(path, None, true)
    }

    /// Crée un substrat EN MÉMOIRE : aucun répertoire, aucun mmap, aucune E/S.
    ///
    /// MÊME FORMAT que [`create`](Self::create) — méta vierge, zones vides,
    /// sentinelle de stride de la zone nodes — pour que le reste du substrat ne
    /// voie aucune différence de layout, seulement un support différent. C'est
    /// la brique qui permet à une base de naître là où il n'y a PAS de système
    /// de fichiers (wasm32), là où `open_tempfile` paniquait sur
    /// `std::env::temp_dir()`.
    ///
    /// Ce substrat ne persiste RIEN : c'est sa définition, pas une limite
    /// temporaire. Aucun snapshot natif n'existe encore (chantier T17b).
    pub fn open_memory() -> SubstrateResult<Self> {
        let dir = std::sync::Arc::new(RamDir::default());

        // En-tête meta vierge, comme `create()`.
        let meta = dir.zone(zone::META, META_FILE_SIZE);
        {
            let header = MetaHeader::new_blank();
            let n = core::mem::size_of::<MetaHeader>();
            // SAFETY: zone fraîche, créée à l'instant, aucune autre vue.
            let dst = unsafe { meta.as_slice_mut() };
            dst[..n].copy_from_slice(bytemuck::bytes_of(&header));
        }

        // Zones pré-créées vides — l'équivalent des fichiers de taille nulle
        // de `create()`, pour que les ouvertures ultérieures n'aient pas à
        // traiter le cas « absente ».
        for name in zone::ALL_MMAPPED {
            dir.zone(name, 0);
        }

        // (jalon 2) La zone nodes v3 se décrit elle-même : son slot 0 porte le
        // marqueur de stride, gravé dès la CRÉATION.
        let nodes = dir.zone(zone::NODES, 0);
        let sentinelle = crate::record::NodeRecord::sentinelle_zone_v3();
        let octets = bytemuck::bytes_of(&sentinelle);
        // SAFETY: zone fraîche ; la vue est prise APRÈS le redimensionnement
        // (aucune vue ne survit à un `resize`, cf. contrat de `RamZone`).
        unsafe {
            nodes.resize(octets.len() as u64);
            nodes.as_slice_mut().copy_from_slice(octets);
        }

        let sf = Self {
            path: PathBuf::from(MEMORY_DIR),
            meta: MetaBacking::Ram(meta),
            _temp: None,
            _verrou: None,
            // En mémoire il n'y a pas de second processus : pas de partage.
            partage: false,
            ram: Some(dir),
            prive: None,
            feature_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        sf.validate_header()?;
        Ok(sf)
    }

    /// Create an ephemeral substrate in a unique temporary directory. The
    /// directory is unlinked when the returned handle is dropped.
    pub fn open_tempfile() -> SubstrateResult<Self> {
        let td = tempfile_guard::TempDirGuard::new("substrate-")?;
        let path = td.path().to_path_buf();
        // Create as usual, then stash the guard so it outlives the handle.
        Self::create(&path)?;
        Self::open_inner(path, Some(td), true)
    }

    fn open_inner(
        path: PathBuf,
        temp: Option<tempfile_guard::TempDirGuard>,
        exclusif: bool,
    ) -> SubstrateResult<Self> {
        let meta_path = path.join(zone::META);
        let meta_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&meta_path)
            .map_err(SubstrateError::Io)?;
        let file_len = meta_file.metadata()?.len();
        if file_len < META_FILE_SIZE as u64 {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("meta file too short: {} (< {})", file_len, META_FILE_SIZE),
            )));
        }

        // (2026-08-21) VERROU INTER-PROCESSUS — l'invariant qui
        // autorise le `unsafe` ci-dessous. Voir `acquerir_verrou_repertoire`.
        // (2026-08-31) `exclusif=false` (open read-only) → verrou PARTAGÉ :
        // plusieurs processus peuvent lire le même store simultanément.
        // (2026-10-12) Le verrou est posé sur `substrate.dirlock`, PAS sur
        // `substrate.meta` : ce dernier est RENOMMÉ à chaque checkpoint
        // (`atomically_write_meta`), ce qui remplaçait l'inode et invalidait
        // le `flock` (deux écrivains sur deux inodes distincts).
        let verrou_path = path.join(zone::DIR_LOCK);
        let verrou = acquerir_verrou_repertoire(&path, &verrou_path, exclusif, true)?;

        // SAFETY: we just opened the file and hold an advisory lock on it for
        // the lifetime of the returned `SubstrateFile` — exclusif (`open`) ou
        // partagé (`open_shared_write` : d'autres écrivains partagés peuvent
        // vivre, coordonnés par le WAL et les registres). La lecture seule ne
        // passe PLUS par ici : voir `open_inner_lecture_seule`, qui projette en
        // privé. On ne tronque jamais le fichier meta.
        let meta_map = unsafe { MmapOptions::new().len(META_FILE_SIZE).map_mut(&meta_file)? };

        let sf = Self {
            path,
            meta: MetaBacking::Mmap {
                file: meta_file,
                map: meta_map,
            },
            _temp: temp,
            partage: !exclusif,
            _verrou: Some(verrou),
            ram: None,
            prive: None,
            feature_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        // (jalon 0 — RFC principal-natif-v2 §3) MIGRATION A L'OUVERTURE.
        // Sous verrou EXCLUSIF seulement : un store d'une version ANTERIEURE
        // est migré sur place par la chaîne de `migration::table()`, la
        // version meta bumpée en dernier (point de commit). Les ouvertures
        // partagées (read-only, shared-write) ne migrent jamais — le refus
        // propre de `validate_header` reste leur réponse. Une version
        // SUPÉRIEURE au binaire n'entre pas ici : refus propre inchangé.
        if exclusif {
            let header = sf.meta_header();
            if header.is_valid_magic() && header.format_version < SUBSTRATE_FORMAT_VERSION {
                crate::migration::migrer_repertoire(&sf.path)?;
            }
        }
        sf.validate_header()?;
        Ok(sf)
    }

    /// (2026-08-21) SIMULATION DE KILL — rend le verrou exclusif sans rien
    /// flusher.
    ///
    /// Les tests de reprise apres coupure simulent la mort du processus par
    /// `std::mem::forget(store)` : aucun destructeur ne tourne, donc aucun flush.
    /// C'est fidele sur ce point et INFIDELE sur un autre : un vrai kill fait
    /// fermer TOUS les descripteurs par le noyau, ce qui rend les verrous
    /// consultatifs. `mem::forget`, lui, garde le descripteur vivant — et donc
    /// le verrou — pour toute la duree du test.
    ///
    /// Cette methode retablit la fidelite du simulateur. Elle n'affaiblit pas
    /// l'invariant : elle modele ce que le noyau fait de toute facon.
    #[doc(hidden)]
    pub fn deverrouiller_pour_simuler_un_kill(&self) {
        // Rien à rendre en mode mémoire : aucun verrou n'a été pris. (Depuis
        // le 2026-10-12 le verrou vit sur `substrate.dirlock`, porté par la
        // part — pas sur `substrate.meta`, que cette méthode déverrouillait
        // encore, sans effet.)
        if let Some(part) = &self._verrou
            && let Some(fichier) = &part.fichier
        {
            let _ = deverrouiller_fichier(fichier);
        }
    }

    fn validate_header(&self) -> SubstrateResult<()> {
        let header = self.meta_header();
        if !header.is_valid_magic() {
            return Err(SubstrateError::BadMagic);
        }
        if header.format_version != SUBSTRATE_FORMAT_VERSION {
            return Err(SubstrateError::UnsupportedVersion(
                header.format_version,
                SUBSTRATE_FORMAT_VERSION,
            ));
        }
        Ok(())
    }

    /// Return a copy of the current meta header (small, 96 B — cheap to copy).
    pub fn meta_header(&self) -> MetaHeader {
        let n = core::mem::size_of::<MetaHeader>();
        match &self.meta {
            MetaBacking::Mmap { map, .. } => *bytemuck::from_bytes::<MetaHeader>(&map[..n]),
            MetaBacking::Ram(z) => {
                // Copie octet à octet, et non `bytemuck::from_bytes` : un
                // `Vec<u8>` n'offre pas l'alignement qu'exige la conversion
                // directe, qui paniquerait sur un en-tête mal aligné.
                // SAFETY: aucune vue mutable vivante (contrat du substrat).
                let octets = unsafe { z.as_slice() };
                let mut h = MetaHeader::new_blank();
                bytemuck::bytes_of_mut(&mut h).copy_from_slice(&octets[..n]);
                h
            }
        }
    }

    /// Rewrite the meta header in-place. Caller is responsible for ordering
    /// (the header is typically rewritten only after a successful checkpoint).
    pub fn write_meta_header(&mut self, header: &MetaHeader) -> SubstrateResult<()> {
        self.exiger_ecriture("réécrire l'en-tête meta")?;
        let n = core::mem::size_of::<MetaHeader>();
        match &mut self.meta {
            MetaBacking::Mmap { file, map } => {
                map[..n].copy_from_slice(bytemuck::bytes_of(header));
                map.flush()?;
                file.sync_all()?;
            }
            MetaBacking::Ram(z) => {
                // SAFETY: écriture de l'en-tête, aucune vue vivante (contrat).
                let dst = unsafe { z.as_slice_mut() };
                dst[..n].copy_from_slice(bytemuck::bytes_of(header));
            }
        }
        Ok(())
    }

    /// Path to the substrate directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Liste TOUTES les zones du substrat, quel que soit son support.
    ///
    /// Répertoire scanné en mode fichier, registre interrogé en mode mémoire —
    /// l'appelant voit la même chose. Sont EXCLUES : la zone meta (transportée à
    /// part par l'instantané, cf. `meta_bytes`) et le WAL (journal de rejeu, qui
    /// n'est pas une zone du support en mode mémoire).
    ///
    /// C'est ce qui rend `crate::instantane` portable : il ne connaît pas le
    /// support, il demande les noms et lit des octets.
    pub fn zone_names(&self) -> SubstrateResult<Vec<String>> {
        let mut noms: Vec<String> = match &self.ram {
            Some(dir) => dir.noms(),
            None => {
                let mut v = Vec::new();
                for entree in std::fs::read_dir(&self.path)? {
                    let entree = entree?;
                    if !entree.file_type()?.is_file() {
                        continue;
                    }
                    if let Some(nom) = entree.file_name().to_str() {
                        v.push(nom.to_string());
                    }
                }
                v
            }
        };
        noms.retain(|n| n != zone::META && n != zone::WAL && n != zone::DIR_LOCK);
        // ── LE SIDE-CAR D'ANCRE N'EST PAS UNE ZONE (axe APPEND, 2026-10-06) ──
        // `substrate.wal.lsn` décrit le WAL — et `instantane.rs` inclut TOUT ce
        // que cette fonction rend, alors qu'il EXCLUT explicitement le WAL. Une
        // image emporterait donc l'ancre d'un journal qu'elle ne contient pas :
        // un store restauré hériterait d'une base de LSN décrivant un fichier
        // disparu, et repartirait très au-dessus de 1 sans que rien ne
        // l'explique. On l'exclut, comme le WAL qu'il décrit — et avec lui le
        // temporaire `.lsn.tmp`, qui n'existe qu'entre le `write` et le `rename`
        // d'une publication d'ancre et rendrait l'inventaire non déterministe.
        noms.retain(|n| !(n.starts_with(zone::WAL) && n.len() > zone::WAL.len()));
        // ── LE FENCE DE PUBLICATION DU DICTIONNAIRE N'EST PAS UNE ZONE ───────
        // `substrate.dict.lock` (Phase B, pièce 3) ne porte AUCUN octet de données :
        // il porte un `flock` — le verrou qui sérialise le [lire → fusionner →
        // écrire] du dictionnaire entre processus (`store.rs::VerrouDictionnaire`).
        // L'inventaire ci-dessus, lui, prend TOUT fichier du répertoire : le fence
        // y entrait donc comme une zone de longueur nulle, et une image l'aurait
        // emporté (« un instantané transporte un verrou »), un store restauré
        // recréant un fichier dont il n'a que faire. Même précédent, et même
        // raison, que l'ancre du WAL juste au-dessus.
        let fence_dict = format!(
            "{}{}",
            crate::store::DICT_FILENAME_PUB,
            crate::store::SUFFIXE_FENCE_DICTIONNAIRE
        );
        noms.retain(|n| n != &fence_dict);
        noms.sort();
        noms.dedup();
        Ok(noms)
    }

    /// Les octets BRUTS de l'en-tête meta, tels que le support les porte.
    ///
    /// L'instantané transporte le support, pas une re-sérialisation : c'est ce
    /// qui garantit qu'un store restauré a EXACTEMENT la même méta, champs
    /// réservés compris.
    pub fn meta_bytes(&self) -> Vec<u8> {
        let n = crate::meta::META_FILE_SIZE;
        let mut out = vec![0u8; n];
        match &self.meta {
            MetaBacking::Mmap { map, .. } => out.copy_from_slice(&map[..n]),
            MetaBacking::Ram(z) => {
                // SAFETY: aucune vue mutable vivante (contrat du substrat) ; la
                // zone meta est créée à META_FILE_SIZE, donc la longueur suffit.
                let src = unsafe { z.as_slice() };
                let copie = src.len().min(n);
                out[..copie].copy_from_slice(&src[..copie]);
            }
        }
        out
    }

    /// Écrit l'en-tête meta depuis des octets BRUTS (restauration d'instantané).
    ///
    /// Pendant de [`meta_bytes`](Self::meta_bytes) : ce qui a été transporté tel
    /// quel est reposé tel quel.
    pub fn write_meta_from_bytes(&mut self, octets: &[u8]) -> SubstrateResult<()> {
        self.exiger_ecriture("restaurer l'en-tête meta")?;
        let n = crate::meta::META_FILE_SIZE.min(octets.len());
        match &mut self.meta {
            MetaBacking::Mmap { file, map } => {
                map[..n].copy_from_slice(&octets[..n]);
                map.flush()?;
                file.sync_all()?;
            }
            MetaBacking::Ram(z) => {
                // SAFETY: écriture de l'en-tête, aucune vue vivante (contrat).
                let dst = unsafe { z.as_slice_mut() };
                let n = n.min(dst.len());
                dst[..n].copy_from_slice(&octets[..n]);
            }
        }
        Ok(())
    }

    /// Le substrat vit-il EN MÉMOIRE (aucun système de fichiers) ?
    ///
    /// Les rares endroits qui doivent choisir leur support — le WAL
    /// (`Writer::new`), les gardes lisant l'état durable — consultent ce
    /// prédicat au lieu de deviner d'après un chemin.
    pub fn is_memory(&self) -> bool {
        self.ram.is_some()
    }

    /// Path of a given zone file.
    pub fn zone_path(&self, zone: Zone) -> PathBuf {
        self.path.join(zone.filename())
    }

    /// Persiste la grille de tri `(order, max_degree)` du dernier bulk sort
    /// dans le sidecar `substrate.zgrid` (12 octets LE :
    /// magic `ZGRD` | order | max_degree, écriture tmp+rename+fsync).
    ///
    /// Sémantique « hint » (RFC zorder-superposition, D3b) : un échec
    /// d'écriture est loggé et NON fatal — l'absence du fichier désactive
    /// seulement l'accélération zruns des fenêtres 2D (repli scan exact).
    pub fn write_zgrid(&self, order: u32, max_degree: u32) -> SubstrateResult<()> {
        self.exiger_ecriture("écrire la grille de tri (substrate.zgrid)")?;
        use std::io::Write as _;
        const ZGRID_MAGIC: u32 = 0x5A47_5244; // "ZGRD"
        let final_path = self.path.join(zone::ZGRID);
        let tmp_path = self.path.join(format!("{}.tmp", zone::ZGRID));
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(&ZGRID_MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&order.to_le_bytes());
        bytes[8..12].copy_from_slice(&max_degree.to_le_bytes());
        let res = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp_path, &final_path)?;
            Ok(())
        })();
        match res {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    path = %final_path.display(),
                    error = %e,
                    "write_zgrid failed (hint — acceleration zruns désactivée)"
                );
                Ok(())
            }
        }
    }

    /// Lit la grille de tri persistée `(order, max_degree)`.
    ///
    /// Retourne `None` si le fichier est absent, trop court, ou de magic
    /// invalide — jamais d'erreur (hint). Lecture par `fs::read` (pas de
    /// mmap) : fonctionne aussi sur les ouvertures en lecture seule.
    pub fn read_zgrid(&self) -> Option<(u32, u32)> {
        const ZGRID_MAGIC: u32 = 0x5A47_5244; // "ZGRD"
        let path = self.path.join(zone::ZGRID);
        let bytes = std::fs::read(&path).ok()?;
        if bytes.len() < 12 {
            return None;
        }
        let rd = |r: std::ops::Range<usize>| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(r)?.try_into().ok()?))
        };
        if rd(0..4)? != ZGRID_MAGIC {
            return None;
        }
        Some((rd(4..8)?, rd(8..12)?))
    }

    /// Open (or re-open) a zone mmap handle. Each call creates a fresh
    /// [`ZoneFile`]; the caller is expected to keep it alive for as long as
    /// writes/reads are needed.
    pub fn open_zone(&self, zone: Zone) -> SubstrateResult<ZoneFile> {
        if let Some(dir) = &self.ram {
            return Ok(ZoneFile::open_ram(
                zone.filename(),
                dir.zone(zone.filename(), 0),
            ));
        }
        if let Some(reg) = &self.prive {
            return Ok(ZoneFile::open_privee(
                reg.zone(&self.path, zone.filename())?,
            ));
        }
        ZoneFile::open_or_create(&self.zone_path(zone))
    }

    /// Open a zone file by raw filename inside the substrate directory.
    ///
    /// This is the escape hatch for zones whose filenames are built
    /// dynamically from per-column metadata — currently only the
    /// `vec_column` subsystem (`substrate.veccol.<key>.<dtype>.<dim>`,
    /// one file per distinct prop-key / dimension). Each call creates a
    /// fresh [`ZoneFile`]; the caller keeps it alive for the window
    /// during which writes / reads are issued.
    ///
    /// The `filename` is interpreted verbatim as a file name **inside**
    /// `self.path()`; the caller must not pass a path with directory
    /// separators. We enforce this with a debug-assertion to catch
    /// caller bugs early without an extra syscall in release.
    pub fn open_named_zone(&self, filename: &str) -> SubstrateResult<ZoneFile> {
        debug_assert!(
            !filename.contains(std::path::MAIN_SEPARATOR) && !filename.contains('/'),
            "open_named_zone: filename must not contain path separators (got {filename:?})"
        );
        if let Some(dir) = &self.ram {
            return Ok(ZoneFile::open_ram(filename, dir.zone(filename, 0)));
        }
        if let Some(reg) = &self.prive {
            return Ok(ZoneFile::open_privee(reg.zone(&self.path, filename)?));
        }
        ZoneFile::open_or_create(&self.path.join(filename))
    }

    /// Ouvre une zone en LECTURE SEULE (le bridge RO partagé lit les zones
    /// journal sans exiger l'écriture — `open_or_create` refuse en RO).
    ///
    /// En mode mémoire, « lecture seule » est une PROMESSE de l'appelant, pas un
    /// mécanisme : il n'y a aucun moyen de refuser une écriture sur des octets
    /// partagés. La zone doit exister, comme en mode fichier.
    pub fn open_named_zone_read_only(&self, filename: &str) -> SubstrateResult<ZoneFile> {
        debug_assert!(
            !filename.contains(std::path::MAIN_SEPARATOR) && !filename.contains('/'),
            "open_named_zone: filename must not contain path separators (got {filename:?})"
        );
        if let Some(dir) = &self.ram {
            return match dir.zone_existante(filename) {
                Some(z) => Ok(ZoneFile::open_ram(filename, z)),
                None => Err(SubstrateError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("zone not found: {filename} (substrat en mémoire)"),
                ))),
            };
        }
        let path = self.path.join(filename);
        if !path.exists() {
            return Err(SubstrateError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("zone not found: {}", path.display()),
            )));
        }
        // Ouverture en lecture seule : la zone passe par le registre privé,
        // comme toutes les autres (une seule projection par fichier).
        if let Some(reg) = &self.prive {
            return Ok(ZoneFile::open_privee(reg.zone(&self.path, filename)?));
        }
        ZoneFile::open_read_only_zone(&path)
    }

    /// L'ouverture est-elle PARTAGÉE (verrou `LOCK_SH`) ? Voir le champ.
    pub fn est_ecriture_partagee(&self) -> bool {
        self.partage
    }

    /// Path of the WAL file.
    pub fn wal_path(&self) -> PathBuf {
        self.path.join(zone::WAL)
    }

    /// Path of the checkpoint marker.
    pub fn checkpoint_path(&self) -> PathBuf {
        self.path.join(zone::CHECKPOINT)
    }

    /// Fsync the meta file. Typically called at the end of a checkpoint.
    pub fn fsync_meta(&self) -> SubstrateResult<()> {
        // Lecture seule : la projection est privée, il n'y a rien à pousser.
        if self.est_lecture_seule() {
            return Ok(());
        }
        if let MetaBacking::Mmap { file, map } = &self.meta {
            map.flush()?;
            file.sync_all()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tempdir guard — owns the temporary directory and deletes it on drop.
// ---------------------------------------------------------------------------

mod tempfile_guard {
    use std::path::{Path, PathBuf};

    #[derive(Debug)]
    pub struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        pub fn new(prefix: &str) -> std::io::Result<Self> {
            // Build a unique directory in std::env::temp_dir(). We use
            // `tempfile::TempDir` indirectly via `std::env::temp_dir` +
            // process-id + monotonic counter to avoid pulling an extra dep —
            // but the `tempfile` crate is already in dev-dependencies, so for
            // tests we can rely on it; here we roll our own for the runtime
            // path since `tempfile` is a dev-dep only.
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "{prefix}{pid}-{nanos}-{n}",
                pid = crate::wal_io::pid_portable()
            ));
            std::fs::create_dir_all(&path)?;
            Ok(Self { path })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_then_open_roundtrip() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb1");
        let s = SubstrateFile::create(&sub_path).unwrap();
        let h = s.meta_header();
        assert!(h.is_valid_magic());
        assert_eq!(h.format_version, SUBSTRATE_FORMAT_VERSION);
        assert_eq!(h.node_count, 0);
        drop(s);
        let s2 = SubstrateFile::open(&sub_path).unwrap();
        assert!(s2.meta_header().is_valid_magic());
    }

    /// (2026-08-31) NON-RÉGRESSION du verrou PARTAGÉ read-only : deux
    /// ouvertures RO (verrou `LOCK_SH`) coexistent dans le même processus ;
    /// une ouverture RW (exclusive) pendant qu'un RO tient est refusée.
    #[test]
    fn verrou_partage_ro_coexiste_et_bloque_rw() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb-ro");
        drop(SubstrateFile::create(&sub_path).unwrap());

        // 1. Deux RO simultanés : OK (verrou partagé intra-processus).
        let ro1 = SubstrateFile::open_read_only(&sub_path).unwrap();
        let ro2 = SubstrateFile::open_read_only(&sub_path).unwrap();
        assert!(ro1.meta_header().is_valid_magic());
        assert!(ro2.meta_header().is_valid_magic());

        // 2. RW pendant qu'un RO tient : REFUSÉ (le registre ne permet pas
        //    d'upgrader une part partagée en exclusive).
        let err = SubstrateFile::open(&sub_path).unwrap_err();
        matches!(err, SubstrateError::Io(_));

        // 3. Après fermeture des RO, RW repasse.
        drop(ro1);
        drop(ro2);
        let rw = SubstrateFile::open(&sub_path).unwrap();
        assert!(rw.meta_header().is_valid_magic());
    }

    #[test]
    fn double_create_fails() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb1");
        SubstrateFile::create(&sub_path).unwrap();
        let err = SubstrateFile::create(&sub_path).unwrap_err();
        matches!(err, SubstrateError::Io(_));
    }

    #[test]
    fn open_nonexistent_fails() {
        let dir = tempdir().unwrap();
        let err = SubstrateFile::open(dir.path().join("nope")).unwrap_err();
        matches!(err, SubstrateError::Io(_));
    }

    #[test]
    fn open_wrong_magic_fails() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("bad");
        std::fs::create_dir_all(&sub_path).unwrap();
        // Write a bogus meta file.
        let meta_path = sub_path.join(zone::META);
        std::fs::write(&meta_path, vec![0xFFu8; META_FILE_SIZE]).unwrap();
        let err = SubstrateFile::open(&sub_path).unwrap_err();
        assert!(matches!(err, SubstrateError::BadMagic));
    }

    #[test]
    fn tempfile_auto_deletes() {
        let path: PathBuf;
        {
            let s = SubstrateFile::open_tempfile().unwrap();
            path = s.path().to_path_buf();
            assert!(path.join(zone::META).exists());
        }
        // After drop, the tempdir should be gone.
        assert!(!path.exists());
    }

    #[test]
    fn zones_are_created_empty() {
        let s = SubstrateFile::open_tempfile().unwrap();
        for name in zone::ALL_MMAPPED {
            assert!(
                s.path().join(name).exists(),
                "zone file not created: {name}"
            );
        }
        assert!(s.path().join(zone::WAL).exists());
    }

    #[test]
    fn write_meta_header_persists() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let mut s = SubstrateFile::create(&sub_path).unwrap();
        let mut h = s.meta_header();
        h.node_count = 42;
        h.edge_count = 137;
        s.write_meta_header(&h).unwrap();
        drop(s);
        let s2 = SubstrateFile::open(&sub_path).unwrap();
        assert_eq!(s2.meta_header().node_count, 42);
        assert_eq!(s2.meta_header().edge_count, 137);
    }

    #[test]
    fn zonefile_grow_and_write() {
        let s = SubstrateFile::open_tempfile().unwrap();
        let mut zf = s.open_zone(Zone::Nodes).unwrap();
        // (jalon 2) la zone nodes nait avec le record sentinelle de stride
        // au slot 0 — plus jamais vide a la creation.
        assert_eq!(zf.len(), crate::record::NodeRecord::SIZE as u64);
        zf.grow_to(64).unwrap();
        assert_eq!(zf.len(), 64);
        zf.as_slice_mut()[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        zf.msync().unwrap();
        zf.fsync().unwrap();
        // Reopen via a fresh ZoneFile and check bytes.
        let zf2 = s.open_zone(Zone::Nodes).unwrap();
        assert_eq!(&zf2.as_slice()[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn zonefile_refuses_to_shrink() {
        let s = SubstrateFile::open_tempfile().unwrap();
        let mut zf = s.open_zone(Zone::Nodes).unwrap();
        zf.grow_to(4096).unwrap();
        let err = zf.grow_to(1024).unwrap_err();
        assert!(matches!(err, SubstrateError::WalBadFrame(_)));
    }

    // ═══════════════════════════════════════════════════════════════════
    //  VERROU DE RÉPERTOIRE
    // ═══════════════════════════════════════════════════════════════════

    /// Variable qui porte, à l'enfant de la sonde, le chemin du fichier de
    /// verrou à tenter. Absente : `sonde_verrou_enfant_exterieur` est inerte.
    const SONDE_VERROU_ENV: &str = "OBRAIN_SONDE_VERROU_EXTERIEUR";

    /// Un détenteur EXTÉRIEUR est un AUTRE PROCESSUS, rien de moins. Le verrou
    /// est un verrou de processus (`fcntl`, voir `verrouiller_fichier`) : un
    /// second descripteur ouvert ICI l'obtiendrait (même processus) et, pire,
    /// sa fermeture rendrait celui de la part vivante — la simulation d'avant
    /// (un descripteur indépendant, fidèle pour `flock`) dirait désormais le
    /// contraire de la réalité. La sonde engendre donc le binaire de test
    /// lui-même, qui tente la pose depuis son propre processus et répond par
    /// son code de sortie.
    fn un_autre_detenteur_peut_il_verrouiller(verrou: &Path) -> bool {
        let exe = std::env::current_exe().expect("binaire de test");
        std::process::Command::new(exe)
            .args([
                "--exact",
                "--test-threads=1",
                "file::tests::sonde_verrou_enfant_exterieur",
            ])
            .env(SONDE_VERROU_ENV, verrou)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("engendrer la sonde")
            .success()
    }

    /// Côté ENFANT de la sonde : tente le verrou exclusif sur le fichier que
    /// désigne `OBRAIN_SONDE_VERROU_EXTERIEUR`, et échoue (code de sortie
    /// non nul) s'il est refusé. Sans la variable — exécution ordinaire de la
    /// suite — ne fait rien et passe.
    #[test]
    fn sonde_verrou_enfant_exterieur() {
        let Some(verrou) = std::env::var_os(SONDE_VERROU_ENV) else {
            return;
        };
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&verrou)
            .expect("ouvrir le fichier de verrou");
        assert!(
            verrouiller_fichier(&f, true).is_ok(),
            "verrou tenu par un autre processus"
        );
    }

    /// LE CAS QUI A CASSÉ LA PRODUCTION. Le pool d'`obrain-tmn` ouvre une même
    /// base deux fois en concurrence — délibérément, hors du verrou de sa
    /// table — puis jette le perdant. Un `flock` nu refusait la seconde
    /// ouverture et faisait tomber le test de non-régression de la famine du
    /// 2026-07-23. Les deux ouvertures doivent réussir.
    #[test]
    fn deux_ouvertures_du_meme_repertoire_dans_ce_processus_reussissent() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let a = SubstrateFile::create(&chemin).expect("premiere ouverture");
        let b = SubstrateFile::open(&chemin).expect(
            "la SECONDE ouverture dans le meme processus doit reussir — \
             c'est la course benigne du pool (obrain_db_pool.rs:714)",
        );
        // Les deux sont utilisables, et voient la même en-tête.
        assert!(a.meta_header().is_valid_magic());
        assert!(b.meta_header().is_valid_magic());
    }

    /// L'autre moitié de l'invariant : un verrou qui ne bloque plus personne
    /// serait aussi faux que celui qui bloquait tout le monde. C'est lui qui
    /// justifie le `allow(unsafe_code)` du module.
    #[test]
    fn un_detenteur_exterieur_est_refuse_pendant_qu_une_part_vit() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let verrou = chemin.join(zone::DIR_LOCK);
        let sf = SubstrateFile::create(&chemin).unwrap();
        assert!(
            !un_autre_detenteur_peut_il_verrouiller(&verrou),
            "un detenteur EXTERIEUR ne doit pas pouvoir verrouiller pendant qu'une part vit"
        );
        drop(sf);
    }

    /// Le verrou ne doit être rendu qu'à la disparition de la DERNIÈRE part.
    /// Fermer la première ouverture pendant que la seconde vit encore
    /// libérerait le répertoire alors qu'il est toujours mmappé — soit
    /// exactement la corruption que l'invariant existe pour empêcher.
    #[test]
    fn le_verrou_survit_a_la_fermeture_d_une_part_et_tombe_avec_la_derniere() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let verrou = chemin.join(zone::DIR_LOCK);

        let a = SubstrateFile::create(&chemin).unwrap();
        let b = SubstrateFile::open(&chemin).unwrap();

        drop(a);
        assert!(
            !un_autre_detenteur_peut_il_verrouiller(&verrou),
            "le verrou ne doit PAS etre rendu tant qu'une part vit encore"
        );

        drop(b);
        assert!(
            un_autre_detenteur_peut_il_verrouiller(&verrou),
            "le verrou doit etre rendu quand la DERNIERE part disparait"
        );
    }

    /// NON-RÉGRESSION (2026-10-12, perte de messages multi-writer).
    ///
    /// Le checkpoint réécrit `substrate.meta` par `tmp + rename` — ce qui
    /// REMPLACE son inode. Avant le correctif, le verrou d'écrivain était posé
    /// sur `substrate.meta` : après le rename, un second processus pouvait
    /// prendre un `flock` frais sur le nouvel inode, et deux écrivains
    /// coexistaient (WAL corrompu). Le verrou vit désormais sur
    /// `substrate.dirlock`, un inode STABLE que le checkpoint ne renomme pas.
    ///
    /// Ce test simule exactement la réécriture atomique du meta pendant qu'une
    /// part exclusive vit, puis vérifie qu'un détenteur extérieur est TOUJOURS
    /// refusé — le verrou a survécu au rename.
    #[test]
    fn le_verrou_survit_a_la_reecriture_atomique_du_meta() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let verrou = chemin.join(zone::DIR_LOCK);
        let sf = SubstrateFile::create(&chemin).unwrap();

        // Simule `checkpoint::atomically_write_meta` : tmp + rename sur le meta.
        let meta = chemin.join(zone::META);
        let tmp = chemin.join(format!("{}.tmp", zone::META));
        let len = std::fs::metadata(&meta).unwrap().len();
        std::fs::write(&tmp, vec![0u8; len as usize]).unwrap();
        std::fs::rename(&tmp, &meta).unwrap();

        // Le verrou doit tenir sur substrate.dirlock, inode intact.
        assert!(
            !un_autre_detenteur_peut_il_verrouiller(&verrou),
            "le verrou d'ecrivain doit survivre au rename atomique du meta (checkpoint)"
        );
        drop(sf);
    }

    /// Le registre est indexé sur le chemin CANONISÉ : deux écritures
    /// différentes du même répertoire doivent tomber sur la même entrée.
    /// Sinon le registre croirait à deux répertoires distincts et le second
    /// `flock` échouerait sur le premier.
    #[test]
    fn deux_ecritures_du_meme_chemin_partagent_la_meme_part() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");
        let a = SubstrateFile::create(&chemin).unwrap();
        // Même répertoire, écrit avec un détour. Le répertoire traversé doit
        // EXISTER : `canonicalize` échoue si un composant manque, et le
        // registre retombe alors sur le chemin brut — deux clés distinctes
        // pour un même répertoire, donc un `flock` qui se refuse lui-même.
        // Cest une limite RÉELLE de limplémentation, pas du test : un chemin
        // non canonisable ne partage pas.
        std::fs::create_dir_all(td.path().join("autre")).unwrap();
        let detour = td.path().join("autre").join("..").join("kb");
        let b = SubstrateFile::open(&detour)
            .expect("un chemin equivalent doit partager la part, pas se refuser lui-meme");
        assert!(a.meta_header().is_valid_magic());
        assert!(b.meta_header().is_valid_magic());
    }

    // ═══════════════════════════════════════════════════════════════════
    //  FILET MMAP — invariants de ZoneFile AVANT la résolution mmap
    //  (huge pages conditionnelles, advise_dontneed, remap).
    //
    //  Cible verrouillée : open_or_create / grow_to / advise_huge_pages /
    //  advise_dontneed / advise_willneed. Ce que ces tests protègent, dans
    //  l'ordre de criticité :
    //   1. write-through : ce qu'on écrit dans le mmap se relit à l'octet près
    //      après reouverture (open_or_create garde le contenu, mmap partagé) ;
    //   2. survie du contenu à la CROISSANCE (grow_to → set_len + remap) — c'est
    //      le WAL/store, aucune perte tolérée, y compris en franchissant le
    //      seuil huge (8 Mio) où MADV_HUGEPAGE entre en jeu ;
    //   3. DONTNEED est bénin sur un mapping fichier partagé (re-fault correct,
    //      zone toujours écrivable) — jamais appelé en prod aujourd'hui, mais
    //      candidat de la résolution mmap ;
    //   4. frontières de page (4 Ko, 2 Mio) et bornes (dernier octet, zone vide,
    //      zone exactement au seuil) tiennent au remap.
    // ═══════════════════════════════════════════════════════════════════

    /// Motif déterministe couvrant TOUT l'octet (0x00..=0xFF présents), avec un
    /// blob UTF-8 (unicode multi-octets), un NUL, des scalaires aux bornes, et
    /// 0xFF / 0x00 aux deux derniers octets. `sel` sale la région pour que deux
    /// zones écrites dans le même test diffèrent.
    fn motif(len: usize, sel: u8) -> Vec<u8> {
        let mut v = vec![0u8; len];
        for (i, b) in v.iter_mut().enumerate() {
            *b = ((i as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((sel as u64).wrapping_mul(0x1000_0001))
                >> 11) as u8;
        }
        // Unicode multi-octets + NUL + scalaires aux bornes de plan.
        let blob = "abcé→世界\u{0}\u{7f}\u{80}\u{ffff}".as_bytes();
        let n = blob.len().min(len);
        v[..n].copy_from_slice(&blob[..n]);
        if len >= 1 {
            v[len - 1] = 0xFF;
        }
        if len >= 2 {
            v[len - 2] = 0x00;
        }
        v
    }

    fn zone_jetable(dir: &tempfile::TempDir, nom: &str) -> ZoneFile {
        ZoneFile::open_or_create(&dir.path().join(nom)).unwrap()
    }

    /// INVARIANT 1 — write-through : le motif écrit dans le mmap se relit à
    /// l'octet près après fermeture + réouverture. Verrouille `open_or_create`
    /// (`truncate(false)`, garde le contenu) et le mmap PARTAGÉ (les écritures
    /// atterrissent dans le fichier).
    #[test]
    fn mmap_roundtrip_survit_a_reouverture() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("z");
        let len = 4096usize;
        let attendu = motif(len, 0);
        {
            let mut zf = ZoneFile::open_or_create(&p).unwrap();
            assert!(zf.is_empty(), "résultat attendu vide");
            zf.grow_to(len as u64).unwrap();
            zf.as_slice_mut().copy_from_slice(&attendu);
            zf.msync().unwrap();
            zf.fsync().unwrap();
        }
        let zf2 = ZoneFile::open_or_create(&p).unwrap();
        assert_eq!(zf2.len(), len as u64);
        assert_eq!(
            zf2.as_slice(),
            &attendu[..],
            "le write-through doit persister a l'octet pres apres reouverture"
        );
    }

    /// INVARIANT 2 (CRITIQUE, WAL/store) — le contenu survit à `grow_to` /
    /// remap sans reouverture : les octets écrits avant chaque croissance
    /// restent intacts, la région neuve est zéro-remplie, et `remap_count`
    /// suit exactement le nombre de croissances effectives.
    #[test]
    fn mmap_contenu_survit_a_grow_et_remap() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let (a, b, c) = (4096usize, 65_536usize, 200_003usize);
        let p0 = motif(a, 0);
        zf.grow_to(a as u64).unwrap();
        zf.as_slice_mut().copy_from_slice(&p0);
        assert_eq!(zf.remap_count(), 1);

        zf.grow_to(b as u64).unwrap();
        assert_eq!(zf.remap_count(), 2);
        assert_eq!(
            &zf.as_slice()[..a],
            &p0[..],
            "les octets ecrits avant le grow doivent survivre au remap"
        );
        assert!(
            zf.as_slice()[a..b].iter().all(|&x| x == 0),
            "la region neuve doit etre zero-remplie"
        );
        let p1 = motif(b - a, 1);
        zf.as_slice_mut()[a..b].copy_from_slice(&p1);

        zf.grow_to(c as u64).unwrap();
        assert_eq!(zf.remap_count(), 3);
        assert_eq!(&zf.as_slice()[..a], &p0[..]);
        assert_eq!(&zf.as_slice()[a..b], &p1[..]);
        assert!(zf.as_slice()[b..c].iter().all(|&x| x == 0));
    }

    /// INVARIANT 2 bis — croissance À TRAVERS le seuil huge (8 Mio) : le grow
    /// qui fait franchir `HUGE_PAGE_MIN_BYTES` déclenche `advise_huge_pages`
    /// (MADV_HUGEPAGE sur Linux). Le contenu écrit AVANT doit survivre, et la
    /// zone doit rester lisible ET écrivable APRÈS l'advise.
    #[test]
    fn mmap_contenu_survit_a_grow_a_travers_le_seuil_huge() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let sous = (HUGE_PAGE_MIN_BYTES - 4096) as usize; // sous le seuil : pas d'advise
        let sur = (HUGE_PAGE_MIN_BYTES + 2 * 1024 * 1024) as usize; // au-dessus + une frontiere huge
        let p = motif(sous, 0);
        zf.grow_to(sous as u64).unwrap();
        zf.as_slice_mut().copy_from_slice(&p);

        zf.grow_to(sur as u64).unwrap(); // franchit le seuil -> advise_huge_pages s'applique
        assert_eq!(
            &zf.as_slice()[..sous],
            &p[..],
            "le contenu doit survivre au grow qui franchit le seuil huge (WAL/store)"
        );
        // La zone reste LISIBLE et ECRIVABLE apres MADV_HUGEPAGE.
        let q = motif(sur - sous, 1);
        zf.as_slice_mut()[sous..sur].copy_from_slice(&q);
        assert_eq!(&zf.as_slice()[sous..sur], &q[..]);
        assert_eq!(&zf.as_slice()[..sous], &p[..]);
    }

    /// BORNE — zone EXACTEMENT au seuil huge (`len == HUGE_PAGE_MIN_BYTES`) :
    /// l'advise s'applique (`len < seuil` est faux). La zone doit être
    /// entièrement lisible/écrivable, dernier octet compris, avant et après
    /// réécriture complète.
    #[test]
    fn mmap_zone_exactement_au_seuil_huge_reste_lisible_et_ecrivable() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let n = HUGE_PAGE_MIN_BYTES as usize;
        zf.grow_to(n as u64).unwrap();
        let p = motif(n, 0);
        zf.as_slice_mut().copy_from_slice(&p);
        assert_eq!(zf.as_slice(), &p[..]);
        let q = motif(n, 2);
        zf.as_slice_mut().copy_from_slice(&q);
        assert_eq!(zf.as_slice(), &q[..]);
        assert_eq!(
            zf.as_slice()[n - 1],
            q[n - 1],
            "dernier octet de la zone au seuil"
        );
    }

    /// INVARIANT 3 — `advise_dontneed` est BÉNIN sur un mapping fichier
    /// PARTAGÉ. On écrit P1, on le `msync` (dans le fichier), puis P2 SANS
    /// msync (pages sales), puis DONTNEED. Le re-fault doit rendre P2 (l'état
    /// courant), jamais un retour à P1 : un mapping privé (MAP_PRIVATE)
    /// reviendrait à P1 — c'est le piège que ce test verrouille. La zone reste
    /// écrivable après DONTNEED.
    #[test]
    fn mmap_dontneed_refault_sans_corruption_et_reste_ecrivable() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let n = HUGE_PAGE_MIN_BYTES as usize; // >= seuil : le cas le plus proche de la prod
        zf.grow_to(n as u64).unwrap();
        let p1 = motif(n, 0);
        zf.as_slice_mut().copy_from_slice(&p1);
        zf.msync().unwrap(); // P1 pousse dans le fichier
        let p2 = motif(n, 1);
        zf.as_slice_mut().copy_from_slice(&p2); // P2 : pages sales, PAS de msync
        // SAFETY: aucune reference &[u8] vivante sur cette zone pendant l'appel.
        unsafe {
            zf.advise_dontneed().unwrap();
        }
        assert_eq!(
            zf.as_slice(),
            &p2[..],
            "DONTNEED ne doit ni corrompre ni annuler les ecritures sales (mapping PARTAGE)"
        );
        let p3 = motif(n, 3);
        zf.as_slice_mut().copy_from_slice(&p3);
        assert_eq!(
            zf.as_slice(),
            &p3[..],
            "la zone doit rester ecrivable apres DONTNEED"
        );
    }

    /// BORNE — `advise_dontneed` sur une zone VIDE (map `None`) est un no-op
    /// silencieux, pas une panique.
    #[test]
    fn mmap_dontneed_sur_zone_vide_est_un_noop() {
        let dir = tempdir().unwrap();
        let zf = zone_jetable(&dir, "z");
        assert!(zf.is_empty(), "résultat attendu vide");
        // SAFETY: zone vide, aucune reference vivante.
        unsafe {
            zf.advise_dontneed().unwrap();
        }
    }

    /// INVARIANT — `advise_willneed` clampe `offset + len` à `self.len` : une
    /// plage qui déborde la zone (typique de `last_slot + 1` sur la dernière
    /// page partielle) ne doit ni paniquer ni échouer. Couvre aussi zone vide,
    /// offset au-delà de la fin, et `len == 0`.
    #[test]
    fn mmap_willneed_clampe_les_depassements_sans_paniquer() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        zf.advise_willneed(0, 4096).unwrap(); // zone vide : no-op
        zf.grow_to(8192).unwrap();
        zf.advise_willneed(4096, 1 << 20).unwrap(); // deborde largement -> clampe
        zf.advise_willneed(8192, 4096).unwrap(); // offset == fin : no-op
        zf.advise_willneed(999_999, 10).unwrap(); // offset au-dela : no-op
        zf.advise_willneed(0, 0).unwrap(); // len 0 : no-op
    }

    /// FRONTIÈRES DE PAGE — sentinelles à cheval sur 4 Ko et 2 Mio (avant / sur
    /// / après la frontière), qui doivent survivre à l'octet près au remap.
    #[test]
    fn mmap_frontieres_de_page_4k_et_2m_survivent_au_remap() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let n = 2 * 1024 * 1024 + 8192usize; // franchit 4 Ko et 2 Mio
        zf.grow_to(n as u64).unwrap();
        let bornes = [4096usize, 2 * 1024 * 1024];
        for (k, &off) in bornes.iter().enumerate() {
            zf.as_slice_mut()[off - 1] = 0xA0 + k as u8;
            zf.as_slice_mut()[off] = 0xB0 + k as u8;
            zf.as_slice_mut()[off + 1] = 0xC0 + k as u8;
        }
        zf.grow_to((n + 4096) as u64).unwrap(); // remap
        for (k, &off) in bornes.iter().enumerate() {
            assert_eq!(
                zf.as_slice()[off - 1],
                0xA0 + k as u8,
                "octet avant la frontiere {off}"
            );
            assert_eq!(
                zf.as_slice()[off],
                0xB0 + k as u8,
                "octet a la frontiere {off}"
            );
            assert_eq!(
                zf.as_slice()[off + 1],
                0xC0 + k as u8,
                "octet apres la frontiere {off}"
            );
        }
    }

    /// BORNE — écriture au DERNIER octet avant remap : elle survit au grow, la
    /// région neuve est zéro-remplie, et le nouveau dernier octet est écrivable.
    #[test]
    fn mmap_ecriture_au_dernier_octet_avant_remap_survit() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let n = 4096usize;
        zf.grow_to(n as u64).unwrap();
        zf.as_slice_mut()[n - 1] = 0x5A;
        let m = 3 * n;
        zf.grow_to(m as u64).unwrap();
        assert_eq!(
            zf.as_slice()[n - 1],
            0x5A,
            "le dernier octet ecrit avant le remap doit survivre"
        );
        assert!(
            zf.as_slice()[n..m].iter().all(|&x| x == 0),
            "la region neuve est zero-remplie"
        );
        zf.as_slice_mut()[m - 1] = 0xA5;
        assert_eq!(
            zf.as_slice()[m - 1],
            0xA5,
            "le nouveau dernier octet est ecrivable"
        );
    }

    /// BORNE — `grow_to` vers la MÊME taille est un no-op : pas de remap, pas
    /// de perturbation du contenu.
    #[test]
    fn mmap_grow_egal_est_un_noop_et_ne_remap_pas() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        zf.grow_to(4096).unwrap();
        let p = motif(4096, 0);
        zf.as_slice_mut().copy_from_slice(&p);
        assert_eq!(zf.remap_count(), 1);
        zf.grow_to(4096).unwrap(); // egal : no-op
        assert_eq!(
            zf.remap_count(),
            1,
            "un grow vers la meme taille ne doit pas remapper"
        );
        assert_eq!(
            zf.as_slice(),
            &p[..],
            "le no-op ne doit pas toucher au contenu"
        );
    }

    /// BORNE — un fichier VIDE n'est pas mappé et expose un slice vide dans les
    /// deux sens (lecture et écriture).
    #[test]
    fn mmap_fichier_vide_expose_un_slice_vide_et_pas_de_map() {
        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        assert_eq!(zf.len(), 0);
        assert!(zf.is_empty(), "résultat attendu vide");
        assert!(
            matches!(&zf.backing, ZoneBacking::Mmap { map: None, .. }),
            "un fichier vide ne doit pas etre mappe"
        );
        assert_eq!(zf.as_slice(), &[] as &[u8]);
        assert!(zf.as_slice_mut().is_empty(), "résultat attendu vide");
    }

    // ═══════════════════════════════════════════════════════════════════
    //  WS-B — huge pages CONDITIONNELLES (flag OBRAIN_TMN_HUGEPAGE)
    //
    //  `advise_huge_pages` ne pose `MADV_HUGEPAGE` que si, et seulement si,
    //  `should_advise_huge_pages(len, huge_pages_enabled())` est vrai : c'est
    //  sa SEULE porte. Tester ces deux fonctions verrouille donc, de bout en
    //  bout, « le flag pilote l'application de l'advise » — falsifiable sans
    //  avoir à observer le madvise dans /proc/self/smaps.
    // ═══════════════════════════════════════════════════════════════════

    /// Sérialise les tests qui modifient `OBRAIN_TMN_HUGEPAGE` : la variable
    /// d'environnement est PROCESS-WIDE et `set_var`/`remove_var` sont `unsafe`
    /// précisément parce qu'ils courent avec les lecteurs concurrents.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// FALSIFIABILITÉ — la décision d'advise est pilotée par le flag ET par le
    /// seuil de taille, RIEN d'autre. Flag ON + zone ≥ seuil ⇒ advise ; flag OFF
    /// ⇒ jamais d'advise, quelle que soit la taille ; sous le seuil ⇒ pas
    /// d'advise même flag ON. Casse le lien flag→advise et ce test tombe.
    #[test]
    fn should_advise_huge_pages_est_pilote_par_le_flag_et_le_seuil() {
        let seuil = HUGE_PAGE_MIN_BYTES;
        // Flag ON (défaut) + zone au seuil ou au-dessus ⇒ advise posé.
        assert!(
            should_advise_huge_pages(seuil, true),
            "ON + len == seuil ⇒ advise"
        );
        assert!(
            should_advise_huge_pages(seuil * 8, true),
            "ON + grande zone ⇒ advise"
        );
        // Flag OFF ⇒ AUCUN advise, même très au-dessus du seuil : c'est le levier WS-B.
        assert!(
            !should_advise_huge_pages(seuil * 8, false),
            "OFF ⇒ pas d'advise, quelle que soit la taille"
        );
        assert!(
            !should_advise_huge_pages(seuil, false),
            "OFF + len == seuil ⇒ pas d'advise"
        );
        // Le seuil de taille reste opposable même flag ON (invariant pré-existant).
        assert!(
            !should_advise_huge_pages(seuil - 1, true),
            "ON mais sous le seuil ⇒ pas d'advise"
        );
        assert!(
            !should_advise_huge_pages(0, true),
            "ON + zone vide ⇒ pas d'advise"
        );
    }

    /// FALSIFIABILITÉ — `huge_pages_enabled()` lit bien l'environnement : défaut
    /// ON quand absente (comportement INCHANGÉ), OFF sur `"0"` uniquement, et
    /// toute AUTRE valeur retombe sur ON (le côté sûr : pas de régression
    /// séquentielle silencieuse). Restaure l'état initial de la variable en
    /// sortie.
    #[test]
    fn huge_pages_enabled_defaut_on_off_sur_zero() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let sauv = std::env::var("OBRAIN_TMN_HUGEPAGE").ok();

        // Absente ⇒ défaut ON : aucun déploiement ne change de comportement.
        unsafe { std::env::remove_var("OBRAIN_TMN_HUGEPAGE") };
        assert!(huge_pages_enabled(), "absente ⇒ défaut ON (inchangé)");

        // Seul "0" coupe les huge pages.
        unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", "0") };
        assert!(!huge_pages_enabled(), "\"0\" ⇒ OFF");

        // Toute autre valeur ⇒ ON (fallback sûr).
        for on in ["1", "true", "on", "yes", "banane", ""] {
            unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", on) };
            assert!(huge_pages_enabled(), "{on:?} ⇒ ON (fallback sûr)");
        }

        // Restaure l'état initial exact.
        match sauv {
            Some(v) => unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", v) },
            None => unsafe { std::env::remove_var("OBRAIN_TMN_HUGEPAGE") },
        }
    }

    /// FALSIFIABILITÉ (bout en bout) — le flag ABSENT pose l'advise, le flag
    /// `"0"` le SAUTE, aux DEUX sites (`open_or_create` et `grow_to`). On passe
    /// par `advise_huge_pages` lui-même (via `huge_pages_enabled()`), donc c'est
    /// la porte réelle qui est testée, pas seulement la décision pure.
    #[test]
    fn flag_pilote_l_advise_aux_deux_sites() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let sauv = std::env::var("OBRAIN_TMN_HUGEPAGE").ok();
        let seuil = HUGE_PAGE_MIN_BYTES;

        // Défaut (absente) ⇒ ON : advise posé aux deux sites.
        unsafe { std::env::remove_var("OBRAIN_TMN_HUGEPAGE") };
        assert!(
            should_advise_huge_pages(seuil, huge_pages_enabled()),
            "absente + open_or_create sur zone ≥ seuil ⇒ advise posé"
        );

        // "0" ⇒ OFF : advise sauté aux deux sites, même sur grande zone.
        unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", "0") };
        assert!(
            !should_advise_huge_pages(seuil, huge_pages_enabled()),
            "\"0\" + open_or_create ⇒ advise SAUTÉ"
        );
        assert!(
            !should_advise_huge_pages(seuil * 8, huge_pages_enabled()),
            "\"0\" + grow_to/remap ⇒ advise SAUTÉ quelle que soit la taille"
        );

        match sauv {
            Some(v) => unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", v) },
            None => unsafe { std::env::remove_var("OBRAIN_TMN_HUGEPAGE") },
        }
    }

    /// GARDE — le flag ON (défaut) laisse une zone ≥ seuil pleinement lisible et
    /// écrivable à travers grow/remap : c'est la non-régression du chemin de
    /// PROD (défaut inchangé), pendant t. Le contenu écrit avant l'advise doit
    /// survivre au remap qui (re)pose MADV_HUGEPAGE.
    #[test]
    fn flag_on_zone_advisee_reste_lisible_et_ecrivable() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let sauv = std::env::var("OBRAIN_TMN_HUGEPAGE").ok();
        unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", "1") };

        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let n = (HUGE_PAGE_MIN_BYTES + 2 * 1024 * 1024) as usize; // ≥ seuil : advise posé
        zf.grow_to(n as u64).unwrap(); // advise_huge_pages appliqué (flag ON)
        let p = motif(n, 0);
        zf.as_slice_mut().copy_from_slice(&p);
        assert_eq!(zf.as_slice(), &p[..], "zone advisée lisible à l'octet près");
        let q = motif(n, 3);
        zf.as_slice_mut().copy_from_slice(&q);
        assert_eq!(zf.as_slice(), &q[..], "zone advisée toujours écrivable");

        match sauv {
            Some(v) => unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", v) },
            None => unsafe { std::env::remove_var("OBRAIN_TMN_HUGEPAGE") },
        }
    }

    /// GARDE — le flag OFF (le nouveau chemin, à mesurer au banc) n'introduit
    /// AUCUNE régression fonctionnelle : la même zone ≥ seuil, non advisée,
    /// reste lisible et écrivable à travers grow/remap. Seule la backing en
    /// pages change (4 Ko au lieu de 2 Mo) ; le contenu est intact.
    #[test]
    fn flag_off_zone_non_advisee_reste_lisible_et_ecrivable() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let sauv = std::env::var("OBRAIN_TMN_HUGEPAGE").ok();
        unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", "0") };

        let dir = tempdir().unwrap();
        let mut zf = zone_jetable(&dir, "z");
        let n = (HUGE_PAGE_MIN_BYTES + 2 * 1024 * 1024) as usize; // ≥ seuil mais flag "0" ⇒ pas d'advise
        zf.grow_to(n as u64).unwrap();
        let p = motif(n, 0);
        zf.as_slice_mut().copy_from_slice(&p);
        assert_eq!(
            zf.as_slice(),
            &p[..],
            "zone non advisée lisible à l'octet près"
        );
        let q = motif(n, 4);
        zf.as_slice_mut().copy_from_slice(&q);
        assert_eq!(zf.as_slice(), &q[..], "zone non advisée toujours écrivable");

        match sauv {
            Some(v) => unsafe { std::env::set_var("OBRAIN_TMN_HUGEPAGE", v) },
            None => unsafe { std::env::remove_var("OBRAIN_TMN_HUGEPAGE") },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — mode MÉMOIRE (le substrat sans système de fichiers)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_memoire {
    use super::*;

    /// LE test qui justifie le registre : deux poignées ouvertes sur la même
    /// zone doivent voir les MÊMES octets, comme deux mmaps du même fichier
    /// voient le même cache de pages. Sans le registre de `RamDir`, chacune
    /// posséderait son tampon et la seconde lirait des zéros — une corruption
    /// silencieuse, pas une erreur.
    #[test]
    fn deux_poignees_de_la_meme_zone_partagent_les_octets() {
        let sub = SubstrateFile::open_memory().expect("substrat en mémoire");

        let mut ecrivain = sub.open_named_zone("substrate.partagee").unwrap();
        ecrivain.grow_to(16).unwrap();
        ecrivain.as_slice_mut()[..5].copy_from_slice(b"alix!");

        let lecteur = sub.open_named_zone("substrate.partagee").unwrap();
        assert_eq!(lecteur.len(), 16);
        assert_eq!(&lecteur.as_slice()[..5], b"alix!");

        // Et dans l'autre sens : la seconde poignée écrit, la première voit.
        let sub2 = sub.open_named_zone("substrate.partagee").unwrap();
        drop(ecrivain);
        let mut second_ecrivain = sub2;
        second_ecrivain.as_slice_mut()[5..10].copy_from_slice(b"gus!!");
        let relu = sub.open_named_zone("substrate.partagee").unwrap();
        assert_eq!(&relu.as_slice()[..10], b"alix!gus!!");
    }

    /// La croissance en mémoire se comporte comme un remap : la zone grandit,
    /// le contenu déjà écrit est conservé, et les octets neufs sont à zéro.
    #[test]
    fn croissance_conserve_le_contenu_et_zero_remplit() {
        let sub = SubstrateFile::open_memory().unwrap();
        let mut zone = sub.open_named_zone("substrate.croissance").unwrap();
        zone.grow_to(8).unwrap();
        zone.as_slice_mut()[..3].copy_from_slice(b"abc");
        assert_eq!(zone.remap_count(), 1);

        zone.grow_to(64).unwrap();
        assert_eq!(zone.remap_count(), 2);
        assert_eq!(zone.len(), 64);
        assert_eq!(&zone.as_slice()[..3], b"abc");
        assert!(
            zone.as_slice()[3..].iter().all(|o| *o == 0),
            "les octets neufs doivent être à zéro"
        );
        assert!(zone.shrink_to(16).is_ok());
        assert_eq!(zone.len(), 16);
        assert_eq!(&zone.as_slice()[..3], b"abc");
    }

    /// L'en-tête meta doit faire l'aller-retour SANS alignement garanti (un
    /// `Vec<u8>` ne promet rien) : on écrit un en-tête non vierge et on le relit.
    #[test]
    fn en_tete_meta_aller_retour() {
        let mut sub = SubstrateFile::open_memory().unwrap();
        let vierge = sub.meta_header();
        assert!(
            vierge.is_valid_magic(),
            "la méta vierge doit porter le magic"
        );

        let mut modifie = vierge;
        let avant = modifie.last_checkpoint;
        modifie.last_checkpoint = avant.wrapping_add(42);
        sub.write_meta_header(&modifie).unwrap();

        let relu = sub.meta_header();
        assert_eq!(relu.last_checkpoint, avant.wrapping_add(42));
        assert_eq!(relu.format_version, vierge.format_version);
        // Fsync d'une zone en mémoire : rien à pousser, mais pas d'erreur.
        sub.fsync_meta().unwrap();
    }

    /// Les zones pré-créées par `create()` existent aussi en mémoire, vides —
    /// sans quoi chaque ouverture aurait à traiter le cas « absente ».
    #[test]
    fn zones_de_base_presentes_et_vides() {
        let sub = SubstrateFile::open_memory().unwrap();
        for nom in zone::ALL_MMAPPED {
            let z = sub.open_named_zone(nom).expect("zone pré-créée");
            if *nom == zone::NODES {
                continue; // seule zone non vide : elle porte la sentinelle v3
            }
            assert!(z.is_empty(), "{nom} doit être vide à la création");
        }
        // Sauf la zone nodes, qui porte sa sentinelle de stride.
        let n = sub.open_zone(Zone::Nodes).unwrap();
        assert!(!n.is_empty(), "la zone nodes porte la sentinelle v3");
    }

    /// Ouvrir en lecture seule une zone absente doit ÉCHOUER (NotFound), comme
    /// en mode fichier — la promesse « lecture seule » n'autorise pas à créer.
    #[test]
    fn lecture_seule_sur_zone_absente_echoue() {
        let sub = SubstrateFile::open_memory().unwrap();
        let err = sub.open_named_zone_read_only("substrate.jamais.creee");
        assert!(
            err.is_err(),
            "une zone absente ne doit pas être créée en RO"
        );
    }

    /// LE MODE MÉMOIRE NE SAIT PAS ENCORE CHECKPOINTER — ce test fige la raison.
    ///
    /// (2026-09-17, MESURÉ) `SubstrateStore::checkpoint()` rend `Err(NotFound)` :
    /// son `flush()` initial persiste dictionnaire, propriétés et arêtes externes
    /// dans un répertoire qui n'existe pas.
    ///
    /// Les SAUTER ne suffit pas — essai fait, puis REVERTÉ : le flush aboutit
    /// alors, ce qui laisse la surveillance du WAL déclencher un VRAI checkpoint,
    /// et la ré-ancrage des index qui le suit nomme ses fichiers temporaires avec
    /// `std::process::id()` (`props_index.rs:1009`, `props_eq_index.rs:668`,
    /// `props_ord_index.rs:406`) — qui PANIQUE sur wasm32, « no pids on this
    /// platform ». Mesuré : la suite wasm est passée de 22/22 à 1/22 lors de cet
    /// essai.
    ///
    /// Pour le rendre possible : (1) des `fusionner_index_*` sans `process::id`
    /// (un compteur atomique suffit), (2) PUIS sauter les trois persistances de
    /// `flush()` en mode mémoire.
    #[test]
    fn le_mode_memoire_ne_sait_pas_encore_checkpointer() {
        let store = crate::store::SubstrateStore::open_memory().expect("store en mémoire");
        assert!(
            store.checkpoint().is_err(),
            "si le checkpoint en mémoire se met à aboutir, c'est une BONNE nouvelle : \
             mettre à jour ce test ET la documentation, pas l'inverse"
        );
        assert!(
            !std::path::Path::new(MEMORY_DIR).exists(),
            "le mode mémoire ne doit créer AUCUN répertoire, même en échouant"
        );
    }

    /// Le store complet doit pouvoir naître en mémoire — c'est la vraie
    /// exigence : `Config::in_memory()` doit donner une base utilisable.
    #[test]
    fn store_complet_en_memoire() {
        use obrain_core::graph::GraphStore as _;

        let store = crate::store::SubstrateStore::open_memory().expect("store en mémoire");
        // Le store doit être LISIBLE sans système de fichiers, pas seulement
        // constructible : une base neuve n'a aucun nœud, et la lecture d'un
        // identifiant absent rend `None` au lieu d'échouer.
        assert!(store.get_node(obrain_common::types::NodeId(1)).is_none());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  ZONES PRIVÉES (ouverture en lecture seule) — rafraîchissement, croissance,
//  plafond. Chaque test exige que le FICHIER ne bouge pas d'un octet.
// ═══════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests_zone_privee {
    use super::*;

    fn fichier_de(dir: &tempfile::TempDir, octets: &[u8]) -> PathBuf {
        let p = dir.path().join("zone");
        std::fs::write(&p, octets).unwrap();
        p
    }

    /// Une zone privée qui grandit (rejeu d'un WAL au-delà du fichier) le fait EN
    /// MÉMOIRE : le fichier garde sa taille et son contenu, les octets d'origine
    /// sont conservés et le complément est zéro-rempli.
    #[test]
    fn grandir_une_zone_privee_ne_touche_pas_au_fichier() {
        let td = tempfile::tempdir().unwrap();
        let origine = vec![7u8; 8192];
        let chemin = fichier_de(&td, &origine);
        let mut zf =
            ZoneFile::open_privee(std::sync::Arc::new(ZonePrivee::ouvrir(&chemin).unwrap()));
        assert_eq!(zf.len(), 8192);

        zf.grow_to(20_000).unwrap();
        assert_eq!(zf.len(), 20_000);
        assert_eq!(
            &zf.as_slice()[..8192],
            &origine[..],
            "octets d'origine conservés"
        );
        assert!(
            zf.as_slice()[8192..].iter().all(|&b| b == 0),
            "complément zéro-rempli"
        );
        zf.as_slice_mut()[10_000] = 9;
        zf.ecrire_a(15_000, &[5, 5]).unwrap();
        zf.msync().unwrap();
        zf.fsync().unwrap();

        assert_eq!(
            std::fs::read(&chemin).unwrap(),
            origine,
            "le fichier a changé"
        );
    }

    /// Croissance dans la réserve : aucune recopie, la même projection sert.
    #[test]
    fn grandir_dans_la_reserve_garde_les_octets_ecrits() {
        let td = tempfile::tempdir().unwrap();
        let chemin = fichier_de(&td, &[1u8; 4096]);
        let mut zf =
            ZoneFile::open_privee(std::sync::Arc::new(ZonePrivee::ouvrir(&chemin).unwrap()));
        zf.grow_to(8192).unwrap();
        zf.as_slice_mut()[6000] = 42;
        zf.grow_to(12_288).unwrap();
        assert_eq!(
            zf.as_slice()[6000],
            42,
            "un octet écrit avant la 2ᵉ croissance a disparu"
        );
        assert_eq!(zf.len(), 12_288);
    }

    /// Une zone dont le fichier n'existe pas vit en mémoire : rien n'est créé.
    #[test]
    fn une_zone_sans_fichier_vit_en_memoire_et_ne_cree_rien() {
        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("absente");
        let mut zf =
            ZoneFile::open_privee(std::sync::Arc::new(ZonePrivee::ouvrir(&chemin).unwrap()));
        assert!(zf.is_empty());
        zf.grow_to(4096).unwrap();
        zf.as_slice_mut()[0] = 1;
        assert!(!chemin.exists(), "la lecture seule a créé le fichier");
    }

    /// `rafraichir` : un écrivain agrandit le fichier, le lecteur reprojette la
    /// nouvelle taille — en privé, sans écrire. Une zone sortie de son fichier
    /// (devenue mémoire) n'est plus reprojetée.
    #[test]
    fn rafraichir_reprojette_un_fichier_agrandi_sans_l_ecrire() {
        let td = tempfile::tempdir().unwrap();
        let chemin = fichier_de(&td, &[3u8; 4096]);
        let mut zf =
            ZoneFile::open_privee(std::sync::Arc::new(ZonePrivee::ouvrir(&chemin).unwrap()));
        assert!(!zf.rafraichir().unwrap(), "rien n'a grandi");

        // Un écrivain agrandit le fichier.
        let mut agrandi = vec![3u8; 4096];
        agrandi.extend(vec![8u8; 4096]);
        std::fs::write(&chemin, &agrandi).unwrap();
        assert!(
            zf.rafraichir().unwrap(),
            "la croissance du fichier n'est pas vue"
        );
        assert_eq!(zf.len(), 8192);
        assert_eq!(&zf.as_slice()[4096..], &agrandi[4096..]);
        assert_eq!(
            std::fs::read(&chemin).unwrap(),
            agrandi,
            "le lecteur a écrit le fichier"
        );

        // Zone sortie de son fichier : plus de reprojection.
        zf.grow_to(20_000).unwrap();
        std::fs::write(&chemin, vec![0u8; 100_000]).unwrap();
        assert!(!zf.rafraichir().unwrap());
        assert_eq!(zf.len(), 20_000);
    }

    /// Plafond : une zone adossée à un fichier plus gros que le plafond ne se
    /// recopie pas en mémoire pour grandir — refus NOMMÉ, qui dit quoi lancer.
    #[test]
    fn au_dela_du_plafond_la_croissance_est_refusee_par_une_erreur_nommee() {
        let td = tempfile::tempdir().unwrap();
        let chemin = fichier_de(&td, &[1u8; 16_384]);
        let zp = ZonePrivee::ouvrir(&chemin).unwrap();
        // SAFETY: test mono-fil, aucune vue vivante.
        let r = unsafe { zp.grandir_avec_plafond(32_768, 8192) };
        match r {
            Err(SubstrateError::LectureSeuleRefusee { raison }) => {
                assert!(
                    raison.contains("consolider"),
                    "le refus doit dire quoi faire : {raison}"
                );
            }
            autre => panic!("refus nommé attendu, obtenu {autre:?}"),
        }
        assert_eq!(zp.len(), 16_384, "la zone a changé malgré le refus");
        // Sous le plafond, la même croissance passe.
        // SAFETY: idem.
        unsafe { zp.grandir_avec_plafond(32_768, 1 << 20) }.unwrap();
        assert_eq!(zp.len(), 32_768);
        assert_eq!(std::fs::read(&chemin).unwrap(), vec![1u8; 16_384]);
    }

    /// `shrink_to` est refusé nommément : rétrécir, c'est `set_len`.
    #[test]
    fn retrecir_une_zone_privee_est_refuse_par_lecture_seule() {
        let td = tempfile::tempdir().unwrap();
        let chemin = fichier_de(&td, &[1u8; 8192]);
        let mut zf =
            ZoneFile::open_privee(std::sync::Arc::new(ZonePrivee::ouvrir(&chemin).unwrap()));
        assert!(matches!(
            zf.shrink_to(4096),
            Err(SubstrateError::LectureSeule { .. })
        ));
        assert_eq!(std::fs::read(&chemin).unwrap().len(), 8192);
    }
}
