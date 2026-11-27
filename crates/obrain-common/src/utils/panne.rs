//! Point d'injection de panne : tue le processus ENTRE deux écritures d'une
//! même opération, pour qu'un test de durabilité vise la fenêtre exacte au
//! lieu de la viser au hasard avec un `kill -9` chronométré (obrain#120).
//!
//! Désarmé par défaut ; le coût en production est un chargement atomique
//! relâché par propriété posée.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;

/// 0 = désarmé ; sinon `rang + 1` après lequel le processus s'arrête.
static ARRET_APRES_PROPRIETE: AtomicU64 = AtomicU64::new(0);

/// Arme l'arrêt : le processus s'interrompt (`abort`, ni destructeur ni
/// flush — état identique à un SIGKILL) juste après que la propriété de rang
/// `rang` (0 = juste après la création du nœud, avant toute propriété) d'une
/// création de nœud avec propriétés a été écrite.
#[doc(hidden)]
pub fn armer_arret_creation_noeud(rang: u64) {
    ARRET_APRES_PROPRIETE.store(rang + 1, Ordering::SeqCst);
}

/// Points nommés d'une création EN TRANSACTION EXPLICITE : juste après
/// l'écriture de l'entité (`NodeInsert` / `EdgeInsert`), là où, avant le
/// correctif, la déclaration `TxNoeudEnAttente` / `TxAreteEnAttente` n'était
/// pas encore écrite.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PointTx {
    /// `create_node_versioned`, après l'écriture du nœud.
    NoeudEcrit = 1,
    /// `create_edge_versioned`, après l'écriture de l'arête.
    AreteEcrite = 2,
}

/// 0 = désarmé ; sinon le `PointTx` où le processus s'arrête.
static ARRET_POINT_TX: AtomicU8 = AtomicU8::new(0);

/// Arme l'arrêt (`abort`, état d'un SIGKILL) au point `point`.
#[doc(hidden)]
pub fn armer_arret_tx(point: PointTx) {
    ARRET_POINT_TX.store(point as u8, Ordering::SeqCst);
}

/// À appeler au point `point` d'une création en transaction explicite.
#[doc(hidden)]
#[inline]
pub fn point_tx(point: PointTx) {
    if ARRET_POINT_TX.load(Ordering::Relaxed) == point as u8 {
        std::process::abort();
    }
}

/// À appeler après le `rang`-ième temps d'une création de nœud avec
/// propriétés (0 = nœud créé, aucune propriété).
#[doc(hidden)]
#[inline]
pub fn point_creation_noeud(rang: u64) {
    if ARRET_APRES_PROPRIETE.load(Ordering::Relaxed) == rang + 1 {
        std::process::abort();
    }
}

/// 0 = désarmé ; sinon le processus s'arrête entre l'écriture de l'ancre
/// composite et celle du miroir de colonne, dans `decay_clock::reinforce`.
static ARRET_ENTRE_ANCRE_ET_MIROIR: AtomicU8 = AtomicU8::new(0);

/// Arme l'arrêt (`abort`, état d'un SIGKILL) juste après l'écriture de
/// l'ancre composite et AVANT le miroir de colonne.
#[doc(hidden)]
pub fn armer_arret_entre_ancre_et_miroir() {
    ARRET_ENTRE_ANCRE_ET_MIROIR.store(1, Ordering::SeqCst);
}

/// Porte de blocage (test) : quand armée, `point_entre_ancre_et_miroir` signale
/// son arrivée puis attend la libération, au lieu d'abort. Permet un test
/// DÉTERMINISTE de l'entrelacement A/B (deux écrivains concurrents) sans tuer
/// le processus. Une seule porte à la fois (consommée par `take`).
static PORTE_BLOCAGE_ENTRE_ANCRE_ET_MIROIR: Mutex<Option<(Sender<()>, Receiver<()>)>> =
    Mutex::new(None);

/// 0 = désarmé ; sinon le blocage entre ancre et miroir est armé (test).
static BLOCAGE_ENTRE_ANCRE_ET_MIROIR_ARME: AtomicU8 = AtomicU8::new(0);

