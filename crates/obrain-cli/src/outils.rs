//! OUTILS DE L'ENGINE — **le chat interroge la base au lieu de la lire une fois**.
//!
//! ## Le défaut que ce module corrige
//!
//! Jusqu'ici la conversation recevait un contexte figé au temps t0 puis un seul
//! aller-retour vers le modèle. Le modèle ne pouvait **pas** demander la suite :
//! il concluait sur 0,7 % de la matière. Densifier le contexte initial ne
//! pouvait pas suffire — on améliorait *quoi* est lu au premier tour, pas la
//! capacité d'en lire davantage.
//!
//! Deux faits mesurés rendent la boucle possible :
//!
//! 1. **Le tool-calling natif fonctionne** sur le service local. Contrôle
//!    envoyé à `POST /v1/chat/completions` avec un outil `obrain_query` :
//!    réponse `finish_reason: "tool_calls"` et arguments bien formés.
//! 2. **Le référentiel d'interrogation existe déjà dans la base** : 29 recettes
//!    `proc:ref*` (familles `F1-self`, `F2-code`, `F3-runtime`) portant un
//!    `body` GQL exécutable. Mesuré : elles vivent dans **`self-demo`**, et
//!    runtime-dev / runtime-prod en ont **zéro** — d'où l'`Atlas`.
//!
//! ## Les contraintes dures, et ce qu'elles imposent
//!
//! - **`n_ctx = 8192` jetons au total** (fenêtre partagée entre contexte,
//!   historique, résultats d'outils ET réponse). On ne peut donc pas laisser un
//!   outil rendre 500 lignes : chaque résultat est **borné en caractères**, et le
//!   nombre de tours est plafonné.
//! - **Les parcours non bornés sont catastrophiques** : `MATCH (n)--(m) …
//!   ORDER BY count(m) DESC` coûte **14,1 s** sur runtime-dev, et un sujet absent
//!   a coûté **16 s** avant qu'on apprenne à vérifier par index. Une requête sans
//!   `LIMIT` est donc **incomplétée d'un `LIMIT`** avant exécution.
//! - **La base est en lecture seule** (`cached_ro`) — mais on refuse les
//!   écritures **avant** de les envoyer, pour rendre l'erreur lisible au modèle.
//!
//! ## Les quatre outils
//!
//! | outil | ce qu'il donne | pourquoi |
//! |---|---|---|
//! | `obrain_query` | une requête GQL bornée, sur le monde courant ou un autre | la densification à la demande |
//! | `obrain_echantillon` | n nœuds d'un TYPE (`Function`, `Class`…) | le levier mesuré le moins cher (0,3-12 ms) |
//! | `obrain_voisinage` | le voisinage réel à 1 saut d'un nœud | passer d'un type à une entité précise |
//! | `obrain_mondes` | les mondes et leurs synapses, **avec les clés de jointure** | interroger les autres graphes *par leurs liens* |

use std::time::Instant;

use anyhow::Result;
use obrain_common::types::Value as OV;
use obrain_engine::ObrainDB;

use crate::ancrage::Atlas;

/// Nombre de tours d'outils maximum dans une même réponse.
///
/// Dérivé de la fenêtre (`engine::tours_outils`) : quatre tours sur 8 192 jetons,
/// six au-delà de 16 384. Au-delà, le modèle explore au lieu de répondre.
pub fn max_tours() -> usize {
    crate::engine::tours_outils()
}

/// Plafond de lignes rendues par un appel d'outil.
pub fn max_lignes() -> usize {
    crate::engine::max_lignes_outil_pour(crate::engine::fenetre_jetons())
}

/// Plafond de **caractères** rendus par un appel d'outil.
///
/// C'est la contrainte réelle. Réglé à 900 d'abord — et mesuré TROP SERRÉ : à la
/// question « montre-moi 20 Method », l'outil a bien rendu 20 lignes en 0 ms,
/// mais seules 4 tenaient dans le résultat. Le modèle a fait ce qu'il fallait
/// (il a DIT « le résultat est tronqué à 900 caractères »), mais la demande
/// n'était pas servie.
///
/// 8 192 → ~2 000 caractères · 32 768 → jusqu'à 8 000 : un huitième de la
/// fenêtre, borné des deux côtés. Le plafond suit le modèle servi — le figer à
/// 2 000 faisait refuser 16 des 20 lignes demandées par une question légitime.
pub fn max_caracteres() -> usize {
    crate::engine::max_caracteres_outil_pour(crate::engine::fenetre_jetons())
}

/// Une requête sans `LIMIT` reçoit celui-ci.
pub const LIMITE_IMPOSEE: usize = 40;

/// Plafond d'appels d'outils EXÉCUTÉS dans un même tour.
///
/// Le nombre d'appels est piloté par le modèle. Sans plafond, un tour pouvait en
/// demander vingt et lancer vingt requêtes — jusqu'à 14,1 s chacune, mesuré —
/// avant que le seul contrôle de fenêtre (en TÊTE DE TOUR SUIVANT) n'intervienne.
/// Huit couvre largement un besoin réel (la boucle entière est déjà bornée à
/// quatre tours) et arrête la rafale.
pub const MAX_APPELS_PAR_TOUR: usize = 8;

/// Un appel d'outil demandé par le modèle.
#[derive(Clone, Debug, Default)]
pub struct Appel {
    pub id: String,
    pub nom: String,
    /// Arguments JSON bruts (tels que le modèle les a produits).
    pub arguments: String,
}

/// Ce qu'un outil a réellement rendu — et ce qu'il a coûté.
#[derive(Clone, Debug)]
pub struct Resultat {
    /// Le texte envoyé au modèle (borné).
    pub texte: String,
    /// Le monde **réellement interrogé** — pas celui qu'on croyait viser.
    ///
    /// Défaut corrigé : l'événement affiché portait le monde ANCré. À la
    /// question sur `this-rs/this`, l'écran aurait annoncé ce monde même si
    /// l'outil avait été chercher le référentiel dans `self-demo`. Le nom du
    /// monde vient donc du résultat, où il est connu.
    pub monde: String,
    pub lignes: usize,
    pub ms: u64,
    /// Vrai si le contenu a été coupé au plafond : le modèle doit le savoir.
    pub tronque: bool,
    pub erreur: Option<String>,
    /// Renseigné par les outils d'ÉCRITURE : ce qui vient d'être journalisé.
    pub memoire: Option<crate::memoire::Entry>,
}

/// La description des outils, telle qu'on l'envoie au modèle.
///
/// Les descriptions sont **courtes à dessein** : elles occupent la fenêtre de
/// 8 192 jetons à chaque tour. Elles disent l'essentiel — quoi, et sous quelle
/// contrainte — sans pédagogie inutile.
pub fn definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "obrain_query",
                "description": "Requête GQL EN LECTURE SEULE (lignes bornées). Sans `monde` : \
                                la base ancrée. Pour densifier : compter, lister, suivre des arêtes.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Requête GQL" },
                        "monde": { "type": "string", "description": "Monde visé (qn, nom ou fragment). Défaut : le monde ancré." }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_echantillon",
                "description": "n nœuds d'un TYPE (Function, Class, Method…). Le contexte liste \
                                les types et leur volume : c'est l'outil pour en voir le contenu.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "description": "Label du type de nœud" },
                        "n": { "type": "integer", "description": "Nombre de nœuds (max 30)" },
                        "monde": { "type": "string", "description": "Monde visé. Défaut : le monde ancré." }
                    },
                    "required": ["type"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_voisinage",
                "description": "Voisinage réel à 1 saut, par qualified_name EXACT (les noms \
                                courts ne sont pas indexés).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "qn": { "type": "string", "description": "qualified_name exact du nœud" },
                        "monde": { "type": "string", "description": "Monde visé. Défaut : le monde ancré." }
                    },
                    "required": ["qn"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_recherche",
                "description": "RECHERCHE par MOTS-CLÉS (BM25, fusion RRF entre index) : trouve \
                                les nœuds SANS connaître le qualified_name exact. Rend \
                                `qualified_name · score · source`, à passer à obrain_voisinage.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "q": { "type": "string", "description": "Termes de recherche (mots-clés, pas de syntaxe GQL)." },
                        "k": { "type": "integer", "description": "Nombre de résultats (défaut 20)." },
                        "label": { "type": "string", "description": "Restreindre à un type de nœud (défaut : index du store + Function)." },
                        "monde": { "type": "string", "description": "Monde visé. Défaut : le monde ancré." }
                    },
                    "required": ["q"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_mondes",
                "description": "Les mondes de l'agent et les SYNAPSES qui les relient, avec \
                                leurs preuves et les CLÉS partagées : c'est par là qu'on interroge \
                                un autre graphe.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "monde": { "type": "string", "description": "Si donné : les synapses de CE monde seulement." }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_hubs",
                "description": "Les nœuds LES PLUS CONNECTÉS du monde (par registre de degrés, \
                                quelques millisecondes — jamais un parcours). Dit qui porte la \
                                structure.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "n": { "type": "integer", "description": "Combien de hubs (défaut 20, max 40)." },
                        "monde": { "type": "string", "description": "Monde visé. Défaut : le monde ancré." }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_traverser",
                "description": "SUIT UNE SYNAPSE : à partir d'une CLÉ partagée (un \
                                qualified_name que obrain_mondes a montré), dit dans QUELS \
                                mondes elle vit et ce qu'elle Y EST. C'est ainsi qu'on \
                                interroge un autre graphe par son lien, sans recopier la clé.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cle": { "type": "string", "description": "La clé partagée (qualified_name exact)." },
                        "monde": { "type": "string", "description": "Ne traverser que depuis CE monde (défaut : le monde ancré)." }
                    },
                    "required": ["cle"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_recettes",
                "description": "Les recettes d'interrogation du référentiel de l'agent. Sans \
                                `monde`, il est cherché dans les mondes du self (il n'est pas \
                                dans la base ancrée).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "monde": { "type": "string", "description": "Monde où chercher le référentiel. Défaut : recherche automatique." }
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_memoriser",
                "description": "ÉCRITURE. Retient une NOTE (journalisée, appliquée ensuite dans \
                                la base du Self). Seulement si on demande de retenir, ou pour un \
                                fait VÉRIFIÉ et durable.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "texte": { "type": "string", "description": "Ce qu'il faut retenir, en une ou deux phrases." },
                        "sujet": { "type": "string", "description": "Sujet ou monde concerné." }
                    },
                    "required": ["texte"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_structurer",
                "description": "ÉCRITURE. Structure ce qui vient d'être établi : OBJECTIF, \
                                PROCEDURE (recette réutilisable) ou ÉTAT du Self (versionné).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "genre": { "type": "string", "enum": ["objectif", "procedure", "etat-self"] },
                        "nom": { "type": "string", "description": "Nom court (objectif / procedure)." },
                        "texte": { "type": "string", "description": "Le contenu : l'objectif, la description de la recette, ou l'état." },
                        "corps": { "type": "string", "description": "procedure seulement : la requête GQL de la recette." },
                        "prochaine_action": { "type": "string", "description": "etat-self seulement : ce qui vient ensuite." }
                    },
                    "required": ["genre", "texte"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "obrain_recette",
                "description": "Le corps GQL d'une recette nommée (ex. `code.hot`), à exécuter \
                                ensuite via obrain_query.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "nom": { "type": "string", "description": "Nom de la recette, ex. code.hot" },
                        "monde": { "type": "string", "description": "Monde où chercher la recette. Défaut : recherche automatique." }
                    },
                    "required": ["nom"]
                }
            }
        }
    ])
}

