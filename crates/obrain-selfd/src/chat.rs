//! Le chat du nœud — parler à **ce** nœud, et pas au self global.
//!
//! # La distinction qui donne son sens à cette surface
//!
//! Le moteur obrain expose déjà une conversation (`/api/engine/chat`, en SSE).
//! Elle est **ancrée sur le graphe global** : sa réponse vient du self, de ses
//! mondes, de ses bases. C'est utile, et ce n'est pas la même chose.
//!
//! Ici, le contexte est construit **du nœud seul** : son identité persistée, sa
//! politique effective, son état de marche, ce qu'il voit de sa racine. Aucune
//! requête n'est faite au graphe global, aucune clé de base n'est lue. Un nœud
//! qui répondrait depuis le graphe global serait un nœud qui ment sur ce qu'il
//! est — et l'opérateur croirait parler à une machine alors qu'il parle à la
//! mémoire partagée.
//!
//! # Ce que ce chat ne fait pas (v1), et pourquoi c'est écrit
//!
//! Il **converse**, il n'agit pas. Il n'exécute pas de commande et ne lit pas de
//! fichier à la demande du modèle : ce serait une boucle d'outils, avec ses
//! approbations et son budget, et la faire à moitié serait pire que de ne pas la
//! faire. Pour agir, la surface existe déjà — MCP, `exec` — et elle est
//! gouvernée par la politique du nœud.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use obrain_garde::{Garde, Motif};

use crate::identite::Identite;
use crate::politique::Politique;

/// Délai d'une **lecture** de flux du modèle.
///
/// Large : un modèle local de 9 B met légitimement des dizaines de secondes à
/// produire un tour, et un délai serré couperait des réponses justes. Ce que ce
/// délai borne, c'est le SILENCE (une trame qui n'arrive plus) ; ce que borne le
/// budget de la garde, c'est la DURÉE TOTALE de la question. Les deux sont
/// nécessaires, et ils ne mesurent pas la même chose.
pub const DELAI_MODELE: std::time::Duration = std::time::Duration::from_secs(600);

/// URL par défaut du modèle : le serveur local d'obrain.
pub const LLM_DEFAUT: &str = "http://127.0.0.1:9901/v1";

/// Résout le modèle à employer.
pub fn llm_par_defaut() -> String {
    std::env::var("SELFD_LLM")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| LLM_DEFAUT.to_string())
}

