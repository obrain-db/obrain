//! La boucle du harness — un nœud qui **agit**, ancré sur le self.
//!
//! # Ce qui sépare un chat d'un harness
//!
//! Un chat rend une réponse. Un harness **déroule** : il regarde la machine,
//! décide d'un acte, l'exécute, regarde le résultat, recommence — et s'arrête
//! quand il a fini ou quand son budget est épuisé. La différence n'est pas le
//! modèle, c'est la boucle.
//!
//! Le modèle local sait appeler des outils nativement (vérifié :
//! `finish_reason: tool_calls` sur Qwen3.5-9B servi par llama-server). On emploie
//! donc le protocole standard, pas une convention inventée à lire au fond d'un
//! texte — une convention maison aurait demandé au modèle de deviner une syntaxe,
//! et son échec serait passé pour une incapacité à raisonner.
//!
//! # Ce sur quoi la boucle est ancrée
//!
//! Le prompt système est **le self** (identité, mondes, foyers, ce qui est en
//! attente) **et** le nœud (identité stable, politique effective, machine). Un
//! harness ancré sur rien décide au hasard ; ancré sur le graphe global, il sait
//! seulement ce qui existe. Ancré sur le self, il sait **qui il sert et ce qui est
//! en cours** — et c'est ce qui rend une décision justifiable.
//!
//! # Les bornes, et pourquoi elles sont là
//!
//! Un budget d'étapes, un plafond de sortie par acte, et un arrêt net à
//! l'échéance. Sans elles, un modèle qui boucle sur un outil qui échoue occupe la
//! machine indéfiniment : le défaut n'est pas visible dans les journaux, il l'est
//! seulement dans la charge. Une boucle qui s'arrête **le dit** au lieu de rendre
//! une réponse partielle pour une réponse finie.

use std::path::Path;

use anyhow::{bail, Context, Result};
use obrain_garde::{Garde, Motif, index_appel_valide};
use serde_json::{json, Value};

use crate::identite::Identite;
use crate::outils;
use crate::politique::Politique;
use crate::soi::{self, Socle};

/// La boucle s'arrête sur la garde — **et le dit**.
///
/// Le texte rendu porte le motif, parce qu'un texte partiel présenté comme une
/// conclusion serait le pire des mensonges : l'opérateur croirait l'objectif
/// atteint. C'est la même règle que la borne d'étapes (`Evenement::Borne`), qui
/// existait déjà.
fn borner_sur_garde(
    motif: Motif,
    sur: &mut dyn FnMut(Evenement) -> Result<()>,
    derniere_reponse: &str,
) -> String {
    let phrase = motif.phrase();
    let _ = sur(Evenement::Arret { code: motif.code(), phrase: phrase.clone() });
    format!(
        "{derniere_reponse}\n\n[{phrase} — la boucle s'arrête ici et le dit : \
         l'objectif n'est pas conclu.]"
    )
}

