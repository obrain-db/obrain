//! ── REGISTRE D'APPEND MMAPÉ PARTAGÉ — `substrate.wal.reg` ───────────────────
//!
//! ## Le défaut mesuré qu'il ferme
//!
//! En coordination partagée (`CoordinationAppend::OAppend`), chaque append payait
//! **trois appels système** : un `statx` AVANT le `write` (pour connaître la
//! position : la garde relit la taille RÉELLE, `wal_io::WriterInner::taille_reelle`)
//! et un `statx` APRÈS (pour vérifier qu'aucun pair ne s'est glissé dans la
//! fenêtre). Mesuré le 2026-10-06 : `statx(2)` = **802 ns** (contre `fstat` = 571),
//! OAppend = **2 735 ns**/record contre **942 ns** pour le mono-écrivain prouvé —
//! soit Δ = 2 × 802 ns, exactement les deux `statx`. `VerrouExact` en payait cinq
//! (les deux mêmes plus deux `flock` par record + un aller-retour).
//!
//! ## Le choix : un registre partagé, et une RÉSERVATION qui remplace la mesure
//!
//! Un `AtomicU64` dans une page `MAP_SHARED` est lisible et modifiable par tous
//! les processus du store sans aucun appel système (~20 ns non-contendu, 40 ns au
//! p50 CONTENDU, mesurés). Il porte donc ce que le `statx` allait chercher :
//! **où écrire**. La position cesse d'être une MESURE (un `statx` par record) pour
//! devenir une RÉSERVATION (un `compare_exchange` par record).
//!
//! Les QUATRE mots, et ce que chacun garantit :
//!
//! | mot              | qui l'écrit                                   | ce qu'il dit |
//! |------------------|-----------------------------------------------|--------------|
//! | `tete_reservee`  | tout append (un CAS)                           | fin de la chaîne des RÉSERVATIONS (qui a le droit d'écrire où) |
//! | `fin_publiee`    | tout append, APRÈS un `write` complet          | fin de ce qui EXISTE dans le fichier |
//! | `generation`     | le checkpoint ÉLU (une fois par troncature)    | « le fichier a été tronqué, l'ancre a changé » |
//! | `ancre_partagee` | le checkpoint ÉLU, et tout pair qui en sait plus | base des LSN (`lsn = ancre + position`) |
//!
//! ── POURQUOI DEUX MOTS DE CHAÎNE, ET PAS UN SEUL (défaut MESURÉ) ────────────
//! Un seul mot ne peut pas dire à la fois « cette plage est à moi » et « cette
//! plage est écrite ». Conséquence, mesurée sur `mw_interprocess_append::t2`
//! (6 processus × 1 500 records) avec un mot unique : **24,55 % des frames
//! portaient un LSN différent de leur position**, et 595 LSN étaient réémis. La
//! raison est structurelle : `O_APPEND` place chaque `write` à la fin RÉELLE du
//! fichier, donc pour que `lsn = ancre + position` soit VRAI il faut écrire quand
//! la fin réelle vaut DÉJÀ sa position de départ — c'est-à-dire attendre que le
//! prédécesseur ait PUBLIÉ. Avec un mot unique, cette attente ne peut se faire
//! qu'en lisant la taille du fichier (`statx`, un appel système par record) :
//! exactement le coût que la pièce 7 supprime.
//!
//! Avec `tete_reservee` ET `fin_publiee`, l'attente est un SPIN SUR UN ATOMIQUE
//! partagé — **zéro appel système** — et l'ordre physique devient égal à l'ordre
//! des LSN **par construction** : on n'écrit que lorsque tout ce qui est
//! au-dessous a été publié. Le `statx` ne reste que sur les chemins d'ANOMALIE
//! (troncature annoncée par la génération, prédécesseur mort), soit une fois par
//! checkpoint ou par crash — jamais une fois par record.
//!
//! ## La CHAÎNE, et pourquoi ce n'est pas un compteur nu
//!
//! Une réservation est un `compare_exchange` sur `tete_reservee`, de la valeur
//! OBSERVÉE vers `observée + len` : deux écrivains ne peuvent donc pas obtenir la
//! même plage, et l'offset rendu est EXACT sans qu'aucun appel système n'ait été
//! fait. Deux propriétés que le compteur nu n'a pas :
//!
//! * **une réservation ne saute jamais un trou** : la valeur attendue du CAS est la
//!   valeur LUE, donc si un pair a ré-ancré la tête entre notre lecture et notre
//!   CAS, celui-ci ÉCHOUE au lieu d'écraser sa décision. C'est le CAS qui rend la
//!   récupération d'un pair DÉTECTABLE au lieu d'être silencieuse.
//! * **un écrivain tué entre son CAS et son `write` laisse sa plage réservée dans
//!   la tête** : les suivants la voient, l'attendent (délai de récupération borné),
//!   puis la récupèrent par le CHEMIN FROID (voir plus bas) — jamais en l'ignorant.
//!
//! ## Ce que la chaîne ne garantit pas TOUTE SEULE, et le mot qui le ferme
//!
//! `O_APPEND` place chaque `write` à la fin RÉELLE du fichier, pas à la position
//! réservée : la position physique n'est celle de la réservation que si tous les
//! écrivains écrivent DANS L'ORDRE des réservations. C'est exactement ce que
//! `fin_publiee` permet d'obtenir sans aucun appel système : l'écrivain SPINNE sur
//! ce mot jusqu'à ce qu'il vaille son début de plage — c'est-à-dire jusqu'à ce que
//! tout ce qui est au-dessous ait été ÉCRIT — et n'écrit qu'alors. L'ordre physique
//! est donc égal à l'ordre des LSN **par construction**, et non par espérance.
//!
//! ## Ce que ce fichier NE contient pas
//!
//! Aucun verrou chaud : le seul `flock` du module est pris UNE fois, à la CRÉATION
//! du fichier (chemin froid, une fois par vie de base), pour que deux processus qui
//! créent le registre en même temps ne s'écrasent pas l'un l'autre. Le chemin chaud
//! (écrivain seul) est exactement : deux lectures d'atomes (la chaîne est-elle
//! quiescente et connue ?), un CAS, un `write`, une publication. Le chemin
//! concurrent ajoute un spin sur `fin_publiee` — de la mémoire partagée, toujours
//! zéro appel système.

