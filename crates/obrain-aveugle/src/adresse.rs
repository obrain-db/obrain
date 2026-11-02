//! L'adresse d'une extrémité — et pourquoi elle n'est **pas** un nom.
//!
//! Le porteur doit savoir où livrer. C'est la seule chose qu'il a besoin de
//! savoir, et c'est déjà une métadonnée : « A parle à B » est une information,
//! même sans un octet de contenu. Deux choix s'offrent, et un seul tient :
//!
//! * **dériver l'adresse du nom** (« le harness d'Alice ») : le porteur apprend
//!   alors *qui* parle à *qui*, et le graphe social se reconstitue à
//!   l'observation — sans jamais déchiffrer quoi que ce soit ;
//! * **tirer l'adresse au sort**, et l'échanger hors bande : le porteur voit
//!   deux jetons opaques qui se parlent.
//!
//! C'est le second qui est implémenté. Ce que cela **ne** fait pas : les
//! adresses restent stables, donc un observateur patient peut corréler les
//! échanges d'une même adresse dans le temps. Le porteur, lui, ne sait pas à qui
//! elles appartiennent — c'est toute la différence entre surveiller un trafic et
//! connaître des gens.

/// Longueur d'une adresse, en octets.
pub const TAILLE_ADRESSE: usize = 16;

/// L'adresse d'une extrémité : 16 octets tirés au sort, échangés hors bande.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Adresse([u8; TAILLE_ADRESSE]);

impl Adresse {
    /// Tire une adresse neuve depuis l'entropie du système.
    ///
    /// # Erreurs
    ///
    /// [`ErreurAveugle::Entropie`](crate::ErreurAveugle::Entropie) si le noyau
    /// n'a pas rendu d'octets — aucune adresse de repli n'est employée, une
    /// adresse devinable n'étant plus une adresse.
    pub fn tirer() -> crate::Resultat<Self> {
        let mut octets = [0u8; TAILLE_ADRESSE];
        getrandom::fill(&mut octets).map_err(|e| crate::ErreurAveugle::Entropie(e.to_string()))?;
        Ok(Self(octets))
    }

    /// Reconstruit une adresse depuis ses octets.
    #[must_use]
    pub const fn depuis_octets(octets: [u8; TAILLE_ADRESSE]) -> Self {
        Self(octets)
    }

    /// Les octets de l'adresse.
    #[must_use]
    pub const fn octets(&self) -> &[u8; TAILLE_ADRESSE] {
        &self.0
    }
}

impl core::fmt::Debug for Adresse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Douze caractères suffisent à distinguer deux adresses dans un journal,
        // et n'impriment pas l'adresse entière dans les traces.
        let mut rendu = hex::encode(self.0);
        rendu.truncate(12);
        write!(f, "Adresse({rendu}…)")
    }
}
