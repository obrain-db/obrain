//! `obrain iam` — administrer les identités d'une base `.obrain`, HORS BANDE.
//!
//! ── LE DÉFAUT QUE CETTE SURFACE FERME ───────────────────────────────────────
//! La chaîne d'identités savait tout faire et ne l'exposait nulle part : ajouter
//! un collègue supposait d'écrire un programme, et `examples/amorcer_iam.rs`
//! n'amorçait qu'UN compte — sans liste, sans désactivation, sans second compte.
//! Les gestes eux-mêmes vivent dans `obrain_iam::admin`, une SEULE
//! implémentation partagée avec le serveur, pour qu'une règle (plafond,
//! verrouillage du dernier administrateur, trace) ne puisse pas diverger entre
//! deux surfaces. Ce module n'est que la SURFACE : il ouvre une base, choisit
//! l'administration hors bande, et rend ce que la bibliothèque a décidé.
//!
//! ── POURQUOI HORS BANDE, ET POURQUOI CE N'EST PAS UNE COMÉDIE ───────────────
//! Le CLI agit sur un FICHIER : le système d'exploitation arbitre déjà les
//! droits, et prétendre le contraire ici serait une comédie. Le plafond
//! (`Administration::avec_plafond`) est la règle des surfaces RÉSEAU, où un
//! administrateur authentifié ne doit pas s'octroyer plus qu'une politique ne
//! lui a donné.
//!
//! ── LA COMMANDE EXISTE MÊME SANS LA CHAÎNE IAM ──────────────────────────────
//! `obrain-iam` est une dépendance optionnelle. Sans `--features iam`, la
//! commande est TOUJOURS là, s'annonce « IAM NON COMPILÉ » et sort en erreur :
//! une sous-commande qui disparaît selon la construction rend le diagnostic
//! impossible — on cherche une faute de frappe là où il manque une feature.
//!
//! Sortie : française et lisible par défaut (tableaux alignés), `--json` (ou
//! `--format json`) pour la machine. Les mots de passe et jetons sont imprimés
//! UNE SEULE FOIS, en évidence : la bibliothèque n'en garde que l'empreinte
//! (bcrypt, SHA-256), donc les perdre oblige à en poser un autre.
//!
//! ```text
//! obrain iam amorcer   <base.obrain> <espace> [--utilisateur admin] [--mot-de-passe X]
//! obrain iam utilisateurs | utilisateur <nom> | ajouter <nom> --espace E…
//! obrain iam desactiver | reactiver | mot-de-passe <nom>
//! obrain iam espace-ajouter | espace-retirer | admin-ajouter | admin-retirer <nom> <espace>
//! obrain iam jetons <nom> | jeton <nom> [--ttl S] | jeton-revoquer <nom>
//! obrain iam roles | politiques | politique-supprimer <id> | audit [--limite N]
//! obrain iam exporter <nom> | effacer <nom> --oui
//! ```

// `Path` n'est importé nulle part : les deux aides qui s'en servent vivent
// derrière la feature `iam`, et un import inutilisé dans le build par défaut
// ferait du bruit à chaque compilation.
use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::OutputFormat;

/// Le plafond du binaire non compilé, dit une fois.
///
/// Il nomme la construction EXACTE qui manque : « IAM non disponible » sans le
/// remède laisse l'opérateur deviner, et deviner une ligne de commande coûte
/// plus cher que la lire.
#[cfg(not(feature = "iam"))]
const IAM_NON_COMPILE: &str = "IAM NON COMPILÉ — cette construction n'embarque pas la chaîne \
     d'identités (la dépendance `obrain-iam` est optionnelle).\n\
     Construire avec : cargo build --release -p obrain-cli --features ai,iam";

