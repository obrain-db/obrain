//! L'audit chaîné — l'histoire que l'opérateur ne peut plus réécrire en silence.
//!
//! ## Le constat de l'ADR, et ce que ce module y répond
//!
//! `docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md`, §4.1, réserve n° 1 —
//! **CONSTATÉ** :
//!
//! > `model.rs` décrit `IamAuditEvent` comme des *« immutable audit trail
//! > entries »*. Or il n'y a **ni chaînage par hachage, ni signature**, et le
//! > magasin appartient à l'opérateur. Il peut supprimer ou réécrire un événement
//! > sans que rien ne le signale — ce qui disqualifie l'audit comme garantie.
//!
//! Ce module branche le [`Store`](crate::store::IamStore) d'audit sur
//! [`obrain_journal`] : chaque événement inscrit devient une entrée **chaînée et
//! signée**, et le reçu rendu au client est opposable.
//!
//! ## L'attaque qui compte, et c'est la plus simple
//!
//! Un journal d'audit n'a pas besoin qu'on en retire des lignes pour mentir : il
//! suffit de changer un **verdict**. `Deny` qui devient `Allow` est une ligne
//! modifiée, une ligne qui reste, et une histoire qui dit le contraire. C'est
//! pourquoi l'empreinte d'un événement couvre **le résultat**, et pourquoi
//! [`RegistreAudit::contient`] existe : il confronte l'événement *montré* au
//! reçu *détenu*.
//!
//! ## Ce que ce module ne ferme pas
//!
//! * **L'omission.** Un opérateur qui n'appelle pas [`RegistreAudit::enregistrer`]
//!   ne laisse aucune trace, et aucun chaînage ne peut le prendre : c'est le
//!   client qui attend un reçu, calcul après calcul, qui rend l'omission visible
//!   ([`obrain_journal::sequences_manquantes`]).
//! * **La réécriture complète.** L'opérateur détient la clé : il peut
//!   reconstruire et resigner toute l'histoire. Seul le reçu détenu par le client
//!   s'en aperçoit.
//! * **L'invalidation de la clé publique.** Elle doit être connue du client
//!   **avant** la première écriture, par un canal qui ne passe pas par
//!   l'opérateur — sinon elle est réécrite avec l'histoire qu'elle authentifie.
//! * **L'écriture des événements par les surfaces.** Ce module fournit le
//!   registre ; il ne force personne à s'en servir. Tant qu'une surface écrit son
//!   audit directement dans le store, l'audit de cette surface reste sans
//!   garantie, et il faut le dire plutôt que de le laisser croire.

use obrain_journal::{CleJournal, Entree, Journal, Resultat, empreinte_de};

use crate::model::{AuditEvent, PolicyDecision};

/// Le domaine des empreintes d'événements d'audit.
const DOMAINE_EVENEMENT: &[u8] = b"obrain-iam/v1/audit";

/// Le registre d'audit chaîné d'un espace.
///
/// Il ne remplace pas le store : il l'accompagne. Le store garde les événements
/// lisibles (c'est ce qu'on veut consulter), le registre garde les empreintes
/// chaînées (c'est ce qui rend l'histoire opposable). Les deux sont écrits au
/// même moment, par la même surface.
pub struct RegistreAudit {
    cle: CleJournal,
    journal: Journal,
}

impl RegistreAudit {
    /// Ouvre le registre d'audit d'un espace.
    ///
    /// # Erreurs
    ///
    /// [`obrain_journal::ErreurJournal::Reference`] si le nom d'espace ne peut
    /// pas désigner un journal (vide, ou trop long).
    pub fn neuf(espace: &str, cle: CleJournal) -> Resultat<Self> {
        Ok(Self {
            cle,
            journal: Journal::neuf(espace)?,
        })
    }

    /// Recharge un registre depuis ses octets.
    ///
    /// La chaîne est vérifiée au chargement : un registre rompu ne se charge pas.
    /// L'identifiant **aussi** : charger sous un nom d'espace le registre d'un
    /// autre verserait des reçus d'un espace dans l'audit d'un autre, et chaque
    /// confrontation ultérieure échouerait sans que la cause soit visible.
    ///
    /// # Erreurs
    ///
    /// Celles de [`Journal::depuis_octets`], plus
    /// [`obrain_journal::ErreurJournal::Reference`] si les octets portent le nom
    /// d'un autre espace.
    pub fn depuis_octets(espace: &str, cle: CleJournal, octets: &[u8]) -> Resultat<Self> {
        let journal = Journal::depuis_octets(octets)?;
        if journal.identifiant() != espace.trim() {
            return Err(obrain_journal::ErreurJournal::Reference(format!(
                "registre de l'espace « {} » chargé sous le nom « {} »",
                journal.identifiant(),
                espace.trim()
            )));
        }
        Ok(Self { cle, journal })
    }

