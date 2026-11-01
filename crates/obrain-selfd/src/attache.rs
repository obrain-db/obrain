//! ATTACHE — la machine se branche au moteur, et attend du travail.
//!
//! ── POURQUOI C'EST LE CLIENT QUI APPELLE ────────────────────────────────────
//! La machine d'un utilisateur est derrière un routeur : le moteur ne peut pas
//! l'appeler. Elle ouvre donc une connexion SORTANTE, demande s'il y a du
//! travail, l'exécute, rend son verdict, et recommence. Rien à ouvrir sur le
//! routeur, rien à exposer sur Internet.
//!
//! ── LA POLITIQUE LOCALE GARDE LE DERNIER MOT ────────────────────────────────
//! Ce que le moteur envoie est une DEMANDE, pas un ordre exécutoire. Elle
//! passe par `protocole::executer_outil`, exactement comme un appel venu de la
//! socket ou du pont MCP : même racine confinée, même refus d'exécuter sans
//! autorisation explicite. Un refus REVIENT au moteur, motivé — le silence
//! serait la pire des réponses, parce qu'il ressemble à une panne.
//!
//! ── ET LE JETON NE SERT QU'À ENTRER ─────────────────────────────────────────
//! Le jeton présenté au moteur dit QUI est cette machine ; il ne dit rien de ce
//! qu'elle accepte de faire. C'est la politique, fixée au démarrage et jamais
//! élargie par un appel, qui en décide — le moteur n'a aucun moyen de la
//! desserrer, et c'est la propriété qu'on veut.
//!
//! ── CE QU'ON ANNONCE EST CE QU'ON FERA ──────────────────────────────────────
//! L'annonce d'attachement engage : le moteur en déduit ce qu'il peut demander à
//! cette machine. Elle portait `write` en dur — sur une racine en lecture seule,
//! le moteur proposait une écriture que le nœud refusait ensuite, et l'échec
//! arrivait après coup, du côté de l'utilisateur. L'annonce porte désormais ce
//! que la politique a MESURÉ, plus les limites (racine réelle, délai, octets par
//! flux, version) et les refus motivés. Voir `politique.rs`.
//!
//! ── UNE ANNONCE QUI PART N'EST PAS UNE ANNONCE QUI ARRIVE ───────────────────
//! Annoncer les refus ne suffisait pas : le moteur ne retenait que `capacites` et
//! jetait le reste **en silence** (`crates/obrain-cli/src/remote.rs`, l'incident
//! est décrit dans son en-tête). La machine disait donc ce qu'elle ne faisait pas
//! à un moteur qui n'en gardait rien — et le silence, encore une fois, ressemble
//! à une panne : on croit que la machine ne refuse rien.
//!
//! Le nœud va donc jusqu'au bout de sa phrase : après s'être branché, il
//! **relit ce que le moteur a gardé de lui** ([`verifier_annonce_retenue`]) et
//! dit ce qui manque. C'est une garde, et une garde qui tombe en silence ne
//! garde rien : si la moitié « ce qu'elle ne fait pas » s'est perdue en route, ça
//! se lit sur la sortie d'erreur du nœud, avec le nom de ce qui manque.
//! Best-effort assumé : cette vérification ne fait jamais tomber la boucle. Un
//! moteur qui ne répond pas est une coupure dont on revient, pas une raison de
//! disparaître de la flotte.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::time::Duration;

use crate::protocole::Etat;

/// L'attente du long-poll, côté client. Le moteur la borne de son côté (25 s) ;
/// on demande la même chose, et on considère un silence plus long comme une
/// coupure.
const ATTENTE_S: u64 = 20;

/// Repos entre deux tentatives de rattachement. Croissant, mais BORNÉ : une
/// machine qui dort toute la nuit pendant que le moteur redémarre doit revenir
/// d'elle-même, sans qu'on ait à s'en occuper.
const REPOS_MIN_MS: u64 = 500;
const REPOS_MAX_MS: u64 = 30_000;

fn racine(moteur: &str) -> String {
    moteur.trim_end_matches('/').to_string()
}

