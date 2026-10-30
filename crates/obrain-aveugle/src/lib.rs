//! # obrain-aveugle — l'engine qui transporte sans pouvoir lire
//!
//! **Étape 3 de l'ADR `docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md`,
//! et cœur de la variante V-B** — celle que l'ADR *recommande* comme plancher
//! (§7) : « le self ne voit que des enveloppes opaques et des jetons de
//! capacité ; la compréhension vit dans le micro-harness de l'utilisateur ».
//!
//! ## Le critère de l'ADR, et il est falsifiable
//!
//! > **étape 3** — « l'engine reçoit une enveloppe de taille fixe et ne peut en
//! > extraire ni la requête ni la cible ».
//!
//! Ce n'est pas un vœu : c'est une propriété de **type**. [`Porteur`] ne détient
//! aucune [`CleEspace`], donc il ne peut pas appeler `Enveloppe::desceller` — il
//! n'existe aucun chemin, dans ce fichier, qui rende du clair à partir de ce
//! qu'il transporte. Tout le reste en découle, et
//! `tests/porte_aveugle.rs` le mesure : ce que le porteur *peut* dire de son
//! trafic ne contient aucun octet du contenu.
//!
//! ## Les trois pièces
//!
//! | Pièce | Ce qu'elle fait | Ce que le porteur en voit |
//! |---|---|---|
//! | [`Charge`] | la requête, le résultat, le jeton de capacité — **scellés** | rien |
//! | [`message::sceller`] | emballe la charge dans une enveloppe à taille fixe | une longueur de classe |
//! | [`Porteur`] | livre, compte, refuse les rejeux, journalise ses transitions | des adresses, des tailles, des instants |
//!
//! ## Ce que le porteur apprend malgré tout — et il faut le dire
//!
//! * **Qui parle à qui.** Les adresses sont nécessaires pour livrer. Elles sont
//!   tirées au sort ([`Adresse`]) et non dérivées d'un nom, donc le porteur ne
//!   sait pas *qui* elles désignent — mais il peut corréler les échanges d'une
//!   même adresse dans le temps.
//! * **Quand, et combien.** La fréquence et le volume d'un trafic restent
//!   visibles. Le bourrage à taille fixe masque les longueurs, pas les rythmes.
//! * **La classe de taille.** Le nombre de classes est fini (256 → 65 536), donc
//!   la longueur est devinée à la classe près — ce qui est exactement l'arbitrage
//!   que l'ADR demande pour G3.
//!
//! ## Ce que cette crate ne fait pas
//!
//! * **Elle n'empêche pas le détournement**, elle le rend **détectable** : le
//!   porteur tient les octets, donc il peut réétiqueter une enveloppe. La
//!   destination est scellée *dedans*, et [`message::ouvrir`] refuse un message
//!   qui n'était pas destiné à l'appelant ([`ErreurAveugle::Detourne`]).
//! * **Elle ne protège pas le calcul lui-même.** Le clair est chez celui qui
//!   détient la clé — c'est-à-dire dans le micro-harness de l'utilisateur, et
//!   c'est précisément le but de la variante V-B.
//! * **Elle ne force personne à l'employer.** Un engine peut continuer de servir
//!   des requêtes en clair ; cette crate fournit le chemin, elle ne l'impose pas.
//!   C'est le même travail restant que pour le TLS et l'audit chaîné : **les
//!   surfaces doivent adopter**.

#![forbid(unsafe_code)]

pub mod adresse;
pub mod message;
pub mod porteur;

pub use adresse::{Adresse, TAILLE_ADRESSE};
pub use message::{Charge, ouvrir, sceller, taille_conforme};
pub use porteur::{CeQueJaiVu, Porteur};

/// Résultat des opérations de la crate.
pub type Resultat<T> = core::result::Result<T, ErreurAveugle>;

/// Toutes les erreurs de `obrain-aveugle`.
#[derive(Debug, thiserror::Error)]
pub enum ErreurAveugle {
    /// Le système n'a pas rendu d'entropie pour une adresse.
    #[error("entropie indisponible : {0}")]
    Entropie(String),

    /// Les octets d'une charge ne sont pas ceux d'une charge.
    #[error("message illisible : {0}")]
    MessageIllisible(String),

    /// **Le message n'était pas destiné à celui qui l'a reçu.**
    ///
    /// C'est la trace d'un détournement : le porteur a livré ailleurs, ou
    /// réétiqueté une enveloppe. Il ne peut pas changer ce qui est scellé
    /// dedans, donc ce refus est la seule chose qu'il ne peut pas fabriquer.
    #[error("message détourné : destinataire scellé {scelle}, reçu par {attendu}")]
    Detourne {
        /// L'adresse qui a reçu le message.
        attendu: String,
        /// L'adresse scellée dans le message.
        scelle: String,
    },

    /// La charge dépasse la dernière classe d'enveloppe.
    #[error("charge de {taille} octets : au-delà de la dernière classe")]
    TropGrand {
        /// Taille demandée.
        taille: usize,
    },

    /// Le porteur a reçu des octets qui n'ont pas la taille d'une enveloppe du
    /// protocole.
    ///
    /// Accepter n'importe quelle longueur ferait du porteur un canal où la
    /// taille elle-même raconterait le contenu.
    #[error("dépôt de {taille} octets : aucune classe du protocole n'a cette taille")]
    TailleNonConforme {
        /// Taille reçue.
        taille: usize,
    },

    /// La même enveloppe a déjà été déposée.
    ///
    /// Sans ce refus, un porteur — ou un intermédiaire — pourrait faire exécuter
    /// deux fois un calcul en dupliquant ses octets.
    #[error("rejeu : cette enveloppe a déjà été déposée ({empreinte})")]
    Rejeu {
        /// Empreinte de l'enveloppe, en hexadécimal court.
        empreinte: String,
    },

    /// La boîte du destinataire est pleine.
    #[error("boîte de {destinataire} pleine ({quota} en attente) : le porteur n'est pas un amplificateur")]
    BoitePleine {
        /// Le destinataire dont la boîte est pleine.
        destinataire: String,
        /// Le quota atteint.
        quota: usize,
    },

    /// Le destinataire n'a jamais été enregistré auprès du porteur.
    #[error("destinataire inconnu du porteur : {0}")]
    DestinataireInconnu(String),

    /// Le coffre a refusé — mauvaise clé, altération, ou classe inconnue.
    #[error("coffre : {0}")]
    Coffre(#[from] obrain_coffre::CoffreError),

    /// Le registre des transitions a refusé.
    #[error("registre du porteur : {0}")]
    Journal(#[from] obrain_journal::ErreurJournal),
}