/// Arme le BLOCAGE (test) entre l'ancre et le miroir : rend `(liberation,
/// arrivee)`. Le premier écrivain qui atteint `point_entre_ancre_et_miroir`
/// signale sur `arrivee` puis attend sur `liberation`.
#[doc(hidden)]
pub fn armer_blocage_entre_ancre_et_miroir() -> (Sender<()>, Receiver<()>) {
    let (arrivee_tx, arrivee_rx) = std::sync::mpsc::channel();
    let (liberation_tx, liberation_rx) = std::sync::mpsc::channel();
    *PORTE_BLOCAGE_ENTRE_ANCRE_ET_MIROIR
        .lock()
        .unwrap() = Some((arrivee_tx, liberation_rx));
    BLOCAGE_ENTRE_ANCRE_ET_MIROIR_ARME.store(1, Ordering::SeqCst);
    (liberation_tx, arrivee_rx)
}

/// À appeler entre l'ancre composite et le miroir de colonne.
#[doc(hidden)]
#[inline]
pub fn point_entre_ancre_et_miroir() {
    if ARRET_ENTRE_ANCRE_ET_MIROIR.load(Ordering::Relaxed) == 1 {
        std::process::abort();
    }
    // Blocage (test), consommé UNE fois : signale l'arrivée puis attend la
    // libération. Sans effet en production (une charge atomique relâchée).
    if BLOCAGE_ENTRE_ANCRE_ET_MIROIR_ARME.load(Ordering::Relaxed) == 1 {
        BLOCAGE_ENTRE_ANCRE_ET_MIROIR_ARME.store(0, Ordering::Relaxed);
        if let Some((arrivee, liberation)) =
            PORTE_BLOCAGE_ENTRE_ANCRE_ET_MIROIR.lock().unwrap().take()
        {
            let _ = arrivee.send(());
            let _ = liberation.recv();
        }
    }
}

/// 0 = désarmé ; sinon `rang + 1` du point d'arrêt dans l'écriture du couple
/// (membres, poids) d'un engramme — voir `Writer::set_engram_state` dans
/// `obrain-substrate`.
static ARRET_ETAT_ENGRAM: AtomicU64 = AtomicU64::new(0);

/// Arme l'arrêt (`abort`, état d'un SIGKILL) au point `rang` de l'écriture du
/// couple d'un engramme :
///
/// - `rang = 0` : le record WAL est écrit, AUCUNE zone n'a été touchée ;
/// - `rang = 1` : les membres sont écrits, les poids ne le sont pas encore
///   (c'est la fenêtre où un demi-état existe dans les zones).
///
/// Sert à mesurer que le WAL fait AUTORITÉ : quelle que soit la zone où la
/// panne tombe, le rejeu rend le couple COMPLET du nouvel état — jamais
/// « membres neufs + poids anciens ». Deux bancs le mesurent dans
/// `obrain-substrate/tests/autorite_etat_engram.rs`.
#[doc(hidden)]
pub fn armer_arret_etat_engram(rang: u64) {
    ARRET_ETAT_ENGRAM.store(rang + 1, Ordering::SeqCst);
}

/// À appeler au point `rang` de l'écriture du couple d'un engramme.
#[doc(hidden)]
#[inline]
pub fn point_etat_engram(rang: u64) {
    if ARRET_ETAT_ENGRAM.load(Ordering::Relaxed) == rang + 1 {
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L'armement pose le drapeau, le désarmement le relève, et le point
    /// DÉSARMÉ traverse sans `abort`. Le drapeau est remis à zéro dans le même
    /// test : il est global au processus et les tests tournent en parallèle.
    #[test]
    fn armer_puis_desarmer_et_le_point_desarme_n_arrete_pas() {
        armer_arret_entre_ancre_et_miroir();
        assert_eq!(ARRET_ENTRE_ANCRE_ET_MIROIR.load(Ordering::SeqCst), 1);
        // Désarmement : un point armé appellerait `abort()` (testé par
        // `ancre_composite_kill9`, dans un processus enfant).
        ARRET_ENTRE_ANCRE_ET_MIROIR.store(0, Ordering::SeqCst);
        point_entre_ancre_et_miroir(); // ne doit pas s'arrêter
    }
}