/// S'annoncer auprès du moteur. Rend l'identifiant que le moteur nous donne.
///
/// ── CE QUE CETTE MACHINE SAIT FAIRE, TEL QUE SA POLITIQUE LE PERMET ─────────
/// On annonce l'état réel, pas le catalogue : afficher `exec` sur un nœud qui
/// refuse d'exécuter ferait proposer à l'utilisateur une action vouée au refus.
/// Le même défaut existait pour `write`, annoncé **en dur** — sur une racine en
/// lecture seule, le moteur apprenait que la machine savait écrire, et
/// l'utilisateur ne l'apprenait qu'à l'échec. Les capacités viennent de
/// `Politique::annonce()`, qui les MESURE (écriture réelle dans la racine,
/// ouverture réelle du répertoire, `--allow-exec`).
fn attacher(moteur: &str, jeton: &str, machine: &str, etat: &Etat) -> Result<String> {
    // ── L'ANNONCE PORTE AUSSI CE QUE LA MACHINE NE FERA PAS ─────────────────
    // `capacites` garde sa forme (tableau de chaînes) : c'est elle que le moteur
    // lit déjà, et l'interface comme la porte d'acceptation la lisent ainsi.
    // `capacites_refusees` et `limites` s'ajoutent À CÔTÉ — un moteur qui ne les
    // connaît pas les ignore, un moteur qui les lit sait ce que la machine
    // accepte (racine réelle, délai, octets par flux, version) au lieu de le
    // deviner, et sait ce qu'elle REFUSE, motif compris.
    //
    // Que le moteur les garde n'est pas une supposition : [`verifier_annonce_retenue`]
    // le mesure juste après, et le dit quand ce n'est pas le cas.
    //
    // Les deux moitiés sortent d'UNE SEULE mesure (`politique::annonce`) : deux
    // mesures successives pourraient se contredire si la racine changeait d'état
    // entre les deux, et l'annonce porterait alors `write` ouvert ET refusé.
    let (ouvertes, refusees) = etat.politique.annonce();
    let corps = json!({
        "machine": machine,
        "capacites": ouvertes,
        "capacites_refusees": refusees,
        "limites": etat.politique.limites(),
    })
    .to_string();
    let r = obrain_httpmin::post_json(
        &format!("{}/api/remote/attacher", racine(moteur)),
        &corps,
        Some(jeton),
    )
    .with_context(|| format!("attachement au moteur ({moteur})"))?;
    if r.status == 401 || r.status == 403 {
        bail!("le moteur refuse ce jeton — vérifiez le jeton de VOTRE compte");
    }
    let v = r.json().context("réponse d'attachement illisible")?;
    v.get("client_id")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .context("le moteur n'a pas nommé ce client")
}

/// Exécuter UNE demande, et rendre ce qu'on répondra au moteur.
///
/// Ne peut pas échouer : un refus de politique, une erreur d'outil et un succès
/// sont trois RÉSULTATS. Remonter une erreur ici ferait tomber la boucle — donc
/// disparaître la machine — pour une demande qui, elle, a bien reçu sa réponse.
fn executer(etat: &mut Etat, outil: &str, arguments: &Value) -> Value {
    match crate::protocole::executer_outil(etat, outil, arguments, None) {
        Ok(v) => json!({ "ok": true, "resultat": v }),
        Err(e) => {
            // Le motif du refus est ce qui rend l'échec ACTIONNABLE : « hors de
            // la racine autorisée » se corrige, « erreur » ne se corrige pas.
            let motif = e
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("refus sans motif")
                .to_string();
            json!({ "ok": false, "refus": motif })
        }
    }
}

fn rendre_verdict(
    moteur: &str,
    jeton: &str,
    client_id: &str,
    appel_id: &str,
    resultat: Value,
) -> Result<()> {
    let corps =
        json!({ "client_id": client_id, "appel_id": appel_id, "resultat": resultat }).to_string();
    obrain_httpmin::post_json(&format!("{}/api/remote/resultat", racine(moteur)), &corps, Some(jeton))
        .context("dépôt du verdict")?;
    Ok(())
}

