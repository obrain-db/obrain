//! ── BORNE D'ALLOCATION PARTAGÉE ENTRE PROCESSUS (Phase B, pièce 2) ──────────
//!
//! ## Le défaut mesuré qu'elle ferme
//!
//! `next_node_id` / `next_edge_id` étaient des atomiques **PAR PROCESSUS** :
//! chargés du dictionnaire à l'ouverture (`store.rs`, `next_node_id: AtomicU32::
//! new(snapshot.next_node_id)`), avancés par `fetch_add`, persistés au flush.
//! Tant qu'un store n'avait qu'un écrivain, ce compteur suffisait.
//!
//! Dès qu'il en a DEUX — ce que `SubstrateFile::open_shared_write` autorise
//! explicitement (verrou `LOCK_SH`, fait pour le GraphWriter) — les deux
//! chargent le MÊME dictionnaire, donc la MÊME valeur, donc allouent les MÊMES
//! slots. Le second écrase le `NodeRecord` du premier **dans la zone mmap**,
//! avec des CRC valides et sans un mot : une perte silencieuse que rien ne
//! rattrape ensuite (le WAL contient bien les deux écritures, mais elles portent
//! le même `node_id` — le rejeu applique la seconde sur la première, ce qui est
//! exactement ce que le disque montre).
//!
//! ## Pourquoi une FILE mmap partagée, et non un compteur relu du disque
//!
//! Relire une borne sur le disque à chaque allocation (`fetch_max` sur un fichier)
//! coûterait une E/S par nœud. Un `flock` autour de [lire, incrémenter, écrire]
//! coûterait un aller-retour noyau par nœud ET sérialiserait les écrivains — le
//! « verrou chaud » que la phase interdit.
//!
//! Un `AtomicU64` sur un mapping **MAP_SHARED** est, lui, partagé par tous les
//! processus qui mappent le même fichier : `fetch_add` y est une instruction
//! atomique du processeur (`lock xadd` sur x86-64), donc deux processus ne
//! peuvent PAS obtenir la même valeur — sans verrou, sans syscall, sans
//! coordination. C'est la même famille de garantie que le `O_APPEND` de l'axe
//! append : **l'atome et le noyau**, jamais un verrou applicatif.
//!
//! L'allocation devient donc : `fetch_add` sur la borne partagée (≈ 5-20 ns),
//! puis `fetch_max` sur le miroir local du processus (pour que les lecteurs de CE
//! processus voient un filigrane cohérent).
//!
//! ## La durabilité, et ce qui la répare
//!
//! La borne vit dans `substrate.alloc` (4 Kio, mappés partagés). Ses pages
//! sales sont écrites par le noyau, mais un arrêt brutal peut les perdre — et
//! c'est SANS GRAVITÉ, parce que la borne n'est pas la source de vérité : **la
//! source de vérité est la ZONE**. À chaque ouverture, le filigrane est RELEVÉ
//! des zones (`SubstrateStore::relever_filigranes_des_zones`) : le plus grand
//! slot ALLOUÉ et non tombstoné, +1. La borne n'est qu'un ACCÉLÉRATEUR et un
//! FUSIBLE : elle empêche deux processus ouverts EN MÊME TEMPS de se partager un
//! slot, ce que la zone ne peut pas faire (elle n'est relue qu'à l'ouverture).
//!
//! ## Pourquoi le filigrane ne recule JAMAIS
//!
//! Toutes les écritures de la borne passent par `fetch_max`, et la relecture des
//! zones à l'ouverture passe aussi par `fetch_max`. Un écrivain en retard (qui a
//! chargé un dictionnaire périmé) ne peut donc pas faire RECULER la borne — s'il
//! pouvait, il réallouerait un slot vivant. C'est la même discipline que
//! `persist_dict`, qui fusionne le dictionnaire sur disque au lieu de l'écraser.

#![allow(unsafe_code)]

use crate::error::{SubstrateError, SubstrateResult};
use memmap2::{MmapMut, MmapOptions};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Nom du fichier de borne dans le répertoire du substrat.
pub const FILENAME_BORNE_ALLOCATION: &str = "substrate.alloc";

/// Magie du fichier : « OBRA-ALC-1 ». Toute autre valeur = fichier d'une autre
/// nature (ou d'une version future) : on le RÉINITIALISE depuis les zones plutôt
/// que de lire des octets qui ne veulent rien dire.
const MAGIE: u64 = u64::from_le_bytes(*b"OBRAALC1");

