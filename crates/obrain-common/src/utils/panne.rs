//! Point d'injection de panne : tue le processus ENTRE deux écritures d'une
//! même opération, pour qu'un test de durabilité vise la fenêtre exacte au
//! lieu de la viser au hasard avec un `kill -9` chronométré (obrain#120).
//!
//! Désarmé par défaut ; le coût en production est un chargement atomique
//! relâché par propriété posée.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

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