/// Ce que le moteur a GARDÉ de notre annonce — et ce qu'il a laissé en route.
///
/// ── POURQUOI LE NŒUD RELIT, AU LIEU DE CROIRE ───────────────────────────────
/// Parce que l'annonce traversait trois étapes (le corps, le registre, le
/// journal) et que la seconde en jetait la moitié sans le dire. Un nœud qui se
/// contente d'envoyer ne peut pas distinguer « le moteur sait que je refuse
/// `exec` » de « le moteur croit que je ne refuse rien » — et c'est cette
/// différence qui fait proposer à l'utilisateur un geste voué au refus.
///
/// On interroge donc la liste des machines DU COMPTE (la seule route qui rende
/// cette information, et elle est cloisonnée), on retrouve la nôtre par son
/// identifiant, et on regarde le verdict du moteur sur sa complétude.
///
/// ── ELLE NE FAIT JAMAIS TOMBER LA BOUCLE ────────────────────────────────────
/// Tout échec ici est une PHRASE sur la sortie d'erreur, jamais un `Err` : une
/// machine qui disparaît de la flotte parce qu'une vérification facultative a
/// échoué serait un remède pire que le mal. Un nœud qui ne peut pas vérifier le
/// DIT ; il ne fait pas semblant d'avoir vérifié.
fn verifier_annonce_retenue(moteur: &str, jeton: &str, client_id: &str, etat: &Etat) {
    let url = format!("{}/api/remote/clients", racine(moteur));
    let r = match obrain_httpmin::get(&url, &[("client", client_id)], Some(jeton)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[attache] annonce non vérifiable : le moteur n'a pas répondu à la relecture \
                 ({e:#}) — on ne sait donc pas s'il a gardé ce que cette machine REFUSE"
            );
            return;
        }
    };
    let Ok(v) = r.json() else {
        eprintln!("[attache] annonce non vérifiable : relecture illisible (HTTP {})", r.status);
        return;
    };
    let Some(nous) = v
        .get("clients")
        .and_then(|c| c.as_array())
        .and_then(|a| a.iter().find(|c| c.get("id").and_then(|i| i.as_str()) == Some(client_id)))
    else {
        eprintln!(
            "[attache] annonce non vérifiable : le moteur ne rend pas « {client_id} » dans la \
             liste de ce compte — il a accepté cette machine sans la garder"
        );
        return;
    };

    // Le verdict du moteur, quand il sait le rendre. Un moteur antérieur ne
    // connaît pas ce bloc : « il ne dit rien » n'est pas « tout va bien », et on
    // ne l'écrit pas comme si c'était la même chose.
    let Some(annonce) = nous.get("annonce") else {
        let refus = etat.politique.verdicts_retenus().iter().filter(|v| !v.exercable).count();
        eprintln!(
            "[attache] ce moteur ne dit pas ce qu'il a gardé de l'annonce : il ne retient \
             peut-être que les capacités, et la moitié « ce que cette machine REFUSE » \
             ({refus} refus mesuré(s) ici) peut avoir été perdue en route"
        );
        return;
    };
    if annonce.get("complete").and_then(Value::as_bool) != Some(true) {
        let manquante = annonce
            .get("manquante")
            .and_then(|m| m.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_else(|| "on ne sait pas quoi".to_string());
        eprintln!(
            "[attache] le moteur a gardé l'annonce INCOMPLÈTE : « {manquante} » n'est pas \
             arrivé jusqu'à lui. Cette machine dit ce qu'elle REFUSE ; tant que ce n'est pas \
             gardé, un geste voué au refus peut encore être proposé — et l'échec arrivera \
             après coup."
        );
        return;
    }
    let verdicts = etat.politique.verdicts_retenus();
    let ouvertes = verdicts.iter().filter(|v| v.exercable).count();
    let refusees = verdicts.iter().filter(|v| !v.exercable).count();
    eprintln!(
        "[attache] annonce gardée en entier par le moteur ({ouvertes} capacité(s) exercable(s), \
         {refusees} refus motivé(s), limites comprises)"
    );
}

