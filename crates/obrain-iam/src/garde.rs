//! LA GARDE D'ENTRÉE — le point unique où un point d'entrée demande une
//! identité, puis fait trancher la politique.
//!
//! ── POURQUOI ELLE EST ICI, ET PAS DANS LE CLI ─────────────────────────────
//!
//! Audit du 2026-09-13 : la bibliothèque IAM existait (identités, sessions,
//! politiques, ORN, RGPD — 95 tests verts) mais **aucun appelant hors
//! d'elle-même** : `session_ext.rs` décrivait son intégration en *pseudo-code*,
//! et la feature `iam` n'était ni dans `default` ni dans `ai`. Autrement dit :
//! la règle existait, personne ne la posait.
//!
//! La garde vit donc dans la BIBLIOTHÈQUE, pas dans un binaire : le serveur
//! HTTP, le pont MCP et le CLI appellent tous **la même** décision, et une
//! correction de politique ne se réplique pas trois fois.
//!
//! ── POSTURE : FAIL-CLOSED QUAND ARMÉE, ET BRUYANTE QUAND NON ──────────────
//!
//! *ARMÉE* (`OBRAIN_IAM=1`) : aucune identité présentée ⇒ refus ; identité
//! valide mais aucune politique ne l'autorise ⇒ refus (la décision du modèle
//! est `ImplicitDeny`, il n'y a pas d'« abstention » qui laisserait passer).
//!
//! *NON ARMÉE* : la requête passe — mais l'état est **annoncé** au démarrage du
//! serveur et publié sur `/api/info` ([`Garde::etat`]). Une porte non protégée
//! ne doit jamais être silencieuse : c'est exactement ce qui a laissé le
//! serveur de graphe exposé sans que personne ne le voie.
//!
//! Pourquoi ne pas armer par défaut : le jour où la garde a été écrite,
//! `obrain viz serve` servait le dashboard depuis `192.168.1.103` et le pont
//! MCP servait le harness — aucun des deux ne présentait la moindre identité.
//! Armer d'office aurait coupé les deux instantanément. L'interrupteur existe
//! donc, il est unique, et il est documenté :
//!
//! | variable | défaut | effet |
//! |---|---|---|
//! | `OBRAIN_IAM=1` | non armée | exige une identité et un `Allow` de politique |
//! | `OBRAIN_IAM_ACCOUNT` | `obrain` | compte porteur des identités et politiques |
//! | `OBRAIN_IAM_TOKEN` | — | jeton présenté par un point d'entrée sans requête HTTP (pont MCP) |
//! | `OBRAIN_IAM_ACTION_READ` / `_WRITE` | `graph:read` / `graph:write` | noms d'action évalués |
//!
//! ── CE QUI MANQUE ENCORE, ET QUI N'EST PAS CACHÉ ────────────────────────
//!
//! Il n'existe **aucune route de remise de jeton** dans le serveur : les jetons
//! se créent hors bande (`IamStore::create_user` / `create_credential`, ou
//! `AuthProvider::create_session`). Tant que ce chemin n'existe pas, armer la
//! garde suppose de fabriquer le premier jeton à la main.
//!
//! La portée de ressource utilisée est `("tenant", <espace>)` où l'espace est
//! le nom de la base interrogée : une politique peut donc autoriser une base et
//! pas une autre (`Orn::tenant(compte, "runtime-dev")`).

use std::sync::Arc;

use obrain_core::graph::GraphStoreMut;

use crate::error::{IamError, IamResult};
use crate::orn::Orn;
use crate::provider::{AuthProvider, AuthRequest, ObrainIamProvider};
use crate::session_ext::AuthenticatedSession;
use crate::store::IamStore;

