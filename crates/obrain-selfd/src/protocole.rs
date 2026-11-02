//! L'état partagé d'un nœud, et sa surface JSON-RPC native.
//!
//! Un seul état, trois façons de l'atteindre : la socket locale (`selfd serve`),
//! la surface MCP (`selfd mcp`, pour que le harness monte le nœud), et stdio
//! direct (`ssh machine selfd`, le chemin sans démon). Les trois appellent **le
//! même** `traiter` : une capacité ajoutée ici est disponible partout, et il n'y
//! a pas trois implémentations à garder d'accord.
//!
//! # `caps` et `status` disent ce que le nœud FERA, mesuré sur sa racine
//!
//! Ces deux réponses portaient un bloc `capabilities` écrit en dur
//! (`"fsWrite": true` sur un nœud dont la racine peut être en lecture seule) : le
//! modèle qui s'y fiait proposait un geste que le nœud refusait ensuite. Elles
//! portent maintenant les capacités que `politique::Politique` a mesurées, les
//! refus avec leur motif, et les limites du nœud — voir `politique.rs` et
//! [`verifier_capacites`].

use std::time::Instant;

use anyhow::Result;
use obrain_garde::{Garde, Reglages};
use serde_json::{Value, json};

use crate::agent;
use crate::chat;
use crate::identite::Identite;
use crate::outils;
use crate::politique::Politique;
use crate::soi;

/// Tout ce qu'un appel peut lire ou changer.
pub struct Etat {
    pub politique: Politique,
    pub identite: Identite,
    pub debut: Instant,
    /// Comment ce nœud est atteint (`stdio`, `socket`, `mcp`) — rapporté, parce
    /// qu'un même binaire ne se comporte pas pareil selon qui l'appelle.
    pub mode: &'static str,
    /// Ce nœud exige-t-il un jeton sur CETTE surface ?
    ///
    /// Fixé au démarrage, jamais changé par un appel : un déverrouillage global
    /// (« le premier client autorisé ouvre le nœud pour tous ») serait un état
    /// partagé qui autorise ce qu'aucun client n'a présenté. L'authentification
    /// est donc **sans état** : chaque appel porte son jeton, ou n'en a pas.
    pub exiger_jeton: bool,
    /// URL du modèle OpenAI-compatible du chat local.
    pub llm: String,
    /// L'historique du chat — **propre à ce nœud**, sous son state-dir.
    pub historique: chat::Historique,
    /// URL du moteur : c'est **le socle** que le harness sert.
    pub moteur: String,
    /// Notes locales du nœud (le self n'accepte pas d'écriture).
    pub notes: std::path::PathBuf,
}

impl Etat {
    pub fn nouveau(
        politique: Politique,
        identite: Identite,
        mode: &'static str,
        secret: Option<&str>,
        llm: String,
        historique: chat::Historique,
    ) -> Self {
        Self {
            politique,
            identite,
            debut: Instant::now(),
            mode,
            exiger_jeton: secret.is_some(),
            llm,
            historique,
            moteur: std::env::var("SELFD_MOTEUR")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:8432".to_string()),
            notes: std::path::PathBuf::new(),
        }
    }
}

/// Un appel entrant.
pub struct Appel {
    pub id: Option<Value>,
    pub methode: String,
    pub params: Value,
}

/// Compare deux secrets en temps constant.
///
/// Une comparaison naïve s'arrête au premier octet différent : le temps de
/// réponse révèle alors combien d'octets sont justes, et le secret se devine
/// octet par octet. C'est le seul endroit du nœud où un attaquant peut mesurer
/// quelque chose.
pub fn secret_egal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// La réponse d'erreur quand le jeton manque ou ne correspond pas.
fn refus_jeton(methode: &str) -> Value {
    json!({
        "code": -32001,
        "message": format!(
            "jeton requis pour « {methode} » : cette surface est gardée par un secret \
             (params.token). Le nœud compare en temps constant et ne dit jamais \
             combien d'octets sont justes."
        )
    })
}

