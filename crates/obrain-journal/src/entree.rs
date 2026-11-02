//! L'entrée du journal — et le reçu que le client emporte.
//!
//! C'est **le même objet**, et c'est le point : ce que l'opérateur inscrit dans
//! son journal et ce que le client conserve dans sa poche sont identiques, octet
//! pour octet, signature comprise. Il n'existe donc pas de « version pour le
//! client » qui pourrait différer de la version inscrite.
//!
//! ## Ce que l'empreinte couvre, et ce qu'elle ne couvre pas
//!
//! `empreinte()` couvre le journal d'appartenance, la séquence, l'horodatage, la
//! charge et le maillon précédent — **elle ne couvre pas la signature**, qui
//! signe l'empreinte. C'est la construction habituelle d'une chaîne signée, et
//! elle a une conséquence qu'il faut nommer : modifier la signature sans toucher
//! au reste ne rompt pas la chaîne, cela rend seulement la signature invalide.
//! Les deux contrôles sont donc nécessaires, et `Journal::verifier_signatures`
//! les exerce tous les deux.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::erreur::{ErreurJournal, Resultat};

/// Taille d'une empreinte, en octets.
pub const TAILLE_EMPREINTE: usize = 32;

/// Taille d'une signature Ed25519, en octets.
pub const TAILLE_SIGNATURE: usize = 64;

/// Domaine de l'empreinte d'une entrée.
const DOMAINE_EMPREINTE: &[u8] = b"obrain-journal/v1/entree";
/// Domaine de l'empreinte d'un journal.
const DOMAINE_JOURNAL: &[u8] = b"obrain-journal/v1/journal";
/// Domaine de la genèse — le maillon qui précède la première entrée.
const DOMAINE_GENESE: &[u8] = b"obrain-journal/v1/genese";
/// Domaine de ce qui est signé.
const DOMAINE_SIGNATURE: &[u8] = b"obrain-journal/v1/signature";
/// Domaine du hachage d'une charge utile (« quoi »).
const DOMAINE_QUOI: &[u8] = b"obrain-journal/v1/quoi";

/// Une entrée du journal, qui est aussi le reçu du client.
///
/// Tous ses champs ont une taille fixe : il n'existe donc aucune ambiguïté de
/// concaténation dans l'empreinte, et deux entrées différentes ne peuvent pas
/// produire la même empreinte par décalage de frontière.
#[derive(Clone, PartialEq, Eq)]
pub struct Entree {
    /// Empreinte du journal auquel cette entrée appartient.
    pub journal: [u8; TAILLE_EMPREINTE],
    /// Rang de l'entrée, à partir de zéro, sans trou possible.
    pub sequence: u64,
    /// Horodatage en secondes depuis l'époque Unix, **déclaré par l'opérateur**.
    pub horodatage: u64,
    /// Empreinte de ce que l'entrée enregistre (voir [`empreinte_de`]).
    pub quoi: [u8; TAILLE_EMPREINTE],
    /// Empreinte de l'entrée précédente, ou la genèse pour la première.
    pub precedent: [u8; TAILLE_EMPREINTE],
    /// Signature de l'empreinte par la clé du journal.
    pub signature: [u8; TAILLE_SIGNATURE],
}

impl Entree {
    /// Construit une entrée. Réservé à la crate : une entrée non signée n'a pas
    /// de sens, et laisser l'appelant en fabriquer une serait laisser passer un
    /// reçu que personne ne peut vérifier.
    pub(crate) fn neuve(
        journal: [u8; TAILLE_EMPREINTE],
        sequence: u64,
        horodatage: u64,
        quoi: [u8; TAILLE_EMPREINTE],
        precedent: [u8; TAILLE_EMPREINTE],
        signature: [u8; TAILLE_SIGNATURE],
    ) -> Self {
        Self {
            journal,
            sequence,
            horodatage,
            quoi,
            precedent,
            signature,
        }
    }

    /// L'empreinte de l'entrée — le maillon que la suivante devra citer.
    #[must_use]
    pub fn empreinte(&self) -> [u8; TAILLE_EMPREINTE] {
        let mut h = Sha256::new();
        h.update(DOMAINE_EMPREINTE);
        h.update(self.journal);
        h.update(self.sequence.to_le_bytes());
        h.update(self.horodatage.to_le_bytes());
        h.update(self.quoi);
        h.update(self.precedent);
        h.finalize().into()
    }