#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use memmap2::{MmapMut, MmapOptions};

use crate::error::SubstrateResult;
use crate::wal_presence::{EtatProcessus, etat_processus};

/// Suffixe du registre, ajouté au chemin du WAL : `substrate.wal` →
/// `substrate.wal.reg`.
///
/// ── POURQUOI CE N'EST PAS LE MÊME FICHIER QUE LE SIDE-CAR D'ANCRE ───────────
/// `<wal>.lsn` est publié par `rename` ATOMIQUE : son inode change à chaque
/// publication. Un mapping `MAP_SHARED` sur un fichier renommé pointerait sur
/// l'ANCIEN inode — c'est-à-dire sur un fichier que plus personne ne met à jour.
/// Le registre doit donc être un fichier à inode STABLE, jamais renommé, écrit en
/// place : c'est la même raison qui fait que le verrou d'append se prenait sur le
/// WAL et non sur le side-car.
pub const SUFFIXE_REGISTRE_APPEND: &str = ".reg";

/// Magie du registre — « OBRA WAL REGistre ». Un fichier dont la magie diffère
/// n'est pas un registre d'append : on le réinitialise (il n'est pas la source de
/// vérité, le WAL et son ancre le sont).
const MAGIE: u64 = u64::from_le_bytes(*b"OBRAWREG");

/// Version du LAYOUT. Un incrément = les décalages changent : on réinitialise
/// plutôt que de lire des octets qui ne veulent plus rien dire.
const VERSION_LAYOUT: u64 = 1;