/// Taille du fichier : une page, pour que le mapping soit aligné et que le
/// fichier ne soit jamais remappé (un remap changerait l'adresse de l'atome sous
/// les pieds d'un autre processus — le mmap partagé exige une adresse stable).
const TAILLE: u64 = 4096;

/// Décalages (en octets, multiples de 8 — l'alignement des `AtomicU64`).
const OFF_MAGIE: usize = 0;
const OFF_NOEUD: usize = 8;
const OFF_ARETE: usize = 16;
const OFF_ENGRAM: usize = 24;
/// Compteur d'écritures de la borne — diagnostic : « la borne a-t-elle servi ? »
const OFF_PUBLICATIONS: usize = 32;
/// (MP-3) Curseurs d'allocation de `PropsZone`, PARTAGÉS entre processus : la
/// prochaine page de propriétés v2 (`substrate.props.v2`) et la prochaine page
/// de tas (`substrate.props.heap.v2`). Même défaut, même remède que les slots :
/// deux écrivains `open_shared_write` partaient chacun d'un curseur LOCAL
/// (recherche binaire sur les magics à l'ouverture, `+= 1` ensuite) et posaient
/// la MÊME page — mesuré le 24/11/2026 : 616 valeurs sur 1 200 relues comme
/// celles d'un autre nœud (`mw_interprocess_phase_b::t8`). Mots jusque-là nuls
/// dans tout `substrate.alloc` existant : un fichier antérieur publie donc son
/// curseur local par `fetch_max` et repart de là, sans migration.
const OFF_PAGES_PROPS: usize = 40;
const OFF_PAGES_TAS: usize = 48;

/// ── VERROUS D'ENTITÉ INTER-PROCESSUS (MP-3b) ─────────────────────────────────
///
/// Un mot par tranche, à partir de `OFF_VERROUS_ENTITE`. Valeur `0` = libre ;
/// sinon `pid` (32 bits bas) `|` empreinte du démarrage (32 bits hauts) — voir
/// `wal_presence::empreinte_demarrage` pour la raison d'un mot UNIQUE.
///
/// 240 tranches × 8 o = 1920 o, à partir de 64 : fin à 1984 o, bien dans les
/// 4096 de la page. Aucune migration : ces octets étaient nuls.
const OFF_VERROUS_ENTITE: usize = 64;
const NB_VERROUS_ENTITE: usize = 240;

/// ── LA BORNE, VUE PAR UN PROCESSUS ─────────────────────────────────────────
///
/// Un `BorneAllocation` par store ouvert. Il tient le `File` (pour qu'il ne soit
/// pas fermé sous le mapping) et le `MmapMut`.
#[derive(Debug)]
pub struct BorneAllocation {
    chemin: PathBuf,
    map: MmapMut,
    /// Gardé pour la durée de vie du mapping : fermer le descripteur ne détruit
    /// pas un mapping sur Linux, mais le garder rend la structure lisible.
    _fichier: File,
}

/// Une vue atomique sur un mot du mapping partagé.
///
/// SAFETY (le seul `unsafe` du module) : l'offset est un multiple de 8 et la base
/// du mapping est alignée sur une page (4096) — donc l'adresse est alignée pour
/// un `u64`. Le mapping est `MAP_SHARED` et vit aussi longtemps que `map`, qui
/// vit aussi longtemps que la référence rendue (durée d'emprunt). Aucune autre
/// vue mutable n'existe sur ces octets : `MmapMut` est emprunté par `self`.
fn mot(map: &MmapMut, offset: usize) -> &AtomicU64 {
    debug_assert_eq!(offset % 8, 0, "un AtomicU64 exige un offset aligné");
    debug_assert!(offset + 8 <= map.len(), "offset hors du mapping");
    // SAFETY: voir la doc ci-dessus — alignement et durée de vie garantis par
    // la construction de `BorneAllocation`.
    unsafe { &*(map.as_ptr().add(offset) as *const AtomicU64) }
}

impl BorneAllocation {
    /// Chemin du fichier de borne pour un répertoire de substrat.
    #[must_use]
    pub fn chemin_pour(dir: &Path) -> PathBuf {
        dir.join(FILENAME_BORNE_ALLOCATION)
    }