    /// Vérifie que la signature est bien celle de la clé publique donnée, **et**
    /// qu'elle porte sur ce que cette entrée prétend être.
    ///
    /// Un client peut faire ce contrôle hors ligne, avec la seule clé publique :
    /// c'est ce qui rend le reçu opposable — l'opérateur ne peut pas le renier
    /// plus tard sans produire une signature qui ne correspond pas.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::SignatureInvalide`] si la signature ne vaut pas pour
    /// cette entrée et cette clé.
    pub fn verifier_signature(&self, publique: &[u8; 32]) -> Resultat<()> {
        let cle =
            VerifyingKey::from_bytes(publique).map_err(|_| ErreurJournal::SignatureInvalide {
                sequence: self.sequence,
            })?;
        let signature = Signature::from_bytes(&self.signature);
        cle.verify_strict(&message_signe(&self.empreinte()), &signature)
            .map_err(|_| ErreurJournal::SignatureInvalide {
                sequence: self.sequence,
            })
    }

    /// L'empreinte, en hexadécimal court : pour les messages, jamais pour
    /// comparer des octets.
    #[must_use]
    pub fn empreinte_courte(&self) -> String {
        court(&self.empreinte())
    }
}

impl core::fmt::Debug for Entree {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Entree")
            .field("journal", &court(&self.journal))
            .field("sequence", &self.sequence)
            .field("horodatage", &self.horodatage)
            .field("quoi", &court(&self.quoi))
            .field("precedent", &court(&self.precedent))
            .field("signature", &"•••")
            .finish()
    }
}

/// L'empreinte de ce qu'une entrée enregistre : une requête, un résultat, un
/// identifiant de calcul — ce que l'appelant veut rendre opposable.
///
/// L'appelant hache ce qu'il veut ; le journal ne conserve que l'empreinte, donc
/// **il n'apprend rien** de ce qu'il enregistre. C'est ce qui permet de tenir un
/// journal de calculs confidentiels : le journal prouve *qu'un* calcul a eu
/// lieu, et à quelle place, sans dire lequel.
#[must_use]
pub fn empreinte_de(charge: &[u8]) -> [u8; TAILLE_EMPREINTE] {
    let mut h = Sha256::new();
    h.update(DOMAINE_QUOI);
    h.update((charge.len() as u64).to_le_bytes());
    h.update(charge);
    h.finalize().into()
}

/// L'empreinte d'un journal, à partir de son identifiant.
#[must_use]
pub fn empreinte_du_journal(identifiant: &str) -> [u8; TAILLE_EMPREINTE] {
    let mut h = Sha256::new();
    h.update(DOMAINE_JOURNAL);
    h.update(identifiant.trim().as_bytes());
    h.finalize().into()
}

/// La genèse : le maillon qui précède la première entrée.
///
/// Elle dépend du journal, donc deux journaux différents ne peuvent pas
/// s'échanger leurs entrées — même à la séquence zéro.
#[must_use]
pub fn genese(journal: &[u8; TAILLE_EMPREINTE]) -> [u8; TAILLE_EMPREINTE] {
    let mut h = Sha256::new();
    h.update(DOMAINE_GENESE);
    h.update(journal);
    h.finalize().into()
}

/// Le message réellement signé : l'empreinte, préfixée du domaine.
///
/// Sans ce préfixe, une signature de journal pourrait être rejouée là où une
/// autre signature de 32 octets est attendue.
pub(crate) fn message_signe(empreinte: &[u8; TAILLE_EMPREINTE]) -> Vec<u8> {
    let mut message = Vec::with_capacity(DOMAINE_SIGNATURE.len() + TAILLE_EMPREINTE);
    message.extend_from_slice(DOMAINE_SIGNATURE);
    message.extend_from_slice(empreinte);
    message
}

/// Douze caractères hexadécimaux et une ellipsis : assez pour comparer deux
/// empreintes dans un message, trop peu pour en dire autre chose.
pub(crate) fn court(octets: &[u8]) -> String {
    let mut rendu = hex::encode(octets);
    rendu.truncate(12);
    rendu.push('…');
    rendu
}
