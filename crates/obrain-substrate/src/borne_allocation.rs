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

    /// Force les pages sales de la borne vers le disque.
    ///
    /// Appelé aux points où le substrat est déjà en train de rendre tout le reste
    /// durable (le flush). Ce n'est PAS une exigence de correction : la borne est
    /// reconstruite depuis les zones à chaque ouverture. C'est une politesse —
    /// sans elle, un redémarrage propre après un flush rejouerait le relevé des
    /// zones pour rien.
    pub fn synchroniser(&self) {
        let _ = self.map.flush();
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