/// Taille du mapping : UNE page. La page entière est mappée (l'unité de `mmap`),
/// et les QUATRE mots sont placés sur des lignes de cache DIFFÉRENTES : un append
/// ne charge que les lignes de `tete_reservee` et de `fin_publiee` (deux lectures
/// par record), il ne dispute donc pas la ligne de `generation` (changée une fois
/// par checkpoint) ni celle de `ancre_partagee` (changée encore plus rarement).
const TAILLE: usize = 4096;

const OFF_MAGIE: usize = 0;
const OFF_VERSION: usize = 8;
/// Fin de la chaîne des RÉSERVATIONS (le CAS des appends). Ligne de cache 1.
const OFF_TETE_RESERVEE: usize = 64;
/// Fin de ce qui EXISTE dans le fichier (publié après un `write` complet).
/// Ligne de cache 2.
const OFF_FIN_PUBLIEE: usize = 128;
/// Génération : incrémentée par le checkpoint ÉLU quand il publie l'ancre et
/// tronque. Ligne de cache 3.
const OFF_GENERATION: usize = 192;
/// Base des LSN du segment courant. Ligne de cache 4.
const OFF_ANCRE: usize = 256;

// ── LES EMPLACEMENTS DE PRÉSENCE (preuve de vie, V1-T2B) ────────────────────
//
// Zone AJOUTÉE dans les octets inutilisés de la même page, après `OFF_ANCRE`
// (qui se termine à 264). Aucun décalage existant ne bouge, et `VERSION_LAYOUT`
// reste 1 : un registre écrit avant cette zone a des zéros, lus comme « aucun
// emplacement » (aucune preuve, donc le comportement d'avant). Un registre
// écrit par un binaire antérieur est lu sans erreur ; un binaire antérieur ignore
// la zone. C'est une extension ADDITIVE, pas un changement de layout.
//
// Chaque emplacement (32 octets, 64 emplacements) porte :
//   · pid du détenteur (0 = libre) ;
//   · heure de démarrage du détenteur (voir `wal_presence`) ;
//   · début de la plage réservée EN COURS, plus un (0 = aucune) : la plage
//     annoncée AVANT le CAS, effacée à la publication ou à toute sortie.
const OFF_EMPLACEMENTS: usize = 512;
/// Nombre d'écrivains (`WalWriter`) identifiables à la fois dans un même registre.
/// Un écrivain sans emplacement reste correct : il n'est simplement pas une preuve.
pub const NB_EMPLACEMENTS: usize = 64;
const TAILLE_EMPLACEMENT: usize = 32;
const EMP_PID: usize = 0;
const EMP_DEMARRAGE: usize = 8;
const EMP_DEBUT_PLUS_UN: usize = 16;

/// Chemin du registre d'append d'un WAL.
#[must_use]
pub fn chemin_registre(wal: &Path) -> PathBuf {
    let mut s = wal.as_os_str().to_os_string();
    s.push(SUFFIXE_REGISTRE_APPEND);
    PathBuf::from(s)
}

/// ── UNE VUE ATOMIQUE SUR UN MOT DU MAPPING ──────────────────────────────────
///
/// SAFETY (le seul `unsafe` du module, même contrat que `registres_partages.rs`
/// et `borne_allocation.rs`) : les trois décalages sont des multiples de 8 et la
/// base d'un mapping `mmap` est alignée sur la page — l'adresse est donc alignée
/// pour un `AtomicU64`. Le mapping est `MAP_SHARED` et vit aussi longtemps que
/// `map`, lui-même emprunté par `self` : la référence rendue ne peut pas survivre
/// aux octets qu'elle désigne.
fn mot(map: &MmapMut, offset: usize) -> &AtomicU64 {
    debug_assert_eq!(offset % 8, 0, "un AtomicU64 exige un offset aligné");
    debug_assert!(offset + 8 <= map.len(), "offset hors du mapping");
    // SAFETY: voir la doc ci-dessus — alignement et durée de vie garantis par la
    // construction de `RegistreAppend`.
    unsafe { &*(map.as_ptr().add(offset) as *const AtomicU64) }
}