/// Sous-commandes `iam`.
///
/// Le chemin du magasin d'identités est le PREMIER argument positionnel, comme
/// le store de `viz subgraph` : un chemin en tête se lit et se complète sans
/// ambiguïté. `--compte` est le locataire manipulé (les ORN le portent),
/// `--acteur` est le nom TRACÉ dans la piste d'audit — les deux sont distincts
/// parce qu'une piste où l'acteur se confond avec la cible ne répond pas à
/// « qui a fait ça ? ».
#[derive(Subcommand)]
pub enum IamCommands {
    /// Amorce un compte ADMINISTRATEUR (mot de passe + jeton), et le dit
    Amorcer {
        /// Base `.obrain` (le magasin d'identités)
        path: PathBuf,
        /// Espace accordé à l'administrateur (borné à celui-là, jamais un joker)
        espace: String,
        /// Nom du compte administrateur
        #[arg(long, default_value = "admin")]
        utilisateur: String,
        /// Mot de passe (engendré et imprimé une seule fois si omis)
        #[arg(long = "mot-de-passe")]
        mot_de_passe: Option<String>,
        /// Compte (locataire) manipulé
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON) — implicite si `--format json`
        #[arg(long)]
        json: bool,
    },

    /// Liste les comptes, avec leurs droits réels
    Utilisateurs {
        /// Base `.obrain`
        path: PathBuf,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Détaille un compte
    Utilisateur {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Ajoute un compte (aucun droit d'administration : promouvoir est un geste distinct)
    Ajouter {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du nouveau compte
        nom: String,
        /// Espace autorisé (répétable, au moins un)
        #[arg(long = "espace", required = true)]
        espaces: Vec<String>,
        /// Mot de passe (engendré et imprimé une seule fois si omis)
        #[arg(long = "mot-de-passe")]
        mot_de_passe: Option<String>,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Désactive un compte (refusé pour le dernier administrateur actif)
    Desactiver {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Réactive un compte désactivé
    Reactiver {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Pose un nouveau mot de passe (le compte devra le changer)
    MotDePasse {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Mot de passe (engendré et imprimé une seule fois si omis)
        #[arg(long = "mot-de-passe")]
        mot_de_passe: Option<String>,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Accorde un espace supplémentaire
    EspaceAjouter {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Espace à accorder
        espace: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Retire un espace (droits, piste d'audit et administration compris)
    EspaceRetirer {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Espace à retirer
        espace: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Promeut un compte administrateur sur un espace
    AdminAjouter {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Espace où accorder l'administration
        espace: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Dégrade un compte (refusé pour le dernier administrateur actif)
    AdminRetirer {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Espace où retirer l'administration
        espace: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Liste les jetons d'un compte (jamais leur valeur)
    Jetons {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Émet un jeton de session pour un compte (imprimé UNE SEULE FOIS)
    Jeton {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Durée de vie en secondes (86400 = 24 h, le défaut de la bibliothèque)
        ///
        /// La valeur est écrite en dur parce que cette énumération doit compiler
        /// SANS la chaîne IAM : `TTL_JETON_DEFAUT` vit derrière la feature.
        #[arg(long, default_value_t = 86400)]
        ttl: u64,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Révoque tous les jetons de session d'un compte
    JetonRevoquer {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Liste les rôles
    Roles {
        /// Base `.obrain`
        path: PathBuf,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Liste les politiques
    Politiques {
        /// Base `.obrain`
        path: PathBuf,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Supprime une politique (refusé si elle porte le dernier accès d'administration)
    PolitiqueSupprimer {
        /// Base `.obrain`
        path: PathBuf,
        /// Identifiant de la politique
        id: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Lit la piste d'audit, du plus récent au plus ancien
    Audit {
        /// Base `.obrain`
        path: PathBuf,
        /// Nombre d'entrées lues
        #[arg(long, default_value_t = 50)]
        limite: usize,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Exporte les données d'un compte (RGPD, droit d'accès)
    Exporter {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },

    /// Efface un compte et ses données (RGPD) — IRRÉVERSIBLE, exige `--oui`
    Effacer {
        /// Base `.obrain`
        path: PathBuf,
        /// Nom du compte
        nom: String,
        /// Atteste qu'on assume un geste irréversible
        #[arg(long)]
        oui: bool,
        /// Compte (locataire)
        #[arg(long, default_value = "obrain")]
        compte: String,
        /// Nom tracé dans la piste d'audit
        #[arg(long, default_value = "cli")]
        acteur: String,
        /// Sortie machine (JSON)
        #[arg(long)]
        json: bool,
    },
}

/// Point d'entrée du groupe `iam`.
pub fn run(cmd: IamCommands, format: OutputFormat) -> Result<()> {
    #[cfg(feature = "iam")]
    {
        executer(cmd, format)
    }
    #[cfg(not(feature = "iam"))]
    {
        // Les arguments sont liés pour que rien ne parte en `unused` : une
        // commande absente doit DIRE pourquoi, pas se taire en silence.
        let _ = (cmd, format);
        anyhow::bail!("{}", IAM_NON_COMPILE)
    }
}

// ---------------------------------------------------------------------------
// Mise en œuvre (présente seulement si la chaîne IAM est compilée)
// ---------------------------------------------------------------------------

/// Exécute la sous-commande demandée.
#[cfg(feature = "iam")]
fn executer(cmd: IamCommands, format: OutputFormat) -> Result<()> {
    match cmd {
        IamCommands::Amorcer {
            path,
            espace,
            utilisateur,
            mot_de_passe,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let amorcage = administration.amorcer(&utilisateur, &espace, mot_de_passe)?;
            if machine(json, format) {
                return sortir_json(&amorcage);
            }
            entete("AMORÇAGE IAM", &path, &compte, &acteur);
            champ("administrateur", &amorcage.utilisateur);
            champ("espace accordé", &amorcage.espace);
            champ("politiques posées", amorcage.politiques.join(", "));
            println!();
            secret("MOT DE PASSE", &amorcage.mot_de_passe);
            secret(
                &format!("JETON (valable {} s)", amorcage.ttl_secs),
                &amorcage.jeton,
            );
            println!("Pour armer la garde sur un serveur :");
            println!(
                "  OBRAIN_IAM=1 OBRAIN_IAM_STORE={} OBRAIN_IAM_ACCOUNT={compte} obrain viz serve …",
                path.display()
            );
            Ok(())
        }

        IamCommands::Utilisateurs {
            path,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let comptes = administration.lister_utilisateurs();
            if machine(json, format) {
                return sortir_json(&comptes);
            }
            if comptes.is_empty() {
                println!(
                    "aucun compte dans {} — amorcer avec `obrain iam amorcer {} <espace>`",
                    path.display(),
                    path.display()
                );
                return Ok(());
            }
            let lignes: Vec<Vec<String>> = comptes
                .iter()
                .map(|c| {
                    vec![
                        c.utilisateur.clone(),
                        c.statut.clone(),
                        oui_non(c.admin),
                        liste(&c.espaces),
                        c.jetons_vivants.to_string(),
                    ]
                })
                .collect();
            print!(
                "{}",
                tableau(
                    &["UTILISATEUR", "STATUT", "ADMIN", "ESPACES", "JETONS"],
                    &lignes
                )
            );
            println!("{} compte(s) — magasin {}", comptes.len(), path.display());
            Ok(())
        }

        IamCommands::Utilisateur {
            path,
            nom,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let resume = administration.montrer_utilisateur(&nom)?;
            if machine(json, format) {
                return sortir_json(&resume);
            }
            println!("COMPTE {}", resume.utilisateur);
            champ("identifiant", &resume.id);
            champ(
                "courriel",
                resume.email.clone().unwrap_or_else(|| "—".to_string()),
            );
            champ("statut", &resume.statut);
            champ("administrateur", oui_non(resume.admin));
            champ(
                "mot de passe",
                if resume.a_mot_de_passe {
                    "posé"
                } else {
                    "AUCUN — ce compte ne peut pas se connecter"
                },
            );
            champ("à changer", oui_non(resume.doit_changer_mot_de_passe));
            champ("rôles", liste(&resume.roles));
            champ("espaces", liste(&resume.espaces));
            champ("actions", liste(&resume.actions));
            champ("jetons vivants", resume.jetons_vivants);
            champ("créé le", &resume.cree_le);
            Ok(())
        }

        IamCommands::Ajouter {
            path,
            nom,
            espaces,
            mot_de_passe,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let identifiants = administration.ajouter_utilisateur(&nom, &espaces, mot_de_passe)?;
            if machine(json, format) {
                return sortir_json(&identifiants);
            }
            println!(
                "COMPTE CRÉÉ — {} (espaces : {})",
                identifiants.utilisateur,
                espaces.join(", ")
            );
            println!("  aucun droit d'administration : `admin-ajouter` est un geste distinct");
            println!();
            secret(
                &titre_mot_de_passe(&identifiants),
                &identifiants.mot_de_passe,
            );
            Ok(())
        }

        IamCommands::Desactiver {
            path,
            nom,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            administration.desactiver_utilisateur(&nom)?;
            confirmation(
                machine(json, format),
                &format!(
                    "compte « {nom} » DÉSACTIVÉ — ses jetons cessent d'être acceptés à la \
                     requête suivante, sans purge"
                ),
                serde_json::json!({ "utilisateur": nom, "statut": "inactive" }),
            )
        }

        IamCommands::Reactiver {
            path,
            nom,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            administration.reactiver_utilisateur(&nom)?;
            confirmation(
                machine(json, format),
                &format!("compte « {nom} » RÉACTIVÉ — ses jetons d'avant redeviennent acceptables"),
                serde_json::json!({ "utilisateur": nom, "statut": "active" }),
            )
        }

        IamCommands::MotDePasse {
            path,
            nom,
            mot_de_passe,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let identifiants = administration.changer_mot_de_passe(&nom, mot_de_passe)?;
            if machine(json, format) {
                return sortir_json(&identifiants);
            }
            println!("MOT DE PASSE REMPLACÉ — {}", identifiants.utilisateur);
            println!("  le compte devra le changer à la prochaine connexion");
            println!(
                "  les sessions ouvertes avec l'ancien restent valides : \
                 `jeton-revoquer {}` les ferme",
                identifiants.utilisateur
            );
            println!();
            secret(
                &titre_mot_de_passe(&identifiants),
                &identifiants.mot_de_passe,
            );
            Ok(())
        }

        IamCommands::EspaceAjouter {
            path,
            nom,
            espace,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            administration.accorder_espace(&nom, &espace)?;
            confirmation(
                machine(json, format),
                &format!(
                    "espace « {espace} » ACCORDÉ à {nom} : graph:read, graph:write et iam:audit \
                     sur cet espace"
                ),
                serde_json::json!({ "utilisateur": nom, "espace": espace, "accorde": true }),
            )
        }

        IamCommands::EspaceRetirer {
            path,
            nom,
            espace,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let retires = administration.retirer_espace(&nom, &espace)?;
            confirmation(
                machine(json, format),
                &format!(
                    "espace « {espace} » RETIRÉ à {nom} : {retires} politique(s) supprimée(s) \
                     (droits, piste d'audit et administration)"
                ),
                serde_json::json!({
                    "utilisateur": nom, "espace": espace, "politiques_supprimees": retires
                }),
            )
        }

        IamCommands::AdminAjouter {
            path,
            nom,
            espace,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            administration.promouvoir_admin(&nom, &espace)?;
            confirmation(
                machine(json, format),
                &format!(
                    "administration ACCORDÉE à {nom} sur l'espace « {espace} » (iam:admin) — \
                     ce compte peut désormais administrer cet espace"
                ),
                serde_json::json!({ "utilisateur": nom, "espace": espace, "admin": true }),
            )
        }

        IamCommands::AdminRetirer {
            path,
            nom,
            espace,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            administration.retirer_admin(&nom, &espace)?;
            confirmation(
                machine(json, format),
                &format!("administration RETIRÉE à {nom} sur l'espace « {espace} » (iam:admin)"),
                serde_json::json!({ "utilisateur": nom, "espace": espace, "admin": false }),
            )
        }

        IamCommands::Jetons {
            path,
            nom,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let jetons = administration.lister_jetons(&nom)?;
            if machine(json, format) {
                return sortir_json(&jetons);
            }
            if jetons.is_empty() {
                println!("aucun jeton de session pour « {nom} »");
                return Ok(());
            }
            let lignes: Vec<Vec<String>> = jetons
                .iter()
                .map(|j| {
                    vec![
                        j.id.clone(),
                        j.type_credential.clone(),
                        j.cree_le.clone(),
                        j.expire_le.clone(),
                        if j.vivant { "oui" } else { "non" }.to_string(),
                    ]
                })
                .collect();
            print!(
                "{}",
                tableau(&["ID", "TYPE", "CRÉÉ LE", "EXPIRE LE", "VIVANT"], &lignes)
            );
            println!(
                "{} jeton(s) pour « {nom} » — {} vivant(s) ; leur valeur n'est pas relisible \
                 (le magasin n'en garde que l'empreinte SHA-256)",
                jetons.len(),
                jetons.iter().filter(|j| j.vivant).count()
            );
            Ok(())
        }

        IamCommands::Jeton {
            path,
            nom,
            ttl,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let emis = administration.emettre_jeton(&nom, ttl)?;
            if machine(json, format) {
                return sortir_json(&emis);
            }
            println!(
                "JETON ÉMIS pour {} (valable {} s) — geste d'exploitant, hors bande",
                emis.utilisateur, emis.ttl_secs
            );
            println!();
            secret("JETON", &emis.jeton);
            println!("Côté client :  Authorization: Bearer <jeton>");
            Ok(())
        }

        IamCommands::JetonRevoquer {
            path,
            nom,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let revoques = administration.revoquer_jetons(&nom)?;
            confirmation(
                machine(json, format),
                &format!("{revoques} jeton(s) de session RÉVOQUÉ(S) pour « {nom} »"),
                serde_json::json!({ "utilisateur": nom, "revoques": revoques }),
            )
        }

        IamCommands::Roles {
            path,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let roles = administration.lister_roles();
            if machine(json, format) {
                return sortir_json(&roles);
            }
            let lignes: Vec<Vec<String>> = roles
                .iter()
                .map(|r| {
                    vec![
                        r.id.clone(),
                        r.name.clone(),
                        r.description.clone().unwrap_or_else(|| "—".to_string()),
                        r.created_at.clone(),
                    ]
                })
                .collect();
            print!(
                "{}",
                tableau(&["ID", "NOM", "DESCRIPTION", "CRÉÉ LE"], &lignes)
            );
            println!("{} rôle(s)", roles.len());
            Ok(())
        }

        IamCommands::Politiques {
            path,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let politiques = administration.lister_politiques();
            if machine(json, format) {
                return sortir_json(&politiques);
            }
            let lignes: Vec<Vec<String>> = politiques
                .iter()
                .map(|p| {
                    vec![
                        p.id.clone(),
                        p.effect.to_string(),
                        p.actions.join(","),
                        p.resources
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ]
                })
                .collect();
            print!(
                "{}",
                tableau(&["ID", "EFFET", "ACTIONS", "RESSOURCES"], &lignes)
            );
            println!("{} politique(s)", politiques.len());
            Ok(())
        }

        IamCommands::PolitiqueSupprimer {
            path,
            id,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            administration.supprimer_politique(&id)?;
            confirmation(
                machine(json, format),
                &format!("politique « {id} » SUPPRIMÉE"),
                serde_json::json!({ "politique": id, "supprimee": true }),
            )
        }

        IamCommands::Audit {
            path,
            limite,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let evenements = administration.piste_audit(limite);
            if machine(json, format) {
                return sortir_json(&evenements);
            }
            if evenements.is_empty() {
                println!("piste d'audit vide dans {}", path.display());
                return Ok(());
            }
            let lignes: Vec<Vec<String>> = evenements
                .iter()
                .map(|e| {
                    vec![
                        e.timestamp.clone(),
                        e.action.clone(),
                        orn_court(&e.principal),
                        orn_court(&e.resource),
                        decision(e.result),
                    ]
                })
                .collect();
            // L'acteur vient AVANT la cible : une piste où les deux se
            // confondent ne répond pas à « qui a fait ça ? ».
            print!(
                "{}",
                tableau(
                    &["HORODATAGE", "ACTION", "ACTEUR", "CIBLE", "DÉCISION"],
                    &lignes
                )
            );
            println!(
                "{} entrée(s) (limite {limite}) — magasin {}",
                evenements.len(),
                path.display()
            );
            Ok(())
        }

        IamCommands::Exporter {
            path,
            nom,
            compte,
            acteur,
            json,
        } => {
            let administration = ouvrir(&path, &compte, &acteur)?;
            let export = administration.exporter_utilisateur(&nom)?;
            let texte = serde_json::to_string_pretty(&export)
                .map_err(|e| anyhow::anyhow!("iam exporter : sérialisation JSON : {e}"))?;
            if machine(json, format) {
                println!("{texte}");
                return Ok(());
            }
            println!("EXPORT RGPD (droit d'accès) — {}", export.user.username);
            champ("identifiant", &export.user.id);
            champ("statut", &export.user.status);
            champ("créé le", &export.user.created_at);
            champ("rôles", liste(&export.roles));
            champ("consentements", export.consents.len());
            champ("événements d'audit", export.audit_events.len());
            champ("exporté le", &export.exported_at);
            println!();
            println!("{texte}");
            Ok(())
        }

        IamCommands::Effacer {
            path,
            nom,
            oui,
            compte,
            acteur,
            json,
        } => {
            // ── LE REFUS EST LA COMMANDE ────────────────────────────────────
            // Le geste n'a pas de retour : le compte est ANONYMISÉ (nom
            // « ERASED », statut « erased »), ses empreintes de jetons sont
            // écrasées et ses données personnelles effacées. Le message dit ce
            // que la bibliothèque fait RÉELLEMENT — promettre une suppression
            // physique que `request_erasure` ne fait pas serait un mensonge
            // qu'aucun opérateur ne peut vérifier avant de l'avoir commis.
            if !oui {
                anyhow::bail!(
                    "effacement RGPD de « {nom} » REFUSÉ : le geste est sans retour \
                     (le compte est anonymisé — nom « ERASED », statut « erased » — et ses \
                     données personnelles effacées, empreintes de jetons comprises).\n\
                     Pour l'assumer explicitement :\n  \
                     obrain iam effacer {} {nom} --oui",
                    path.display()
                );
            }
            let administration = ouvrir(&path, &compte, &acteur)?;
            let certificat = administration.effacer_utilisateur(&nom)?;
            if machine(json, format) {
                return sortir_json(&certificat);
            }
            println!("EFFACEMENT RGPD — {}", certificat.username);
            champ("identifiant", &certificat.user_id);
            champ("effacé le", &certificat.erased_at);
            champ("acteur tracé", &certificat.principal);
            champ("portée", &certificat.scope);
            // `nodes_deleted` est un nom du modèle, pas un compte de
            // suppressions : la bibliothèque y totalise les nœuds TRAITÉS
            // (compte + jetons + consentements + audit). Le libellé humain dit
            // donc « traités » — un certificat qui annonce des suppressions
            // inexistantes est pire qu'inutile le jour d'un contrôle.
            champ("nœuds traités", certificat.nodes_deleted);
            champ("consentements", certificat.consents_deleted);
            champ("événements d'audit", certificat.audit_events_deleted);
            champ("identifiants (jetons)", certificat.credentials_deleted);
            // Ce que l'opérateur verra ensuite, dit d'avance : `request_erasure`
            // ANONYMISE le nœud du compte au lieu de le retirer du graphe
            // (la suppression physique n'est pas uniformément disponible dans
            // les backends). Sans cette ligne, la ligne « ERASED » de
            // `iam utilisateurs` passerait pour un compte fantôme.
            println!(
                "note : le nœud du compte subsiste, anonymisé (« ERASED », statut « erased ») — \
                 `iam utilisateurs` le montre encore, sous le nom ERASED"
            );
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Aides
// ---------------------------------------------------------------------------

/// Ouvre le magasin d'identités et rend l'administration **hors bande**.
///
/// `open_existing` et non `ObrainDB::open` : administrer des comptes suppose une
/// base EXISTANTE. En créer une au passage masquerait une faute de frappe dans
/// le chemin, et l'opérateur croirait avoir agi sur son parc — en écrivant dans
/// une base neuve que personne ne lit.
#[cfg(feature = "iam")]
fn ouvrir(
    path: &std::path::Path,
    compte: &str,
    acteur: &str,
) -> Result<obrain_iam::admin::Administration> {
    let db = crate::commands::open_existing(path)?;
    let magasin = std::sync::Arc::new(obrain_iam::store::IamStore::new(db.graph_store()));
    Ok(obrain_iam::admin::Administration::hors_bande(
        magasin, compte, acteur,
    ))
}

/// Vrai si la sortie doit être machine.
///
/// `--json` l'impose ; le format global la demande aussi (`--format json`, ou
/// une sortie non-TTY en mode `auto`, qui est la convention du binaire : un pipe
/// attend du JSON).
#[cfg(feature = "iam")]
fn machine(json: bool, format: OutputFormat) -> bool {
    json || crate::output::Format::from(format) == crate::output::Format::Json
}

/// Imprime une charge utile sérialisable en JSON.
#[cfg(feature = "iam")]
fn sortir_json<T: serde::Serialize>(valeur: &T) -> Result<()> {
    let texte = serde_json::to_string_pretty(valeur)
        .map_err(|e| anyhow::anyhow!("iam : sérialisation JSON : {e}"))?;
    println!("{texte}");
    Ok(())
}

/// Rend soit la forme machine, soit la phrase française passée par l'appelant.
///
/// Les gestes qui ne rendent rien (désactiver, accorder un espace…) ne doivent
/// pas se taire en JSON : une sortie vide se confond avec un échec silencieux.
#[cfg(feature = "iam")]
fn confirmation(machine: bool, phrase: &str, champs: serde_json::Value) -> Result<()> {
    if machine {
        sortir_json(&champs)
    } else {
        println!("{phrase}");
        Ok(())
    }
}

/// Le titre d'un secret engendré ou fourni — le dire évite de croire qu'un mot
/// de passe fourni par l'appelant a été engendré, et réciproquement.
#[cfg(feature = "iam")]
fn titre_mot_de_passe(identifiants: &obrain_iam::admin::Identifiants) -> String {
    if identifiants.genere {
        "MOT DE PASSE ENGENDRÉ".to_string()
    } else {
        "MOT DE PASSE (fourni par vous)".to_string()
    }
}

/// Imprime un secret EN ÉVIDENCE, avec l'avertissement qui va avec.
#[cfg(feature = "iam")]
fn secret(quoi: &str, valeur: &str) {
    println!(
        "{}",
        crate::output::colored(
            &format!("{quoi} — imprimé UNE SEULE FOIS, jamais relisible ensuite :"),
            comfy_table::Color::Yellow
        )
    );
    println!("  {valeur}");
    println!("(la bibliothèque n'en garde que l'empreinte : perdu, il faut en poser un autre)");
}

/// L'en-tête d'une sortie qui nomme la base, le compte et l'acteur tracé.
#[cfg(feature = "iam")]
fn entete(titre: &str, path: &std::path::Path, compte: &str, acteur: &str) {
    println!("{titre} — {}", path.display());
    champ("compte (locataire)", compte);
    champ("acteur tracé", acteur);
}

/// Une ligne « libellé : valeur », alignée sur les autres.
///
/// L'alignement est CALCULÉ, pas compté à la main : une espace oubliée dans un
/// littéral ne se voit qu'à l'écran, et personne ne relit dix `println!` pour
/// la retrouver. La largeur (20) est celle du plus long libellé employé
/// ci-dessous, suivie d'une espace : les deux-points restent donc alignés ET
/// détachés du libellé, même quand celui-ci remplit la colonne.
#[cfg(feature = "iam")]
fn champ(libelle: &str, valeur: impl std::fmt::Display) {
    println!("  {libelle:<20} : {valeur}");
}

/// La partie utile d'un ORN pour l'œil : `orn:…:user/cli` → `cli`.
///
/// Le JSON, lui, porte l'ORN entier : abréger une piste d'audit dans la sortie
/// machine ferait perdre le locataire exact au moment précis où il compte.
#[cfg(feature = "iam")]
fn orn_court(orn: &obrain_iam::orn::Orn) -> String {
    orn.resource_path().to_string()
}

/// La décision d'audit, en français. `PolicyDecision` n'implémente pas
/// `Display` : la correspondance est écrite ici, et non devinée par un `Debug`.
#[cfg(feature = "iam")]
fn decision(decision: obrain_iam::model::PolicyDecision) -> String {
    use obrain_iam::model::PolicyDecision;
    match decision {
        PolicyDecision::Allow => "accordé".to_string(),
        PolicyDecision::Deny => "refusé".to_string(),
        PolicyDecision::ImplicitDeny => "refus implicite".to_string(),
    }
}

/// `oui` / `non` — plus lisible qu'un booléen dans un tableau.
#[cfg(feature = "iam")]
fn oui_non(valeur: bool) -> String {
    if valeur { "oui" } else { "non" }.to_string()
}

/// Une liste, ou un tiret quand elle est vide : une cellule vide se lit mal.
#[cfg(feature = "iam")]
fn liste(valeurs: &[String]) -> String {
    if valeurs.is_empty() {
        "—".to_string()
    } else {
        valeurs.join(",")
    }
}

/// Un tableau aligné simple (largeur calculée sur le contenu).
///
/// `comfy-table` sert les commandes historiques ; ici, un alignement sur espaces
/// rend un texte identique quel que soit le TTY et se lit tel quel dans un
/// rapport ou un journal — ce que ces sorties deviennent la plupart du temps.
#[cfg(feature = "iam")]
fn tableau(entetes: &[&str], lignes: &[Vec<String>]) -> String {
    let mut largeurs: Vec<usize> = entetes.iter().map(|e| e.chars().count()).collect();
    for ligne in lignes {
        for (i, cellule) in ligne.iter().enumerate() {
            if i < largeurs.len() {
                largeurs[i] = largeurs[i].max(cellule.chars().count());
            }
        }
    }
    let formater = |cellules: &[String]| -> String {
        let mut sortie = String::new();
        for (i, largeur) in largeurs.iter().enumerate() {
            let cellule = cellules.get(i).map_or("", String::as_str);
            sortie.push_str(cellule);
            if i + 1 < largeurs.len() {
                sortie.push_str(&" ".repeat(largeur.saturating_sub(cellule.chars().count()) + 2));
            }
        }
        sortie.trim_end().to_string()
    };

    let entetes: Vec<String> = entetes.iter().map(|e| (*e).to_string()).collect();
    let mut sortie = formater(&entetes);
    sortie.push('\n');
    sortie.push_str(
        &largeurs
            .iter()
            .map(|l| "-".repeat(*l))
            .collect::<Vec<_>>()
            .join("  "),
    );
    sortie.push('\n');
    for ligne in lignes {
        sortie.push_str(&formater(ligne));
        sortie.push('\n');
    }
    sortie
}