/// Les outils d'écriture, traités à part : ils ne lisent aucune base.
fn appareil_ecriture(nom: &str) -> bool {
    matches!(nom, "obrain_memoriser" | "obrain_structurer")
}

/// Mots-clés qui trahissent une ÉCRITURE.
///
/// La base est ouverte en lecture seule : l'écriture échouerait de toute façon.
/// Mais on refuse **avant** l'envoi, pour que le modèle reçoive une raison claire
/// au lieu d'une erreur d'exécution opaque — et pour qu'aucun dialecte acceptant
/// l'écriture ne devienne une porte ouverte par mégarde.
const ECRITURES: &[&str] = &[
    "CREATE", "MERGE", "DELETE", "DETACH", "SET", "REMOVE", "DROP", "INSERT", "UPDATE",
];

/// La requête est-elle en lecture seule ? (contrôle par MOT entier)
fn est_lecture_seule(gql: &str) -> Option<&'static str> {
    let haut = gql.to_uppercase();
    let mots: Vec<&str> = haut
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|m| !m.is_empty())
        .collect();
    ECRITURES
        .iter()
        .find(|e| mots.iter().any(|m| m == *e))
        .copied()
}

/// Ajoute un `LIMIT` si la requête n'en porte pas.
///
/// Une requête non bornée est le risque principal : `MATCH (n)--(m) …` coûte
/// 14,1 s sur runtime-dev (mesuré). On borne donc systématiquement.
pub fn borner(gql: &str) -> String {
    // ── LA BORNE DOIT ÊTRE UNE BORNE (2026-09-11) ─────────────────────────
    // Avant : `if gql.to_uppercase().contains("LIMIT")` — une SOUS-CHAÎNE,
    // n'importe où. Trois conséquences mesurables :
    //   · `MATCH (n:Function) WHERE n.name CONTAINS 'limit' RETURN n` n'était
    //     PAS bornée (le mot est dans un littéral) ;
    //   · un `LIMIT 1000000` écrit par le modèle était honoré tel quel —
    //     LIMITE_IMPOSEE n'était qu'un défaut, pas un plafond ;
    //   · une requête finissant par `// commentaire` recevait un `LIMIT` avalé
    //     par le commentaire.
    // On cherche donc le VRAI jeton `LIMIT`, hors chaînes et hors commentaires,
    // on le REMPLACE s'il dépasse le plafond, et on insère la borne avant tout
    // commentaire de fin.
    match limite_de(gql) {
        Some((debut, fin_nombre, n)) => {
            if n <= LIMITE_IMPOSEE as u64 {
                gql.to_string()
            } else {
                let mut out = String::with_capacity(gql.len());
                out.push_str(&gql[..debut]);
                out.push_str(&LIMITE_IMPOSEE.to_string());
                out.push_str(&gql[fin_nombre..]);
                out
            }
        }
        None => {
            let (corps, commentaire) = scinder_commentaire_final(gql);
            let corps = corps.trim_end().trim_end_matches(';');
            if commentaire.is_empty() {
                format!("{corps} LIMIT {LIMITE_IMPOSEE}")
            } else {
                format!("{corps} LIMIT {LIMITE_IMPOSEE}\n{commentaire}")
            }
        }
    }
}

/// Le VRAI jeton `LIMIT` d'une requête : `(début du nombre, fin du nombre, valeur)`.
///
/// Ne regarde ni dans les littéraux (`'…'`, `"…"`), ni dans les commentaires
/// (`//`, `/* … */`) : c'est précisément ce que la version précédente ignorait.
fn limite_de(gql: &str) -> Option<(usize, usize, u64)> {
    let b = gql.as_bytes();
    let mut i = 0usize;
    let mut trouve: Option<(usize, usize, u64)> = None;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' { i += 1; }
                    i += 1;
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' { i += 1; }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { i += 1; }
                i += 2;
            }
            c if c.is_ascii_alphabetic() => {
                let debut = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
                if gql[debut..i].eq_ignore_ascii_case("LIMIT") {
                    let mut j = i;
                    while j < b.len() && b[j].is_ascii_whitespace() { j += 1; }
                    let debut_nombre = j;
                    while j < b.len() && b[j].is_ascii_digit() { j += 1; }
                    if j > debut_nombre
                        && let Ok(n) = gql[debut_nombre..j].parse::<u64>()
                    {
                        trouve = Some((debut_nombre, j, n));
                    }
                }
            }
            _ => i += 1,
        }
    }
    trouve
}

/// Sépare une requête de son commentaire `// …` ou `/* … */`.
///
/// Le commentaire est cherché par un BALAYAGE qui ignore les littéraux : un
/// `//` à l'intérieur d'une chaîne (`WHERE n.x = 'a // b'`) n'est pas un
/// commentaire, et le prendre pour tel couperait la requête en deux — un
/// `rfind(" //")` faisait exactement cela.
fn scinder_commentaire_final(gql: &str) -> (&str, &str) {
    let b = gql.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') || b.get(i + 1) == Some(&b'*') => {
                return (gql[..i].trim_end(), &gql[i..]);
            }
            _ => i += 1,
        }
    }
    (gql.trim_end(), "")
}

/// Lit la base — **et rend l'échec au lieu de le confondre avec un vide**.
///
/// Défaut mesuré (2026-09-11) : `lire` faisait `.unwrap_or_default()`. Une
/// requête REFUSÉE par le moteur devenait donc une liste vide, rendue au modèle
/// comme « (aucune ligne) » avec `erreur: null`. Dans un chat réel, la même
/// requête sur la même base répondait `semantic error: Undefined variable
/// '_agg_0'` à `/api/query`, pendant que l'outil annonçait zéro ligne : le
/// modèle a conclu « il n'y a rien », a rappelé un outil pour rien, puis a
/// laissé fuir un `<tool_call>` brut dans sa réponse. Un vide et un refus sont
/// deux faits DIFFÉRENTS ; les confondre est le mensonge que ce module existe
/// pour empêcher.
fn lire(db: &ObrainDB, gql: &str) -> Result<Vec<Vec<OV>>, String> {
    db.session()
        .execute(gql)
        .map(|r| r.rows)
        .map_err(|e| e.to_string())
}

/// Transforme un refus du moteur en RÉSULTAT : le modèle voit le message exact,
/// et l'écran le voit aussi (`erreur` est poussé dans l'événement).
fn echec_moteur(e: String, gql: &str, ms: u64) -> Resultat {
    let cause = format!("le moteur a REFUSÉ la requête : {e}");
    let mut r = echec(cause, ms);
    r.texte = format!("ERREUR : {e}\nRequête refusée : {gql}");
    r
}

fn txt(v: Option<&OV>) -> String {
    match v {
        Some(OV::String(s)) => s.to_string(),
        Some(OV::Int64(x)) => x.to_string(),
        Some(OV::Float64(x)) => x.to_string(),
        Some(OV::Bool(b)) => b.to_string(),
        Some(OV::Null) | None => "null".to_string(),
        Some(autre) => format!("{autre:?}"),
    }
}

/// Assemble un résultat : lignes mises en forme, bornées en taille, et la coupe
/// est **dite** — un modèle qui reçoit 30 lignes tronquées sans le savoir
/// conclurait sur une liste incomplète.
fn rendre_sur(mut lignes: Vec<String>, ms: u64, monde: &str) -> Resultat {
    let mut r = rendre(lignes.drain(..).collect(), ms);
    r.monde = monde.to_string();
    r
}

fn rendre(mut lignes: Vec<String>, ms: u64) -> Resultat {
    let total = lignes.len();
    let mut texte = String::new();
    let mut tronque = false;
    let MAX_CARACTERES = max_caracteres();
    // ── LE NOMBRE ANNONCÉ EST CELUI QUI A ÉTÉ LIVRÉ ────────────────────────
    // `lignes: total` comptait les lignes OFFERTES, calculées avant la coupe au
    // plafond de caractères. Mesuré : l'écran affichait « 29 lignes » pendant
    // que le raisonnement du modèle disait « 17 out of 29 lines are
    // displayed ». Un chiffre qui ne mesure pas ce qu'il annonce est exactement
    // ce que la vue corrige partout ailleurs : on compte donc les lignes
    // RÉELLEMENT écrites dans le texte envoyé.
    let mut livrees = 0usize;
    for (i, l) in lignes.drain(..).enumerate() {
        let l = if l.chars().count() > 300 {
            let court: String = l.chars().take(300).collect();
            format!("{court}…")
        } else {
            l
        };
        if texte.len() + l.len() + 1 > MAX_CARACTERES {
            tronque = true;
            texte.push_str(&format!(
                "\n[COUPÉ au plafond de {MAX_CARACTERES} caractères : {} lignes sur {total} \
                 affichées — affine la requête.]\n",
                livrees
            ));
            break;
        }
        texte.push_str(&l);
        texte.push('\n');
        livrees += 1;
    }
    if texte.is_empty() {
        texte = "(aucune ligne)".to_string();
    }
    Resultat {
        texte,
        monde: String::new(),
        lignes: livrees,
        ms,
        tronque,
        erreur: None,
        memoire: None,
    }
}

fn echec(msg: String, ms: u64) -> Resultat {
    Resultat {
        texte: format!("ERREUR : {msg}"),
        monde: String::new(),
        lignes: 0,
        ms,
        tronque: false,
        erreur: Some(msg),
        memoire: None,
    }
}

/// Argmuent chaîne d'un JSON d'appel (tolérant).
fn arg<'a>(v: &'a serde_json::Value, nom: &str) -> Option<&'a str> {
    v.get(nom).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
}

