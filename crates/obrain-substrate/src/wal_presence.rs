//! ── PREUVE DE VIE D'UN ÉCRIVAIN DU WAL PARTAGÉ (V1-T2B) ─────────────────────
//!
//! ## Le défaut qu'il ferme
//!
//! Une réservation de la chaîne d'append (voir `wal_registre`) dont l'écrivain
//! n'écrit jamais bloque ses successeurs. Ceux-ci la déclaraient ABANDONNÉE après
//! un DÉLAI ([`crate::wal_io::DELAI_TETE_BLOQUEE`], 25 ms), puis ré-ancraient la
//! chaîne. Un délai ne distingue pas un écrivain MORT d'un écrivain VIVANT mais
//! préempté plus longtemps que lui : le successeur ré-ancrait sous un écrivain qui
//! allait écrire, et sa frame partait avec un LSN inférieur à sa position
//! (mesuré : 10 échecs sur 40 sous charge, `t2b`).
//!
//! ## Ce que ce module répond, et pourquoi c'est une preuve
//!
//! La question n'est plus « ça fait combien de temps ? » mais « ce processus
//! existe-t-il encore ? ». Trois réponses fiables, sans durée :
//!
//! * `kill(pid, 0)` renvoie `ESRCH` : le processus n'existe plus (SIGKILL, sortie).
//! * un processus MORT NON RÉCOLTÉ (zombie) est compté mort : son état système
//!   l'affirme, et `kill` réussirait encore sur lui.
//! * l'HEURE DE DÉMARRAGE du processus, comparée à celle que l'écrivain a écrite
//!   dans le registre à sa prise d'emplacement. C'est ce qui gère la
//!   RÉUTILISATION DE PID : après la mort de l'écrivain, son pid peut être donné à
//!   un processus étranger, pour lequel `kill(pid, 0)` réussit. Le démarrage, lui,
//!   ne se répète pas : un démarrage différent prouve que ce n'est plus l'écrivain.
//!   Le pid seul ne suffirait donc pas (choix justifié : un battement de cœur aurait
//!   exigé une horloge et une écriture périodique, et un écrivain préempté plus
//!   longtemps que le battement serait redevenu « mort »).
//!
//! Un processus VIVANT mais préempté répond « vivant » à toute heure : on ATTEND
//! sa publication, on ne ré-ancre pas. Un processus MORT est reconnu au premier
//! contrôle après le délai : la récupération reste bornée, comme avant.
//!
//! ## Où la lecture du démarrage est indisponible
//!
//! Sous Linux (`/proc/<pid>/stat`) et macOS (`proc_pidinfo`), elle l'est. Ailleurs
//! (autres Unix), seul `kill` fait foi : le démarrage est inconnu, et un pid vivant
//! est tenu pour vivant. Hors Unix, aucune preuve n'est possible : le comportement
//! d'avant est repris (un écrivain sans preuve est tenu pour mort après le délai).

#![allow(unsafe_code)]

/// Le verdict sur un écrivain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EtatProcessus {
    /// Le processus existe et n'est pas mort : on attend sa publication.
    Vivant,
    /// Le processus n'existe plus, est un zombie, ou son pid a été réattribué à un
    /// processus démarré APRÈS l'écrivain : on peut récupérer sa plage.
    Mort,
}

/// Identité de l'écrivain courant : `(pid, heure de démarrage)`. L'heure vaut `0`
/// quand elle ne peut pas être lue ; une heure nulle ne sert alors à rien, et la
/// comparaison est sautée (le pid seul décide).
pub(crate) fn identite_du_processus_courant() -> (u64, u64) {
    let pid = u64::from(crate::wal_io::pid_portable());
    (pid, demarrage_connu(pid))
}

/// L'état d'un écrivain identifié par `(pid, demarrage)`. `demarrage == 0` signifie
/// « inconnu » (voir [`identite_du_processus_courant`]).
pub(crate) fn etat_processus(pid: u64, demarrage: u64) -> EtatProcessus {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return EtatProcessus::Mort;
        };
        // pid 0 et les négatifs désignent des GROUPES de processus : `kill` les
        // adresserait, et leur existence ne prouve rien sur l'écrivain.
        if pid <= 0 {
            return EtatProcessus::Mort;
        }
        // SAFETY: `kill(pid, 0)` ne livre aucun signal : il teste seulement
        // l'existence du processus et la permission de le signaler.
        let existe = unsafe { libc::kill(pid, 0) } == 0;
        if !existe && errno() == Some(libc::ESRCH) {
            return EtatProcessus::Mort;
        }
        match lire_demarrage(pid) {
            Lecture::Absent => EtatProcessus::Mort,
            Lecture::Connu(h) if demarrage != 0 && h != demarrage => EtatProcessus::Mort,
            Lecture::Connu(_) | Lecture::Inconnu => EtatProcessus::Vivant,
        }
    }
    #[cfg(not(unix))]
    {
        // Pas de preuve disponible : comportement d'avant (délai, puis récupération).
        let _ = (pid, demarrage);
        EtatProcessus::Mort
    }
}

