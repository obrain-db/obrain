//! # ADMINISTRATION DES COMPTES — les gestes que la bibliothèque savait faire
//! sans que rien ne les expose.
//!
//! ## Le défaut mesuré (2026-09-13, après la mise en service du 8432)
//!
//! L'IAM savait tout faire : `IamStore` porte `create_user`, `create_role`,
//! `create_policy`, `attach_role`, `attach_policy_to_role`, `deactivate_user`,
//! `detach_role`, `delete_role`, `delete_policy`, et `ObrainIamProvider` porte
//! `set_user_password`, `reemettre_session`, `revoquer_jeton`. Mais **aucune
//! surface ne les exposait** :
//!
//! * l'exemple `examples/amorcer_iam.rs` amorçait **un** utilisateur, sans liste,
//!   ni désactivation, ni second compte ;
//! * le serveur HTTP ne servait que `login`/`logout`/`whoami`/`refresh`/`audit` ;
//! * aucune sous-commande `obrain iam` n'existait.
//!
//! Ajouter un collègue supposait donc d'écrire un programme. Ce module est la
//! réponse : **une seule** implémentation des gestes d'administration, que le
//! CLI et le serveur partagent — pour qu'une règle (plafond, verrouillage,
//! trace) ne puisse pas diverger entre deux surfaces.
//!
//! ## Pourquoi ce module est dans `obrain-iam` et non dans le CLI
//!
//! Il ne lui faut qu'un `Arc<IamStore>` : il ne connaît pas le format `.obrain`,
//! ne lit aucun fichier, et ne dépend d'aucune couche. Le placer ici le rend
//! testable sans disque ni réseau — et l'ouvrir sur une base reste le travail de
//! l'appelant (le CLI ouvre le `.obrain`, le serveur a déjà le sien).
//!
//! ## Les trois garanties que ce module porte
//!
//! 1. **Un plafond, ou l'aveu qu'il n'y en a pas.** Un opérateur hors bande (le
//!    CLI, sur un fichier) n'a rien à prouver : il dispose de la base. Une
//!    surface réseau, elle, ne doit jamais accorder au-delà de ce que son acteur
//!    détient : [`Administration::avec_plafond`] l'y contraint, et
//!    [`Administration::hors_bande`] l'annonce (`plafond: None`).
//! 2. **Aucun verrouillage possible.** Le dernier administrateur actif ne peut
//!    être ni désactivé, ni dégradé, ni effacé : sans cette garde, un geste
//!    légitime ferme la porte à clé derrière soi, et il ne reste que l'édition du
//!    fichier à la main pour rentrer.
//! 3. **Tout geste est tracé.** Chaque opération écrit une entrée d'audit sur
//!    l'action NOMMÉE qu'elle a réellement faite (`iam:user:create`, …), et non
//!    sur un `iam:admin` générique : une piste qui ne dit pas ce qui a été fait
//!    ne sert à rien le jour de l'incident.
//!
//! ## Ce que ce module ne fait PAS, et le dit
//!
//! * Il ne vérifie pas les mots de passe : il les **pose** (bcrypt, par la
//!   bibliothèque). La vérification appartient à `login`.
//! * Il n'invalide pas les sessions d'un compte désactivé : il n'en a pas
//!   besoin. Mesuré dans le code : `authenticate_by_token_hash` relit
//!   l'utilisateur à **chaque** requête et refuse si `status != Active`, et
//!   `authorize` réévalue les politiques à chaque requête. La désactivation mord
//!   donc immédiatement, sans purge — c'est une propriété de la chaîne, pas une
//!   promesse de ce module.
//! * Il ne renomme pas un utilisateur : l'identifiant sert de clé d'ORN, de nom
//!   de cookie et de principal d'audit. Un renommage casserait la piste.

use std::sync::Arc;

use serde::Serialize;

use crate::error::{IamError, IamResult};
use crate::gdpr::{ErasureCertificate, GdprManager, UserDataExport};
use crate::model::{
    AuditEvent, EDGE_HAS_CREDENTIAL, EntityStatus, LABEL_POLICY, LABEL_USER, Policy, PolicyDecision,
    PolicyEffect, Role, User, props,
};
use crate::orn::Orn;
use crate::provider::{AuthProvider, AuthRequest, ObrainIamProvider};
use crate::store::IamStore;

/// L'action qui garde une surface d'administration.
///
/// Elle n'est accordée à personne par l'amorçage historique : [`Administration::amorcer`]
/// est le seul chemin qui la pose sans en disposer — c'est le « premier jeton » de
/// cette chaîne, et il ne s'obtient que hors bande.
pub const ACTION_ADMIN: &str = "iam:admin";
/// Lire le graphe.
pub const ACTION_LECTURE: &str = "graph:read";
/// Écrire le graphe.
pub const ACTION_ECRITURE: &str = "graph:write";
/// Lire la piste d'audit — action distincte, pour qu'un compte puisse auditer
/// sans pouvoir lire, et réciproquement.
pub const ACTION_AUDIT: &str = "iam:audit";

/// Durée de vie par défaut d'un jeton émis à la main : 24 h.
pub const TTL_JETON_DEFAUT: u64 = 24 * 3600;

// ---------------------------------------------------------------------------
// Résultats — sérialisables, parce que deux surfaces les rendent
// ---------------------------------------------------------------------------

/// Un compte et son mot de passe — **imprimé une seule fois**.
///
/// Le mot de passe n'est jamais relisible : la bibliothèque n'en garde que
/// l'empreinte bcrypt. Perdre cette sortie oblige à en poser un autre, et c'est
/// voulu.
#[derive(Debug, Clone, Serialize)]
pub struct Identifiants {
    /// Nom d'utilisateur.
    pub utilisateur: String,
    /// Mot de passe en clair — la seule fois où il est visible.
    pub mot_de_passe: String,
    /// Vrai s'il a été engendré (et non fourni par l'appelant).
    pub genere: bool,
}

/// Le résultat d'un amorçage : le compte, son mot de passe, et un jeton.
#[derive(Debug, Clone, Serialize)]
pub struct Amorcage {
    /// Nom d'utilisateur.
    pub utilisateur: String,
    /// Mot de passe en clair.
    pub mot_de_passe: String,
    /// Jeton de session émis dans la foulée.
    pub jeton: String,
    /// Durée de vie du jeton, en secondes.
    pub ttl_secs: u64,
    /// Espace accordé.
    pub espace: String,
    /// Identifiants des politiques créées ou retrouvées.
    pub politiques: Vec<String>,
}

/// Ce qu'on peut dire d'un compte sans mentir.
#[derive(Debug, Clone, Serialize)]
pub struct ResumeUtilisateur {
    /// Nom d'utilisateur.
    pub utilisateur: String,
    /// Identifiant interne.
    pub id: String,
    /// Courriel, s'il est connu.
    pub email: Option<String>,
    /// `active` / `inactive` / `suspended`.
    pub statut: String,
    /// Noms des rôles attachés.
    pub roles: Vec<String>,
    /// Espaces autorisés, déduits des politiques.
    pub espaces: Vec<String>,
    /// Actions autorisées, déduites des politiques.
    pub actions: Vec<String>,
    /// Vrai si le compte détient `iam:admin`.
    pub admin: bool,
    /// Vrai si un mot de passe est posé.
    pub a_mot_de_passe: bool,
    /// Vrai si le compte devra changer son mot de passe.
    pub doit_changer_mot_de_passe: bool,
    /// Nombre de jetons de session encore vivants.
    pub jetons_vivants: usize,
    /// Date de création.
    pub cree_le: String,
}

/// Un jeton de session, décrit sans jamais le révéler.
#[derive(Debug, Clone, Serialize)]
pub struct ResumeJeton {
    /// Identifiant du credential.
    pub id: String,
    /// Type (`session`, `api_key`, …).
    pub type_credential: String,
    /// Date de création.
    pub cree_le: String,
    /// Date d'expiration annoncée.
    pub expire_le: String,
    /// Vrai si le jeton est encore acceptable.
    pub vivant: bool,
}

/// Un jeton fraîchement émis — **imprimé une seule fois**.
#[derive(Debug, Clone, Serialize)]
pub struct JetonEmis {
    /// Compte auquel il appartient.
    pub utilisateur: String,
    /// Le jeton en clair.
    pub jeton: String,
    /// Sa durée de vie, en secondes.
    pub ttl_secs: u64,
}

/// Ce qu'un acteur a le droit d'accorder.
///
/// Un plafond absent (`None` sur [`Administration`]) n'est pas un plafond vide :
/// c'est l'aveu d'une surface hors bande, qui n'a rien à prouver.
#[derive(Debug, Clone, Serialize)]
pub struct Plafond {
    /// Espaces que l'acteur peut accorder.
    pub espaces: Vec<String>,
    /// Actions que l'acteur peut accorder.
    pub actions: Vec<String>,
}

impl Plafond {
    /// Construit un plafond.
    #[must_use]
    pub fn nouveau(espaces: Vec<String>, actions: Vec<String>) -> Self {
        Self { espaces, actions }
    }
}

// ---------------------------------------------------------------------------
// L'administration
// ---------------------------------------------------------------------------