/// ── LE LABEL DOIT ÊTRE DANS LE MOTIF, PAS DANS LE PRÉDICAT ─────────────────
///
/// Mesuré le 2026-09-13 (chat du 8432, store `runtime-dev`, 408 832 nœuds) :
///
/// ```text
/// MATCH (n) WHERE n.qualified_name STARTS WITH '…' AND labels(n) CONTAINS 'Class'  → 68,6 · 71,0 ms
/// MATCH (n) WHERE n.qualified_name STARTS WITH '…' AND n:Class                     → 56,0 · 57,3 ms
/// MATCH (n:Class) WHERE n.qualified_name STARTS WITH '…'                           →  2,4 ms
/// ```
///
/// Le planificateur ne reconnaît un label que dans le MOTIF (`NodeScan { label }`) ;
/// en prédicat, il balaie le graphe entier et vérifie. Le modèle, lui, écrit la
/// forme « fonction » — c'est le `labels(n) CONTAINS 'Class'` de sa requête — et
/// payait donc 30 fois le prix de la même question.
///
/// ON AJOUTE, ON NE RETIRE RIEN. Le prédicat d'origine est laissé en place : il
/// devient redondant (le motif l'implique), donc la réponse ne peut pas changer —
/// alors que le retirer serait une chirurgie de texte dont une erreur produirait
/// des lignes MANQUANTES. Un label déjà présent dans le motif fait abandonner la
/// traduction : ajouter `:Class` à `(n:Function)` serait une contradiction.
pub fn label_dans_le_motif(gql: &str) -> String {
    let mut sortie = gql.to_string();
    for (variable, label) in predicats_label(gql) {
        // Le motif doit être NU : `(n)` et rien d'autre. Un motif déjà étiqueté
        // (`(n:Function)`) ou relié (`(n)-->(m)`) est laissé tel quel.
        let nu = format!("({variable})");
        let deja = format!("({variable}:");
        if sortie.contains(&deja) {
            continue;
        }
        // ── LE MOTIF SEULEMENT, JAMAIS L'INTÉRIEUR D'UN APPEL ──────────────
        // `labels(n)` CONTIENT `(n)`. Une substitution aveugle produisait
        // `labels(n:Class)` — faute attrapée par le test
        // `la_traduction_ne_touche_pas_ce_qu_elle_ne_comprend_pas`, et c'est
        // exactement le risque de la chirurgie de texte. Un motif `(n)` commence
        // un JETON : il est précédé d'un espace, d'une parenthèse, d'une virgule
        // ou du début du texte — jamais d'un caractère de nom.
        let Some(position) = position_du_motif(&sortie, &nu) else {
            continue;
        };
        sortie.replace_range(position..position + nu.len(), &format!("({variable}:{label})"));
    }
    sortie
}

/// La position du PREMIER `(var)` qui est bien un MOTIF, ou `None`.
///
/// Un motif commence un jeton : le caractère précédent n'appartient pas à un nom
/// (`labels(n)` ne compte donc pas). Sans cette règle, la traduction corrompait
/// la requête — le test le montre, et c'est la seule raison d'être de cette
/// fonction.
fn position_du_motif(texte: &str, motif: &str) -> Option<usize> {
    let octets = texte.as_bytes();
    let mut depuis = 0usize;
    while let Some(p) = texte[depuis..].find(motif) {
        let position = depuis + p;
        let precedent_ok = position == 0
            || !(octets[position - 1].is_ascii_alphanumeric()
                || octets[position - 1] == b'_'
                || octets[position - 1] == b'$'
                || octets[position - 1] == b'.');
        if precedent_ok {
            return Some(position);
        }
        depuis = position + 1;
    }
    None
}

/// Les couples `(variable, label)` des prédicats de label d'un texte GQL :
/// `labels(n) CONTAINS 'Class'` et `'Class' IN labels(n)`. Analyse par balayage,
/// sans expression régulière : on ne dépend d'aucun moteur pour une substitution
/// dont une erreur coûterait des lignes.
fn predicats_label(gql: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let octets = gql.as_bytes();
    let mut i = 0usize;
    while i < octets.len() {
        // Forme 1 : `labels(<var>) CONTAINS '<label>'`
        if gql[i..].starts_with("labels(") {
            let debut_var = i + "labels(".len();
            if let Some(fin_var) = gql[debut_var..].find(')') {
                let variable = gql[debut_var..debut_var + fin_var].trim().to_string();
                let apres = &gql[debut_var + fin_var + 1..];
                let apres = apres.trim_start();
                if let Some(reste) = apres.strip_prefix("CONTAINS") {
                    if let Some(label) = premier_litteral(reste.trim_start()) {
                        if est_identifiant(&variable) && est_identifiant(&label) {
                            out.push((variable, label));
                        }
                    }
                }
                i = debut_var + fin_var + 1;
                continue;
            }
        }
        // Forme 2 : `'<label>' IN labels(<var>)`
        if let Some((label, apres)) = litteral_en_tete(&gql[i..])
            && let Some(reste) = apres.trim_start().strip_prefix("IN")
            && let Some(reste) = reste.trim_start().strip_prefix("labels(")
            && let Some(fin) = reste.find(')')
        {
            let variable = reste[..fin].trim().to_string();
            if est_identifiant(&variable) && est_identifiant(&label) {
                out.push((variable, label));
            }
        }
        i += 1;
    }
    out
}

/// Le premier littéral `'…'` ou `"…"` d'un texte, et ce qui suit.
fn litteral_en_tete(texte: &str) -> Option<(String, &str)> {
    let t = texte.trim_start();
    let guillemet = t.chars().next()?;
    if guillemet != '\'' && guillemet != '"' {
        return None;
    }
    let interieur = &t[1..];
    let fin = interieur.find(guillemet)?;
    Some((interieur[..fin].to_string(), &interieur[fin + 1..]))
}

/// Le premier littéral d'un texte, sans rendre le reste.
fn premier_litteral(texte: &str) -> Option<String> {
    litteral_en_tete(texte).map(|(l, _)| l)
}

/// Un nom de variable ou de label : lettres, chiffres, `_`, `$`, `.` — jamais un
/// opérateur, jamais un espace. C'est ce qui rend la substitution sûre.
fn est_identifiant(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.')
}

/// ── UN `tool_call` ÉCRIT DANS LE TEXTE N'EST PAS UNE RÉPONSE ────────────────
///
/// Défaut mesuré le 2026-09-13 sur le dashboard (chat du port 8432), moteur
/// Qwen3.8-27B servi par le DGX : après huit appels d'outils exécutés, le tour
/// suivant a produit **zéro appel dans le canal `tool_calls`** et, à la place, le
/// markup que le gabarit du modèle utilise, écrit dans son RAISONNEMENT :
///
/// ```text
/// <tool_call> <function=obrain_query> <parameter=query> MATCH (n) WHERE
/// n.qualified_name STARTS WITH '…' RETURN … </parameter> </function> </tool_call>
/// ```
///
/// Le moteur n'y a vu que du texte : le markup est parti à l'écran comme
/// « raisonnement », aucun outil n'a été appelé, et la fin annoncée était `stop`
/// — une réponse présentée comme complète alors qu'elle n'était qu'un appel
/// d'outil déguisé. C'est ce que l'utilisateur a refusé : « il ne doit pas
/// répondre un tool_call, c'est un défaut ».
///
/// Ce parseur rend donc les appels RÉELS que ce texte contient, pour qu'ils
/// soient exécutés comme s'ils étaient arrivés par le canal normal. Il est
/// TOLÉRANT à dessein (deux syntaxes, balises manquantes, espaces et retours à la
/// ligne) : ce markup vient d'un gabarit, pas d'une spécification, et refuser une
/// variante reviendrait à laisser le défaut en place.
///
/// Rend une liste VIDE quand le texte n'est pas un appel d'outil : l'appelant ne
/// doit donc jamais confondre « prose » et « appel ».
pub fn appels_du_texte(texte: &str) -> Vec<Appel> {
    let mut appels: Vec<Appel> = Vec::new();
    // ── SYNTAXE 1 : JSON (`{"name": …, "arguments": {…}}`) ──────────────────
    // Également ce que produit `tools` en mode strict chez plusieurs services :
    // un objet JSON dans le texte. On tente AVANT l'analyse par balises, qui
    // verrait des `<` nulle part et ne trouverait rien.
    for bloc in blocs_json(texte) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&bloc) {
            let obj = match &v {
                serde_json::Value::Array(a) => a.first().cloned(),
                autre => Some(autre.clone()),
            };
            if let Some(o) = obj {
                let nom = o.get("name").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                let arguments = match o.get("arguments") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(a) => a.to_string(),
                    None => "{}".to_string(),
                };
                if !nom.is_empty() {
                    appels.push(Appel {
                        id: format!("texte-{}", appels.len()),
                        nom,
                        arguments,
                    });
                }
            }
        }
    }
    if !appels.is_empty() {
        return appels;
    }

    // ── SYNTAXE 2 : BALISES (`<function=…><parameter=clé>valeur</parameter>`) ─
    // On découpe d'abord en appels : chaque `<function=` ouvre un appel, que
    // ferme `</function>` — ou le `<function=` suivant, quand le modèle a oublié
    // la fermeture (constaté : un flux coupé en plein appel).
    let mut curseur = 0usize;
    while let Some(p) = texte[curseur..].find("<function=") {
        let debut = curseur + p;
        let suite = &texte[debut + "<function=".len()..];
        let fin_nom = suite
            .find(|c: char| c == '>' || c.is_whitespace())
            .unwrap_or(suite.len());
        let nom = suite[..fin_nom]
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        // Le corps va jusqu'à `</function>` inclus, ou jusqu'au prochain appel.
        let corps_debut = debut + "<function=".len() + fin_nom;
        let reste = &texte[corps_debut..];
        let corps_fin = reste.find("</function>").map(|q| q + "</function>".len());
        let prochain = reste.find("<function=");
        let coupe = match (corps_fin, prochain) {
            (Some(f), Some(n)) => f.min(n),
            (Some(f), None) => f,
            (None, Some(n)) => n,
            (None, None) => reste.len(),
        };
        if !nom.is_empty() {
            appels.push(Appel {
                id: format!("texte-{}", appels.len()),
                nom,
                arguments: arguments_du_corps(&reste[..coupe]),
            });
        }
        // `corps_debut` dépasse toujours `curseur` (on a consommé `<function=`) :
        // la progression est donc garantie sans garde supplémentaire — un garde
        // ferait ici SAUTER l'appel suivant, écrit sans fermeture.
        curseur = corps_debut + coupe;
    }
    appels
}