/// Ce que la boucle raconte en avançant.
///
/// Les surfaces (SSE, CLI, MCP) en font ce qu'elles veulent : c'est ce qui rend
/// le déroulé **observable** au lieu d'être une boîte noire qui rend un texte.
pub enum Evenement<'a> {
    /// Un morceau de texte du modèle (réponse en cours).
    Texte(&'a str),
    /// Le modèle a décidé d'un acte.
    Acte { etape: usize, outil: &'a str, argument: &'a str },
    /// L'acte a rendu.
    Rend { etape: usize, ok: bool, apercu: &'a str },
    /// Aucun outil : le modèle a répondu.
    Fin { etapes: usize, ms: u64 },
    /// La boucle s'est arrêtée sur une borne, et le dit.
    Borne { etapes: usize },
    /// **La garde a arrêté la boucle** — client parti, budget épuisé, ou modèle
    /// qui se répète. Distinct de [`Evenement::Borne`] parce que ce ne sont pas
    /// les mêmes causes : `Borne` est le budget d'ÉTAPES que l'appelant a donné,
    /// `Arret` est une garantie du moteur. Les confondre ferait croire qu'un
    /// modèle qui boucle a « atteint son nombre d'étapes ».
    Arret { code: &'static str, phrase: String },
}

/// Ce à quoi la boucle a accès.
pub struct Greement<'a> {
    pub politique: &'a Politique,
    pub socle: &'a Socle,
    pub moteur: &'a str,
    /// Fichier de notes **locales au nœud** — le self n'accepte pas d'écriture.
    pub notes: &'a Path,
}

/// Un appel d'outil assemblé depuis le flux.
#[derive(Debug, Clone, Default)]
struct Appel {
    id: String,
    nom: String,
    arguments: String,
}

/// Le résultat d'un tour de modèle.
#[derive(Debug, Default)]
struct Tour {
    texte: String,
    appels: Vec<Appel>,
    raison_fin: Option<String>,
}

/// Les outils présentés au modèle. Ils décrivent des actes **sur la machine de ce
/// nœud** et des lectures **du self** — jamais autre chose.
fn outils_modele() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "exec",
                "description": "Exécute un programme sur la machine de ce nœud et rend code de sortie, stdout et stderr. \
                                Peut être refusé si le nœud n'a pas été démarré avec --allow-exec.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string", "description": "Le programme (ex. `df`), pas une ligne de shell." },
                        "args": { "type": "array", "items": { "type": "string" } },
                        "cwd": { "type": "string", "description": "Répertoire, sous la racine du nœud." },
                        "shell": { "type": "boolean", "description": "Passer par /bin/sh -c (à demander explicitement)." }
                    },
                    "required": ["cmd"]
                }
            }
        },
        { "type": "function", "function": {
            "name": "read",
            "description": "Lit un fichier sur la machine de ce nœud (chemin sous la racine).",
            "parameters": { "type": "object", "properties": {
                "path": { "type": "string" }, "maxBytes": { "type": "integer" } }, "required": ["path"] } } },
        { "type": "function", "function": {
            "name": "write",
            "description": "Écrit un fichier sur la machine de ce nœud (chemin sous la racine).",
            "parameters": { "type": "object", "properties": {
                "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } } },
        { "type": "function", "function": {
            "name": "list",
            "description": "Liste un répertoire de la machine de ce nœud.",
            "parameters": { "type": "object", "properties": { "path": { "type": "string" } } } } },
        { "type": "function", "function": {
            "name": "self_requete",
            "description": "Exécute une requête GQL sur la base du SELF : son état (SelfState), ses procédures, ses buts, ses mondes. \
                            À employer pour savoir ce que le self poursuit ou ce qu'il sait déjà.",
            "parameters": { "type": "object", "properties": {
                "gql": { "type": "string", "description": "La requête GQL." } }, "required": ["gql"] } } },
        { "type": "function", "function": {
            "name": "self_note",
            "description": "Écrit une note DANS LA MÉMOIRE LOCALE DU NŒUD. Ce n'est pas une écriture dans le self : \
                            le moteur n'offre aucune route d'écriture. Sers-t'en pour garder une trace de ce que tu as constaté.",
            "parameters": { "type": "object", "properties": {
                "texte": { "type": "string" } }, "required": ["texte"] } } }
    ])
}

/// Exécute un outil demandé par le modèle, et rend un texte à lui redonner.
fn executer(nom: &str, arguments: &str, g: &Greement) -> Result<String> {
    let args: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments)
            .with_context(|| format!("arguments illisibles pour « {nom} » : {arguments}"))?
    };
    let v = match nom {
        "exec" => outils::exec(g.politique, &args)?,
        "read" => outils::read(g.politique, &args)?,
        "write" => outils::write(g.politique, &args)?,
        "list" => outils::list(g.politique, &args)?,
        "self_requete" => {
            let gql = args
                .get("gql")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("self_requete : « gql » requis"))?;
            let Some(store) = g.socle.store.as_deref() else {
                bail!("self_requete : le socle est injoignable, aucune base à interroger");
            };
            soi::requete(g.moteur, store, gql, 10_000)?
        }
        "self_note" => {
            let texte = args.get("texte").and_then(|v| v.as_str()).unwrap_or("").trim();
            if texte.is_empty() {
                bail!("self_note : « texte » requis");
            }
            let ligne = json!({
                "t": crate::identite::maintenant_ms(),
                "texte": texte,
            });
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(g.notes)
                .with_context(|| format!("notes locales ({})", g.notes.display()))?;
            writeln!(f, "{ligne}")?;
            json!({ "note": "gardée localement", "fichier": g.notes.display().to_string() })
        }
        autre => bail!("outil inconnu du modèle : {autre}"),
    };
    // Le résultat est rendu en TEXTE : c'est ce que le protocole attend, et ce
    // qui traverse tous les modèles sans convention supplémentaire.
    Ok(serde_json::to_string(&v).unwrap_or_else(|_| v.to_string()))
}

