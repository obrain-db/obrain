//! PONT VERS L'ENGINE — un LLM qui raisonne SUR le graphe.
//!
//! Trois principes, qui viennent directement de la demande :
//!
//! 1. **Le contexte vient de ce que l'utilisateur regarde.** Le `sujet` partagé
//!    (base + nœud visé) est le point d'entrée : on extrait son voisinage RÉEL
//!    par le même code que la viz (`extract_subgraph`), pas une invention.
//! 2. **La consultation est tracée.** Les nœuds envoyés au modèle sont inscrits
//!    au journal d'activation (`crate::activation`) : le graphe s'allume
//!    pendant que l'engine raisonne — c'est le point (4) de l'objectif, et il
//!    tient ici sans câblage supplémentaire.
//! 3. **Rien n'est inventé sur la latence.** On mesure et on renvoie le temps
//!    réel, la taille réelle du contexte et le modèle réellement servi.
//!
//! Le service visé est un serveur OpenAI-compatible local (llama-server), par
//! défaut `http://127.0.0.1:9901`. Les modèles à RAISONNEMENT (Qwen3.5 ici)
//! remplissent `reasoning_content` et peuvent laisser `content` vide si la
//! génération est coupée : on lit les deux, dans cet ordre de préférence.
//!
//! ─────────────────────────────────────────────────────────────────────────────
//! ## 2026-09-11 · LA DENSIFICATION — ce qu'on lisait, et ce qu'on lit
//!
//! Défaut mesuré : sur `world:this-rs-this--github` (4 685 nœuds, 1 500
//! `Function`, 1 175 `Class`, 1 039 `Method`), le contexte envoyé au modèle
//! contenait **24 titres de `.github/ISSUE_TEMPLATE/`** et **aucune** fonction.
//! Cause : `MATCH (n) RETURN n.qualified_name LIMIT 24`, **sans `ORDER BY`** —
//! c'est-à-dire l'ordre de stockage, qui n'est pas un échantillon.
//!
//! Sur `runtime-dev` (372 019 nœuds), 21 des 24 nœuds appartenaient à **une
//! seule session** (`session-c254ac35…`) : le modèle ne voyait pas « le projet »,
//! il voyait une conversation.
//!
//! La densification retenue est **contrainte par la mesure**, pas par l'intuition :
//!
//! | axe | coût mesuré | verdict |
//! |---|---|---|
//! | compter nœuds/arêtes | 0,04 ms | toujours |
//! | échantillon par label (`MATCH (n:Function) … LIMIT n`) | 0,3 – 12 ms | ✅ retenu |
//! | arêtes réelles par type (`MATCH (a)-[r:CALLS]->(b) …`) | 5 – 70 ms | ✅ retenu |
//! | histogramme des labels | 160 ms (372 k) · 675 ms (1,5 M) | ✅ mémorisé sur disque |
//! | histogramme des types d'arêtes | **2,3 s (372 k) · 3,5 s (1,5 M)** | ✅ mémorisé sur disque |
//! | hubs par degré (`MATCH (n)--(m) … ORDER BY count(m) DESC`) | **14,1 s (372 k) · 14,3 s (1,5 M)** | ❌ **exclu** |
//!
//! Conséquence assumée : on ne prétend **pas** montrer le « cœur » du graphe. On
//! montre un échantillon **représentatif par type** et des **arêtes réelles**, et
//! on annonce le pourcentage lu. Un plafond silencieux fait croire que la donnée
//! n'existe pas — c'est pire qu'une donnée manquante.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use obrain_common::types::Value as OV;
use obrain_engine::ObrainDB;

use crate::viz_extract::{ExtractOptions, extract_subgraph_centers};

/// Plafond d'arêtes citées — suit la fenêtre comme le reste.
fn max_relations() -> usize {
    max_relations_pour(fenetre_jetons())
}

/// Nombre de labels échantillonnés, et nœuds par label.
///
/// C'est LE levier de densification, et il était calibré pour 8 192 jetons. Avec
/// une fenêtre de 32 768 on peut montrer trois fois plus de matière pour le même
/// coût de requête (l'échantillon par label est mesuré à 0,3-12 ms) : le
/// gaspiller serait refuser de densifier alors que c'est devenu gratuit.
fn labels_echantillonnes() -> usize {
    labels_echantillonnes_pour(fenetre_jetons())
}
fn noeuds_par_label() -> usize {
    noeuds_par_label_pour(fenetre_jetons())
}
/// Types d'arêtes dont on lit de VRAIES arêtes, et combien par type.
fn types_aretes() -> usize {
    types_aretes_pour(fenetre_jetons())
}
fn aretes_par_type() -> usize {
    aretes_par_type_pour(fenetre_jetons())
}
/// Budget de caractères du contexte — **dérivé de la fenêtre réelle**, plus une
/// constante calibrée pour un seul modèle.
///
/// Le contexte est partagé avec la conversation, les résultats d'outils ET la
/// réponse : densifier en bourrant le prompt reste impossible, on densifie par
/// SÉLECTION. Mais le PLAFOND, lui, doit suivre le modèle servi — voir
/// `budget_contexte_caracteres()`.
fn budget_caracteres() -> usize {
    budget_contexte_caracteres()
}
/// Nombre de labels pour lesquels on échantillonne des nœuds.
/// Nœuds lus par label (l'échantillon par type).
/// Types d'arêtes dont on lit de VRAIES arêtes.
/// Arêtes lues par type.

/// Comportement par défaut du service LLM local.
fn url_llm() -> String {
    std::env::var("OBRAIN_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:9901".to_string())
}

fn modele() -> String {
    std::env::var("OBRAIN_LLM_MODEL").unwrap_or_else(|_| "obrain-local".to_string())
}

/// Fenêtre de jetons du service local (`llama-server --ctx-size`, 8192 par
/// défaut ici). Surchargeable par `OBRAIN_LLM_CTX` quand le service change.
///
/// C'est un budget **PARTAGÉ** : l'invite, l'historique, le contexte du graphe
/// et la réponse à venir y tiennent ENSEMBLE. Le traiter comme une limite par
/// requête produit le défaut mesuré le 2026-09-11 : sur la base « Runtime dev
/// (engrams) » (384 626 nœuds, contexte de 12 000 caractères), le chat
/// annonçait le contexte puis ne rendait plus rien — la génération franchissait
/// la fenêtre et le service refusait la requête.
pub(crate) fn fenetre_jetons() -> usize {
    // 1 · Un réglage explicite fait toujours foi.
    if let Some(v) = std::env::var("OBRAIN_LLM_CTX")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v >= 1024)
    {
        return v;
    }
    // 2 · SINON ON DEMANDE AU SERVICE. La fenêtre n'est pas une constante du
    //     logiciel : elle appartient au modèle servi. Défaut mesuré le
    //     2026-09-11 : le moteur avait changé pour un modèle distant qui annonce
    //     **32 768** jetons, alors que le code en supposait 8 192 — il tronquait
    //     donc son contexte, ses résultats d'outils et ses tours d'exploration
    //     d'un facteur QUATRE, en silence. On lit donc `/props` (llama-server)
    //     une fois, et on garde le résultat.
    static ANNONCEE: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let annoncee = *ANNONCEE.get_or_init(|| {
        let url = format!("{}/props", url_llm());
        let Ok(r) = crate::http_min::get(&url, Duration::from_secs(3)) else {
            return None;
        };
        let v: serde_json::Value = serde_json::from_str(&r.body).ok()?;
        // `default_generation_settings.n_ctx` est la fenêtre PAR CRÉNEAU ; c'est
        // celle qui s'applique à une requête. `n_ctx` sert de repli.
        v.get("default_generation_settings")
            .and_then(|g| g.get("n_ctx"))
            .and_then(|x| x.as_u64())
            .or_else(|| v.get("n_ctx").and_then(|x| x.as_u64()))
            .map(|x| x as usize)
            .filter(|x| *x >= 1024)
    });
    // 3 · Repli : la valeur historique. On ne DESCEND jamais en dessous : une
    //     fenêtre surestimée ferait refuser la requête, une fenêtre sous-estimée
    //     ne coûte que de la place — on préfère perdre de la place.
    match annoncee {
        Some(n) => n.min(262_144),
        None => 8192,
    }
}

/// D'OÙ VIENT la fenêtre retenue — `env`, `service` ou `défaut`.
///
/// Un chiffre sans provenance ne se vérifie pas : « 32 768 » ne dit pas si c'est
/// le modèle, le service ou une constante du logiciel. Or c'est exactement la
/// question qu'on se pose devant un contexte tronqué.
pub(crate) fn source_fenetre() -> String {
    if std::env::var("OBRAIN_LLM_CTX").is_ok() {
        return format!("env (OBRAIN_LLM_CTX) · service {} ({})", url_llm(), modele());
    }
    let nom = url_llm();
    if fenetre_jetons() != 8192 {
        return format!("annoncée par le service {nom}");
    }
    format!("défaut du logiciel (le service {nom} n'a rien annoncé)")
}

/// Ce que la fenêtre PERMET, à partir de la fenêtre réellement annoncée.
///
/// Ces budgets étaient des constantes calibrées pour 8 192 jetons. Elles
/// brident désormais ce que le contexte peut contenir et ce qu'un outil peut
/// rendre : avec 32 768, garder 12 000 caractères de contexte et 30 lignes par
/// appel gaspille les trois quarts de la capacité — et c'est exactement la
/// « densification » qu'on cherche à obtenir.
pub(crate) fn budget_contexte_caracteres() -> usize {
    budget_contexte_pour(fenetre_jetons())
}

/// Nombre de tours d'outils que la fenêtre peut porter.
pub(crate) fn tours_outils_pour(fenetre: usize) -> usize {
    if fenetre >= 16_384 { 6 } else { 4 }
}
pub(crate) fn tours_outils() -> usize {
    tours_outils_pour(fenetre_jetons())
}

/// Combien de nœuds un contexte peut citer sans noyer le modèle.
pub(crate) fn budget_noeuds_pour(fenetre: usize) -> usize {
    (fenetre / 160).clamp(60, 200)
}
pub(crate) fn budget_noeuds() -> usize {
    budget_noeuds_pour(fenetre_jetons())
}

// ── LES MÊMES BUDGETS, PRIS SUR LA FENÊTRE — donc vérifiables sans réseau ───
// Ces fonctions prennent la fenêtre en paramètre : un test peut donc affirmer
// des valeurs EXACTES sans dépendre d'un service ni d'une variable
// d'environnement (les tests s'exécutent en parallèle dans un même processus :
// y toucher à l'environnement serait une source de faux échecs).
pub(crate) fn budget_contexte_pour(fenetre: usize) -> usize {
    let par_jetons = (fenetre as f64 * 0.40 * 2.6) as usize;
    par_jetons.clamp(12_000, 40_000)
}
pub(crate) fn labels_echantillonnes_pour(fenetre: usize) -> usize {
    if fenetre >= 16_384 { 14 } else { 8 }
}
pub(crate) fn noeuds_par_label_pour(fenetre: usize) -> usize {
    if fenetre >= 16_384 { 8 } else { 4 }
}
pub(crate) fn types_aretes_pour(fenetre: usize) -> usize {
    if fenetre >= 16_384 { 6 } else { 4 }
}
pub(crate) fn aretes_par_type_pour(fenetre: usize) -> usize {
    if fenetre >= 16_384 { 5 } else { 3 }
}
pub(crate) fn max_relations_pour(fenetre: usize) -> usize {
    if fenetre >= 16_384 { 80 } else { 40 }
}
/// Plafond de caractères d'un résultat d'outil — un huitième de la fenêtre.
pub(crate) fn max_caracteres_outil_pour(fenetre: usize) -> usize {
    ((fenetre as f64 / 8.0 * 2.6) as usize).clamp(900, 8_000)
}
/// Plafond de lignes d'un résultat d'outil, déduit du plafond de caractères.
pub(crate) fn max_lignes_outil_pour(fenetre: usize) -> usize {
    (max_caracteres_outil_pour(fenetre) / 90).clamp(12, 60)
}

/// Estimation de jetons, SANS tokeniseur.
///
/// Le ratio était de **3,2 caractères par jeton**. Il a été mesuré FAUX dans le
/// sens dangereux : avec les définitions d'outils envoyées à chaque tour, le
/// service a refusé la requête — « Context size has been exceeded » — alors que
/// le budget se croyait respecté. Le français accentué, le GQL ponctué et les
/// identifiants qualifiés produisent **plus** de jetons par caractère qu'un
/// texte anglais courant ; sous-estimer fait dépasser la fenêtre, et une
/// requête refusée coûte la réponse entière.
///
/// On resserre donc à **2,6 caractères par jeton** : volontairement pessimiste.
/// Se tromper en réservant trop coûte un peu de contexte ; se tromper en
/// réservant trop peu coûte la conversation.
fn jetons_estimes(texte: &str) -> usize {
    (texte.chars().count() as f64 / 2.6).ceil() as usize + 8
}

/// Marge de sécurité : gabarit de la requête, rôles, et l'imprécision ci-dessus.
const MARGE_JETONS: usize = 384;
/// Place gardée dans la fenêtre pour la réponse quand l'appelant n'en demande
/// pas plus. C'est une RÉSERVATION, pas un plafond de génération.
///
/// Défaut mesuré le 2026-09-12 : les deux notions étaient confondues, et le
/// plafond envoyé au service valait toujours au moins 512 jetons — même quand
/// l'appelant en demandait 24. Sur `/api/engine/ask?max_tokens=24`, la réponse
/// mettait **50 128 ms** (365 jetons réellement produits) au lieu des **2 400 ms**
/// que coûtent 24 jetons. Le paramètre `max_tokens` était donc un mensonge : il
/// n'y a plus qu'un seul plafond, celui de l'appelant, borné par ce que la
/// fenêtre peut porter (voir `plafond_generation`).
const SORTIE_MINIMALE: u32 = 512;

/// Le plafond de génération RÉELLEMENT envoyé au service : ce que l'appelant
/// demande, borné par la place qui reste dans la fenêtre.
///
/// Il n'est volontairement PAS relevé. Le modèle local écrit sa pensée avant sa
/// réponse (mesuré : ~128 jetons avant le premier caractère de `content`), donc
/// un plafond bas rend une pensée tronquée plutôt qu'une réponse. C'est un fait
/// qu'il faut DIRE — `depuis_raisonnement`, `max_tokens_effectif`, `jetons_sortie`
/// le portent — et non corriger en douce en produisant 512 jetons que personne
/// n'a demandés.
fn plafond_generation(demande: u32, restant: usize) -> u32 {
    (demande as usize).min(restant).max(1) as u32
}

/// Ce qu'un tour de conversation doit pouvoir écrire au minimum pour valoir
/// d'être envoyé : en dessous, la boucle du flux conclut « fenêtre saturée » et
/// ne génère RIEN. C'est l'invariant de la boucle (`sortie_tour < 64`), pas un
/// confort — honorer brutalement un `max_tokens=24` sur le flux ferait
/// disparaître la réponse au lieu de la raccourcir.
const SORTIE_TOUR_MINIMALE: u32 = 64;

/// Le réglage de pensée transmis au gabarit de chat, ou `None` pour laisser le
/// gabarit décider (comportement d'origine).
///
/// Mesuré le 2026-09-12 sur le modèle local (Qwen3.5-9B, gabarit Qwen3), même
/// question courte sur un contexte de 4 nœuds :
///   - pensée ACTIVÉE  → de 122 à 400 jetons de pensée, jusqu'à **33 s** pour
///     `?max_tokens=400`, et la réponse finale tantôt absente (le plafond est
///     consommé par la pensée), tantôt rendue en anglais malgré la consigne ;
///   - pensée COUPÉE   → **5,5 s**, 33 jetons, réponse française exacte citant
///     les bons nœuds.
///
/// D'où le partage retenu, et il n'est pas le même sur les deux routes :
///   - `/api/engine/ask` (bloquante, AUCUN tour d'outil) → pensée coupée par
///     défaut : elle n'y servait qu'à consumer le plafond avant la réponse ;
///   - `/api/engine/chat` (flux, avec outils) → pensée ACTIVÉE : c'est elle qui
///     déclenche les appels d'outils. La couper par défaut appauvrirait le chat.
///
/// Le réglage est FAIL-OPEN : un gabarit qui ne connaît pas `enable_thinking`
/// l'ignore, la requête reste valide et la pensée continue simplement.
/// Ce qui part au template du modèle sur le RAISONNEMENT — **deux** leviers.
///
///   - `enable_thinking` : couper la pensée (mesuré : levier de 6× sur ce modèle) ;
///   - `reasoning_effort` : `xhigh` (**défaut du template**), `medium`, `low`.
///
/// CE QUI MANQUAIT (mesuré le 2026-09-15) : le chemin du CHAT
/// (`demander_stream`) n'envoyait **rien**, donc le template appliquait son
/// défaut — `xhigh`, dont le texte est « think carefully through the task,
/// validate key assumptions, consider plausible alternatives ». Aucune consigne
/// de brièveté dans le prompt système ne pouvait l'emporter sur une instruction
/// aussi explicite : c'est ce qui faisait **37 s de raisonnement pour une
/// question de comptage**. Le réglage est désormais **explicite sur les deux
/// chemins**, et le défaut du chat est `medium`, le niveau neutre du template.
///
/// Un gabarit qui ne connaît pas ces clés les ignore : la requête reste valide.
fn gabarit_de_pensee(pensee: bool, effort: &str) -> Option<serde_json::Value> {
    if !pensee {
        return Some(serde_json::json!({ "enable_thinking": false }));
    }
    Some(serde_json::json!({ "enable_thinking": true, "reasoning_effort": effort }))
}