/// Les objets JSON complets d'un texte (profondeur d'accolades respectée, chaînes
/// échappées respectées) — c'est ce qui évite de prendre le JSON d'un appel pour
/// le JSON d'un argument.
fn blocs_json(texte: &str) -> Vec<String> {
    let mut blocs = Vec::new();
    let octets = texte.as_bytes();
    let mut i = 0usize;
    while i < octets.len() {
        if octets[i] != b'{' {
            i += 1;
            continue;
        }
        let debut = i;
        let mut profondeur = 0i32;
        let mut dans_chaine = false;
        let mut echappe = false;
        let mut fin = None;
        while i < octets.len() {
            let c = octets[i];
            if dans_chaine {
                if echappe {
                    echappe = false;
                } else if c == b'\\' {
                    echappe = true;
                } else if c == b'"' {
                    dans_chaine = false;
                }
            } else if c == b'"' {
                dans_chaine = true;
            } else if c == b'{' {
                profondeur += 1;
            } else if c == b'}' {
                profondeur -= 1;
                if profondeur == 0 {
                    fin = Some(i + 1);
                    break;
                }
            }
            i += 1;
        }
        match fin {
            Some(f) => {
                blocs.push(texte[debut..f].to_string());
                i = f;
            }
            None => break, // accolade jamais fermée : rien d'exploitable
        }
    }
    blocs
}

/// Les arguments d'un corps d'appel en balises, rendus en JSON.
///
/// `<parameter=query>MATCH …</parameter>` → `{"query":"MATCH …"}`. Une balise
/// sans valeur ou sans fermeture est CONSERVÉE vide plutôt que rejetée : un
/// argument manquant est une erreur que l'outil sait dire, un appel perdu ne dit
/// rien.
fn arguments_du_corps(corps: &str) -> String {
    let mut obj = serde_json::Map::new();
    let mut i = 0usize;
    while let Some(p) = corps[i..].find("<parameter=") {
        let debut = i + p + "<parameter=".len();
        let suite = &corps[debut..];
        let fin_cle = suite.find('>').unwrap_or(suite.len());
        let cle = suite[..fin_cle]
            .trim()
            .trim_matches(|c| c == '"' || c == '\'' || c == '=')
            .to_string();
        // La valeur commence APRÈS le `>` de la balise ouvrante (absent si le
        // modèle n'en a pas mis : la valeur commence alors tout de suite).
        let valeur_debut = debut + fin_cle + usize::from(fin_cle < suite.len());
        let reste = &corps[valeur_debut.min(corps.len())..];
        let (valeur, fin) = match reste.find("</parameter>") {
            Some(q) => (reste[..q].to_string(), valeur_debut + q + "</parameter>".len()),
            // Sans fermeture : jusqu'à la balise suivante, ou la fin du corps.
            None => {
                let q = reste.find("<parameter=").unwrap_or(reste.len());
                (reste[..q].to_string(), valeur_debut + q)
            }
        };
        if !cle.is_empty() {
            obj.insert(cle, serde_json::Value::String(valeur.trim().to_string()));
        }
        i = if fin > i { fin } else { i + 1 };
    }
    serde_json::Value::Object(obj).to_string()
}

/// Les mondes où peut vivre le référentiel de l'agent : ceux de nature `self`.
///
/// Mesuré : les 29 recettes `proc:ref*` sont dans `self-demo`, et runtime-dev /
/// runtime-prod en ont **zéro**. Le référentiel appartient à l'agent, pas au
/// graphe qu'on regarde : le chercher là où il est est la seule façon qu'un outil
/// le trouve sans que l'utilisateur ait à savoir où il habite.
fn mondes_du_self(atlas: &Atlas) -> Vec<String> {
    atlas
        .mondes
        .iter()
        .filter(|m| m.nature == "self")
        .map(|m| m.qn.clone())
        .collect()
}