/// Réécrit les capacités d'une réponse d'identité d'après ce que la politique a
/// MESURÉ.
///
/// `pub(crate)` à dessein : la surface HTTP (`ecoute.rs`, `GET /api/status`)
/// appelle `outils::statut` **directement**, sans passer par ici — elle porte donc
/// encore le bloc hérité. Un correctif d'une ligne y suffit :
/// `verifier_capacites(&mut v, &s.politique)`.
///
/// `outils::identite` (et donc `outils::statut`) porte un bloc `capabilities`
/// **écrit en dur** : `"fsWrite": true`, `"fsRead": true`, `"list": true`. Ces
/// valeurs datent d'avant la politique de racine, et elles mentent : sur une
/// racine en lecture seule, elles annoncent une écriture que `outils::write`
/// refusera. `outils.rs` n'est pas dans le périmètre de ce chantier ; refuser d'y
/// toucher ne dispense pas de refuser d'annoncer faux.
///
/// Ce qui est ajouté ici a la forme de l'annonce au moteur : `capacites` (tableau
/// de chaînes, jamais un objet), `capacites_refusees` avec leur motif, et
/// `limites`. Le nœud dit donc la MÊME chose par `caps`, par `status` et par son
/// attachement.
pub(crate) fn verifier_capacites(v: &mut Value, p: &Politique) {
    let verdicts = p.verdicts();
    let exercable = |nom: &str| verdicts.iter().any(|c| c.nom == nom && c.exercable);
    v["capabilities"] = json!({
        "exec": exercable("exec"),
        "fsRead": exercable("read"),
        "fsWrite": exercable("write"),
        "list": exercable("list"),
    });
    v["capacites"] = json!(
        verdicts
            .iter()
            .filter(|c| c.exercable)
            .map(|c| c.nom)
            .collect::<Vec<_>>()
    );
    v["capacites_refusees"] = json!(
        verdicts
            .iter()
            .filter(|c| !c.exercable)
            .map(crate::politique::Verdict::en_json)
            .collect::<Vec<_>>()
    );
    v["limites"] = p.limites();
}