/// Traduit ce que l'appelant demande en niveau d'effort du template.
///
/// Les valeurs du template sont `xhigh` (son défaut), `medium` et `low` — toute
/// autre fait **lever le template**, donc on ne laisse rien passer : ce qui
/// n'est pas reconnu retombe sur `medium`.
pub(crate) fn effort_de_pensee(demande: Option<&str>) -> &'static str {
    match demande.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("courte") | Some("court") | Some("low") => "low",
        Some("profonde") | Some("profond") | Some("xhigh") => "xhigh",
        Some("moyenne") | Some("medium") => "medium",
        // Défaut du CHAT : `medium`. C'était `xhigh` par omission du template, et
        // ce n'était pas un choix — c'était un héritage.
        _ => "medium",
    }
}

/// Ajuste le contexte du graphe à la place RÉELLEMENT disponible.
///
/// S'il ne tient pas, on le tronque à une frontière de ligne (une arête coupée
/// au milieu ferait inventer le modèle sur une donnée incomplète) et **on le
/// dit dans le contexte lui-même** : un extrait tu n'est pas un tout, et une
/// coupe silencieuse se lirait comme une base vide.
fn contexte_ajuste(texte: &str, jetons_disponibles: usize) -> (String, bool) {
    if jetons_estimes(texte) <= jetons_disponibles {
        return (texte.to_string(), false);
    }
    let max_car = (jetons_disponibles as f64 * 2.6).floor() as usize;
    let mut coupe = max_car.min(texte.len());
    while coupe > 0 && !texte.is_char_boundary(coupe) {
        coupe -= 1;
    }
    let fin = texte[..coupe].rfind('\n').unwrap_or(coupe);
    let ajuste = format!(
        "{}\n\n[CONTEXTE TRONQUÉ : {} caractères sur {} — la fenêtre du modèle est de {} jetons, \
dont la réponse à venir. Ce qui précède est un extrait : ne conclus pas sur la base entière à \
partir de lui, et dis ce qui manque.]",
        &texte[..fin],
        fin,
        texte.chars().count(),
        fenetre_jetons()
    );
    (ajuste, true)
}

// ── lecture tolérante des valeurs du moteur ─────────────────────────────────

fn lire(db: &ObrainDB, gql: &str) -> Vec<Vec<OV>> {
    db.session().execute(gql).map(|r| r.rows).unwrap_or_default()
}

fn txt(v: Option<&OV>) -> Option<String> {
    match v? {
        OV::String(s) => Some(s.to_string()),
        OV::Int64(x) => Some(x.to_string()),
        OV::Float64(x) => Some(x.to_string()),
        _ => None,
    }
}

fn ent(v: Option<&OV>) -> i64 {
    match v {
        Some(OV::Int64(x)) => *x,
        Some(OV::Float64(x)) => *x as i64,
        Some(OV::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Un type d'arête ne s'interpole que s'il est un identifiant : on refuse tout
/// le reste. Une donnée qui vient de la base ne doit pas pouvoir écrire du GQL.
fn type_interpolable(nom: &str) -> bool {
    !nom.is_empty() && nom.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Ce sur quoi la conversation est ancrée.
///
/// Calculé par l'appelant (le serveur), **jamais deviné par l'engine** : c'est
/// la correction du défaut d'ancrage — le chat annonçait un monde et en lisait
/// un autre. Ce qui est ancré est désormais porté explicitement, et renvoyé à
/// l'écran tel quel.
#[derive(Clone, Debug)]
pub struct Ancrage {
    /// Chemin du store réellement ouvert.
    pub store: String,
    /// Libellé du monde, quand le sujet en désigne un (`world:<slug>`).
    pub monde: Option<String>,
    /// Le Self de l'agent, propriétaire de ce qui sera lu **et écrit**.
    ///
    /// Il est porté dès l'ancrage parce que l'identité du self divergeait :
    /// la toile et `self-demo` portent `self:UTILISATEUR`, l'ancien chemin MCP
    /// retombait sur `self:agent`, la documentation citait
    /// `self:agent:state:v72`. Une conversation doit savoir à qui elle
    /// appartient **avant** de prétendre mémoriser quoi que ce soit.
    pub self_qn: String,
}

/// Où passe le temps de lecture du contexte — **mesuré, pas estimé**.
///
/// Ce détail existe parce qu'une latence inexpliquée se répare mal : on a passé
/// 16 s sur un sujet absent, corrigé une première fois à 4 s, puis découvert que
/// le coût restant n'était PAS là où on le croyait. Sans découpe, chaque
/// correction est une hypothèse. Avec, c'est une mesure.
#[derive(Clone, Debug, Default)]
pub struct Decomposition {
    pub totaux_ms: u64,
    pub resolution_ms: u64,
    pub signature_ms: u64,
    pub voisinage_ms: u64,
    /// Vrai quand la carte du store vient du disque (aucun agrégat repayé).
    pub carte_memoire: bool,
    /// La répartition INTERNE du voisinage, en µs : résolution des centres,
    /// parcours des arêtes, composantes, construction. `voisinage_ms` seul
    /// annonçait ~900 ms sans dire quelle phase les payait — c'est justement ce
    /// qu'on ne pouvait pas voir.
    pub voisinage_phases: Option<crate::viz_extract::PhasesUs>,
}

impl Decomposition {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "totaux": self.totaux_ms,
            "resolution": self.resolution_ms,
            "signature": self.signature_ms,
            "voisinage": self.voisinage_ms,
            "voisinage_us": self.voisinage_phases,
            "carte_memoire": self.carte_memoire,
        })
    }
}

/// Contexte réel extrait du graphe autour d'un sujet — **et la part lue**.
pub struct Contexte {
    pub qn: String,
    pub noeuds: Vec<String>,
    pub relations: Vec<String>,
    pub texte: String,
    /// Libellé du monde lu, quand le sujet en désigne un.
    pub monde: Option<String>,
    /// Chemin du store réellement ouvert : l'écran ne peut plus mentir.
    pub store: String,
    /// Ce que la base contient (le dénominateur).
    pub total_noeuds: i64,
    pub total_aretes: i64,
    /// Répartition par label : (label, total dans la base, dont lus).
    pub par_label: Vec<(String, i64, usize)>,
    /// Le Self de l'agent propriétaire de cette lecture.
    pub self_qn: String,
    /// Les nœuds les plus connectés, déjà mis en forme.
    pub hubs: Vec<String>,
    /// Temps du calcul des hubs (mesuré par l'entretien).
    pub hubs_ms: u64,
    /// Où le temps est réellement passé.
    pub decompose: Decomposition,
}

/// La part lue, **en clair** — et « part inconnue » quand elle ne se calcule pas.
///
/// On ne divise jamais par zéro, et on n'invente pas un pourcentage : un total
/// inconnu est DIT inconnu. Même règle que partout ailleurs ici — un chiffre faux
/// est pire qu'un chiffre absent, parce qu'il se lit comme une preuve.
fn part_lue(lus: usize, total: i64, decimales: usize) -> String {
    if total <= 0 {
        return "part inconnue".to_string();
    }
    format!("{:.*} %", decimales, 100.0 * lus as f64 / total as f64)
}

// ── LA SIGNATURE D'UN STORE, MÉMORISÉE ──────────────────────────────────────

fn paires(v: &serde_json::Value) -> Vec<(String, i64)> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| Some((x.get(0)?.as_str()?.to_string(), x.get(1)?.as_i64()?)))
        .collect()
}

/// Signature d'un store : (labels, types d'arêtes) avec leurs volumes.
///
/// C'est la partie **lente** de la description d'un monde, et parfaitement
/// stable — un store est un instantané. On la mémorise sur disque, validée par
/// le mtime du store : exactement le mécanisme dont se sert déjà l'`overview` de
/// la vue. Sans cette mémoire, chaque tour de conversation repayait 2,3 s
/// (runtime-dev) à 3,5 s (linux-full3).
pub fn signature_du_store(
    db: &ObrainDB,
    store: &Path,
) -> (Vec<(String, i64)>, Vec<(String, i64)>) {
    let mt = crate::viz_serve::store_mtime(store);
    if let Some(b) = crate::viz_serve::disk_load("engsig", store, "", mt)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&b)
    {
        return (paires(&v["labels"]), paires(&v["rels"]));
    }
    let labels: Vec<(String, i64)> = lire(
        db,
        "MATCH (n) RETURN labels(n)[0] AS t, count(n) AS w ORDER BY w DESC LIMIT 14",
    )
    .into_iter()
    .filter_map(|r| Some((txt(r.first())?, ent(r.get(1)))))
    .collect();
    let rels: Vec<(String, i64)> = lire(
        db,
        "MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS w ORDER BY w DESC LIMIT 14",
    )
    .into_iter()
    .filter_map(|r| Some((txt(r.first())?, ent(r.get(1)))))
    .collect();
    let body = serde_json::json!({ "labels": labels, "rels": rels }).to_string();
    crate::viz_serve::disk_save("engsig", store, "", mt, &body);
    (labels, rels)
}

/// Totaux du store : `(nœuds, arêtes)`. Mesuré à 0,04 ms — jamais caché.
fn totaux(db: &ObrainDB) -> (i64, i64) {
    let n = lire(db, "MATCH (n) RETURN count(n)")
        .first()
        .map(|r| ent(r.first()))
        .unwrap_or(0);
    let r = lire(db, "MATCH ()-[r]->() RETURN count(r)")
        .first()
        .map(|r| ent(r.first()))
        .unwrap_or(0);
    (n, r)
}

/// Plafond de nœuds au-delà duquel on renonce aux recherches NON INDEXÉES pour
/// résoudre le sujet (`name`, et suffixe de `qualified_name`).
///
/// La résolution d'un sujet par l'extracteur de la viz connaît trois voies :
///
/// | voie | index | coût mesuré (runtime-dev, 383 k nœuds) |
/// |---|---|---|
/// | `n.qualified_name = …` | d'égalité | **0,6 ms** |
/// | `n {qualified_name: …}` | d'égalité | **1,0 ms** |
/// | `n.name = …` | **aucun** | **4 981 ms** — parcours complet |
/// | `n.qualified_name ENDS WITH '.…'` | aucun | parcours complet |
///
/// Deux constats mesurés commandent la règle :
///
/// 1. un sujet ABSENT faisait payer **14 040 ms** avant d'échouer, parce que ces
///    parcours sont payés intégralement même quand ils ne trouvent rien ;
/// 2. `name` n'est pas indexé sur ces stores — 91 ms quand le nœud est en tête
///    du parcours (`dsh-runtime`), 4 981 ms quand il n'y est pas. Un `name` qui
///    répond vite ne prouve donc rien sur le coût du `name` qui ne répond pas.
///
/// En dessous de ce seuil, ces voies restent abordables et utiles : c'est elles
/// qui permettent de viser `main` sans donner le qn entier. Au-dessus, on
/// s'en tient aux index et on **le dit** — une limite annoncée, jamais
/// silencieuse.
const SEUIL_RECHERCHE_SUFFIXE: i64 = 200_000;

/// Ce qu'on sait du sujet **avant** de payer une extraction.
enum Resolution {
    /// Le sujet est là, ou les voies non indexées restent abordables : on extrait.
    Proceder,
    /// Aucune voie indexée ne le trouve, et le store est petit : les voies non
    /// indexées valent d'être tentées, on laisse l'extracteur trancher.
    AConfirmerParExtraction,
    /// Le sujet n'existe pas dans ce store (index négatif, petit store).
    Absent,
    /// Les index rapides sont négatifs et les voies non indexées sont hors
    /// budget : on ne peut pas affirmer l'absence, seulement l'ignorer en le
    /// disant.
    AbsentIndexSeulement,
}

/// Le sujet est-il un nœud de ce store — sans payer un parcours complet ?
///
/// Les index interrogés ici sont des index d'égalité : mesuré à **0,6 ms** sur
/// un store de 383 k nœuds. C'est ce contrôle qui remplace un échec à 14 s.
fn resoudre_sujet(db: &ObrainDB, sujet: &str, total_noeuds: i64) -> Resolution {
    // Identifiant numérique : l'extracteur le prend tel quel.
    if sujet.parse::<u64>().is_ok() {
        return Resolution::Proceder;
    }
    // Guillemets et antislash sont refusés par l'extracteur : le sujet ne peut
    // donc pas s'échapper de la chaîne GQL ci-dessous, qui reste sûre.
    if sujet.contains(['\'', '"', '\\']) {
        return Resolution::Absent;
    }
    // Voie INDEXÉE — la seule qui soit bon marché, et la seule qu'on s'autorise
    // sur un gros store. Les deux formes sont essayées : elles coûtent 0,6 ms et
    // 1,0 ms, donc les payer toutes les deux reste négligeable.
    let existe = |gql: String| lire(db, &gql).first().is_some();
    if existe(format!(
        "MATCH (n) WHERE n.qualified_name = '{sujet}' RETURN id(n) LIMIT 1"
    )) || existe(format!(
        "MATCH (n {{qualified_name: '{sujet}'}}) RETURN id(n) LIMIT 1"
    )) {
        return Resolution::Proceder;
    }
    if total_noeuds > SEUIL_RECHERCHE_SUFFIXE {
        // `name` (4 981 ms à vide) et le suffixe : HORS BUDGET. On ne les tente
        // pas et on le dira.
        return Resolution::AbsentIndexSeulement;
    }
    Resolution::AConfirmerParExtraction
}

// ── CE QU'ON LIT D'UN MONDE ─────────────────────────────────────────────────

/// Contexte d'un MONDE : un monde n'est pas un nœud du store interrogé.
///
/// Constat d'usage : depuis le Cosmos, le sujet est `world:<slug>` (l'identité
/// de la base dans la toile) — un nœud qui vit dans la BASE SYNAPTIQUE, pas dans
/// le store interrogé. Un monde se décrit donc par sa SIGNATURE réelle, ce qui
/// est non seulement correct mais plus utile : c'est ce qu'on veut savoir d'un
/// monde.
///
/// ## Ce qui a changé (2026-09-11) — l'échantillon n'est plus l'ordre de stockage
///
/// Avant : `MATCH (n) RETURN n.qualified_name LIMIT 24`. Sur `this-rs/this`,
/// ces 24 nœuds étaient 22 titres de `.github/ISSUE_TEMPLATE/`. Maintenant :
///
///   - **noyau structurel** : de VRAIES arêtes, lues par type (`CALLS`, `DEFINES`…).
///     Leurs extrémités sont des nœuds *engagés dans la structure*, pas les
///     premiers tombés sur le disque. C'est la seule sélection d'ancres qui soit
///     à la fois peu coûteuse (5-70 ms) et défendable ;
///   - **représentativité par type** : `NOEUDS_PAR_LABEL` nœuds pour chacun des
///     `LABELS_ECHANTILLONNES` labels les plus peuplés. Coût mesuré : 0,3-12 ms ;
///   - **la part lue est annoncée** : « 46 nœuds sur 4 685 (1,0 %) ». Un plafond
///     tu est un mensonge par omission ;
///   - **ce qui reste à densifier est nommé** : le modèle sait quels types
///     existent et en quel volume, donc il peut demander la suite (voir la
///     boucle d'outils) au lieu de conclure sur 1 % de la matière.
///
/// Ce qu'on ne fait **pas**, et pourquoi : classer par degré coûte 14,1 s sur
/// runtime-dev et 14,3 s sur linux-full3 (mesuré). On ne prétend donc pas
/// montrer le cœur du graphe — on montre un échantillon reçu comme tel.
/// LA CARTE D'UN STORE — ce qu'on lit quand on veut le décrire, mémorisé.
///
/// On regroupe ici les quatre lectures qui décrivent un store : la signature
/// (labels, types d'arêtes), les **arêtes réelles** échantillonnées par type, les
/// **échantillons par label**, et les têtes de base. Elles ont un point commun
/// décisif : elles sont **coûteuses** (mesuré : 1 300-2 071 ms pour les seules
/// arêtes sur runtime-dev) et **parfaitement stables** — un store obrain est un
/// instantané.
///
/// Les recalculer à chaque tour de conversation était le défaut : le contexte
/// coûtait plusieurs secondes avant même d'appeler le modèle, pour un résultat
/// identique d'un tour à l'autre. On les mémorise donc sur disque, validées par
/// `viz_serve::store_mtime` — c'est-à-dire par le **manifeste** du store
/// (`substrate.meta`), qui change quand les DONNÉES changent, pas quand le
/// journal d'activation écrit une ligne.
///
/// Une seule règle de fraîcheur dans tout le serveur, et c'est la bonne : la
/// première version de cette carte la validait sur le **maximum des mtime des
/// fichiers**, ce qui inclut le WAL et le journal d'activation — donc la carte
/// du store VIVANT (`runtime-dev`, écrit en permanence) n'était jamais
/// réutilisée, et le chat repayait son contexte à chaque tour. Deux règles pour
/// une même chose, c'est un défaut ; ici c'était aussi une régression de
/// performance déguisée en prudence.
#[derive(Clone, Debug, Default)]
struct CarteStore {
    /// (label, nombre de nœuds).
    labels: Vec<(String, i64)>,
    /// (type d'arête, nombre d'arêtes).
    rels: Vec<(String, i64)>,
    /// Arêtes réelles lues, déjà mises en forme (`a -[T]-> b`).
    aretes: Vec<String>,
    /// Extrémités d'arêtes réelles — les nœuds engagés dans la structure.
    ancre_reelle: Vec<String>,
    /// (label, nœud) : l'échantillon représentatif par type.
    echantillons: Vec<String>,
    /// (label, total, lus) — la densification disponible, chiffrée.
    par_label: Vec<(String, i64, usize)>,
    /// Premiers nœuds du stockage (souvent la racine). Nommés comme tels.
    tetes: Vec<String>,
}

