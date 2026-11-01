//! Le jeton de capacité — déléguer une **autorisation**, jamais une donnée.
//!
//! ## Le principe
//!
//! Un jeton dit : « l'action `graph:lire` sur l'espace *e1* est autorisée
//! jusqu'à telle seconde ». Il ne dit rien d'autre — en particulier, il ne
//! porte **aucune donnée**. C'est la monnaie d'action d'un self aveugle : il
//! route un jeton opaque, il ne le comprend pas.
//!
//! ## Deux propriétés, et leur raison
//!
//! 1. **Le nom de l'action ne se lit pas dans le jeton.** Il est *dans* le
//!    chiffré. Le porteur — l'engine — apprend qu'il transporte une capacité,
//!    sa classe de taille, et rien de ce qu'elle autorise.
//! 2. **Taille constante** : un jeton fait toujours 130 octets, quelle que soit
//!    la longueur du nom d'action. L'entropie du nom d'action ne fuit donc pas
//!    par la taille.
//!
//! ## Ce que le jeton ne protège pas
//!
//! * **La relecture.** Un jeton est porteur : qui le copie peut s'en servir
//!   jusqu'à son échéance. Il n'y a pas d'état côté vérificateur dans cette
//!   crate — un anti-rejeu demande un magasin de nonces consommés, à construire
//!   par l'appelant s'il en a besoin.
//! * **L'horloge.** `maintenant` est fourni par l'appelant, et l'appelant
//!   tourne chez l'opérateur. Un opérateur qui recule son horloge fait revivre
//!   un jeton expiré. La parade est une horloge monotone **côté client**, pas
//!   une ligne de cette crate. Le dire ici vaut mieux que de le laisser croire.
//! * **La vérification sans la clé.** Vérifier exige la clé d'espace. L'engine
//!   qui n'a que le jeton ne peut **ni le vérifier ni le forger** : c'est
//!   délibéré, et c'est ce qui rend le jeton inexploitable par lui.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use zeroize::Zeroizing;

use crate::aleatoire;
use crate::cle::{
    CleEspace, EmpreinteEspace, Usage, LONGUEUR_EMPREINTE, LONGUEUR_ETIQUETTE, LONGUEUR_NONCE,
};
use crate::erreur::{CoffreError, Resultat};

/// Taille de l'en-tête d'un jeton, en octets.
pub const TETE_JETON: usize = 38;

/// Taille du clair d'un jeton, en octets.
pub const CLAIR_JETON: usize = 64;

/// Taille fixe d'un jeton, en octets.
pub const TAILLE_JETON: usize = TETE_JETON + LONGUEUR_NONCE + CLAIR_JETON + LONGUEUR_ETIQUETTE;

/// Longueur maximale d'un nom d'action, en octets.
pub const ACTION_MAXIMUM: usize = 47;

const MAGIC_JETON: &[u8; 4] = b"OBJT";
const VERSION_JETON: u8 = 1;

/// Ce qu'un jeton autorise : un espace, une action, une échéance.
///
/// L'espace est désigné par son **empreinte** : le jeton ne transporte jamais
/// le nom de l'espace, seulement de quoi le reconnaître.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Portee {
    /// L'espace sur lequel porte l'autorisation.
    pub espace: EmpreinteEspace,
    /// L'action autorisée — `graph:read`, `graph:write`, `iam:audit`…
    pub action: String,
    /// L'échéance, en secondes depuis l'époque Unix.
    pub expiration: u64,
}

impl Portee {
    /// Construit une portée.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si l'action est plus longue que
    /// [`ACTION_MAXIMUM`] — refusée, jamais tronquée : une action tronquée
    /// autoriserait autre chose que ce qui a été demandé.
    pub fn nouvelle(espace: EmpreinteEspace, action: &str, expiration: u64) -> Resultat<Self> {
        if action.is_empty() {
            return Err(CoffreError::Format(
                "action vide : un jeton doit autoriser quelque chose de nommé".to_owned(),
            ));
        }
        if action.len() > ACTION_MAXIMUM {
            return Err(CoffreError::Format(format!(
                "action de {} octets, maximum {ACTION_MAXIMUM}",
                action.len()
            )));
        }
        Ok(Self {
            espace,
            action: action.to_owned(),
            expiration,
        })
    }

    /// Vrai si l'échéance n'est pas atteinte à l'instant donné.
    #[must_use]
    pub const fn valide_a(&self, maintenant: u64) -> bool {
        maintenant < self.expiration
    }
}