/// ── LE REGISTRE D'APPEND D'UN WAL ──────────────────────────────────────────
#[derive(Debug)]
pub struct RegistreAppend {
    chemin: PathBuf,
    map: MmapMut,
    /// Gardé pour la durée de vie du mapping (fermer le descripteur ne détruit
    /// pas un mapping sur Linux, mais le garder rend la structure lisible — et
    /// c'est ce descripteur qui porte le `flock` de création).
    _fichier: File,
}

impl RegistreAppend {
    /// Ouvre (ou crée) le registre d'append du WAL `wal`.
    ///
    /// `fin_initiale` est la taille RÉELLE du WAL au moment de l'ouverture : elle
    /// est publiée en `fetch_max` (jamais en arrière), pour qu'un registre neuf
    /// posé à côté d'un WAL déjà écrit ne fasse pas repartir la chaîne de zéro —
    /// ce qui ferait écrire la prochaine frame à un offset déjà occupé.
    ///
    /// ── POURQUOI `fetch_max` ET PAS `store` ─────────────────────────────────
    /// L'ouverture d'un processus n'est PAS un point de synchronisation : un pair
    /// peut être en train d'appender. Un `store` de notre taille (mesurée il y a
    /// quelques microsecondes) reculerait la tête SOUS sa réservation, et le pair
    /// suivant réserverait la même plage. `fetch_max` ne recule jamais : au pire
    /// la tête est en avance sur le fichier, et c'est le chemin froid du premier
    /// append qui la ré-ancrera (avec la mesure, et avec l'attente bornée qui
    /// distingue un pair lent d'un pair mort).
    pub fn ouvrir(wal: &Path, fin_initiale: u64, ancre: u64) -> SubstrateResult<Self> {
        let chemin = chemin_registre(wal);
        let fichier = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&chemin)?;
        if fichier.metadata()?.len() != TAILLE as u64 {
            // Taille inattendue (fichier neuf = 0, ou tronqué par un pair) : on la
            // remet à TAILLE. Aucun `set_len` plus PETIT n'arrive ici (TAILLE est
            // une constante), donc on n'ampute jamais un registre existant.
            fichier.set_len(TAILLE as u64)?;
        }
        // SAFETY: le fichier vient d'être dimensionné à TAILLE octets, et cette
        // taille ne bouge plus pour toute la vie de l'objet (aucun `set_len`
        // postérieur) : l'adresse des trois atomes est donc stable, ce qui est la
        // condition du partage entre processus (un remap la changerait sous les
        // pieds d'un pair).
        let map = unsafe { MmapOptions::new().len(TAILLE).map_mut(&fichier)? };
        let reg = Self {
            chemin,
            map,
            _fichier: fichier,
        };
        if reg.mot(OFF_MAGIE).load(Ordering::Acquire) != MAGIE
            || reg.mot(OFF_VERSION).load(Ordering::Acquire) != VERSION_LAYOUT
        {
            reg.creer(fin_initiale, ancre);
        }
        // Les deux têtes ne reculent jamais à l'ouverture (voir la doc de la
        // fonction). Si un pair est en vol, sa réservation est donc PRÉSERVÉE :
        // `tete_reservee > fin_publiee` est exactement ce qui dit « la chaîne
        // n'est pas quiescente », et le premier append attendra sa publication.
        reg.mot(OFF_TETE_RESERVEE)
            .fetch_max(fin_initiale, Ordering::AcqRel);
        reg.mot(OFF_FIN_PUBLIEE)
            .fetch_max(fin_initiale, Ordering::AcqRel);
        Ok(reg)
    }

    fn mot(&self, offset: usize) -> &AtomicU64 {
        mot(&self.map, offset)
    }

    /// Chemin du fichier.
    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    /// Fin de la chaîne des RÉSERVATIONS : c'est le mot du `compare_exchange`
    /// qui attribue une plage à un écrivain (offset du prochain octet réservable).
    #[must_use]
    pub fn tete_reservee(&self) -> &AtomicU64 {
        self.mot(OFF_TETE_RESERVEE)
    }

    /// Fin de ce qui EXISTE dans le fichier : publiée par chaque écrivain APRÈS
    /// un `write` complet, et jamais en arrière (`fetch_max`). C'est ce mot qui
    /// permet à un écrivain d'attendre son prédécesseur **sans aucun appel
    /// système** — et donc à `lsn = ancre + position` d'être vrai par
    /// construction.
    #[must_use]
    pub fn fin_publiee(&self) -> &AtomicU64 {
        self.mot(OFF_FIN_PUBLIEE)
    }

    /// Publie une fin ÉCRITE. `fetch_max` et non `store` : l'ordre des
    /// publications suit l'ordre des réservations (chacun n'écrit qu'après son
    /// prédécesseur), donc la valeur ne recule jamais — et si une anomalie
    /// tentait de la faire reculer, `fetch_max` la refuse au lieu de rendre une
    /// plage déjà écrite « à écrire » à quelqu'un d'autre.
    pub fn publier_la_fin(&self, fin: u64) {
        self.fin_publiee().fetch_max(fin, Ordering::Release);
    }

    /// Génération du registre : incrémentée par le checkpoint ÉLU au moment où il
    /// publie l'ancre et tronque. Un pair qui la voit changer SAIT que le fichier
    /// a été tronqué, et n'a donc pas à attendre pour se ré-ancrer.
    #[must_use]
    pub fn generation(&self) -> &AtomicU64 {
        self.mot(OFF_GENERATION)
    }

    /// Base des LSN du segment courant (`lsn = ancre_partagee + position`).
    #[must_use]
    pub fn ancre_partagee(&self) -> &AtomicU64 {
        self.mot(OFF_ANCRE)
    }

    fn mot_emplacement(&self, i: usize, champ: usize) -> &AtomicU64 {
        self.mot(OFF_EMPLACEMENTS + i * TAILLE_EMPLACEMENT + champ)
    }

    /// Prend un emplacement pour cet écrivain et y écrit son identité. Rend son
    /// indice, ou `None` si la table est pleine de détenteurs VIVANTS.
    ///
    /// Un emplacement dont le détenteur est MORT est repris : son éventuelle plage
    /// annoncée ne peut plus être publiée par personne, elle est donc effacée.
    /// L'ordre compte : la prise (CAS) précède l'effacement, car un autre écrivain
    /// peut avoir pris l'emplacement entre-temps, et son annonce ne doit pas être
    /// effacée par nous.
    pub(crate) fn reclamer_emplacement(&self, pid: u64, demarrage: u64) -> Option<usize> {
        for i in 0..NB_EMPLACEMENTS {
            let proprietaire = self.mot_emplacement(i, EMP_PID);
            let actuel = proprietaire.load(Ordering::Acquire);
            if actuel != 0 {
                let demarrage_actuel = self
                    .mot_emplacement(i, EMP_DEMARRAGE)
                    .load(Ordering::Acquire);
                if etat_processus(actuel, demarrage_actuel) == EtatProcessus::Vivant {
                    continue;
                }
            }
            if proprietaire
                .compare_exchange(actuel, pid, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.mot_emplacement(i, EMP_DEBUT_PLUS_UN)
                    .store(0, Ordering::Release);
                self.mot_emplacement(i, EMP_DEMARRAGE)
                    .store(demarrage, Ordering::Release);
                return Some(i);
            }
        }
        None
    }

    /// Libère l'emplacement `i` : l'annonce d'abord, le pid ensuite, pour qu'un
    /// lecteur ne voie jamais un détenteur sans plage (et inversement).
    pub(crate) fn liberer_emplacement(&self, i: usize) {
        self.mot_emplacement(i, EMP_DEBUT_PLUS_UN)
            .store(0, Ordering::Release);
        self.mot_emplacement(i, EMP_PID).store(0, Ordering::Release);
    }

    /// Annonce, avant le CAS, la plage que l'écrivain `i` s'apprête à réserver à
    /// `debut`. Un CAS perdu laisse une annonce périmée : elle est réécrite à la
    /// tentative suivante, et elle ne désigne jamais qu'un écrivain VIVANT (son
    /// propre pid), jamais un pair mort.
    pub(crate) fn annoncer_reservation(&self, i: usize, debut: u64) {
        self.mot_emplacement(i, EMP_DEBUT_PLUS_UN)
            .store(debut + 1, Ordering::Release);
    }

    /// Efface l'annonce de l'écrivain `i` : sa plage est publiée, ou abandonnée.
    pub(crate) fn effacer_reservation(&self, i: usize) {
        self.mot_emplacement(i, EMP_DEBUT_PLUS_UN)
            .store(0, Ordering::Release);
    }

    /// ── LA QUESTION QUE LE DÉLAI NE SAVAIT PAS POSER ────────────────────────
    ///
    /// Un écrivain a-t-il réservé la plage qui commence à `fin_publiee` (la plage
    /// que tout le monde attend), et est-il ENCORE VIVANT ? Si oui, il finira de
    /// publier : on doit l'attendre, et ne pas le déclarer mort.
    ///
    /// Rend `false` s'il n'existe AUCUNE annonce pour cette plage (écrivain sans
    /// emplacement, binaire antérieur, ou plateforme sans preuve) : c'est alors
    /// le comportement d'avant, le délai, qui tranche.
    pub(crate) fn predecesseur_vivant(&self, fin_publiee: u64) -> bool {
        for i in 0..NB_EMPLACEMENTS {
            let annonce = self
                .mot_emplacement(i, EMP_DEBUT_PLUS_UN)
                .load(Ordering::Acquire);
            if annonce != fin_publiee + 1 {
                continue;
            }
            let pid = self.mot_emplacement(i, EMP_PID).load(Ordering::Acquire);
            if pid == 0 {
                continue;
            }
            let demarrage = self
                .mot_emplacement(i, EMP_DEMARRAGE)
                .load(Ordering::Acquire);
            if etat_processus(pid, demarrage) == EtatProcessus::Vivant {
                return true;
            }
        }
        false
    }

    /// Première création (fichier neuf, d'une autre magie, ou d'un autre layout).
    ///
    /// ── LE `flock` ICI N'EST PAS UN VERROU CHAUD ────────────────────────────
    /// Il n'est pris que si la magie/layout ne sont PAS les nôtres, c'est-à-dire à
    /// la CRÉATION du registre : une fois par vie de base, pas par append. Sans
    /// lui, deux processus qui ouvrent un WAL neuf en même temps pourraient
    /// initialiser ensemble et l'un des deux reculerait la tête de l'autre (une
    /// fenêtre de quelques nanosecondes, mais un offset réservé deux fois). Le
    /// noyau arbitre, et rend le verrou à la mort du processus (même `kill -9`) :
    /// aucun état résiduel à nettoyer.
    fn creer(&self, fin_initiale: u64, ancre: u64) {
        // (le garde vit jusqu'à la fin de la fonction : le verrou est rendu par
        // son Drop, après la publication de la magie)
        #[cfg(not(target_arch = "wasm32"))]
        let _garde = fs2::FileExt::lock_exclusive(&self._fichier);
        // Relecture SOUS le verrou : si un pair a initialisé entre notre test et
        // notre prise de verrou, il n'y a plus rien à faire — et c'est le cas
        // NOMINAL quand six processus ouvrent un WAL vide en même temps.
        if self.mot(OFF_MAGIE).load(Ordering::Acquire) == MAGIE
            && self.mot(OFF_VERSION).load(Ordering::Acquire) == VERSION_LAYOUT
        {
            return;
        }
        // `fetch_max` : un pair peut avoir déjà publié (il a le même fichier
        // mappé). La GÉNÉRATION, elle, n'est JAMAIS remise à zéro : un pair a pu
        // l'incrémenter (troncature), et l'écraser ferait rater l'annonce à un
        // troisième processus — qui conclurait que rien n'a bougé.
        self.mot(OFF_TETE_RESERVEE)
            .fetch_max(fin_initiale, Ordering::AcqRel);
        self.mot(OFF_FIN_PUBLIEE)
            .fetch_max(fin_initiale, Ordering::AcqRel);
        self.mot(OFF_ANCRE).fetch_max(ancre, Ordering::AcqRel);
        // La MAGIE en DERNIER (`Release`) : aucun pair ne peut voir un registre
        // à demi initialisé comme valide.
        self.mot(OFF_VERSION)
            .store(VERSION_LAYOUT, Ordering::Release);
        self.mot(OFF_MAGIE).store(MAGIE, Ordering::Release);
    }
}