impl CarteStore {
    fn vers_json(&self) -> serde_json::Value {
        serde_json::json!({
            "labels": self.labels,
            "rels": self.rels,
            "aretes": self.aretes,
            "ancre_reelle": self.ancre_reelle,
            "echantillons": self.echantillons,
            "par_label": self.par_label,
            "tetes": self.tetes,
        })
    }

    fn depuis_json(v: &serde_json::Value) -> Option<Self> {
        let paires = |x: &serde_json::Value| -> Vec<(String, i64)> {
            x.as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_i64()?)))
                .collect()
        };
        let strs = |x: &serde_json::Value| -> Vec<String> {
            x.as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        };
        let par_label = v["par_label"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let a = p.as_array()?;
                Some((
                    a.first()?.as_str()?.to_string(),
                    a.get(1)?.as_i64()?,
                    a.get(2)?.as_u64()? as usize,
                ))
            })
            .collect();
        Some(CarteStore {
            labels: paires(&v["labels"]),
            rels: paires(&v["rels"]),
            aretes: strs(&v["aretes"]),
            ancre_reelle: strs(&v["ancre_reelle"]),
            echantillons: strs(&v["echantillons"]),
            par_label,
            tetes: strs(&v["tetes"]),
        })
    }
}

/// Lit la carte du store — mémorisée si le store n'a pas bougé depuis.
///
/// Le retour porte aussi `memoire` : vrai quand rien n'a été recalculé. C'est
/// exposé à l'écran parce qu'un temps de lecture qui s'effondre sans explication
/// ressemble à une mesure truquée ; avec le drapeau, il est vérifiable.
fn carte_du_store(db: &ObrainDB, store: &Path) -> (CarteStore, bool) {
    let mt = crate::viz_serve::store_mtime(store);
    // La fenêtre FAIT PARTIE de la clé : la carte contient un échantillon dont la
    // taille en dépend.
    let cle_fenetre = format!("w{}", fenetre_jetons());
    if let Some(b) = crate::viz_serve::disk_load("engcarte2", store, &cle_fenetre, mt)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&b)
        && let Some(c) = CarteStore::depuis_json(&v)
        && !c.labels.is_empty()
    {
        return (c, true);
    }
    let (labels, rels) = signature_du_store(db, store);
    let mut carte = CarteStore { labels, rels, ..Default::default() };

    // ── Le noyau structurel : de VRAIES arêtes, par type ────────────────────
    // Le libellé des extrémités est lu DANS la même requête : on ne cite jamais
    // une arête dont on n'a pas les deux bouts.
    for (t, _) in carte.rels.iter().take(types_aretes()).cloned().collect::<Vec<_>>() {
        if !type_interpolable(&t) {
            continue;
        }
        // ── LE TYPE DE CHAQUE EXTRÉMITÉ EST LU AVEC L'ARÊTE ─────────────────
        // Défaut mesuré le 2026-09-11 : cette liste était SANS type. À la
        // question « montre-moi 20 Method », le modèle a listé des noms RÉELS
        // dont trois sur six étaient un `Function`, un `Field` et une
        // `Variable` : il a attribué le type DEMANDÉ à des nœuds qu'il ne
        // pouvait pas typer. Les noms étaient justes, la nature était fausse.
        //
        // On lit donc `labels(a)[0]` dans la MÊME requête (coût nul : l'expansion
        // tourne déjà) et on l'écrit à côté de chaque nœud. Une information
        // juste vaut mieux qu'une consigne de plus.
        let q = format!(
            "MATCH (a)-[r:{t}]->(b) RETURN a.qualified_name, labels(a)[0], \
             b.qualified_name, labels(b)[0] LIMIT {}",
            aretes_par_type()
        );
        for row in lire(db, &q) {
            let (Some(a), Some(la), Some(b), Some(lb)) = (
                txt(row.first()),
                txt(row.get(1)),
                txt(row.get(2)),
                txt(row.get(3)),
            ) else {
                continue;
            };
            carte.aretes.push(format!("{a} [{la}] -[{t}]-> {b} [{lb}]"));
            carte.ancre_reelle.push(format!("{a} [{la}]"));
            carte.ancre_reelle.push(format!("{b} [{lb}]"));
        }
    }

    // ── La représentativité par type : n nœuds par label ────────────────────
    for (label, total) in carte.labels.iter().take(labels_echantillonnes()).cloned().collect::<Vec<_>>() {
        // Un label ne s'interpole que s'il est un identifiant.
        if !type_interpolable(&label) {
            carte.par_label.push((label, total, 0));
            continue;
        }
        let q = format!(
            "MATCH (n:{label}) RETURN n.qualified_name, n.file_path LIMIT {}",
            noeuds_par_label()
        );
        let mut lus = 0usize;
        for row in lire(db, &q) {
            let Some(qn) = txt(row.first()) else { continue };
            let mut s = qn;
            if let Some(f) = txt(row.get(1))
                && !f.is_empty()
                && f != "{}"
            {
                s.push_str(&format!(" ({f})"));
            }
            carte.echantillons.push(s);
            lus += 1;
        }
        carte.par_label.push((label, total, lus));
    }

    // ── LES HUBS : LE CŒUR DU GRAPHE, ENFIN ABORDABLE ──────────────────────
    // Le classement par degré était IMPOSSIBLE dans le chemin du chat :
    // `MATCH (n)--(m) … ORDER BY count(m) DESC` coûte 14,1 s sur runtime-dev, et
    // l'outil refusait la requête. Les registres de degrés du substrat répondent
    // en O(1) par nœud : l'entretien calcule donc ce classement (mesuré : 16 ms
    // sur 401 k nœuds) et le persiste ; ici on le RELIT, en millisecondes.
    //

    // Les têtes de base (premiers nœuds du stockage) : c'est souvent la racine du
    // projet. On les NOMME comme telles — « tête de base » n'est pas un rang.
    carte.tetes = lire(db, "MATCH (n) RETURN n.qualified_name LIMIT 3")
        .into_iter()
        .filter_map(|r| txt(r.first()))
        .collect();

    crate::viz_serve::disk_save("engcarte2", store, &cle_fenetre, mt, &carte.vers_json().to_string());
    (carte, false)
}

/// PROFIL DU SELF DU COMPTE — lu dans SA base dediee (`self-<compte>.obrain`,
/// posee a cote du store runtime par la passe `memoire --appliquer`).
/// Ouverture en LECTURE breve : le handle est rendu aussitot, aucun verrou
/// garde — la passe d application (exclusive) n est jamais bloquee par une
/// conversation. Rend None sans base ou sans contenu : l injection
/// n empeche JAMAIS une conversation.
fn profil_du_self(ancrage: &Ancrage) -> Option<String> {
    let compte = ancrage.self_qn.strip_prefix("self:")?;
    if compte.is_empty()
        || compte == "agent"
        || !compte.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None; // le Self de l agent a deja son propre circuit
    }
    // ESPACE DE NOMS DEDIE : `selfs/<compte>.obrain`. Le plat
    // `self-<compte>.obrain` ENTRAIT EN COLLISION avec les bases
    // historiques du repertoire (mesure : le compte « demo » heritait de
    // `self-demo.obrain`, une base de demonstration sans rapport — 8
    // lignes injectees a un compte vierge). Un sous-repertoire ne peut
    // percuter aucune base existante.
    let base = std::path::Path::new(&ancrage.store)
        .parent()?
        .join("selfs")
        .join(format!("{compte}.obrain"));
    if !base.is_dir() {
        return None;
    }
    let db = ObrainDB::open_read_only(&base).ok()?;
    let s = db.session();
    let tronque = |t: &str| -> String {
        let t = t.trim();
        if t.chars().count() > 160 {
            let mut c: String = t.chars().take(157).collect();
            c.push_str("...");
            c
        } else {
            t.to_string()
        }
    };
    let mut lignes: Vec<String> = Vec::new();
    if let Ok(r) = s.execute(
        "MATCH (:Self)-[:HAS_NOTE]->(n:Note) RETURN n.sujet, n.text LIMIT 12",
    ) {
        for row in &r.rows {
            let sujet = row.first().map(|v| v.to_string()).unwrap_or_default();
            let texte = row.get(1).map(|v| v.to_string()).unwrap_or_default();
            if !texte.is_empty() && texte != "NULL" {
                lignes.push(format!("  - [{sujet}] {}", tronque(&texte)));
            }
        }
    }
    if let Ok(r) = s.execute(
        "MATCH (:Self)-[:HAS_GOAL]->(g:Goal) RETURN g.name, g.text LIMIT 6",
    ) {
        for row in &r.rows {
            let nom = row.first().map(|v| v.to_string()).unwrap_or_default();
            let texte = row.get(1).map(|v| v.to_string()).unwrap_or_default();
            if !nom.is_empty() && nom != "NULL" {
                lignes.push(format!("  - OBJECTIF {nom} : {}", tronque(&texte)));
            }
        }
    }
    if let Ok(r) = s.execute(
        "MATCH (:Self)-[:HAS_STATE]->(st:SelfState) RETURN st.version, st.epithet, st.objective ORDER BY st.version DESC LIMIT 1",
    ) {
        if let Some(row) = r.rows.first() {
            let v = row.first().and_then(|x| x.as_int64()).unwrap_or(0);
            let ep = row.get(1).map(|x| x.to_string()).unwrap_or_default();
            let ob = row.get(2).map(|x| x.to_string()).unwrap_or_default();
            if v > 0 {
                lignes.push(format!("  - ETAT v{v} : {ep} — {}", tronque(&ob)));
            }
        }
    }
    if lignes.is_empty() {
        None
    } else {
        eprintln!(
            "[self] profil injecte : {} ligne(s) pour {}",
            lignes.len(),
            ancrage.self_qn
        );
        Some(lignes.join("\n"))
    }
}

fn contexte_du_monde(
    db: &ObrainDB,
    soumis: &str,
    ancrage: &Ancrage,
    est_un_monde: bool,
) -> Result<Contexte> {
    let t = Instant::now();
    let (total_noeuds, total_aretes) = totaux(db);
    let mut dec = Decomposition { totaux_ms: t.elapsed().as_millis() as u64, ..Default::default() };

    let t = Instant::now();
    let (carte, memoire) = carte_du_store(db, Path::new(&ancrage.store));
    dec.signature_ms = t.elapsed().as_millis() as u64;
    dec.carte_memoire = memoire;
    let CarteStore { rels, aretes: relations, ancre_reelle, echantillons, par_label, tetes, .. } = carte;

    // Les extrémités d'arêtes réelles rejoignent l'échantillon : ce sont les
    // nœuds dont on sait qu'ils PORTENT de la structure.
    let mut notes_structurales: Vec<String> = Vec::new();
    for a in ancre_reelle {
        if notes_structurales.len() < 12 && !notes_structurales.contains(&a) {
            notes_structurales.push(a);
        }
    }

    // ── LA SECTION DES HUBS ────────────────────────────────────────────────
    // Elle est écrite AVANT l'échantillon par type : ce sont les nœuds qui
    // portent la structure, et le lecteur doit les voir d'abord.
    //
    // Les hubs ont leur PROPRE fichier (l'entretien les recalcule à chaque
    // passe) : on ne les duplique pas dans la carte mémorisée du store.
    let classement: Option<crate::hubs::Classement> =
        crate::hubs::lire(std::path::Path::new(&ancrage.store), crate::viz_serve::store_mtime(std::path::Path::new(&ancrage.store)));
    let mut lignes_hubs: Vec<String> = Vec::new();
    let (hubs, hubs_ms) = match &classement {
        Some(c) => (c.hubs.as_slice(), c.calcul_ms),
        None => (&[][..], 0),
    };
    if let Some(refus) = classement.as_ref().and_then(|c| c.refus.as_deref()) {
        // Un refus est une INFORMATION, pas un vide : on le transmet tel quel.
        lignes_hubs.push(format!("  CLASSEMENT REFUSÉ — {refus}"));
    } else if hubs.is_empty() {
        lignes_hubs.push(
            "  (classement par degré non encore calculé — l'entretien le produit et le \
             persiste ; voir les registres de degrés du substrat)"
                .to_string(),
        );
    } else {
        lignes_hubs.push(format!(
            "  ({} nœuds les plus connectés, par REGISTRE de degrés — {} ms de calcul,              pas de parcours)",
            hubs.len(),
            hubs_ms
        ));
        lignes_hubs.extend(
            crate::hubs::lignes(hubs, 20)
                .into_iter()
                .map(|l| format!("  {l}")),
        );
    }

    let mut noeuds = echantillons;
    let mut relations = relations;
    noeuds.sort();
    noeuds.dedup();
    noeuds.truncate(budget_noeuds());
    relations.sort();
    relations.dedup();
    relations.truncate(max_relations());

    let lus = noeuds.len();
    let part = part_lue(lus, total_noeuds, 1);

    // ── Le texte : borné par le budget de la fenêtre, et la coupe est DITE ──
    let mut texte = String::new();
    let mut coupe = false;
    let budget = budget_caracteres();
    let ajouter = |texte: &mut String, ligne: String, coupe: &mut bool| {
        if texte.len() + ligne.len() + 1 > budget {
            *coupe = true;
            return;
        }
        texte.push_str(&ligne);
        texte.push('\n');
    };

    if est_un_monde {
        let nom = ancrage
            .monde
            .clone()
            .unwrap_or_else(|| soumis.trim_start_matches("world:").to_string());
        ajouter(
            &mut texte,
            format!("SUJET : {soumis} — le MONDE « {nom} » (pas un nœud précis)"),
            &mut coupe,
        );
    } else {
        ajouter(&mut texte, format!("SUJET : {soumis}"), &mut coupe);
    }
    ajouter(&mut texte, format!("STORE LU : {}", ancrage.store), &mut coupe);
    ajouter(
        &mut texte,
        format!(
            "SELF DE L'AGENT : {} — c'est à lui que cette lecture, et ce qui en \
             sera structuré, appartiennent",
            ancrage.self_qn
        ),
        &mut coupe,
    );
    // ── LE SELF DU COMPTE CONNECTE (T12) ────────────────────────────────────
    // Quand la conversation appartient a un COMPTE (self:<utilisateur>), son
    // profil — ce que SES conversations ont structure — entre en tete du
    // contexte : l assistant le connait, conversation apres conversation.
    if let Some(profil) = profil_du_self(ancrage) {
        ajouter(
            &mut texte,
            "\nPROFIL DU COMPTE (son Self — retenu de ses conversations) :".to_string(),
            &mut coupe,
        );
        for l in profil.lines() {
            ajouter(&mut texte, l.to_string(), &mut coupe);
        }
    }
    ajouter(
        &mut texte,
        format!("CE QUE LA BASE CONTIENT : {total_noeuds} nœuds · {total_aretes} arêtes"),
        &mut coupe,
    );
    ajouter(
        &mut texte,
        "NŒUDS LES PLUS CONNECTÉS (le cœur du graphe) :".to_string(),
        &mut coupe,
    );
    for l in &lignes_hubs {
        ajouter(&mut texte, l.clone(), &mut coupe);
    }
    ajouter(
        &mut texte,
        format!(
            "CE QUI A ÉTÉ LU : {lus} nœuds ({part}) · {} arêtes réelles — \
             l'échantillon est représentatif par TYPE, il n'est pas exhaustif",
            relations.len()
        ),
        &mut coupe,
    );

    ajouter(&mut texte, "\nTYPES DE CONNAISSANCE (label — nœuds — dont lus) :".into(), &mut coupe);
    for (label, total, lus_label) in &par_label {
        ajouter(
            &mut texte,
            format!("  - {label} — {total} (dont {lus_label} lus)"),
            &mut coupe,
        );
    }

    ajouter(&mut texte, "\nRELATIONS RÉELLES (type — arêtes) :".into(), &mut coupe);
    for (t, w) in &rels {
        ajouter(&mut texte, format!("  - {t} — {w}"), &mut coupe);
    }

    if !tetes.is_empty() {
        ajouter(
            &mut texte,
            "\nTÊTES DE BASE (premiers nœuds du stockage — pas un classement) :".into(),
            &mut coupe,
        );
        for t in &tetes {
            ajouter(&mut texte, format!("  - {t}"), &mut coupe);
        }
    }

    if !notes_structurales.is_empty() {
        ajouter(
            &mut texte,
            "\nNŒUDS ENGAGÉS DANS LA STRUCTURE (extrémités d'arêtes réelles lues ; le TYPE est donné entre crochets) :".into(),
            &mut coupe,
        );
        for n in &notes_structurales {
            ajouter(&mut texte, format!("  - {n}"), &mut coupe);
        }
    }

    ajouter(
        &mut texte,
        format!("\nÉCHANTILLON PAR TYPE ({} nœuds, un par ligne) :", noeuds.len()),
        &mut coupe,
    );
    for n in &noeuds {
        ajouter(&mut texte, format!("  - {n}"), &mut coupe);
    }

    ajouter(&mut texte, "\nARÊTES RÉELLEMENT LUES :".into(), &mut coupe);
    for r in &relations {
        ajouter(&mut texte, format!("  - {r}"), &mut coupe);
    }

    ajouter(
        &mut texte,
        format!(
            "\nPOUR DENSIFIER : cette lecture ne couvre que {part} des nœuds. Les types \
             ci-dessus sont tous interrogeables — demandez un type précis (ses nœuds, ses \
             arêtes), le voisinage d'un nœud cité, ou les modules du projet. Ne conclus pas \
             sur ce que cet échantillon ne montre pas."
        ),
        &mut coupe,
    );
    if coupe {
        ajouter(
            &mut texte,
            "\nNOTE : le contexte a été coupé au budget de caractères — la fin des listes \
             manque, elle n'est pas vide."
                .into(),
            &mut coupe,
        );
    }

    Ok(Contexte {
        qn: soumis.to_string(),
        noeuds,
        relations,
        texte,
        monde: ancrage.monde.clone(),
        store: ancrage.store.clone(),
        total_noeuds,
        total_aretes,
        par_label,
        self_qn: ancrage.self_qn.clone(),
        hubs: lignes_hubs
            .iter()
            .filter(|l| !l.trim_start().starts_with('('))
            .take(8)
            .map(|l| l.trim().to_string())
            .collect(),
        hubs_ms,
        decompose: dec,
    })
}

