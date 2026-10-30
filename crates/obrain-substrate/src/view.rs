//! # Vues zero-copy sur les propriétés — le pont mémoire (chantier O1)
//!
//! Ce module expose des VUES empruntées sur les octets du substrate, pour
//! qu'un moteur d'inférence embarquant obrain lise le texte d'un fait ou la
//! propriété d'un nœud **sans une seule copie intermédiaire** — là où le
//! chemin historique payait : copie de page 4 KiB (`read_heap_page`),
//! `to_vec()` du payload, `ArcStr::from` à la matérialisation en [`Value`],
//! puis sérialisation JSON côté appelant.
//!
//! ## Contrat de lifetimes — une vue est un bail de lecture
//!
//! Chaque vue tient LA garde de lecture de la zone d'où viennent ses octets :
//!
//! * [`StrView::Zone`] — garde mappée (`parking_lot::RwLockReadGuard::try_map`)
//!   sur la `PropsZone` : le `&str` pointe DANS la page heap mmap.
//! * [`StrView::Blob`] — garde `Arc` (`read_arc`, feature `arc_lock`) sur la
//!   colonne blob : le slice est re-dérivé à chaque accès, bornes et UTF-8
//!   revérifiés — safe, sans cache d'adresse.
//! * [`StrView::Shared`] — chemin hérité (DashMap) : clone d'`ArcStr`, un
//!   bump de refcount, zéro copie d'octets.
//!
//! **Pourquoi c'est sûr** : toute croissance ou remap d'un mmap de zone passe
//! par `&mut` (donc par la garde d'ÉCRITURE du même `RwLock`). Tant qu'une
//! vue vit, sa garde de lecture est tenue : les octets ne bougent pas. Le
//! borrow checker interdit de faire survivre la vue au store :
//!
//! ```compile_fail
//! use obrain_substrate::store::SubstrateStore;
//! use obrain_common::types::NodeId;
//!
//! let vue;
//! {
//!     let store = SubstrateStore::open_tempfile().unwrap();
//!     vue = store.node_prop_str_view(NodeId(0), "texte");
//! } // `store` détruit ici…
//! let _ = vue; // …erreur E0597 : la vue ne peut pas lui survivre.
//! ```
//!
//! **Discipline de verrous** : une vue est un bail COURT. Ne pas tenir une
//! vue en appelant une écriture sur le même store (l'écrivain attendrait la
//! garde) — même règle que les commentaires VERROUS des chemins `lookup_*_v2`.
//! Lecture concurrente : plusieurs vues simultanées sont licites (gardes
//! partagées), y compris sur un store `open_read_only` partagé entre process.
//!
//! ## Multi-architectures
//!
//! Les formats lus ici sont little-endian EXPLICITE (`u32::from_le_bytes`
//! dans `HeapPage::read_at`, `try_pod_read_unaligned` sur l'index blob) et
//! l'alignement est VÉRIFIÉ à l'exécution (`bytemuck::try_from_bytes`), pas
//! supposé. aarch64, x86_64 et wasm32 sont tous LE : mêmes octets, même code,
//! aucun `transmute`, aucun bloc `unsafe` dans ce module.

use crate::blob_column::BlobColumnWriter;
use obrain_common::types::ArcStr;
use parking_lot::lock_api::ArcRwLockReadGuard;
use parking_lot::{MappedRwLockReadGuard, RawRwLock};
use std::fmt;
use std::ops::Deref;

/// D'où viennent les octets d'une [`StrView`] — diagnostic et benchs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrViewSource {
    /// Heap mmap de la PropsZone v2 (`substrate.props.heap.v2`).
    Zone,
    /// Colonne blob (`substrate.blobcol.*`, valeurs > 256 B).
    Blob,
    /// DashMap héritée (sidecar props non migré) — `ArcStr` partagé.
    Shared,
}

/// Vue sur une propriété `String` d'une colonne blob : la garde de lecture
/// de la colonne (comptée par `Arc`, donc sans lifetime) plus le slot.
///
/// Le slice n'est PAS mis en cache : chaque [`Self::as_str`] refait la
/// lecture bornée (`read_slot`) et la validation UTF-8 — deux passes sûres
/// sur des octets immobiles (le remap de la colonne exige la garde
/// d'écriture, exclue tant que `self` vit). La validation a déjà réussi à la
/// construction ; les `expect` ci-dessous sont donc inatteignables.
pub struct BlobStrView {
    guard: ArcRwLockReadGuard<RawRwLock, BlobColumnWriter>,
    slot: u32,
}

impl BlobStrView {
    /// Construit la vue APRÈS validation par l'appelant (tag String + UTF-8).
    pub(crate) fn new(guard: ArcRwLockReadGuard<RawRwLock, BlobColumnWriter>, slot: u32) -> Self {
        Self { guard, slot }
    }

    /// Le texte, emprunté au mmap de la colonne sous la garde tenue.
    pub fn as_str(&self) -> &str {
        let bytes = self
            .guard
            .read_slot(self.slot)
            .expect("slot validé à la construction, immobile sous garde de lecture");
        let (_tag, payload) = bytes
            .split_first()
            .expect("payload blob non vide, validé à la construction");
        std::str::from_utf8(payload)
            .expect("UTF-8 validé à la construction, octets immobiles sous garde")
    }
}

/// Vue zero-copy sur une propriété `String` : `&str` emprunté aux pages du
/// substrate, vivant sous une garde de lecture (voir le contrat au niveau
/// du module).
///
/// Se déréférence en `&str` : `&*vue`, `vue.len()`, `tokenizer.encode(&vue)`…
pub enum StrView<'a> {
    /// Emprunt direct au heap mmap de la PropsZone.
    Zone(MappedRwLockReadGuard<'a, str>),
    /// Emprunt à une colonne blob (> 256 B) sous garde `Arc`.
    Blob(BlobStrView),
    /// `ArcStr` partagé (chemin hérité) — refcount, pas de copie d'octets.
    Shared(ArcStr),
}

impl StrView<'_> {
    /// Le texte visé.
    pub fn as_str(&self) -> &str {
        match self {
            StrView::Zone(g) => g,
            StrView::Blob(b) => b.as_str(),
            StrView::Shared(s) => s.as_str(),
        }
    }

    /// D'où viennent les octets (diagnostic, benchs, tests).
    pub fn source(&self) -> StrViewSource {
        match self {
            StrView::Zone(_) => StrViewSource::Zone,
            StrView::Blob(_) => StrViewSource::Blob,
            StrView::Shared(_) => StrViewSource::Shared,
        }
    }
}

impl Deref for StrView<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for StrView<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for StrView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for StrView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StrView")
            .field("source", &self.source())
            .field("len", &self.as_str().len())
            .finish()
    }
}

impl PartialEq<str> for StrView<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for StrView<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// La vue métier du seeding spéculatif : un fait du graphe, son texte
/// emprunté au substrate. Le moteur d'inférence tokenise `&fact.text`
/// sans copie ni sérialisation.
pub struct FactView<'a> {
    /// Le nœud porteur du fait.
    pub node: obrain_common::types::NodeId,
    /// Son texte, vue zero-copy.
    pub text: StrView<'a>,
}