#[cfg(all(test, unix))]
mod tests_presence {
    use super::*;
    use crate::wal_presence::identite_du_processus_courant;

    fn registre_neuf() -> (tempfile::TempDir, RegistreAppend) {
        let td = tempfile::tempdir().unwrap();
        let wal = td.path().join("substrate.wal");
        std::fs::write(&wal, b"").unwrap();
        let reg = RegistreAppend::ouvrir(&wal, 0, 0).unwrap();
        (td, reg)
    }

    #[test]
    fn sans_annonce_aucun_pair_vivant() {
        let (_td, reg) = registre_neuf();
        assert!(!reg.predecesseur_vivant(0));
    }

    #[test]
    fn un_pair_vivant_qui_a_annonce_sa_plage_bloque_la_recuperation() {
        let (_td, reg) = registre_neuf();
        let (pid, demarrage) = identite_du_processus_courant();
        let i = reg
            .reclamer_emplacement(pid, demarrage)
            .expect("emplacement libre");
        reg.annoncer_reservation(i, 100);
        assert!(reg.predecesseur_vivant(100), "pair vivant annoncé en 100");
        assert!(!reg.predecesseur_vivant(101), "aucune annonce en 101");
        reg.effacer_reservation(i);
        assert!(!reg.predecesseur_vivant(100), "annonce effacée");
    }

