//! Les erreurs — toutes **fail-closed**, aucune sans issue.
//!
//! Il n'existe ici aucune variante qui signifie « continuer en clair ». C'est
//! délibéré : le mode de défaillance dangereux d'une couche TLS n'est pas de
//! refuser de démarrer, c'est de démarrer quand même — certificat introuvable,
//! clé illisible, poignée de main ratée, et le serveur se rabat silencieusement
//! sur du texte en clair parce qu'il « fonctionne ». Un serveur qui refuse de
//! démarrer se voit ; un serveur qui sert en clair ne se voit pas.
//!
//! Chaque variante nomme ce qui a échoué **et à quel étage** : la configuration
//! (avant toute écoute) ou la session (après).

use std::time::Duration;

/// Résultat des opérations de la crate.
pub type Resultat<T> = core::result::Result<T, ErreurTls>;

/// Toutes les erreurs de `obrain-tls`.
#[derive(Debug, thiserror::Error)]
pub enum ErreurTls {
    /// Le fichier de certificat n'a pas pu être lu.
    ///
    /// Erreur de **configuration** : elle survient avant toute écoute, donc
    /// avant qu'une seule connexion ait pu être servie en clair.
    #[error("certificat illisible : {chemin} ({detail})")]
    CertificatAbsent {
        /// Chemin demandé.
        chemin: String,
        /// Motif rendu par le système.
        detail: String,
    },

    /// Le fichier de clé privée n'a pas pu être lu.
    #[error("clé privée illisible : {chemin} ({detail})")]
    CleAbsente {
        /// Chemin demandé.
        chemin: String,
        /// Motif rendu par le système.
        detail: String,
    },

    /// Le PEM ne contient aucun certificat.
    #[error("aucun certificat dans le PEM fourni")]
    AucunCertificat,

    /// Le PEM ne contient aucune clé privée.
    #[error("aucune clé privée dans le PEM fourni")]
    AucuneCle,

    /// Le PEM est malformé.
    #[error("PEM illisible ({quoi}) : {detail}")]
    PemIllisible {
        /// Ce qu'on cherchait à lire.
        quoi: &'static str,
        /// Motif rendu par le lecteur PEM.
        detail: String,
    },

    /// La clé privée ne correspond pas à la clé publique du certificat.
    ///
    /// Détecté **au démarrage**, pas à la première connexion : un serveur qui
    /// démarre avec une clé qui ne correspond pas serait un serveur qui refuse
    /// toutes les sessions, ou pire, qui en accepte avec la mauvaise identité.
    #[error("la clé privée ne correspond pas au certificat : {0}")]
    CleEtCertificatIncoherents(String),

    /// La configuration TLS elle-même a été refusée par rustls.
    #[error("configuration TLS refusée : {0}")]
    ConfigurationRefusee(String),

    /// L'adresse d'écoute n'a pas pu être liée.
    #[error("écoute impossible : {0}")]
    Ecoute(String),

    /// La poignée de main a été refusée par le pair ou par la pile.
    #[error("poignée de main refusée : {0}")]
    PoigneeDeMainRefusee(String),

    /// Le pair a ouvert une connexion puis n'a **rien dit**.
    ///
    /// Sans délai, c'est le déni de service le moins coûteux du monde : un
    /// millier de connexions muettes immobilisent un millier de fils. Le pair
    /// est renvoyé, et l'erreur dit après combien de temps.
    #[error("poignée de main sans réponse après {0:?} — le pair a été renvoyé")]
    PoigneeDeMainSansReponse(Duration),

    /// Une entrée/sortie a échoué sur une session déjà établie.
    #[error("entrée/sortie : {0}")]
    EntreeSortie(String),
}

impl ErreurTls {
    /// Vrai si l'erreur a eu lieu **avant** toute écoute : le serveur n'a jamais
    /// accepté une seule connexion, donc rien n'a pu être servi en clair.
    ///
    /// C'est la distinction qui permet à une surface d'annoncer honnêtement
    /// « je ne suis pas démarré » plutôt que « je suis démarré ».
    #[must_use]
    pub fn avant_toute_ecoute(&self) -> bool {
        matches!(
            self,
            Self::CertificatAbsent { .. }
                | Self::CleAbsente { .. }
                | Self::AucunCertificat
                | Self::AucuneCle
                | Self::PemIllisible { .. }
                | Self::CleEtCertificatIncoherents(_)
                | Self::ConfigurationRefusee(_)
        )
    }
}