/// La garde : une identité à demander, une politique à faire trancher.
pub struct Garde {
    provider: Arc<dyn AuthProvider>,
    /// Le MÊME fournisseur, en type concret : l'émission (`set_user_password`)
    /// et la révocation par jeton (`revoquer_jeton`) n'appartiennent pas au
    /// trait `AuthProvider`. Les garder ici évite d'élargir un trait public
    /// pour deux gestes qui ne concernent que cette implémentation.
    emetteur: Arc<ObrainIamProvider>,
    armee: bool,
    action_lecture: String,
    action_ecriture: String,
    /// Journaliser l'audit dans le magasin ? Faux pour un processus LECTEUR
    /// (pont MCP) : le magasin d'identités n'a qu'un écrivain, et un audit
    /// écrit par un second processus écrase des identités (n3, 2026-09-16).
    journal: bool,
}

impl std::fmt::Debug for Garde {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Garde")
            .field("compte", &self.emetteur.account_id())
            .field("armee", &self.armee)
            .field("action_lecture", &self.action_lecture)
            .field("action_ecriture", &self.action_ecriture)
            .finish()
    }
}

impl Garde {
    /// La garde sur un magasin de graphe déjà ouvert (les identités et les
    /// politiques y vivent, dans le graphe nommé `__system`).
    pub fn nouvelle(magasin: Arc<dyn GraphStoreMut>, armee: bool, compte: &str) -> Self {
        let store = Arc::new(IamStore::new(magasin));
        let emetteur = Arc::new(ObrainIamProvider::new(store, compte));
        let provider: Arc<dyn AuthProvider> = Arc::clone(&emetteur) as Arc<dyn AuthProvider>;
        Self {
            provider,
            emetteur,
            armee,
            action_lecture: variable_ou("OBRAIN_IAM_ACTION_READ", "graph:read"),
            action_ecriture: variable_ou("OBRAIN_IAM_ACTION_WRITE", "graph:write"),
            journal: true,
        }
    }

    /// La garde d'un processus LECTEUR (pont MCP) : elle vérifie les
    /// identités et n'écrit RIEN dans le magasin — ni son propre journal, ni
    /// l'audit du fournisseur (`IamStore::sans_audit`).
    pub fn depuis_env_lecteur(magasin: Arc<dyn GraphStoreMut>) -> Self {
        let compte = variable_ou("OBRAIN_IAM_ACCOUNT", "obrain");
        let store = Arc::new(IamStore::new(magasin).sans_audit());
        let emetteur = Arc::new(ObrainIamProvider::new(store, &compte));
        let provider: Arc<dyn AuthProvider> = Arc::clone(&emetteur) as Arc<dyn AuthProvider>;
        Self {
            provider,
            emetteur,
            armee: armee_par_env(),
            action_lecture: variable_ou("OBRAIN_IAM_ACTION_READ", "graph:read"),
            action_ecriture: variable_ou("OBRAIN_IAM_ACTION_WRITE", "graph:write"),
            journal: false,
        }
    }

    /// La garde telle que l'environnement la décrit. L'appelant fournit le
    /// magasin : lui seul sait ouvrir une base du produit.
    pub fn depuis_env(magasin: Arc<dyn GraphStoreMut>) -> Self {
        Self::nouvelle(
            magasin,
            armee_par_env(),
            &variable_ou("OBRAIN_IAM_ACCOUNT", "obrain"),
        )
    }

    /// L'IAM est-il armé ?
    #[must_use]
    pub fn armee(&self) -> bool {
        self.armee
    }

    /// L'état, en une phrase. C'est ce que le serveur annonce au démarrage et
    /// publie sur `/api/info` : une porte non protégée doit être VISIBLE.
    #[must_use]
    pub fn etat(&self) -> String {
        if self.armee {
            format!(
                "IAM ARMÉ — toute requête doit présenter une identité, et une politique doit \
                 l'autoriser (actions « {} » / « {} », ressource (« tenant », <base>)). \
                 Identité absente ou politique muette ⇒ refus.",
                self.action_lecture, self.action_ecriture
            )
        } else {
            "IAM NON ARMÉ — aucune identité n'est exigée : quiconque atteint ce point d'entrée \
             lit et écrit tout le graphe. Armer avec OBRAIN_IAM=1 (le magasin d'identités et de \
             politiques se désigne par OBRAIN_IAM_STORE)."
                .to_string()
        }
    }