/// Ce que le système dit du démarrage d'un processus.
#[cfg(unix)]
enum Lecture {
    /// Le processus existe, et son démarrage est lu.
    Connu(u64),
    /// Le processus n'existe plus, ou c'est un zombie.
    Absent,
    /// Lecture impossible (permission, plateforme sans source) : on ne conclut pas.
    Inconnu,
}

#[cfg(unix)]
fn errno() -> Option<i32> {
    std::io::Error::last_os_error().raw_os_error()
}

#[cfg(unix)]
fn demarrage_connu(pid: u64) -> u64 {
    match libc::pid_t::try_from(pid).map(lire_demarrage) {
        Ok(Lecture::Connu(h)) => h,
        _ => 0,
    }
}

#[cfg(not(unix))]
fn demarrage_connu(_pid: u64) -> u64 {
    0
}

/// Linux : `/proc/<pid>/stat`. Le démarrage est le champ 22 (en ticks depuis le
/// boot, stable pour toute la vie du processus) ; l'état est le champ 3.
#[cfg(target_os = "linux")]
fn lire_demarrage(pid: libc::pid_t) -> Lecture {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(texte) => {
            // Le nom du programme (champ 2) est entre parenthèses et peut en
            // contenir : on coupe au DERNIER `)`.
            let Some(apres) = texte.rfind(')').map(|i| &texte[i + 1..]) else {
                return Lecture::Inconnu;
            };
            // `champs[0]` est le champ 3 (état) : le champ 22 est donc `champs[19]`.
            let champs: Vec<&str> = apres.split_whitespace().collect();
            if matches!(champs.first(), Some(&"Z" | &"X")) {
                return Lecture::Absent;
            }
            champs
                .get(19)
                .and_then(|t| t.parse::<u64>().ok())
                .map_or(Lecture::Inconnu, Lecture::Connu)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lecture::Absent,
        Err(_) => Lecture::Inconnu,
    }
}

/// macOS : `proc_pidinfo(PROC_PIDTBSDINFO)` donne le démarrage (secondes et
/// microsecondes) et l'état (`SZOMB` pour un zombie).
#[cfg(target_os = "macos")]
fn lire_demarrage(pid: libc::pid_t) -> Lecture {
    // SAFETY: `proc_bsdinfo` est une structure C sans invariant : la valeur nulle
    // est valide, et l'appel ne l'écrit que sur `taille` octets.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let taille = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: le tampon fait exactement `taille` octets et vit pendant l'appel.
    let lus = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            taille,
        )
    };
    if lus != taille {
        return if errno() == Some(libc::ESRCH) {
            Lecture::Absent
        } else {
            Lecture::Inconnu
        };
    }
    if info.pbi_status == libc::SZOMB {
        return Lecture::Absent;
    }
    Lecture::Connu(
        info.pbi_start_tvsec
            .wrapping_mul(1_000_000)
            .wrapping_add(info.pbi_start_tvusec),
    )
}

/// Autres Unix : pas de source de démarrage portable ici. Seul `kill` fait foi.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn lire_demarrage(_pid: libc::pid_t) -> Lecture {
    Lecture::Inconnu
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn le_processus_courant_est_vivant_avec_sa_propre_identite() {
        let (pid, demarrage) = identite_du_processus_courant();
        assert_eq!(pid, u64::from(std::process::id()));
        assert_eq!(etat_processus(pid, demarrage), EtatProcessus::Vivant);
    }

    #[test]
    fn un_pid_reattribue_avec_un_autre_demarrage_est_mort() {
        let (pid, demarrage) = identite_du_processus_courant();
        if demarrage != 0 {
            // Même pid, démarrage différent : c'est un AUTRE processus, et l'écrivain
            // d'origine n'existe plus.
            assert_eq!(
                etat_processus(pid, demarrage.wrapping_add(1)),
                EtatProcessus::Mort
            );
        }
    }

    #[test]
    fn un_processus_termine_et_recolte_est_mort() {
        let mut enfant = Command::new("true").spawn().expect("lancer `true`");
        let pid = u64::from(enfant.id());
        enfant.wait().expect("récolter l'enfant");
        assert_eq!(etat_processus(pid, 0), EtatProcessus::Mort);
    }

    #[test]
    fn un_zombie_non_recolte_est_mort() {
        let enfant = Command::new("true").spawn().expect("lancer `true`");
        let pid = u64::from(enfant.id());
        // L'enfant se termine sans être récolté : il reste un zombie, que `kill`
        // croit encore vivant. La preuve doit le compter mort.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(etat_processus(pid, 0), EtatProcessus::Mort);
        drop(enfant);
    }
}