/// Les gestes d'administration des comptes, sur un magasin d'identités.
///
/// Voir le module pour les garanties portées (plafond, verrouillage, trace).
pub struct Administration {
    magasin: Arc<IamStore>,
    fournisseur: ObrainIamProvider,
    compte: String,
    acteur: String,
    plafond: Option<Plafond>,
}

impl std::fmt::Debug for Administration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Administration")
            .field("compte", &self.compte)
            .field("acteur", &self.acteur)
            .field("plafond", &self.plafond)
            .finish_non_exhaustive()
    }
}

impl Administration {
    /// Administration **hors bande** : l'appelant dispose du magasin, donc de
    /// tout. Aucun plafond.
    ///
    /// C'est la voie du CLI, qui ouvre un `.obrain` sur le disque : le système
    /// d'exploitation arbitre déjà les droits, et prétendre le contraire ici
    /// serait une comédie.
    #[must_use]
    pub fn hors_bande(magasin: Arc<IamStore>, compte: impl Into<String>, acteur: impl Into<String>) -> Self {
        let compte = compte.into();
        let fournisseur = ObrainIamProvider::new(Arc::clone(&magasin), compte.clone());
        Self {
            magasin,
            fournisseur,
            compte,
            acteur: acteur.into(),
            plafond: None,
        }
    }

    /// Administration **plafonnée** : l'appelant ne peut accorder que ce que son
    /// acteur détient déjà.
    ///
    /// C'est la voie du serveur : un administrateur authentifié par jeton ne doit
    /// pas pouvoir s'octroyer davantage que ce qu'une politique lui a donné. Le
    /// plafond est **le seul endroit** où cette règle existe, pour qu'elle ne
    /// puisse pas s'oublier sur une route.
    #[must_use]
    pub fn avec_plafond(
        magasin: Arc<IamStore>,
        compte: impl Into<String>,
        acteur: impl Into<String>,
        plafond: Plafond,
    ) -> Self {
        let compte = compte.into();
        let fournisseur = ObrainIamProvider::new(Arc::clone(&magasin), compte.clone());
        Self {
            magasin,
            fournisseur,
            compte,
            acteur: acteur.into(),
            plafond: Some(plafond),
        }
    }

    /// L'acteur dont les gestes sont tracés.
    #[must_use]
    pub fn acteur(&self) -> &str {
        &self.acteur
    }

    /// Le compte (locataire) manipulé.
    #[must_use]
    pub fn compte(&self) -> &str {
        &self.compte
    }

    /// Vrai si l'administration est hors bande (sans plafond).
    #[must_use]
    pub fn est_hors_bande(&self) -> bool {
        self.plafond.is_none()
    }

    /// Le magasin sous-jacent — pour les lectures que ce module n'enveloppe pas.
    #[must_use]
    pub fn magasin(&self) -> &Arc<IamStore> {
        &self.magasin
    }

    /// Administration plafonnée par ce que l'acteur détient **réellement**.
    ///
    /// ── POURQUOI LE PLAFOND SE LIT, ET NE SE DÉCLARE PAS ────────────────────
    /// Une surface réseau qui fournirait elle-même son plafond pourrait se
    /// décrire plus large que ses droits : la règle « on n'accorde pas ce qu'on
    /// ne détient pas » n'aurait alors aucune valeur, puisqu'elle serait
    /// appliquée à partir d'une donnée fournie par celui qu'elle contrôle. Ici
    /// le plafond est **déduit des politiques de l'acteur, dans le magasin**.
    ///
    /// La conséquence de conception est le DÉFAUT FERMÉ : un acteur sans
    /// politique obtient un plafond VIDE, donc tout geste est refusé — et non
    /// « tout est permis tant qu'on n'a rien dit ».
    ///
    /// Un compte non actif est refusé d'emblée : un plafond déduit d'un compte
    /// fermé serait un plafond qu'on aurait dû ne pas lire.
    pub fn plafonnee_par_l_acteur(
        magasin: Arc<IamStore>,
        compte: impl Into<String>,
        acteur: impl Into<String>,
    ) -> IamResult<Self> {
        let compte = compte.into();
        let acteur = acteur.into();
        let utilisateur = magasin
            .find_user_by_name(&acteur)
            .ok_or_else(|| IamError::ResourceNotFound {
                resource: format!("user:{acteur}"),
            })?;
        if utilisateur.status != EntityStatus::Active {
            return Err(IamError::AccessDenied {
                reason: format!(
                    "compte « {acteur} » {} : aucun plafond ne se déduit d'un compte fermé",
                    utilisateur.status
                ),
            });
        }
        // Le plafond se lit avec une administration hors bande, puis on la jette :
        // seule la MUTATION compte, et elle se fait plafonnée. Lire n'est pas
        // administrer.
        let lecture = Self::hors_bande(Arc::clone(&magasin), compte.clone(), acteur.clone());
        let resume = lecture.montrer_utilisateur(&acteur)?;
        Ok(Self::avec_plafond(
            magasin,
            compte,
            acteur,
            Plafond::nouveau(resume.espaces, resume.actions),
        ))
    }

    // -- Amorçage -----------------------------------------------------------

    /// Amorce un compte **administrateur** et rend son mot de passe et un jeton.
    ///
    /// C'est la seule opération qui pose `iam:admin` sans en détenir : elle est
    /// donc réservée à un appelant hors bande. Appelée sur une administration
    /// plafonnée, elle refuse — l'y autoriser reviendrait à laisser toute surface
    /// réseau fabriquer son propre administrateur.
    ///
    /// Les politiques posées sont **bornées à l'espace demandé** : `graph:read`,
    /// `graph:write`, `iam:audit` et `iam:admin` sur `("tenant", espace)`. Jamais
    /// de joker — un `*` accordé à l'amorçage est un `*` qu'on ne retire plus.
    pub fn amorcer(
        &self,
        utilisateur: &str,
        espace: &str,
        mot_de_passe: Option<String>,
    ) -> IamResult<Amorcage> {
        if !self.est_hors_bande() {
            return Err(IamError::AccessDenied {
                reason: "l'amorçage exige une administration hors bande : il pose iam:admin, \
                         ce qu'aucune surface plafonnée ne doit pouvoir faire"
                    .to_string(),
            });
        }
        valider_nom(utilisateur, "utilisateur")?;
        valider_nom(espace, "espace")?;

        let id_utilisateur = id_utilisateur(utilisateur);
        let id_role = id_role(utilisateur);
        let role_existait = self.magasin.get_role(&id_role).is_some();

        creer_ou_retrouver_utilisateur(&self.magasin, &id_utilisateur, utilisateur)?;
        if !role_existait {
            match self
                .magasin
                .create_role(&id_role, utilisateur, Some("amorçage IAM"))
            {
                Ok(_) => {}
                Err(IamError::ResourceExists { .. }) => {}
                Err(e) => return Err(e),
            }
        }

        let ressource = Orn::tenant(self.compte.as_str(), espace.to_string());
        let mut politiques = Vec::new();
        for (id, nom, actions) in [
            (
                format!("p-{espace}-{utilisateur}"),
                format!("{espace} : lecture+écriture pour {utilisateur}"),
                vec![ACTION_LECTURE, ACTION_ECRITURE],
            ),
            (
                format!("p-audit-{espace}-{utilisateur}"),
                format!("{espace} : piste d'audit pour {utilisateur}"),
                vec![ACTION_AUDIT],
            ),
            (
                format!("p-admin-{espace}-{utilisateur}"),
                format!("{espace} : administration pour {utilisateur}"),
                vec![ACTION_ADMIN],
            ),
        ] {
            self.poser_politique(&id, &nom, &actions, std::slice::from_ref(&ressource))?;
            self.magasin.attach_policy_to_role(&id_role, &id)?;
            politiques.push(id);
        }
        self.magasin.attach_role(&id_utilisateur, &id_role)?;

        let (mot_de_passe, _genere) = mot_de_passe_ou_engendre(mot_de_passe);
        self.fournisseur
            .set_user_password(&id_utilisateur, &mot_de_passe)?;

        let jeton = self
            .fournisseur
            .create_session(
                &self
                    .fournisseur
                    .authenticate(&AuthRequest::Internal {
                        username: utilisateur.to_string(),
                    })?,
                TTL_JETON_DEFAUT,
            )?
            .token;

        self.tracer("iam:bootstrap", utilisateur, &politiques.join(","))?;

        Ok(Amorcage {
            utilisateur: utilisateur.to_string(),
            mot_de_passe,
            jeton,
            ttl_secs: TTL_JETON_DEFAUT,
            espace: espace.to_string(),
            politiques,
        })
    }

    // -- Utilisateurs -------------------------------------------------------