    /// Le jeton d'ambiance, pour les points d'entrée qui n'ont pas de requête
    /// HTTP à lire (le pont MCP parle JSON-RPC sur son entrée standard).
    #[must_use]
    pub fn jeton_d_ambiance() -> Option<String> {
        std::env::var("OBRAIN_IAM_TOKEN")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// ── CONNEXION : mot de passe → jeton de session ─────────────────────
    ///
    /// Le premier maillon de la chaîne. Sans lui, un navigateur ne pouvait pas
    /// obtenir de jeton : il n'existait aucun chemin d'émission côté service, et
    /// un dashboard ne peut pas porter un en-tête `Authorization` sans proxy.
    /// Le mot de passe est vérifié par bcrypt côté bibliothèque, et le compte
    /// doit être actif.
    pub fn connecter(
        &self,
        utilisateur: &str,
        mot_de_passe: &str,
        ttl_secs: u64,
    ) -> IamResult<String> {
        let principal = Orn::user(self.emetteur.account_id(), utilisateur);
        let identite = match self.provider.authenticate(&AuthRequest::Password {
            username: utilisateur.to_string(),
            password: mot_de_passe.to_string(),
        }) {
            Ok(i) => i,
            Err(e) => {
                // Une connexion refusée est exactement ce qu'un incident fait
                // chercher : on la trace, avec le nom présenté.
                self.journaliser(&principal, "authenticate:password", "*", "deny");
                return Err(e);
            }
        };
        let jeton = self.provider.create_session(&identite, ttl_secs)?.token;
        self.journaliser(&principal, "session:open", "*", "allow");
        Ok(jeton)
    }

    /// ── DÉCONNEXION : le jeton cesse d'être valable, pour de bon ─────────
    ///
    /// Rend `true` si un credential a été révoqué. Avant le 2026-09-13, la
    /// révocation était un no-op : elle rendait `Ok(())` en laissant le jeton
    /// vivant (voir `provider::revoke_session`).
    pub fn deconnecter(&self, jeton: &str) -> IamResult<bool> {
        let revoque = self.emetteur.revoquer_jeton(jeton)?;
        if revoque {
            self.journaliser(
                &Orn::user(self.emetteur.account_id(), "session"),
                "session:close",
                "*",
                "allow",
            );
        }
        Ok(revoque)
    }

    /// ── QUI SUIS-JE ? ────────────────────────────────────────────────────
    /// `None` quand aucune identité ne correspond au jeton.
    pub fn identite(&self, jeton: &str) -> IamResult<Option<String>> {
        if jeton.trim().is_empty() {
            return Ok(None);
        }
        match self.provider.authenticate(&AuthRequest::SessionToken {
            token: jeton.trim().to_string(),
        }) {
            Ok(identite) => Ok(Some(identite.to_string())),
            Err(IamError::AccessDenied { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// ── RENOUVELLEMENT DE SESSION : rotation du jeton ────────────────────
    ///
    /// Un jeton qui vit des heures doit pouvoir être renouvelé sans que
    /// l'utilisateur se reconnecte — et la rotation doit RÉVOQUER l'ancien,
    /// sinon chaque renouvellement laisse un jeton valable derrière lui : la
    /// session s'accumule au lieu de tourner.
    pub fn renouveler(&self, jeton: &str, ttl_secs: u64) -> IamResult<Option<String>> {
        if self.identite(jeton)?.is_none() {
            self.journaliser(
                &Orn::user(self.emetteur.account_id(), "anonyme"),
                "session:refresh",
                "*",
                "deny",
            );
            return Ok(None);
        }
        let nouveau = self.emetteur.reemettre_session(jeton, ttl_secs)?;
        if nouveau.is_some() {
            self.journaliser(
                &Orn::user(self.emetteur.account_id(), "session"),
                "session:refresh",
                "*",
                "allow",
            );
        }
        Ok(nouveau)
    }

    /// ── LA PISTE D'AUDIT ─────────────────────────────────────────────────
    ///
    /// La bibliothèque SAVAIT journaliser (`IamStore::log_audit_event`, utilisée
    /// par l'export RGPD) mais **aucune décision de sécurité n'y était écrite** :
    /// ni les refus, ni les connexions, ni les révocations. Un IAM sans trace ne
    /// répond pas à « qui a été refusé, et quand » — la première question posée
    /// après un incident.
    #[must_use]
    pub fn audit(&self, limite: usize) -> Vec<crate::model::AuditEvent> {
        self.emetteur.store().list_audit_events(limite)
    }

    /// Écrit une décision dans la piste. Best-effort : une panne du journal ne
    /// doit pas refuser une requête légitime, mais elle ne doit pas passer
    /// inaperçue non plus (l'appelant peut lire le `Result`).
    fn journaliser(&self, principal: &Orn, action: &str, espace: &str, resultat: &str) {
        if !self.journal {
            return;
        }
        use crate::model::PolicyDecision;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COMPTEUR: AtomicU64 = AtomicU64::new(1);
        let n = COMPTEUR.fetch_add(1, Ordering::Relaxed);
        let id = format!("evt-{:016x}-{n:08x}", crate::store::maintenant_ts());
        let decision = match resultat {
            "allow" => PolicyDecision::Allow,
            "deny" => PolicyDecision::Deny,
            _ => PolicyDecision::ImplicitDeny,
        };
        let ressource = Orn::tenant(self.emetteur.account_id(), espace);
        let _ = self
            .emetteur
            .store()
            .log_audit_event(&id, principal, action, &ressource, decision);
    }

    /// Authentifie une requête. `None` = aucune identité présentée.
    ///
    /// Non armée, une identité illisible ne fait pas échouer la requête (on ne
    /// change pas le comportement d'un système qui n'a pas demandé l'IAM) —
    /// mais armée, elle refuse, et le message dit quoi présenter.
    pub fn authentifier(&self, jeton: Option<&str>) -> IamResult<Option<AuthenticatedSession>> {
        let Some(jeton) = jeton.map(str::trim).filter(|t| !t.is_empty()) else {
            // Un refus sans identité est la trace la plus utile d'une attaque :
            // on l'écrit AVANT de rendre l'erreur.
            if self.armee {
                self.journaliser(
                    &Orn::user(self.emetteur.account_id(), "anonyme"),
                    "authenticate",
                    "*",
                    "implicit_deny",
                );
            }
            return if self.armee {
                Err(IamError::AccessDenied {
                    reason: "aucune identité présentée (en-tête `Authorization: Bearer <jeton>`, \
                             ou `OBRAIN_IAM_TOKEN` pour un point d'entrée sans requête HTTP)"
                        .to_string(),
                })
            } else {
                Ok(None)
            };
        };
        match self.provider.authenticate(&AuthRequest::SessionToken {
            token: jeton.to_string(),
        }) {
            Ok(identite) => Ok(Some(AuthenticatedSession::new(
                identite,
                Arc::clone(&self.provider),
            ))),
            Err(e) => {
                if self.armee {
                    Err(e)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Tranche pour une lecture ou une écriture sur un espace donné.
    ///
    /// `ecriture` distingue `graph:write` de `graph:read` : une route qui
    /// n'observe pas n'a pas à exiger le droit d'écrire. Le nom d'espace est
    /// celui de la base visée, pour qu'une politique puisse autoriser une base
    /// sans autoriser les autres.
    pub fn controler(
        &self,
        session: Option<&AuthenticatedSession>,
        ecriture: bool,
        espace: &str,
    ) -> IamResult<()> {
        if !self.armee {
            return Ok(());
        }
        let Some(session) = session else {
            return Err(IamError::AccessDenied {
                reason: "IAM armé et aucune identité authentifiée : refus".to_string(),
            });
        };
        let action = if ecriture {
            &self.action_ecriture
        } else {
            &self.action_lecture
        };
        self.controler_action(Some(session), action, espace)
    }

    /// Contrôle pour une action NOMMÉE, au-delà de lecture/écriture : `iam:audit`
    /// par exemple. Une politique peut donc accorder la lecture de la piste
    /// d'audit sans accorder celle du graphe — et réciproquement.
    pub fn controler_action(
        &self,
        session: Option<&AuthenticatedSession>,
        action: &str,
        espace: &str,
    ) -> IamResult<()> {
        if !self.armee {
            return Ok(());
        }
        let Some(session) = session else {
            return Err(IamError::AccessDenied {
                reason: "IAM armé et aucune identité authentifiée : refus".to_string(),
            });
        };
        let verdict = session.check_graph_access(action, "tenant", espace);
        // ── LE VERDICT EST TRACÉ, PAS SEULEMENT RENDU ───────────────────────
        self.journaliser(
            session.principal(),
            action,
            espace,
            if verdict.is_ok() { "allow" } else { "deny" },
        );
        verdict
    }

    /// L'action d'ÉCRITURE configurée (`graph:write` par défaut, réglable par
    /// `OBRAIN_IAM_ACTION_WRITE`). Exposée pour qu'un point d'entrée qui juge une
    /// écriture sur une ressource NOMMÉE PAR L'APPELANT n'ait pas à recopier la
    /// constante — deux copies d'une même vérité finissent par diverger.
    #[must_use]
    pub fn action_ecriture(&self) -> &str {
        &self.action_ecriture
    }

    /// ── UNE ACTION SUR **SES PROPRES** ESPACES ──────────────────────────────
    ///
    /// Défaut mesuré le 2026-09-15, en ouvrant obrain aux comptes : certaines
    /// actions ne visent aucune base (`POST /api/worlds` crée une base NOUVELLE).
    /// Les juger sur l'« espace visé » retombe sur le store par défaut du
    /// serveur — un espace que l'appelant n'a jamais nommé, et sur lequel il n'a
    /// évidemment aucun droit. Un compte ami se voyait donc refuser l'ajout d'un
    /// dépôt **chez lui**, avec un refus parlant d'ailleurs.
    ///
    /// Ici l'appelant est jugé sur SES espaces : l'action passe dès qu'UNE
    /// politique l'y autorise. Sans droit d'écriture nulle part, le refus NOMME
    /// les espaces essayés — « refusé » sans dire où n'est pas une réponse.
    ///
    /// `espaces` vient de l'appelant (l'annuaire des identités est tenu en
    /// écriture par le serveur armé, donc illisible par une requête) ; ce qui
    /// n'est PAS cru sur parole, c'est le DROIT : chaque espace est évalué par le
    /// moteur de politiques, sur la session réelle.
    pub fn controler_soi(
        &self,
        session: Option<&AuthenticatedSession>,
        espaces: &[String],
    ) -> IamResult<()> {
        if !self.armee {
            return Ok(());
        }
        let Some(session) = session else {
            return Err(IamError::AccessDenied {
                reason: "IAM armé et aucune identité authentifiée : refus".to_string(),
            });
        };
        if espaces.is_empty() {
            return Err(IamError::AccessDenied {
                reason: format!(
                    "le compte « {} » n'a accès à aucun espace : cette action n'a nulle part où \
                     s'appliquer (demandez à l'administrateur de vous accorder un espace)",
                    session.username()
                ),
            });
        }
        let action = self.action_ecriture.clone();
        let mut refus: Option<IamError> = None;
        for espace in espaces {
            match self.controler_action(Some(session), &action, espace) {
                // UN espace qui autorise suffit : l'action a un lieu où avoir lieu.
                Ok(()) => return Ok(()),
                Err(e) => refus = Some(e),
            }
        }
        Err(IamError::AccessDenied {
            reason: format!(
                "aucun des espaces du compte « {} » ({}) n'autorise « {action} » : {:?} — \
                 cette action est donc refusée partout",
                session.username(),
                espaces.join(", "),
                refus
            ),
        })
    }

    /// L'identité d'une session, pour la journaliser.
    #[must_use]
    pub fn nom(session: Option<&AuthenticatedSession>) -> String {
        session.map_or_else(|| "anonyme".to_string(), |s| s.identity().to_string())
    }
}

fn variable_ou(cle: &str, defaut: &str) -> String {
    std::env::var(cle)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| defaut.to_string())
}

fn armee_par_env() -> bool {
    std::env::var("OBRAIN_IAM")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "oui"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PolicyEffect;
    use crate::orn::Orn;
    use crate::provider::AuthRequest;
    use obrain_substrate::SubstrateStore;

    /// Un magasin amorcé comme le ferait un exploitant : un utilisateur, un
    /// rôle, une politique `Allow` bornée à `graph:read` sur la SEULE base
    /// `runtime-dev`, et un jeton.
    fn amorce(armee: bool) -> (Garde, String) {
        let magasin: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let store = Arc::new(IamStore::new(Arc::clone(&magasin)));
        store.create_user("u1", "amorce", None).unwrap();
        store.create_role("r1", "amorce", None).unwrap();
        store
            .create_policy(
                "p1",
                "lecture de runtime-dev",
                PolicyEffect::Allow,
                &["graph:read"],
                &[Orn::tenant("obrain", "runtime-dev")],
            )
            .unwrap();
        store.attach_role("u1", "r1").unwrap();
        store.attach_policy_to_role("r1", "p1").unwrap();

        let provider = ObrainIamProvider::new(Arc::clone(&store), "obrain");
        let identite = provider
            .authenticate(&AuthRequest::Internal {
                username: "amorce".to_string(),
            })
            .unwrap();
        let jeton = provider.create_session(&identite, 3600).unwrap().token;

        (Garde::nouvelle(magasin, armee, "obrain"), jeton)
    }

    /// Un magasin amorcé **avec un mot de passe** : la chaîne complète, du
    /// geste humain (se connecter) au verdict.
    fn amorce_avec_mot_de_passe(armee: bool) -> (Garde, String) {
        let magasin: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
        let store = Arc::new(IamStore::new(Arc::clone(&magasin)));
        store.create_user("u1", "amorce", None).unwrap();
        store.create_role("r1", "amorce", None).unwrap();
        store
            .create_policy(
                "p1",
                "lecture de runtime-dev",
                PolicyEffect::Allow,
                &["graph:read"],
                &[Orn::tenant("obrain", "runtime-dev")],
            )
            .unwrap();
        store.attach_role("u1", "r1").unwrap();
        store.attach_policy_to_role("r1", "p1").unwrap();
        // Le mot de passe se pose par un fournisseur CONCRET — mais il vit dans
        // le magasin, donc la garde (qui a le sien) le vérifiera.
        let amorceur = ObrainIamProvider::new(Arc::clone(&store), "obrain");
        amorceur.set_user_password("u1", "mot-de-passe-de-test").unwrap();
        (
            Garde::nouvelle(magasin, armee, "obrain"),
            "mot-de-passe-de-test".to_string(),
        )
    }

    /// ── LA CHAÎNE COMPLÈTE ─────────────────────────────────────────────────
    /// Se connecter émet un jeton ; ce jeton ouvre ce que la politique permet ;
    /// se déconnecter le FERME pour de bon. Sans le dernier maillon, un jeton
    /// volé restait valable à vie.
    #[test]
    fn la_chaine_complete_connexion_acces_deconnexion() {
        let (garde, mot_de_passe) = amorce_avec_mot_de_passe(true);

        // 1) se connecter
        let jeton = garde
            .connecter("amorce", &mot_de_passe, 3600)
            .expect("connexion avec le bon mot de passe");
        assert!(jeton.starts_with("obt_"), "jeton opaque : {jeton}");
        assert_eq!(
            garde.identite(&jeton).unwrap().as_deref(),
            Some("amorce@obrain"),
            "« qui suis-je » doit rendre l'identité"
        );

        // 2) ce jeton ouvre ce que la politique permet
        let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();
        assert!(garde.controler(Some(&session), false, "runtime-dev").is_ok());

        // 3) se déconnecter le ferme POUR DE BON
        assert!(garde.deconnecter(&jeton).unwrap(), "un credential a été révoqué");
        assert!(
            garde.authentifier(Some(&jeton)).is_err(),
            "après déconnexion, le jeton ne doit plus rien ouvrir"
        );
        assert!(garde.identite(&jeton).unwrap().is_none());
        // Et une seconde déconnexion dit qu'il n'y avait plus rien à révoquer.
        assert!(!garde.deconnecter(&jeton).unwrap());
    }

    /// Un mauvais mot de passe n'émet rien.
    #[test]
    fn un_mauvais_mot_de_passe_n_emet_aucun_jeton() {
        let (garde, _) = amorce_avec_mot_de_passe(true);
        assert!(garde.connecter("amorce", "mauvais", 3600).is_err());
        assert!(garde.connecter("inconnu", "mot-de-passe-de-test", 3600).is_err());
    }

    /// Un jeton EXPIRÉ ne vaut plus rien. Défaut corrigé le 2026-09-13 :
    /// l'expiration était annoncée (« invalid or expired credentials ») et
    /// jamais vérifiée.
    #[test]
    fn un_jeton_expire_ne_vaut_plus_rien() {
        let (garde, mot_de_passe) = amorce_avec_mot_de_passe(true);
        let jeton = garde.connecter("amorce", &mot_de_passe, 1).unwrap();
        assert!(garde.authentifier(Some(&jeton)).is_ok(), "valable à l'émission");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(
            garde.authentifier(Some(&jeton)).is_err(),
            "après l'échéance, le jeton doit être refusé"
        );
    }

    /// ── LE RENOUVELLEMENT TOURNE LA SESSION ────────────────────────────────
    /// Un renouvellement qui n'invalide pas l'ancien jeton laisse la session
    /// s'accumuler : chaque ancien jeton reste une clé valable.
    #[test]
    fn le_renouvellement_remplace_le_jeton_et_tue_l_ancien() {
        let (garde, mot_de_passe) = amorce_avec_mot_de_passe(true);
        let ancien = garde.connecter("amorce", &mot_de_passe, 3600).unwrap();
        let nouveau = garde
            .renouveler(&ancien, 3600)
            .unwrap()
            .expect("un jeton valable se renouvelle");
        assert_ne!(ancien, nouveau, "le jeton doit changer");
        assert!(
            garde.authentifier(Some(&nouveau)).is_ok(),
            "le nouveau jeton ouvre"
        );
        assert!(
            garde.authentifier(Some(&ancien)).is_err(),
            "l'ANCIEN jeton doit être mort (rotation)"
        );
        // Un jeton déjà révoqué ne se renouvelle pas.
        assert!(garde.renouveler("jeton-invente", 3600).unwrap().is_none());
    }

    /// ── LA PISTE D'AUDIT EXISTE VRAIMENT ──────────────────────────────────
    /// La bibliothèque savait journaliser ; rien n'y écrivait. On vérifie que
    /// les trois décisions qui comptent y sont : autorisation, refus par
    /// politique, et connexion refusée.
    #[test]
    fn les_decisions_sont_tracees() {
        let (garde, mot_de_passe) = amorce_avec_mot_de_passe(true);

        // 1) une connexion REFUSÉE
        assert!(garde.connecter("amorce", "mauvais", 3600).is_err());
        // 2) une connexion acceptée
        let jeton = garde.connecter("amorce", &mot_de_passe, 3600).unwrap();
        let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();
        // 3) une autorisation…
        assert!(garde.controler(Some(&session), false, "runtime-dev").is_ok());
        // …et un refus (autre base)
        assert!(garde.controler(Some(&session), false, "autre-base").is_err());

        let evenements = garde.audit(100);
        let actions: Vec<&str> = evenements.iter().map(|e| e.action.as_str()).collect();
        assert!(
            actions.contains(&"authenticate:password"),
            "une connexion refusée doit laisser une trace : {actions:?}"
        );
        assert!(
            actions.contains(&"session:open"),
            "une connexion acceptée doit laisser une trace : {actions:?}"
        );
        assert!(
            actions.contains(&"graph:read"),
            "les verdicts de lecture doivent laisser une trace : {actions:?}"
        );
        let resultats: Vec<String> = evenements
            .iter()
            .map(|e| format!("{:?}", e.result))
            .collect();
        assert!(
            resultats.iter().any(|r| r == "Allow") && resultats.iter().any(|r| r == "Deny"),
            "les deux verdicts doivent être tracés : {resultats:?}"
        );
    }

    /// NON ARMÉE : rien ne change. C'est la condition pour pouvoir livrer la
    /// garde sans couper le dashboard ni le pont qui ne présentent rien.
    #[test]
    fn non_armee_laisse_passer_sans_identite() {
        let (garde, _) = amorce(false);
        assert!(!garde.armee());
        assert!(garde.authentifier(None).unwrap().is_none());
        assert!(garde.controler(None, false, "runtime-dev").is_ok());
        assert!(garde.controler(None, true, "runtime-dev").is_ok());
        assert!(garde.etat().contains("NON ARMÉ"));
    }

    /// ARMÉE, aucune identité : refus. Le message dit quoi présenter.
    #[test]
    fn armee_refuse_sans_identite() {
        let (garde, _) = amorce(true);
        assert!(garde.armee());
        assert!(garde.authentifier(None).is_err());
        // La bibliothèque rend une ERREUR d'IAM ; c'est le point d'entrée
        // (transport) qui en fait un 401 plutôt qu'un 403 — la règle ne doit
        // pas connaître HTTP.
        match garde.controler(None, false, "runtime-dev") {
            Err(IamError::AccessDenied { reason }) => {
                assert!(reason.contains("aucune identité"), "raison utile : {reason}");
            }
            autre => panic!("attendu AccessDenied, obtenu {autre:?}"),
        }
    }

    /// ARMÉE, jeton inconnu : refus.
    #[test]
    fn armee_refuse_un_jeton_inconnu() {
        let (garde, _) = amorce(true);
        assert!(garde.authentifier(Some("jeton-invente")).is_err());
    }

    /// ARMÉE, la politique autorise : on passe.
    #[test]
    fn armee_autorise_ce_que_la_politique_autorise() {
        let (garde, jeton) = amorce(true);
        let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();
        assert!(garde.controler(Some(&session), false, "runtime-dev").is_ok());
    }

    /// La portée tient : la même identité n'obtient RIEN sur une autre base.
    /// C'est la propriété qui distingue « une politique » d'un joker.
    #[test]
    fn la_politique_ne_deborde_pas_sur_une_autre_base() {
        let (garde, jeton) = amorce(true);
        let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();
        match garde.controler(Some(&session), false, "runtime-prod") {
            Err(IamError::PermissionDenied { action, resource }) => {
                assert_eq!(action, "graph:read");
                assert!(
                    resource.contains("runtime-prod"),
                    "le refus doit NOMMER la ressource visée : {resource}"
                );
            }
            autre => panic!("attendu PermissionDenied, obtenu {autre:?}"),
        }
    }

    /// Lire n'est pas écrire : la politique n'accorde que `graph:read`, donc
    /// une écriture sur la MÊME base est refusée.
    #[test]
    fn une_ecriture_exige_le_droit_d_ecrire() {
        let (garde, jeton) = amorce(true);
        let session = garde.authentifier(Some(&jeton)).unwrap().unwrap();
        assert!(garde.controler(Some(&session), false, "runtime-dev").is_ok());
        assert!(
            garde.controler(Some(&session), true, "runtime-dev").is_err(),
            "graph:read ne doit pas ouvrir l'écriture"
        );
    }
}