/// Exécute un appel d'outil — **toujours** en lecture seule, toujours borné.
///
/// `db_ancre` est la base déjà ouverte du monde ancré ; `ouvrir` résout un autre
/// monde (fourni par le serveur, qui possède le cache de handles).
pub fn executer(
    db_ancre: &ObrainDB,
    db_du_monde: &dyn Fn(&str) -> Option<std::sync::Arc<ObrainDB>>,
    // La base SYNAPTIQUE, si elle est DÉJÀ ouverte : c'est elle qui porte les
    // ancres (`Anchor`) et leur appartenance aux mondes (`IN_WORLD`), donc de
    // quoi savoir OÙ vit une clé partagée. Sans elle, la traversée se rabat sur
    // les échantillons de clés portés par les fibres.
    db_synapse: Option<&ObrainDB>,
    ancrage: &crate::engine::Ancrage,
    atlas: &Atlas,
    appel: &Appel,
) -> Resultat {
    let t = Instant::now();
    let args: serde_json::Value =
        serde_json::from_str(&appel.arguments).unwrap_or_else(|_| serde_json::json!({}));

    // ── LE MONDE VISÉ ───────────────────────────────────────────────────────
    // Sans `monde`, on reste sur la base ancrée. Avec, on résout par l'atlas —
    // c'est ainsi que le référentiel (qui vit dans self-demo, absent du store
    // regardé) devient atteignable.
    let demande = arg(&args, "monde");
    let monde = demande.and_then(|d| atlas.trouver(d));
    if demande.is_some() && monde.is_none() {
        return echec(
            format!(
                "monde introuvable : « {} ». Mondes connus : {}",
                demande.unwrap_or_default(),
                atlas
                    .mondes
                    .iter()
                    .map(|m| m.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            t.elapsed().as_millis() as u64,
        );
    }
    // Un autre monde que celui déjà ouvert par le serveur : on l'ouvre par son
    // handle. `garde` MANTIENT l'Arc vivant aussi longtemps que `db` l'emprunte.
    let autre = match monde {
        Some(m) if m.path.to_string_lossy() != ancrage.store => Some(m),
        _ => None,
    };
    let garde: Option<std::sync::Arc<ObrainDB>> = match autre {
        Some(m) => match db_du_monde(&m.qn) {
            Some(d) => Some(d),
            None => {
                return echec(
                    format!("le monde « {} » n'a pas pu être ouvert", m.label),
                    t.elapsed().as_millis() as u64,
                );
            }
        },
        None => None,
    };
    let db: &ObrainDB = garde.as_deref().unwrap_or(db_ancre);
    let nom_base = autre
        .map(|m| m.label.clone())
        .unwrap_or_else(|| ancrage.monde.clone().unwrap_or_else(|| "le monde ancré".into()));

    // ── LES OUTILS D'ÉCRITURE ───────────────────────────────────────────────
    // Ils n'ouvrent AUCUNE base : ils JOURNALISENT une intention, qui sera
    // appliquée plus tard dans la base du Self (voir `crate::memoire`, et
    // pourquoi le serveur ne peut pas écrire directement). C'est aussi ce qui
    // rend l'appel inoffensif : il ne peut ni bloquer, ni échouer sur un verrou.
    if appareil_ecriture(appel.nom.as_str()) {
        let provenance = format!(
            "{} · {}",
            ancrage.monde.clone().unwrap_or_else(|| "graphe global".into()),
            ancrage.store
        );
        let intention = match appel.nom.as_str() {
            "obrain_memoriser" => {
                let Some(texte) = arg(&args, "texte") else {
                    return echec("`texte` est requis".into(), t.elapsed().as_millis() as u64);
                };
                crate::memoire::Intention::Note {
                    texte: texte.to_string(),
                    sujet: arg(&args, "sujet").unwrap_or("").to_string(),
                }
            }
            "obrain_structurer" => {
                let Some(genre) = arg(&args, "genre") else {
                    return echec("`genre` est requis".into(), t.elapsed().as_millis() as u64);
                };
                let texte = arg(&args, "texte").unwrap_or("").to_string();
                match genre {
                    "objectif" => crate::memoire::Intention::Objectif {
                        nom: arg(&args, "nom").unwrap_or(&texte).to_string(),
                        texte,
                    },
                    "procedure" => crate::memoire::Intention::Procedure {
                        nom: arg(&args, "nom").unwrap_or("recette-sans-nom").to_string(),
                        description: texte,
                        corps: arg(&args, "corps").unwrap_or("").to_string(),
                    },
                    "etat-self" => crate::memoire::Intention::EtatSelf {
                        epithet: arg(&args, "nom").unwrap_or("").to_string(),
                        objectif: texte,
                        prochaine_action: arg(&args, "prochaine_action").unwrap_or("").to_string(),
                        note: format!("journalisé depuis {}", ancrage.monde.clone().unwrap_or_default()),
                    },
                    autre => {
                        return echec(
                            format!(
                                "genre inconnu : « {autre} » — attendu objectif, procedure ou etat-self"
                            ),
                            t.elapsed().as_millis() as u64,
                        );
                    }
                }
            }
            _ => unreachable!("appareil_ecriture ne couvre que les deux outils d'écriture"),
        };
        let genre = intention.genre();
        let reference = intention.reference();
        return match crate::memoire::enregistrer(&ancrage.self_qn, &provenance, intention) {
            Ok(e) => Resultat {
                texte: format!(
                    "MÉMORISÉ ({genre}) pour {} : « {reference} »\nCe n'est pas encore dans la \
                     base : l'écriture est JOURNALISÉE, puis appliquée par une passe qui ouvre \
                     la base du Self en exclusif (`obrain memoire --appliquer`). L'utilisateur \
                     peut voir ce qui est en attente dans le chat.",
                    ancrage.self_qn
                ),
                monde: ancrage.monde.clone().unwrap_or_default(),
                lignes: 1,
                ms: t.elapsed().as_millis() as u64,
                tronque: false,
                erreur: None,
                memoire: Some(e),
            },
            Err(e) => echec(
                format!("journalisation impossible : {e}"),
                t.elapsed().as_millis() as u64,
            ),
        };
    }

    match appel.nom.as_str() {
        "obrain_query" => {
            let Some(brute) = arg(&args, "query") else {
                return echec("`query` est requis".into(), t.elapsed().as_millis() as u64);
            };
            if let Some(mot) = est_lecture_seule(brute) {
                return echec(
                    format!(
                        "écriture refusée (`{mot}`) : cette base est ouverte EN LECTURE SEULE. \
                         N'utilise que MATCH / RETURN."
                    ),
                    t.elapsed().as_millis() as u64,
                );
            }
            // La borne d'abord (elle est sémantiquement neutre), puis le label
            // déplacé dans le motif : c'est ce qui fait passer la requête du
            // modèle de 68,6 ms à 2,4 ms (voir `label_dans_le_motif`).
            let gql = label_dans_le_motif(&borner(brute));
            let mut lignes: Vec<String> = Vec::new();
            let refus = match lire(db, &gql) {
                Ok(rows) => {
                    for row in rows.into_iter().take(max_lignes()) {
                        lignes.push(
                            row.iter()
                                .map(|v| txt(Some(v)))
                                .collect::<Vec<_>>()
                                .join(" | "),
                        );
                    }
                    None
                }
                Err(e) => Some(e),
            };
            if let Some(e) = refus {
                return echec_moteur(e, &gql, t.elapsed().as_millis() as u64);
            }
            let mut r = rendre_sur(lignes, t.elapsed().as_millis() as u64, &nom_base);
            r.texte = format!("[{nom_base}] {}\n{}", gql, r.texte);
            r
        }
        "obrain_echantillon" => {
            let Some(label) = arg(&args, "type") else {
                return echec("`type` est requis".into(), t.elapsed().as_millis() as u64);
            };
            if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return echec(format!("type invalide : « {label} »"), t.elapsed().as_millis() as u64);
            }
            let n = args
                .get("n")
                .and_then(|x| x.as_i64())
                .unwrap_or(12)
                .clamp(1, max_lignes() as i64);
            let gql = format!("MATCH (n:{label}) RETURN n.qualified_name, n.file_path LIMIT {n}");
            let rows = match lire(db, &gql) {
                Ok(r) => r,
                Err(e) => return echec_moteur(e, &gql, t.elapsed().as_millis() as u64),
            };
            let lignes = rows
                .into_iter()
                .map(|row| {
                    let qn = txt(row.first());
                    match row.get(1) {
                        Some(OV::String(f)) if !f.is_empty() && f.as_str() != "{}" => format!("{qn} ({f})"),
                        _ => qn,
                    }
                })
                .collect();
            let mut r = rendre_sur(lignes, t.elapsed().as_millis() as u64, &nom_base);
            r.texte = format!("[{nom_base}] échantillon {label} :\n{}", r.texte);
            r
        }
        "obrain_voisinage" => {
            let Some(qn) = arg(&args, "qn") else {
                return echec("`qn` est requis".into(), t.elapsed().as_millis() as u64);
            };
            if qn.contains(['\'', '"', '\\']) {
                return echec("qualified_name invalide".into(), t.elapsed().as_millis() as u64);
            }
            // Deux directions, bornées : les sortantes puis les entrantes.
            let gql_sortantes = format!(
                "MATCH (a {{qualified_name: '{qn}'}})-[r]->(b) \
                 RETURN a.qualified_name, type(r), b.qualified_name LIMIT {}",
                    max_lignes()
            );
            let gql_entrantes = format!(
                "MATCH (a)-[r]->(b {{qualified_name: '{qn}'}}) \
                 RETURN a.qualified_name, type(r), b.qualified_name LIMIT {}",
                    max_lignes()
            );
            let sortantes = match lire(db, &gql_sortantes) {
                Ok(r) => r,
                Err(e) => return echec_moteur(e, &gql_sortantes, t.elapsed().as_millis() as u64),
            };
            let entrantes = match lire(db, &gql_entrantes) {
                Ok(r) => r,
                Err(e) => return echec_moteur(e, &gql_entrantes, t.elapsed().as_millis() as u64),
            };
            let lignes = sortantes
                .into_iter()
                .map(|row| format!("{} -[{}]-> {}", txt(row.first()), txt(row.get(1)), txt(row.get(2))))
                .chain(
                    entrantes
                        .into_iter()
                        .map(|row| {
                            format!("{} -[{}]-> {}", txt(row.first()), txt(row.get(1)), txt(row.get(2)))
                        }),
                )
                .collect();
            let mut r = rendre_sur(lignes, t.elapsed().as_millis() as u64, &nom_base);
            r.texte = format!("[{nom_base}] voisinage de {qn} :\n{}", r.texte);
            r
        }
        "obrain_recherche" => {
            let Some(q) = arg(&args, "q") else {
                return echec("`q` est requis".into(), t.elapsed().as_millis() as u64);
            };
            let k = args
                .get("k")
                .and_then(|x| x.as_i64())
                .unwrap_or(20)
                .clamp(1, max_lignes() as i64) as usize;
            recherche(db, &nom_base, q, arg(&args, "label"), k, &t)
        }
        "obrain_mondes" => {
            let cible = arg(&args, "monde").and_then(|d| atlas.trouver(d)).map(|m| m.qn.clone());
            let mut lignes: Vec<String> = Vec::new();
            // L'ATLAS EST-IL COMPLET ? Sans la base synaptique il n'a ni
            // volumétrie ni fibres, et le rendu disait « NON TISSÉ » pour tout le
            // monde — ce qui se lit comme un fait et non comme une panne de
            // lecture. On le dit AVANT la liste.
            if !atlas.base_synaptique_lue {
                lignes.push(
                    "⚠ ATLAS AMPUTÉ — la base synaptique n'a pas pu être lue : aucune \
                     volumétrie, aucune fibre, aucun lien ne sont disponibles. Les mentions \
                     « NON TISSÉ » ci-dessous ne veulent donc PAS dire qu'un monde n'est pas \
                     tissé : elles disent que l'atlas n'a rien pu lire. Relancez le service \
                     pour retenter."
                        .to_string(),
                );
            }
            for m in &atlas.mondes {
                if let Some(c) = &cible
                    && &m.qn != c
                {
                    continue;
                }
                // ── LA VOLUMÉTRIE EST-ELLE VÉRIFIÉE, OU SEULEMENT DÉCLARÉE ? ──
                // Un chiffre sans sa provenance n'est pas vérifiable : « 1 553 071
                // nœuds » ne dit pas si on les a lus ou si on répète un compteur.
                // Chaque monde porte donc son état de lisibilité, EXPLICITE.
                let vol = match (m.nodes, m.edges, m.lisible) {
                    // Le compteur annonce et RIEN ne se lit : le dire AVANT le chiffre.
                    (Some(n), Some(e), Some(false)) => format!(
                        "⚠ INCOMPLET — le compteur annonce {n} nœuds et {e} arêtes, mais AUCUN \
                         enregistrement n'est lisible : ce monde n'a plus de données. Ne pas \
                         s'appuyer sur ses chiffres."
                    ),
                    (Some(n), Some(e), Some(true)) => {
                        format!("{n} nœuds · {e} arêtes · volumétrie VÉRIFIÉE par lecture")
                    }
                    (Some(n), Some(e), None) => format!(
                        "{n} nœuds · {e} arêtes · volumétrie NON VÉRIFIÉE (compteur du tissage, \
                         store non sondé)"
                    ),
                    // « non tissé » est une information, pas un zéro trompeur.
                    _ => "NON TISSÉ (volumétrie non calculée)".to_string(),
                };
                lignes.push(format!("{} [{}] {} — {}", m.qn, m.nature, m.label, vol));
            }
            if cible.is_none() {
                lignes.push(String::new());
                lignes.push("SYNAPSES (preuves réelles) :".to_string());
                for s in &atlas.synapses {
                    lignes.push(format!(
                        "{} <-> {} · poids {} · entités partagées {} · sessions vécues {} · \
                         flux {} · datées {}",
                        s.a, s.b, s.poids, s.shared, s.bridged, s.flows, s.dated
                    ));
                    if !s.cles.is_empty() {
                        lignes.push(format!(
                            "    clés partagées (à interroger dans l'autre monde) : {}",
                            s.cles.join(", ")
                        ));
                    }
                }
            } else if let Some(c) = &cible {
                for s in atlas.fibres(c) {
                    let autre = if &s.a == c { &s.b } else { &s.a };
                    lignes.push(format!(
                        "→ {autre} · poids {} · partagées {} · vécu {} · flux {} · datées {}",
                        s.poids, s.shared, s.bridged, s.flows, s.dated
                    ));
                    if !s.cles.is_empty() {
                        lignes.push(format!("    clés : {}", s.cles.join(", ")));
                    }
                }
            }
            // ── CE N'EST PAS LE MONDE ANCRÉ QUI A ÉTÉ LU ────────────────────
            // `obrain_mondes` ne lit AUCUNE base de monde : il lit l'ATLAS, dans
            // la base synaptique. Annoncer « le monde ancré » (mesuré :
            // `"monde":"le monde ancré"` pour un appel qui n'a rien lu du store
            // ancré) faisait dire à l'écran — dont le contrat est « le monde
            // réellement interrogé » — une chose qui n'avait pas eu lieu.
            let mut r = rendre_sur(lignes, t.elapsed().as_millis() as u64, &nom_base);
            r.monde = "l'atlas (base synaptique — aucun store de monde lu)".to_string();
            r
        }
        "obrain_hubs" => {
            // Le classement est CALCULÉ et PERSISTÉ par l'entretien (16 ms sur
            // 401 k nœuds, par registres) : on le relit, on ne le refait pas.
            let n = args
                .get("n")
                .and_then(|x| x.as_i64())
                .unwrap_or(20)
                .clamp(1, crate::hubs::HUBS_PAR_DEFAUT as i64) as usize;
            // ── SANS `monde`, LE MONDE ANCRÉ EST CONNU — ON NE LE CHERCHE PAS ──
            // Défaut mesuré le 2026-09-13 (chat du port 8432, ancrage sur le
            // GRAPHE GLOBAL) : `obrain_hubs{"n":30}` échouait en « le monde visé
            // n'a pas pu être résolu » alors que le store réellement lu était
            // connu (`ancrage.store`) et que son classement est persisté PAR
            // STORE par l'entretien. L'outil se privait donc d'une information
            // qu'il détenait, et le modèle recevait une erreur au lieu du cœur du
            // graphe. On retombe sur le monde ancré — résolu par son CHEMIN, pas
            // par son nom (c'est le store ouvert qui fait foi).
            let ancre = monde.or_else(|| {
                atlas
                    .mondes
                    .iter()
                    .find(|m| m.path.to_string_lossy() == ancrage.store)
            });
            // Le classement est celui du monde VISÉ — pas celui du monde ancré.
            // Défaut corrigé au passage : demander les hubs d'un AUTRE monde
            // relisait le classement du store ancré et l'étiquetait du nom du
            // monde demandé — un classement juste, sous un nom faux.
            let cible: std::path::PathBuf = ancre
                .map(|m| m.path.clone())
                .unwrap_or_else(|| std::path::PathBuf::from(&ancrage.store));
            let etiquette = ancre
                .map(|m| m.label.clone())
                .unwrap_or_else(|| {
                    cible
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| "le monde ancré".to_string())
                });
            let classement = crate::hubs::lire(&cible, crate::viz_serve::store_mtime(&cible));
            match classement {
                Some(c) if c.refus.is_some() => echec(
                    format!(
                        "classement par degré REFUSÉ pour « {etiquette} » : {}",
                        c.refus.unwrap_or_default()
                    ),
                    t.elapsed().as_millis() as u64,
                ),
                Some(c) => {
                    let r = rendre_sur(
                        crate::hubs::lignes(&c.hubs, n),
                        t.elapsed().as_millis() as u64,
                        &etiquette,
                    );
                    Resultat {
                        texte: format!(
                            "[{etiquette}] {} nœuds les plus connectés (classement calculé en {} ms par {}, \
                             relu ici) :\n{}",
                            c.hubs.len(),
                            c.calcul_ms,
                            c.source,
                            r.texte
                        ),
                        ..r
                    }
                }
                None => echec(
                    format!(
                        "classement des hubs non disponible pour « {etiquette} » : l'entretien ne l'a \
                         pas encore produit (ou il est périmé)."
                    ),
                    t.elapsed().as_millis() as u64,
                ),
            }
        }
        "obrain_traverser" => {
            // ── SUIVRE UNE SYNAPSE ──────────────────────────────────────────
            // Une fibre dit qu'un lien EXISTE ; elle ne dit pas ce que la chose
            // partagée DEVIENT dans l'autre graphe. C'est pourtant la question
            // utile : « session-77370046 » est un enregistrement dans
            // `runtime-dev` et une origine dans `self-demo`.
            //
            // Deux sources, dans l'ordre de force : la base synaptique (les
            // `Anchor` et leurs `IN_WORLD` — la vérité complète du tissage), puis
            // les échantillons de clés des fibres (ce que la toile a gardé).
            let Some(cle) = arg(&args, "cle") else {
                return echec("`cle` est requis".into(), t.elapsed().as_millis() as u64);
            };
            if cle.contains(['\'', '"', '\\']) {
                return echec("clé invalide".into(), t.elapsed().as_millis() as u64);
            }
            let depart = arg(&args, "monde")
                .and_then(|d| atlas.trouver(d))
                .map(|m| m.qn.clone())
                .or_else(|| ancrage.monde.clone());

            let mut mondes: Vec<String> = Vec::new();
            let mut source = "";
            if let Some(syn) = db_synapse {
                let q = format!(
                    "MATCH (a:Anchor {{qn: '{cle}'}})-[:IN_WORLD]->(w:World) RETURN w.qn LIMIT 12"
                );
                for row in lire(syn, &q).unwrap_or_default() {
                    let q = txt(row.first());
                    if !q.is_empty() && q != "null" {
                        mondes.push(q);
                    }
                }
                if !mondes.is_empty() {
                    source = "base synaptique (Anchor → IN_WORLD)";
                }
            }
            if mondes.is_empty() {
                // Repli : les échantillons de clés que les fibres portent déjà.
                for s in &atlas.synapses {
                    if s.cles.iter().any(|c| c == cle) {
                        mondes.push(s.a.clone());
                        mondes.push(s.b.clone());
                    }
                }
                mondes.sort();
                mondes.dedup();
                if !mondes.is_empty() {
                    source = "échantillons de clés des fibres (la base synaptique n'est pas ouverte)";
                }
            }

            let mut lignes: Vec<String> = Vec::new();
            if mondes.is_empty() {
                lignes.push(format!(
                    "Aucun lien connu pour « {cle} » : elle n'apparaît ni comme ancre tissée, \
                     ni dans les échantillons des fibres."
                ));
            } else {
                lignes.push(format!(
                    "« {cle} » vit dans {} monde(s) — source : {source}",
                    mondes.len()
                ));
                for qn in &mondes {
                    let nom = atlas
                        .mondes
                        .iter()
                        .find(|m| &m.qn == qn)
                        .map(|m| m.label.clone())
                        .unwrap_or_else(|| qn.clone());
                    let ici = depart.as_deref() == Some(qn.as_str());
                    lignes.push(format!("\n── {nom}{} ──", if ici { " (le monde ancré)" } else { "" }));
                    match db_du_monde(qn) {
                        Some(base) => {
                            // Ce que la clé EST là-bas : le nœud, puis ses voisins.
                            let q_noeud = format!(
                                "MATCH (n {{qualified_name: '{cle}'}})                                  RETURN labels(n)[0], n.name, n.file_path LIMIT 1"
                            );
                            let noeud = lire(base.as_ref(), &q_noeud).unwrap_or_default();
                            match noeud.first() {
                                Some(r) => lignes.push(format!(
                                    "  nœud : [{}] {} {}",
                                    txt(r.first()),
                                    txt(r.get(1)),
                                    txt(r.get(2))
                                )),
                                None => lignes.push(
                                    "  pas de nœud portant ce qualified_name dans cette base \
                                     (la clé y est citée sans y être définie)"
                                        .to_string(),
                                ),
                            }
                            let q_voisins = format!(
                                "MATCH (a {{qualified_name: '{cle}'}})-[r]->(b)                                  RETURN type(r), b.qualified_name LIMIT 5"
                            );
                            for row in lire(base.as_ref(), &q_voisins).unwrap_or_default() {
                                lignes.push(format!("  → {} {}", txt(row.first()), txt(row.get(1))));
                            }
                        }
                        // Une base non ouverte n'est pas une absence : on le DIT.
                        None => lignes.push(
                            "  base non ouverte par le service — le lien est connu, le contenu \
                             de ce monde n'a pas pu être lu"
                                .to_string(),
                        ),
                    }
                }
            }
            rendre_sur(lignes, t.elapsed().as_millis() as u64, &nom_base)
        }
        "obrain_recettes" => {
            // ── LE RÉFÉRENTIEL EST CELUI DE L'AGENT, PAS DU GRAPHE REGARDÉ ───
            // Mesuré : 29 recettes `proc:ref*` dans `self-demo`, ZÉRO dans
            // runtime-dev et runtime-prod. Un premier essai se contentait de
            // rendre une erreur invitant le modèle à réessayer avec `monde` —
            // défaut constaté : le schéma de l'outil n'acceptait PAS `monde`, et
            // le modèle a rappelé le même outil trois fois de suite, sans issue.
            //
            // Chercher là où vit le référentiel est la seule façon qu'un outil le
            // trouve sans que l'utilisateur ait à savoir où il habite. Le monde
            // réellement lu est DIT dans le résultat.
            let requete = "MATCH (p:Procedure) WHERE p.qualified_name STARTS WITH 'proc:ref' \
                           RETURN p.family, p.name, p.description LIMIT 31";
            let mut cibles: Vec<(String, String)> = Vec::new();
            if autre.is_some() {
                // `monde` a été donné : on ne cherche QUE là.
                cibles.push((nom_base.clone(), String::new()));
            } else {
                cibles.push((nom_base.clone(), String::new()));
                for qn in mondes_du_self(atlas) {
                    if let Some(m) = atlas.mondes.iter().find(|m| m.qn == qn)
                        && m.path.to_string_lossy() != ancrage.store
                    {
                        cibles.push((m.label.clone(), qn));
                    }
                }
            }
            let mut trouve: Option<Resultat> = None;
            // Un REFUS du moteur n'est pas « pas trouvé dans ce monde » : on le
            // retient, on continue de chercher ailleurs, et s'il n'y a rien nulle
            // part, c'est le REFUS qu'on rend — pas « aucune recette trouvée ».
            let mut refus: Option<String> = None;
            for (nom, qn) in &cibles {
                let base: Option<std::sync::Arc<ObrainDB>> = if qn.is_empty() {
                    None
                } else {
                    db_du_monde(qn)
                };
                let session_db: &ObrainDB = base.as_deref().unwrap_or(db_ancre);
                let v = match lire(session_db, requete) {
                    Ok(v) => v,
                    Err(e) => {
                        refus = Some(e);
                        continue;
                    }
                };
                if v.is_empty() {
                    continue;
                }
                let lignes: Vec<String> = v
                    .into_iter()
                    .map(|row| {
                        format!("[{}] {} — {}", txt(row.first()), txt(row.get(1)), txt(row.get(2)))
                    })
                    .collect();
                let r = rendre_sur(lignes, t.elapsed().as_millis() as u64, nom);
                trouve = Some(Resultat {
                    texte: format!("[{nom}] référentiel de l'agent :\n{}", r.texte),
                    ..r
                });
                break;
            }
            trouve.unwrap_or_else(|| {
                if let Some(e) = refus {
                    return echec_moteur(e, requete, t.elapsed().as_millis() as u64);
                }
                echec(
                    format!(
                        "aucune recette `proc:ref*` trouvée — ni dans « {nom_base} », ni dans \
                         les mondes du self ({})",
                        mondes_du_self(atlas).join(", ")
                    ),
                    t.elapsed().as_millis() as u64,
                )
            })
        }
        "obrain_recette" => {
            let Some(nom) = arg(&args, "nom") else {
                return echec("`nom` est requis".into(), t.elapsed().as_millis() as u64);
            };
            // Le nom peut être court (`code.hot`) ou complet (`proc:ref.code.hot`).
            let complet = if nom.starts_with("proc:") { nom.to_string() } else { format!("proc:ref.{nom}") };
            let gql = format!(
                "MATCH (p:Procedure {{qualified_name: '{complet}'}}) \
                 RETURN p.name, p.family, p.params, p.returns, p.body LIMIT 1"
            );
            // Même découverte que `obrain_recettes` : la recette appartient au
            // référentiel de l'agent, qui ne vit pas dans le graphe regardé.
            let mut cibles: Vec<(String, String)> = vec![(nom_base.clone(), String::new())];
            if autre.is_none() {
                for qn in mondes_du_self(atlas) {
                    if let Some(m) = atlas.mondes.iter().find(|m| m.qn == qn)
                        && m.path.to_string_lossy() != ancrage.store
                    {
                        cibles.push((m.label.clone(), qn));
                    }
                }
            }
            let mut brut: Vec<Vec<OV>> = Vec::new();
            let mut ou: String = nom_base.clone();
            let mut refus: Option<String> = None;
            for (nom_monde, qn) in &cibles {
                let base: Option<std::sync::Arc<ObrainDB>> =
                    if qn.is_empty() { None } else { db_du_monde(qn) };
                match lire(base.as_deref().unwrap_or(db_ancre), &gql) {
                    Ok(v) => {
                        brut = v;
                        if !brut.is_empty() {
                            ou = nom_monde.clone();
                            break;
                        }
                    }
                    Err(e) => refus = Some(e),
                }
            }
            // Le refus est dit, même quand aucun monde n'a répondu : sinon
            // « recette absente » accuserait le contenu alors que c'est la
            // requête qui a été rejetée.
            if brut.is_empty()
                && let Some(e) = refus
            {
                return echec_moteur(e, &gql, t.elapsed().as_millis() as u64);
            }
            let lignes: Vec<String> = brut
                .into_iter()
                .map(|row| {
                    format!(
                        "recette {} [{}]\nparamètres : {}\nrenvoie : {}\nCORPS GQL :\n{}",
                        txt(row.first()),
                        txt(row.get(1)),
                        txt(row.get(2)),
                        txt(row.get(3)),
                        txt(row.get(4))
                    )
                })
                .collect();
            match rendre_sur(lignes, t.elapsed().as_millis() as u64, &ou) {
                r if r.lignes > 0 => Resultat { texte: format!("[{ou}] {}", r.texte), ..r },
                r => Resultat {
                    erreur: Some(format!("recette inconnue : {complet}")),
                    texte: format!(
                        "ERREUR : recette inconnue : {complet}, ni dans « {nom_base} » ni dans \
                         les mondes du self ({}). Appelez obrain_recettes pour la liste.",
                        mondes_du_self(atlas).join(", ")
                    ),
                    ..r
                },
            }
        }
        autre => echec(format!("outil inconnu : {autre}"), t.elapsed().as_millis() as u64),
    }
}