/// Exécute l'outil demandé, **sans** se soucier du protocole appelant.
///
/// Rend `Ok(valeur)` ou `Err(erreur json-rpc)`. La garde du verrou est **ici**,
/// en un seul point : MCP, socket et stdio en héritent, et un outil ajouté
/// demain aussi.
pub fn executer_outil(
    etat: &mut Etat,
    methode: &str,
    params: &Value,
    secret: Option<&str>,
) -> Result<Value, Value> {
    // ── LA GARDE, EN UN SEUL POINT, ET SANS ÉTAT ────────────────────────────
    // Elle couvre TOUTES les méthodes — y compris celles qui ne font que
    // décrire le nœud : savoir à quelle machine on parle est déjà une
    // information. Un outil ajouté demain l'hérite sans qu'on y pense.
    if etat.exiger_jeton {
        let attendu = secret.unwrap_or_default();
        let fourni = params
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !secret_egal(fourni, attendu) {
            return Err(refus_jeton(methode));
        }
    }

    match methode {
        // ── LES CAPACITÉS VÉRIFIÉES, PAS LE CATALOGUE ───────────────────────
        // Voir `verifier_capacites` : le bloc `capabilities` hérité d'`outils.rs`
        // est réécrit d'après les verdicts MESURÉS. Sans cela, `caps` annonçait
        // `fsWrite: true` sur une racine en lecture seule — et l'annonçait EN MÊME
        // TEMPS que `capacites_refusees` disait le contraire. Deux vérités dans une
        // réponse, c'est une de trop.
        "initialize" | "caps" => {
            let mut v = outils::identite(&etat.politique, &etat.identite, etat.mode);
            verifier_capacites(&mut v, &etat.politique);
            Ok(v)
        }
        "status" => {
            let mut v = outils::statut(
                &etat.politique,
                &etat.identite,
                etat.debut,
                etat.mode,
                &etat.moteur,
            );
            // `status` porte le MÊME bloc hérité : le corriger ici aussi, sans quoi
            // la vérité dépendrait de la méthode appelée — le défaut exact que ce
            // module combat ailleurs (« deux chemins pour un même travail ne
            // restent pas d'accord »).
            verifier_capacites(&mut v, &etat.politique);
            Ok(v)
        }
        "exec" => outils::exec(&etat.politique, params)
            .map_err(|e| json!({"code": -32000, "message": format!("{e:#}")})),
        "read" => outils::read(&etat.politique, params)
            .map_err(|e| json!({"code": -32000, "message": format!("{e:#}")})),
        "write" => outils::write(&etat.politique, params)
            .map_err(|e| json!({"code": -32000, "message": format!("{e:#}")})),
        "list" => outils::list(&etat.politique, params)
            .map_err(|e| json!({"code": -32000, "message": format!("{e:#}")})),
        // ── LE CHAT DU NŒUD ─────────────────────────────────────────────────
        // Le contexte vient du NŒUD SEUL (identité, politique, état, racine) :
        // aucune lecture du graphe global, aucune clé de base. Un nœud qui
        // répondrait depuis le graphe global mentirait sur ce qu'il est.
        "chat" => {
            let question = params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if question.is_empty() {
                return Err(json!({"code": -32602, "message": "« message » requis"}));
            }
            let systeme = chat::contexte(&etat.politique, &etat.identite, etat.debut, etat.mode);
            let historique = etat.historique.derniers(12);
            etat.historique.ajouter("user", &question);
            // Un tour de conversation passe par la GARDE : budget de la
            // question, détection de répétition, arrêt. Le rappel de fragment
            // rend un motif pour arrêter — ici personne n'écoute en flux, donc
            // il rend toujours `None` (continuer).
            let mut garde = Garde::nouvelle(Reglages::default());
            match chat::converser(
                &etat.llm,
                &systeme,
                &historique,
                &question,
                &mut garde,
                |_| None,
            ) {
                Ok(tour) => {
                    // Un arrêt se DIT, il ne se fait pas passer pour une fin : le
                    // motif et sa phrase sont dans la réponse, pas seulement dans
                    // un journal que personne ne lit.
                    etat.historique.ajouter("assistant", &tour.texte);
                    Ok(json!({
                        "reponse": tour.texte,
                        "modele": etat.llm,
                        "arret": tour.arret.as_ref().map(|m| m.code()),
                        "arretPhrase": tour.arret.as_ref().map(|m| m.phrase()),
                    }))
                }
                Err(e) => Err(json!({"code": -32000, "message": format!("{e:#}")})),
            }
        }
        // ── LE HARNESS À L'ŒUVRE ────────────────────────────────────────────
        // `chat` CONVERSE (un tour, sans outil) ; `run` DÉROULE (le modèle
        // décide, le nœud exécute, on recommence). La distinction est celle qui
        // sépare un assistant d'un harness, et elle est visible dans les noms.
        "run" => {
            let objectif = params
                .get("objectif")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if objectif.is_empty() {
                return Err(json!({"code": -32602, "message": "« objectif » requis"}));
            }
            let max_etapes = params
                .get("maxSteps")
                .and_then(|v| v.as_u64())
                .unwrap_or(12)
                .clamp(1, 40) as usize;

            let socle = soi::Socle::lire(&etat.moteur, 10_000);
            let mut trace: Vec<Value> = Vec::new();
            // Une garde NEUVE par objectif : le budget est celui de la question,
            // il ne se partage pas entre deux appels du même nœud.
            let mut garde = Garde::nouvelle(Reglages::default());
            let reponse = {
                let mut cb = |ev: agent::Evenement| -> anyhow::Result<()> {
                    match ev {
                        agent::Evenement::Texte(t) => trace.push(json!({"type": "texte", "t": t})),
                        agent::Evenement::Acte { etape, outil, argument } => trace.push(
                            json!({"type": "acte", "etape": etape, "outil": outil, "argument": argument}),
                        ),
                        agent::Evenement::Rend { etape, ok, apercu } => trace.push(
                            json!({"type": "rend", "etape": etape, "ok": ok, "apercu": apercu}),
                        ),
                        agent::Evenement::Fin { etapes, ms } => {
                            trace.push(json!({"type": "fin", "etapes": etapes, "ms": ms}))
                        }
                        agent::Evenement::Borne { etapes } => {
                            trace.push(json!({"type": "borne", "etapes": etapes}))
                        }
                        // La GARDE a arrêté la boucle : cause distincte du
                        // budget d'étapes, et elle se dit avec son code.
                        agent::Evenement::Arret { code, phrase } => {
                            trace.push(json!({"type": "arret", "code": code, "phrase": phrase}))
                        }
                    }
                    Ok(())
                };
                match agent::derouler(
                    &etat.llm,
                    &socle,
                    &etat.politique,
                    &etat.identite,
                    &etat.moteur,
                    &etat.notes,
                    &objectif,
                    max_etapes,
                    &mut garde,
                    &mut cb,
                ) {
                    Ok(r) => r,
                    Err(e) => return Err(json!({"code": -32000, "message": format!("{e:#}")})),
                }
            };
            etat.historique.ajouter("user", &objectif);
            etat.historique.ajouter("assistant", &reponse);
            Ok(json!({
                "reponse": reponse,
                "self": socle.self_qn,
                "selfJoignable": socle.joignable,
                "etapes": trace.iter().filter(|t| t["type"] == "acte").count(),
                "trace": trace,
            }))
        }
        "shutdown" => Ok(json!({ "ok": true })),
        autre => Err(json!({"code": -32601, "message": format!("méthode inconnue : {autre}")})),
    }
}

/// Le tour complet d'un appel : rend la réponse, ou `None` pour une notification.
pub fn repondre(etat: &mut Etat, appel: &Appel, secret: Option<&str>) -> Option<Value> {
    let id = appel.id.clone()?;
    // Une notification (sans `id`) ne reçoit JAMAIS de réponse : en rendre une
    // ferait diverger le protocole pour un client qui compte les réponses.
    Some(
        match executer_outil(etat, &appel.methode, &appel.params, secret) {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
        },
    )
}

/// Lit un objet JSON-RPC d'une ligne, en tolérant l'absence de `id`.
pub fn analyser(ligne: &str) -> Result<Appel, String> {
    let v: Value = serde_json::from_str(ligne).map_err(|e| format!("trame illisible : {e}"))?;
    Ok(Appel {
        id: v.get("id").cloned(),
        methode: v
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string(),
        params: v.get("params").cloned().unwrap_or_else(|| json!({})),
    })
}