/// Construit le contexte d'un sujet : son voisinage réel à 1 saut.
pub fn contexte_du_sujet(db: &ObrainDB, sujet: &str, ancrage: &Ancrage) -> Result<Contexte> {
    // Un monde se décrit par sa signature ; un nœud par son voisinage.
    if sujet.starts_with("world:") {
        return contexte_du_monde(db, sujet, ancrage, true);
    }
    // ── LE SUJET EST-IL LÀ ? ────────────────────────────────────────────────
    // Mesuré : un sujet absent de ce store faisait payer **14 040 ms** — le
    // temps que l'extracteur parcoure tout le store à la recherche d'un suffixe
    // qui n'existe pas — avant d'échouer et de retomber sur la signature. Le
    // contrôle d'existence par index coûte 0,08 ms. On le fait donc AVANT.
    let t = Instant::now();
    let (total_avant, _) = totaux(db);
    let mut dec = Decomposition { totaux_ms: t.elapsed().as_millis() as u64, ..Default::default() };
    let t = Instant::now();
    let resolution = resoudre_sujet(db, sujet, total_avant);
    dec.resolution_ms = t.elapsed().as_millis() as u64;
    if !matches!(resolution, Resolution::Proceder) {
        let hors_budget = matches!(resolution, Resolution::AbsentIndexSeulement);
        let mut c = contexte_du_monde(db, sujet, ancrage, false)?;
        let pourquoi = if hors_budget {
            format!(
                "« {sujet} » n'est pas un `qualified_name` de ce store. Les voies NON INDEXÉES \
                 ont été ÉVITÉES : sur {total_avant} nœuds, `n.name = …` coûte un parcours \
                 complet (mesuré 4 981 ms à vide) et le suffixe davantage. L'absence n'est donc \
                 pas prouvée, elle est probable. Donnez un `qualified_name` exact pour un \
                 voisinage précis."
            )
        } else {
            format!("« {sujet} » n'est pas un nœud de ce store.")
        };
        c.texte = format!(
            "NOTE : {pourquoi} Contexte fourni : la signature du store lui-même.\n\n{}",
            c.texte
        );
        return Ok(c);
    }
    let opts = ExtractOptions {
        hops: 1,
        closure_max_depth: None,
        limit_nodes: Some(budget_noeuds() + 20),
        edge_types: None,
        outbound: true,
        inbound: true,
        signals: true,
        ..Default::default()
    };
    let t = Instant::now();
    let (sg, rapport) = match extract_subgraph_centers(db, &[sujet.to_string()], &opts) {
        Ok(v) => v,
        // Sujet introuvable DANS CE STORE (monde, nœud d'une autre base) : plutôt
        // que d'échouer, on décrit le store — et on le DIT dans le contexte.
        Err(_) => {
            let mut c = contexte_du_monde(db, sujet, ancrage, false)?;
            c.texte = format!(
                "NOTE : « {sujet} » n'est pas un nœud de ce store. Contexte fourni : \
                 la signature du store lui-même.\n\n{}",
                c.texte
            );
            return Ok(c);
        }
    };

    dec.voisinage_ms = t.elapsed().as_millis() as u64;
    dec.voisinage_phases = rapport.phases;

    let nom = |id: &str| -> String {
        sg.nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.qn.clone().or_else(|| n.label.clone()))
            .unwrap_or_else(|| id.to_string())
    };

    let mut noeuds: Vec<String> = sg
        .nodes
        .iter()
        .take(budget_noeuds())
        .map(|n| {
            let qn = n.qn.clone().unwrap_or_else(|| n.id.clone());
            let mut s = qn;
            if let Some(e) = n.energy {
                s.push_str(&format!(" [énergie {e:.2}]"));
            }
            if let Some(c) = n.curvature {
                s.push_str(&format!(" [courbure {c:.2}]"));
            }
            if let Some(f) = &n.file_path {
                s.push_str(&format!(" ({f})"));
            }
            s
        })
        .collect();
    noeuds.sort();
    noeuds.dedup();

    let mut relations: Vec<String> = sg
        .edges
        .iter()
        .take(max_relations())
        .map(|e| format!("{} -[{}]-> {}", nom(&e.src), e.edge_type, nom(&e.dst)))
        .collect();
    relations.sort();
    relations.dedup();

    // La part lue est annoncée ici aussi : un voisinage à 1 saut n'est pas le
    // store, et le dire évite de laisser croire à une lecture exhaustive.
    let (total_noeuds, total_aretes) = totaux(db);
    let part = part_lue(noeuds.len(), total_noeuds, 3);

    let mut texte = String::new();
    texte.push_str(&format!("SUJET : {sujet}\n"));
    texte.push_str(&format!("STORE LU : {}\n", ancrage.store));
    texte.push_str(&format!(
        "CE QUE LA BASE CONTIENT : {total_noeuds} nœuds · {total_aretes} arêtes\n"
    ));
    texte.push_str(&format!(
        "CE QUI A ÉTÉ LU : {} nœuds ({part}) — voisinage réel à 1 saut du sujet, \
         pas le store entier\n",
        noeuds.len()
    ));
    texte.push_str(&format!("\nNŒUDS DU VOISINAGE ({} lus) :\n", noeuds.len()));
    for n in &noeuds {
        texte.push_str("  - ");
        texte.push_str(n);
        texte.push('\n');
    }
    texte.push_str(&format!("\nRELATIONS RÉELLES ({} lues) :\n", relations.len()));
    for r in &relations {
        texte.push_str("  - ");
        texte.push_str(r);
        texte.push('\n');
    }
    texte.push_str(
        "\nPOUR DENSIFIER : demandez le voisinage d'un autre nœud cité, ou la signature \
         du store (types et volumes), plutôt que de conclure sur ce seul voisinage.\n",
    );

    Ok(Contexte {
        qn: sujet.to_string(),
        noeuds,
        relations,
        texte,
        monde: ancrage.monde.clone(),
        store: ancrage.store.clone(),
        total_noeuds,
        total_aretes,
        par_label: Vec::new(),
        self_qn: ancrage.self_qn.clone(),
        // Un voisinage à 1 saut n'est pas un store : il n'y a pas de classement
        // par degré à en tirer, et on ne fabrique pas une liste vide qui se
        // lirait comme « aucun hub ».
        hubs: Vec::new(),
        hubs_ms: 0,
        decompose: dec,
    })
}

/// Réponse de l'engine, telle qu'on la renvoie au dashboard.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ReponseEngine {
    pub question: String,
    pub reponse: String,
    pub modele: String,
    pub sujet: String,
    /// Vrai quand le texte provient du raisonnement faute de `content`.
    pub depuis_raisonnement: bool,
    pub noeuds_contexte: usize,
    pub relations_contexte: usize,
    pub ms_moteur: u64,
    pub ms_llm: u64,
    /// Le plafond DEMANDÉ par l'appelant (`max_tokens` de la requête).
    pub max_tokens_demande: u32,
    /// Le plafond RÉELLEMENT envoyé au service, après bornage par la fenêtre.
    ///
    /// C'est lui qui explique la durée : le modèle local produit ~0,14 s/jeton
    /// (mesuré). Un `ms_llm` de 50 s pour 365 jetons se vérifie ici.
    pub max_tokens_effectif: u32,
    /// Jetons réellement produits, tels que le service les compte (0 si le
    /// service ne les annonce pas).
    pub jetons_sortie: u32,
    /// Vrai quand le modèle a ÉPUISÉ son plafond : la réponse peut être coupée.
    ///
    /// Il faut le dire plutôt que de laisser croire à une fin naturelle — c'est
    /// la même règle que le contexte tronqué, qui s'annonce lui aussi.
    pub plafond_atteint: bool,
    /// La pensée a-t-elle été autorisée pour cet appel ? Sans cela, une réponse
    /// courte et directe serait indistinguable d'un modèle bridé.
    pub raisonnement_demande: bool,
    /// OÙ le temps du MOTEUR est réellement passé (totaux, résolution, signature,
    /// voisinage), en ms.
    ///
    /// `ms_moteur` seul n'est pas un diagnostic : il annonçait ~900 ms pour lire
    /// **4 nœuds et 3 relations**, sans dire quelle étape les payait. La
    /// décomposition existait déjà pour le contexte ; elle est publiée ici pour
    /// que la même lecture serve au même endroit.
    pub moteur_decompose: serde_json::Value,
    /// Le contexte RÉEL envoyé, pour que l'utilisateur vérifie sur quoi on a
    /// raisonné — un LLM qui répond sans montrer ses entrées n'est pas
    /// vérifiable.
    pub contexte: String,
}

/// Le prompt système de la route BLOQUANTE (`demander`), qui n'envoie AUCUN
/// outil au modèle.
///
/// Défaut mesuré le 2026-09-12 : les deux routes partageaient le même texte,
/// dont la règle 1 ordonnait « APPELLE UN OUTIL — `obrain_echantillon` pour un
/// type, `obrain_query` pour une requête ». Or `demander` ne joint jamais de
/// champ `tools` (le seul du fichier est dans `demander_stream`) : le modèle
/// était donc sommé d'appeler des outils qu'il n'avait pas. Il ne pouvait que
/// promettre l'appel en prose, ou consumer son plafond à l'annoncer — un coût
/// payé plein tarif quand le plafond est petit. Ici, le manque de matière se
/// DIT, il ne s'appelle pas.
const SYSTEME_SANS_OUTILS: &str = "Tu raisonnes sur un graphe de connaissances obrain (nœuds et \
relations réelles extraites d'une base). Réponds dans la LANGUE DE LA QUESTION — \
la même langue que celle où l'utilisateur a écrit —, uniquement à partir du \
CONTEXTE fourni. Ta réflexion éventuelle se fait dans CETTE MÊME LANGUE et elle \
est COURTE : quelques phrases de brouillon pour toi, pas un essai — la réponse \
est ce qui compte. Si le contexte ne permet pas de répondre, dis-le \
explicitement au lieu d'inventer. Cite les noms exacts des nœuds et des \
relations que tu utilises.\n\nDEUX RÈGLES, nées de défauts mesurés :\n1. Le contexte ne montre qu'un ÉCHANTILLON (le pourcentage lu est indiqué). Si la \
question demande une LISTE, un COMPTE par type, ou plus d'éléments que l'échantillon \
n'en montre, DIS-LE : aucun outil n'est disponible sur cette route, et extrapoler \
une liste à partir d'un échantillon serait une invention.\n2. N'attribue JAMAIS un type à un nœud s'il ne t'est pas donné : chaque nœud cité \
porte son type entre crochets. Annoncer « Method » pour un nœud non typé est une \
erreur, même si le nom est exact. Rends ta réponse en Markdown (titres, listes, \
tableaux, code) et écris les mathématiques en LaTeX : $…$ en ligne et $$…$$ \
pour une formule d'affichage.";

/// Le prompt système de la conversation EN FLUX (`demander_stream`), qui envoie
/// RÉELLEMENT les outils (`"tools": crate::outils::definitions()`) : la règle 1
/// y est donc applicable, et c'est elle qui déclenche les appels d'outils.
///
/// (Les deux textes annonçaient « TROIS RÈGLES » pour deux règles énumérées :
/// le compte est corrigé ici et dans l'autre prompt.)
const SYSTEME_AVEC_OUTILS: &str = "Tu raisonnes sur un graphe de connaissances obrain (nœuds et \
relations réelles extraites d'une base). Réponds dans la LANGUE DE LA QUESTION — \
la même langue que celle où l'utilisateur a écrit —, uniquement à partir du \
CONTEXTE fourni. Ta réflexion éventuelle se fait dans CETTE MÊME LANGUE et elle \
est COURTE : quelques phrases de brouillon pour toi, pas un essai — la réponse \
est ce qui compte. Si le contexte ne permet pas de répondre, dis-le \
explicitement au lieu d'inventer. Cite les noms exacts des nœuds et des \
relations que tu utilises.\n\nDEUX RÈGLES, nées de défauts mesurés :\n1. Le contexte ne montre qu'un ÉCHANTILLON (le pourcentage lu est indiqué). Si la \
question demande une LISTE, un COMPTE par type, ou plus d'éléments que l'échantillon \
n'en montre, APPELLE UN OUTIL — `obrain_echantillon` pour un type, `obrain_query` \
pour une requête. N'extrapole JAMAIS une liste à partir de l'échantillon.\n2. N'attribue JAMAIS un type à un nœud s'il ne t'est pas donné : chaque nœud cité \
porte son type entre crochets. Annoncer « Method » pour un nœud non typé est une \
erreur, même si le nom est exact. Rends ta réponse en Markdown (titres, listes, \
tableaux, code) et écris les mathématiques en LaTeX : $…$ en ligne et $$…$$ \
pour une formule d'affichage.";