    /// Ouvre (ou crée) la borne d'un répertoire de substrat.
    ///
    /// Une borne illisible ou d'une autre magie est RÉINITIALISÉE à `(1, 1, 1)` :
    /// c'est sans danger, parce que l'appelant (`from_substrate`) fait suivre
    /// d'un `fetch_max` depuis les ZONES, qui est la vraie source de vérité. Un
    /// fichier de borne corrompu ne peut donc pas faire réallouer un slot vivant.
    pub fn ouvrir(dir: &Path) -> SubstrateResult<Self> {
        let chemin = Self::chemin_pour(dir);
        let fichier = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&chemin)?;
        let taille_actuelle = fichier.metadata()?.len();
        if taille_actuelle != TAILLE {
            // Une taille différente (fichier neuf = 0, ou tronqué) : on la remet à
            // 4096. Un `set_len` PLUS PETIT n'arrive jamais ici (TAILLE est
            // constante), donc on n'efface jamais une borne existante.
            fichier.set_len(TAILLE)?;
        }
        // SAFETY: le fichier vient d'être dimensionné à TAILLE octets ; le
        // mapping est immuable en taille pour toute la vie de l'objet (aucun
        // `set_len` postérieur), donc l'adresse des atomes ne bouge pas.
        let map = unsafe { MmapOptions::new().len(TAILLE as usize).map_mut(&fichier)? };
        let borne = Self {
            chemin,
            map,
            _fichier: fichier,
        };
        if mot(&borne.map, OFF_MAGIE).load(Ordering::Acquire) != MAGIE {
            // (ré)initialisation : magie + bornes à 1 (le slot 0 est la sentinelle
            // nulle — voir `record::node_flags`).
            mot(&borne.map, OFF_MAGIE).store(MAGIE, Ordering::Release);
            mot(&borne.map, OFF_NOEUD).store(1, Ordering::Release);
            mot(&borne.map, OFF_ARETE).store(1, Ordering::Release);
            mot(&borne.map, OFF_ENGRAM).store(1, Ordering::Release);
            borne.synchroniser();
        }
        Ok(borne)
    }

    /// LIT la borne d'un répertoire sans l'ouvrir en écriture, sans la créer et
    /// sans rien y publier : `(noeud, arete, engram)`, ou `None` si le fichier
    /// est absent, trop court ou d'une autre magie.
    ///
    /// C'est la voie d'une ouverture en LECTURE SEULE : [`Self::ouvrir`] crée le
    /// fichier, le redimensionne et incrémente son compteur de publications —
    /// trois écritures qu'un lecteur ne doit pas faire. Un lecteur n'alloue
    /// rien ; il lui suffit de savoir jusqu'où les pairs ont réservé.
    #[must_use]
    pub fn lire(dir: &Path) -> Option<(u64, u64, u64)> {
        use std::io::Read as _;
        let mut tampon = [0u8; 32];
        std::fs::File::open(Self::chemin_pour(dir))
            .ok()?
            .read_exact(&mut tampon)
            .ok()?;
        let mot = |off: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&tampon[off..off + 8]);
            u64::from_ne_bytes(b)
        };
        if mot(OFF_MAGIE) != MAGIE {
            return None;
        }
        Some((mot(OFF_NOEUD), mot(OFF_ARETE), mot(OFF_ENGRAM)))
    }

    /// Chemin du fichier.
    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    /// Borne courante des nœuds (`next_node_id` partagé).
    #[must_use]
    pub fn noeud(&self) -> u64 {
        mot(&self.map, OFF_NOEUD).load(Ordering::Acquire)
    }

    /// Borne courante des arêtes.
    #[must_use]
    pub fn arete(&self) -> u64 {
        mot(&self.map, OFF_ARETE).load(Ordering::Acquire)
    }

    /// Borne courante des engrams.
    #[must_use]
    pub fn engram(&self) -> u64 {
        mot(&self.map, OFF_ENGRAM).load(Ordering::Acquire)
    }

    /// Nombre de réservations publiées sur cette borne depuis sa création.
    #[must_use]
    pub fn publications(&self) -> u64 {
        mot(&self.map, OFF_PUBLICATIONS).load(Ordering::Relaxed)
    }

    /// ── RÉSERVE UN SLOT DE NŒUD ────────────────────────────────────────────
    ///
    /// Rend la valeur AVANT incrément (le slot réservé) : c'est un `fetch_add`,
    /// donc deux processus (ou deux threads) ne peuvent pas recevoir la même.
    pub fn reserver_noeud(&self) -> u64 {
        mot(&self.map, OFF_NOEUD).fetch_add(1, Ordering::AcqRel)
    }

    /// Réserve un slot d'arête — pendant de [`Self::reserver_noeud`].
    pub fn reserver_arete(&self) -> u64 {
        mot(&self.map, OFF_ARETE).fetch_add(1, Ordering::AcqRel)
    }

    /// Réserve un slot d'engram — pendant de [`Self::reserver_noeud`].
    pub fn reserver_engram(&self) -> u64 {
        mot(&self.map, OFF_ENGRAM).fetch_add(1, Ordering::AcqRel)
    }

    /// ── RÉSERVE UN BLOC DE `k` SLOTS DE NŒUDS ───────────────────────────────
    ///
    /// Rend le DÉBUT `d` du bloc `[d, d+k)` : c'est un `fetch_add(k, AcqRel)`,
    /// donc deux processus (ou deux threads) reçoivent des blocs DISJOINTS.
    /// C'est le « bulk bump » qui retire la borne partagée du chemin chaud :
    /// l'appelant ne la touche plus qu'une fois par `k` réservations au lieu
    /// d'une fois par réservation, et l'éclatement de ligne de cache (MESI)
    /// du `substrate.alloc` cesse de plafonner le débit cumulé.
    pub fn reserver_bloc_noeuds(&self, k: u64) -> u64 {
        debug_assert!(k > 0, "un bloc de 0 slot n'a pas de sens");
        mot(&self.map, OFF_NOEUD).fetch_add(k, Ordering::AcqRel)
    }

    /// Pendant de [`Self::reserver_bloc_noeuds`] pour les arêtes.
    pub fn reserver_bloc_aretes(&self, k: u64) -> u64 {
        debug_assert!(k > 0, "un bloc de 0 slot n'a pas de sens");
        mot(&self.map, OFF_ARETE).fetch_add(k, Ordering::AcqRel)
    }

    /// ── PUBLIE UNE BORNE SANS JAMAIS LA RECULER ────────────────────────────
    ///
    /// `fetch_max` et non `store` : un écrivain qui a chargé un dictionnaire (ou
    /// une zone) en retard publierait une valeur PLUS PETITE, et le prochain
    /// `reserver_*` rendrait un slot DÉJÀ VIVANT. C'est l'écrasement silencieux
    /// que cette pièce existe pour fermer — il ne faut pas le réintroduire par la
    /// porte de la persistance.
    pub fn publier_filigrane_noeuds(&self, v: u64) {
        mot(&self.map, OFF_NOEUD).fetch_max(v.max(1), Ordering::AcqRel);
        mot(&self.map, OFF_PUBLICATIONS).fetch_add(1, Ordering::Relaxed);
    }

    /// Pendant de [`Self::publier_filigrane_noeuds`] pour les arêtes.
    pub fn publier_filigrane_aretes(&self, v: u64) {
        mot(&self.map, OFF_ARETE).fetch_max(v.max(1), Ordering::AcqRel);
        mot(&self.map, OFF_PUBLICATIONS).fetch_add(1, Ordering::Relaxed);
    }

    /// Pendant de [`Self::publier_filigrane_noeuds`] pour les engrams.
    pub fn publier_filigrane_engrams(&self, v: u64) {
        mot(&self.map, OFF_ENGRAM).fetch_max(v.max(1), Ordering::AcqRel);
        mot(&self.map, OFF_PUBLICATIONS).fetch_add(1, Ordering::Relaxed);
    }

    /// (MP-3) Prochaine page de propriétés v2 (curseur partagé).
    #[must_use]
    pub fn pages_props(&self) -> u64 {
        mot(&self.map, OFF_PAGES_PROPS).load(Ordering::Acquire)
    }

    /// (MP-3) Prochaine page de tas (curseur partagé).
    #[must_use]
    pub fn pages_tas(&self) -> u64 {
        mot(&self.map, OFF_PAGES_TAS).load(Ordering::Acquire)
    }

    /// ── RÉSERVE `k` PAGES DE PROPRIÉTÉS v2 ──────────────────────────────────
    ///
    /// Rend le DÉBUT `d` de la plage `[d, d+k)` : `fetch_add`, donc deux
    /// processus reçoivent des plages DISJOINTES. `k == 0` ne réserve rien et
    /// rend le curseur courant.
    pub fn reserver_pages_props(&self, k: u64) -> u64 {
        mot(&self.map, OFF_PAGES_PROPS).fetch_add(k, Ordering::AcqRel)
    }

    /// Pendant de [`Self::reserver_pages_props`] pour les pages de tas.
    pub fn reserver_pages_tas(&self, k: u64) -> u64 {
        mot(&self.map, OFF_PAGES_TAS).fetch_add(k, Ordering::AcqRel)
    }

    /// (MP-3) Publie le curseur de pages de propriétés calculé depuis la zone,
    /// sans jamais le reculer (`fetch_max`, même raison que pour les slots).
    pub fn publier_filigrane_pages_props(&self, v: u64) {
        mot(&self.map, OFF_PAGES_PROPS).fetch_max(v, Ordering::AcqRel);
        mot(&self.map, OFF_PUBLICATIONS).fetch_add(1, Ordering::Relaxed);
    }

    /// Pendant de [`Self::publier_filigrane_pages_props`] pour le tas.
    pub fn publier_filigrane_pages_tas(&self, v: u64) {
        mot(&self.map, OFF_PAGES_TAS).fetch_max(v, Ordering::AcqRel);
        mot(&self.map, OFF_PUBLICATIONS).fetch_add(1, Ordering::Relaxed);
    }

    /// Tranche de verrou d'une entité : `clé % 240`. Nœuds et arêtes partagent
    /// le tableau (une collision entre un nœud et une arête ne coûte qu'une
    /// attente, jamais une erreur).
    fn rang_verrou(cle: u64) -> usize {
        (cle % NB_VERROUS_ENTITE as u64) as usize
    }

    fn mot_verrou(&self, rang: usize) -> &AtomicU64 {
        mot(&self.map, OFF_VERROUS_ENTITE + rang * 8)
    }

    /// Ce qu'on écrit dans le mot : `pid | empreinte << 32`. Jamais `0` (un pid
    /// nul désignerait le groupe de processus, et `0` signifie « libre »).
    ///
    /// CALCULÉ UNE SEULE FOIS. `identite_du_processus_courant()` lit l'heure de
    /// démarrage par un appel système (`proc_pidinfo` sur macOS, `/proc` sur
    /// Linux) : le refaire à chaque écriture de propriété coûtait un aller-retour
    /// noyau par écriture. Mesuré le 24/11/2026 en alternance avant/après, à
    /// charge 51-63 : 2 232 → 4 724 ns/écriture (+112 %) au premier tour. Le pid
    /// et le démarrage d'un processus ne changent pas de sa vie : un `OnceLock`
    /// suffit, et il n'y a plus qu'un CAS et un `store` par écriture.
    fn mon_jeton() -> u64 {
        static JETON: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *JETON.get_or_init(|| {
            let (pid, demarrage) = crate::wal_presence::identite_du_processus_courant();
            (pid & 0xFFFF_FFFF)
                | (u64::from(crate::wal_presence::empreinte_demarrage(demarrage)) << 32)
        })
    }

    /// Un SEUL essai, sans attente : `None` si la tranche est tenue par un autre.
    #[must_use]
    pub fn essayer_entite(&self, cle: u64) -> Option<VerrouEntite<'_>> {
        let rang = Self::rang_verrou(cle);
        let mot = self.mot_verrou(rang);
        match mot.compare_exchange(0, Self::mon_jeton(), Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Some(VerrouEntite::neuf(mot, rang)),
            Err(_) => None,
        }
    }

    /// Prend le verrou d'une entité, en attendant qu'il se libère.
    ///
    /// ## Pas de vol sur délai — uniquement sur PREUVE DE MORT
    ///
    /// Un écrivain vivant mais préempté garde son verrou : on attend. Un
    /// délai ne distingue pas « mort » de « lent », et voler un vivant
    /// corromprait la page qu'il est en train d'écrire. Le seul cas de vol est
    /// `wal_presence::etat_processus_empreinte` qui répond `Mort` (pid
    /// disparu, zombie, ou pid réattribué à un processus démarré après lui).
    ///
    /// La preuve de vie est un aller-retour noyau : elle n'est tentée qu'après
    /// 64 tours d'attente, et au plus tous les 64 tours ensuite.
    pub fn verrouiller_entite(&self, cle: u64) -> VerrouEntite<'_> {
        let rang = Self::rang_verrou(cle);
        let mot = self.mot_verrou(rang);
        let jeton = Self::mon_jeton();
        let mut tours: u32 = 0;
        loop {
            match mot.compare_exchange(0, jeton, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return VerrouEntite::neuf(mot, rang),
                Err(courant) => {
                    // `wrapping_add` et non `saturating_add` : à saturation
                    // (u32::MAX, reste 63 modulo 64), la condition
                    // `is_multiple_of(64)` ne serait plus jamais vraie et la
                    // preuve de vie cesserait définitivement de tourner.
                    tours = tours.wrapping_add(1);
                    if tours > 64 && tours.is_multiple_of(64) {
                        let pid = courant & 0xFFFF_FFFF;
                        let empreinte = (courant >> 32) as u32;
                        if crate::wal_presence::etat_processus_empreinte(pid, empreinte)
                            == crate::wal_presence::EtatProcessus::Mort
                        {
                            // Le vol DOIT rendre le garde quand il réussit. Sans
                            // ce `return`, la boucle réexamine le mot, y trouve
                            // NOTRE jeton (que le CAS vient d'écrire), demande la
                            // preuve de vie sur notre propre pid — vivant, à coup
                            // sûr — et attend un verrou que nous tenons.
                            // Mesuré le 24/11 (`t9c`) : attente infinie, mot à
                            // `pid=nous`, verdict `Vivant`.
                            if mot
                                .compare_exchange(
                                    courant,
                                    jeton,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_ok()
                            {
                                return VerrouEntite::neuf(mot, rang);
                            }
                        }
                    }
                    if tours < 64 {
                        std::hint::spin_loop();
                    } else if tours < 256 {
                        std::thread::yield_now();
                    } else {
                        std::thread::sleep(std::time::Duration::from_micros(100));
                    }
                }
            }
        }
    }

    /// (MP-3) Comme [`Self::synchroniser`], mais l'erreur REMONTE : depuis que la
    /// borne porte les curseurs de pages, sa durabilité avant celle des zones
    /// est une exigence de correction en écriture partagée (voir `flush`).
    pub fn synchroniser_durable(&self) -> SubstrateResult<()> {
        self.map.flush()?;
        Ok(())
    }

    /// Force les pages sales de la borne vers le disque, en IGNORANT l'échec.
    ///
    /// Ce n'est PAS une exigence de correction pour les bornes d'IDENTIFIANTS
    /// (`noeud`, `arete`, `engram`) : elles se reconstituent depuis les zones à
    /// chaque ouverture, donc les perdre ne coûte qu'un relevé.
    ///
    /// Ce n'est plus vrai depuis MP-3 des curseurs de PAGES (`pages_props`,
    /// `pages_tas`) : une page réservée mais pas encore écrite laisse un TROU,
    /// et la frontière recalculée à l'ouverture (préfixe contigu supposé) passe
    /// SOUS ce trou. Sans la borne pour la relever (`fetch_max`), l'ouverture
    /// réallouerait des pages vivantes. Ces deux mots-là exigent
    /// [`Self::synchroniser_durable`], appelée AVANT les zones.
    ///
    /// Les appelants qui ne peuvent pas échouer (chemin « store propre » du
    /// dictionnaire, où le WAL est immobile donc aucune réservation n'a eu lieu
    /// depuis le dernier `flush`) peuvent garder cette variante : la borne y est
    /// déjà durable.
    pub fn synchroniser(&self) {
        let _ = self.map.flush();
    }
}