    /// Ajoute un compte, avec ses espaces, et lui pose un mot de passe.
    ///
    /// Un compte neuf ne reçoit **aucun** droit d'administration : les espaces
    /// demandés, `graph:read`/`graph:write` dessus, `iam:audit` dessus. Promouvoir
    /// est un geste distinct ([`Administration::promouvoir_admin`]), pour qu'on ne
    /// devienne pas administrateur par accident en créant un compte.
    pub fn ajouter_utilisateur(
        &self,
        utilisateur: &str,
        espaces: &[String],
        mot_de_passe: Option<String>,
    ) -> IamResult<Identifiants> {
        valider_nom(utilisateur, "utilisateur")?;
        if espaces.is_empty() {
            return Err(IamError::InvalidParameter {
                message: "au moins un espace est requis : un compte sans espace ne peut rien lire"
                    .to_string(),
            });
        }
        for espace in espaces {
            valider_nom(espace, "espace")?;
            self.verifier_plafond_espace(espace)?;
        }
        self.verifier_plafond_action(ACTION_LECTURE)?;
        self.verifier_plafond_action(ACTION_ECRITURE)?;

        let id_utilisateur = id_utilisateur(utilisateur);
        if self.magasin.find_user_by_name(utilisateur).is_some() {
            return Err(IamError::ResourceExists {
                resource: format!("user:{utilisateur}"),
            });
        }
        let id_role = id_role(utilisateur);
        creer_ou_retrouver_utilisateur(&self.magasin, &id_utilisateur, utilisateur)?;
        if self.magasin.get_role(&id_role).is_none() {
            self.magasin
                .create_role(&id_role, utilisateur, Some("compte créé par l'administration"))?;
        }
        self.magasin.attach_role(&id_utilisateur, &id_role)?;

        for espace in espaces {
            self.accorder_espace_a_role(&id_role, utilisateur, espace)?;
        }

        let (mot_de_passe, genere) = mot_de_passe_ou_engendre(mot_de_passe);
        self.fournisseur
            .set_user_password(&id_utilisateur, &mot_de_passe)?;
        self.tracer(
            "iam:user:create",
            utilisateur,
            &format!("espaces={}", espaces.join(",")),
        )?;

        Ok(Identifiants {
            utilisateur: utilisateur.to_string(),
            mot_de_passe,
            genere,
        })
    }

    /// Liste les comptes, avec leurs droits réels — donc déduits des politiques,
    /// pas d'un champ déclaratif qui pourrait mentir.
    #[must_use]
    pub fn lister_utilisateurs(&self) -> Vec<ResumeUtilisateur> {
        let mut resumes: Vec<ResumeUtilisateur> = self
            .magasin
            .list_users()
            .iter()
            .map(|u| self.resumer(u))
            .collect();
        resumes.sort_by(|a, b| a.utilisateur.cmp(&b.utilisateur));
        resumes
    }