/// RECHERCHE COUPLÉE (chantier RECHERCHE-COUPLEE, 2026-09-13) — le moteur
/// savait chercher, le chat ne pouvait pas lui demander.
///
/// Le défaut mesuré : 7 requêtes `CONTAINS` aveugles du chat pour trouver
/// « zerocopy » — 0 résultat rendu alors que 60 nœuds existent. `CONTAINS`
/// exige de connaître la forme exacte ; BM25 trouve par mots-clés.
///
/// ## La cascade, telle qu'elle est réellement implémentée
///
/// 1. **Sources BM25** : un index texte par couple (label, propriété) — les
///    index DÉCLARÉS par le store (genre "text"), plus les couples éprouvés
///    par le bridge (`search_code`, mcp_server.rs) : `<label>.text` et
///    `<label>.name` (Function par défaut). Construction À LA DEMANDE par
///    `create_text_index`, le même geste que le bridge — sur un lecteur RO,
///    la déclaration non persistée est un simple warn (index.rs:577).
/// 2. **Fusion RRF** (`fuse_results`, k=60 — le défaut de `FusionMethod`) :
///    les scores BM25 de deux index ne sont PAS comparables entre eux, la
///    fusion par rang l'est. C'est la même primitive que `hybrid_search`.
/// 3. **Vecteurs** : `ObrainDB::hybrid_search` est compilé (feature
///    `hybrid-search` câblée dans Cargo.toml), mais fusionner une source
///    vectorielle exige un VECTEUR DE REQUÊTE, et ce binaire n'embarque pas
///    d'embedder (la feature `embed` du moteur — ONNX/ort + téléchargement
///    HF — n'a jamais été câblée au CLI). Un index vectoriel présent est
///    donc ANNONCÉ dans la sortie, pas fusionné — pas de simulacre.
/// 4. **Repli honnête** : aucun couple exploitable → le dire, avec les refus
///    exacts et le geste qui débloque (donner un `label` dont les nœuds
///    portent du texte — `obrain_echantillon` montre les types).
///
/// La requête `q` n'entre dans AUCUNE chaîne GQL : elle va au tokenizer BM25
/// tel quel — pas de surface d'injection, contrairement à `obrain_query`.
fn recherche(
    db: &ObrainDB,
    nom_base: &str,
    q: &str,
    label: Option<&str>,
    k: usize,
    t: &Instant,
) -> Resultat {
    use obrain_core::index::text::{FusionMethod, fuse_results};

    // Les couples (label, propriété) candidats : le déclaré d'abord, puis les
    // couples du bridge. Borné — chaque couple absent coûte une construction
    // d'index (rescan du label, ~1,6 s mesurés sur 50 k nœuds côté bridge).
    let declarations = db
        .substrate_handle()
        .map(|s| s.declarations_index())
        .unwrap_or_default();
    let mut couples: Vec<(String, String)> = declarations
        .iter()
        .filter(|d| d.genre == "text")
        .filter(|d| label.is_none_or(|l| d.label == l))
        .map(|d| (d.label.clone(), d.propriete.clone()))
        .collect();
    let defauts: &[(&str, &str)] = match label {
        Some(l) => &[(l, "text"), (l, "name")],
        None => &[("Function", "text"), ("Function", "name")],
    };
    for (l, p) in defauts {
        if !couples.iter().any(|(cl, cp)| cl == l && cp == p) {
            couples.push(((*l).to_string(), (*p).to_string()));
        }
    }
    couples.truncate(8);

    // Une source par couple. Un couple sans nœuds ou sans texte échoue SANS
    // faire tomber les autres — mais l'échec est retenu, pour être DIT.
    let sur_echantillon = k.saturating_mul(2);
    let mut sources: Vec<Vec<(obrain_common::types::NodeId, f64)>> = Vec::new();
    let mut servies: Vec<String> = Vec::new();
    let mut refus: Vec<String> = Vec::new();
    for (l, p) in &couples {
        if !db.has_text_index(l, p)
            && let Err(e) = db.create_text_index(l, p)
        {
            refus.push(format!("{l}.{p} ({e})"));
            continue;
        }
        match db.text_search(l, p, q, sur_echantillon) {
            Ok(r) => {
                servies.push(format!("{l}.{p}"));
                if !r.is_empty() {
                    sources.push(r);
                }
            }
            Err(e) => refus.push(format!("{l}.{p} ({e})")),
        }
    }

    if servies.is_empty() {
        return echec(
            format!(
                "aucun index texte exploitable sur ce store — essayé : {}. Donne un `label` \
                 dont les nœuds portent du texte (name, text…) : l'index se construit alors \
                 à la demande. obrain_echantillon montre les types disponibles.",
                refus.join(" ; ")
            ),
            t.elapsed().as_millis() as u64,
        );
    }

    // Fusion par rang (RRF k=60). Une seule source : scores BM25 bruts.
    let fusionne = sources.len() > 1;
    let resultats = fuse_results(&sources, &FusionMethod::default(), k);

    // NodeId → qualified_name : le nom que TOUS les autres outils prennent.
    let session = db.session();
    let lignes: Vec<String> = resultats
        .into_iter()
        .map(|(nid, score)| {
            let props = session.get_node(nid).map(|n| n.properties).unwrap_or_default();
            let cherche = |cle: &str| {
                props.iter().find(|(pk, _)| pk.as_str() == cle).and_then(|(_, v)| match v {
                    OV::String(s) => Some(s.to_string()),
                    _ => None,
                })
            };
            let qn = cherche("qualified_name")
                .or_else(|| cherche("name"))
                .unwrap_or_else(|| format!("nœud #{}", nid.0));
            format!("{qn} · {score:.4} · bm25")
        })
        .collect();

    let vecteurs: Vec<String> = declarations
        .iter()
        .filter(|d| d.genre == "vector")
        .map(|d| format!("{}.{}", d.label, d.propriete))
        .collect();
    let note_vecteurs = if vecteurs.is_empty() {
        String::new()
    } else {
        format!(
            "\n[index vectoriel présent ({}) mais NON fusionné : ce binaire n'embarque pas \
             d'embedder de requête — BM25 seul]",
            vecteurs.join(", ")
        )
    };
    let mode = if fusionne { " · fusion RRF k=60" } else { " (scores BM25 bruts)" };
    let mut r = rendre_sur(lignes, t.elapsed().as_millis() as u64, nom_base);
    r.texte = format!(
        "[{nom_base}] recherche « {q} » — BM25 sur {}{mode}{note_vecteurs}\n{}",
        servies.join(" + "),
        r.texte
    );
    r
}