    #[test]
    fn un_pair_mort_qui_a_annonce_sa_plage_ne_bloque_pas() {
        let (_td, reg) = registre_neuf();
        // Un pid qui n'existe plus : un enfant lancé puis récolté.
        let mut enfant = std::process::Command::new("true").spawn().unwrap();
        let pid_mort = u64::from(enfant.id());
        enfant.wait().unwrap();
        let i = reg
            .reclamer_emplacement(pid_mort, 0)
            .expect("emplacement libre");
        reg.annoncer_reservation(i, 200);
        assert!(
            !reg.predecesseur_vivant(200),
            "pair MORT : la plage est récupérable"
        );
    }

    #[test]
    fn un_emplacement_libere_ne_bloque_plus() {
        let (_td, reg) = registre_neuf();
        let (pid, demarrage) = identite_du_processus_courant();
        let i = reg.reclamer_emplacement(pid, demarrage).unwrap();
        reg.annoncer_reservation(i, 300);
        reg.liberer_emplacement(i);
        assert!(!reg.predecesseur_vivant(300));
    }

    #[test]
    fn un_emplacement_de_pair_mort_est_repris() {
        let (_td, reg) = registre_neuf();
        let mut enfant = std::process::Command::new("true").spawn().unwrap();
        let pid_mort = u64::from(enfant.id());
        enfant.wait().unwrap();
        let i_mort = reg.reclamer_emplacement(pid_mort, 0).unwrap();
        reg.annoncer_reservation(i_mort, 400);
        let (pid, demarrage) = identite_du_processus_courant();
        let i = reg.reclamer_emplacement(pid, demarrage).expect("repris");
        assert_eq!(i, i_mort, "le pair mort cède son emplacement");
        assert!(
            !reg.predecesseur_vivant(400),
            "l'annonce du mort a été effacée à la reprise"
        );
    }
}