/// Le jeton d'un espace : émet et vérifie les capacités.
///
/// Se construit **uniquement** à partir d'une [`CleEspace`]. Sans la clé, il
/// n'existe pas d'objet capable de vérifier un jeton — et donc pas de chemin
/// pour en fabriquer un.
pub struct Jeton {
    sous_cle: Zeroizing<[u8; 32]>,
    espace: EmpreinteEspace,
}

impl Jeton {
    /// Ouvre les jetons d'un espace.
    ///
    /// # Erreurs
    ///
    /// [`CoffreError::Format`] si HKDF échoue — impossible en pratique.
    pub fn neuf(cle: &CleEspace) -> Resultat<Self> {
        Ok(Self {
            sous_cle: cle.sous_cle(Usage::Jeton)?,
            espace: cle.espace(),
        })
    }

    /// L'empreinte de l'espace de ces jetons.
    #[must_use]
    pub const fn espace(&self) -> EmpreinteEspace {
        self.espace
    }

    /// Émet un jeton pour une action, valable jusqu'à `expiration`.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::Format`] si l'action est vide ou trop longue ;
    /// * [`CoffreError::Entropie`] si le nonce ne peut être tiré ;
    /// * [`CoffreError::Format`] si l'AEAD refuse de sceller — impossible.
    pub fn emettre(&self, action: &str, expiration: u64) -> Resultat<Vec<u8>> {
        let portee = Portee::nouvelle(self.espace, action, expiration)?;

        let mut clair = Zeroizing::new([0u8; CLAIR_JETON]);
        clair[0] = portee.action.len() as u8;
        clair[1..=portee.action.len()].copy_from_slice(portee.action.as_bytes());
        clair[48..56].copy_from_slice(&portee.expiration.to_le_bytes());
        // clair[56..64] : réservé, laissé à zéro et vérifié tel au déscellement.

        let en_tete = self.en_tete();
        let nonce: [u8; LONGUEUR_NONCE] = aleatoire::octets()?;
        let chiffre = sceller(&self.sous_cle, &nonce, &en_tete, &clair[..])?;

        let mut sortie = Vec::with_capacity(TAILLE_JETON);
        sortie.extend_from_slice(&en_tete);
        sortie.extend_from_slice(&nonce);
        sortie.extend_from_slice(&chiffre);
        Ok(sortie)
    }

    /// Vérifie un jeton et rend ce qu'il autorise.
    ///
    /// L'ordre des contrôles est délibéré : **authentifier d'abord**, puis
    /// juger. Un jeton non authentique ne doit jamais produire un message qui
    /// parle de son contenu.
    ///
    /// # Erreurs
    ///
    /// * [`CoffreError::Format`] si la structure est incohérente ;
    /// * [`CoffreError::EspaceIncoherent`] si le jeton vise un autre espace ;
    /// * [`CoffreError::Authentification`] si le jeton a été modifié ou forgé ;
    /// * [`CoffreError::JetonExpire`] si l'échéance est atteinte ;
    /// * [`CoffreError::ActionHorsPortee`] si l'action demandée n'est pas celle
    ///   du jeton — `Allow` sur une action n'est pas `Allow` sur une autre.
    pub fn verifier(
        &self,
        octets: &[u8],
        action_demandee: &str,
        maintenant: u64,
    ) -> Resultat<Portee> {
        let portee = self.lire(octets)?;

        if !portee.valide_a(maintenant) {
            return Err(CoffreError::JetonExpire {
                echeance: portee.expiration,
                maintenant,
            });
        }
        if portee.action != action_demandee {
            return Err(CoffreError::ActionHorsPortee {
                portee: portee.action,
                demandee: action_demandee.to_owned(),
            });
        }
        Ok(portee)
    }

    /// Authentifie un jeton et rend sa portée, **sans juger** l'échéance ni
    /// l'action.
    ///
    /// Sert à qui veut inspecter une portée légitimement (journal d'audit côté
    /// client) avant de décider.
    ///
    /// # Erreurs
    ///
    /// Comme [`Jeton::verifier`], sans les deux jugements.
    pub fn lire(&self, octets: &[u8]) -> Resultat<Portee> {
        let en_tete = lire_tete(octets)?;

        let mut empreinte = [0u8; LONGUEUR_EMPREINTE];
        empreinte.copy_from_slice(&en_tete[6..TETE_JETON]);
        let espace_jeton = EmpreinteEspace::depuis_octets(empreinte);
        if espace_jeton != self.espace {
            return Err(CoffreError::EspaceIncoherent {
                attendu: self.espace,
                scelle: espace_jeton,
            });
        }

        let nonce: [u8; LONGUEUR_NONCE] = octets[TETE_JETON..TETE_JETON + LONGUEUR_NONCE]
            .try_into()
            .map_err(|_| CoffreError::Format("nonce de jeton illisible".to_owned()))?;
        let clair = ouvrir(
            &self.sous_cle,
            &nonce,
            &en_tete,
            &octets[TETE_JETON + LONGUEUR_NONCE..],
        )?;

        portee_depuis_clair(&clair, espace_jeton)
    }