/// Le contexte du nœud, en un bloc de texte.
///
/// Il est **relu à chaque tour**, jamais figé au démarrage : une charge ou un
/// nom d'hôte qui change doit apparaître, sinon le chat parlerait d'une machine
/// qui n'existe plus.
pub fn contexte(p: &Politique, id: &Identite, debut: Instant, mode: &str) -> String {
    let mut c = String::new();
    c.push_str(
        "Tu es le nœud décrit ci-dessous, et rien d'autre. Tu réponds de SA place : \
         tu ne connais ni graphe global, ni self, ni base de connaissances partagée — \
         si on te demande ce qu'il y a ailleurs, dis que tu ne le sais pas d'ici.\n\n",
    );
    c.push_str("## Identité du nœud\n");
    c.push_str(&format!("- nodeId (stable) : {}\n", id.node_id));
    c.push_str(&format!("- machine         : {}\n", id.hostname_courant));
    if id.hostname != id.hostname_courant {
        c.push_str(&format!(
            "- (identité engendrée sous « {} » ; la machine a été renommée)\n",
            id.hostname
        ));
    }
    c.push_str(&format!(
        "- système         : {} / {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    c.push_str(&format!("- atteint par      : {mode}\n"));

    c.push_str("\n## Politique effective (fixée au démarrage, non négociable)\n");
    c.push_str(&format!("- racine          : {}\n", p.racine.display()));
    c.push_str(&format!(
        "- exécution       : {}\n",
        if p.exec_autorise {
            "AUTORISÉE (--allow-exec)"
        } else {
            "REFUSÉE — ce nœud observe et n'agit pas"
        }
    ));
    c.push_str(&format!(
        "- plafonds        : {} octets par flux, {} ms par commande\n",
        p.plafond_octets, p.delai_ms
    ));

    c.push_str("\n## État de marche\n");
    c.push_str(&format!(
        "- en marche depuis : {:.0} s\n",
        debut.elapsed().as_secs_f64()
    ));
    if let Ok(txt) = std::fs::read_to_string("/proc/loadavg") {
        let ch: Vec<&str> = txt.split_whitespace().collect();
        if ch.len() >= 3 {
            c.push_str(&format!(
                "- charge          : {} {} {}\n",
                ch[0], ch[1], ch[2]
            ));
        }
    }
    if let Some(brut) = lister_racine(&p.racine) {
        c.push_str(&format!(
            "\n## Racine du nœud ({})\n{}\n",
            p.racine.display(),
            brut
        ));
    }
    c
}

/// Un aperçu borné du contenu de la racine — de quoi situer, pas de quoi noyer.
fn lister_racine(racine: &Path) -> Option<String> {
    let mut entrees: Vec<String> = Vec::new();
    for e in std::fs::read_dir(racine).ok()?.flatten().take(40) {
        let nom = e.file_name().to_string_lossy().to_string();
        let suffixe = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
        entrees.push(format!("- {nom}{suffixe}"));
    }
    entrees.sort();
    Some(entrees.join("\n"))
}

/// L'historique du chat, propre au nœud.
///
/// Un fichier JSONL sous le state-dir, **par nœud** : la conversation avec une
/// machine est la mémoire de cette machine, pas une mémoire partagée. Deux nœuds
/// n'ont pas à relire les échanges de l'autre.
pub struct Historique {
    pub fichier: PathBuf,
}

impl Historique {
    pub fn nouveau(state_dir: &Path) -> Self {
        Self {
            fichier: state_dir.join("chat.jsonl"),
        }
    }

    /// Ajoute un tour, en JSONL append-only.
    ///
    /// Append-only plutôt que réécriture : une coupure ne coûte que le dernier
    /// tour, au lieu de tout le fichier — même raison que le journal du moteur.
    pub fn ajouter(&self, role: &str, contenu: &str) {
        let ligne = json!({
            "t": crate::identite::maintenant_ms(),
            "role": role,
            "content": contenu,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.fichier)
        {
            let _ = writeln!(f, "{ligne}");
        }
    }

    /// Les `n` derniers tours, du plus ancien au plus récent.
    pub fn derniers(&self, n: usize) -> Vec<Value> {
        let Ok(txt) = std::fs::read_to_string(&self.fichier) else {
            return Vec::new();
        };
        let lignes: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
        lignes
            .iter()
            .rev()
            .take(n)
            .rev()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }
}

/// Ce qu'un tour de conversation a rendu — **arrêt compris**.
///
/// La distinction entre « le modèle a fini » et « nous l'avons arrêté » n'est pas
/// un détail de journal : sans elle, une réponse coupée (client parti, budget
/// épuisé, modèle qui se répète) se lirait comme une réponse complète, et
/// l'opérateur croirait à une conclusion.
#[derive(Debug, Clone)]
pub struct Tour {
    /// Le texte reçu (complet, ou ce qui a été émis avant l'arrêt).
    pub texte: String,
    /// Le motif de l'arrêt, `None` quand le flux est allé à son terme.
    pub arret: Option<Motif>,
}

/// Un tour de conversation avec le nœud, en flux, **sous garde**.
///
/// `sur_fragment` est appelé à chaque morceau de texte : c'est ce qui permet à la
/// page de chat d'afficher la réponse **pendant** qu'elle s'écrit, comme le fait
/// déjà la vue du dashboard pour le moteur. Il rend `None` pour continuer, ou un
/// [`Motif`] pour arrêter — c'est ainsi qu'une écriture refusée (page fermée) fait
/// cesser la LECTURE du flux du modèle, au lieu de laisser le nœud générer pour
/// personne.
///
/// `garde` porte l'échéance, la détection de répétition et le drapeau du client
/// parti : les trois garanties d'arrêt vivent dans `obrain-garde`, pas ici.
///
/// Rend le tour (texte + motif d'arrêt éventuel), ou une erreur **explicite** —
/// jamais une réponse vide qui passerait pour une fin naturelle.
pub fn converser<F>(
    llm: &str,
    systeme: &str,
    historique: &[Value],
    question: &str,
    garde: &mut Garde,
    mut sur_fragment: F,
) -> Result<Tour>
where
    F: FnMut(&str) -> Option<Motif>,
{
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": systeme })];
    for h in historique {
        let role = h.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let contenu = h.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if !contenu.is_empty() {
            messages.push(json!({ "role": role, "content": contenu }));
        }
    }
    messages.push(json!({ "role": "user", "content": question }));

    let corps = json!({
        "messages": messages,
        "stream": true,
        // Borné : un tour qui n'en finit pas occupe le nœud pour rien.
        "max_tokens": 2048,
    })
    .to_string();

    let url = format!("{}/chat/completions", llm.trim_end_matches('/'));
    let mut complet = String::new();
    let mut erreur: Option<String> = None;

    // La commande d'arrêt est consultée AVANT chaque lecture : un client parti
    // ne se voit pas dans le texte, donc sans cette place il n'était remarqué
    // qu'à la trame suivante — c'est-à-dire jamais, si le modèle s'était tu.
    let veille = garde.veille();
    let mut premier_tour = true;

    // Le modèle local répond en SSE (`data: {...}`, puis `data: [DONE]`).
    // `lire_sse` rend la ligne telle quelle ; on ne consomme que le contenu des
    // choix, et on IGNORE le reste au lieu de le confondre avec du texte.
    let resultat = obrain_httpmin::post_sse_supervise(
        &url,
        &corps,
        None,
        DELAI_MODELE,
        |_nom, donnees| {
            if donnees.trim() == "[DONE]" {
                return true;
            }
            let Ok(v) = serde_json::from_str::<Value>(donnees) else {
                return true;
            };
            if let Some(e) = v.get("error") {
                erreur = Some(e.to_string());
                return true; // on lit jusqu'au bout : le verdict est rendu après
            }
            if let Some(frag) = v["choices"][0]["delta"]["content"].as_str() {
                if premier_tour {
                    garde.nouvelle_page();
                    premier_tour = false;
                }
                complet.push_str(frag);
                // La répétition se juge sur ce qui a été ÉMIS, et elle arrête
                // la lecture : un modèle qui boucle écrivait jusqu'au plafond,
                // ce qui se lit, à l'écran, comme une génération infinie.
                if let Err(motif) = garde.prose(frag) {
                    let _ = motif;
                    return false;
                }
                // Une erreur d'écriture vers le client (page fermée) ne doit pas
                // être confondue avec une erreur du modèle : on la garde pour la
                // remonter, mais le texte déjà reçu n'est pas perdu.
                if let Some(motif) = sur_fragment(frag) {
                    garde.arreter(motif);
                    return false;
                }
            }
            true
        },
        || veille.continuer(),
    )
    .map(|arrete_par_nous| {
        // ── QUI A ARRÊTÉ ? ON LE DIT, ON NE LE DEVINE PAS ──────────────────
        // Le rappel peut avoir décidé (répétition, client parti) ; sinon la
        // lecture a cessé parce que la veille a dit non (client ou budget) —
        // et c'est `tour()` qui pose alors le motif, dans le même ordre de
        // priorité que partout ailleurs.
        if arrete_par_nous && garde.motif().is_none() {
            let _ = garde.tour();
        }
        arrete_par_nous
    });

    let arrete_par_nous = resultat.with_context(|| format!("appel du modèle ({url})"))?;
    if let Some(e) = erreur {
        bail!("{e}");
    }
    let arret = garde.motif().cloned();
    if complet.is_empty() {
        // Un arrêt SANS texte est un arrêt, pas une réponse vide : on le dit.
        if let Some(m) = arret {
            bail!("{}", m.phrase());
        }
        let _ = arrete_par_nous;
        bail!("le modèle n'a rendu aucun texte (vérifier {url})");
    }
    Ok(Tour {
        texte: complet,
        arret,
    })
}

/// Le modèle répond-il ? (sonde courte, pour que `status` puisse le dire)
pub fn modele_disponible(llm: &str) -> bool {
    let url = format!("{}/models", llm.trim_end_matches('/'));
    matches!(obrain_httpmin::get(&url, &[], None), Ok(r) if (200..300).contains(&r.status))
}
