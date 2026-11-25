//! Réservation d'identité générique (#416, garantie transactionnelle
//! générique #413) — construction transactionnelle d'un nœud sous une
//! identité réservée par `(kind, clé)`, indépendante de `KnowledgeSpec` et
//! des engrammes.
//!
//! # Le modèle
//!
//! Réutilise le modèle de [`crate::writer::Writer::upsert_simplex`] :
//!
//! 1. une **clé canonique** (`cle: &[u8]`) dont un **hash** n'est qu'un
//!    départage — jamais une preuve d'égalité (les collisions sont levées
//!    par comparaison du contenu complet, relu depuis la propriété réservée) ;
//! 2. un **index vérifié** ([`ReservationIndex`]) tenu sous une **section
//!    critique commune** (un `Mutex` par store) : la recherche et la
//!    construction ne peuvent pas s'entrelacer ;
//! 3. la **matérialisation** n'arrive qu'après la recherche, sous la même
//!    section critique, en une unité de rejeu atomique (nœud + propriétés
//!    liées) : pas de nœud visible entre « clé écrite » et « propriétés
//!    posées ».
//!
//! La clé et la définition sont persistées comme propriétés du nœud
//! ([`PROP_CLE`], [`PROP_DEFINITION`], `Value::Bytes`) — sans nouveau format
//! disque ni record WAL ad hoc : la réservation est un nœud ordinaire, et le
//! rejeu existant (NodeInsert + PropSet + TxCommit) la rend durable.
//!
//! # Périmètre
//!
//! **Unicité LOCALE** : la section critique est un `Mutex` par processus.
//! Deux processus écrivant la même base n'ont pas ce verrou en commun —
//! l'unicité concurrente multi-processus relève de la convergence
//! distribuée, qui n'est PAS la promesse d'ici.

use std::collections::HashMap;

/// Nom de la propriété réservée qui porte la clé d'identité
/// (`Value::Bytes`, premier octet = `kind`). Réservé : un appelant ne doit
/// pas écrire cette propriété lui-même.
pub const PROP_CLE: &str = "_obrain_reservation_cle";
/// Nom de la propriété réservée qui porte la définition canonique
/// (`Value::Bytes`). Réservé, comme [`PROP_CLE`].
pub const PROP_DEFINITION: &str = "_obrain_reservation_definition";

/// Hash de départage d'une clé d'identité — xxh3 64 bits, même famille que
/// `simplex_zone::key_hash_members`. N'est JAMAIS une preuve d'égalité.
pub fn hash_cle(cle: &[u8]) -> u64 {
    // Surcharge de test : force le hash à une valeur fixe pour provoquer des
    // collisions et prouver que l'index VÉRIFIÉ les lève par comparaison du
    // contenu complet (jamais le hash seul).
    #[cfg(test)]
    if let Some(h) = HASH_CLE_FORCE.with(|c| c.get()) {
        return h;
    }
    twox_hash::XxHash3_64::oneshot(cle)
}

// Surcharge de test du hash (collision forcée). Absente du binaire de
// production.
#[cfg(test)]
thread_local! {
    static HASH_CLE_FORCE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Force le hash de toutes les clés à `h` (`None` = hachage réel) — tests de
/// collision uniquement. Absent du binaire de production.
#[cfg(test)]
pub(crate) fn forcer_hash_cle(h: Option<u64>) {
    HASH_CLE_FORCE.with(|c| c.set(h));
}

/// Résultat d'une construction sous identité réservée.
///
/// L'identifiant porté est un SLOT de nœud `u32` (l'espace d'identité du
/// substrat, borné à `u32::MAX` — résolution externe via
/// `NodeId(u64::from(slot))`). Ce n'est PAS un identifiant distribué ni un
/// hash : c'est le nœud qui porte la clé réservée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueReservation {
    /// L'identité était déjà réservée avec une définition identique : aucun
    /// nœud n'a été créé.
    Existante(u32),
    /// Nouvelle identité publiée (la construction a été exécutée une fois).
    Cree(u32),
    /// Même clé, définition incompatible : conflit explicite, rien n'est
    /// écrit.
    Conflit(u32),
}

impl IssueReservation {
    /// Le slot de nœud qui porte l'identité, quelle que soit l'issue
    /// (résolution en `NodeId` : `NodeId(u64::from(slot))`).
    pub fn identite(&self) -> u32 {
        match *self {
            IssueReservation::Existante(id)
            | IssueReservation::Cree(id)
            | IssueReservation::Conflit(id) => id,
        }
    }

    /// Vrai si la construction a réellement créé un nœud.
    pub fn est_creation(&self) -> bool {
        matches!(self, IssueReservation::Cree(_))
    }
}

/// Index d'identités réservées `(kind, hash_cle) → nœuds candidats`.
///
/// Miroir en mémoire, jamais persisté : reconstruit paresseusement au
/// premier usage par balayage des nœuds vivants (même contrat que
/// `SimplexKeyIndex` — l'ouverture ne paie rien). Les collisions de hash
/// sont portées par le `Vec` et levées par comparaison du contenu complet
/// (la clé relue depuis la propriété réservée).
#[derive(Default)]
pub struct ReservationIndex {
    map: HashMap<(u8, u64), Vec<u32>>,
}

impl ReservationIndex {
    pub fn insert(&mut self, kind: u8, hash: u64, id: u32) {
        self.map.entry((kind, hash)).or_default().push(id);
    }

    /// Les nœuds candidats pour `(kind, hash)`, à revérifier par contenu.
    pub fn candidats(&self, kind: u8, hash: u64) -> Option<&[u32]> {
        self.map.get(&(kind, hash)).map(Vec::as_slice)
    }
}