    /// Construit l'en-tête en clair d'un jeton.
    fn en_tete(&self) -> [u8; TETE_JETON] {
        let mut en_tete = [0u8; TETE_JETON];
        en_tete[0..4].copy_from_slice(MAGIC_JETON);
        en_tete[4] = VERSION_JETON;
        en_tete[5] = 0;
        en_tete[6..TETE_JETON].copy_from_slice(self.espace.octets());
        en_tete
    }
}

impl core::fmt::Debug for Jeton {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Jeton(espace={}, •••)", self.espace)
    }
}

fn lire_tete(octets: &[u8]) -> Resultat<[u8; TETE_JETON]> {
    if octets.len() != TAILLE_JETON {
        return Err(CoffreError::Format(format!(
            "jeton de {} octets, {TAILLE_JETON} attendus (taille fixe)",
            octets.len()
        )));
    }
    if &octets[0..4] != MAGIC_JETON {
        return Err(CoffreError::Format("magic de jeton inconnu".to_owned()));
    }
    if octets[4] != VERSION_JETON {
        return Err(CoffreError::Format(format!(
            "version de jeton {} inconnue (cette build lit la {VERSION_JETON})",
            octets[4]
        )));
    }
    if octets[5] != 0 {
        return Err(CoffreError::Format(format!(
            "drapeau de jeton inconnu : {} (aucun drapeau n'est défini)",
            octets[5]
        )));
    }
    let mut en_tete = [0u8; TETE_JETON];
    en_tete.copy_from_slice(&octets[0..TETE_JETON]);
    Ok(en_tete)
}

/// Relit la portée depuis le clair d'un jeton, en vérifiant la zone réservée.
///
/// L'empreinte d'espace est passée par l'appelant : elle vient d'être
/// authentifiée par l'AEAD de l'en-tête, et c'est elle qui fait foi — le clair
/// ne la répète pas, ce qui évite d'avoir deux endroits à croire.
fn portee_depuis_clair(clair: &[u8], espace: EmpreinteEspace) -> Resultat<Portee> {
    if clair.len() != CLAIR_JETON {
        return Err(CoffreError::Format(format!(
            "clair de jeton de {} octets, {CLAIR_JETON} attendus",
            clair.len()
        )));
    }
    let longueur = clair[0] as usize;
    if longueur == 0 || longueur > ACTION_MAXIMUM {
        return Err(CoffreError::Format(format!(
            "longueur d'action inscrite invalide : {longueur}"
        )));
    }
    if clair[1 + longueur..48].iter().any(|o| *o != 0) {
        return Err(CoffreError::Format(
            "remplissage d'action non nul dans un jeton".to_owned(),
        ));
    }
    if clair[56..64].iter().any(|o| *o != 0) {
        return Err(CoffreError::Format(
            "zone réservée non nulle dans un jeton".to_owned(),
        ));
    }

    let action = String::from_utf8(clair[1..=longueur].to_vec())
        .map_err(|_| CoffreError::Format("action de jeton non UTF-8".to_owned()))?;
    let mut expiration = [0u8; 8];
    expiration.copy_from_slice(&clair[48..56]);

    Ok(Portee {
        espace,
        action,
        expiration: u64::from_le_bytes(expiration),
    })
}

fn sceller(
    sous_cle: &[u8; 32],
    nonce: &[u8; LONGUEUR_NONCE],
    aad: &[u8],
    clair: &[u8],
) -> Resultat<Vec<u8>> {
    let chiffreur = Aes256Gcm::new_from_slice(sous_cle)
        .map_err(|_| CoffreError::Format("sous-clé AES-256 de taille invalide".to_owned()))?;
    chiffreur
        .encrypt(Nonce::from_slice(nonce), Payload { msg: clair, aad })
        .map_err(|_| CoffreError::Format("scellement AEAD refusé".to_owned()))
}