/// Garde du verrou d'entité : rend la tranche libre en tombant, y compris sur
/// panique (déroulement), jamais sur `SIGKILL` — c'est la preuve de vie qui
/// récupère ce cas.
pub struct VerrouEntite<'a> {
    mot: &'a AtomicU64,
    rang: usize,
}

impl<'a> VerrouEntite<'a> {
    fn neuf(mot: &'a AtomicU64, rang: usize) -> Self {
        #[cfg(test)]
        tranches::prendre(rang);
        Self { mot, rang }
    }

    /// Rang de la tranche tenue (diagnostic, tests).
    #[must_use]
    pub fn rang(&self) -> usize {
        self.rang
    }
}

impl Drop for VerrouEntite<'_> {
    fn drop(&mut self) {
        // `0` = libre. Le jeton du suivant sera publié par SON CAS : rien à
        // effacer d'autre, donc aucun risque d'écraser son empreinte.
        self.mot.store(0, Ordering::Release);
        #[cfg(test)]
        tranches::rendre(self.rang);
    }
}

/// Filet de développement : deux prises de la MÊME tranche par le même thread
/// seraient un interblocage (le verrou n'est pas réentrant). En debug on le dit
/// au lieu de figer le test.
#[cfg(test)]
mod tranches {
    use std::cell::RefCell;