/// Déroule un objectif : le modèle décide, le nœud exécute, jusqu'à la fin ou la
/// borne.
#[allow(clippy::too_many_arguments)]
pub fn derouler(
    llm: &str,
    socle: &Socle,
    politique: &Politique,
    identite: &Identite,
    moteur: &str,
    notes: &Path,
    objectif: &str,
    max_etapes: usize,
    garde: &mut Garde,
    sur: &mut dyn FnMut(Evenement) -> Result<()>,
) -> Result<String> {
    let g = Greement { politique, socle, moteur, notes };

    let systeme = format!(
        "Tu es un harness de machine : tu AGIS sur la machine où tu tournes, et tu rends compte.\n\
         Tu sers le self décrit ci-dessous. Tu ne prétends jamais avoir fait ce que tu n'as pas fait : \
         si un outil échoue ou est refusé, tu le dis et tu changes d'approche.\n\
         Tu réponds dans la langue de l'objectif.\n\n{}\n\n{}",
        socle.en_texte(),
        crate::chat::contexte(politique, identite, std::time::Instant::now(), "agent"),
    );

    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": systeme }),
        json!({ "role": "user", "content": objectif }),
    ];
    let outils = outils_modele();
    let debut = std::time::Instant::now();
    let mut derniere_reponse = String::new();

    for etape in 1..=max_etapes {
        // ── LA GARDE SE CONSULTE AVANT CHAQUE TOUR ──────────────────────────
        // Trois raisons de ne pas rappeler le modèle : le client est parti
        // pendant que les outils du tour précédent s'exécutaient, le budget de
        // la question est épuisé (c'est une SOMME, pas un délai par tour), ou le
        // tour précédent a bouclé. Aucune n'était vérifiée ici.
        if let Err(motif) = garde.tour() {
            return Ok(borner_sur_garde(motif, &mut *sur, &derniere_reponse));
        }
        // La prose d'un tour ne déborde pas sur le suivant : deux tours qui se
        // ressemblent sont le lot d'une conversation, pas une boucle.
        garde.nouvelle_page();
        let corps = json!({
            "messages": messages,
            "tools": outils,
            "tool_choice": "auto",
            "stream": true,
            "max_tokens": 2048,
        })
        .to_string();

        // ── LE TOUR DE MODÈLE, EN FLUX ──────────────────────────────────────
        // On assemble les appels d'outils depuis les deltas : le texte s'affiche
        // pendant qu'il s'écrit, ET les actes restent exploitables. Attendre la
        // fin du flux pour tout traiter priverait l'opérateur de la seule chose
        // qui lui dit que ça avance.
        let mut tour = Tour::default();
        let mut erreur: Option<anyhow::Error> = None;
        let url = format!("{}/chat/completions", llm.trim_end_matches('/'));
        // La commande d'arrêt lue AVANT chaque trame : un client parti ne se
        // voit pas dans le texte du modèle, il se voit dans le drapeau partagé.
        let veille = garde.veille();
        let resultat = obrain_httpmin::post_sse_supervise(
            &url,
            &corps,
            None,
            crate::chat::DELAI_MODELE,
            |_nom, donnees| {
                if donnees.trim() == "[DONE]" {
                    return true;
                }
                let Ok(v) = serde_json::from_str::<Value>(donnees) else {
                    return true;
                };
                if let Some(e) = v.get("error") {
                    erreur = Some(anyhow::anyhow!("le modèle a signalé : {e}"));
                    return true; // le verdict est rendu après la fin de lecture
                }
                let choix = &v["choices"][0];
                if let Some(fr) = choix["finish_reason"].as_str() {
                    tour.raison_fin = Some(fr.to_string());
                }
                let delta = &choix["delta"];
                if let Some(c) = delta["content"].as_str()
                    && !c.is_empty()
                {
                    {
                        tour.texte.push_str(c);
                        // Un modèle qui se répète écrivait jusqu'au plafond, ce
                        // qui se lit comme une génération infinie. On arrête la
                        // LECTURE, et le tour ne sera pas exécuté.
                        if garde.prose(c).is_err() {
                            return false;
                        }
                        if let Err(e) = sur(Evenement::Texte(c)) {
                            erreur = Some(e);
                            return false;
                        }
                    }
                }
                if let Some(appels) = delta["tool_calls"].as_array() {
                    for tc in appels {
                        let i = tc["index"].as_u64().unwrap_or(0) as usize;
                        // ── UN INDEX ABERRANT N'ALLOCUE PAS DES GO ──────────
                        // `i` vient du flux distant : `while appels.len() <= i`
                        // avec un index à 1e9 allouait un milliard de structures
                        // (~70 Go) et faisait avorter le processus. Le correctif
                        // existait dans une seule des trois boucles du dépôt.
                        if !index_appel_valide(i, garde.reglages().max_appels_par_tour) {
                            continue;
                        }
                        while tour.appels.len() <= i {
                            tour.appels.push(Appel::default());
                        }
                        if let Some(id) = tc["id"].as_str() {
                            tour.appels[i].id = id.to_string();
                        }
                        if let Some(f) = tc["function"]["name"].as_str() {
                            tour.appels[i].nom.push_str(f);
                        }
                        if let Some(a) = tc["function"]["arguments"].as_str() {
                            tour.appels[i].arguments.push_str(a);
                        }
                    }
                }
                true
            },
            || veille.continuer(),
        );
        if let Some(e) = erreur {
            return Err(e);
        }
        let arrete_par_nous = resultat.context("tour de modèle")?;
        // ── QUI A ARRÊTÉ ? ON LE DIT, ON NE LE DEVINE PAS ───────────────────
        // Le rappel a pu décider (répétition) ; sinon la lecture a cessé parce
        // que la veille a dit non (client parti ou budget) — et c'est `tour()`
        // qui pose alors le motif, dans le même ordre de priorité que partout.
        if arrete_par_nous && garde.motif().is_none() {
            let _ = garde.tour();
        }
        if let Some(motif) = garde.motif().cloned() {
            // Un tour arrêté n'est PAS exécuté : rejouer les outils d'un tour que
            // le modèle n'a pas fini d'écrire reviendrait à agir sur un
            // raisonnement tronqué. C'est la règle déjà tenue par le chat du
            // moteur (« un motif dégénéré arrête la lecture, l'annonce, et
            // n'exécute aucun outil »).
            if !tour.texte.trim().is_empty() {
                derniere_reponse.clone_from(&tour.texte);
            }
            return Ok(borner_sur_garde(motif, &mut *sur, &derniere_reponse));
        }

        // Aucun outil demandé : le modèle a répondu, la boucle est finie.
        if tour.appels.is_empty() {
            let etapes = etape;
            sur(Evenement::Fin { etapes, ms: debut.elapsed().as_millis() as u64 })?;
            return Ok(if tour.texte.trim().is_empty() { derniere_reponse } else { tour.texte });
        }

        // Le message assistant DOIT être rejoué avec ses appels : sans lui, le
        // modèle relirait des résultats d'outils qu'il n'a jamais demandés, et
        // certains serveurs refusent la conversation.
        messages.push(json!({
            "role": "assistant",
            "content": tour.texte,
            "tool_calls": tour.appels.iter().map(|a| json!({
                "id": a.id,
                "type": "function",
                "function": { "name": a.nom, "arguments": a.arguments }
            })).collect::<Vec<_>>()
        }));
        if !tour.texte.trim().is_empty() {
            derniere_reponse.clone_from(&tour.texte);
        }

        for a in &tour.appels {
            sur(Evenement::Acte { etape, outil: &a.nom, argument: &a.arguments })?;
            let (rendu, ok) = match executer(&a.nom, &a.arguments, &g) {
                Ok(t) => (t, true),
                // Un refus est un RÉSULTAT : le modèle doit le lire et changer
                // d'approche. Le taire le ferait insister sur la même action.
                Err(e) => (format!("{{\"error\": \"{:#}\"}}", e).replace('\n', " "), false),
            };
            let apercu: String = rendu.chars().take(180).collect();
            sur(Evenement::Rend { etape, ok, apercu: &apercu })?;
            messages.push(json!({
                "role": "tool",
                "tool_call_id": a.id,
                "content": rendu,
            }));
        }
    }

    // ── LA BORNE EST DITE ───────────────────────────────────────────────────
    // Rendre la dernière réponse partielle comme si c'était une conclusion
    // serait le pire des mensonges : l'opérateur croirait l'objectif atteint.
    sur(Evenement::Borne { etapes: max_etapes })?;
    Ok(format!(
        "{derniere_reponse}\n\n[budget d'étapes épuisé ({max_etapes}) — objectif non conclu, \
         la boucle s'arrête ici et le dit.]"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write as _};
    use std::net::TcpListener;

    use obrain_garde::Reglages;

    /// Un modèle OpenAI-compatible de laboratoire : une réponse SSE par
    /// connexion, dans l'ordre, sur un port éphémère.
    ///
    /// `tenir_ouvert_ms` sert aux cas où la réponse ne doit PAS finir : sans lui,
    /// la fin du flux suffirait à sortir de la boucle et un test d'arrêt ne
    /// prouverait rien.
    fn modele_de_laboratoire(reponses: Vec<String>, tenir_ouvert_ms: u64) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("port éphémère");
        let port = listener.local_addr().expect("adresse locale").port();
        std::thread::spawn(move || {
            for corps in reponses {
                let Ok((mut flux, _)) = listener.accept() else { return };
                // On lit la requête jusqu'à la ligne vide : le corps ne nous
                // intéresse pas, un serveur OpenAI-compatible n'en a pas besoin
                // pour répondre en flux.
                let Ok(clone) = flux.try_clone() else { return };
                let mut lecteur = BufReader::new(clone);
                let mut ligne = String::new();
                while lecteur.read_line(&mut ligne).unwrap_or(0) > 0 {
                    if ligne == "\r\n" || ligne == "\n" {
                        break;
                    }
                    ligne.clear();
                }
                let entete = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                              Cache-Control: no-cache\r\nConnection: close\r\n\r\n";
                let _ = flux.write_all(entete.as_bytes());
                let _ = flux.write_all(corps.as_bytes());
                let _ = flux.flush();
                if tenir_ouvert_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(tenir_ouvert_ms));
                }
                // `flux` tombe ici : la connexion se ferme, donc un flux qui n'a
                // pas été arrêté se termine normalement.
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// Une trame SSE de contenu, telle que la produit un serveur local.
    fn trame_contenu(texte: &str) -> String {
        format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":{"content":texte}}]})
        )
    }

    /// Une trame SSE d'appel d'outil, avec l'index fourni par le flux distant.
    fn trame_appel(index: u64, nom: &str, arguments: &str) -> String {
        format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":{"tool_calls":[
                {"index":index,"id":"appel-1","function":{"name":nom,"arguments":arguments}}
            ]}}]})
        )
    }

    /// Les greements d'un nœud de laboratoire : aucune machine réelle n'est
    /// touchée (racine = dossier temporaire, exécution NON autorisée, moteur
    /// injoignable — `Socle::lire` dégrade « sans socle », c'est sa conception).
    fn greement_de_laboratoire(dir: &Path, moteur: &str) -> (Politique, Identite, Socle) {
        let politique = Politique { racine: dir.to_path_buf(), ..Politique::default() };
        let identite = Identite::charger_ou_creer(dir, None).expect("identité de laboratoire");
        let socle = Socle::lire(moteur, 200);
        (politique, identite, socle)
    }

    /// Un dossier de travail neuf, par test (aucune dépendance externe : le
    /// crate doit rester installable sans chaîne d'outils).
    fn dossier_de_laboratoire(nom: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("selfd-test-{nom}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dossier temporaire");
        d
    }

    /// ── LE DÉFAUT MESURÉ, TENU PAR UN TEST ─────────────────────────────────
    ///
    /// « L'engine a tendance à faire une génération en boucle infinie. » La
    /// boucle du micro-harness n'avait AUCUNE détection de répétition : elle
    /// écrivait jusqu'au plafond de jetons, tour après tour. Ce test la tient
    /// ouverte : un modèle qui se répète doit se faire arrêter, et le dire.
    #[test]
    fn un_modele_qui_se_repete_est_arrete_et_le_dit() {
        // Un préambule assez long pour que le détecteur ait le droit de conclure
        // (il ne conclut RIEN en dessous de 600 caractères), puis le même bloc
        // répété : c'est exactement la forme d'une génération qui boucle.
        let mut corps = String::new();
        for i in 0..12 {
            corps.push_str(&trame_contenu(&format!(
                "Constat numero {i} sur la lecture du module, avec son chiffre reel. "
            )));
        }
        let bloc = "D'apres le graphe, la fonction tcp_v4_rcv recoit 11 077 appels. ";
        for _ in 0..8 {
            corps.push_str(&trame_contenu(bloc));
        }
        corps.push_str("data: [DONE]\n\n");
        // Le service tient la connexion ouverte : sans arrêt, la lecture
        // attendrait le délai au lieu de sortir d'elle-même.
        let moteur_llm = modele_de_laboratoire(vec![corps], 500);
        let dir = dossier_de_laboratoire("repetition");
        let (politique, identite, socle) = greement_de_laboratoire(&dir, "http://127.0.0.1:1");

        let mut garde = Garde::nouvelle(Reglages::default());
        let mut codes: Vec<String> = Vec::new();
        let mut actes = 0usize;
        let mut fragments = 0usize;
        let mut cb = |ev: Evenement| -> Result<()> {
            match ev {
                Evenement::Arret { code, .. } => codes.push(code.to_string()),
                Evenement::Acte { .. } => actes += 1,
                Evenement::Texte(_) => fragments += 1,
                _ => {}
            }
            Ok(())
        };
        let texte = derouler(
            &moteur_llm,
            &socle,
            &politique,
            &identite,
            "http://127.0.0.1:1",
            &dir.join("notes.jsonl"),
            "compte les appels entrants",
            3,
            &mut garde,
            &mut cb,
        )
        .expect("un arrêt n'est pas une erreur : c'est un arrêt");

        assert_eq!(codes, vec!["repetition".to_string()], "l'arrêt doit se dire, avec sa cause");
        assert!(texte.contains("se répète"), "le texte rendu doit porter le motif : {texte}");
        assert_eq!(actes, 0, "un tour arrêté n'exécute AUCUN outil (raisonnement tronqué)");
        assert!(garde.arrete());
        // ── LA COUPE DOIT ÊTRE RÉELLE, PAS SEULEMENT FINALE ────────────────
        // Trouvé par le test de mutation : sans cette assertion, le test passait
        // encore quand la détection ne servait plus à RIEN — le motif était posé
        // (donc l'arrêt annoncé), mais le flux était lu jusqu'au bout. Or « lire
        // jusqu'au bout puis dire qu'on a bouclé » n'est pas la garantie : c'est
        // exactement le défaut d'origine, qui écrivait jusqu'au plafond.
        // 20 trames sont envoyées ; 15 suffisent pour conclure (12 de préambule
        // puis le troisième bloc identique).
        assert!(
            fragments < 20,
            "la lecture doit cesser DÈS la répétition (fragments lus : {fragments})"
        );
    }

    /// ── L'INDEX ABERRANT ───────────────────────────────────────────────────
    ///
    /// Défaut mesuré ailleurs dans le dépôt : `while appels.len() <= i` avec un
    /// index venu du flux distant allouait un milliard de structures (~70 Go) et
    /// faisait avorter le processus. Le correctif existait dans une seule des
    /// trois boucles ; ce test tient la deuxième.
    #[test]
    fn un_index_d_appel_aberrant_est_ignore_au_lieu_d_allouer() {
        // 100 000 (et non 1e9) : la régression se mesure alors sans faire
        // tomber la machine du test — l'assertion porte sur le COMPORTEMENT
        // (l'appel est ignoré, donc le tour est une réponse), pas sur la
        // consommation mémoire.
        let corps = format!("{}{}", trame_appel(100_000, "exec", "{}"), "data: [DONE]\n\n");
        let moteur_llm = modele_de_laboratoire(vec![corps.clone(), corps.clone(), corps], 0);
        let dir = dossier_de_laboratoire("index-aberrant");
        let (politique, identite, socle) = greement_de_laboratoire(&dir, "http://127.0.0.1:1");

        let mut garde = Garde::nouvelle(Reglages::default());
        let mut etapes_fin: Vec<usize> = Vec::new();
        let mut actes = 0usize;
        let mut cb = |ev: Evenement| -> Result<()> {
            match ev {
                Evenement::Fin { etapes, .. } => etapes_fin.push(etapes),
                Evenement::Acte { .. } => actes += 1,
                _ => {}
            }
            Ok(())
        };
        derouler(
            &moteur_llm,
            &socle,
            &politique,
            &identite,
            "http://127.0.0.1:1",
            &dir.join("notes.jsonl"),
            "explore la machine",
            3,
            &mut garde,
            &mut cb,
        )
        .expect("la boucle doit se terminer");

        assert_eq!(etapes_fin, vec![1], "l'appel aberrant est ignoré : le tour 1 est une réponse");
        assert_eq!(actes, 0, "un appel hors plafond ne doit jamais être exécuté");
    }
}
