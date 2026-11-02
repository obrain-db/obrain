//! Les erreurs — et la distinction qui compte plus que toutes les autres.
//!
//! Ce module sépare deux choses que le reste du dépôt confond parfois :
//!
//! * [`CoffreError::CleAbsente`] — **une impossibilité**. Il n'y a pas de clé,
//!   donc il n'y a rien à déchiffrer. Aucune décision, aucun verdict, aucun
//!   administrateur ne peut la lever.
//! * [`CoffreError::AutorisationRefusee`] — **une décision**. Quelqu'un a jugé.
//!   Un jugement peut être faux ; c'est précisément ce qu'a coûté
//!   `delete_policy` dans ce dépôt (voir `docs/ADMINISTRATION-IAM-2026-09-13.md`).
//!
//! Confondre les deux dans un message d'erreur, c'est vendre une impossibilité
//! là où on livre une décision. Les variantes restent donc distinctes jusqu'à
//! l'appelant.

use crate::cle::EmpreinteEspace;

/// Résultat des opérations de la crate.
pub type Resultat<T> = core::result::Result<T, CoffreError>;

/// Toutes les erreurs de `obrain-coffre`.
///
/// Chaque variante nomme ce qui a été constaté, jamais ce qu'il faudrait en
/// déduire : c'est à l'appelant de décider s'il annonce un refus ou une
/// impossibilité.
#[derive(Debug, thiserror::Error)]
pub enum CoffreError {
    /// **Impossibilité** : la clé n'est pas là. Aucun droit ne la remplace.
    #[error("clé absente — il n'y a rien à déchiffrer : {0}")]
    CleAbsente(String),

    /// **Décision** : une autorisation a été jugée et refusée (jeton expiré,
    /// action hors portée). Un jugement, donc — pas une impossibilité.
    #[error("autorisation refusée : {0}")]
    AutorisationRefusee(String),

    /// L'AEAD a rejeté : l'étiquette ne correspond pas à ce qui a été reçu.
    ///
    /// Le message ne dit **jamais** où l'altération se trouve — un oracle
    /// bavard est une fuite.
    #[error("authentification refusée — les octets reçus ne sont pas ceux qui ont été scellés")]
    Authentification,

    /// Les octets ne respectent pas le format (magic, version, longueurs
    /// incohérentes entre elles).
    #[error("format invalide : {0}")]
    Format(String),

    /// Une clé a été employée pour un espace dont l'empreinte ne correspond pas
    /// à celle scellée. Copier une clé ne copie donc pas l'accès.
    #[error("espace incohérent : attendu {attendu}, scellé {scelle}")]
    EspaceIncoherent {
        /// Empreinte de l'espace de la clé employée.
        attendu: EmpreinteEspace,
        /// Empreinte de l'espace réellement scellé dans les octets reçus.
        scelle: EmpreinteEspace,
    },

    /// Les paramètres de dérivation lus sur le disque sont **sous le plancher**.
    ///
    /// C'est une attaque par abaissement : l'attaquant ne casse pas la clé, il
    /// réécrit l'en-tête pour que la victime en dérive une faible. Le refus a
    /// lieu **avant** toute dérivation.
    #[error("paramètres de dérivation sous le plancher ({0}) — refusés avant dérivation")]
    KdfTropFaible(String),

    /// La charge utile dépasse la plus grande classe d'enveloppe.
    #[error("charge trop grande : {taille} octets, maximum {maximum}")]
    TropGrand {
        /// Taille demandée.
        taille: usize,
        /// Capacité de la plus grande classe.
        maximum: usize,
    },

    /// L'identifiant du coffre reçu n'est pas celui attendu.
    ///
    /// Se produit quand un porteur substitue un coffre **entier** — bien formé,
    /// authentique, scellé par la même clé — à celui qui était attendu. Sans
    /// identifiant attendu, cette substitution passerait pour une lecture
    /// normale : c'est pourquoi `desceller` seul ne peut pas la voir, et pourquoi
    /// [`crate::Coffre::desceller_avec_id`] existe.
    #[error("coffre inconnu : attendu {attendu}, reçu {lu}")]
    CoffreInconnu {
        /// Identifiant attendu par l'appelant.
        attendu: String,
        /// Identifiant lu dans le coffre reçu, **après** authentification.
        lu: String,
    },

    /// La clé maîtresse n'a pas produit la clé attendue.
    #[error("la clé maîtresse ne descelle pas cette enveloppe")]
    CleMaitresseIncorrecte,

    /// Le jeton est bien authentique, mais son échéance est passée.
    #[error("jeton expiré (échéance {echeance}, maintenant {maintenant})")]
    JetonExpire {
        /// Échéance inscrite dans le jeton (secondes Unix).
        echeance: u64,
        /// Instant de la vérification (secondes Unix).
        maintenant: u64,
    },

    /// Le jeton est authentique et valide, mais ne couvre pas l'action
    /// demandée : `Allow` sur une action n'est pas `Allow` sur une autre.
    #[error("action hors portée : le jeton autorise « {portee} », « {demandee} » a été demandée")]
    ActionHorsPortee {
        /// Action inscrite dans le jeton.
        portee: String,
        /// Action demandée par l'appelant.
        demandee: String,
    },

    /// Le système d'exploitation n'a pas rendu d'entropie.
    #[error("entropie indisponible : {0}")]
    Entropie(String),

    /// Un fichier de clé n'a pas pu être écrit ou lu.
    #[error("dépôt de clé : {0}")]
    Depot(String),

    /// Le fichier de clé est lisible par d'autres que son propriétaire.
    ///
    /// Refuser de l'utiliser est un choix : une clé que le voisin peut lire
    /// n'est pas une clé, et continuer donnerait à l'utilisateur l'illusion
    /// d'une garantie tenue.
    #[error(
        "fichier de clé trop permissif : {chemin} est lisible au-delà du propriétaire (mode {mode:o})"
    )]
    PermissionsTropLarges {
        /// Chemin du fichier refusé.
        chemin: String,
        /// Mode constaté.
        mode: u32,
    },
}

impl CoffreError {
    /// Vrai si l'erreur traduit une **impossibilité** plutôt qu'une décision.
    ///
    /// Sert aux surfaces qui doivent annoncer honnêtement laquelle des deux
    /// elles opposent à l'utilisateur : « il n'y a pas de clé » n'est pas
    /// « vous n'avez pas le droit ».
    #[must_use]
    pub fn est_une_impossibilite(&self) -> bool {
        matches!(self, Self::CleAbsente(_))
    }
}