/// Interroge l'engine : contexte réel → LLM → réponse.
///
/// `store` sert à inscrire l'activation (le graphe s'allume pendant l'appel).
/// `pensee` choisit si le modèle a le droit d'écrire sa pensée avant sa réponse
/// (voir `gabarit_de_pensee` : c'est un levier de 6× sur ce modèle).
pub fn demander(
    db: &ObrainDB,
    store: &Path,
    sujet: &str,
    question: &str,
    max_tokens: u32,
    pensee: bool,
    // Niveau d'effort de raisonnement (`low` / `medium` / `xhigh`) — voir
    // `gabarit_de_pensee`. Il part au template À LA PLACE de son défaut.
    effort: &str,
    // (T12) Compte connecte : route le Self de la conversation.
    compte: Option<&str>,
) -> Result<ReponseEngine> {
    let ancrage = Ancrage {
        store: store.to_string_lossy().to_string(),
        monde: None,
        self_qn: compte
            .map(|u| format!("self:{u}"))
            .unwrap_or_else(|| crate::ancrage::self_qn(None)),
    };
    let t0 = Instant::now();
    let ctx = contexte_du_sujet(db, sujet, &ancrage)?;
    let ms_moteur = t0.elapsed().as_millis() as u64;

    // LE GRAPHE S'ALLUME : ce que l'engine consulte est inscrit au journal
    // d'activation, que le dashboard relit en direct.
    crate::activation::record(store, &ctx.noeuds, "engine");
    if !ctx.qn.is_empty() {
        crate::activation::record(store, &[ctx.qn.clone()], "engine-sujet");
    }

    let systeme = SYSTEME_SANS_OUTILS;
    // Même budget partagé que dans le flux : la place de la réponse se réserve
    // AVANT d'envoyer, sinon c'est la requête entière que le service refuse.
    let fixe = jetons_estimes(systeme) + jetons_estimes(question);
    let sortie_demandee = max_tokens.max(SORTIE_MINIMALE) as usize;
    let pour_contexte = fenetre_jetons()
        .saturating_sub(fixe)
        .saturating_sub(sortie_demandee)
        .saturating_sub(MARGE_JETONS)
        .max(256);
    let (texte_contexte, tronque) = contexte_ajuste(&ctx.texte, pour_contexte);
    if tronque {
        eprintln!(
            "engine (ask) : contexte tronqué (fenêtre {} jetons · invite {fixe} · \
sortie réservée {sortie_demandee} · contexte gardé {pour_contexte})",
            fenetre_jetons()
        );
    }
    let restant =
        fenetre_jetons().saturating_sub(fixe + jetons_estimes(&texte_contexte) + MARGE_JETONS);
    // ── LE PLAFOND HONORE L'APPELANT ────────────────────────────────────────
    // `sortie_demandee` ci-dessus n'est que la place RÉSERVÉE dans la fenêtre
    // (elle, garde son plancher : c'est le service qui refuserait la requête
    // entière). Ce qui part réellement comme plafond est ce que l'appelant a
    // demandé — sinon `?max_tokens=24` coûtait 50 s.
    let plafond = plafond_generation(max_tokens, restant);
    let invite = format!(
        "CONTEXTE DU GRAPHE\n{}\n\nQUESTION DE L'UTILISATEUR\n{}",
        texte_contexte, question
    );

    let mut payload = serde_json::json!({
        "model": modele(),
        "messages": [
            { "role": "system", "content": systeme },
            { "role": "user", "content": invite },
        ],
        "max_tokens": plafond,
        "temperature": 0.2,
        "stream": false,
    });
    if let Some(gabarit) = gabarit_de_pensee(pensee, effort) {
        payload["chat_template_kwargs"] = gabarit;
    }
    let payload = payload.to_string();

    let t1 = Instant::now();
    // Délai généreux : un modèle local de 9 B peut mettre plusieurs dizaines de
    // secondes sur un contexte de 60 nœuds. Le serveur étant thread-par-requête,
    // cette attente ne bloque ni la navigation ni le sondage d'activation.
    let rep = crate::http_min::post_json(
        &format!("{}/v1/chat/completions", url_llm()),
        &payload,
        Duration::from_secs(180),
    )?;
    let ms_llm = t1.elapsed().as_millis() as u64;

    if rep.status != 200 {
        anyhow::bail!(
            "engine : le service LLM a répondu {} — {}",
            rep.status,
            rep.body.chars().take(300).collect::<String>()
        );
    }
    let v: serde_json::Value =
        serde_json::from_str(&rep.body).context("engine : réponse LLM illisible")?;
    let msg = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"));
    let content = msg
        .and_then(|m| m.get("content"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let raisonnement = msg
        .and_then(|m| m.get("reasoning_content"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let depuis_raisonnement = content.is_empty() && !raisonnement.is_empty();
    let reponse = if depuis_raisonnement { raisonnement } else { content };
    // DEUXIÈME IMPULSION : la consultation est instantanée (elle a allumé le
    // graphe au début), mais un modèle local met ici ~75 s à répondre — le
    // temps que la réponse arrive, la première activation a décru. On réinscrit
    // donc les mêmes nœuds à la CONCLUSION, ce qui correspond à l'événement
    // réel : l'engine relit ses preuves pour conclure. Mesuré : sans cela, le
    // pic retombait à 0,056 avant que l'utilisateur voie la réponse.
    crate::activation::record(store, &ctx.noeuds, "engine-reponse");

    let modele_servi = v
        .get("model")
        .and_then(|x| x.as_str())
        .unwrap_or("(inconnu)")
        .to_string();
    // Ce que le service dit avoir produit : la seule mesure qui permette de
    // vérifier qu'un `ms_llm` long vient du nombre de jetons et non d'autre chose.
    let jetons_sortie = v
        .get("usage")
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;

    Ok(ReponseEngine {
        question: question.to_string(),
        reponse,
        modele: modele_servi,
        sujet: sujet.to_string(),
        depuis_raisonnement,
        noeuds_contexte: ctx.noeuds.len(),
        relations_contexte: ctx.relations.len(),
        ms_moteur,
        ms_llm,
        max_tokens_demande: max_tokens,
        max_tokens_effectif: plafond,
        jetons_sortie,
        plafond_atteint: jetons_sortie >= plafond,
        raisonnement_demande: pensee,
        moteur_decompose: ctx.decompose.json(),
        contexte: ctx.texte,
    })
}

/// Le service LLM répond-il ? Sert au diagnostic du dashboard (et à dire
/// POURQUOI l'engine est indisponible plutôt que d'échouer sans explication).
pub fn sante() -> serde_json::Value {
    let url = url_llm();
    match crate::http_min::get(&format!("{url}/v1/models"), Duration::from_secs(5)) {
        Ok(r) => {
            let modele = serde_json::from_str::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v.get("models")?.get(0)?.get("name")?.as_str().map(String::from))
                .unwrap_or_else(|| "(non annoncé)".into());
            serde_json::json!({ "disponible": r.status == 200, "url": url, "modele": modele })
        }
        Err(e) => serde_json::json!({
            "disponible": false, "url": url,
            "raison": format!("{e}"),
        }),
    }
}

/// ── CONVERSATION EN FLUX ────────────────────────────────────────────────────
///
/// Ce que la vue attend, et que `demander` ne pouvait pas donner :
///   - le *think* et le texte ARRIVENT au lieu d'apparaître d'un bloc ;
///   - les appels obrain (le contexte réellement lu) sont annoncés AVANT la
///     réponse, donc visibles en temps réel ;
///   - la conversation est MULTI-TOURS (historique transmis) ;
///   - sans sujet visé, l'ancrage est le **GRAPHE GLOBAL** : la signature du
///     store (types, relations, échantillon), pas un voisinage choisi.
pub enum Evenement {
    /// Ce qu'obrain a réellement été interrogé — émis AVANT toute réponse.
    ///
    /// Porte désormais **le store réellement ouvert** et **la part lue** : le
    /// défaut corrigé était une interface qui nommait un monde et en lisait un
    /// autre ; ce qui est ancré ne peut donc plus être implicite.
    Contexte {
        noeuds: usize,
        relations: usize,
        apercu: String,
        ms: u64,
        store: String,
        monde: Option<String>,
        total_noeuds: i64,
        total_aretes: i64,
        par_label: Vec<(String, i64, usize)>,
        /// Le Self à qui cette lecture appartient — affiché par la vue, parce
        /// qu'une mémoire sans propriétaire n'est pas une mémoire préservée.
        self_qn: String,
        /// Les nœuds les plus connectés (le « cœur » du graphe), pour que la vue
        /// les montre comme le modèle les voit.
        hubs: Vec<String>,
        /// Temps du calcul des hubs, tel que l'entretien l'a mesuré.
        hubs_ms: u64,
        /// La fenêtre de jetons RÉELLEMENT servie par le modèle, et d'où elle
        /// vient. Affichée parce qu'elle commande tout le reste (taille du
        /// contexte, des résultats d'outils, nombre de tours) : un plafond
        /// invisible se lit comme une limite du logiciel, pas du service.
        fenetre: u64,
        fenetre_source: String,
        /// Où le temps est réellement passé (objet JSON `{totaux, resolution,
        /// signature, aretes, echantillons, voisinage}`).
        decompose: serde_json::Value,
    },
    /// Un appel d'outil RÉELLEMENT exécuté contre la base.
    ///
    /// Poussé au client pour que le travail soit visible : c'est ce qui rend la
    /// densification observables — on voit la requête, le nombre de lignes, la
    /// durée, et l'erreur s'il y en a une.
    Outil {
        nom: String,
        /// Arguments tels que le modèle les a produits (bornés à l'affichage).
        arguments: String,
        lignes: usize,
        ms: u64,
        /// Le monde réellement interrogé.
        monde: String,
        /// Le contenu a été COUPÉ au plafond de caractères.
        ///
        /// Défaut mesuré (2026-09-11) : le plafond était annoncé AU MODÈLE
        /// (« COUPÉ … 17 lignes sur 29 »), mais pas à l'écran, qui affichait
        /// « 29 lignes ». Le nombre transmis est désormais celui des lignes
        /// LIVRÉES, et ce drapeau dit que le compte est un plafond, pas le tout.
        tronque: bool,
        erreur: Option<String>,
    },
    /// La conversation a STRUCTURÉ quelque chose de durable.
    ///
    /// On l'annonce séparément des autres appels d'outils parce que ce n'est pas
    /// une lecture : c'est une écriture, journalisée pour la base du Self. La
    /// taire laisserait croire que rien n'a été retenu — ou, pire, que c'est déjà
    /// dans la base alors que la passe d'application reste à faire.
    Memoire {
        /// `note`, `objectif`, `procedure` ou `etat-self`.
        genre: String,
        /// Ce qui a été retenu (nom, ou début du texte).
        reference: String,
        /// Le Self propriétaire.
        self_qn: String,
    },
    /// Le raisonnement du modèle (flux `reasoning_content`).
    Think(String),
    /// La réponse (flux `content`).
    Texte(String),
    /// Fin de génération. `raison` vient du modèle lui-même : `"stop"` = fin
    /// naturelle, `"length"` = PLAFOND DE TOKENS ATTEINT. Sans cette
    /// information, une réponse coupée était présentée comme une réponse
    /// complète — c'est exactement le défaut constaté (« les réponses se
    /// coupent », sans que rien ne le dise).
    Fin { ms_llm: u64, raison: Option<String> },
}

/// LA QUESTION PORTE-T-ELLE SUR CE QUE LE CONTEXTE N'A PAS MONTRÉ ?
///
/// Défaut mesuré le 2026-09-11, et c'est le plus grave de la journée. Avec
/// `tool_choice: "auto"`, à la question « quelles recettes d'interrogation
/// existent, et que fait celle du tissage ? », le modèle n'a appelé AUCUN outil
/// et a **inventé** : les recettes seraient « encapsulées » dans `build.main` et
/// `InvoiceDescriptor`, et le tissage « structurerait les nœuds et créerait des
/// ponts ». Le contexte ne contenait ni « recette », ni « tissage », ni « weave ».
///
/// Un modèle qui ne sait pas doit DIRE qu'il ne sait pas — c'est ce que demande
/// la consigne — mais un 9 B ne s'y tient pas de lui-même. On ne compte donc pas
/// sur sa discipline : on la lui **impose** quand la question sort du contexte.
///
/// Le test est volontairement grossier : un mot significatif de la question
/// (≥ 4 caractères, hors mots vides) absent du contexte ⇒ un outil est EXIGÉ. Il
/// peut forcer un tour inutile sur une question qui se répond seule ; c'est le
/// prix assumé, et il est bien moindre que celui d'une réponse inventée.
fn exige_un_outil(question: &str, contexte: &str) -> Option<String> {
    const VIDES: &[&str] = &[
        "avec", "cette", "celle", "celui", "dans", "donne", "entre", "elle", "elles", "encore",
        "faire", "faut", "leur", "leurs", "liste", "meme", "montre", "nous", "plus", "pour",
        "pourquoi", "projet", "quand", "quel", "quelle", "quelles", "quels", "sans", "selon",
        "sont", "sous", "tout", "tous", "toute", "toutes", "tres", "utilise", "voici", "vous",
        "combien", "comment", "depuis", "alors", "aussi", "apres", "avant",
        // Verbes et adverbes à haute fréquence : « Que fait ceci ? » ne doit pas
        // exiger un outil. Trouvé par le test `mots_vides_ignores`.
        "fait", "faites", "peut", "peuvent", "doit", "doivent", "etre", "avoir", "sais",
        "quoi", "quelque", "chose", "dire", "explique", "decris", "resume", "parle",
        // Démonstratifs, pronoms et adverbes — la classe est fermée, on peut
        // l'épuiser : ce sont eux qui faisaient exiger un outil pour rien.
        "ceci", "cela", "ceux", "celles", "ici", "voila", "ainsi", "donc", "toujours",
        "jamais", "souvent", "surtout", "notamment", "egalement", "encore", "enfin",
        "plutot", "surtout", "aussitot", "ensuite", "puis", "enfin", "enfin",
        "est", "sont", "etait", "sera", "serait", "etait-ce", "sert", "servent",
    ];
    let bas = contexte.to_lowercase();
    let mut vus: Vec<String> = Vec::new();
    for brut in question.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.') {
        let m = brut.trim_matches('.').to_lowercase();
        if m.chars().count() < 4 || VIDES.contains(&m.as_str()) || vus.contains(&m) {
            continue;
        }
        vus.push(m.clone());
        if !bas.contains(&m) {
            return Some(m);
        }
    }
    None
}


/// Les ouvertures d'appel reconnues dans un TEXTE (pas dans le canal
/// `tool_calls`) : `<tool_call>` du gabarit Qwen, et `<function=…>` qu'il contient.
const MARQUEURS_APPEL: [&str; 2] = ["<tool_call", "<function="];

/// Les fermetures ORPHELINES : le modèle écrit `</function></tool_call>` alors que
/// le bloc a été capturé par une autre ouverture. Elles ne s'affichent jamais.
const FERMETURES_ORPHELINES: [&str; 3] = ["</tool_call>", "</function>", "</parameter>"];

/// Plafond de capture d'un bloc d'appel : un flux qui ouvre sans jamais fermer ne
/// doit pas grossir sans fin (un appel réel pèse quelques centaines d'octets).
const MAX_CAPTURE: usize = 64 * 1024;

/// ── UN `tool_call` ÉCRIT DANS LE TEXTE N'EST PAS UNE RÉPONSE ────────────────
///
/// Défaut mesuré le 2026-09-13 sur le chat du port 8432 (Qwen3.8-27B servi par le
/// DGX) : après huit appels d'outils exécutés, le tour suivant a produit ZÉRO
/// appel dans le canal `tool_calls` et, à la place, le markup du gabarit écrit
/// dans son RAISONNEMENT :
///
/// ```text
/// <tool_call> <function=obrain_query> <parameter=query> MATCH (n) WHERE
/// n.qualified_name STARTS WITH '…' RETURN … </parameter> </function> </tool_call>
/// ```
///
/// Le moteur n'y voyait que du texte : le markup partait à l'écran comme
/// « raisonnement », aucun outil n'était appelé, et la fin annoncée était `stop` —
/// une réponse présentée comme complète alors qu'elle n'était qu'un appel d'outil
/// déguisé. L'utilisateur l'a refusé : « il ne doit pas répondre un tool_call,
/// c'est un défaut ».
///
/// Cet intercepteur sépare donc les deux, morceau par morceau (le markup arrive
/// découpé par le flux, y compris au milieu d'une balise) :
///   - la PROSE est rendue à l'écran, immédiatement ;
///   - le MARKUP est mis de côté, parsé par `outils::appels_du_texte`, et rendu
///     EXÉCUTABLE comme s'il était arrivé par le canal normal.
///
/// Il retient en permanence la queue qui pourrait être le début d'une balise
/// coupée entre deux morceaux : c'est ce qui évite d'afficher `<tool` puis
/// `_call>` à l'écran les fois où le réseau coupe au mauvais endroit.
#[derive(Default)]
pub(crate) struct ExtracteurAppels {
    /// Le texte reçu et pas encore jugé.
    tampon: String,
    /// La fermeture attendue — `Some` quand on est DANS un bloc d'appel.
    fermeture: Option<&'static str>,
    /// Ce qui a été capturé depuis l'ouverture du bloc courant.
    capture: String,
    /// Les appels RÉCUPÉRÉS du texte, donc exécutables comme les autres.
    pub(crate) appels: Vec<crate::outils::Appel>,
    /// Le markup a-t-il été retiré de ce qui s'affiche ?
    pub(crate) intercepte: bool,
    /// Un bloc est-il resté OUVERT (le flux s'est arrêté en plein appel) ?
    pub(crate) inacheve: bool,
    /// Un bloc a-t-il été REJETÉ (trop gros pour être un appel) ?
    pub(crate) rejete: bool,
}

impl ExtracteurAppels {
    /// Pousse un morceau du flux et rend la PROSE à afficher — jamais de markup.
    pub(crate) fn pousser(&mut self, morceau: &str) -> String {
        if morceau.is_empty() {
            return String::new();
        }
        self.tampon.push_str(morceau);
        let mut prose = String::new();
        loop {
            // ── DANS UN BLOC : on cherche sa fermeture ──────────────────────
            if let Some(fermeture) = self.fermeture {
                match self.tampon.find(fermeture) {
                    Some(p) => {
                        let bloc: String = self.tampon.drain(..p + fermeture.len()).collect();
                        self.capture.push_str(&bloc);
                        self.appels.extend(crate::outils::appels_du_texte(&self.capture));
                        self.capture.clear();
                        self.fermeture = None;
                        self.intercepte = true;
                        continue;
                    }
                    None => {
                        if self.capture.len() + self.tampon.len() > MAX_CAPTURE {
                            // Trop gros pour un appel : on jette le bloc entier
                            // plutôt que de le laisser s'afficher ou grandir.
                            self.capture.clear();
                            self.tampon.clear();
                            self.fermeture = None;
                            self.rejete = true;
                        } else {
                            // On garde tout : la fermeture peut venir au morceau suivant.
                            let reste: String = self.tampon.drain(..).collect();
                            self.capture.push_str(&reste);
                        }
                        break;
                    }
                }
            }
            // ── HORS BLOC : une fermeture orpheline ne s'affiche pas ────────
            for f in FERMETURES_ORPHELINES {
                while self.tampon.starts_with(f) {
                    self.tampon.drain(..f.len());
                    self.intercepte = true;
                }
            }
            match self.marqueur() {
                Some((p, ouverture)) => {
                    let avant: String = self.tampon.drain(..p).collect();
                    prose.push_str(&avant);
                    self.capture = self.tampon.drain(..).collect();
                    self.fermeture = Some(if ouverture == "<tool_call" {
                        "</tool_call>"
                    } else {
                        "</function>"
                    });
                    self.intercepte = true;
                    continue;
                }
                None => {
                    // Aucun marqueur complet : ce qui n'est pas le DÉBUT possible
                    // d'une balise part tout de suite à l'écran.
                    let garde = queue_a_garder(&self.tampon);
                    let n = self.tampon.len().saturating_sub(garde);
                    prose.push_str(&self.tampon[..n]);
                    let queue = self.tampon.split_off(n);
                    self.tampon = queue;
                    break;
                }
            }
        }
        prose
    }

    /// Le marqueur d'ouverture le plus PRÉCOCE, complet dans le tampon.
    fn marqueur(&self) -> Option<(usize, &'static str)> {
        MARQUEURS_APPEL
            .iter()
            .filter_map(|m| self.tampon.find(m).map(|p| (p, *m)))
            .min_by_key(|(p, _)| *p)
    }

    /// Vide ce qui reste à la FIN du tour : la prose retenue est rendue, le markup
    /// tronqué ne l'est jamais.
    pub(crate) fn cloturer(&mut self) -> String {
        // ── BLOC OUVERT ET JAMAIS FERMÉ : un appel interrompu ───────────────
        if self.fermeture.take().is_some() {
            self.capture.push_str(&self.tampon);
            self.tampon.clear();
            self.appels.extend(crate::outils::appels_du_texte(&self.capture));
            self.capture.clear();
            self.intercepte = true;
            self.inacheve = true;
            return String::new();
        }
        let reste = std::mem::take(&mut self.tampon);
        if reste.is_empty() {
            return String::new();
        }
        // Un appel COUPÉ garde son intérêt : le parser en tire ce qu'il peut.
        let recuperes = crate::outils::appels_du_texte(&reste);
        if !recuperes.is_empty() {
            self.appels.extend(recuperes);
            self.intercepte = true;
            self.inacheve = true;
            return String::new();
        }
        // Sinon c'est un DÉBUT de balise tronqué (`<tool` à la fin du flux) : il ne
        // s'affiche pas — afficher un fragment de balise est exactement le défaut
        // qu'on corrige. Le reste, s'il y en a, est de la prose.
        if queue_a_garder(&reste) >= reste.len() {
            self.intercepte = true;
            return String::new();
        }
        reste
    }
}

/// La plus longue QUEUE à retenir : un suffixe qui pourrait être le DÉBUT d'une
/// balise coupée entre deux morceaux du flux (`<tool` puis `_call>`).
fn queue_a_garder(texte: &str) -> usize {
    let mut garde = 0usize;
    for jeton in MARQUEURS_APPEL.iter().chain(FERMETURES_ORPHELINES.iter()) {
        let max = jeton.len().min(texte.len());
        for n in 1..=max {
            // `jeton` est ASCII : découper à `n` octets ne coupe aucun caractère.
            if texte.ends_with(&jeton[..n]) {
                garde = garde.max(n);
            }
        }
    }
    garde
}

/// ── LA REPRISE D'UNE RÉPONSE COUPÉE ─────────────────────────────────────────
///
/// Défaut mesuré le 2026-09-13 (chat du port 8432) : le flux SSE meurt au milieu
/// d'une réponse — le service `viz serve` est redémarré par un déploiement
/// (`status=9/KILL` au journal systemd, 1 248 redémarrages comptés) — et la vue
/// n'avait plus qu'à écrire « flux interrompu avant la fin — ce qui précède est
/// incomplet ». Une panne de TRANSPORT devenait donc une réponse inutilisable.
///
/// Ce n'était pas une fatalité : le texte déjà produit est CONNU du client, et un
/// modèle sait continuer une réponse qu'on lui remontre. On rejoue donc le même
/// tour avec la réponse partielle et cette consigne, et la vue ajoute la suite au
/// même tour. La coupure cesse d'être une fin.
pub(crate) const CONSIGNE_REPRISE: &str = "Ta réponse précédente (ci-dessus) a été COUPÉE par \
    une panne de transport — le flux s'est interrompu, pas toi. Reprends EXACTEMENT au caractère \
    qui suit la fin du texte ci-dessus : n'écris ni préambule, ni excuse, ni rappel, ni résumé, \
    ne répète pas ce qui est déjà écrit. Écris directement la suite, puis termine la réponse.";

/// Les messages d'un tour : historique, question — et, en REPRISE, la réponse
/// déjà produite suivie de la consigne de continuation.
///
/// Extrait de `demander_stream` pour être VÉRIFIABLE : la place de la réponse
/// partielle et de la consigne dans la conversation commande tout le comportement
/// (« le modèle recommence » ou « le modèle continue »), et cela ne se juge pas
/// à l'œil dans une fonction de trois cents lignes.
pub(crate) fn messages_du_tour(
    historique: &[(String, String)],
    question: &str,
    reprise: Option<&str>,
) -> Vec<serde_json::Value> {
    let mut messages: Vec<serde_json::Value> = Vec::new();
    // Multi-tours : l'historique est transmis tel quel, dans l'ordre.
    for (role, contenu) in historique {
        messages.push(serde_json::json!({ "role": role, "content": contenu }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": question }));
    if let Some(deja) = reprise.filter(|d| !d.trim().is_empty()) {
        // L'assistant a DÉJÀ dit cela : le modèle le relit comme sa propre
        // réponse, ce qui est la seule façon qu'il en reprenne le fil.
        messages.push(serde_json::json!({ "role": "assistant", "content": deja }));
        messages.push(serde_json::json!({ "role": "user", "content": CONSIGNE_REPRISE }));
    }
    messages
}

/// `atlas` = la carte des mondes et de leurs synapses (lue par le serveur dans la
/// base synaptique) : sans elle, le modèle ne peut ni nommer un autre graphe ni
/// atteindre le référentiel de l'agent, qui ne vit PAS dans le store regardé.
/// `ouvrir_monde` = le moyen d'ouvrir un autre store (le serveur en possède le
/// cache de handles) ; il rend `None` si le monde est illisible.
///
/// `reprise` = **le texte DÉJÀ produit**, quand la vue reprend une réponse que le
/// transport a coupée (voir `CONSIGNE_REPRISE`, et la reprise automatique côté
/// vue). Le tour rejoué n'offre AUCUN outil : il continue une prose, il n'ouvre
/// pas une exploration — et il ne peut donc pas rejouer une écriture mémorisée.
pub fn demander_stream(
    db: &ObrainDB,
    store: &Path,
    ancrage: &Ancrage,
    atlas: &crate::ancrage::Atlas,
    ouvrir_monde: &dyn Fn(&str) -> Option<std::sync::Arc<ObrainDB>>,
    db_synapse: Option<&ObrainDB>,
    sujet: &str,
    question: &str,
    historique: &[(String, String)],
    reprise: Option<&str>,
    max_tokens: u32,
    // La pensée : `pensee` la coupe, `effort` en règle le niveau. Le chemin du
    // chat n'envoyait AUCUN des deux avant le 2026-09-15, donc le template
    // appliquait son défaut `xhigh` — 37 s mesurés pour une question de comptage.
    pensee: bool,
    effort: &str,
    mut on_ev: impl FnMut(Evenement),
    // ── DEUX ARRÊTS QUI MANQUAIENT (2026-09-16) ─────────────────────────────
    // 1. `client_parti` : le transport le pose quand une écriture SSE échoue. Un
    //    navigateur qui ferme l'onglet faisait continuer la génération jusqu'au
    //    bout — mesuré : 600 s de délai PAR appel de modèle, et autant de fois
    //    qu'il y a de tours. On cesse donc de générer, et on le DIT.
    // 2. Le BUDGET PAR QUESTION : la seule borne était le plafond de longueur et
    //    le délai d'un appel. Une question n'a pas de durée maximale : on lui en
    //    donne une (`BUDGET_QUESTION_S`), pour qu'une génération qui traîne
    //    s'arrête avec une raison au lieu de tenir la connexion.
    client_parti: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<()> {
    let t0 = Instant::now();
    // ── LA GARDE DE CETTE QUESTION ──────────────────────────────────────────
    // Échéance, drapeau du client parti, prose accumulée : les trois vivent
    // maintenant dans `obrain-garde`, la MÊME pièce que traversent les boucles
    // de `selfd`. Elles étaient recopiées ici — et absentes là-bas, ce qui est
    // exactement le défaut que ce module partagé corrige.
    let mut garde = obrain_garde::Garde::avec_client(
        obrain_garde::Reglages {
            budget: Duration::from_secs(BUDGET_QUESTION_S),
            max_appels_par_tour: crate::outils::MAX_APPELS_PAR_TOUR,
        },
        client_parti,
    );
    // Ancrage. Un sujet visé donne son voisinage ; sinon, et si le sujet ne
    // résout pas, on part de la SIGNATURE DU STORE — le graphe global.
    let ctx = if sujet.trim().is_empty() {
        contexte_du_monde(db, "le graphe global", ancrage, false)?
    } else {
        match contexte_du_sujet(db, sujet, ancrage) {
            Ok(c) if !c.noeuds.is_empty() => c,
            _ => contexte_du_monde(db, sujet, ancrage, true)?,
        }
    };
    let ms_moteur = t0.elapsed().as_millis() as u64;

    // Le graphe s'allume : ce que l'engine lit est inscrit au journal, que la
    // vue relit en direct. C'est le même journal qui allume les mondes.
    crate::activation::record(store, &ctx.noeuds, "engine");
    if !ctx.qn.is_empty() {
        crate::activation::record(store, &[ctx.qn.clone()], "engine-sujet");
    }
    on_ev(Evenement::Contexte {
        noeuds: ctx.noeuds.len(),
        relations: ctx.relations.len(),
        apercu: ctx.qn.clone(),
        ms: ms_moteur,
        store: ctx.store.clone(),
        monde: ctx.monde.clone(),
        total_noeuds: ctx.total_noeuds,
        total_aretes: ctx.total_aretes,
        par_label: ctx.par_label.clone(),
        hubs: ctx.hubs.clone(),
        hubs_ms: ctx.hubs_ms,
        self_qn: ctx.self_qn.clone(),
        fenetre: fenetre_jetons() as u64,
        fenetre_source: source_fenetre(),
        decompose: ctx.decompose.json(),
    });

    let systeme = SYSTEME_AVEC_OUTILS;

    // ── LA FENÊTRE EST UN BUDGET PARTAGÉ : LA SORTIE SE RÉSERVE ─────────────
    // Mesuré le 2026-09-11, base « Runtime dev (engrams) » : contexte de
    // 12 000 caractères (~3 750 jetons) + 4 096 jetons de réponse demandés, dans
    // une fenêtre de 8 192 — la génération franchissait la fenêtre, le service
    // refusait la requête, et le chat annonçait le contexte puis restait MUET.
    // On réserve donc la place de la réponse AVANT d'envoyer.
    let fixe = jetons_estimes(systeme)
        + jetons_estimes(question)
        + reprise
            .map(|deja| jetons_estimes(deja) + jetons_estimes(CONSIGNE_REPRISE))
            .unwrap_or(0)
        + historique
            .iter()
            .map(|(role, contenu)| jetons_estimes(role) + jetons_estimes(contenu))
            .sum::<usize>();
    let sortie_demandee = max_tokens.max(SORTIE_MINIMALE) as usize;
    let pour_contexte = fenetre_jetons()
        .saturating_sub(fixe)
        .saturating_sub(sortie_demandee)
        .saturating_sub(MARGE_JETONS)
        .max(256);
    let (texte_contexte, tronque) = contexte_ajuste(&ctx.texte, pour_contexte);
    if tronque {
        eprintln!(
            "engine : contexte tronqué (fenêtre {} jetons · invite+historique {fixe} · \
sortie réservée {sortie_demandee} · contexte gardé {pour_contexte})",
            fenetre_jetons()
        );
    }
    // Ce qui reste après l'invite, le contexte et la marge plafonne la réponse :
    // demander plus que la fenêtre ne contient fait refuser la requête entière.
    let restant =
        fenetre_jetons().saturating_sub(fixe + jetons_estimes(&texte_contexte) + MARGE_JETONS);
    // Même séparation que dans `demander` : la place RÉSERVÉE garde son plancher,
    // le plafond suit l'appelant — au plancher de tour près, que la boucle exige
    // pour ne pas confondre « l'appelant veut peu » et « la fenêtre est saturée ».
    let max_tokens = plafond_generation(max_tokens, restant).max(SORTIE_TOUR_MINIMALE);

    let mut messages: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "system", "content": format!("{systeme}\n\nCONTEXTE DU GRAPHE\n{texte_contexte}")
    })];
    messages.extend(messages_du_tour(historique, question, reprise));

    // LE COÛT DES OUTILS EST RÉSERVÉ, ET IL REPART À CHAQUE TOUR.
    // Il a été oublié d'abord — conséquence mesurée : la première question qui a
    // réellement appelé un outil a vu son tour suivant refusé par le service
    // (« Context size has been exceeded »).
    let cout_outils = jetons_estimes(&crate::outils::definitions().to_string());
    // ── LA BOUCLE : LE MODÈLE PEUT DEMANDER LA SUITE ────────────────────────
    // Le contexte initial n'est qu'un point de départ (la carte du store). S'il
    // ne suffit pas, le modèle appelle un outil, on l'exécute CONTRE LA BASE, et
    // on lui rend le résultat — jusqu'à `outils::max_tours()`.
    //
    // Le budget est partagé avec la fenêtre : chaque tour ajoute son résultat et
    // sa requête aux messages. Avant chaque nouveau tour on estime le total et
    // on s'arrête si la fenêtre ne le porte plus — plutôt que de faire refuser
    // la requête entière par le service.
    let url = format!("{}/v1/chat/completions", url_llm());
    let debut = Instant::now();
    // La question sort-elle de ce que le contexte montre ? Si oui, un outil sera
    // exigé au premier tour, et on l'annonce — une contrainte invisible serait
    // une boîte noire de plus. En REPRISE, aucun outil n'est offert : la question
    // est de continuer une prose, pas d'ouvrir une lecture — et l'exiger ferait
    // écrire un appel d'outil là où la réponse doit finir.
    let hors_contexte = if reprise.is_some() { None } else { exige_un_outil(question, &texte_contexte) };
    if let Some(mot) = &hors_contexte {
        on_ev(Evenement::Think(format!(
            "[« {mot} » n'apparaît pas dans le contexte lu : l'appel d'un outil est exigé              plutôt que de laisser le modèle combler le vide]\n"
        )));
    }
    let mut raison: Option<String> = None;
    /// Durée cumulée des SEULS appels au modèle (`ms_llm` de l'événement `Fin`).
    let mut ms_llm_reel: u64 = 0;
    /// Un tour SUPPLÉMENTAIRE a-t-il été accordé au tour précédent, parce qu'un
    /// appel d'outil avait été récupéré d'un texte (voir `ExtracteurAppels`) ?
    let mut tour_sup_du_texte = false;
    let mut jetons_conversation = jetons_estimes(&messages.iter().map(|m| m.to_string()).collect::<String>());

    /**
     * Le budget d'une question, en secondes. Il borne la SOMME des appels au
     * modèle — pas un appel (le délai de lecture, lui, vaut 600 s). Au-delà, on
     * rend ce qui a été écrit et on dit pourquoi : c'est la différence entre une
     * réponse tronquée EXPLIQUÉE et un flux qui pend.
     */
    const BUDGET_QUESTION_S: u64 = 300;
    for tour in 0..=crate::outils::max_tours() + 1 {
        // ── LE CLIENT EST PARTI, OU LA QUESTION A TROP DURÉ : ON S'ARRÊTE ──
        // Placé EN TÊTE de tour : c'est le seul moment où l'on peut encore
        // épargner un appel au modèle entier. Le texte déjà écrit est parti sur
        // la socket ; s'il n'y a plus de socket, il n'y a plus rien à produire.
        if let Err(motif) = garde.tour() {
            match motif {
                // ── UN ARRÊT SE DIT (dans le journal du service) ────────────
                // Sans cette ligne, l'arrêt serait invisible : le client est
                // parti, donc aucune trame ne peut le lui apprendre, et le
                // journal du serveur est le seul endroit où l'on peut constater
                // que l'engine a CESSÉ de générer au lieu de payer un tour de
                // plus. Niveau WARN à dessein : le service journalise en WARN
                // par défaut, et un arrêt qui ne s'écrit pas est un arrêt qu'on
                // ne peut pas constater. (Mesuré : en `info!`, la ligne
                // n'apparaissait pas.)
                obrain_garde::Motif::ClientParti => {
                    tracing::warn!(
                        "engine : client parti — génération arrêtée au tour {tour} ({:.1} s de modèle cumulées)",
                        ms_llm_reel as f64 / 1000.0
                    );
                    raison = Some(motif.code().to_string());
                }
                obrain_garde::Motif::Budget(s) => {
                    raison = Some(format!("budget_question_{s}s"));
                    on_ev(Evenement::Texte(format!(
                        "\n\n[réponse interrompue : la question a dépassé son budget de {s} s — \
                         relancez-la, ou resserrez-la ; ce qui précède est ce qui a été produit]\n"
                    )));
                }
                // La répétition se pose PENDANT la lecture d'un tour, pas au
                // début du suivant : ce bras ne devrait pas être atteint. S'il
                // l'est, on ne le tait pas — un motif d'arrêt qui disparaîtrait
                // serait pire que le défaut qu'il annonce.
                obrain_garde::Motif::Repetition(_) => {
                    raison = Some(motif.code().to_string());
                }
            }
            break;
        }
        // ── LE DERNIER TOUR N'EST PAS LE DERNIER MOT QUAND UN APPEL EST EN JEU ─
        // `dernier` veut dire « plus d'outils offerts ». Un appel d'outil ÉCRIT
        // DANS LE TEXTE (voir `ExtracteurAppels`) est un appel que le modèle a
        // réellement voulu faire : le tour suivant lui est donc accordé pour
        // conclure avec le résultat en main, au lieu de rendre ce texte comme une
        // réponse — ce que l'utilisateur a explicitement refusé. Un seul tour est
        // ajouté ainsi : la borne de boucle est `max_tours() + 1`.
        let mut dernier = tour >= crate::outils::max_tours() + usize::from(tour_sup_du_texte);
        tour_sup_du_texte = false;
        // ── LA FENÊTRE EST VÉRIFIÉE AVANT D'ENVOYER, PAS APRÈS AVOIR ÉCHOUÉ ──
        // Ce qui compte, c'est ce qui part : conversation accumulée + définitions
        // d'outils + sortie demandée + marge. Si ça ne tient plus, on retire les
        // outils (ils coûtent cher) et on rogne la sortie — plutôt que de laisser
        // le service refuser la requête entière et perdre la réponse en cours.
        let dispo = fenetre_jetons()
            .saturating_sub(jetons_conversation)
            .saturating_sub(cout_outils)
            .saturating_sub(MARGE_JETONS);
        let mut sortie_tour = max_tokens as usize;
        if dispo < sortie_tour + 64 {
            // ── ON RÉDUIT LA SORTIE **AVANT** DE RETIRER LES OUTILS ──────────
            // Défaut mesuré le 2026-09-11, et il était DÉTERMINISTE : le client
            // du chat ne demande pas de `max_tokens`, donc le défaut (4 096) est
            // pris, la fenêtre ne le porte pas, et la version précédente en
            // concluait aussitôt « dernier tour » — donc **sans outils**.
            // Conséquence : « retiens ceci dans ta mémoire » n'appelait aucun
            // outil et le modèle répondait à côté. Un test en ligne de commande
            // avec `max_tokens=700` passait, le navigateur échouait : même code,
            // deux tailles de sortie demandées.
            //
            // La bonne arbitrage est l'inverse : la sortie est un CONFORT, les
            // outils sont une CAPACITÉ. On rogne d'abord la première ; on ne
            // retire les seconds que si même le minimum ne tient plus.
            if dispo >= SORTIE_MINIMALE as usize + 64 {
                sortie_tour = dispo - 64;
            } else {
                // Vraiment à l'étroit : on rend la place des définitions à la
                // réponse, et le modèle conclut avec ce qu'il a.
                dernier = true;
                let dispo_sans = fenetre_jetons()
                    .saturating_sub(jetons_conversation)
                    .saturating_sub(MARGE_JETONS);
                sortie_tour = dispo_sans.saturating_sub(64);
            }
        }
        if sortie_tour < 64 {
            on_ev(Evenement::Think(
                "\n[fenêtre de contexte saturée : conclusion sans lecture supplémentaire]".into(),
            ));
            // ── LA RAISON NE DOIT PAS DATER DU TOUR PRÉCÉDENT ───────────────
            // `raison` est remis à `None` plus bas, APRÈS ce `break` : en
            // sortant ici, l'événement `Fin` annonçait donc la raison du tour
            // PRÉCÉDENT (« stop ») pour une génération qui n'avait pas eu lieu —
            // une fin présentée comme naturelle alors que c'est une saturation.
            raison = Some("fenetre_saturee".to_string());
            break;
        }
        let payload = serde_json::json!({
            "model": modele(),
            "messages": messages,
            "max_tokens": sortie_tour,
            "temperature": 0.2,
            "stream": true,
            // ── LE RAISONNEMENT SE DIT AU TEMPLATE, SUR CHAQUE TOUR ─────────
            // DÉFAUT MESURÉ (2026-09-15) : le chemin du chat n'envoyait RIEN, donc
            // le template appliquait son propre défaut — `xhigh`, dont le texte
            // est « think carefully through the task, validate key assumptions,
            // consider plausible alternatives ». 37 s de raisonnement pour une
            // question de comptage, et aucune consigne de brièveté du prompt
            // système ne pouvait l'emporter là-dessus. Le réglage vient de la
            // MÊME source que pour `ask` : `gabarit_de_pensee`.
            "chat_template_kwargs": crate::engine::gabarit_de_pensee(pensee, effort)
                .unwrap_or(serde_json::Value::Null),
            // Au dernier tour on RETIRE les outils : le modèle doit conclure avec
            // ce qu'il a, pas ouvrir une exploration de plus.
            "tools": if dernier { serde_json::Value::Null } else { crate::outils::definitions() },
            // Au PREMIER tour, si la question sort du contexte, l'outil est
            // EXIGÉ : c'est ce qui empêche une réponse inventée (voir
            // `exige_un_outil`, défaut mesuré). Ensuite, main libre.
            "tool_choice": if dernier {
                serde_json::Value::Null
            } else if tour == 0 && hors_contexte.is_some() {
                serde_json::json!("required")
            } else {
                serde_json::json!("auto")
            },
        })
        .to_string();

        let mut erreur: Option<String> = None;
        let mut appels: Vec<crate::outils::Appel> = Vec::new();
        raison = None;
        // Le service a-t-il dit lui-même que la génération était FINIE (`[DONE]`) ?
        // Sans ce témoin, un flux qui s'arrête est une coupure, pas une fin.
        let mut vu_done = false;
        // Les deux canaux du modèle passent par l'interception des appels d'outils
        // écrits en texte — le canal `content` comme le RAISONNEMENT, parce que
        // c'est dans le raisonnement que le défaut a été mesuré (voir
        // `ExtracteurAppels`).
        let mut filtre_texte = ExtracteurAppels::default();
        let mut filtre_think = ExtracteurAppels::default();
        // ── `ms_llm` MESURE LE MODÈLE, PAS LE TOUR ENTIER ───────────────────
        // `Fin` portait `debut.elapsed()`, où `debut` est posé AVANT la boucle :
        // le chiffre annoncé « modèle : X s » comprenait donc jusqu'à cinq appels
        // au service, l'exécution de tous les outils ET leurs requêtes en base.
        // Mesuré : `ms_llm: 152333` affiché « modèle : 152.3 s » alors que les
        // outils de ce tour ont coûté 0-2 ms — le chiffre était juste, son nom
        // était faux. On accumule la durée RÉELLE des appels au modèle.
        let t_llm = Instant::now();
        /**
         * ── LE COUPE-CIRCUIT DE RÉPÉTITION (défaut signalé le 2026-09-16) ─────
         *
         * « L'engine a tendance à faire une génération en boucle infinie. »
         * Vérifié : la boucle d'outils est bornée (4-6 tours), il n'y a AUCUN
         * réessai, et la seule borne de longueur est `plafond_generation` — un
         * modèle qui se répète écrit donc jusqu'au plafond, ce qui se lit comme
         * une génération infinie. Rien ne détectait la répétition.
         * On accumule ce qui est ÉMIS (raisonnement ET texte : c'est dans le
         * raisonnement que ce dépôt a déjà mesuré le défaut de markup) et on
         * ARRÊTE la lecture du flux dès qu'un motif dégénéré apparaît — le rappel
         * en a maintenant le pouvoir (`post_sse` rend « arrêté par le rappel »).
         */
        // La prose d'un tour ne déborde pas sur le suivant : deux tours qui se
        // ressemblent sont le lot d'une conversation, pas une boucle.
        garde.nouvelle_page();
        let veille = garde.veille();
        let arrete_par_le_rappel = crate::http_min::post_sse(&url, &payload, Duration::from_secs(600), |ligne| {
            // Le client a fermé, ou la question a dépassé son budget : inutile de
            // lire la suite du flux, et le service doit cesser d'écrire (le
            // rappel en a le pouvoir depuis `post_sse` → `Ok(true)`).
            if !veille.continuer() {
                return false;
            }
            let Some(donnees) = ligne.strip_prefix("data:") else { return true };
            let donnees = donnees.trim();
            if donnees == "[DONE]" {
                vu_done = true;
                return true;
            }
            if donnees.is_empty() {
                return true;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(donnees) else { return true };
            if let Some(e) = v.get("error") {
                erreur = Some(e.to_string());
                return true;   // on lit jusqu'au bout : le verdict est rendu après l'appel
            }
            if let Some(f) = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("finish_reason"))
                .and_then(|x| x.as_str())
            {
                raison = Some(f.to_string());
            }
            let Some(delta) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")) else {
                return true;   // trame sans delta (rôle, fin) : rien à émettre
            };
            // Les appels d'outils arrivent EN MORCEAUX : `index` identifie l'appel,
            // le nom arrive d'un coup, les arguments caractère par caractère. On
            // les recolle par index — sans quoi on exécuterait un JSON tronqué.
            if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                for tc in tcs {
                    let i = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                    // ── UN INDEX ABERRANT N'ALLOCUE PAS DES GO ──────────────
                    // `i` vient du flux distant : `while appels.len() <= i` avec
                    // un index à 1e9 allouait un milliard de structures (~70 Go)
                    // et faisait avorter le processus. Au-delà du plafond par
                    // tour, on ignore l'entrée plutôt que de mourir pour elle.
                    if i >= crate::outils::MAX_APPELS_PAR_TOUR {
                        continue;
                    }
                    while appels.len() <= i {
                        appels.push(crate::outils::Appel::default());
                    }
                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                        appels[i].id = id.to_string();
                    }
                    if let Some(f) = tc.get("function") {
                        if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                            appels[i].nom.push_str(n);
                        }
                        if let Some(a) = f.get("arguments").and_then(|x| x.as_str()) {
                            appels[i].arguments.push_str(a);
                        }
                    }
                }
            }
            // ── LE MARKUP D'APPEL NE SORT JAMAIS D'ICI ──────────────────────
            // Ce qui est de la PROSE part à l'écran (raisonnement ou texte) ; ce
            // qui est un APPEL est mis de côté et rendu exécutable. Un tour ne
            // peut donc plus « répondre un tool_call » : c'est le défaut mesuré
            // le 2026-09-13, et il se produisait dans le raisonnement.
            if let Some(t) = delta.get("reasoning_content").and_then(|x| x.as_str())
                && !t.is_empty()
            {
                let prose = filtre_think.pousser(t);
                if !prose.is_empty() {
                    // La garde juge AVANT l'affichage, mais la prose part à
                    // l'écran dans le même ordre qu'avant : ce qui a été émis
                    // doit rester visible, même quand c'est ce qui a conclu.
                    let boucle = garde.prose(&prose).is_err();
                    on_ev(Evenement::Think(prose));
                    if boucle {
                        return false;   // on cesse de lire : le modèle boucle
                    }
                }
            }
            if let Some(t) = delta.get("content").and_then(|x| x.as_str())
                && !t.is_empty()
            {
                let prose = filtre_texte.pousser(t);
                if !prose.is_empty() {
                    // La garde juge AVANT l'affichage, mais la prose part à
                    // l'écran dans le même ordre qu'avant : ce qui a été émis
                    // doit rester visible, même quand c'est ce qui a conclu.
                    let boucle = garde.prose(&prose).is_err();
                    on_ev(Evenement::Texte(prose));
                    if boucle {
                        return false;   // le modèle boucle : on cesse de lire
                    }
                }
            }
            true   // rien de dégénéré dans cette ligne : on continue
        })?;
        ms_llm_reel += t_llm.elapsed().as_millis() as u64;
        // ── L'ARRÊT DU RAPPEL SE DIT ICI, LÀ OÙ IL A EU LIEU ────────────────
        // MESURÉ : un client qui part fait échouer la prochaine écriture SSE ; le
        // drapeau est alors posé, et c'est le RAPPEL qui cesse de lire (il consulte
        // le drapeau à chaque ligne). Le journal posé en tête de tour ne pouvait
        // donc RIEN dire : la boucle s'arrêtait d'elle-même au premier tour, faute
        // de tours suivants. On journalise et on conclut ici.
        if arrete_par_le_rappel && garde.client_parti() {
            tracing::warn!(
                "engine : client parti — lecture du flux interrompue après {:.1} s de modèle (tour {tour})",
                ms_llm_reel as f64 / 1000.0
            );
            raison = Some("client_parti".to_string());
            break;
        }
        if let Some(e) = erreur {
            return Err(anyhow::anyhow!(
                "engine : le serveur de modèle a refusé la requête : {e}"
            ));
        }
        // ── CE QUI RESTE DANS LES FILTRES À LA FIN DU TOUR ──────────────────
        // Une queue retenue (préfixe de balise à cheval) doit être rendue si c'est
        // de la prose, et un bloc laissé OUVERT est un appel interrompu : il est
        // récupéré, jamais affiché.
        for (filtre, canal) in [(&mut filtre_think, true), (&mut filtre_texte, false)] {
            let reste = filtre.cloturer();
            if !reste.is_empty() {
                if canal {
                    on_ev(Evenement::Think(reste));
                } else {
                    on_ev(Evenement::Texte(reste));
                }
            }
        }
        let bloc_inacheve = filtre_think.inacheve || filtre_texte.inacheve;
        let bloc_rejete = filtre_think.rejete || filtre_texte.rejete;
        // Les appels RÉCUPÉRÉS du texte s'ajoutent aux appels du canal normal.
        let nb_appels_du_texte = filtre_texte.appels.len() + filtre_think.appels.len();
        if nb_appels_du_texte > 0 {
            // L'APPEL RÉCUPÉRÉ SE DIT : sans cette ligne, un appel d'outil qui
            // disparaît du raisonnement serait un tour de magie — et le lecteur
            // croirait à une réponse amputée.
            on_ev(Evenement::Think(format!(
                "\n[{nb_appels_du_texte} appel(s) d'outil écrit(s) dans le texte : récupéré(s) et \
                 exécuté(s) — un appel d'outil n'est pas une réponse, il n'est donc pas affiché]\n"
            )));
            let mut recuperes = filtre_think.appels;
            recuperes.extend(filtre_texte.appels);
            // Ils passent APRÈS ceux du canal normal, et sous le même plafond.
            for a in recuperes {
                if appels.len() >= crate::outils::MAX_APPELS_PAR_TOUR {
                    break;
                }
                // Un identifiant est exigé pour rattacher le résultat d'outil à son
                // appel ; celui du parseur est propre et neutre.
                appels.push(a);
            }
        } else if bloc_inacheve || bloc_rejete || filtre_think.intercepte || filtre_texte.intercepte {
            // Un markup a été RETIRÉ et n'a produit aucun appel exploitable : le
            // dire, plutôt que de laisser l'utilisateur devant un texte qui
            // s'arrête sans raison visible.
            let cause = if bloc_rejete {
                "hors de proportion"
            } else if bloc_inacheve {
                "tronqué par la coupure"
            } else {
                "illisible"
            };
            on_ev(Evenement::Think(format!(
                "\n[un appel d'outil {cause} a été écarté : il n'est pas affiché comme réponse]\n"
            )));
        }
        // ── LA BOUCLE DE RÉPÉTITION : ON L'ARRÊTE, ET ON LE DIT ────────────
        // Décidé ICI, après que tous les appels du tour ont été rassemblés et
        // avant qu'aucun ne soit exécuté : poursuivre un modèle qui se répète ne
        // produirait que de la répétition de plus. On rend ce qui a été écrit
        // (il est déjà parti à l'écran), on annonce POURQUOI la génération
        // s'arrête, et on n'exécute aucun outil. `vu_done` est posé parce que la
        // coupure est EXPLIQUÉE : le verdict générique de coupure ne doit pas
        // ajouter un second avertissement au même endroit.
        if let Some(obrain_garde::Motif::Repetition(r)) = garde.motif().cloned() {
            on_ev(Evenement::Texte(format!("\n\n[{}]\n", r.phrase())));
            appels.clear();
            vu_done = true;
        }
        // ── UN FLUX QUI S'ARRÊTE SANS DIRE QU'IL A FINI EST UNE COUPURE ─────
        // Défaut mesuré le 2026-09-13 : `http_min::post_sse` rend `Ok` quand la
        // connexion se ferme (`Ok(0)`) ou quand le délai de lecture expire — un
        // flux coupé en pleine génération était donc rendu comme une lecture
        // réussie, sans `finish_reason`, et la vue affichait le texte partiel
        // comme une réponse COMPLÈTE. On nomme la coupure, pour que la vue puisse
        // reprendre la réponse au lieu de la croire finie.
        if raison.is_none() && !vu_done {
            raison = Some("flux_coupe".to_string());
        }

        // ── UN APPEL RÉCUPÉRÉ AU TOUR DE CONCLUSION DEMANDE UN TOUR DE PLUS ──
        // `dernier` veut dire « aucun outil offert ». Un appel ÉCRIT DANS LE TEXTE
        // est un appel que le modèle a réellement voulu faire : on l'exécute, et on
        // accorde UN tour (la borne de boucle est `max_tours() + 1`) pour qu'il
        // conclue avec le résultat en main. Sans ce tour, le texte suppressé
        // laisserait la conversation sans réponse — et rendre ce texte serait
        // répondre un tool_call, ce que l'utilisateur a refusé.
        if appels.is_empty() {
            break;
        }
        if dernier {
            if nb_appels_du_texte == 0 {
                break;
            }
            tour_sup_du_texte = true;
        }

        // Le message de l'assistant PORTE ses appels : sans lui, la réponse
        // d'outil qui suit serait orpheline et le service la refuserait.
        let appels_json: Vec<serde_json::Value> = appels
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "type": "function",
                    "function": { "name": a.nom, "arguments": a.arguments }
                })
            })
            .collect();
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": appels_json
        }));

        // ── UN PLAFOND PAR TOUR, PAS SEULEMENT PAR CONVERSATION ────────────
        // Cette boucle exécutait TOUS les appels du tour, dont le nombre est
        // piloté par le modèle, et le contrôle de fenêtre n'intervenait qu'au
        // tour SUIVANT : un tour à vingt appels lançait vingt requêtes (jusqu'à
        // 14,1 s chacune, mesuré) avant le seul garde disponible.
        if appels.len() > crate::outils::MAX_APPELS_PAR_TOUR {
            on_ev(Evenement::Think(format!(
                "\n[{} appels d'outils demandés dans ce tour : les {} premiers sont \
                 exécutés, le reste est refusé — affine la demande]\n",
                appels.len(),
                crate::outils::MAX_APPELS_PAR_TOUR
            )));
        }
        for a in appels.iter().take(crate::outils::MAX_APPELS_PAR_TOUR) {
            let r = crate::outils::executer(db, &ouvrir_monde, db_synapse, ancrage, atlas, a);
            if let Some(m) = &r.memoire {
                on_ev(Evenement::Memoire {
                    genre: m.genre.to_string(),
                    reference: m.reference.clone(),
                    self_qn: ancrage.self_qn.clone(),
                });
            }
            on_ev(Evenement::Outil {
                nom: a.nom.clone(),
                arguments: a.arguments.chars().take(200).collect(),
                lignes: r.lignes,
                ms: r.ms,
                // Le monde vient du RÉSULTAT : c'est celui qui a été réellement
                // interrogé, pas celui qu'on croyait viser.
                monde: if r.monde.is_empty() {
                    ancrage.monde.clone().unwrap_or_else(|| "le monde ancré".into())
                } else {
                    r.monde.clone()
                },
                tronque: r.tronque,
                erreur: r.erreur.clone(),
            });
            jetons_conversation += jetons_estimes(&r.texte) + jetons_estimes(&a.arguments);
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": a.id,
                "content": r.texte
            }));
        }

        // ── CE GARDE A ÉTÉ RETIRÉ, ET VOICI POURQUOI ────────────────────────
        // Il coupait la boucle dès que la fenêtre devenait serrante — et comme
        // il s'exécutait APRÈS un tour d'outils, la conversation s'arrêtait sur
        // l'écriture, SANS réponse : constaté à la mesure (« retiens ceci »
        // produisait la note, puis rien). Le contrôle de budget en tête de tour
        // suffit et fait mieux : il retire les outils et rogne la sortie, et il
        // ne s'arrête que si même 64 jetons de réponse ne tiennent plus. Un
        // garde qui empêche de répondre n'est pas une prudence, c'est une panne.
    }
    // Le tour entier reste mesurable (`debut`), mais `ms_llm` dit ce que son nom
    // annonce : le temps passé DANS le modèle.
    let _ = &debut;
    on_ev(Evenement::Fin { ms_llm: ms_llm_reel, raison });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LE PLAFOND DE GÉNÉRATION SUIT L'APPELANT — c'est une mesure, pas un goût.
    ///
    /// Défaut mesuré le 2026-09-12 sur `/api/engine/ask?max_tokens=24` : le
    /// plafond réellement envoyé au service valait **512** (le plancher de
    /// RÉSERVATION de fenêtre servait aussi de plafond), donc **365 jetons
    /// produits et 50 128 ms**, là où les 24 jetons demandés coûtent **2 400 ms**.
    /// Le paramètre `max_tokens` de la route ne mentait pas qu'un peu : il coûtait
    /// vingt fois le prix annoncé.
    #[test]
    fn le_plafond_de_generation_suit_l_appelant() {
        // Demandé petit, fenêtre large : on envoie PETIT.
        assert_eq!(plafond_generation(24, 6_000), 24);
        assert_eq!(plafond_generation(400, 6_000), 400);
        assert_eq!(plafond_generation(700, 6_000), 700);
        // La fenêtre reste la borne dure : demander plus qu'elle ne porte ne doit
        // pas faire refuser la requête entière par le service.
        assert_eq!(plafond_generation(4_096, 300), 300);
        // Jamais zéro : un plafond nul ne demanderait aucune génération.
        assert_eq!(plafond_generation(0, 6_000), 1);
        assert_eq!(plafond_generation(24, 0), 1);
    }

    /// La propriété qui doit tenir pour TOUTE demande et TOUTE fenêtre : le
    /// plafond ne dépasse jamais ce qui a été demandé, et jamais ce que la
    /// fenêtre peut porter. C'est exactement ce que l'ancien code violait.
    #[test]
    fn le_plafond_ne_depasse_jamais_la_demande_ni_la_fenetre() {
        for demande in [0u32, 1, 24, 64, 400, 512, 700, 4_096, 100_000] {
            for restant in [0usize, 1, 63, 64, 513, 8_000, 32_768] {
                let p = plafond_generation(demande, restant);
                assert!(p >= 1, "plafond nul (demandé {demande}, restant {restant})");
                assert!(
                    p as usize <= (demande as usize).max(1),
                    "le plafond {p} dépasse la demande {demande} (restant {restant})"
                );
                assert!(
                    p as usize <= restant.max(1),
                    "le plafond {p} dépasse la fenêtre {restant}"
                );
            }
        }
    }

    /// La RÉSERVATION, elle, garde son plancher : c'est elle qui empêche le
    /// service de refuser la requête entière quand le contexte du graphe est
    /// gros. Séparer les deux notions ne doit pas la faire disparaître.
    #[test]
    fn la_reservation_de_fenetre_garde_son_plancher() {
        for demande in [0u32, 1, 24, 400, 512, 700, 4_096] {
            assert!(
                demande.max(SORTIE_MINIMALE) >= SORTIE_MINIMALE,
                "la place réservée est passée sous {SORTIE_MINIMALE} (demandé {demande})"
            );
        }
        // Le plancher de tour du flux doit rester SOUS la réservation, sinon la
        // boucle enverrait un plafond qu'elle croit déjà saturé.
        assert!(SORTIE_TOUR_MINIMALE <= SORTIE_MINIMALE);
    }

    /// LE LEVIER DE PENSÉE : rien quand on la veut (le gabarit décide), un
    /// réglage explicite quand on la coupe. Un réglage à moitié posé ferait
    /// dépendre le coût de l'appel d'un défaut invisible.
    #[test]
    fn le_gabarit_de_pensee_dit_TOUJOURS_ce_qu_il_veut() {
        // ── CE TEST A CHANGÉ DE SENS LE 2026-09-15, ET VOICI POURQUOI ────────
        // Il affirmait « pensée voulue : aucun réglage ne doit partir, le gabarit
        // décide ». Or le gabarit décidait `xhigh` — son défaut — c'est-à-dire
        // l'instruction la PLUS coûteuse, et le chat ne le savait pas : 37 s de
        // raisonnement pour une question de comptage, mesuré. Un réglage qui
        // décide tout seul n'est pas neutre, c'est un choix qu'on n'a pas fait.
        // On l'envoie donc, et on l'épingle.
        let g = gabarit_de_pensee(true, "medium").expect("la pensée doit être dite, pas héritée");
        assert_eq!(g.get("enable_thinking").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            g.get("reasoning_effort").and_then(|v| v.as_str()),
            Some("medium"),
            "l'effort demandé doit partir tel quel"
        );
        let coupe = gabarit_de_pensee(false, "medium").expect("pensée coupée : le réglage doit partir");
        assert_eq!(
            coupe.get("enable_thinking").and_then(|v| v.as_bool()),
            Some(false),
            "le réglage doit être `enable_thinking: false`, pas autre chose"
        );
        assert_eq!(
            coupe.get("reasoning_effort"),
            None,
            "pensée coupée : l'effort n'a plus d'objet et ne doit pas partir"
        );
    }

    /// LE NIVEAU D'EFFORT NE LAISSE RIEN PASSER.
    ///
    /// Le template **lève une exception** sur une valeur inconnue
    /// (`Unexpected reasoning effort … Supported types are xhigh (default),
    /// medium, and low`) : une faute de frappe dans un paramètre d'URL ferait
    /// donc échouer la requête entière au lieu de retomber proprement. On
    /// vérifie les traductions ET le repli.
    #[test]
    fn l_effort_de_pensee_retombe_sur_medium() {
        assert_eq!(effort_de_pensee(Some("courte")), "low");
        assert_eq!(effort_de_pensee(Some(" LOW ")), "low");
        assert_eq!(effort_de_pensee(Some("profonde")), "xhigh");
        assert_eq!(effort_de_pensee(Some("medium")), "medium");
        assert_eq!(effort_de_pensee(None), "medium", "défaut du chat");
        assert_eq!(effort_de_pensee(Some("n'importe quoi")), "medium");
        assert_eq!(effort_de_pensee(Some("")), "medium");
    }

    /// LE PROMPT DOIT DIRE LA VÉRITÉ SUR LES OUTILS QU'IL A.
    ///
    /// Défaut mesuré le 2026-09-12 : les deux routes partageaient un texte qui
    /// ordonnait « APPELLE UN OUTIL — `obrain_echantillon` … », alors que la
    /// route bloquante n'envoie AUCUN outil (le seul champ `tools` du fichier
    /// est dans `demander_stream`). Le modèle ne pouvait donc que promettre en
    /// prose un appel impossible, ou brûler son plafond à l'annoncer.
    #[test]
    fn le_prompt_dit_la_verite_sur_les_outils() {
        assert!(
            !SYSTEME_SANS_OUTILS.contains("APPELLE UN OUTIL"),
            "la route bloquante n'envoie aucun outil : son prompt ne doit pas en réclamer"
        );
        assert!(
            SYSTEME_SANS_OUTILS.contains("aucun outil n'est disponible"),
            "elle doit DIRE l'absence d'outil, pas la taire"
        );
        assert!(
            SYSTEME_AVEC_OUTILS.contains("APPELLE UN OUTIL"),
            "le flux envoie les outils : sa règle doit continuer de les déclencher"
        );
        // Le compte annoncé doit être celui des règles réellement énumérées
        // (« TROIS RÈGLES » pour deux items était faux).
        for (nom, p) in [("sans", SYSTEME_SANS_OUTILS), ("avec", SYSTEME_AVEC_OUTILS)] {
            let enumerees = p.matches("\n1.").count() + p.matches("\n2.").count();
            assert!(
                p.contains("DEUX RÈGLES") && enumerees == 2,
                "prompt « {nom} outils » : compte de règles faux ({enumerees} énumérées)"
            );
        }
    }

    /// LES BUDGETS SUIVENT LA FENÊTRE — et c'est vérifiable sans réseau.
    ///
    /// Défaut mesuré le 2026-09-11 : le moteur avait changé pour un modèle qui
    /// annonce 32 768 jetons, alors que tous ces budgets étaient calibrés pour
    /// 8 192. Le chat tronquait donc son contexte, ses résultats d'outils et ses
    /// tours d'exploration d'un facteur QUATRE, en silence.
    #[test]
    fn les_budgets_suivent_la_fenetre() {
        // 8 192 : le comportement d'origine, préservé à l'identique.
        assert_eq!(budget_noeuds_pour(8192), 60);
        assert_eq!(labels_echantillonnes_pour(8192), 8);
        assert_eq!(noeuds_par_label_pour(8192), 4);
        assert_eq!(max_relations_pour(8192), 40);
        assert_eq!(tours_outils_pour(8192), 4);
        assert_eq!(budget_contexte_pour(8192), 12_000);

        // 32 768 : ce que le service distant sert réellement (mesuré : accepté,
        // refusé à 46 205 jetons). Les budgets doivent en profiter.
        assert_eq!(budget_noeuds_pour(32_768), 200);
        assert_eq!(labels_echantillonnes_pour(32_768), 14);
        assert_eq!(noeuds_par_label_pour(32_768), 8);
        assert_eq!(max_relations_pour(32_768), 80);
        assert_eq!(tours_outils_pour(32_768), 6);
        assert_eq!(budget_contexte_pour(32_768), 34_078);
        assert_eq!(max_caracteres_outil_pour(32_768), 8_000);
    }

    /// Une fenêtre plus large ne doit JAMAIS donner un budget plus petit : sinon
    /// un modèle plus capable recevrait moins de matière, ce qui serait absurde
    /// et invisible.
    #[test]
    fn les_budgets_ne_diminuent_jamais_avec_la_fenetre() {
        let fenetres = [1024usize, 4096, 8192, 16_384, 32_768, 65_536, 262_144];
        let mut prec = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        for f in fenetres {
            let actuel = (
                budget_noeuds_pour(f),
                labels_echantillonnes_pour(f),
                noeuds_par_label_pour(f),
                max_relations_pour(f),
                tours_outils_pour(f),
                max_caracteres_outil_pour(f),
            );
            assert!(
                actuel.0 >= prec.0
                    && actuel.1 >= prec.1
                    && actuel.2 >= prec.2
                    && actuel.3 >= prec.3
                    && actuel.4 >= prec.4
                    && actuel.5 >= prec.5,
                "fenêtre {f} : budget en recul ({actuel:?} < {prec:?})"
            );
            prec = actuel;
        }
    }

    /// Les plafonds restent BORNÉS : une fenêtre énorme ne doit pas faire rendre
    /// 300 lignes à un outil ni citer 10 000 nœuds — noyer le modèle coûte la
    /// réponse aussi sûrement qu'une fenêtre trop petite.
    #[test]
    fn les_plafonds_restent_bornees() {
        assert_eq!(budget_noeuds_pour(262_144), 200);
        assert_eq!(budget_contexte_pour(262_144), 40_000);
        assert_eq!(max_caracteres_outil_pour(262_144), 8_000);
        assert_eq!(max_lignes_outil_pour(262_144), 60);
        assert!(max_lignes_outil_pour(1) >= 12, "jamais moins de 12 lignes");
    }

    #[test]
    fn type_arête_non_identifiant_refuse() {
        assert!(type_interpolable("CALLS"));
        assert!(type_interpolable("HAS_STEP"));
        assert!(!type_interpolable("CALLS]->(x) DETACH DELETE x //"));
        assert!(!type_interpolable(""));
        assert!(!type_interpolable("a b"));
        assert!(!type_interpolable("ÉTÉ"));
    }

    /// La part lue doit être un chiffre, pas une impression. Le cas de référence
    /// est celui du défaut mesuré : 24 nœuds sur les 4 685 de `this-rs/this`.
    #[test]
    fn part_lue_chiffree() {
        assert_eq!(part_lue(24, 4685, 1), "0.5 %");
        assert_eq!(part_lue(46, 4685, 1), "1.0 %");
        assert_eq!(part_lue(0, 100, 1), "0.0 %");
    }

    /// Un total nul ne doit pas produire une part : on ne divise pas par zéro,
    /// et on ne prétend pas non plus connaître la part. Un chiffre faux se lit
    /// comme une preuve — c'est pire qu'un chiffre absent.
    #[test]
    fn part_inconnue_si_total_nul() {
        assert_eq!(part_lue(10, 0, 1), "part inconnue");
        assert_eq!(part_lue(10, -3, 1), "part inconnue");
    }
    /// LE DÉFAUT QUI A COÛTÉ UNE RÉPONSE INVENTÉE. « recette », « tissage » et
    /// « weave » étaient absents du contexte ; le modèle a répondu quand même, en
    /// inventant. Ce test fixe l'exigence d'outil dans ce cas précis.
    #[test]
    fn outil_exige_quand_la_question_sort_du_contexte() {
        let contexte = "STORE LU : /tmp/x.obrain / Function 1500 / Method 1039";
        assert_eq!(
            exige_un_outil("Quelles recettes d'interrogation existent ?", contexte),
            Some("recettes".to_string())
        );
        // Un mot présent dans le contexte ne déclenche rien.
        assert_eq!(exige_un_outil("Combien de Method ?", contexte), None);
    }

    /// Les mots vides ne doivent pas déclencher l'exigence : sinon toute question
    /// forcerait un tour d'outil, et le coût serait payé pour rien.
    #[test]
    fn mots_vides_ignores() {
        let contexte = "STORE LU : /tmp/x.obrain";
        assert_eq!(exige_un_outil("Montre-moi la liste de tout ce projet", contexte), None);
        assert_eq!(exige_un_outil("Que fait ceci ?", contexte), None);
    }
}