fn ouvrir(
    sous_cle: &[u8; 32],
    nonce: &[u8; LONGUEUR_NONCE],
    aad: &[u8],
    chiffre: &[u8],
) -> Resultat<Zeroizing<Vec<u8>>> {
    let chiffreur = Aes256Gcm::new_from_slice(sous_cle)
        .map_err(|_| CoffreError::Format("sous-clé AES-256 de taille invalide".to_owned()))?;
    let clair = chiffreur
        .decrypt(Nonce::from_slice(nonce), Payload { msg: chiffre, aad })
        .map_err(|_| CoffreError::Authentification)?;
    Ok(Zeroizing::new(clair))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::cle::contient;

    fn cle(espace: &str) -> CleEspace {
        CleEspace::engendrer(espace).unwrap()
    }

    fn jeton(espace: &str) -> Jeton {
        Jeton::neuf(&cle(espace)).unwrap()
    }

    /// Émettre puis vérifier, avec la bonne action et la bonne heure.
    #[test]
    fn emission_et_verification() {
        let j = jeton("e1");
        let octets = j.emettre("graph:read", 1_000).unwrap();
        assert_eq!(octets.len(), TAILLE_JETON);

        let portee = j.verifier(&octets, "graph:read", 999).unwrap();
        assert_eq!(portee.action, "graph:read");
        assert_eq!(portee.expiration, 1_000);
        assert_eq!(portee.espace, j.espace());
    }

    /// **Une action autorisée n'en autorise pas une autre.** C'est la règle que
    /// l'IAM payait déjà cher à ne pas tenir.
    #[test]
    fn action_hors_portee_refusee() {
        let j = jeton("e1");
        let octets = j.emettre("graph:read", 1_000).unwrap();
        match j.verifier(&octets, "graph:write", 999) {
            Err(CoffreError::ActionHorsPortee { portee, demandee }) => {
                assert_eq!(portee, "graph:read");
                assert_eq!(demandee, "graph:write");
            }
            autre => panic!("attendu ActionHorsPortee, reçu {autre:?}"),
        }
    }

    /// Un jeton expiré est refusé, et l'erreur porte les deux instants.
    #[test]
    fn jeton_expire_refuse() {
        let j = jeton("e1");
        let octets = j.emettre("graph:read", 1_000).unwrap();

        assert!(j.verifier(&octets, "graph:read", 999).is_ok());
        assert!(j.verifier(&octets, "graph:read", 1_000).is_err(), "l'échéance est exclue");
        match j.verifier(&octets, "graph:read", 1_001) {
            Err(CoffreError::JetonExpire {
                echeance,
                maintenant,
            }) => {
                assert_eq!((echeance, maintenant), (1_000, 1_001));
            }
            autre => panic!("attendu JetonExpire, reçu {autre:?}"),
        }
    }

    /// **Un jeton d'un espace ne sert pas dans un autre** — même si le nom
    /// d'action est identique.
    #[test]
    fn jeton_d_un_autre_espace_refuse() {
        let octets = jeton("e1").emettre("graph:read", 1_000).unwrap();
        let mut faux = octets.clone();
        faux[6..TETE_JETON].copy_from_slice(EmpreinteEspace::calculer("e2").octets());

        assert!(matches!(
            jeton("e1").verifier(&faux, "graph:read", 999),
            Err(CoffreError::EspaceIncoherent { .. })
        ));
    }

    /// **La forge sans la clé échoue** : un jeton émis par une autre clé du même
    /// espace n'est pas reconnu.
    #[test]
    fn forge_sans_la_cle_refusee() {
        let faux = jeton("e1").emettre("graph:read", 1_000).unwrap();
        assert!(matches!(
            jeton("e1").verifier(&faux, "graph:read", 999),
            Err(CoffreError::Authentification)
        ));
    }

    /// Toute zone du jeton est couverte par l'AEAD.
    #[test]
    fn toute_zone_est_couverte() {
        let j = jeton("e1");
        let bon = j.emettre("graph:read", 1_000).unwrap();

        for (nom, position) in [
            ("magic", 0usize),
            ("version", 4),
            ("drapeau", 5),
            ("empreinte d'espace", 20),
            ("nonce", TETE_JETON),
            ("premier octet de chiffré", TETE_JETON + LONGUEUR_NONCE),
            ("dernier octet", bon.len() - 1),
        ] {
            let mut faux = bon.clone();
            faux[position] ^= 0x01;
            assert!(
                j.verifier(&faux, "graph:read", 999).is_err(),
                "modifier « {nom} » doit être détecté"
            );
        }
    }

    /// **Le nom de l'action ne se lit pas dans le jeton** — le porteur transporte
    /// une capacité sans savoir ce qu'elle autorise.
    #[test]
    fn l_action_ne_se_lit_pas() {
        let octets = jeton("e1")
            .emettre("iam:admin:espace-secret", 1_000)
            .unwrap();
        assert!(!contient(&octets, b"iam:admin"));
        assert!(!contient(&octets, b"espace-secret"));
        assert!(!contient(&octets, b"admin"));
    }

    /// **Taille constante** : la longueur du nom d'action ne fuit pas.
    #[test]
    fn taille_constante_quelle_que_soit_l_action() {
        let j = jeton("e1");
        let court = j.emettre("r", 1_000).unwrap();
        let long = j.emettre(&"a".repeat(ACTION_MAXIMUM), 1_000).unwrap();
        assert_eq!(court.len(), TAILLE_JETON);
        assert_eq!(long.len(), TAILLE_JETON);
    }

    /// Une action vide ou trop longue est refusée, jamais tronquée.
    #[test]
    fn action_invalide_refusee() {
        let j = jeton("e1");
        assert!(matches!(j.emettre("", 1_000), Err(CoffreError::Format(_))));
        assert!(matches!(
            j.emettre(&"a".repeat(ACTION_MAXIMUM + 1), 1_000),
            Err(CoffreError::Format(_))
        ));
    }

    /// Un jeton tronqué ou rallongé est refusé : la taille est fixe.
    #[test]
    fn taille_stricte() {
        let j = jeton("e1");
        let bon = j.emettre("graph:read", 1_000).unwrap();
        assert!(j.lire(&bon[..bon.len() - 1]).is_err());
        let mut rallonge = bon.clone();
        rallonge.push(0);
        assert!(j.lire(&rallonge).is_err());
    }

    /// **Séparation des domaines** : un jeton ne passe pas pour une enveloppe,
    /// ni pour un bloc de coffre.
    #[test]
    fn un_jeton_n_est_pas_une_enveloppe() {
        let k = cle("e1");
        let octets = Jeton::neuf(&k).unwrap().emettre("graph:read", 1_000).unwrap();

        let enveloppe = crate::Enveloppe::neuf(&k).unwrap();
        assert!(
            enveloppe.desceller(&octets).is_err(),
            "le magic diffère : rien ne doit passer d'un usage à l'autre"
        );

        let coffre = crate::Coffre::neuf(&k).unwrap();
        assert!(coffre.desceller(&octets).is_err());
    }

    /// Les sous-clés sont bien distinctes : le même octet de départ ne donne pas
    /// la même clé selon l'usage.
    #[test]
    fn sous_cles_distinctes_par_usage() {
        let k = cle("e1");
        assert_ne!(
            k.sous_cle(Usage::Jeton).unwrap()[..],
            k.sous_cle(Usage::Enveloppe).unwrap()[..]
        );
        assert_ne!(
            k.sous_cle(Usage::Coffre).unwrap()[..],
            k.sous_cle(Usage::EnteteCoffre).unwrap()[..]
        );
    }

    /// Une portée construite refuse les actions limites.
    #[test]
    fn portee_bornee() {
        let espace = EmpreinteEspace::calculer("e1");
        assert!(Portee::nouvelle(espace, "a", 10).is_ok());
        assert!(Portee::nouvelle(espace, &"a".repeat(ACTION_MAXIMUM), 10).is_ok());
        assert!(Portee::nouvelle(espace, "", 10).is_err());
        assert!(Portee::nouvelle(espace, &"a".repeat(ACTION_MAXIMUM + 1), 10).is_err());
        assert!(Portee::nouvelle(espace, "a", 10).unwrap().valide_a(9));
        assert!(!Portee::nouvelle(espace, "a", 10).unwrap().valide_a(10));
    }

    /// La zone réservée et le remplissage d'action ne peuvent pas transporter
    /// d'information cachée.
    #[test]
    fn clair_de_jeton_canonique() {
        let espace = EmpreinteEspace::calculer("e1");
        let mut clair = vec![0u8; CLAIR_JETON];
        clair[0] = 3;
        clair[1..4].copy_from_slice(b"abc");
        clair[48..56].copy_from_slice(&42u64.to_le_bytes());
        assert_eq!(portee_depuis_clair(&clair, espace).unwrap().action, "abc");

        let mut remplissage = clair.clone();
        remplissage[10] = 1;
        assert!(portee_depuis_clair(&remplissage, espace).is_err());

        let mut reserve = clair;
        reserve[60] = 1;
        assert!(portee_depuis_clair(&reserve, espace).is_err());
    }
}