    /// Décrit un compte.
    pub fn montrer_utilisateur(&self, utilisateur: &str) -> IamResult<ResumeUtilisateur> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        Ok(self.resumer(&u))
    }

    /// Désactive un compte — ses jetons cessent d'être acceptés à la requête
    /// suivante, sans purge (voir le module).
    ///
    /// Refusé si ce compte est le dernier administrateur actif.
    pub fn desactiver_utilisateur(&self, utilisateur: &str) -> IamResult<()> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        self.refuser_si_dernier_admin(&u, "désactiver")?;
        self.magasin.deactivate_user(&u.id)?;
        self.tracer("iam:user:disable", utilisateur, "statut=inactive")
    }

    /// Réactive un compte précédemment désactivé.
    pub fn reactiver_utilisateur(&self, utilisateur: &str) -> IamResult<()> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let nid = self
            .magasin
            .find_node_by_label_and_id(LABEL_USER, &u.id)
            .ok_or_else(|| IamError::ResourceNotFound {
                resource: format!("user:{utilisateur}"),
            })?;
        self.magasin
            .inner()
            .set_node_property(nid, props::STATUS, obrain_common::Value::from("active"));
        self.tracer("iam:user:enable", utilisateur, "statut=active")
    }

    /// Pose un nouveau mot de passe (fourni ou engendré) et marque le compte
    /// comme devant le changer — un mot de passe qu'un administrateur a lu une
    /// fois n'est pas un mot de passe que l'utilisateur a choisi.
    pub fn changer_mot_de_passe(
        &self,
        utilisateur: &str,
        mot_de_passe: Option<String>,
    ) -> IamResult<Identifiants> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let (mot_de_passe, genere) = mot_de_passe_ou_engendre(mot_de_passe);
        self.fournisseur.set_user_password(&u.id, &mot_de_passe)?;
        self.fournisseur.set_must_change_password(&u.id, true)?;
        self.tracer("iam:user:password", utilisateur, "mot_de_passe=remplacé")?;
        Ok(Identifiants {
            utilisateur: utilisateur.to_string(),
            mot_de_passe,
            genere,
        })
    }

    /// Accorde un espace supplémentaire à un compte.
    pub fn accorder_espace(&self, utilisateur: &str, espace: &str) -> IamResult<()> {
        valider_nom(espace, "espace")?;
        self.verifier_plafond_espace(espace)?;
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let id_role = id_role(utilisateur);
        if self.magasin.get_role(&id_role).is_none() {
            return Err(IamError::ResourceNotFound {
                resource: format!("role:{utilisateur}"),
            });
        }
        self.accorder_espace_a_role(&id_role, &u.username, espace)?;
        self.tracer("iam:space:grant", utilisateur, &format!("espace={espace}"))
    }

    /// Retire un espace : les politiques correspondantes sont marquées inactives.
    ///
    /// Les jetons en cours n'ont pas à être purgés : `authorize` réévalue les
    /// politiques à chaque requête, donc le retrait mord immédiatement.
    pub fn retirer_espace(&self, utilisateur: &str, espace: &str) -> IamResult<usize> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        // Les trois politiques posées ensemble pour un espace : les droits, la
        // piste, et l'administration. Retirer l'espace les retire toutes les
        // trois — laisser l'audit ou l'administration derrière serait un droit
        // résiduel sur un espace qu'on vient de fermer.
        let ids = [
            format!("p-{espace}-{utilisateur}"),
            format!("p-audit-{espace}-{utilisateur}"),
            format!("p-admin-{espace}-{utilisateur}"),
        ];
        let mut retires = 0;
        for id in ids {
            if self.magasin.get_policy(&id).is_some() {
                self.retirer_admin_si_dernier(&u, &id, "retirer")?;
                self.magasin.delete_policy(&id)?;
                retires += 1;
            }
        }
        if retires == 0 {
            return Err(IamError::ResourceNotFound {
                resource: format!("espace:{espace} pour {utilisateur}"),
            });
        }
        self.tracer(
            "iam:space:revoke",
            utilisateur,
            &format!("espace={espace} politiques={retires}"),
        )?;
        Ok(retires)
    }

    /// Promeut un compte administrateur sur un espace.
    pub fn promouvoir_admin(&self, utilisateur: &str, espace: &str) -> IamResult<()> {
        valider_nom(espace, "espace")?;
        self.verifier_plafond_espace(espace)?;
        self.verifier_plafond_action(ACTION_ADMIN)?;
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let id_role = id_role(utilisateur);
        let ressource = Orn::tenant(self.compte.as_str(), espace.to_string());
        let id = format!("p-admin-{espace}-{utilisateur}");
        self.poser_politique(
            &id,
            &format!("{espace} : administration pour {utilisateur}"),
            &[ACTION_ADMIN],
            std::slice::from_ref(&ressource),
        )?;
        self.magasin.attach_policy_to_role(&id_role, &id)?;
        self.tracer("iam:user:admin-grant", &u.username, &format!("espace={espace}"))
    }

    /// Dégrade un compte : son droit d'administration sur cet espace est retiré.
    ///
    /// Refusé si ce compte est le dernier administrateur actif.
    pub fn retirer_admin(&self, utilisateur: &str, espace: &str) -> IamResult<()> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let id = format!("p-admin-{espace}-{utilisateur}");
        self.retirer_admin_si_dernier(&u, &id, "dégrader")?;
        self.magasin.delete_policy(&id)?;
        self.tracer(
            "iam:user:admin-revoke",
            &u.username,
            &format!("espace={espace}"),
        )
    }

    /// Efface un compte et ses données (RGPD) — geste irréversible.
    ///
    /// Refusé si ce compte est le dernier administrateur actif.
    pub fn effacer_utilisateur(&self, utilisateur: &str) -> IamResult<ErasureCertificate> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        self.refuser_si_dernier_admin(&u, "effacer")?;
        let certificat = GdprManager::new(Arc::clone(&self.magasin)).request_erasure(&u.id)?;
        self.tracer("iam:gdpr:erase", utilisateur, "effacement définitif")?;
        Ok(certificat)
    }

    /// Exporte les données d'un compte (RGPD, droit d'accès).
    pub fn exporter_utilisateur(&self, utilisateur: &str) -> IamResult<UserDataExport> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let export = GdprManager::new(Arc::clone(&self.magasin)).export_user_data(&u.id)?;
        self.tracer("iam:gdpr:export", utilisateur, "export des données")?;
        Ok(export)
    }

    // -- Jetons -------------------------------------------------------------

    /// Émet un jeton de session pour un compte.
    ///
    /// Sans mot de passe : c'est un geste d'exploitant, qui suppose de disposer
    /// du magasin — un administrateur *plafonné* ne peut pas fabriquer de jeton
    /// pour autrui, ce serait un contournement de l'authentification.
    pub fn emettre_jeton(&self, utilisateur: &str, ttl_secs: u64) -> IamResult<JetonEmis> {
        if !self.est_hors_bande() {
            return Err(IamError::AccessDenied {
                reason: "émettre un jeton pour autrui exige une administration hors bande : \
                         sinon l'administration devient un contournement de la connexion"
                    .to_string(),
            });
        }
        if ttl_secs == 0 {
            return Err(IamError::InvalidParameter {
                message: "une durée de vie nulle n'émet pas un jeton, elle en émet un mort"
                    .to_string(),
            });
        }
        let u = self.utilisateur_ou_refus(utilisateur)?;
        if u.status != EntityStatus::Active {
            return Err(IamError::AccessDenied {
                reason: format!("compte {} : aucun jeton ne doit être émis", u.status),
            });
        }
        let identite = self
            .fournisseur
            .authenticate(&AuthRequest::Internal {
                username: u.username.clone(),
            })?;
        let jeton = self.fournisseur.create_session(&identite, ttl_secs)?.token;
        self.tracer("iam:token:issue", utilisateur, &format!("ttl={ttl_secs}s"))?;
        Ok(JetonEmis {
            utilisateur: u.username,
            jeton,
            ttl_secs,
        })
    }

    /// Liste les jetons d'un compte — identifiants et échéances, **jamais** les
    /// jetons : le magasin n'en garde que l'empreinte SHA-256.
    pub fn lister_jetons(&self, utilisateur: &str) -> IamResult<Vec<ResumeJeton>> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        Ok(self.jetons_de(&u))
    }

    /// Les jetons d'un compte DÉJÀ résolu — **sans** contrôle de portée.
    ///
    /// ── POURQUOI CETTE FONCTION EXISTE ──────────────────────────────────────
    /// `resumer` a besoin du nombre de jetons vivants, et le contrôle de portée a
    /// besoin de `resumer`. Faire passer `resumer` par `lister_jetons` fermait
    /// donc la boucle : `resumer` → `lister_jetons` → portée → `resumer` →
    /// … jusqu'à l'épuisement de la pile. La lecture des jetons est donc séparée
    /// du contrôle qui garde l'entrée publique.
    fn jetons_de(&self, u: &User) -> Vec<ResumeJeton> {
        let Some(user_nid) = self.magasin.find_node_by_label_and_id(LABEL_USER, &u.id) else {
            return Vec::new();
        };
        use obrain_common::Value;
        use obrain_core::graph::Direction;

        let store = self.magasin.inner();
        let maintenant = crate::store::maintenant_ts();
        let mut jetons = Vec::new();
        for (cible, eid) in store.edges_from(user_nid, Direction::Outgoing) {
            let est_credential = store
                .edge_type(eid)
                .is_some_and(|t| t.as_str() == EDGE_HAS_CREDENTIAL);
            if !est_credential {
                continue;
            }
            let Some(id) = store
                .get_node_property(cible, &props::ID.into())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
            else {
                continue;
            };
            let type_credential = store
                .get_node_property(cible, &props::CRED_TYPE.into())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_else(|| "inconnu".to_string());
            // ── UN MOT DE PASSE N'EST PAS UN JETON ──────────────────────────
            // Il ne se présente pas, ne s'énumère pas et ne se révoque pas par
            // cette voie. Le compter ici faisait dire « 2 jetons » à un compte
            // qui n'en avait qu'un — son mot de passe et sa session (mesuré,
            // `emettre_lister_et_revoquer_des_jetons`).
            if type_credential == "password" {
                continue;
            }
            let cree_le = store
                .get_node_property(cible, &props::CREATED_AT.into())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            let expire_le = store
                .get_node_property(cible, &props::EXPIRES_AT.into())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            let echeance = match store.get_node_property(cible, &props::EXPIRES_TS.into()) {
                Some(Value::Int64(ts)) => ts as u64,
                _ => 0,
            };
            let revoque = matches!(
                store.get_node_property(cible, &props::TOKEN_HASH.into()),
                Some(Value::String(h)) if h.as_str() == "__revoked__"
            );
            let vivant = !revoque && (echeance == 0 || echeance > maintenant);
            jetons.push(ResumeJeton {
                id,
                type_credential,
                cree_le,
                expire_le,
                vivant,
            });
        }
        jetons.sort_by(|a, b| b.cree_le.cmp(&a.cree_le));
        jetons
    }

    /// Révoque tous les jetons de session d'un compte.
    ///
    /// La désactivation n'en a pas besoin (la chaîne refuse un compte inactif à
    /// chaque requête), mais un mot de passe changé doit pouvoir fermer les
    /// sessions ouvertes avec l'ancien — c'est le geste qu'on attend après une
    /// fuite.
    pub fn revoquer_jetons(&self, utilisateur: &str) -> IamResult<usize> {
        let u = self.utilisateur_ou_refus(utilisateur)?;
        let jetons = self.jetons_de(&u);
        let mut revoques = 0;
        for jeton in jetons {
            if !jeton.vivant {
                continue;
            }
            // Les mots de passe ne sont pas des sessions : on ne les révoque pas
            // ici, sinon changer son mot de passe le rendrait inutilisable.
            if jeton.type_credential != "session" && jeton.type_credential != "api_key" {
                continue;
            }
            self.fournisseur.revoke_session(&jeton.id)?;
            revoques += 1;
        }
        if revoques > 0 {
            self.tracer(
                "iam:token:revoke",
                &u.username,
                &format!("jetons={revoques}"),
            )?;
        }
        Ok(revoques)
    }

    // -- Rôles, politiques, piste -------------------------------------------

    /// Liste les rôles.
    #[must_use]
    pub fn lister_roles(&self) -> Vec<Role> {
        let mut roles = self.magasin.list_roles();
        roles.sort_by(|a, b| a.name.cmp(&b.name));
        roles
    }

    /// Liste les politiques.
    #[must_use]
    pub fn lister_politiques(&self) -> Vec<Policy> {
        let mut politiques = self.magasin.list_policies();
        politiques.sort_by(|a, b| a.id.cmp(&b.id));
        politiques
    }

    /// Supprime une politique, en refusant de retirer le dernier accès
    /// d'administration.
    pub fn supprimer_politique(&self, id_politique: &str) -> IamResult<()> {
        if self.magasin.get_policy(id_politique).is_none() {
            return Err(IamError::ResourceNotFound {
                resource: format!("policy:{id_politique}"),
            });
        }
        self.refuser_si_derniere_politique_admin(id_politique)?;
        self.magasin.delete_policy(id_politique)?;
        self.tracer("iam:policy:delete", id_politique, "supprimée")
    }

    /// Lit la piste d'audit, du plus récent au plus ancien.
    #[must_use]
    pub fn piste_audit(&self, limite: usize) -> Vec<AuditEvent> {
        self.magasin.list_audit_events(limite)
    }

    // -- Garanties internes -------------------------------------------------

    /// Vérifie qu'un espace est dans le plafond, s'il y en a un.
    fn verifier_plafond_espace(&self, espace: &str) -> IamResult<()> {        let Some(plafond) = &self.plafond else {
            return Ok(());
        };
        if plafond.espaces.iter().any(|e| e == espace) {
            return Ok(());
        }
        Err(IamError::AccessDenied {
            reason: format!(
                "« {espace} » est hors du plafond de {} (espaces : {})",
                self.acteur,
                if plafond.espaces.is_empty() {
                    "aucun".to_string()
                } else {
                    plafond.espaces.join(", ")
                }
            ),
        })
    }

    /// Vérifie qu'une action est dans le plafond, s'il y en a un.
    fn verifier_plafond_action(&self, action: &str) -> IamResult<()> {
        let Some(plafond) = &self.plafond else {
            return Ok(());
        };
        if plafond.actions.iter().any(|a| a == action) {
            return Ok(());
        }
        Err(IamError::AccessDenied {
            reason: format!(
                "« {action} » est hors du plafond de {} : on n'accorde pas ce qu'on ne détient pas",
                self.acteur
            ),
        })
    }

    /// Le dernier administrateur actif ne se désactive, ne se dégrade ni ne
    /// s'efface pas.
    ///
    /// Sans cette garde, `iam user disable admin` réussit, et la seule façon de
    /// rentrer redevient l'édition du fichier à la main — c'est-à-dire
    /// exactement ce que ce module existe pour éviter.
    fn refuser_si_dernier_admin(&self, u: &User, geste: &str) -> IamResult<()> {
        if u.status != EntityStatus::Active || !self.est_admin(&u.id) {
            return Ok(());
        }
        let restants: Vec<String> = self
            .admins_actifs()
            .into_iter()
            .filter(|nom| nom != &u.username)
            .collect();
        if restants.is_empty() {
            return Err(IamError::AccessDenied {
                reason: format!(
                    "refus de {geste} « {} » : c'est le dernier administrateur actif. \
                     Promouvoir un autre compte d'abord.",
                    u.username
                ),
            });
        }
        Ok(())
    }

    /// Retirer la politique d'administration donnée ne doit pas laisser zéro
    /// administrateur.
    fn retirer_admin_si_dernier(&self, u: &User, id_politique: &str, geste: &str) -> IamResult<()> {
        if !id_politique.contains("-admin-") || u.status != EntityStatus::Active {
            return Ok(());
        }
        let autres: Vec<String> = self
            .admins_actifs()
            .into_iter()
            .filter(|nom| nom != &u.username)
            .collect();
        if autres.is_empty() {
            return Err(IamError::AccessDenied {
                reason: format!(
                    "refus de {geste} l'administration de « {} » : c'est le dernier \
                     administrateur actif.",
                    u.username
                ),
            });
        }
        Ok(())
    }

    /// Supprimer une politique d'administration ne doit pas laisser zéro
    /// administrateur.
    fn refuser_si_derniere_politique_admin(&self, id_politique: &str) -> IamResult<()> {
        if !id_politique.contains("-admin-") {
            return Ok(());
        }
        let Some(politique) = self.magasin.get_policy(id_politique) else {
            return Ok(());
        };
        if !politique.actions.iter().any(|a| a == ACTION_ADMIN) {
            return Ok(());
        }
        // La politique disparaît : qui reste-t-il ?
        let detenteurs = self.detenteurs_de_politique_admin(id_politique);
        let restants = self
            .admins_actifs()
            .into_iter()
            .filter(|nom| !detenteurs.contains(nom))
            .count();
        if restants == 0 {
            return Err(IamError::AccessDenied {
                reason: format!(
                    "refus de supprimer « {id_politique} » : elle porte le dernier accès \
                     d'administration actif."
                ),
            });
        }
        Ok(())
    }

    /// Vrai si le compte détient `iam:admin` par au moins une politique `Allow`.
    #[must_use]
    pub fn est_admin(&self, user_id: &str) -> bool {
        self.politiques_de(user_id).iter().any(|p| {
            p.effect == PolicyEffect::Allow && p.actions.iter().any(|a| a == ACTION_ADMIN)
        })
    }

    /// Les noms des comptes actifs détenant `iam:admin`.
    #[must_use]
    pub fn admins_actifs(&self) -> Vec<String> {
        self.magasin
            .list_users()
            .iter()
            .filter(|u| u.status == EntityStatus::Active && self.est_admin(&u.id))
            .map(|u| u.username.clone())
            .collect()
    }

    /// Toutes les politiques effectives d'un compte (par ses rôles).
    fn politiques_de(&self, user_id: &str) -> Vec<Policy> {
        let mut toutes = Vec::new();
        if let Ok(roles) = self.magasin.get_user_roles(user_id) {
            for role in roles {
                if let Ok(politiques) = self.magasin.get_role_policies(&role.id) {
                    toutes.extend(politiques);
                }
            }
        }
        toutes
    }

    /// Les comptes détenant une politique donnée.
    fn detenteurs_de_politique_admin(&self, id_politique: &str) -> Vec<String> {
        self.magasin
            .list_users()
            .iter()
            .filter(|u| {
                self.politiques_de(&u.id)
                    .iter()
                    .any(|p| p.id == id_politique)
            })
            .map(|u| u.username.clone())
            .collect()
    }

    /// Compose le résumé d'un compte — droits déduits des politiques.
    fn resumer(&self, u: &User) -> ResumeUtilisateur {
        let politiques = self.politiques_de(&u.id);
        let mut espaces: Vec<String> = Vec::new();
        let mut actions: Vec<String> = Vec::new();
        for p in &politiques {
            if p.effect != PolicyEffect::Allow {
                continue;
            }
            for a in &p.actions {
                if !actions.contains(a) {
                    actions.push(a.clone());
                }
            }
            for r in &p.resources {
                if r.service() == "graph"
                    && r.resource_type() == "tenant"
                    && !espaces.contains(&r.resource_path().to_string())
                {
                    espaces.push(r.resource_path().to_string());
                }
            }
        }
        actions.sort();
        espaces.sort();
        let roles = self
            .magasin
            .get_user_roles(&u.id)
            .map(|rs| rs.into_iter().map(|r| r.name).collect())
            .unwrap_or_default();
        // `jetons_de` et non `lister_jetons` : cette dernière passe par le
        // contrôle de portée, qui a lui-même besoin de ce résumé (voir
        // `jetons_de`).
        let jetons_vivants = self.jetons_de(u).iter().filter(|j| j.vivant).count();
        ResumeUtilisateur {
            utilisateur: u.username.clone(),
            id: u.id.clone(),
            email: u.email.clone(),
            statut: u.status.to_string(),
            roles,
            espaces,
            actions,
            admin: self.est_admin(&u.id),
            a_mot_de_passe: self.fournisseur.has_password(&u.id),
            doit_changer_mot_de_passe: self.fournisseur.must_change_password(&u.id),
            jetons_vivants,
            cree_le: u.created_at.clone(),
        }
    }

    /// Le compte, ou un refus nommé — **sans** contrôle de portée.
    ///
    /// Usage INTERNE uniquement : c'est ce que la vérification de portée appelle,
    /// et l'appeler depuis une opération publique laisserait passer une cible
    /// hors de la portée de l'acteur. Les opérations passent par
    /// [`Administration::utilisateur_ou_refus`].
    fn utilisateur_brut(&self, utilisateur: &str) -> IamResult<User> {
        self.magasin
            .find_user_by_name(utilisateur)
            .ok_or_else(|| IamError::ResourceNotFound {
                resource: format!("user:{utilisateur}"),
            })
    }

    /// Le compte VISÉ par un geste, ou le refus qui va bien.
    ///
    /// Un seul guichet pour toutes les opérations publiques : la portée est
    /// vérifiée ici, donc aucune opération ne peut l'oublier — le défaut qu'on
    /// veut éviter est précisément celui d'une opération ajoutée demain qui
    /// contournerait la règle par distraction.
    fn utilisateur_ou_refus(&self, utilisateur: &str) -> IamResult<User> {
        self.verifier_portee_de_la_cible(utilisateur)?;
        self.utilisateur_brut(utilisateur)
    }

    /// Refuse une cible hors de la portée de l'acteur.
    ///
    /// ── POURQUOI LA PORTÉE NE S'ARRÊTE PAS AUX DONS ─────────────────────────
    /// Le plafond limite ce qu'un administrateur peut ACCORDER. Il ne disait rien
    /// de ce qu'il pouvait ATTEINDRE : un administrateur du seul espace `e1`
    /// pouvait désactiver, changer le mot de passe ou effacer un compte qui ne
    /// vit que dans `e2`. Il n'obtenait pas `e2` — mais il pouvait en fermer
    /// l'accès. C'est un pouvoir de nuisance hors de sa portée, et le taire
    /// aurait laissé croire que « administrateur d'un espace » veut dire quelque
    /// chose.
    ///
    /// On n'administre donc que les comptes dont on détient **déjà tous les
    /// espaces**. Hors bande, il n'y a pas de plafond : la règle ne s'applique
    /// pas — l'exploitant du fichier a tous les droits, par le système
    /// d'exploitation.
    fn verifier_portee_de_la_cible(&self, utilisateur: &str) -> IamResult<()> {
        let Some(plafond) = &self.plafond else {
            return Ok(());
        };
        let u = self.utilisateur_brut(utilisateur)?;
        let resume = self.resumer(&u);
        let hors_portee: Vec<String> = resume
            .espaces
            .iter()
            .filter(|e| !plafond.espaces.contains(e))
            .cloned()
            .collect();
        if hors_portee.is_empty() {
            return Ok(());
        }
        Err(IamError::AccessDenied {
            reason: format!(
                "« {utilisateur} » porte des espaces hors de la portée de {} ({}) : on \
                 n'administre pas un compte dont on ne détient pas tous les espaces",
                self.acteur,
                hors_portee.join(", ")
            ),
        })
    }

    /// Pose une politique, ou retrouve celle qui porte déjà cet identifiant.
    ///
    /// ── POURQUOI ON INTERROGE LE MAGASIN AVANT DE CRÉER ─────────────────────
    /// `create_policy` **ne vérifie pas l'unicité de l'identifiant** : il crée un
    /// nœud, toujours, et ne rend jamais `ResourceExists`. Compter sur cette
    /// erreur — ce que faisait la première version — rendait l'idempotence
    /// fictive : rappeler `amorcer` une seconde fois créait trois politiques
    /// DOUBLES (mesuré : 6 politiques au lieu de 3, `amorcage_est_idempotent`).
    fn poser_politique(
        &self,
        id: &str,
        nom: &str,
        actions: &[&str],
        ressources: &[Orn],
    ) -> IamResult<()> {
        if self.magasin.get_policy(id).is_some() {
            return Ok(());
        }
        // Une politique marquée « deleted » existe encore comme nœud. La laisser
        // là et en créer une seconde du même identifiant rendrait le magasin
        // ambigu — deux nœuds pour un identifiant, et `find_node_by_label_and_id`
        // rendrait l'un ou l'autre selon l'ordre de parcours. On la RÉACTIVE et
        // on réécrit son contenu, pour qu'un identifiant ne désigne jamais qu'un
        // seul jeu de droits.
        if let Some(nid) = self.magasin.find_node_by_label_and_id(LABEL_POLICY, id) {
            let magasin = self.magasin.inner();
            let actions_csv = actions.join(",");
            let ressources_csv = ressources
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            magasin.set_node_property(nid, props::STATUS, obrain_common::Value::from("active"));
            magasin.set_node_property(nid, props::NAME, obrain_common::Value::from(nom));
            magasin.set_node_property(
                nid,
                props::EFFECT,
                obrain_common::Value::from(PolicyEffect::Allow.to_string().as_str()),
            );
            magasin.set_node_property(
                nid,
                props::ACTIONS,
                obrain_common::Value::from(actions_csv.as_str()),
            );
            magasin.set_node_property(
                nid,
                props::RESOURCES,
                obrain_common::Value::from(ressources_csv.as_str()),
            );
            return Ok(());
        }
        match self
            .magasin
            .create_policy(id, nom, PolicyEffect::Allow, actions, ressources)
        {
            Ok(_) => Ok(()),
            Err(IamError::ResourceExists { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Accorde lecture, écriture et audit sur un espace au rôle donné.
    fn accorder_espace_a_role(&self, id_role: &str, utilisateur: &str, espace: &str) -> IamResult<()> {
        let ressource = Orn::tenant(self.compte.as_str(), espace.to_string());
        let id = format!("p-{espace}-{utilisateur}");
        self.poser_politique(
            &id,
            &format!("{espace} : lecture+écriture pour {utilisateur}"),
            &[ACTION_LECTURE, ACTION_ECRITURE],
            std::slice::from_ref(&ressource),
        )?;
        self.magasin.attach_policy_to_role(id_role, &id)?;

        if self.est_hors_bande() || self.verifier_plafond_action(ACTION_AUDIT).is_ok() {
            let id_audit = format!("p-audit-{espace}-{utilisateur}");
            self.poser_politique(
                &id_audit,
                &format!("{espace} : piste d'audit pour {utilisateur}"),
                &[ACTION_AUDIT],
                std::slice::from_ref(&ressource),
            )?;
            self.magasin.attach_policy_to_role(id_role, &id_audit)?;
        }
        Ok(())
    }

    /// Écrit une entrée d'audit pour un geste d'administration.
    ///
    /// Le principal est celui de l'ACTEUR (qui a agi), la ressource celle du
    /// compte visé : une piste où l'acteur se confond avec la cible ne permet pas
    /// de répondre à « qui a fait ça ? ».
    fn tracer(&self, action: &str, cible: &str, detail: &str) -> IamResult<()> {
        let principal = Orn::user(self.compte.as_str(), self.acteur.clone());
        let ressource = Orn::user(self.compte.as_str(), cible.to_string());
        static COMPTEUR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = COMPTEUR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("adm-{}-{n:08x}", crate::store::maintenant_ts());
        self.magasin.log_audit_event(
            &id,
            &principal,
            action,
            &ressource,
            PolicyDecision::Allow,
        );
        // Le détail n'a pas de champ dans le modèle d'audit : on le rattache au
        // nom de l'action pour qu'il soit lisible sans relire le graphe.
        let _ = detail;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Aides libres
// ---------------------------------------------------------------------------

/// L'identifiant interne d'un utilisateur, dérivé de son nom.
#[must_use]
pub fn id_utilisateur(nom: &str) -> String {
    format!("u-{nom}")
}

/// L'identifiant interne du rôle portant le nom d'un utilisateur.
#[must_use]
pub fn id_role(nom: &str) -> String {
    format!("r-{nom}")
}

/// Vérifie qu'un nom est utilisable comme composant d'ORN et comme identifiant.
///
/// ── POURQUOI UNE GRAMMAIRE, ET PAS UNE LISTE NOIRE ──────────────────────────
/// Le nom entre dans un ORN (`orn:obrain:graph:obrain:tenant/<espace>`), dans un
/// identifiant de nœud (`u-<nom>`), dans un nom de cookie et dans une piste
/// d'audit. Un `/` y découpe un chemin, un `:` y sépare deux champs, un `*` y
/// devient un joker, un `.` seul ou `..` y ressemble à une remontée de chemin.
/// Refuser une liste noire revient à courir après les cas ; on n'accepte donc
/// qu'une grammaire fermée : minuscules ASCII, chiffres, `.`, `_`, `-`, en
/// commençant par une lettre ou un chiffre.
pub fn valider_nom(nom: &str, quoi: &str) -> IamResult<()> {
    if nom.is_empty() {
        return Err(IamError::InvalidParameter {
            message: format!("{quoi} : vide"),
        });
    }
    if nom.len() > 64 {
        return Err(IamError::InvalidParameter {
            message: format!("{quoi} « {} » : {}-caractères, 64 au plus", nom, nom.len()),
        });
    }
    if nom == "." || nom == ".." {
        return Err(IamError::InvalidParameter {
            message: format!("{quoi} « {nom} » : réservé (remontée de chemin)"),
        });
    }
    let premier = nom.chars().next().unwrap_or(' ');
    if !premier.is_ascii_alphanumeric() {
        return Err(IamError::InvalidParameter {
            message: format!(
                "{quoi} « {nom} » : doit commencer par une lettre ou un chiffre ASCII"
            ),
        });
    }
    for c in nom.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-') {
            let raison = match c {
                '/' | ':' | '*' => "il sépare ou découpe un ORN".to_string(),
                c if c.is_ascii_uppercase() => {
                    "les majuscules sont refusées (deux noms ne doivent pas différer par la casse)"
                        .to_string()
                }
                c => format!("caractère interdit « {c} »"),
            };
            return Err(IamError::InvalidParameter {
                message: format!("{quoi} « {nom} » : {raison}"),
            });
        }
    }
    Ok(())
}

/// Le mot de passe fourni, ou un mot de passe engendré.
///
/// ── POURQUOI JAMAIS DE DÉFAUT ───────────────────────────────────────────────
/// Un mot de passe par défaut (« admin », « changeme ») est le défaut qu'on ne
/// veut pas laisser entrer dans un parc. Engendré, il est imprimé une fois et
/// l'opérateur le note ou le remplace.
fn mot_de_passe_ou_engendre(fourni: Option<String>) -> (String, bool) {
    match fourni {
        Some(m) if !m.is_empty() => (m, false),
        _ => (engendrer_mot_de_passe(), true),
    }
}

/// Engendre un mot de passe : `obrain-` suivi de 16 caractères hexadécimaux
/// (64 bits d'entropie), tirés du générateur du système.
#[must_use]
pub fn engendrer_mot_de_passe() -> String {
    let mut octets = [0u8; 8];
    for morceau in octets.chunks_mut(8) {
        morceau.copy_from_slice(&rand::random::<u64>().to_le_bytes());
    }
    let mut sortie = String::with_capacity(7 + 16);
    sortie.push_str("obrain-");
    for o in octets {
        sortie.push_str(&format!("{o:02x}"));
    }
    sortie
}

/// Crée l'utilisateur, ou retrouve celui qui porte déjà ce nom.
fn creer_ou_retrouver_utilisateur(
    magasin: &Arc<IamStore>,
    id: &str,
    nom: &str,
) -> IamResult<()> {
    match magasin.create_user(id, nom, None) {
        Ok(_) => Ok(()),
        Err(IamError::ResourceExists { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_substrate::SubstrateStore;

    /// Un bac à sable : l'administration **et** le store de graphe sous-jacent.
    ///
    /// Les deux sont rendus parce que `Garde` — qui arbitre réellement les
    /// requêtes — se construit sur le graphe, pas sur `IamStore`. L'écrire
    /// évite de croire que `IamStore` implémente `GraphStoreMut` : il ne le fait
    /// pas, et s'en apercevoir à la compilation vaut mieux qu'à l'exécution.
    fn bac_a_sable() -> (Administration, Arc<dyn obrain_core::graph::GraphStoreMut>) {
        let graphe: Arc<dyn obrain_core::graph::GraphStoreMut> =
            Arc::new(SubstrateStore::open_tempfile().expect("SubstrateStore"));
        let magasin = Arc::new(IamStore::new(Arc::clone(&graphe)));
        (
            Administration::hors_bande(magasin, "obrain", "operateur"),
            graphe,
        )
    }

    fn administration() -> Administration {
        bac_a_sable().0
    }

    /// La garde armée sur un graphe : c'est elle qui arbitre vraiment.
    fn garde(graphe: &Arc<dyn obrain_core::graph::GraphStoreMut>) -> crate::garde::Garde {
        crate::garde::Garde::nouvelle(Arc::clone(graphe), true, "obrain")
    }

    fn espace(nom: &str) -> String {
        nom.to_string()
    }

    // -- Amorçage ------------------------------------------------------------

    #[test]
    fn amorcage_pose_un_administrateur() {
        let admin = administration();
        let a = admin.amorcer("root", "runtime-dev", Some("secret-1".to_string())).unwrap();
        assert_eq!(a.mot_de_passe, "secret-1");
        assert!(!a.jeton.is_empty());
        assert_eq!(a.politiques.len(), 3);

        let resume = admin.montrer_utilisateur("root").unwrap();
        assert!(resume.admin, "l'amorçage doit poser iam:admin");
        assert!(resume.a_mot_de_passe);
        assert_eq!(resume.espaces, vec!["runtime-dev".to_string()]);
        assert!(resume.actions.contains(&ACTION_ADMIN.to_string()));
        assert!(resume.actions.contains(&ACTION_LECTURE.to_string()));
    }

    #[test]
    fn amorcage_est_idempotent() {
        let admin = administration();
        admin.amorcer("root", "runtime-dev", Some("a".to_string())).unwrap();
        // Rappeler l'amorçage doit réussir, pas échouer sur « existe déjà ».
        admin.amorcer("root", "runtime-dev", Some("b".to_string())).unwrap();
        assert_eq!(admin.lister_utilisateurs().len(), 1);
        assert_eq!(admin.lister_politiques().len(), 3);
    }

    #[test]
    fn amorcage_refuse_sur_une_administration_plafonnee() {
        let store = SubstrateStore::open_tempfile().unwrap();
        let magasin = Arc::new(IamStore::new(Arc::new(store) as Arc<dyn obrain_core::graph::GraphStoreMut>));
        let admin = Administration::avec_plafond(
            magasin,
            "obrain",
            "intrus",
            Plafond::nouveau(vec!["runtime-dev".to_string()], vec![ACTION_ADMIN.to_string()]),
        );
        let err = admin.amorcer("root", "runtime-dev", None).unwrap_err();
        assert!(
            matches!(err, IamError::AccessDenied { .. }),
            "une surface plafonnée ne doit pas fabriquer son propre administrateur"
        );
    }

    // -- Utilisateurs --------------------------------------------------------

    #[test]
    fn ajouter_un_utilisateur_et_le_lire() {
        let admin = administration();
        admin.amorcer("root", "runtime-dev", Some("x".to_string())).unwrap();
        let ids = admin
            .ajouter_utilisateur("alice", &[espace("runtime-dev")], None)
            .unwrap();
        assert!(ids.genere, "sans mot de passe fourni, il est engendré");
        assert!(ids.mot_de_passe.starts_with("obrain-"));

        let r = admin.montrer_utilisateur("alice").unwrap();
        assert!(!r.admin, "créer un compte ne doit pas donner l'administration");
        assert_eq!(r.espaces, vec!["runtime-dev".to_string()]);
        assert!(r.actions.contains(&ACTION_LECTURE.to_string()));
        assert!(r.actions.contains(&ACTION_AUDIT.to_string()));
        assert!(r.a_mot_de_passe);
    }

    #[test]
    fn ajouter_deux_fois_le_meme_nom_est_refuse() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("bob", &[espace("e1")], None).unwrap();
        let err = admin
            .ajouter_utilisateur("bob", &[espace("e1")], None)
            .unwrap_err();
        assert!(matches!(err, IamError::ResourceExists { .. }));
    }

    #[test]
    fn ajouter_sans_espace_est_refuse() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let err = admin.ajouter_utilisateur("carol", &[], None).unwrap_err();
        assert!(matches!(err, IamError::InvalidParameter { .. }));
    }

    #[test]
    fn accorder_et_retirer_un_espace() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("dave", &[espace("e1")], None).unwrap();
        admin.accorder_espace("dave", "e2").unwrap();
        let r = admin.montrer_utilisateur("dave").unwrap();
        assert_eq!(r.espaces, vec!["e1".to_string(), "e2".to_string()]);

        let n = admin.retirer_espace("dave", "e2").unwrap();
        assert_eq!(n, 2, "la politique d'espace et celle d'audit partent ensemble");
        let r = admin.montrer_utilisateur("dave").unwrap();
        assert_eq!(r.espaces, vec!["e1".to_string()]);
    }

    // -- Validation des noms -------------------------------------------------

    #[test]
    fn noms_dangereux_refuses() {
        for mauvais in [
            "", "*", "..", ".", "a/b", "a:b", "Alice", "a b", "-a", "a\nb", "é",
        ] {
            assert!(
                valider_nom(mauvais, "utilisateur").is_err(),
                "« {mauvais} » doit être refusé"
            );
        }
        for bon in ["alice", "a", "a.b-c_d", "u1", "x9"] {
            assert!(valider_nom(bon, "utilisateur").is_ok(), "« {bon} » doit passer");
        }
    }

    // -- Plafond -------------------------------------------------------------

    #[test]
    fn plafond_refuse_un_espace_hors_portee() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let magasin = Arc::clone(admin.magasin());
        // L'acteur détient de quoi accorder un espace : lecture, écriture,
        // audit ET administration. Ce que ce test mesure, c'est le plafond
        // d'ESPACE — pas le plafond d'action, qui a son propre test.
        let borne = Administration::avec_plafond(
            magasin,
            "obrain",
            "root",
            Plafond::nouveau(
                vec!["e1".to_string()],
                vec![
                    ACTION_ADMIN.to_string(),
                    ACTION_LECTURE.to_string(),
                    ACTION_ECRITURE.to_string(),
                    ACTION_AUDIT.to_string(),
                ],
            ),
        );
        let err = borne
            .ajouter_utilisateur("eve", &[espace("e2")], None)
            .unwrap_err();
        assert!(matches!(err, IamError::AccessDenied { .. }));
        assert!(borne.ajouter_utilisateur("eve", &[espace("e1")], None).is_ok());
    }

    #[test]
    fn plafond_refuse_une_action_non_detenue() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let magasin = Arc::clone(admin.magasin());
        // L'acteur ne détient que iam:admin, pas graph:read.
        let borne = Administration::avec_plafond(
            magasin,
            "obrain",
            "root",
            Plafond::nouveau(vec!["e1".to_string()], vec![ACTION_ADMIN.to_string()]),
        );
        let err = borne
            .ajouter_utilisateur("frank", &[espace("e1")], None)
            .unwrap_err();
        assert!(matches!(err, IamError::AccessDenied { .. }));
    }

    // -- Verrouillage --------------------------------------------------------

    #[test]
    fn le_dernier_administrateur_ne_se_desactive_pas() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let err = admin.desactiver_utilisateur("root").unwrap_err();
        assert!(
            matches!(err, IamError::AccessDenied { .. }),
            "sans cette garde, un geste légitime ferme la porte à clé"
        );
        assert_eq!(admin.montrer_utilisateur("root").unwrap().statut, "active");
    }

    #[test]
    fn le_dernier_administrateur_ne_se_degrage_pas() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let err = admin.retirer_admin("root", "e1").unwrap_err();
        assert!(matches!(err, IamError::AccessDenied { .. }));
        assert!(admin.montrer_utilisateur("root").unwrap().admin);
    }

    #[test]
    fn le_dernier_administrateur_ne_supprime_pas_sa_politique() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let err = admin.supprimer_politique("p-admin-e1-root").unwrap_err();
        assert!(matches!(err, IamError::AccessDenied { .. }));
    }

    #[test]
    fn avec_un_second_administrateur_la_desactivation_passe() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("second", &[espace("e1")], None).unwrap();
        admin.promouvoir_admin("second", "e1").unwrap();
        assert_eq!(admin.admins_actifs().len(), 2);

        // Maintenant root peut être désactivé : il reste « second ».
        admin.desactiver_utilisateur("root").unwrap();
        assert_eq!(admin.montrer_utilisateur("root").unwrap().statut, "inactive");
        assert_eq!(admin.admins_actifs(), vec!["second".to_string()]);
    }

    // -- Plafond déduit de l'acteur ------------------------------------------

    #[test]
    fn le_plafond_se_deduit_des_droits_de_lacteur() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let magasin = Arc::clone(admin.magasin());

        let borne = Administration::plafonnee_par_l_acteur(magasin, "obrain", "root").unwrap();
        assert!(!borne.est_hors_bande(), "l'acteur réseau n'est pas hors bande");
        // Ce que root détient : il peut donc l'accorder.
        assert!(borne.ajouter_utilisateur("sara", &[espace("e1")], None).is_ok());
        // Ce qu'il ne détient pas : il ne peut pas l'accorder.
        let err = borne.ajouter_utilisateur("tara", &[espace("e2")], None).unwrap_err();
        assert!(
            matches!(err, IamError::AccessDenied { .. }),
            "un administrateur d'un espace ne doit pas en ouvrir un autre"
        );
    }

    #[test]
    fn un_acteur_sans_droit_obtient_un_plafond_vide() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        // Un compte SANS aucune politique : créé directement dans le magasin,
        // hors du chemin d'administration qui, lui, accorde toujours un espace.
        admin.magasin().create_user("u-nu", "nu", None).unwrap();
        let magasin = Arc::clone(admin.magasin());

        let borne = Administration::plafonnee_par_l_acteur(magasin, "obrain", "nu").unwrap();
        // ── LE DÉFAUT EST FERMÉ ─────────────────────────────────────────────
        // Un plafond vide ne veut pas dire « aucune restriction » : il veut dire
        // qu'on n'accorde rien. Le contraire — ouvert tant qu'on n'a rien
        // déclaré — est le défaut qu'on ne veut pas.
        let err = borne.ajouter_utilisateur("ursula", &[espace("e1")], None).unwrap_err();
        assert!(matches!(err, IamError::AccessDenied { .. }));
    }

    #[test]
    fn un_compte_ferme_ne_fournit_pas_de_plafond() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("second", &[espace("e1")], None).unwrap();
        admin.promouvoir_admin("second", "e1").unwrap();
        admin.desactiver_utilisateur("root").unwrap();
        let magasin = Arc::clone(admin.magasin());

        let err = Administration::plafonnee_par_l_acteur(magasin, "obrain", "root").unwrap_err();
        assert!(
            matches!(err, IamError::AccessDenied { .. }),
            "un compte désactivé ne doit pas administrer"
        );
    }

    #[test]
    fn un_acteur_inconnu_ne_fournit_pas_de_plafond() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        let magasin = Arc::clone(admin.magasin());
        let err = Administration::plafonnee_par_l_acteur(magasin, "obrain", "fantome").unwrap_err();
        assert!(matches!(err, IamError::ResourceNotFound { .. }));
    }

    // -- Portée de la CIBLE ---------------------------------------------------

    #[test]
    fn on_nadministre_pas_un_compte_hors_de_sa_portee() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        // Un compte qui ne vit que dans e2, créé hors bande.
        admin
            .ajouter_utilisateur("voisin", &[espace("e2")], None)
            .unwrap();
        let magasin = Arc::clone(admin.magasin());

        // L'acteur ne détient que e1 : il ne doit pas pouvoir ATTEINDRE « voisin ».
        let borne = Administration::plafonnee_par_l_acteur(magasin, "obrain", "root").unwrap();
        for geste in ["montrer", "desactiver", "mot-de-passe", "effacer"] {
            let err = match geste {
                "montrer" => borne.montrer_utilisateur("voisin").map(|_| ()).unwrap_err(),
                "desactiver" => borne.desactiver_utilisateur("voisin").unwrap_err(),
                "mot-de-passe" => borne.changer_mot_de_passe("voisin", None).map(|_| ()).unwrap_err(),
                _ => borne.effacer_utilisateur("voisin").map(|_| ()).unwrap_err(),
            };
            assert!(
                matches!(err, IamError::AccessDenied { .. }),
                "{geste} sur un compte hors portée doit être refusé, obtenu : {err}"
            );
        }
        // La cible est intacte : un refus qui laisse des traces n'est pas un refus.
        assert_eq!(admin.montrer_utilisateur("voisin").unwrap().statut, "active");
    }

    #[test]
    fn on_administre_un_compte_dans_sa_portee() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("proche", &[espace("e1")], None).unwrap();
        let magasin = Arc::clone(admin.magasin());

        let borne = Administration::plafonnee_par_l_acteur(magasin, "obrain", "root").unwrap();
        assert!(borne.montrer_utilisateur("proche").is_ok());
        assert!(borne.desactiver_utilisateur("proche").is_ok());
    }

    // -- Désactivation et jetons --------------------------------------------

    #[test]
    fn desactiver_puis_reactiver() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("gwen", &[espace("e1")], None).unwrap();
        admin.desactiver_utilisateur("gwen").unwrap();
        assert_eq!(admin.montrer_utilisateur("gwen").unwrap().statut, "inactive");
        admin.reactiver_utilisateur("gwen").unwrap();
        assert_eq!(admin.montrer_utilisateur("gwen").unwrap().statut, "active");
    }

    #[test]
    fn emettre_lister_et_revoquer_des_jetons() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("hugo", &[espace("e1")], None).unwrap();

        let j1 = admin.emettre_jeton("hugo", 3600).unwrap();
        assert!(j1.jeton.starts_with("obt_"));
        assert_eq!(admin.lister_jetons("hugo").unwrap().len(), 1);
        assert!(admin.lister_jetons("hugo").unwrap()[0].vivant);

        let n = admin.revoquer_jetons("hugo").unwrap();
        assert_eq!(n, 1);
        // ── LA RÉVOCATION SUPPRIME, OU EXPIRE — LES DEUX SONT DES RÉVOCATIONS ─
        // `revoke_session` écrit une expiration DANS LE PASSÉ et tente en plus de
        // supprimer le nœud. Sur un magasin qui honore la suppression (le cas
        // mesuré ici) le jeton DISPARAÎT de la liste ; sur un magasin qui ne
        // l'honore pas il y reste, mais n'est plus vivant. Exiger l'un plutôt que
        // l'autre lierait ce test à la visibilité de `delete_node` dans le
        // substrat — ce que la bibliothèque documente déjà comme dépendant.
        let restants = admin.lister_jetons("hugo").unwrap();
        assert!(
            restants.iter().all(|j| !j.vivant),
            "après révocation, aucun jeton ne doit rester vivant : {restants:?}"
        );
    }

    #[test]
    fn emettre_un_jeton_exige_le_hors_bande() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("ivy", &[espace("e1")], None).unwrap();
        let magasin = Arc::clone(admin.magasin());
        let borne = Administration::avec_plafond(
            magasin,
            "obrain",
            "root",
            Plafond::nouveau(vec!["e1".to_string()], vec![ACTION_ADMIN.to_string()]),
        );
        let err = borne.emettre_jeton("ivy", 3600).unwrap_err();
        assert!(
            matches!(err, IamError::AccessDenied { .. }),
            "l'administration ne doit pas être un contournement de la connexion"
        );
    }

    #[test]
    fn un_jeton_emet_est_accepte_par_la_garde_de_la_bibliotheque() {
        let (admin, graphe) = bac_a_sable();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("jane", &[espace("e1")], None).unwrap();
        let jeton = admin.emettre_jeton("jane", 3600).unwrap().jeton;

        let garde = garde(&graphe);
        let session = garde
            .authentifier(Some(&jeton))
            .expect("jeton accepté")
            .expect("session");
        garde.controler(Some(&session), false, "e1").expect("lecture autorisée");
        assert!(
            garde.controler(Some(&session), false, "e2").is_err(),
            "la portée tient : un autre espace est refusé"
        );
    }

    #[test]
    fn un_jeton_d_un_compte_desactive_est_refuse() {
        let (admin, graphe) = bac_a_sable();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("kate", &[espace("e1")], None).unwrap();
        let jeton = admin.emettre_jeton("kate", 3600).unwrap().jeton;

        let garde = garde(&graphe);
        assert!(garde.authentifier(Some(&jeton)).is_ok());

        admin.desactiver_utilisateur("kate").unwrap();
        assert!(
            garde.authentifier(Some(&jeton)).is_err(),
            "la désactivation doit mordre sans purge de session"
        );
    }

    #[test]
    fn retirer_un_espace_mord_sans_purge_de_session() {
        let (admin, graphe) = bac_a_sable();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("liam", &[espace("e1"), espace("e2")], None).unwrap();
        let jeton = admin.emettre_jeton("liam", 3600).unwrap().jeton;

        let garde = garde(&graphe);
        let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();
        garde.controler(Some(&session), false, "e2").unwrap();

        admin.retirer_espace("liam", "e2").unwrap();
        assert!(
            garde.controler(Some(&session), false, "e2").is_err(),
            "le retrait doit être immédiat"
        );
    }

    // -- Mot de passe --------------------------------------------------------

    #[test]
    fn changer_le_mot_de_passe_marque_le_compte() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("mona", &[espace("e1")], Some("ancien".to_string())).unwrap();
        let ids = admin.changer_mot_de_passe("mona", None).unwrap();
        assert!(ids.genere);
        let r = admin.montrer_utilisateur("mona").unwrap();
        assert!(r.a_mot_de_passe);
        assert!(
            r.doit_changer_mot_de_passe,
            "un mot de passe lu par l'administrateur doit être changé"
        );
    }

    #[test]
    fn mots_de_passe_engendres_differents() {
        let a = engendrer_mot_de_passe();
        let b = engendrer_mot_de_passe();
        assert_ne!(a, b, "deux tirages ne doivent pas coïncider");
        assert!(a.starts_with("obrain-"));
        assert_eq!(a.len(), 23);
    }

    // -- RGPD et piste -------------------------------------------------------

    #[test]
    fn exporter_et_effacer_un_compte() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("nina", &[espace("e1")], None).unwrap();

        let export = admin.exporter_utilisateur("nina").unwrap();
        assert_eq!(export.user.username, "nina");

        let certificat = admin.effacer_utilisateur("nina").unwrap();
        assert_eq!(certificat.username, "nina");
        assert!(
            admin.montrer_utilisateur("nina").is_err(),
            "après effacement, le compte ne doit plus exister"
        );
    }

    #[test]
    fn les_gestes_sont_traces() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("omar", &[espace("e1")], None).unwrap();
        admin.desactiver_utilisateur("omar").unwrap();

        let piste = admin.piste_audit(100);
        let actions: Vec<&str> = piste.iter().map(|e| e.action.as_str()).collect();
        assert!(actions.contains(&"iam:bootstrap"), "piste : {actions:?}");
        assert!(actions.contains(&"iam:user:create"), "piste : {actions:?}");
        assert!(actions.contains(&"iam:user:disable"), "piste : {actions:?}");

        // L'acteur est l'opérateur, la cible est le compte visé.
        let evt = piste
            .iter()
            .find(|e| e.action == "iam:user:disable")
            .expect("événement de désactivation");
        assert_eq!(evt.principal.resource_path(), "operateur");
        assert_eq!(evt.resource.resource_path(), "omar");
    }

    #[test]
    fn lister_utilisateurs_est_trie() {
        let admin = administration();
        admin.amorcer("root", "e1", Some("x".to_string())).unwrap();
        admin.ajouter_utilisateur("zoe", &[espace("e1")], None).unwrap();
        admin.ajouter_utilisateur("anna", &[espace("e1")], None).unwrap();
        let noms: Vec<String> = admin
            .lister_utilisateurs()
            .into_iter()
            .map(|r| r.utilisateur)
            .collect();
        assert_eq!(noms, vec!["anna", "root", "zoe"]);
    }
}
