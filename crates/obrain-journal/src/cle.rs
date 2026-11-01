//! La clé du journal — celle qui signe les reçus.
//!
//! ## Qui détient quoi, et pourquoi cela change tout
//!
//! L'opérateur détient la clé privée : c'est **son** journal, il l'écrit. Le
//! client ne détient que la clé publique, et les reçus qu'il a emportés.
//!
//! Il en découle une limite qu'il faut dire avant de promettre quoi que ce soit :
//! un opérateur qui détient la clé **peut** resigner une autre histoire. Ce qu'il
//! ne peut pas, c'est le faire sans que les reçus déjà détenus par ses clients
//! cessent de s'accorder avec son journal. La détection ne vient donc pas de la
//! signature seule : elle vient de la **rencontre** entre une signature valide et
//! un reçu que quelqu'un garde.
//!
//! C'est la raison pour laquelle la clé publique doit être **connue du client
//! avant** le calcul, par un canal qui ne passe pas par l'opérateur. Une clé
//! publique livrée avec le journal ne prouve rien : elle serait aussi réécrite.

use ed25519_dalek::{Signer, SigningKey};
use zeroize::Zeroize;

use crate::entree::{TAILLE_EMPREINTE, message_signe};
use crate::erreur::{ErreurJournal, Resultat};

/// La clé de signature d'un journal.
pub struct CleJournal(SigningKey);

impl CleJournal {
    /// Engendre une clé neuve depuis l'entropie du système.
    ///
    /// # Erreurs
    ///
    /// [`ErreurJournal::Entropie`] si le noyau n'a pas rendu d'octets — auquel
    /// cas aucune graine de repli n'est employée : une clé devinable rendrait
    /// toutes les signatures décoratives.
    pub fn engendrer() -> Resultat<Self> {
        let mut graine = [0u8; 32];
        getrandom::fill(&mut graine).map_err(|e| ErreurJournal::Entropie(e.to_string()))?;
        let cle = Self(SigningKey::from_bytes(&graine));
        graine.zeroize();
        Ok(cle)
    }

    /// Pose une clé depuis une graine de 32 octets déjà tirée au sort.
    #[must_use]
    pub fn depuis_octets(graine: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&graine))
    }

    /// La clé publique à communiquer aux clients — **avant** le premier calcul.
    #[must_use]
    pub fn publique(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    /// Signe une empreinte. Réservé à la crate : un reçu se fabrique par
    /// [`crate::Journal::recevoir`], jamais à la main.
    pub(crate) fn signer(&self, empreinte: &[u8; TAILLE_EMPREINTE]) -> [u8; 64] {
        self.0.sign(&message_signe(empreinte)).to_bytes()
    }

    /// Efface la clé immédiatement, sans attendre la destruction.
    ///
    /// L'ancienne valeur est **remplacée**, ce qui déclenche sa destruction —
    /// et la clé `ed25519-dalek` efface son secret en tombant.
    pub fn effacer(&mut self) {
        self.0 = SigningKey::from_bytes(&[0u8; 32]);
    }
}

impl Drop for CleJournal {
    fn drop(&mut self) {
        // La clé sous-jacente efface son propre secret ; ce `Drop` documente
        // l'intention et garantit qu'aucune variante future ne l'oublie.
        self.0 = SigningKey::from_bytes(&[0u8; 32]);
    }
}

impl core::fmt::Debug for CleJournal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CleJournal(publique={}, privée=•••)", hex::encode(&self.publique()[..6]))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn deux_cles_engendrees_different() {
        let a = CleJournal::engendrer().unwrap();
        let b = CleJournal::engendrer().unwrap();
        assert_ne!(a.publique(), b.publique());
    }

    #[test]
    fn la_meme_graine_donne_la_meme_cle() {
        let a = CleJournal::depuis_octets([7u8; 32]);
        let b = CleJournal::depuis_octets([7u8; 32]);
        assert_eq!(a.publique(), b.publique());
    }

    /// Le `Debug` ne doit jamais rendre le secret : un journal de démarrage
    /// n'est pas un coffre.
    #[test]
    fn le_debug_ne_rend_pas_la_cle_privee() {
        let cle = CleJournal::depuis_octets([9u8; 32]);
        let rendu = format!("{cle:?}");
        assert!(rendu.contains("•••"), "{rendu}");
        assert!(!rendu.contains("09090909"), "{rendu}");
    }
}