    thread_local! {
        static TENUES: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn prendre(rang: usize) {
        TENUES.with(|t| {
            let mut t = t.borrow_mut();
            assert!(
                !t.contains(&rang),
                "verrou d'entité déjà tenu par ce thread (tranche {rang}) : le verrou \
                 n'est pas réentrant — deux entités de la même tranche ne doivent pas \
                 s'imbriquer"
            );
            t.push(rang);
        });
    }

    pub(super) fn rendre(rang: usize) {
        TENUES.with(|t| {
            let mut t = t.borrow_mut();
            if let Some(i) = t.iter().rposition(|r| *r == rang) {
                t.remove(i);
            }
        });
    }
}

/// ── LA BORNE D'UN STORE QUI N'EN A PAS ───────────────────────────────────────
///
/// Le mode mémoire (`SubstrateFile::open_memory`, wasm32) n'a pas de système de
/// fichiers : il n'y a ni second processus, ni borne partagée. Les atomiques par
/// processus du store suffisent alors, exactement comme avant cette pièce — et
/// le chemin d'allocation reste celui des tests unitaires.
impl BorneAllocation {
    /// Message de refus quand on demande une borne à un substrat en mémoire.
    #[must_use]
    pub fn refus_memoire() -> SubstrateError {
        SubstrateError::Internal(
            "borne d'allocation partagée : indisponible sur un substrat en mémoire \
             (aucun second processus ne peut exister — le compteur par processus suffit)"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests_pages_mp3 {
    use super::*;

    /// Deux vues du même fichier (deux processus, en pratique) : les plages de
    /// pages réservées sont disjointes, le filigrane ne recule jamais, et le
    /// fichier garde sa taille — aucune migration de `substrate.alloc`.
    #[test]
    fn les_pages_sont_reservees_de_facon_disjointe_et_le_filigrane_ne_recule_pas() {
        let td = tempfile::tempdir().unwrap();
        let a = BorneAllocation::ouvrir(td.path()).unwrap();
        let b = BorneAllocation::ouvrir(td.path()).unwrap();
        assert_eq!(
            (a.pages_props(), a.pages_tas()),
            (0, 0),
            "mots neufs : nuls"
        );

        a.publier_filigrane_pages_props(7);
        b.publier_filigrane_pages_props(3);
        assert_eq!(
            b.pages_props(),
            7,
            "un écrivain en retard ne recule pas la borne"
        );

        let p1 = a.reserver_pages_props(2);
        let p2 = b.reserver_pages_props(1);
        let p3 = a.reserver_pages_props(1);
        assert_eq!(
            (p1, p2, p3),
            (7, 9, 10),
            "plages [7,9) [9,10) [10,11) disjointes"
        );
        assert_eq!(a.pages_props(), 11);
        assert_eq!(a.reserver_pages_props(0), 11, "k = 0 ne réserve rien");

        assert_eq!(b.reserver_pages_tas(4), 0);
        assert_eq!(a.pages_tas(), 4);
        b.publier_filigrane_pages_tas(2);
        assert_eq!(a.pages_tas(), 4, "le tas non plus ne recule pas");

        let taille = std::fs::metadata(BorneAllocation::chemin_pour(td.path()))
            .unwrap()
            .len();
        assert_eq!(
            taille, TAILLE,
            "les mots 40 et 48 tiennent dans la page existante"
        );
        // Les bornes d'avant restent lisibles et intactes.
        assert_eq!(a.noeud(), b.noeud());
    }
}

/// ── VERROU D'ENTITÉ (MP-3b) — tests unitaires ────────────────────────────────
#[cfg(test)]
mod tests_verrou_entite {
    use super::*;

    fn borne(td: &tempfile::TempDir) -> BorneAllocation {
        BorneAllocation::ouvrir(td.path()).expect("borne")
    }

    /// Deux clés de tranches DIFFÉRENTES ne s'attendent pas.
    #[test]
    fn deux_tranches_distinctes_se_prennent_toutes_les_deux() {
        let td = tempfile::tempdir().unwrap();
        let b = borne(&td);
        let a1 = b.essayer_entite(1).expect("tranche de 1");
        let a2 = b.essayer_entite(2).expect("tranche de 2");
        assert_ne!(a1.rang(), a2.rang());
    }

    /// La même tranche, deux fois : la seconde est refusée — c'est le verrou,
    /// et c'est aussi ce qui interdit une réentrance silencieuse.
    #[test]
    fn une_tranche_deja_tenue_est_refusee_au_second_essai() {
        let td = tempfile::tempdir().unwrap();
        let b = borne(&td);
        let _g = b.essayer_entite(7).expect("première prise");
        // 7 et 7 + 240 tombent VOLONTAIREMENT sur la même tranche : c'est ce
        // qu'on veut éprouver ici.
        assert!(
            b.essayer_entite(7 + NB_VERROUS_ENTITE as u64).is_none(),
            "la tranche est tenue : le second essai doit échouer"
        );
    }

    /// Un détenteur PROUVÉ MORT est volé — sans délai, par preuve.
    #[test]
    fn un_detenteur_mort_est_vole() {
        let td = tempfile::tempdir().unwrap();
        let b = borne(&td);
        let rang = BorneAllocation::rang_verrou(11);
        // Un pid très au-delà du maximum de la plateforme : `kill` rend ESRCH.
        let pid_mort: u64 = 999_999_999;
        b.mot_verrou(rang)
            .store(pid_mort | (0x1234_5678u64 << 32), Ordering::Release);
        let g = b.verrouiller_entite(11);
        assert_eq!(g.rang(), rang, "le verrou a été volé au mort");
        assert_ne!(
            b.mot_verrou(rang).load(Ordering::Acquire),
            pid_mort | (0x1234_5678u64 << 32),
            "le mot porte maintenant notre jeton"
        );
    }

    /// Un détenteur VIVANT n'est pas volé : l'essai unique échoue.
    #[test]
    fn un_detenteur_vivant_n_est_pas_vole() {
        let td = tempfile::tempdir().unwrap();
        let b = borne(&td);
        let rang = BorneAllocation::rang_verrou(13);
        // Notre propre pid, avec notre propre empreinte : vivant, à coup sûr.
        let (pid, demarrage) = crate::wal_presence::identite_du_processus_courant();
        let jeton = (pid & 0xFFFF_FFFF)
            | (u64::from(crate::wal_presence::empreinte_demarrage(demarrage)) << 32);
        b.mot_verrou(rang).store(jeton, Ordering::Release);
        assert!(
            b.essayer_entite(13).is_none(),
            "un détenteur vivant garde son verrou"
        );
        assert_eq!(b.mot_verrou(rang).load(Ordering::Acquire), jeton);
    }

    /// La chute du garde libère la tranche.
    #[test]
    fn la_liberation_rend_la_tranche_prenable() {
        let td = tempfile::tempdir().unwrap();
        let b = borne(&td);
        {
            let _g = b.essayer_entite(21).expect("prise");
        }
        let _g = b.essayer_entite(21).expect("reprise après libération");
    }

    /// L'empreinte dépend des 64 bits du démarrage : un repli `haut ^ bas`
    /// laisserait deux démarrages structurés identiques se confondre.
    #[test]
    fn l_empreinte_depend_des_64_bits_du_demarrage() {
        let base: u64 = 1_700_000_000_000_000;
        assert_ne!(
            crate::wal_presence::empreinte_demarrage(base),
            crate::wal_presence::empreinte_demarrage(base ^ (1u64 << 32)),
            "changer les 32 bits hauts doit changer l'empreinte"
        );
        assert_eq!(
            crate::wal_presence::empreinte_demarrage(0),
            0,
            "démarrage inconnu = empreinte 0 (comparaison sautée)"
        );
    }

    /// Le verrou tient dans la page : 240 tranches à partir de 64.
    #[test]
    fn les_verrous_tiennent_dans_la_page() {
        assert_eq!(OFF_VERROUS_ENTITE % 8, 0);
        assert!(
            OFF_VERROUS_ENTITE + NB_VERROUS_ENTITE * 8 <= TAILLE as usize,
            "240 verrous de 8 o doivent tenir dans {TAILLE} o"
        );
    }
}