    /// Inscrit un événement d'audit, et rend le reçu à remettre au client.
    ///
    /// **Le seul chemin d'écriture.** Il n'existe pas de variante non signée :
    /// une entrée d'audit que personne ne peut opposer à l'opérateur ne sert à
    /// rien, et en produire une serait donner l'illusion d'une garantie.
    ///
    /// `horodatage` est en secondes depuis l'époque Unix. C'est l'appelant qui le
    /// fournit, et il est **déclaré** : le journal prouve l'ordre, pas la date.
    ///
    /// # Erreurs
    ///
    /// Celles de [`Journal::recevoir`] — notamment si la queue du registre ne se
    /// tient plus, auquel cas rien n'est écrit.
    pub fn enregistrer(
        &mut self,
        evenement: &AuditEvent,
        horodatage: u64,
    ) -> Resultat<Entree> {
        let quoi = empreinte_evenement(evenement);
        self.journal.recevoir(&self.cle, quoi, horodatage)
    }

    /// Le journal sous-jacent — pour le persister, ou le confronter.
    #[must_use]
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// La tête du registre : ce qu'un client conserve pour détecter une
    /// troncature plus tard.
    #[must_use]
    pub fn tete(&self) -> [u8; obrain_journal::TAILLE_EMPREINTE] {
        self.journal.tete()
    }

    /// Le nombre d'événements inscrits.
    #[must_use]
    pub fn longueur(&self) -> usize {
        self.journal.longueur()
    }

    /// Vérifie la chaîne **et** toutes les signatures contre la clé publique
    /// publiée.
    ///
    /// # Erreurs
    ///
    /// Celles de [`Journal::verifier_signatures`].
    pub fn verifier(&self, publique: &[u8; 32]) -> Resultat<()> {
        self.journal.verifier_signatures(publique)
    }

    /// Confronte un événement **montré** au reçu **détenu**.
    ///
    /// C'est le contrôle que fait un client : « l'événement que tu m'affiches
    /// est-il celui que tu m'as signé ? ». Il attrape exactement ce qu'aucune
    /// vérification interne ne peut voir — un verdict réécrit dans le store, ou
    /// une histoire reconstruite et resignée.
    ///
    /// # Erreurs
    ///
    /// * [`obrain_journal::ErreurJournal::ElementDifferent`] si l'événement
    ///   présenté n'a pas l'empreinte inscrite à cette séquence ;
    /// * celles de [`Journal::accorde_avec`] (reçu d'un autre journal, séquence
    ///   absente, entrée changée).
    pub fn contient(&self, recu: &Entree, evenement: &AuditEvent) -> Resultat<()> {
        // D'abord le reçu contre le journal : si l'histoire a été réécrite, la
        // suite n'a plus de sens.
        self.journal.accorde_avec(recu)?;
        // Puis l'événement contre le reçu : c'est ici qu'un verdict changé se
        // démasque, alors que le journal, lui, est intact.
        if recu.quoi != empreinte_evenement(evenement) {
            return Err(obrain_journal::ErreurJournal::ElementDifferent {
                sequence: recu.sequence,
            });
        }
        Ok(())
    }

    /// Les octets à persister.
    #[must_use]
    pub fn vers_octets(&self) -> Vec<u8> {
        self.journal.vers_octets()
    }
}

impl core::fmt::Debug for RegistreAudit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RegistreAudit")
            .field("journal", &self.journal)
            .field("cle", &self.cle)
            .finish()
    }
}

/// L'empreinte canonique d'un événement d'audit.
///
/// ## Pourquoi chaque champ est préfixé par sa longueur
///
/// Sans préfixe, deux événements différents pourraient produire la même
/// concaténation : `principal = "ab", action = "c"` et `principal = "a",
/// action = "bc"` s'écrivent tous deux `abc`. Un opérateur qui déplace une
/// frontière changerait alors le sens d'une ligne **sans changer son
/// empreinte** — c'est-à-dire sans que rien ne le signale. Le préfixe de
/// longueur ferme cette porte, et le test
/// `deux_decoupages_differents_ont_deux_empreintes` le vérifie.
///
/// ## Ce que l'empreinte couvre
///
/// L'identifiant, le principal, l'action, la ressource, **le verdict** et
/// l'horodatage. Le verdict est le champ le plus important : `Deny` devenu
/// `Allow` est l'attaque la plus simple contre un journal d'audit, et la plus
/// rentable.
#[must_use]
pub fn empreinte_evenement(evenement: &AuditEvent) -> [u8; obrain_journal::TAILLE_EMPREINTE] {
    let mut charge = Vec::with_capacity(128);
    charge.extend_from_slice(DOMAINE_EVENEMENT);

    let principal = evenement.principal.to_string();
    let ressource = evenement.resource.to_string();
    for champ in [
        evenement.id.as_bytes(),
        principal.as_bytes(),
        evenement.action.as_bytes(),
        ressource.as_bytes(),
        evenement.timestamp.as_bytes(),
    ] {
        let longueur = u32::try_from(champ.len()).unwrap_or(u32::MAX);
        charge.extend_from_slice(&longueur.to_le_bytes());
        charge.extend_from_slice(champ);
    }

    // Le verdict, sur un octet nommé plutôt que sur son rendu textuel : un
    // changement de libellé dans l'affichage ne doit pas changer l'empreinte des
    // événements déjà inscrits.
    charge.push(match evenement.result {
        PolicyDecision::Allow => 0,
        PolicyDecision::Deny => 1,
        PolicyDecision::ImplicitDeny => 2,
    });

    empreinte_de(&charge)
}
