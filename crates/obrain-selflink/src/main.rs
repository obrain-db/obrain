//! `selflink` — le contact minimal avec le moteur : le self, la mémoire, les bases.
//!
//! # Pourquoi ce binaire existe
//!
//! Le moteur obrain expose tout ce qu'il faut pour être **piloté de
//! l'extérieur** : une IAM (`/api/auth/*`), les bases (`/api/stores`), une
//! console de requêtes (`/api/query`), la mémoire du self
//! (`/api/engine/memoire`) et une conversation **ancrée sur le graphe**
//! (`/api/engine/chat`, en SSE). Mais la seule façon de lui parler aujourd'hui
//! depuis un programme, c'est d'ouvrir le dashboard dans un navigateur, ou de
//! lancer `obrain` — c'est-à-dire d'avoir le binaire **et** ses bases sur la
//! même machine.
//!
//! `selflink` est la troisième voie : un client qui ne suppose qu'une chose, une
//! URL, et qui ne dépend ni du binaire `obrain`, ni d'un store local, ni d'un
//! système de fichiers partagé. C'est la brique de base d'un harness qui tourne
//! **ailleurs** et qui parle à un self situé ici.
//!
//! # Ce qu'il ne fait pas (encore), et pourquoi c'est écrit
//!
//! L'HTTP du moteur est **en lecture seule** pour ce qui compte : il n'existe
//! aucune route d'ingestion. Les écritures durables (`import_events`,
//! `self/consolidate`, `memory_register`) passent par le bridge MCP en
//! **stdio**, donc par un processus local. `selflink` lit, interroge et
//! converse ; il ne consolide pas encore le self. Prétendre le contraire
//! supposerait une route qui n'existe pas.
//!
//! # Usage
//!
//! ```text
//! selflink info                      signature du moteur
//! selflink stores                    les bases (mondes) connues
//! selflink self      [--store S]     le self : qn, sortances, mondes
//! selflink memoire   [--store S]     intentions du self en attente
//! selflink query "<gql>" [--store S] requête brute
//! selflink ask "<question>" [--store S] [--qn QN]   conversation ancrée (SSE)
//! selflink sante                     santé du moteur de conversation
//! selflink login --user U [--password P]            connexion IAM
//! ```
//!
//! Options globales : `--url`, `--store`, `--token`, `--json`.
//! Variables d'environnement : `SELFLINK_URL` / `OBRAIN_URL`,
//! `SELFLINK_STORE` / `OBRAIN_STORE`, `SELFLINK_TOKEN` / `OBRAIN_TOKEN`,
//! `SELFLINK_PASSWORD`.

mod engine;

use std::io::Write;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use engine::{Moteur, URL_DEFAUT};

/// Variables d'environnement lues, avec le préfixe maison d'abord.
///
/// `OBRAIN_*` est accepté **exprès** : c'est le nom que porte déjà
/// l'écosystème (bridge, serveur), et deux noms pour la même intention valent
/// mieux qu'une variable inventée qui ne se branche sur rien.
fn env_premier(noms: &[&str]) -> Option<String> {
    noms.iter()
        .find_map(|n| std::env::var(n).ok())
        .filter(|v| !v.trim().is_empty())
}

struct Args {
    commande: String,
    positionnels: Vec<String>,
    url: String,
    store: Option<String>,
    token: Option<String>,
    qn: Option<String>,
    json: bool,
    user: Option<String>,
    password: Option<String>,
}

const AIDE: &str = "\
selflink — le contact minimal avec le moteur : le self, la mémoire, les bases

USAGE
  selflink <commande> [arguments] [options]

COMMANDES
  info                       signature du moteur (base par défaut, nœuds)
  stores                     les bases (mondes) connues du serveur
  mondes [--store S]         les TROIS référentiels de mondes et leurs désaccords
  hubs [n] [entrant|sortant] les hubs du graphe, CLASSÉS par nature de chemin
  self                       le self : qn, sortances par type, mondes possédés
  memoire                    intentions du self en attente
  query <gql>                exécute une requête et rend les lignes
  ask <question>             un tour de conversation ancré sur le self (SSE)
  sante                      santé du moteur de conversation
  login --user U [--password P]   connexion IAM, rend un jeton