/// La requête est-elle sûre ? (contrôle exposé pour les tests et le serveur)
pub fn controle(gql: &str) -> Result<String, String> {
    if let Some(mot) = est_lecture_seule(gql) {
        return Err(format!("écriture refusée (`{mot}`) : base en lecture seule"));
    }
    Ok(borner(gql))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecriture_refusee() {
        assert!(est_lecture_seule("MATCH (n) RETURN n").is_none());
        assert_eq!(est_lecture_seule("CREATE (n:X) RETURN n"), Some("CREATE"));
        // Le mot signalé dépend de l'ordre de la liste (DELETE ou DETACH) :
        // l'important est qu'une écriture soit REFUSÉE, pas lequel est nommé.
        assert!(est_lecture_seule("MATCH (n) DETACH DELETE n").is_some());
        // Le contrôle se fait sur des MOTS entiers : « SET » ne doit pas être
        // trouvé dans un identifiant qui le contient.
        assert!(est_lecture_seule("MATCH (n:Settings) RETURN n").is_none());
        assert!(est_lecture_seule("MATCH (n:Offsets) RETURN n").is_none());
    }

    /// Une requête sans `LIMIT` en reçoit un : c'est ce qui empêche qu'un
    /// `MATCH (n)--(m) …` (14,1 s mesurés) parte sans borne.
    #[test]
    fn limit_impose() {
        // Sans LIMIT : la borne est ajoutée (la valeur vient de la constante).
        assert_eq!(
            borner("MATCH (n) RETURN n"),
            format!("MATCH (n) RETURN n LIMIT {LIMITE_IMPOSEE}")
        );
        // Le point-virgule final ne casse pas la requête.
        assert_eq!(
            borner("MATCH (n) RETURN n;"),
            format!("MATCH (n) RETURN n LIMIT {LIMITE_IMPOSEE}")
        );
        // Un LIMIT sous le plafond est respecté tel quel.
        assert_eq!(borner("MATCH (n) RETURN n LIMIT 5"), "MATCH (n) RETURN n LIMIT 5");
    }

    /// ── LES TROIS TROUS QUE LA SOUS-CHAÎNE LAISSAIT PASSER ────────────────
    /// Chaque cas correspond à un défaut mesuré de l'ancienne implémentation
    /// (`haut.contains("LIMIT")`) : une borne annoncée mais absente.
    #[test]
    fn la_borne_est_vraie_meme_dans_ces_cas() {
        // 1. Le mot dans un LITTÉRAL n'est pas un LIMIT.
        assert_eq!(
            borner("MATCH (n:Function) WHERE n.name CONTAINS 'limit' RETURN n"),
            format!("MATCH (n:Function) WHERE n.name CONTAINS 'limit' RETURN n LIMIT {LIMITE_IMPOSEE}")
        );
        // 2. Un LIMIT démesuré du modèle est RAMENÉ au plafond.
        assert_eq!(
            borner("MATCH (n) RETURN n LIMIT 1000000"),
            format!("MATCH (n) RETURN n LIMIT {LIMITE_IMPOSEE}")
        );
        // 3. La borne n'est pas avalée par un commentaire de fin.
        assert_eq!(
            borner("MATCH (n) RETURN n // toute la base"),
            format!("MATCH (n) RETURN n LIMIT {LIMITE_IMPOSEE}\n// toute la base")
        );
        // Un nom de propriété contenant « limit » ne compte pas non plus.
        assert_eq!(
            borner("MATCH (n) RETURN n.limite"),
            format!("MATCH (n) RETURN n.limite LIMIT {LIMITE_IMPOSEE}")
        );
    }

    /// Un résultat coupé doit le DIRE : un modèle qui reçoit une liste tronquée
    /// sans le savoir conclut sur une liste incomplète.
    #[test]
    fn coupe_annoncee() {
        // Chaque ligne est PLUS LONGUE que la moyenne (200 caractères pour ~90
        // estimés par le plafond de lignes) : le total dépasse donc le plafond de
        // caractères QUELLE QUE SOIT la fenêtre. La version précédente calibrait
        // ses lignes sur l'ancien plafond de 2 000 caractères — elle est tombée
        // dès que la fenêtre découverte a porté le plafond à 8 000.
        let lignes: Vec<String> = (0..max_lignes())
            .map(|i| format!("ligne {i} {}", "x".repeat(200)))
            .collect();
        let r = rendre(lignes, 0);
        assert!(r.tronque, "le plafond doit être atteint");
        assert!(r.texte.contains("COUPÉ"), "la coupe doit être écrite : {}", r.texte);
        assert!(r.texte.len() <= max_caracteres() + 200);
    }

    #[test]
    fn resultat_vide_dit_vide() {
        let r = rendre(vec![], 0);
        assert_eq!(r.texte.trim(), "(aucune ligne)");
        assert!(!r.tronque);
    }

    /// ── LE LABEL PASSE DANS LE MOTIF, ET RIEN N'EST RETIRÉ ─────────────────
    /// C'est la mesure qui commande : `labels(n) CONTAINS 'Class'` en prédicat
    /// coûte 68,6 ms (balayage du graphe entier) contre 2,4 ms avec le label
    /// dans le motif. Le prédicat d'origine est LAISSÉ : il devient redondant,
    /// donc la réponse ne peut pas changer.
    #[test]
    fn le_label_passe_dans_le_motif_sans_rien_retirer() {
        let q = "MATCH (n) WHERE n.qualified_name STARTS WITH 'home' AND labels(n) CONTAINS 'Class' RETURN n.qualified_name LIMIT 30";
        let r = label_dans_le_motif(q);
        assert!(r.starts_with("MATCH (n:Class)"), "le motif doit porter le label : {r}");
        assert!(r.contains("labels(n) CONTAINS 'Class'"), "le prédicat reste (redondant, donc sûr) : {r}");
        assert!(r.contains("LIMIT 30"), "la borne est intacte");
    }

    /// La seconde forme, `'Class' IN labels(n)`, est reconnue aussi.
    #[test]
    fn la_forme_in_est_reconnue() {
        let r = label_dans_le_motif("MATCH (n) WHERE 'Function' IN labels(n) RETURN n LIMIT 5");
        assert!(r.starts_with("MATCH (n:Function)"), "{r}");
    }

    /// ── CE QU'ON NE DOIT PAS FAIRE ──────────────────────────────────────────
    /// Un motif déjà étiqueté ne reçoit rien (`(n:Function)` + `:Class` serait une
    /// contradiction, donc une réponse fausse par excès de restriction), une
    /// requête sans prédicat de label n'est pas touchée, et une variable liée
    /// ailleurs (`(m)`) n'est pas confondue.
    #[test]
    fn la_traduction_ne_touche_pas_ce_qu_elle_ne_comprend_pas() {
        assert_eq!(
            label_dans_le_motif("MATCH (n:Function) WHERE labels(n) CONTAINS 'Class' RETURN n LIMIT 5"),
            "MATCH (n:Function) WHERE labels(n) CONTAINS 'Class' RETURN n LIMIT 5"
        );
        assert_eq!(label_dans_le_motif("MATCH (n) RETURN n LIMIT 5"), "MATCH (n) RETURN n LIMIT 5");
        assert_eq!(
            label_dans_le_motif("MATCH (m) WHERE labels(n) CONTAINS 'Class' RETURN m LIMIT 5"),
            "MATCH (m) WHERE labels(n) CONTAINS 'Class' RETURN m LIMIT 5"
        );
        // Un texte libre ne devient pas une requête : rien n'est inventé.
        assert_eq!(label_dans_le_motif(""), "");
    }

    /// ── LE MARKUP RÉELLEMENT OBSERVÉ, TEL QUEL ─────────────────────────────
    /// C'est la chaîne que le DGX a écrite dans le RAISONNEMENT du tour (chat du
    /// 8432, 2026-09-13) : elle partait à l'écran comme « raisonnement » et
    /// aucun outil n'était appelé. Le test la reprend mot pour mot.
    #[test]
    fn le_tool_call_ecrit_dans_le_texte_est_reconnu() {
        let texte = "<tool_call> <function=obrain_query> <parameter=query> MATCH (n) WHERE \
                     n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.' \
                     RETURN split(n.qualified_name, '.')[5] AS crate, count(*) AS n ORDER BY n \
                     DESC LIMIT 30 </parameter> </function> </tool_call>";
        let appels = appels_du_texte(texte);
        assert_eq!(appels.len(), 1, "un appel attendu : {appels:?}");
        assert_eq!(appels[0].nom, "obrain_query");
        let args: serde_json::Value = serde_json::from_str(&appels[0].arguments).unwrap();
        let q = args.get("query").and_then(|x| x.as_str()).unwrap();
        assert!(q.starts_with("MATCH (n) WHERE"), "requête non vide : {q}");
        assert!(q.ends_with("LIMIT 30"), "requête complète, sans balise : {q}");
        assert!(!q.contains("<parameter"), "la balise ne doit pas entrer dans la requête");
        assert!(!appels[0].id.is_empty(), "un identifiant est exigé pour le résultat d'outil");
    }

    /// DEUX appels dans un même bloc, et une seconde syntaxe (`name=`).
    #[test]
    fn plusieurs_appels_dans_un_texte() {
        let texte = "<tool_call>\
                     <function=obrain_hubs><parameter=n>30</parameter></function>\
                     <function=obrain_echantillon><parameter=type>Class</parameter></function>\
                     </tool_call>";
        let appels = appels_du_texte(texte);
        assert_eq!(appels.len(), 2, "{appels:?}");
        assert_eq!(appels[0].nom, "obrain_hubs");
        assert_eq!(appels[1].nom, "obrain_echantillon");
        assert_eq!(appels[1].arguments, r#"{"type":"Class"}"#);
        assert_ne!(appels[0].id, appels[1].id, "deux appels ne partagent pas d'id");
    }

    /// ── LA FERMETURE MANQUANTE N'ANNULE PAS L'APPEL ────────────────────────
    /// Un flux coupé en plein appel (c'est le cas réel : la coupure vient
    /// d'ailleurs) laisse `</function>` et `</tool_call>` absents. Refuser ce
    /// texte reviendrait à préférer une réponse vide à un appel récupéré.
    #[test]
    fn un_appel_sans_fermeture_est_quand_meme_rendu() {
        let appels = appels_du_texte(
            "<tool_call><function=obrain_query><parameter=query>MATCH (n) RETURN n LIMIT 5",
        );
        assert_eq!(appels.len(), 1, "{appels:?}");
        assert_eq!(appels[0].nom, "obrain_query");
        let args: serde_json::Value = serde_json::from_str(&appels[0].arguments).unwrap();
        assert_eq!(args.get("query").and_then(|x| x.as_str()).unwrap(), "MATCH (n) RETURN n LIMIT 5");
    }

    /// La syntaxe JSON dans le texte (certains services n'émettent que celle-là).
    #[test]
    fn le_tool_call_json_dans_le_texte_est_reconnu() {
        let appels = appels_du_texte(
            r#"<tool_call>{"name":"obrain_voisinage","arguments":{"qn":"world:runtime-dev"}}</tool_call>"#,
        );
        assert_eq!(appels.len(), 1, "{appels:?}");
        assert_eq!(appels[0].nom, "obrain_voisinage");
        assert_eq!(appels[0].arguments, r#"{"qn":"world:runtime-dev"}"#);
    }

    /// ── CE QUI N'EST PAS UN APPEL NE DEVIENT PAS UN APPEL ──────────────────
    /// La propriété inverse compte autant : une prose qui parle d'outils, ou un
    /// JSON d'illustration, ne doit déclencher AUCUNE exécution.
    #[test]
    fn la_prose_ne_declenche_aucun_appel() {
        assert!(appels_du_texte("La réponse est 42.").is_empty());
        assert!(appels_du_texte("Le paramètre est `n=30`, et la fonction est pure.").is_empty());
        assert!(appels_du_texte("").is_empty());
        // Un JSON sans `name` n'est pas un appel.
        assert!(appels_du_texte(r#"{"query":"MATCH (n) RETURN n"}"#).is_empty());
    }

    /// `obrain_recherche` de bout en bout — le bras de match, comme le serveur
    /// l'appellerait — sur un STORE RÉEL désigné par `OBRAIN_TEST_STORE`.
    ///
    /// Gouverné par l'environnement parce qu'un chemin de store en dur mentirait
    /// sur toute autre machine. Sans la variable, le test passe SANS RIEN
    /// AFFIRMER (dit sur stdout). Ouverture `open_read_only` : verrou PARTAGÉ,
    /// qui coexiste avec un viz ou un bridge tenant le même store.
    ///
    ///   OBRAIN_TEST_STORE=/tmp/linux-full3.obrain \
    ///     cargo test -p obrain-cli recherche_sur_store_reel -- --nocapture
    #[test]
    fn recherche_sur_store_reel() {
        let Some(store) = std::env::var_os("OBRAIN_TEST_STORE") else {
            println!("OBRAIN_TEST_STORE absent : test sauté (aucune assertion).");
            return;
        };
        let store = std::path::PathBuf::from(store);
        let q = std::env::var("OBRAIN_TEST_Q").unwrap_or_else(|_| "zerocopy".into());
        let db = ObrainDB::open_read_only(&store).expect("ouverture lecture seule");
        let ancrage = crate::engine::Ancrage {
            store: store.to_string_lossy().into_owned(),
            monde: None,
            self_qn: "self:test".into(),
        };
        // `base_synaptique_lue: false` : ce test ne lit AUCUNE synapse, et
        // l'atlas vide le dit. Ce champ a ete ajoute a `Atlas` sans mettre a
        // jour cet initialiseur, ce qui rendait TOUT le banc de test
        // d'`obrain-cli` incollable (E0063) — y compris les tests de
        // `viz_extract` dont depend la verification du sous-graphe induit.
        let atlas = Atlas {
            mondes: vec![],
            synapses: vec![],
            base_synaptique_lue: false,
        };
        let appel = Appel {
            id: "t-recherche".into(),
            nom: "obrain_recherche".into(),
            arguments: serde_json::json!({ "q": q }).to_string(),
        };
        let aucun_monde = |_: &str| None::<std::sync::Arc<ObrainDB>>;
        let r = executer(&db, &aucun_monde, None, &ancrage, &atlas, &appel);
        println!("--- obrain_recherche « {q} » sur {} ---", store.display());
        println!("{}", r.texte);
        println!("--- {} lignes · {} ms ---", r.lignes, r.ms);
        assert!(r.erreur.is_none(), "erreur outil : {:?}", r.erreur);
        assert!(r.lignes > 0, "aucun résultat rendu :\n{}", r.texte);
    }
}