/// La boucle : s'annoncer, attendre du travail, l'exécuter, recommencer.
///
/// Ne rend la main que sur une erreur DÉFINITIVE (un jeton refusé) : tout le
/// reste — moteur éteint, réseau coupé, machine en veille — est une coupure
/// dont on revient.
pub fn boucle(etat: &mut Etat, moteur: &str, jeton: &str, machine: &str) -> Result<()> {
    let mut repos = REPOS_MIN_MS;
    let mut client_id: Option<String> = None;

    loop {
        // ── 1. être attaché ────────────────────────────────────────────────
        let id = match client_id.clone() {
            Some(id) => id,
            None => match attacher(moteur, jeton, machine, etat) {
                Ok(id) => {
                    eprintln!("[attache] « {machine} » branchée au moteur ({id})");
                    // On vient d'annoncer : on mesure ce que le moteur en a
                    // retenu, et on le dit. Voir `verifier_annonce_retenue`.
                    verifier_annonce_retenue(moteur, jeton, &id, etat);
                    repos = REPOS_MIN_MS;
                    client_id = Some(id.clone());
                    id
                }
                Err(e) => {
                    let texte = format!("{e:#}");
                    if texte.contains("refuse ce jeton") {
                        // Réessayer avec un jeton refusé ne peut que le refuser
                        // encore : on le dit, et on s'arrête.
                        return Err(e);
                    }
                    eprintln!("[attache] moteur injoignable ({texte}) — nouvelle tentative dans {repos} ms");
                    std::thread::sleep(Duration::from_millis(repos));
                    repos = (repos * 2).min(REPOS_MAX_MS);
                    continue;
                }
            },
        };

        // ── 2. demander du travail (attente bornée côté moteur) ─────────────
        let attente = ATTENTE_S.to_string();
        let r = obrain_httpmin::get(
            &format!("{}/api/remote/ordres", racine(moteur)),
            &[("client", id.as_str()), ("attente", attente.as_str())],
            Some(jeton),
        );
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[attache] canal coupé ({e:#}) — on se rebranche");
                client_id = None;
                std::thread::sleep(Duration::from_millis(repos));
                repos = (repos * 2).min(REPOS_MAX_MS);
                continue;
            }
        };
        repos = REPOS_MIN_MS;

        match r.status {
            // Rien à faire : c'est le rythme NORMAL du canal, pas une anomalie.
            204 => continue,
            // Le moteur ne nous connaît plus (il a redémarré, ou nous a oubliés
            // après un long silence) : on se réannonce plutôt que de boucler.
            403 | 404 => {
                eprintln!("[attache] le moteur ne nous connaît plus — on se réannonce");
                client_id = None;
                continue;
            }
            200 => {}
            autre => {
                eprintln!("[attache] réponse inattendue du moteur ({autre}) — on réessaie");
                std::thread::sleep(Duration::from_millis(REPOS_MIN_MS));
                continue;
            }
        }

        let Ok(ordre) = r.json() else {
            eprintln!("[attache] demande illisible — ignorée");
            continue;
        };
        let (Some(appel_id), Some(outil)) = (
            ordre.get("appel_id").and_then(|x| x.as_str()),
            ordre.get("outil").and_then(|x| x.as_str()),
        ) else {
            eprintln!("[attache] demande sans appel ni outil — ignorée");
            continue;
        };
        let arguments = ordre.get("arguments").cloned().unwrap_or_else(|| json!({}));

        // ── 3. exécuter SOUS LA POLITIQUE LOCALE, et rendre le verdict ──────
        eprintln!("[attache] demande reçue : {outil}");
        let verdict = executer(etat, outil, &arguments);
        if let Err(e) = rendre_verdict(moteur, jeton, &id, appel_id, verdict) {
            // Le travail est fait ; c'est le verdict qui s'est perdu. On le dit
            // — sinon l'utilisateur verrait un délai dépassé pour une action
            // qui a bel et bien eu lieu, ce qui est le pire des deux mondes.
            eprintln!("[attache] verdict non remis ({e:#}) — l'action a pourtant eu lieu");
            client_id = None;
        }
    }
}