OPTIONS
  --url <url>       racine du moteur       (défaut $SELFLINK_URL, sinon 127.0.0.1:8432)
  --store <base>    base visée             (défaut $SELFLINK_STORE, sinon celle du serveur)
  --token <jeton>   jeton IAM              (défaut $SELFLINK_TOKEN)
  --qn <qn>         sujet ancré de `ask`   (défaut : le graphe global)
  --json            sortie JSON brute
  -h, --help        cette aide

NOTE
  Le serveur doit tourner et être joignable. `selflink` n'ouvre aucun store et
  ne lit aucun fichier : il ne dépend que d'une URL.
";

fn parse_args() -> Result<Args> {
    let mut commande = String::new();
    let mut positionnels: Vec<String> = Vec::new();
    let (mut url, mut store, mut token, mut qn) = (None, None, None, None);
    let (mut user, mut password) = (None, None);
    let mut json = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{AIDE}");
                std::process::exit(0);
            }
            "--json" => json = true,
            "--url" => url = it.next(),
            "--store" => store = it.next(),
            "--token" => token = it.next(),
            "--qn" => qn = it.next(),
            "--user" => user = it.next(),
            "--password" => password = it.next(),
            _ if a.starts_with("--") => bail!("option inconnue : {a} (voir --help)"),
            _ => {
                if commande.is_empty() {
                    commande = a;
                } else {
                    positionnels.push(a);
                }
            }
        }
    }

    if commande.is_empty() {
        print!("{AIDE}");
        std::process::exit(2);
    }

    Ok(Args {
        commande,
        positionnels,
        url: url
            .or_else(|| env_premier(&["SELFLINK_URL", "OBRAIN_URL"]))
            .unwrap_or_else(|| URL_DEFAUT.to_string()),
        store: store.or_else(|| env_premier(&["SELFLINK_STORE", "OBRAIN_STORE"])),
        token: token.or_else(|| env_premier(&["SELFLINK_TOKEN", "OBRAIN_TOKEN"])),
        qn,
        json,
        user: user.or_else(|| env_premier(&["SELFLINK_USER"])),
        password: password.or_else(|| env_premier(&["SELFLINK_PASSWORD"])),
    })
}

/// Imprime une valeur JSON en jolie-forme.
fn rendre_json(v: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

/// Extrait les lignes d'une réponse de `/api/query` : `rows` est une liste de
/// listes, indexée par `columns`.
fn lignes(v: &Value) -> Vec<Vec<String>> {
    let cols: Vec<String> = v["columns"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|x| x.as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Some(rows) = v["rows"].as_array() {
        for r in rows {
            let Some(cellules) = r.as_array() else { continue };
            let ligne: Vec<String> = cellules
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let nom = cols.get(i).map(String::as_str).unwrap_or("?");
                    format!("{nom}={}", compact(c))
                })
                .collect();
            out.push(ligne);
        }
    }
    out
}

/// Rend une cellule en une ligne, sans guillemets parasites.
fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "∅".to_string(),
        autre => autre.to_string(),
    }
}

fn main() {
    if let Err(e) = executer() {
        // La chaîne complète (`{:#}`) : un contexte ajouté par `with_context`
        // n'apparaît pas avec `{}` seul, et c'est justement lui qui dit *quelle*
        // étape a échoué.
        eprintln!("selflink : {e:#}");
        std::process::exit(1);
    }
}

