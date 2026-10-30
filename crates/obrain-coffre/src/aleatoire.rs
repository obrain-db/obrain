//! L'entropie — la seule chose de cette crate qui ne se négocie pas.
//!
//! Toute la garantie repose sur des nonces et des clés tirés au sort. Une
//! source d'entropie prévisible (horloge, compteur, générateur ensemencé) ne
//! « affaiblit » pas la garantie : elle l'annule. On passe donc directement par
//! le système d'exploitation, sans générateur intermédiaire à ensemencer, et
//! l'échec est une erreur remontée — jamais un repli silencieux.

use crate::erreur::{CoffreError, Resultat};

/// Remplit `tampon` d'octets tirés du système d'exploitation.
///
/// # Erreurs
///
/// [`CoffreError::Entropie`] si le noyau n'a pas rendu d'octets — auquel cas
/// **aucune** valeur de repli n'est employée : mieux vaut refuser que sceller
/// sous une clé devinable.
pub fn remplir(tampon: &mut [u8]) -> Resultat<()> {
    getrandom::fill(tampon).map_err(|e| CoffreError::Entropie(e.to_string()))
}

/// Rend `N` octets tirés du système d'exploitation.
///
/// # Erreurs
///
/// [`CoffreError::Entropie`], comme [`remplir`].
pub fn octets<const N: usize>() -> Resultat<[u8; N]> {
    let mut tampon = [0u8; N];
    remplir(&mut tampon)?;
    Ok(tampon)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Deux tirages ne sont jamais égaux. C'est faible comme test (une
    /// coïncidence de 2⁻¹²⁸ passerait pour un bug), mais il attrape le cas
    /// réellement dangereux : un tampon laissé à zéro.
    #[test]
    fn deux_tirages_differents() {
        let a: [u8; 32] = octets().unwrap();
        let b: [u8; 32] = octets().unwrap();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
    }

    /// Le remplissage d'un tampon de taille quelconque fonctionne.
    #[test]
    fn remplissage_de_taille_arbitraire() {
        let mut tampon = vec![0u8; 4096];
        remplir(&mut tampon).unwrap();
        assert!(tampon.iter().any(|o| *o != 0));
    }
}