/// Les trois référentiels de « mondes », et leurs désaccords.
///
/// # Pourquoi cette commande existe
///
/// Mesuré : interrogé en conversation, le self a répondu tenir **12 mondes**.
/// Aucune requête unique ne produit 12. Trois référentiels coexistent en effet,
/// et ils ne s'accordent pas :
///
/// 1. **le graphe** — les nœuds `World` et les arêtes `OWNS` du self
///    (6 nœuds, 6 arêtes dans `agent-synapse-triviere-dsh.obrain`) ;
/// 2. **les presets du serveur** — `/api/stores` (7 bases) ;
/// 3. **le registre du serveur** — `/api/worlds`, qui ajoute les dépôts
///    importés à chaud (`/tmp/obrain-worlds/mondes.json`, 5 dépôts GitHub).
///
/// Un chiffre de self qui ne cite pas son référentiel n'est pas vérifiable.
/// Cette commande rend les trois côte à côte, avec leur masse, pour qu'un
/// désaccord se **voie** au lieu de se découvrir en conversation.
fn commande_mondes(moteur: &Moteur, args: &Args) -> Result<()> {
    let serveur = moteur.mondes()?;
    let presets = moteur.stores()?;

    let liste_serveur: Vec<(String, String, i64, i64, bool)> = serveur["mondes"]
        .as_array()
        .map(|m| {
            m.iter()
                .map(|e| {
                    (
                        e["label"].as_str().unwrap_or("?").to_string(),
                        e["path"].as_str().unwrap_or("?").to_string(),
                        e["nodes"].as_i64().unwrap_or(0),
                        e["edges"].as_i64().unwrap_or(0),
                        e["ajoute_a_chaud"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let liste_presets: Vec<(String, String)> = presets["presets"]
        .as_array()
        .map(|p| {
            p.iter()
                .map(|e| {
                    (
                        e["label"].as_str().unwrap_or("?").to_string(),
                        e["path"].as_str().unwrap_or("?").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // 3e référentiel : le graphe. Il faut une base — et sur la base par défaut
    // (`runtime-dev`), il n'y a AUCUN nœud `World` : le dire, plutôt que
    // d'afficher « 0 » comme si c'était une réponse.
    let graphe: Option<Vec<(String, String)>> = if moteur.store.is_some() {
        let v = moteur.query(
            "MATCH (w:World) RETURN w.qn AS qn, w.label AS label LIMIT 500",
            "gql",
        )?;
        Some(
            v["rows"]
                .as_array()
                .map(|lignes| {
                    lignes
                        .iter()
                        .filter_map(|l| l.as_array())
                        .map(|c| {
                            (
                                c.first().and_then(Value::as_str).unwrap_or("").to_string(),
                                c.get(1).and_then(Value::as_str).unwrap_or("").to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    } else {
        None
    };

    if args.json {
        let v = serde_json::json!({
            "registre_serveur": liste_serveur.iter().map(|(l, p, n, e, a)| serde_json::json!({
                "label": l, "path": p, "nodes": n, "edges": e, "ajoute_a_chaud": a
            })).collect::<Vec<_>>(),
            "presets": liste_presets.iter().map(|(l, p)| serde_json::json!({
                "label": l, "path": p
            })).collect::<Vec<_>>(),
            "graphe": graphe.as_ref().map(|g| g.iter().map(|(q, l)| serde_json::json!({
                "qn": q, "label": l
            })).collect::<Vec<_>>()),
        });
        return rendre_json(&v);
    }

    println!("A. registre du serveur — /api/worlds ({} mondes)", liste_serveur.len());
    for (label, path, n, e, chaud) in &liste_serveur {
        println!(
            "   {:<28} {:>9} nœuds {:>10} arêtes  {}{}",
            label,
            n,
            e,
            path,
            if *chaud { "   [ajouté à chaud]" } else { "" }
        );
    }

    println!("\nB. presets configurés — /api/stores ({} bases)", liste_presets.len());
    for (label, path) in &liste_presets {
        println!("   {label:<28} {path}");
    }

    // Divergences PROUVABLES : comparaison par chemin, pas par ressemblance de
    // libellé — deux libellés qui se ressemblent ne prouvent rien.
    let chemins_serveur: Vec<&str> = liste_serveur.iter().map(|(_, p, _, _, _)| p.as_str()).collect();
    let chemins_presets: Vec<&str> = liste_presets.iter().map(|(_, p)| p.as_str()).collect();
    let hors_presets: Vec<&(String, String, i64, i64, bool)> = liste_serveur
        .iter()
        .filter(|(_, p, _, _, _)| !chemins_presets.contains(&p.as_str()))
        .collect();
    let hors_serveur: Vec<&(String, String)> = liste_presets
        .iter()
        .filter(|(_, p)| !chemins_serveur.contains(&p.as_str()))
        .collect();

    println!("\nC. désaccords prouvables (comparaison par CHEMIN)");
    if hors_presets.is_empty() && hors_serveur.is_empty() {
        println!("   aucun : les deux registres du serveur coïncident");
    } else {
        for (l, p, _, _, _) in &hors_presets {
            println!("   - dans /api/worlds, absent des presets : {l} → {p}");
        }
        for (l, p) in &hors_serveur {
            println!("   - dans les presets, absent de /api/worlds : {l} → {p}");
        }
    }

    match &graphe {
        Some(g) => {
            println!("\nD. graphe de la base visée — {} nœuds World", g.len());
            for (qn, label) in g {
                println!("   {qn:<34} {label}");
            }
            println!(
                "\n   le graphe porte {} mondes, le serveur en annonce {} : {}",
                g.len(),
                liste_serveur.len(),
                if g.len() == liste_serveur.len() {
                    "ils coïncident"
                } else {
                    "ILS DIVERGENT — tout chiffre doit citer son référentiel"
                }
            );
        }
        None => println!(
            "\nD. graphe : non interrogé (aucun --store).\n   \
             Attention — sur la base par défaut (`runtime-dev`) il n'y a AUCUN nœud `World` :\n   \
             le self vit dans la base synaptique. Relancer avec --store <chemin absolu>."
        ),
    }

    Ok(())
}

/// Classe un nœud par la **nature de son chemin**, pas par son nom.
///
/// # Pourquoi cette fonction existe
///
/// Mesuré sur `runtime-dev` : les premiers nœuds du graphe par degré sortant sont
/// `aletheia/dist/static/assets/la` (28 368 arêtes), `…/Os` (23 792) et
/// `deepseek_harness/pnpm-lock.yaml.__file__` (17 585). Ce ne sont pas des hubs
/// d'architecture — ce sont des **sorties de build** et un fichier de
/// verrouillage, c'est-à-dire des effets de l'indexation.
///
/// Et le cas qui a mis sur la piste : `MultiDrawRenderList.push`, méthode de
/// **three.js vendorisé**, porte **23 676 appelants** sur 210 fichiers, dont des
/// bundles `@tanstack/router` qui ne peuvent pas l'appeler. Collision de nom,
/// pas dépendance.
///
/// Sans provenance de résolution dans le store (calculée puis jetée à l'import —
/// voir `index_repository`), on ne peut pas trier les arêtes par confiance. On
/// peut au moins dire **d'où vient le nœud**, ce qui sépare déjà l'architecture
/// du bruit d'indexation.
fn classe_du_chemin(chemin: &str, nom: &str) -> &'static str {
    let c = chemin.to_lowercase();
    let fichier = c.rsplit('/').next().unwrap_or(&c);

    if c.is_empty() {
        return "sans chemin";
    }
    if fichier.contains("lock")
        && (fichier.ends_with(".yaml") || fichier.ends_with(".json") || fichier.ends_with(".lock"))
    {
        return "verrouillage";
    }
    if c.contains("/vendor/") || c.contains("/third_party/") || c.contains("/node_modules/") {
        return "tiers vendorisé";
    }
    if c.contains("/.vercel/")
        || c.contains("/dist/")
        || c.contains("/build/")
        || c.contains("/.next/")
        || c.contains("/out/")
        || c.contains("/target/")
        || c.contains("/static/assets")
    {
        return "sortie de build";
    }
    if c.contains(".min.") {
        return "minifié";
    }
    // Identifiant d'une ou deux lettres dans un fichier de bundle : signature
    // d'un minifieur, même sans `/dist/` dans le chemin.
    if nom.len() <= 2
        && nom.chars().all(|x| x.is_ascii_alphanumeric())
        && (c.ends_with(".js") || c.ends_with(".mjs"))
    {
        return "identifiant minifié";
    }
    "source"
}

/// Est-ce un artéfact d'indexation plutôt qu'une architecture ?
fn est_artefact(classe: &str) -> bool {
    matches!(
        classe,
        "verrouillage" | "tiers vendorisé" | "sortie de build" | "minifié" | "identifiant minifié"
    )
}

/// L'audit des hubs : qui domine le graphe, et est-ce de l'architecture ?
fn commande_hubs(moteur: &Moteur, args: &Args) -> Result<()> {
    let n: usize = args
        .positionnels
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let sens = args.positionnels.get(1).map(String::as_str).unwrap_or("entrant");

    // Le degré se compte **par nœud**, et le chemin accompagne chaque ligne : un
    // hub sans son chemin ne se classe pas.
    let gql = if sens == "sortant" {
        format!(
            "MATCH (a)-[r]->(b) RETURN a.qualified_name AS noeud, a.file_path AS chemin, \
             a.name AS nom, type(r) AS lien, count(b) AS degre ORDER BY degre DESC LIMIT {n}"
        )
    } else {
        format!(
            "MATCH (a)-[r]->(b) RETURN b.qualified_name AS noeud, b.file_path AS chemin, \
             b.name AS nom, type(r) AS lien, count(a) AS degre ORDER BY degre DESC LIMIT {n}"
        )
    };
    let v = moteur.query(&gql, "gql")?;
    let lignes = v["rows"].as_array().cloned().unwrap_or_default();

    if lignes.is_empty() {
        println!("(aucun hub — base vide, ou requête sans résultat)");
        return Ok(());
    }

    let mut total_degre: i64 = 0;
    let mut artefact_degre: i64 = 0;
    let mut artefact_nb = 0usize;
    let mut detail: Vec<(String, String, String, i64, String)> = Vec::new();

    for l in &lignes {
        let Some(c) = l.as_array() else { continue };
        let noeud = c.first().and_then(Value::as_str).unwrap_or("").to_string();
        let chemin = c.get(1).and_then(Value::as_str).unwrap_or("").to_string();
        let nom = c.get(2).and_then(Value::as_str).unwrap_or("").to_string();
        let lien = c.get(3).and_then(Value::as_str).unwrap_or("").to_string();
        let degre = c.get(4).and_then(Value::as_i64).unwrap_or(0);
        let classe = classe_du_chemin(&chemin, &nom).to_string();
        if est_artefact(&classe) {
            artefact_nb += 1;
            artefact_degre += degre;
        }
        total_degre += degre;
        detail.push((noeud, chemin, lien, degre, classe));
    }

    if args.json {
        let v = serde_json::json!({
            "sens": sens,
            "hubs": detail.iter().map(|(n, c, l, d, k)| serde_json::json!({
                "noeud": n, "chemin": c, "lien": l, "degre": d, "classe": k
            })).collect::<Vec<_>>(),
            "synthese": {
                "analyses": detail.len(),
                "artefacts": artefact_nb,
                "part_artefacts": artefact_nb as f64 / detail.len() as f64,
                "degre_total": total_degre,
                "degre_artefacts": artefact_degre,
                "part_degre_artefacts": if total_degre > 0 {
                    artefact_degre as f64 / total_degre as f64
                } else { 0.0 },
            }
        });
        return rendre_json(&v);
    }

    println!("=== {n} premiers hubs par degré {sens} ===");
    for (noeud, chemin, lien, degre, classe) in &detail {
        let marque = if est_artefact(classe) { "!" } else { " " };
        let court: String = if chemin.is_empty() {
            noeud.chars().take(64).collect()
        } else {
            chemin.chars().take(64).collect()
        };
        println!(" {marque} {degre:>8}  {lien:<14} {classe:<20} {court}");
    }
    println!("\n=== synthèse ===");
    println!(
        "  {artefact_nb}/{} hubs sont des ARTÉFACTS d'indexation ({:.0} %)",
        detail.len(),
        100.0 * artefact_nb as f64 / detail.len() as f64
    );
    println!(
        "  ils portent {artefact_degre} arêtes sur {total_degre} ({:.0} %)",
        if total_degre > 0 {
            100.0 * artefact_degre as f64 / total_degre as f64
        } else {
            0.0
        }
    );
    println!(
        "\n  Un hub « artefact » ne dit rien de l'architecture : sortie de build,\n  \
         tiers vendorisé, fichier de verrouillage. Les filtrer à l'indexation\n  \
         changerait ce que « hub » veut dire."
    );
    Ok(())
}

fn executer() -> Result<()> {
    let args = parse_args()?;
    let mut moteur = Moteur::nouveau(args.url.clone(), args.store.clone(), args.token.clone());
    // Un `--store` qui n'est pas un chemin absolu est RÉSOLU contre les registres
    // du serveur, jamais deviné — sinon un nom nu lit une base vide en silence.
    // La note va sur stderr pour ne pas polluer une sortie `--json`.
    if let Some(note) = moteur.resoudre_store()? {
        eprintln!("{note}");
    }

    match args.commande.as_str() {
        "info" => {
            let v = moteur.info()?;
            if args.json {
                rendre_json(&v)?;
            } else {
                println!("moteur   : {}", args.url);
                println!("base     : {}", v["store"].as_str().unwrap_or("?"));
                println!("nœuds    : {}", v["nodes_estimate"].as_u64().unwrap_or(0));
            }
        }

        "stores" => {
            let v = moteur.stores()?;
            if args.json {
                rendre_json(&v)?;
            } else {
                println!("défaut : {}", v["default"].as_str().unwrap_or("?"));
                if let Some(presets) = v["presets"].as_array() {
                    for p in presets {
                        println!(
                            "  - {:<24} {}",
                            p["label"].as_str().unwrap_or("?"),
                            p["path"].as_str().unwrap_or("?")
                        );
                    }
                }
            }
        }

        "mondes" => {
            commande_mondes(&moteur, &args)?;
        }

        "hubs" => {
            commande_hubs(&moteur, &args)?;
        }

        "self" => {
            let s = moteur.self_noeud()?;
            if args.json {
                let v = serde_json::json!({
                    "qn": s.qn,
                    "sortances": s.sortances,
                    "mondes": s.mondes,
                });
                rendre_json(&v)?;
            } else {
                println!("self : {}", s.qn);
                println!("  tient :");
                for (t, n) in &s.sortances {
                    println!("    {t:<12} {n}");
                }
                println!("  mondes ({}) :", s.mondes.len());
                for (qn, label) in &s.mondes {
                    println!("    - {qn:<32} {label}");
                }
            }
        }

        "memoire" => {
            let v = moteur.memoire()?;
            if args.json {
                rendre_json(&v)?;
            } else {
                println!("en attente : {}", v["en_attente"].as_u64().unwrap_or(0));
                if let Some(entrees) = v["entrees"].as_array() {
                    for e in entrees {
                        println!(
                            "  - [{}] {}",
                            e["genre"].as_str().unwrap_or("?"),
                            e["reference"].as_str().unwrap_or("").trim()
                        );
                    }
                }
                if let Some(a) = v["appliquer"].as_str() {
                    println!("appliquer : {a}");
                }
            }
        }

        "query" => {
            let Some(q) = args.positionnels.first() else {
                bail!("query : requête manquante (selflink query \"MATCH (n) RETURN n LIMIT 1\")");
            };
            let langage = args
                .positionnels
                .get(1)
                .map(String::as_str)
                .unwrap_or("gql");
            let v = moteur.query(q, langage)?;
            if args.json {
                rendre_json(&v)?;
            } else {
                let l = lignes(&v);
                if l.is_empty() {
                    println!("(aucune ligne)");
                }
                for ligne in l {
                    println!("{}", ligne.join("  "));
                }
                if let Some(ms) = v["execution_time_ms"].as_f64() {
                    eprintln!("{:?} ligne(s) en {ms:.1} ms", v["rows"].as_array().map_or(0, Vec::len));
                }
            }
        }

        "ask" => {
            let Some(question) = args.positionnels.first() else {
                bail!("ask : question manquante (selflink ask \"où en est le self ?\")");
            };
            // La question s'affiche AVANT le tour : sans cela, l'attente du LLM
            // local (parfois plusieurs secondes) ressemble à un blocage.
            println!("› {question}");
            let mut premier = true;
            let raison = moteur.demander(question, args.qn.as_deref(), |frag| {
                if premier {
                    print!("‹ ");
                    premier = false;
                }
                print!("{frag}");
                std::io::stdout().flush().ok();
            })?;
            if premier {
                // Aucun fragment : le dire, plutôt que de rendre une ligne vide
                // qui passerait pour une réponse.
                println!("‹ (aucun texte rendu)");
            } else {
                println!();
            }
            // `raison` dit si la réponse a été COUPÉE (plafond de jetons) au lieu
            // de laisser croire à une fin naturelle — le défaut que la vue plein
            // écran du dashboard rapporte déjà explicitement.
            if !raison.is_empty() && raison != "stop" {
                eprintln!("(fin : {raison})");
            }
        }

        "sante" => {
            let v = moteur.sante()?;
            rendre_json(&v)?;
        }

        "login" => {
            let user = args
                .user
                .clone()
                .filter(|u| !u.trim().is_empty())
                .context("login : --user requis (ou $SELFLINK_USER)")?;
            let pass = args
                .password
                .clone()
                .context("login : --password requis (ou $SELFLINK_PASSWORD)")?;
            let v = moteur.login(&user, &pass)?;
            if args.json {
                rendre_json(&v)?;
            } else {
                // Le jeton est rendu, mais on ne le met pas en avant : il
                // s'exporte (`export SELFLINK_TOKEN=…`), il ne se recopie pas
                // dans un historique de shell par distraction.
                match v["token"].as_str().or_else(|| v["jeton"].as_str()) {
                    Some(t) => println!("jeton : {t}\nexport SELFLINK_TOKEN={t}"),
                    None => println!("{}", serde_json::to_string_pretty(&v)?),
                }
            }
        }

        autre => bail!("commande inconnue : {autre} (voir --help)"),
    }

    Ok(())
}
