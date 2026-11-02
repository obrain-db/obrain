//! Serveur MCP (JSON-RPC stdio) — le MODE BRIDGE d'obrain (CH1b, 2026-08-31).
//!
//! Le harness est un client MCP standard (`@modelcontextprotocol` SDK,
//! StdioClientTransport) qui lance le moteur en sous-processus. Pour
//! « switcher la db » sans toucher au harness, ce module expose le même
//! protocole : JSON-RPC 2.0 sur stdin/stdout, `initialize`, `tools/list`,
//! `tools/call` — et route les outils que le harness appelle RÉELLEMENT
//! (vérifié dans `graph-injector.ts` / `session-persistence`) :
//!
//! - `search_graph`    → BM25 (`text_search`) + requêtes structurées
//! - `runtime_search`  → BM25 sur le contenu runtime
//! - `sessions_list`   → liste des sessions (query sur nœuds Session)
//! - `context_build`   → injection composée (goals, summaries, weave) — CH6
//! - `import_events`   → écriture idempotente (MERGE / mutations) — GraphWriter
//! - `get_graph_schema`→ labels, types d'arêtes, clés de propriétés
//!
//! Contrat de réponse (miroir du moteur C) : `structuredContent` = payload
//! JSON objet (l'autorité pour le harness), `content[0].text` = JSON, et
//! `isError` sur échec. Le store est résolu par `OBRAIN_STORE` (env) ou
//! `--store <chemin.obrain>`.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use obrain_engine::ObrainDB;

use serde_json::{Value, json};

/// (2026-09-07) MÉMO-CACHE de `sessions_list` — le pattern UI liste les
/// sessions très souvent ; reconstruire (~9 ms de décodage props) à chaque
/// appel est inutile quand rien n'a changé. Réservé au process écrivain
/// (OBRAIN_WRITE=1) : c'est lui qui sert les listes du GUI et qui voit
/// TOUTES les mutations (`import_events` → génération incrémentée) — pas de
/// staleness possible dans le process. Les process RO (multiples, spawnés
/// par requête) ne cachent pas : leur liste est un point de vue d'un instant,
/// jamais longtemps réutilisé.
static SESSIONS_LIST_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SESSIONS_LIST_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(u64, Arc<Value>)>>> =
    std::sync::OnceLock::new();

fn sessions_cache_get(generation: u64) -> Option<Value> {
    let guard = SESSIONS_LIST_CACHE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &*guard {
        Some((g, rows)) if *g == generation => Some((**rows).clone()),
        _ => None,
    }
}

fn sessions_cache_put(generation: u64, value: Value) {
    let mut guard = SESSIONS_LIST_CACHE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some((generation, Arc::new(value)));
}

fn is_writer_process() -> bool {
    std::env::var("OBRAIN_WRITE")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Arguments du mode `mcp`.
#[derive(Args, Debug)]
pub struct McpArgs {
    /// Chemin du store `.obrain` (défaut : env OBRAIN_STORE).
    #[arg(long)]
    store: Option<PathBuf>,
}

/// Version du protocole MCP annoncée.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
/// Nom du serveur.
const SERVER_NAME: &str = "obrain-mcp";

/// Point d'entrée : boucle JSON-RPC sur stdin.
/// L'échéance des requêtes du PONT MCP (lecture seule).
///
/// Un seul réglage pour tous les points d'entrée : `OBRAIN_QUERY_TIMEOUT_MS`
/// (`0` désarme). Défaut 30 s ici, comme le serveur de visualisation ; le CLI
/// prend 60 s (`commands::query`), parce qu'une requête en ligne de commande est
/// un geste délibéré. Ce qui compte n'est pas la valeur mais le fait que le pont
/// ne soit plus le SEUL chemin non borné : c'était celui du harness, donc celui
/// d'un appel d'outil d'agent qui ne rendait jamais la main sur un motif
/// explosif (`MATCH (a)-[*1..2]->(b) WHERE id(b) IN [315447] …` tournait encore
/// après 150 s, mesuré le 2026-09-12).
///
/// Les six lignes sont dupliquées entre les quatre points d'entrée (serveur,
/// CLI, REPL, pont) tant que leurs fichiers bougent : à consolider dans un
/// module partagé une fois stabilisés.
fn echeance_requete_pont() -> Option<std::time::Duration> {
    let ms = std::env::var("OBRAIN_QUERY_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(30_000);
    (ms > 0).then(|| std::time::Duration::from_millis(ms))
}

pub fn run(args: McpArgs) -> Result<()> {
    // Le pont vérifie des identités, il n'en écrit pas (voir `IAM_EN_LECTURE`).
    crate::viz_serve::GardeIam::lecteur_seulement();
    let store_path = args
        .store
        .or_else(|| std::env::var("OBRAIN_STORE").ok().map(PathBuf::from))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "obrain mcp: aucun store — passez --store <chemin.obrain> ou OBRAIN_STORE"
            )
        })?;

    // Ouvre le store une fois (le process sert toutes les requêtes).
    // (2026-08-31) MODE LECTURE PAR DÉFAUT : le GraphInjector du harness
    // respawn `obrain bridge` après CHAQUE requête (dispose) — avec un
    // verrou exclusif, le respawn suivant échoue tant que l'ancien process
    // n'est pas mort (liste vide → sélecteur de projet vide). L'ouverture
    // EN LECTURE SEULE utilise le verrou PARTAGÉ (LOCK_SH) : les respawns
    // multiples coexistent. L'écriture (import_events) exige le mode RW —
    // activé par OBRAIN_WRITE=1 (le process du GraphWriter, persistant).
    let write_mode = std::env::var("OBRAIN_WRITE")
        .map(|v| v == "1")
        .unwrap_or(false);
    // (2026-09-01) RETRY à l'ouverture : le GraphInjector respawn le bridge
    // après chaque requête — les verrous substrate (partagé RO / exclusif RW)
    // peuvent se croiser pendant ~200ms (dispose). Comme SQLite busy_timeout :
    // on retente (100ms) jusqu'à OBRAIN_OPEN_RETRY_MS (défaut 10s) au lieu
    // d'échouer immédiatement. Garantit la coexistence lecteur (service) +
    // écrivain (langextract) sans verrou exclusif permanent.
    let open_start = std::time::Instant::now();
    let retry_ms: u64 = std::env::var("OBRAIN_OPEN_RETRY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let mut last_err: Option<String> = None;
    let mut db: Option<std::sync::Arc<ObrainDB>> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(retry_ms);
    while db.is_none() && std::time::Instant::now() < deadline {
        let attempt = if write_mode {
            // (2026-09-05) SHARED-WRITE (verrou PARTAGÉ + WAL) au lieu du RW
            // EXCLUSIF. Régression corrigée : le verrou exclusif persistant
            // (gardé tant que le process vit) bloquait TOUTE lecture
            // concurrente — un GraphWriter actif en continu (session live)
            // ne libère jamais le verrou (idle jamais atteint) → graph_query
            // et bridges RO timeout sur le même store. `open_shared_write`
            // est le mode prévu pour l'UNIQUE écrivain (GraphWriter) : verrou
            // partagé (lecteurs RO coexistent), écritures journalisées WAL,
            // les lecteurs rejouent à leur ouverture. Le constat de
            // corruption WAL (2026-09-01) visait le shared-write PUR initial
            // sans sérialisation ; ce mode sérialise via le writer unique.
            // INVARIANT à préserver : un seul process OBRAIN_WRITE=1 par store
            // (deux writers shared corrompraient le WAL).
            //
            // ── ET L'ÉCHÉANCE N'EST **PAS** ARMÉE ICI, EXPRÈS ───────────────
            // C'est le chemin de l'ÉCRIVAIN (GraphWriter, indexation, imports
            // par lots) : une écriture longue est LÉGITIME, et la couper
            // laisserait un travail à moitié fait. Le contrôle vise les
            // LECTURES qui n'ont aucune raison de durer.
            ObrainDB::open_shared_write(&store_path)
        } else {
            // ── LE PONT AUSSI EST BORNÉ ─────────────────────────────────────
            // Le serveur (30 s) et `obrain query` (60 s) arment l'échéance ;
            // le pont MCP ne l'armait pas — or c'est le chemin par lequel le
            // harness interroge le graphe : une requête qui explose y pendait
            // indéfiniment, donc un APPEL D'OUTIL d'agent ne rendait jamais la
            // main. Même constat que côté serveur, mesuré le 2026-09-12 :
            // `MATCH (a)-[*1..2]->(b) WHERE id(b) IN [315447] …` tournait
            // encore après 150 s.
            //
            // Même réglage que partout : `OBRAIN_QUERY_TIMEOUT_MS` (`0`
            // désarme). Défaut 30 s ; les requêtes d'outils se mesurent en
            // millisecondes (la lecture d'un voisinage est passée de 905 ms à
            // 40-190 µs), donc le rail ne gêne que l'explosion.
            ObrainDB::open_read_only(&store_path)
                .map(|db| db.with_query_timeout(echeance_requete_pont()))
        };
        match attempt {
            Ok(d) => db = Some(Arc::new(d)),
            Err(e) => {
                last_err = Some(e.to_string());
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    let db = db.ok_or_else(|| {
        anyhow::anyhow!(
            "obrain mcp: ouverture du store impossible après {} ms (dernière erreur: {:?})",
            retry_ms,
            last_err
        )
    })?;
    let _open_elapsed = open_start.elapsed();
    eprintln!(
        "obrain-mcp: store ouvert ({}, {}, {:.1} ms)",
        store_path.display(),
        if write_mode { "RW" } else { "RO" },
        _open_elapsed.as_millis()
    );

    // (debug) OBRAIN_DEBUG_LOG : journal des requêtes/réponses (diagnostic).
    let debug_log = std::env::var("OBRAIN_DEBUG_LOG").ok();
    if let Some(path) = &debug_log {
        if let Ok(f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write as _;
            let mut f = f;
            let _ = writeln!(
                f,
                "[boot] obrain-mcp pid={} store={}",
                std::process::id(),
                store_path.display()
            );
        }
    }
    // (2026-09-01) IDLE TIMEOUT : le bridge s'auto-ferme après
    // OBRAIN_IDLE_MS (défaut 5000) sans requête sur stdin. C'est ce qui
    // permet la COEXISTENCE service-RO + GraphWriter-RW (langextract) :
    // le registre du harness garde son bridge vivant (verrou partagé
    // permanent) → le RW exclusif ne pourrait jamais s'ouvrir. Avec
    // l'auto-fermeture, le verrou partagé est libéré entre les requêtes
    // (le registre respawn au prochain appel — ensureProc), et le
    // GraphWriter RW s'insère. EOF sur stdin → la boucle se termine.
    let idle_ms: u64 = std::env::var("OBRAIN_IDLE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    // Réarmé à chaque requête : le thread surveille `last_activity`
    // (Instant) ; quand l'inactivité dépasse idle_ms → EOF sur stdin.
    #[allow(unsafe_code)]
    #[allow(clippy::all)]
    let idle_armed = idle_ms > 0;
    let last_activity = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    if idle_armed {
        let last = std::sync::Arc::clone(&last_activity);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(idle_ms.min(1000)));
                let idle_for = last
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .elapsed();
                if idle_for >= std::time::Duration::from_millis(idle_ms) {
                    use std::os::unix::io::FromRawFd as _;
                    // SAFETY: fd 0 (stdin) — fermeture à la fin du timeout.
                    #[allow(unsafe_code)]
                    let f = unsafe { std::fs::File::from_raw_fd(0) };
                    drop(f);
                    break;
                }
            }
        });
    }
    // ── UNE REQUÊTE QUI N'EN FINIT PAS DOIT SE VOIR ─────────────────────────
    // Défaut mesuré le 2026-09-17 : un `obrain bridge` a brûlé un cœur pendant
    // plus de quatre minutes SANS RIEN DIRE — 90 % de CPU, 2,3 Go de RSS, aucune
    // ligne de journal. Une échéance existe pourtant (`OBRAIN_QUERY_TIMEOUT_MS`,
    // 30 s) mais un chemin qui ne la consulte pas passe dessous, et l'incident
    // reste invisible : on ne peut pas corriger ce qu'on ne voit pas.
    //
    // Ce veilleur ne remplace pas l'échéance (il ne peut pas interrompre un
    // calcul qui ne vérifie rien) : il NOMME le coupable — la requête en cours
    // et son âge — pour qu'un `top` sur un pont se lise dans le journal au lieu
    // de se deviner.
    let requete_en_cours: std::sync::Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        let en_cours = std::sync::Arc::clone(&requete_en_cours);
        let seuil = seuil_alerte_requete();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let vue = en_cours
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .map(|(outil, debut)| (outil.clone(), debut.elapsed()));
                if let Some(message) =
                    alerte_requete(vue.as_ref().map(|(o, d)| (o.as_str(), *d)), seuil)
                {
                    tracing::warn!("{message}");
                }
            }
        });
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        *last_activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::time::Instant::now();
        if line.trim().is_empty() {
            continue;
        }
        // Le veilleur nomme la requête EN COURS : posé avant, effacé après.
        let outil = nom_requete(&line);
        {
            *requete_en_cours
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((outil.clone(), std::time::Instant::now()));
        }
        // ── TOUT APPEL D'OUTIL TOURNE SOUS ÉCHÉANCE (2026-09-17) ────────────
        // Défaut mesuré : l'échéance du pont (`OBRAIN_QUERY_TIMEOUT_MS`, 30 s)
        // n'était armée QUE pour les requêtes GQL — les appels d'OUTILS
        // (`import_events`, `auto_consolider` → `search_surprise` /
        // `search_conflicts` / `search_relief`, décodage de la fenêtre CDC)
        // tournaient hors de toute portée, donc `cancel::check()` n'y voyait
        // jamais d'échéance : un travail parti ne pouvait plus être arrêté, et
        // le pont brûlait un cœur jusqu'à ce qu'il finisse. On arme donc
        // l'échéance autour de la DISPATCH ENTIÈRE, pas seulement du texte GQL.
        let _portee = obrain_common::utils::cancel::frontiere(
            Some(false),
            Some(
                std::time::Instant::now()
                    + echeance_requete_pont().unwrap_or(std::time::Duration::from_secs(30)),
            ),
        );
        let response = match handle_line(&db, &line) {
            Ok(Some(resp)) => resp,
            Ok(None) => continue, // notification — pas de réponse
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": extract_id(&line),
                "error": { "code": -32603, "message": e.to_string() },
            }),
        };
        if let Some(path) = &debug_log {
            if let Ok(f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write as _;
                let mut f = f;
                let _ = writeln!(f, "[req] {line}");
                let _ = writeln!(f, "[resp] {response}");
            }
        }
        // ── UNE SORTIE CASSÉE DOIT ARRÊTER LA BOUCLE, PAS ÊTRE IGNORÉE ──────
        // Le défaut d'origine, mesuré le 2026-09-17 sur un pont vivant :
        // 90 % d'un cœur brûlé en espace utilisateur, 2,3 Go de RSS, et un
        // `let _ = writeln!(...)` qui avalait toutes les erreurs d'écriture.
        // Un `Err` ici ne dit qu'une chose : **le correspondant est parti**
        // (tube cassé, socket fermée). Continuer à tourner est alors le pire
        // des choix — on empile des réponses que personne ne lira, en tenant le
        // verrou du store. On s'arrête, et on le dit.
        *requete_en_cours
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        // ── LE PLAFOND S'APPLIQUE À LA RÉPONSE, PAS À LA REQUÊTE ────────────
        // Une réponse démesurée est remplacée par un refus explicite (mesuré :
        // 234 Mo streamés sans un mot, cf. `plafond_reponse_octets`).
        let response = reponse_bornee(
            response,
            &outil,
            serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|v| v.get("id").cloned()),
            plafond_reponse_octets(),
        );
        if let Err(e) = ecrire_reponse(&mut stdout, &response) {
            tracing::warn!(erreur = %e, "sortie du pont cassée — arrêt de la boucle");
            break;
        }
    }
    Ok(())
}

/// Le plafond de taille d'une réponse du pont, en octets.
///
/// ── POURQUOI UN PLAFOND, MESURÉ ─────────────────────────────────────────────
/// Le 2026-09-17, un `obrain bridge` a été observé écrivant **234 Mo** sur son
/// socket en dix minutes (134 écritures par seconde, ~135 Ko/s), à 83 % d'un
/// cœur, sans qu'aucune ligne de journal ne le signale. En face, le harness
/// consommait le flux (41 % d'un cœur) et l'agent ne voyait rien : ni erreur, ni
/// lenteur attribuable, ni fin. Une réponse sans plafond est un défaut qui ne se
/// voit que chez celui qui le subit.
///
/// Un plafond n'est pas une optimisation, c'est un REFUS EXPLICITE : au-delà, le
/// pont rend une erreur qui NOMME la limite et l'outil, au lieu de streamer des
/// gigaoctets que personne ne lira. Réglable par `OBRAIN_REPONSE_MAX_MO`
/// (défaut 32 Mo — très au-dessus de toute réponse utile, très en dessous d'un
/// emballement).
fn plafond_reponse_octets() -> usize {
    let mo = std::env::var("OBRAIN_REPONSE_MAX_MO")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(32);
    mo.saturating_mul(1024 * 1024)
}

/// Le seuil (en secondes) au-delà duquel une requête en cours est signalée.
///
/// `OBRAIN_REQUETE_ALERTE_MS` (défaut 10 s). Un délai NUL ou illisible garde le
/// défaut : on veut un veilleur, pas un interrupteur qu'on éteint par accident.
fn seuil_alerte_requete() -> std::time::Duration {
    let ms = std::env::var("OBRAIN_REQUETE_ALERTE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10_000);
    std::time::Duration::from_millis(ms)
}

/// Le message d'alerte, ou `None` quand il n'y a rien à dire.
///
/// Fonction PURE (aucune horloge, aucun journal) : c'est la décision qu'on
/// éprouve ; le veilleur ne fait que l'appeler.
fn alerte_requete(
    en_cours: Option<(&str, std::time::Duration)>,
    seuil: std::time::Duration,
) -> Option<String> {
    let (outil, ecoule) = en_cours?;
    (ecoule >= seuil).then(|| {
        format!(
            "requête '{outil}' en cours depuis {:.1} s (seuil {:.1} s) — un pont qui ne rend pas la main se voit ici",
            ecoule.as_secs_f64(),
            seuil.as_secs_f64()
        )
    })
}

/// Le nom de l'outil demandé par une ligne JSON-RPC (pour le veilleur et les journaux).
fn nom_requete(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| {
            v.get("params")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_else(|| "?".to_string())
}

/// Écrit une réponse sur la sortie du pont, et rend l'erreur au lieu de l'avaler.
///
/// Séparée de la boucle pour être éprouvable : un `Write` qui échoue doit faire
/// remonter l'erreur (l'appelant arrête), pas disparaître dans un `let _ =`.
fn ecrire_reponse<W: std::io::Write>(sortie: &mut W, response: &Value) -> std::io::Result<()> {
    writeln!(sortie, "{response}")?;
    sortie.flush()
}

/// Remplace une réponse démesurée par une ERREUR qui la nomme.
///
/// Fonction pure : on y éprouve la décision (le pont, lui, ne fait qu'appeler).
/// `id` est repris de la requête pour que l'appelant JSON-RPC reçoive une
/// réponse à SA question, même quand c'est un refus.
fn reponse_bornee(response: Value, outil: &str, id: Option<Value>, plafond: usize) -> Value {
    let taille = serde_json::to_string(&response).map_or(0, |t| t.len());
    if taille <= plafond {
        return response;
    }
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": -32001,
            "message": format!(
                "réponse refusée : {outil} a produit {taille} octets, au-delà du plafond de {plafond}                  (OBRAIN_REPONSE_MAX_MO). Réduire (LIMIT, filtre, ou une requête plus ciblée) — un flux                  de cette taille ne se lit pas, il bloque les deux côtés."
            ),
        },
    })
}

/// Extrait l'id JSON-RPC d'une requête (pour les réponses d'erreur).
fn extract_id(line: &str) -> Value {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(Value::Null)
}

/// Gère une ligne JSON-RPC. Retourne `None` pour les notifications.
fn handle_line(db: &ObrainDB, line: &str) -> Result<Option<Value>> {
    let req: Value = serde_json::from_str(line)?;

    // (2026-08-31) FORMAT BRIDGE C (le GraphWriter du harness) : le
    // harness envoie `{"tool": "...", "args": {...}}` (pas du JSONRPC) —
    // le protocole `codebase-memory-mcp bridge`. Normalise en tools/call.
    if req.get("tool").is_some() && req.get("method").is_none() {
        let tool = req
            .get("tool")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow::anyhow!("bridge sans tool"))?;
        let args = req.get("args").cloned().unwrap_or(Value::Null);
        let (content, is_error) = call_tool(db, tool, &args);
        // CONTRAT DU GRAPHINJECTOR (le client bridge du harness) : la
        // réponse est lue à la RACINE — `parsed.structuredContent` et
        // `parsed.isError` (pas de wrapper jsonrpc/result). Le moteur C
        // répond `{content, structuredContent, isError}` en racine.
        let mut result = json!({
            "content": [ { "type": "text", "text": content.to_string() } ],
            "isError": is_error,
        });
        if !is_error {
            result["structuredContent"] = content;
        }
        return Ok(Some(result));
    }

    let method = req
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow::anyhow!("requête sans méthode"))?;
    let id = req.get("id").cloned();
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                // ── L'ÉTAT D'ACCÈS EST PUBLIÉ ICI AUSSI ─────────────────────
                // Le pont n'a pas de terminal : ses messages vont dans le flux
                // JSON-RPC. `initialize` est donc le seul endroit où il peut
                // DIRE sous quelle condition il sert le graphe.
                "iam": crate::viz_serve::GardeIam::partagee().etat(),
                "iam_armee": crate::viz_serve::GardeIam::partagee().est_armee(),
            },
        }))),
        "notifications/initialized" => Ok(None),
        "ping" => Ok(Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} }))),
        "tools/list" => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tools_list() },
        }))),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| anyhow::anyhow!("tools/call sans name"))?;
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            let (content, is_error) = call_tool(db, name, &arguments);
            let mut result = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [ { "type": "text", "text": content.to_string() } ],
                    "isError": is_error,
                    "structuredContent": content,
                },
            });
            if is_error {
                // ── UN REFUS GARDE SA STRUCTURE ─────────────────────────────
                // L'enveloppe écrasait `structuredContent` par `{"error": <chaîne>}` :
                // tout échec d'outil arrivait aplati, donc inexploitable — un
                // refus IAM porteur de `http`, `espace` et `acces` devenait une
                // phrase. On garde l'INVARIANT (le champ `error` reste une
                // chaîne) et on conserve les autres champs du corps.
                result["result"]["structuredContent"] = match &content {
                    Value::Object(champs) => {
                        let mut m = champs.clone();
                        if let Some(e) = m.get("error") {
                            let texte = e.as_str().map_or_else(|| e.to_string(), str::to_string);
                            m.insert("error".to_string(), Value::String(texte));
                        }
                        Value::Object(m)
                    }
                    autre => json!({ "error": autre.to_string() }),
                };
            }
            Ok(Some(result))
        }
        _ => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("méthode inconnue: {method}") },
        }))),
    }
}

/// Le catalogue des outils (ceux que le harness appelle).
fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "search_graph",
            "description": "Recherche structurée sur le graphe code (BM25 ou sémantique)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "query": { "type": "string" },
                    "semantic_query": { "type": "array", "items": { "type": "string" } },
                    "label": { "type": "string" },
                    "file_pattern": { "type": "string" },
                    "limit": { "type": "integer" },
                    "offset": { "type": "integer" },
                    "format": { "type": "string" },
                },
            },
        }),
        json!({
            "name": "runtime_search",
            "description": "Recherche BM25 sur le contenu runtime (sessions, messages)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "query": { "type": "string" },
                    "limit": { "type": "integer" },
                },
            },
        }),
        json!({
            "name": "sessions_list",
            "description": "Liste les sessions du graphe runtime",
            "inputSchema": {
                "type": "object",
                "properties": { "project": { "type": "string" } },
            },
        }),
        json!({
            "name": "context_build",
            "description": "Injection composée pour une session (goals, summaries, weave, code)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "session_id": { "type": "string" },
                    "limit": { "type": "integer" },
                    "owner": { "type": "string" },
                    "subject": { "type": "string" },
                    "focus_file": { "type": "string" },
                    "code_project": { "type": "string" },
                },
            },
        }),
        json!({
            "name": "checkpoint_material",
            "description": "Matériau d'un checkpoint de compaction piloté par le graphe : faits déjà extraits d'une session (goals, checkpoints antérieurs is_compaction, fichiers TOUCHES agrégés, premiers/derniers messages bornés) sans rejouer l'historique brut — base du résumé de compaction du harness",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "session_id": { "type": "string" },
                    "limit": { "type": "integer" },
                    "owner": { "type": "string" },
                },
            },
        }),
        json!({
            "name": "import_events",
            "description": "Importe des événements runtime dans le graphe (écriture idempotente)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "session_id": { "type": "string" },
                    "events": { "type": "array" },
                },
            },
        }),
        json!({
            "name": "session_load",
            "description": "Charge une session (messages + événements, paginé) — le canal des conversations",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "session_id": { "type": "string" },
                    "events_only": { "type": "boolean" },
                    "events_limit": { "type": "integer" },
                    "events_after_seq": { "type": "integer" },
                    "events_exclude_chunks": { "type": "boolean" },
                },
            },
        }),
        json!({
            "name": "journal_import",
            "description": "Importe le journal brut d'une session dans la zone substrate (migration MCP)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "lines": { "type": "array", "items": { "type": "string" } },
                },
            },
        }),
        json!({
            "name": "memory_register",
            "description": "Marque une co-activation explicite (un épisode engram) — l'agent influence sa mémoire au fil du travail. nodes = ids des nœuds co-activés, prediction_error optionnel.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "nodes": { "type": "array", "items": { "type": "integer" } },
                    "prediction_error": { "type": "number" },
                },
            },
        }),
        json!({
            "name": "memory_recall",
            "description": "Rappelle les engrams pertinents pour des cues (ids de nœuds) — la mémoire « ce qui marche ensemble ».",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "cues": { "type": "array", "items": { "type": "integer" } },
                    "limit": { "type": "integer" },
                },
            },
        }),
        json!({
            "name": "predict",
            "description": "Prédit l'embedding suivant (centroïde spectral pondéré des engrams rappelés) + la surprise — la couche prédictive JEPA. La surprise est le signal qui module l'exploration de la couche de décision (postérieur Beta intact).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "cues": { "type": "array", "items": { "type": "integer" } },
                    "limit": { "type": "integer" },
                },
                "required": ["cues"],
            },
        }),
        json!({
            "name": "get_graph_schema",
            "description": "Labels, types d'arêtes et clés de propriétés du graphe",
            "inputSchema": {
                "type": "object",
                "properties": { "project": { "type": "string" } },
            },
        }),
        // (2026-09-04) OUTIL GÉNÉRIQUE — TOUTES LES SYNTAXES du moteur
        // deviennent accessibles via le bridge : GQL natif (dialecte ISO,
        // CALL procédures inclus — obrain.cognitive.*, obrain.engrams.*,
        // obrain.procedures(), registre d'algorithmes…), et tout dialecte
        // compilé (cypher, sql-pgq, …) via `language`. Même pipeline que la
        // CLI `obrain query` : Session::execute_language → QueryResult
        // sérialisé en {columns, rows, total}.
        json!({
            "name": "query",
            "description": "Exécute UNE requête dans n'importe quel dialecte du moteur (défaut GQL — toutes les syntaxes, CALL inclus : obrain.procedures(), obrain.cognitive.*, obrain.engrams.*, algorithmes pagerank/leiden/scc/articulation…). Réponse : {columns, rows, total}. language: gql (défaut) | cypher | sql | sparql | gremlin | graphql (selon features compilées). params: objet $nom → valeur. limit: cap optionnel du nombre de lignes renvoyées (défaut 10000).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Requête à exécuter (GQL/Cypher/…)" },
                    "language": { "type": "string", "description": "Dialecte : gql (défaut) | cypher | sql | sparql | gremlin | graphql" },
                    "params": { "type": "object", "description": "Paramètres nommés $x de la requête" },
                    "limit": { "type": "integer", "description": "Cap du nombre de lignes renvoyées (défaut 10000)" },
                },
                "required": ["query"],
            },
        }),
        json!({
            "name": "query_graph",
            "description": "Exécute une requête Cypher sur le graphe (parité codebase-memory query_graph). Réponse structurée : {columns, rows, total}. project accepté pour compatibilité d'appel (le bridge est lié au store OBRAIN_STORE) ; max_rows borne les lignes (défaut 10000) ; format 'json' par défaut — le layout texte TOON du C n'est pas reproduit (structuredContent JSON = équivalent ou meilleur).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "query": { "type": "string" },
                    "graph": { "type": "string" },
                    "max_rows": { "type": "integer" },
                    "format": { "type": "string" },
                },
                "required": ["query"],
            },
        }),
        json!({
            "name": "get_code_snippet",
            "description": "Renvoie le code source d'un symbole (parité codebase-memory) : qualified_name exact, ou name court → suggestions. Lit le fichier via file_path/start_line/end_line persistés par obrain-indexer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "qualified_name": { "type": "string" },
                    "name": { "type": "string" },
                },
            },
        }),
        json!({
            "name": "trace_path",
            "description": "Trace les chemins d'appels (parité codebase-memory, mode calls) depuis un symbole : qualified_name (ou name). direction inbound|outbound|both (défaut both), depth 1-5 (défaut 3), limit (défaut 50). Retourne des lignes [from, type, to, hop]. Modes data_flow/cross_service : non portés.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "qualified_name": { "type": "string" },
                    "function_name": { "type": "string" },
                    "name": { "type": "string" },
                    "direction": { "type": "string" },
                    "mode": { "type": "string" },
                    "depth": { "type": "integer" },
                    "limit": { "type": "integer" },
                },
            },
        }),
        json!({
            "name": "structural_roles",
            "description": "Profil structurel d'un nœud (parité codebase-memory) : node = qualified_name OU id numérique. Renvoie scc_id, scc_size, in_cycle (scc_size > 1), is_articulation (graphe CALLS non dirigé), et la liste des membres de sa composante.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node": { "type": "string" },
                    "qualified_name": { "type": "string" },
                },
                "required": ["node"],
            },
        }),
        json!({
            "name": "viz_subgraph",
            "description": "Sous-graphe rendable autour d'un nœud (couche de restitution obrain) : center = id numérique OU qualified_name. Un appel = un sous-graphe rendable : {stats, truncated, center_qn, overlay_report?, graph: json-graph v1 (nodes/edges, signaux energy/ricci/weight/scc), ou texte dot/mermaid si format demandé}.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "center": { "type": "string", "description": "Nœud centre : id numérique ou qualified_name exact" },
                    "hops": { "type": "integer", "description": "Sauts 1..3 (défaut 2)" },
                    "limit": { "type": "integer", "description": "Cap global de nœuds (0 = aucun, défaut)" },
                    "edge_types": { "type": "string", "description": "Types d'arêtes inclus, séparés par des virgules (ex. CALLS,IMPORTS) — vide = tous" },
                    "outbound_only": { "type": "boolean", "description": "N'étendre que vers les arêtes sortantes" },
                    "inbound_only": { "type": "boolean", "description": "N'étendre que vers les arêtes entrantes" },
                    "no_signals": { "type": "boolean", "description": "Ne pas lire les signaux persistés (scc/energy/ricci/weight)" },
                    "overlay": { "type": "string", "description": "Overlay cognitif : ricci | energy | roles (attache color_hint/size_hint aux nœuds/arêtes)" },
                    "format": { "type": "string", "description": "json (défaut, json-graph v1) | dot | mermaid" },
                },
                "required": ["center"],
            },
        }),
        json!({
            "name": "viz_export",
            "description": "Export du store ENTIER en un artefact (couche de restitution obrain) : {stats, graph: json-graph v1}, ou texte dot/mermaid si format demandé. Overlay optionnel ricci|energy|roles.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "edge_types": { "type": "string", "description": "Types d'arêtes inclus, séparés par des virgules — vide = tous" },
                    "no_signals": { "type": "boolean" },
                    "overlay": { "type": "string", "description": "ricci | energy | roles" },
                    "format": { "type": "string", "description": "json (défaut) | dot | mermaid" },
                },
            },
        }),
        json!({
            "name": "node_curvature",
            "description": "Lecture bridge (P0-v05) : courbure nodale agrégée (moyenne pondérée du ricci incident, lue depuis ricci_u8 persisté — fraîcheur à garantir via enrich --ricci) + energy (Q1.15) d'un nœud. node = id numérique OU qualified_name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "id numérique ou qualified_name exact" },
                },
                "required": ["node"],
            },
        }),
        json!({
            "name": "viz_topo",
            "description": "Lecture topologique bridge (P0-v05) : triangles CALLS non dirigés autour d'un centre (calculés en Rust sur le sous-graphe extrait) — renvoie les incidences triangulaires par nœud (chaque triangle compté 3×) + top 20 nœuds. center = id OU qualified_name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "center": { "type": "string" },
                    "hops": { "type": "integer", "description": "Sauts 1..3 (défaut 2)" },
                    "limit": { "type": "integer", "description": "Cap de nœuds du sous-graphe (défaut 200)" },
                },
                "required": ["center"],
            },
        }),
        json!({
            "name": "list_projects",
            "description": "Liste les projets du store courant (nœuds Project, parité codebase-memory list_projects sur substrate). Réponse : {projects:[{name, qualified_name}], total}.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer" },
                },
            },
        }),
        json!({
            "name": "search_code",
            "description": "Recherche de code enrichie graphe (parité codebase-memory search_code) : pattern → fonctions classées (BM25 sur Function.text/name), réponse tuples [qn, label, file, lines, rank] (+ file_path/start_line/end_line).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "query": { "type": "string" },
                    "limit": { "type": "integer" },
                    "file_pattern": { "type": "string" },
                },
            },
        }),
        json!({
            "name": "index_status",
            "description": "État du store (parité codebase-memory index_status) : total nœuds/arêtes + comptage par label. (Les métadonnées miss-graph/parse_partial ne sont pas persistées par obrain-indexer pour l'instant.)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                },
            },
        }),
        json!({
            "name": "graph_link",
            "description": "Crée un lien arbitraire entre deux nœuds identifiés par qualified_name (upsert : crée les nœuds si absents, ajoute l'arête — parité codebase-memory graph_link). Requiert un store en écriture (OBRAIN_WRITE=1).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": { "type": "object" },
                    "target": { "type": "object" },
                    "type": { "type": "string" },
                    "properties": { "type": "object" },
                },
                "required": ["source", "target", "type"],
            },
        }),
        json!({
            "name": "payload_get",
            "description": "Lit les propriétés (payload) d'un nœud par qualified_name (parité payload_get).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "qn": { "type": "string" },
                },
                "required": ["qn"],
            },
        }),
        json!({
            "name": "list_docs",
            "description": "Liste les nœuds Skill/Plan du store (parité list_docs).",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "ingest_traces",
            "description": "Ingère des traces d'appels {caller, callee, count} : upsert des nœuds + arêtes CALLS (parité ingest_traces). Requiert OBRAIN_WRITE=1.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "traces": { "type": "array" },
                },
            },
        }),
        json!({
            "name": "impact_set",
            "description": "Impact transitif (BFS multi-sources CALLS) depuis une liste de seeds : seeds=[qns] OU node=qn. direction inbound (appelants, défaut) | outbound | both ; depth 1-5 ; rows [from,type,to,hop].",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node": { "type": "string" },
                    "seeds": { "type": "array", "items": { "type": "string" } },
                    "direction": { "type": "string" },
                    "depth": { "type": "integer" },
                    "limit": { "type": "integer" },
                },
            },
        }),
        json!({
            "name": "index_repository",
            "description": "Indexe un dépôt (tree-sitter M6) dans le store courant (parité index_repository) — nécessite un build avec la feature indexer et un store en écriture. args : path (dépôt).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "repo": { "type": "string" },
                },
                "required": ["path"],
            },
        }),
        json!({
            "name": "create_property_index",
            "description": "Crée l'index d'égalité substrate sur une propriété (ex. qualified_name) — rend les lookups d'égalité O(1)/µs (parité « mieux » : index hash natif substrate).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "property": { "type": "string" },
                },
                "required": ["property"],
            },
        }),
        // ─────────────────────────────────────────────────────────────────────
        // Lot C (ingestion, parité codebase-memory) — outils d'import de
        // registres / archives RUNTIME (P1, 2026-09-08). Ils lisent un fichier
        // JSON depuis le process (chemin absolu dans args) et écrivent des
        // nœuds/arêtes dans le store courant (upsert par qualified_name, même
        // mécanique que `graph_link` / `merge_node`).
        // ─────────────────────────────────────────────────────────────────────
        json!({
            "name": "import_owners",
            "description": "Importe un registre session→compte {sessionId: accountId} (fichier JSON) : nœuds Account (qn `account:<id>`) + arêtes Session-[:OWNED_BY]->Account (upsert par qn — les Session absentes sont créées ; préfixe `session-` ajouté si absent, même normalisation qu'import_events). Requiert un store en écriture.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "registry": { "type": "string", "description": "Chemin du fichier JSON {sessionId: accountId}" },
                },
                "required": ["registry"],
            },
        }),
        json!({
            "name": "import_workspace",
            "description": "Importe un registre de workspaces {id: {path, title, sessionIds}} (fichier JSON) : nœuds Workspace (qn `workspace:<id>`, props path/title) + arêtes Session-[:WORKSPACE]->Workspace. Upsert par qn ; Session absentes créées. Requiert un store en écriture.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "registry": { "type": "string", "description": "Chemin du fichier JSON {id: {path, title, sessionIds}}" },
                },
                "required": ["registry"],
            },
        }),
        json!({
            "name": "import_docs",
            "description": "Importe des skills/plans en nœuds Skill/Plan (qn `skill:<chemin>` / `plan:<chemin>`, prop payload = contenu complet) : soit un fichier unique {file, kind: Skill|Plan}, soit un répertoire {skills_dir|plans_dir} parcouru récursivement (*.md ; chemin = relatif au répertoire racine, extension retirée). Requiert un store en écriture.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Fichier unique à importer (mode fichier)" },
                    "kind": { "type": "string", "description": "Skill (défaut) | Plan — mode fichier unique" },
                    "skills_dir": { "type": "string", "description": "Répertoire racine des skills (parcours récursif *.md → Skill)" },
                    "plans_dir": { "type": "string", "description": "Répertoire racine des plans (parcours récursif *.md → Plan)" },
                },
            },
        }),
        json!({
            "name": "import_files",
            "description": "Importe les fichiers produits par l'agent en nœuds File (qn `file:<chemin>` par défaut, prop payload = contenu INTÉGRAL + file_path + name) : {files:[{path, qn?}], session_id?}. Upsert par qn = attachement : ré-importer un fichier modifié remplace son contenu. Arête Session→PRODUCED→File quand session_id est fourni et résolu. ADDITIF : aucun fichier supprimé/renommé/déplacé sur disque. Requiert un store en écriture.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Session produisant les fichiers (arête PRODUCED), optionnel" },
                    "files": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "qn": { "type": "string" },
                            },
                            "required": ["path"],
                        },
                    },
                },
                "required": ["files"],
            },
        }),
        json!({
            "name": "import_sessions",
            "description": "Importe des archives de session `.jsonl` NON compressées d'un répertoire (parcours récursif) comme événements runtime via le canal import_events : nœuds Session/Message/ToolCall + journal brut, idempotent par (session, seq). `.zstd` NON supporté ici → erreur explicite (décompressez d'abord). Version honnête et bornée (lots de ≤2000 événements, caps par fichier) — pas la perfection du moteur C.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dir": { "type": "string", "description": "Répertoire contenant les archives session.jsonl" },
                },
                "required": ["dir"],
            },
        }),
        json!({
            "name": "manage_adr",
            "description": "Lit/écrit une Architecture Decision Record comme nœud graphe label ADR (qn `adr:<project>/<path>`), prop content — AUCUN fichier créé sur disque. modes : get (défaut) → renvoie content ; update (alias store) → pose/remplace content ; sections → renvoie les titres `## ` du content. project défaut 'default'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "description": "get (défaut) | update | sections" },
                    "project": { "type": "string" },
                    "path": { "type": "string", "description": "Chemin/nom de l'ADR (alias : id)" },
                    "id": { "type": "string" },
                    "content": { "type": "string", "description": "Contenu markdown complet (mode update)" },
                },
                "required": ["path"],
            },
        }),
        // ─────────────────────────────────────────────────────────────────────
        // DÉCISIONS ENREGISTRÉES EN CODE — outils de l'ancien serveur C que le
        // moteur actuel ne peut PAS servir fidèlement (donc PAS ajoutés ici
        // comme outils qui échoueraient toujours) :
        //
        // - detect_changes      : exige un diff git (base_branch/since) + le
        //   dépôt fraîchement indexé pour résoudre fichiers → symboles → impact
        //   transitif. Le bridge est lié à un store ; équivalent documenté :
        //   `index_repository` (rafraîchir le projet) puis `impact_set` sur les
        //   qn des symboles des fichiers modifiés (requête File.file_path →
        //   trace_path inbound). Un wrapper git+traversal serait un chantier
        //   dédié (P1 ultérieur), pas un wrapper fin.
        //
        // - get_architecture    : profils agrégés (structure, layers, clusters
        //   Leiden, cliques, cycles…) sur le graphe CALLS/import. Les briques
        //   existent (obrain.procedures() expose pagerank/leiden/scc/… via
        //   `query`) mais l'agrégation multi-aspects par répertoire est un
        //   chantier de composition — « à venir », exprimable partiellement
        //   aujourd'hui via `query` (CALL obrain.leiden() …).
        //
        // - cliques_of /
        //   hyperedges_of       : tables matérialisées triangles/K4/K5 et
        //   hyperarêtes n-aires (DISPATCH) publiées par les passes topologiques
        //   du moteur C — pas de telles tables côté substrate bridge. Exigerait
        //   des passes topo moteur (triangles/cliques) non exposées.
        //
        // - delete_project      : suppression d'une partition/projet (métadonnées
        //   Project + sous-graphe) — gestion multi-projets du store, pas d'API
        //   de suppression de partition côté bridge. Équivalent partiel :
        //   recréer le store / requêtes de purge ciblées.
        //
        // - weave_retisser      : ré-résolution des arêtes TOUCHES locales
        //   (File cross-projet) vers les vrais nœuds après indexation — passe
        //   moteur (résolution différée), pas d'équivalent store-local.
        //
        // - check_index_coverage: métadonnées de couverture d'indexation par
        //   fichier (parse_partial/skipped/miss-graph). obrain-indexer ne les
        //   persiste pas dans le store → couvert par `index_status` (qui note
        //   explicitement cette absence) ; rien à servir de plus ici.
        // ─────────────────────────────────────────────────────────────────────
    ]
}

/// Réponse outil « vide » : tableau `rows`/`results` vide, ou `total` = 0.
/// Ne regarde QUE les champs de résultat connus du bridge — un objet sans
/// aucun de ces champs n'est pas considéré vide (on ne devine pas).
fn response_is_empty(content: &Value) -> bool {
    let arr_empty = |k: &str| {
        content
            .get(k)
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
    };
    if let Some(e) = arr_empty("rows").or_else(|| arr_empty("results")) {
        return e;
    }
    content.get("total").and_then(|v| v.as_u64()) == Some(0)
}

/// Next-step hint (ergonomie LLM) : une phrase impérative annexée à la
/// réponse des outils PRINCIPAUX pour guider l'agent vers l'outil suivant
/// pertinent (inspiré GitNexus). Contrat :
/// - jamais appelé sur une réponse d'erreur (garde au point d'appel) ;
/// - résultat vide → orienter vers la découverte du schéma/labels ;
/// - additif strict : champ JSON `hint` optionnel, jamais écrasé s'il est
///   déjà posé par l'outil (ex. `query_graph` sur résultat vide) ;
/// - anglais, comme le hint existant de `query_graph` (face LLM).
fn next_step_hint(tool: &str, content: &Value) -> Option<String> {
    let empty = response_is_empty(content);
    let hint = match tool {
        "query" | "query_graph" => {
            if empty {
                "No rows returned. Call get_graph_schema() to check the actual labels, edge types and property keys, then refine the query."
            } else {
                "Next: fetch a symbol's source with get_code_snippet(qualified_name) or follow relations with trace_path."
            }
        }
        "search_graph" | "search_code" => {
            if empty {
                "No matches. Call get_graph_schema() to verify labels/properties, or broaden the search with query (GQL CONTAINS)."
            } else {
                "Next: read a hit's source with get_code_snippet(qualified_name) or map its call chain with trace_path."
            }
        }
        "runtime_search" => {
            if empty {
                "No runtime hits. List available sessions with sessions_list(), or check labels with get_graph_schema()."
            } else {
                "Next: load the surrounding conversation with session_load(qualified_name)."
            }
        }
        "get_graph_schema" => {
            "Next: query these labels with query (GQL), e.g. MATCH (n:Label) RETURN n LIMIT 10."
        }
        "index_status" => {
            let no_nodes = content.get("total_nodes").and_then(|v| v.as_u64()) == Some(0);
            if no_nodes {
                "Store is empty. Run index_repository to index a codebase first."
            } else {
                "Next: list labels with get_graph_schema() or search symbols with search_graph."
            }
        }
        _ => return None,
    };
    Some(hint.to_string())
}

/// Les outils du pont qui ÉCRIVENT. Le partage lecture/écriture suit l'effet
/// réel : `graph_link`, les imports, l'indexation et l'enregistrement de
/// mémoire modifient le graphe ; le reste observe.
fn outil_ecrivant(nom: &str) -> bool {
    matches!(
        nom,
        "graph_link"
            | "ingest_traces"
            | "import_events"
            | "journal_import"
            | "import_docs"
            | "import_files"
            | "import_owners"
            | "import_workspace"
            | "index_repository"
            | "create_property_index"
            | "memory_register"
    )
}

/// L'espace du pont : le nom de la base qu'il sert. Le pont est lié à UN store
/// (résolu par `OBRAIN_STORE` ou son argument) : son espace est donc stable, et
/// une politique émise pour ce nom vaut aussi pour le serveur HTTP, qui nomme
/// l'espace de la même façon (le radical du fichier).
/// Plafond d'un résumé de compaction injecté, en caractères.
///
/// Aligné sur les autres bornes d'extrait du dépôt (restitution du harness :
/// 600 ; checkpoint de compaction : 800-3000). Ce n'est pas un réglage de
/// confort : c'est ce qui empêche UN checkpoint de manger le budget de tous les
/// autres.
const RESUME_COMPACTION_MAX: usize = 2_000;

/// Tronque un texte à `max` caractères et MARQUE la coupe.
///
/// Le marqueur est le même que celui de la restitution (`… [tronqué]`) : un
/// lecteur — humain ou modèle — doit pouvoir distinguer « la fin » d'« une
/// coupe », sinon il conclut sur un extrait comme s'il avait tout.
fn tronquer_avec_marque(texte: &str, max: usize) -> String {
    if texte.chars().count() <= max {
        return texte.to_string();
    }
    let coupe: String = texte.chars().take(max).collect();
    format!(
        "{coupe}… [tronqué : {} caractères sur {}]",
        max,
        texte.chars().count()
    )
}

fn espace_du_pont() -> String {
    std::env::var("OBRAIN_STORE")
        .ok()
        .map(std::path::PathBuf::from)
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "base-inconnue".to_string())
}

/// Route un outil vers l'API moteur. Retourne (structuredContent, is_error).
fn call_tool(db: &ObrainDB, name: &str, arguments: &Value) -> (Value, bool) {
    // ── LA GARDE PASSE AVANT TOUT OUTIL ─────────────────────────────────────
    // Le pont est la porte du HARNESS : c'est par ici qu'un agent lit et écrit
    // le graphe, et l'écrivain (GraphWriter) travaille par ce même binaire.
    // Audit du 2026-09-13 : il ne demandait aucune identité.
    //
    // (Ce câblage avait été emporté par une écriture concurrente de ce fichier ;
    // il est reposé ici.)
    //
    // Le jeton vient de `OBRAIN_IAM_TOKEN` : un pont parle sur son entrée
    // standard, il n'a pas d'en-tête HTTP à lire, et c'est le processus qui le
    // lance (le harness) qui porte l'identité.
    {
        use crate::viz_serve::{GardeIam, gql_ecrivante, jeton_du_pont};
        let ecriture = outil_ecrivant(name)
            || matches!(name, "query" | "query_graph")
                && arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .is_some_and(gql_ecrivante);
        let espace = espace_du_pont();
        let garde = GardeIam::partagee();
        if let Err((code, raison)) = garde.controler(jeton_du_pont().as_deref(), ecriture, &espace)
        {
            // Champs à PLAT : un client doit pouvoir lire `http` et `acces`
            // sans re-décoder une chaîne.
            return (
                serde_json::json!({
                    "error": raison,
                    "outil": name,
                    "acces": if ecriture { "écriture" } else { "lecture" },
                    "espace": espace,
                    "http": code,
                    "iam": garde.etat(),
                }),
                true,
            );
        }
    }
    // AutoOpt : horodatage interne autour du dispatch → wall_us du logger.
    let start = std::time::Instant::now();
    let (content, is_error) = match name {
        "search_graph" => search_graph(db, arguments),
        "runtime_search" => runtime_search(db, arguments),
        "sessions_list" => sessions_list(db, arguments),
        "context_build" => context_build(db, arguments),
        "checkpoint_material" => checkpoint_material(db, arguments),
        "import_events" => import_events(db, arguments),
        "journal_import" => journal_import(db, arguments),
        "session_load" => session_load(db, arguments),
        "memory_register" => memory_register(db, arguments),
        "memory_recall" => memory_recall(db, arguments),
        "predict" => predict(db, arguments),
        "get_graph_schema" => get_graph_schema(db, arguments),
        "query" => execute_query(db, arguments),
        "query_graph" => query_graph(db, arguments),
        "get_code_snippet" => get_code_snippet(db, arguments),
        "trace_path" => trace_path(db, arguments),
        "structural_roles" => structural_roles(db, arguments),
        "list_projects" => list_projects(db, arguments),
        "search_code" => search_code(db, arguments),
        "index_status" => index_status(db, arguments),
        "graph_link" => graph_link(db, arguments),
        "payload_get" => payload_get(db, arguments),
        "list_docs" => list_docs(db, arguments),
        "ingest_traces" => ingest_traces(db, arguments),
        "impact_set" => impact_set(db, arguments),
        "index_repository" => index_repository(db, arguments),
        "create_property_index" => create_property_index(db, arguments),
        "import_owners" => import_owners(db, arguments),
        "import_workspace" => import_workspace(db, arguments),
        "import_docs" => import_docs(db, arguments),
        "import_files" => import_files(db, arguments),
        "import_sessions" => import_sessions(db, arguments),
        "manage_adr" => manage_adr(db, arguments),
        "viz_subgraph" => viz_subgraph(db, arguments),
        "viz_export" => viz_export(db, arguments),
        "node_curvature" => node_curvature_tool(db, arguments),
        "viz_topo" => viz_topo(db, arguments),
        other => (
            json!({ "status": "error", "error": format!("outil inconnu: {other}") }),
            true,
        ),
    };
    // Next-step hint (ergonomie LLM) : annexé aux réponses de SUCCÈS des
    // outils principaux — additif (champ `hint`), jamais sur erreur, jamais
    // par-dessus un hint posé par l'outil lui-même.
    let mut content = content;
    if !is_error {
        if let Some(hint) = next_step_hint(name, &content) {
            if let Some(obj) = content.as_object_mut() {
                obj.entry("hint").or_insert_with(|| json!(hint));
            }
        }
    }
    // AutoOpt : auto-mesure best-effort des outils de lecture (voir
    // `metrics_append`) — coût nul quand OBRAIN_METRICS_FILE n'est pas défini.
    if METRICS_READ_TOOLS.contains(&name) {
        metrics_append(name, arguments, &content, start.elapsed().as_micros());
    }
    (content, is_error)
}

// ── AutoOpt : auto-mesure du bridge (MVP, 2026-09) ───────────────────────────
// Si `OBRAIN_METRICS_FILE=<chemin>` est défini, après chaque appel d'un outil
// de LECTURE (query, query_graph + surface d'analyse), on append une ligne
// JSON `{"ts":<ms epoch>,"tool":<nom>,"engine_us":<execution_time_ms×1000 si
// présent>,"wall_us":<délai interne>,"q":<préfixe 60 car. de la requête>}` au
// fichier (OpenOptions create/append). Le champ `q` permet de regrouper par
// forme (aucune donnée hors requête n'est journalisée). Coût négligeable
// (env::var + append best-effort), et ABSENT quand l'env n'est pas défini.
const METRICS_READ_TOOLS: &[&str] = &[
    "query",
    "query_graph",
    "search_graph",
    "search_code",
    "runtime_search",
    "get_graph_schema",
    "get_code_snippet",
    "trace_path",
    "structural_roles",
    "impact_set",
    "payload_get",
    "sessions_list",
    "session_load",
    "context_build",
    "checkpoint_material",
    "list_projects",
    "list_docs",
    "index_status",
    "memory_recall",
];

/// Le chemin du journal de mesures : la variable d'environnement, ou le
/// forçage posé par un test.
///
/// ── POURQUOI CE DÉTOUR EXISTE ───────────────────────────────────────────────
/// Le test de ce journal écrivait `OBRAIN_METRICS_FILE` dans l'environnement du
/// PROCESSUS pendant que d'autres tests tournaient dans le même binaire. C'est
/// exactement ce que la bibliothèque standard interdit (une mutation
/// d'environnement concurrente d'une lecture n'est pas définie), et c'était
/// MESURÉ : un échec sur environ six exécutions complètes, sans qu'aucune ligne
/// du code mesuré ait changé. Un test qui dépend de l'environnement de celui qui
/// le lance — et qui le modifie pour les autres — ne prouve rien.
///
/// Le forçage remplace la mutation globale par un état de test, sans toucher à
/// la variable : `Some(None)` veut dire « pas de journal » même si
/// l'environnement en demande un, ce qui rend les DEUX phases hermétiques.
fn chemin_metriques() -> Option<String> {
    // ── LE FORÇAGE DE TEST EST PAR THREAD, ET C'EST UNE CORRECTION ──────────
    // Défaut mesuré le 2026-09-17, sous la charge d'une suite complète : un
    // forçage GLOBAL (un `Mutex<Option<…>>` partagé) faisait écrire TOUS les
    // tests du binaire dans le fichier que ce test-ci observait. Les appends de
    // plusieurs threads s'entrelaçaient, la lecture tombait au milieu d'une
    // écriture, et l'enregistrement relu était illisible — un échec qui
    // n'apprenait rien sur le produit, seulement sur le harnais.
    //
    // Par thread, le forçage ne concerne que les appels DU test qui l'a posé :
    // les autres tests gardent l'environnement (absent), donc n'écrivent nulle
    // part. C'est aussi plus juste : la mesure d'un test n'a jamais eu besoin
    // d'être globale.
    #[cfg(test)]
    if let Some(forcage) = OVERRIDE_METRIQUES_THREAD.with(|c| c.borrow().clone()) {
        return forcage;
    }
    std::env::var("OBRAIN_METRICS_FILE").ok()
}

/// Forçage de test, PAR THREAD : `None` = pas de forçage (l'environnement
/// décide), `Some(None)` = aucun journal, `Some(Some(chemin))` = ce journal.
#[cfg(test)]
thread_local! {
    static OVERRIDE_METRIQUES_THREAD: std::cell::RefCell<Option<Option<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// Pose (ou retire) le forçage de test du thread courant.
#[cfg(test)]
fn forcer_metriques(valeur: Option<Option<String>>) {
    OVERRIDE_METRIQUES_THREAD.with(|c| *c.borrow_mut() = valeur);
}

fn metrics_append(name: &str, arguments: &Value, content: &Value, wall_us: u128) {
    let Some(path) = chemin_metriques() else {
        return; // env absent → aucun coût, aucun fichier
    };
    if path.trim().is_empty() {
        return;
    }
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut rec = json!({
        "ts": ts_ms,
        "tool": name,
        "wall_us": wall_us,
    });
    // `q` : préfixe 60 caractères de la requête — regroupement par forme
    // (les seuls outils dont le texte EST la forme analysée).
    if name == "query" || name == "query_graph" {
        if let Some(q) = arguments.get("query").and_then(|v| v.as_str()) {
            let clipped: String = q.chars().take(60).collect();
            rec["q"] = json!(clipped);
        }
    }
    // engine_us = execution_time_ms (mesure moteur, si présente) × 1000.
    if let Some(ms) = content.get("execution_time_ms").and_then(|v| v.as_f64()) {
        rec["engine_us"] = json!((ms * 1000.0).round() as u64);
    }
    // Best-effort : un échec d'écriture ne doit JAMAIS casser l'outil appelé.
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_str())
    {
        // UNE SEULE écriture par ligne. `writeln!` sur un fichier non tamponné
        // émet la ligne en plusieurs appels système ; plusieurs écrivains
        // (tests en parallèle, ou plusieurs ponts partageant le fichier)
        // entrelaçaient alors leurs MORCEAUX — mesuré : « ligne JSON
        // invalide » dans `metrics_logger_jsonl_quand_env_absent_rien`. Un
        // ajout (O_APPEND) d'une ligne entière en un appel ne se coupe pas.
        let mut ligne = rec.to_string();
        ligne.push('\n');
        let _ = f.write_all(ligne.as_bytes());
    }
}

fn arg_str(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn arg_usize(arguments: &Value, key: &str, default: usize) -> usize {
    arguments
        .get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(default)
}

/// Convertit un serde_json::Value en obrain_common::Value (propriété simple).
fn json_to_obrain_value(v: &serde_json::Value) -> obrain_common::types::Value {
    match v {
        serde_json::Value::String(s) => obrain_common::types::Value::from(s.clone()),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(obrain_common::types::Value::from)
            .or_else(|| n.as_f64().map(obrain_common::types::Value::from))
            .unwrap_or(obrain_common::types::Value::Null),
        serde_json::Value::Bool(b) => obrain_common::types::Value::from(*b),
        _ => obrain_common::types::Value::Null,
    }
}

/// Convertit un Value obrain_common en serde_json (via Display pour les scalaires).
fn to_json_value(v: &obrain_common::types::Value) -> Value {
    match v {
        obrain_common::types::Value::String(s) => Value::String(s.to_string()),
        obrain_common::types::Value::Int64(i) => json!(i),
        obrain_common::types::Value::Float64(f) => json!(f),
        obrain_common::types::Value::Bool(b) => json!(b),
        other => Value::String(other.to_string()),
    }
}

/// Récupère une propriété d'un nœud (HashMap<PropertyKey, Value>).
fn prop<'a>(props: &'a obrain_common::types::PropertyMap, key: &str) -> Value {
    props
        .iter()
        .find(|(k, _)| k.as_str() == key)
        .map(|(_, v)| to_json_value(v))
        .unwrap_or(Value::Null)
}

/// Récupère une propriété STRING d'un nœud — le `qn` des hits runtime doit
/// TOUJOURS être une string (contrat lossless-JSON de `brain_search`) : si la
/// propriété est absente/non-string, on retombe sur le repli fourni.
fn prop_str<'a>(props: &'a obrain_common::types::PropertyMap, key: &str, fallback: &str) -> String {
    props
        .iter()
        .find(|(k, _)| k.as_str() == key)
        .and_then(|(_, v)| match v {
            obrain_common::types::Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| fallback.to_string())
}

/// `search_graph` — BM25 via `text_search` (fallback : requête MATCH).
///
/// CONTRAT canal cbm (M4 : `CBM_BRIDGE_BIN=obrain` remplace codebase-memory
/// pour brain_search) : le tool `brain_search` (TS) déstructure chaque ligne
/// comme un TUPLE `[qn, label, file, lines, rank]` en mode `query`, et
/// `semantic.rows` comme `[qn, label, file, score]` en mode `semantic_query`.
/// Des OBJETS à la place des tuples donnent `qn: undefined` → le validateur
/// lossless-JSON rejette la valeur (« value is not lossless JSON »).
fn search_graph(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let limit = arg_usize(arguments, "limit", 10);
    let session = db.session();
    // Scoping projet (mono-store multi-projets) : `project` → préfixe du
    // qualified_name du nœud Project correspondant (recherche contient, insensible
    // à la casse). Sans match → erreur (parité codebase-memory).
    let project_prefix: Option<String> = {
        let want = arg_str(arguments, "project").filter(|s| !s.is_empty());
        match want {
            None => None,
            Some(p) => {
                let p_low = p.to_lowercase();
                let mut prefix: Option<String> = None;
                if let Ok(res) =
                    session.execute("MATCH (n:Project) RETURN n.qualified_name, n.name LIMIT 2000")
                {
                    for row in res.rows {
                        let hit = row.iter().any(|v| {
                            matches!(v, obrain_common::types::Value::String(s)
                                if s.to_lowercase().contains(&p_low))
                        });
                        if hit {
                            if let Some(obrain_common::types::Value::String(q)) = row.first() {
                                prefix = Some(q.to_string());
                            }
                            break;
                        }
                    }
                }
                if prefix.is_none() {
                    return (
                        json!({ "status": "error", "error": format!("search_graph: projet introuvable : {p}") }),
                        true,
                    );
                }
                prefix
            }
        }
    };
    if let Some(query) = arg_str(arguments, "query") {
        // BM25 sur Function.name (le label le plus peuplé du graphe code).
        match db.create_text_index("Function", "name") {
            Ok(_) | Err(_) => {}
        }
        match db.text_search("Function", "name", &query, limit) {
            Ok(hits) => {
                // Filtre par préfixe de projet (si demandé).
                let hits: Vec<_> = if let Some(pfx) = &project_prefix {
                    hits.into_iter()
                        .filter(|(nid, _)| {
                            session
                                .get_node(*nid)
                                .map(|nd| {
                                    prop_str(&nd.properties, "qualified_name", "")
                                        .starts_with(pfx.as_str())
                                })
                                .unwrap_or(false)
                        })
                        .collect()
                } else {
                    hits
                };
                let rows: Vec<Value> = hits
                    .iter()
                    .map(|(nid, score)| {
                        let props = session
                            .get_node(*nid)
                            .map(|n| n.properties)
                            .unwrap_or_default();
                        // qn = identité du symbole (qualified_name, repli name,
                        // repli id) — jamais un objet ; label = "Function" ;
                        // file/lines = STRINGS (le validateur lossless TS
                        // exige string ou absent, jamais null).
                        let qn = prop_str(
                            &props,
                            "qualified_name",
                            &prop_str(&props, "name", &nid.0.to_string()),
                        );
                        let file = prop_str(&props, "file_path", "");
                        json!([qn, "Function", file, "", score])
                    })
                    .collect();
                let total = hits.len();
                (
                    json!({
                        "status": "ok",
                        "total": total,
                        "cols": ["qn", "label", "file", "lines", "rank"],
                        "rows": rows,
                        "results": rows,
                        "has_more": false,
                    }),
                    false,
                )
            }
            Err(e) => (json!({ "status": "error", "error": e.to_string() }), true),
        }
    } else if let Some(_sem) = arguments.get("semantic_query").and_then(|v| v.as_array()) {
        // Mode cognitif : réponse vide au format cbm attendu (semantic.rows en
        // tuples [qn,label,file,score]) — la recherche vectorielle du graphe
        // code n'est pas branchée sur ce store.
        (
            json!({
                "status": "ok",
                "semantic": { "cols": ["qn", "label", "file", "score"], "rows": [] },
                "total": 0,
                "has_more": false,
            }),
            false,
        )
    } else {
        (
            json!({ "status": "error", "error": "search_graph: requête vide" }),
            true,
        )
    }
}

/// (2026-09-09, P2) Cache a chaud des index texte du runtime : trace
/// process-locale (base, cle "label:propriete") → generation (epoch de
/// commit) de la DERNIERE reconstruction. Meme patron que le memo
/// SESSIONS_LIST : etat par processus, jamais partage entre process — un
/// lecteur RO spawné par requete repart d'un registre vide et reconstruit
/// une fois (cout froid incompressible sans persistance disque de l'index).
static RUNTIME_TEXT_INDEX_GEN: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, std::collections::HashMap<String, u64>>>,
> = std::sync::OnceLock::new();

/// Garantit l'index texte (label, propriete) frais pour la generation donnee.
///
/// Reconstruit si et seulement si : l'index est ABSENT du registre (premier
/// appel du process, drop, ou reutilisation d'adresse de base), OU la
/// generation a avance depuis la derniere construction (des commits
/// d'ecriture sont survenus dans CE process). Correct avant rapide : tout
/// doute sur la fraicheur → rebuild (`create_text_index` rescanne tout le
/// label — c'est exactement ce qu'on cherche a eviter a chaud, pas a
/// supprimer quand c'est justifie).
///
/// Contrat de fraicheur (documente, P2) :
/// - process ecrivain (OBRAIN_WRITE=1, shared-write) : l'epoch substrate
///   avance a chaque commit GQL (`sync_epoch`, session/mod.rs) → un
///   runtime_search apres des ecritures reconstruit, comme avant ; entre
///   deux appels sans ecriture, l'index est reutilise (et les ecritures
///   CRUD de la couche moteur l'entretiennent incrementalement).
/// - lecteur RO : vue figee a l'ouverture, zero commit → generation
///   constante → UN rebuild par process, puis reutilisation. Un rebuild
///   ulterieur ne verrait rien de plus que la vue (le substrat du lecteur
///   n'avance pas) : la reutilisation n'introduit AUCUN decalage propre.
/// - limite assumee : la generation est process-locale (l'epoch substrate
///   n'est pas persistee) — elle ne detecte pas les ecritures d'un AUTRE
///   process arrivees pendant la vie du lecteur. Mais un lecteur RO ne peut
///   de toute facon pas les voir (vue d'ouverture, WAL rejoue a l'ouverture
///   seulement) : la fraicheur reelle reste bornee par le respawn du bridge,
///   exactement comme `sessions_list` cote RO. Rien n'est donc perdu par
///   rapport a l'ancien rebuild-par-appel.
fn runtime_ensure_text_index(db: &ObrainDB, label: &str, prop: &str, generation: u64) {
    let key = format!("{label}:{prop}");
    // Identite de la base : le pointeur ObrainDB est stable pour toute la vie
    // du process bridge (l'Arc est ouvert une fois dans `run`).
    let db_key = db as *const ObrainDB as usize;
    let memo = RUNTIME_TEXT_INDEX_GEN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let stale = {
        let guard = memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(&db_key).and_then(|m| m.get(&key)) {
            Some(last) if *last == generation => !db.has_text_index(label, prop),
            _ => true,
        }
    };
    if !stale {
        return;
    }
    // Verrou relache pendant le rebuild (~1,6 s a froid sur le store de
    // reference, rescan Message) : le bridge est sequentiel, mais ne pas
    // tenir le memo.
    if db.create_text_index(label, prop).is_ok() {
        let mut guard = memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.entry(db_key).or_default().insert(key, generation);
    }
}

/// `runtime_search` — BM25 sur le contenu runtime (télémetrie).
fn runtime_search(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let limit = arg_usize(arguments, "limit", 8);
    let Some(query) = arg_str(arguments, "query") else {
        return (
            json!({ "status": "error", "error": "runtime_search: query requis" }),
            true,
        );
    };
    // (2026-09-09, P2) BM25 sur les labels runtime — à CHAUD.
    //
    // AVANT : trois `create_text_index` à chaque requête. Le moteur REBUILD
    // intégralement l'index BM25 (rescan de tous les nœuds du label) sans
    // test d'existence : sur le store runtime de référence (~334 k nœuds,
    // dont 49 753 Message et 48 856 ToolCall), chaque runtime_search coûtait
    // ~1,6 s, à chaud comme à froid (mesuré — voir P2).
    //
    // MAINTENANT : reconstruction seulement si l'index est absent du
    // registre OU si la génération (epoch de commit, `db.current_epoch`) a
    // avancé depuis la dernière construction — voir
    // `runtime_ensure_text_index` et son contrat de fraîcheur. À chaud
    // (index présent, rien écrit depuis), seule la recherche BM25 reste
    // (~ms, cible DoD < 50 ms).
    //
    // ToolCall:name n'est CHERCHÉ par AUCUN outil (seuls Message:text et
    // Extraction:extraction_text le sont ici) : sorti du chemin chaud — un
    // index mort, rescan inutile à chaque appel. (La mesure montre que le
    // coût dominant du rebuild reste Message:text — textes longs — ; le
    // gain ToolCall est secondaire, le froid par process ~1,6 s demeure
    // tant que l'index n'est pas persistant.)
    let generation = db.current_epoch().as_u64();
    runtime_ensure_text_index(db, "Message", "text", generation);
    runtime_ensure_text_index(db, "Extraction", "extraction_text", generation);
    let session = db.session();

    // Fusion Message + Extraction (scores BM25, décroissant), puis cap limit.
    let mut hits: Vec<(obrain_common::types::NodeId, f64)> = Vec::new();
    if let Ok(message_hits) = db.text_search("Message", "text", &query, limit) {
        hits.extend(message_hits);
    }
    if let Ok(extraction_hits) = db.text_search("Extraction", "extraction_text", &query, limit) {
        hits.extend(extraction_hits);
    }
    hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(limit);

    let rows: Vec<Value> = hits
        .iter()
        .map(|(nid, score)| {
            let props = session
                .get_node(*nid)
                .map(|n| n.properties)
                .unwrap_or_default();
            let label = session
                .get_node(*nid)
                .map(|n| n.labels.join(","))
                .unwrap_or_default();
            let snippet = if label == "Extraction" {
                prop(&props, "extraction_text")
            } else {
                prop(&props, "text")
            };
            // CONTRAT canal cbm (M4 : CBM_BRIDGE_BIN=obrain remplace
            // codebase-memory pour brain_search) : le tool `brain_search` (TS)
            // lit `results[].{qn, label, rank, text}` — `qn`/`text` doivent
            // TOUJOURS être des strings présentes, sinon le validateur
            // lossless-JSON rejette la valeur (« value is not lossless JSON »).
            // `id`/`score`/`snippet` sont conservés en compat (additif strict).
            let qn = prop_str(&props, "qualified_name", &nid.0.to_string());
            json!({
                "id": nid.0.to_string(),
                "score": score,
                "label": label,
                "snippet": snippet,
                "qn": qn,
                "rank": score,
                "text": snippet,
            })
        })
        .collect();
    (
        json!({ "status": "ok", "total": rows.len(), "rows": rows, "results": rows }),
        false,
    )
}

/// `sessions_list` — liste des sessions (query sur les nœuds Session).
/// PRIVACY (U7) : quand `owner` est fourni, seules les sessions dont
/// l'arête OWNED_BY pointe vers ce compte sont servies — la garantie
/// stricte du harness (une session sans propriétaire n'est servie à
/// personne si les comptes sont actifs).

/// (2026-09-07) LECTEUR 100 % SUBSTRATE des lignes Session (chemin « toutes ») :
/// évite la projection de propriétés par le moteur GQL (~18 ms pour 227×7
/// props) — chaque propriété est lue nœud à nœud (`get_node_property`, µs).
/// Rendu strictement identique au contrat (voir `session_rows_to_contract`).
/// Colonnes de sortie : 0 qn, 1 cwd, 2 createdAt, 3 title, 4 source_path,
/// 5 agentPreset, 6 origin, 7 parentSession.
fn sessions_rows_from_substrate(
    sub: &obrain_substrate::SubstrateStore,
    limit: usize,
    offset: usize,
    possedes: Option<&std::collections::HashSet<String>>,
) -> (Vec<Vec<obrain_common::types::Value>>, usize) {
    use obrain_common::types::PropertyKey;
    use obrain_core::graph::GraphStore as _;
    // 8 lectures COLONNAIRES (une par clé, pour TOUS les nœuds) au lieu de
    // lectures unitaires (~5 µs/prop) — ramène la liste de ~10 ms à ~1 ms.
    const KEYS: [&str; 8] = [
        "qualified_name",
        "cwd",
        "createdAt",
        "title",
        "source_path",
        "agentPreset",
        "origin",
        "parentSession",
    ];
    let ids = sub.nodes_by_label("Session");
    let keys: Vec<PropertyKey> = KEYS.iter().map(|k| PropertyKey::new(*k)).collect();
    let batch = |key: &PropertyKey| -> Vec<Option<obrain_common::types::Value>> {
        let v = sub.get_node_property_batch(&ids, key);
        v.into_iter().map(|o| o).collect()
    };
    let qn_col = batch(&keys[0]);
    let cwd_col = batch(&keys[1]);
    let created_col = batch(&keys[2]);
    let title_col = batch(&keys[3]);
    let path_col = batch(&keys[4]);
    let preset_col = batch(&keys[5]);
    let origin_col = batch(&keys[6]);
    let parent_col = batch(&keys[7]);
    let empty = obrain_common::types::Value::Null;
    let mut out: Vec<Vec<obrain_common::types::Value>> = Vec::new();
    for (i, nid) in ids.iter().enumerate() {
        // (2026-09-15) Filtre proprietaire (chemin owner) : seules les
        // sessions du set passent — le set vient d une traversee OWNED_BY
        // legere (qn seul), les props restent servies en colonnaire.
        if let Some(set) = possedes {
            let qn = qn_col
                .get(i)
                .and_then(|o| o.as_ref())
                .map(|v| v.to_string());
            match qn {
                Some(q) if set.contains(&q) => {}
                _ => continue,
            }
        }
        let origin = origin_col
            .get(i)
            .and_then(|o| o.as_ref())
            .unwrap_or(&empty)
            .to_string();
        if origin == "subagent" && sub.count_messages_de_session(nid.0 as u32).is_none() {
            continue;
        }
        let cell = |c: &Vec<Option<obrain_common::types::Value>>, idx: usize| {
            c.get(idx).and_then(|o| o.clone()).unwrap_or(empty.clone())
        };
        /* (2026-10-16) LES DEUX FILTRES DE CONTENU DU CONTRAT SONT APPLIQUES
         * ICI, AVANT LA FENETRE — pas apres, dans `session_rows_to_contract`.
         *
         * Pourquoi ca change tout : `total` valait `rows.len()`, donc la
         * TAILLE DE LA PAGE. Un appelant sans `limit` (le harness) recevait
         * 1000 lignes au plus et lisait « total = 1000 » — aucune trace de la
         * troncature. Mesure du 2026-10-16 sur runtime-dev : 1432 nœuds
         * Session, 998 servis, 97 conversations d'aout amputees par le seul
         * plafond, et RIEN ne le disait.
         *
         * Et filtrer apres la fenetre troue les pages : une page de 1000
         * lignes brutes dont 40 sont ecartees ne rend que 960 lignes, ce qui
         * est indistinguable d'une fin de liste pour un paginateur par
         * `offset`. Les deux ecartements du contrat (qn vide, cwd NULL) sont
         * donc remontes avant le tri et la fenetre. */
        let qn_val = cell(&qn_col, i).to_string();
        if qn_val.is_empty() || qn_val == "NULL" || qn_val == "null" {
            continue;
        }
        let cwd_val = cell(&cwd_col, i).to_string();
        if cwd_val.is_empty() || cwd_val == "NULL" || cwd_val == "null" {
            continue;
        }
        out.push(vec![
            cell(&qn_col, i),
            cell(&cwd_col, i),
            cell(&created_col, i),
            cell(&title_col, i),
            cell(&path_col, i),
            cell(&preset_col, i),
            origin_col
                .get(i)
                .and_then(|o| o.clone())
                .unwrap_or(empty.clone()),
            cell(&parent_col, i),
        ]);
    }
    // Ce que la fenetre ne dit pas : combien de sessions satisfont le contrat
    // AVANT decoupe. C'est le seul chiffre qui rend une troncature visible.
    let total = out.len();
    // (2026-09-14) PAGINATION recente-d-abord : tri par createdAt decroissant
    // puis fenetre [offset, offset+limit). Avant : scan en ordre de creation
    // (les plus ANCIENNES d abord) coupe a 500 -> au-dela de 500 sessions,
    // TOUTES les conversations recentes disparaissaient de la liste.
    out.sort_by_key(|r| std::cmp::Reverse(r.get(2).and_then(|v| v.as_int64()).unwrap_or(0)));
    let fin = offset.saturating_add(limit).min(out.len());
    let debut = offset.min(fin);
    (out[debut..fin].to_vec(), total)
}

/// (2026-09-07) Facteur commun : lignes Session (colonnes 0..7) → contrat
/// `sessions: [{qn, name, properties}]` (props = chaîne JSON ; sauts des
/// nœuds invalides qn/cwd NULL). Sémantique identique à l'ancien rendu GQL.
fn session_rows_to_contract(raw: &[Vec<obrain_common::types::Value>]) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    for r in raw {
        let qn = r.first().map(|v| v.to_string()).unwrap_or_default();
        if qn.is_empty() || qn == "NULL" || qn == "null" {
            continue;
        }
        let cwd_raw = r.get(1).map(|v| v.to_string()).unwrap_or_default();
        if cwd_raw.is_empty() || cwd_raw == "NULL" || cwd_raw == "null" {
            continue;
        }
        let mut props = serde_json::Map::new();
        props.insert("cwd".into(), json!(cwd_raw));
        props.insert(
            "createdAt".into(),
            r.get(2)
                .and_then(|v| v.as_int64())
                .map(|n| json!(n))
                .unwrap_or(json!(0)),
        );
        for (key, idx) in [("title", 3), ("source_path", 4), ("agentPreset", 5)] {
            let val = r.get(idx).map(|v| v.to_string()).unwrap_or_default();
            if !val.is_empty() && val != "NULL" && val != "null" {
                props.insert(key.into(), json!(val));
            }
        }
        let origin = r.get(6).map(|v| v.to_string()).unwrap_or_default();
        if origin == "subagent" {
            props.insert("origin".into(), json!("subagent"));
        }
        let parent = r.get(7).map(|v| v.to_string()).unwrap_or_default();
        if !parent.is_empty() && parent != "null" && parent != "NULL" {
            props.insert("parentSession".into(), json!(parent));
        }
        let props_json = serde_json::to_string(&serde_json::Value::Object(props))
            .unwrap_or_else(|_| "{}".to_string());
        rows.push(json!({
            "qn": qn,
            "name": qn,
            "properties": props_json,
        }));
    }
    rows
}

pub(crate) fn sessions_list(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session = db.session();
    let owner = arg_str(arguments, "owner");
    let substrate = db.substrate_handle();
    // (2026-09-14) Pagination recente-d-abord : limit/offset optionnels.
    // Defauts larges (1000) : l appel dsh sans arguments recoit desormais
    // l historique complet, les plus recentes en premier.
    let limit = arg_usize(arguments, "limit", 1000).clamp(1, 5000);
    let offset = arg_usize(arguments, "offset", 0);
    let page_defaut = limit == 1000 && offset == 0;
    // (2026-09-07) CHEMIN 100 % SUBSTRATE (branche « toutes ») : lecteur
    // nœud à nœud des 8 props (µs/prop) au lieu de la projection GQL
    // (~18 ms de décodage) → cible 1-3 ms. Contrat identique.
    if owner.is_none() {
        if let Some(sub) = &substrate {
            // Mémo-cache (process écrivain) : retour µs quand rien n'a changé.
            let generation = SESSIONS_LIST_GEN.load(std::sync::atomic::Ordering::Relaxed);
            if is_writer_process() && page_defaut {
                if let Some(cached) = sessions_cache_get(generation) {
                    return (cached, false);
                }
            }
            let (raw, total) = sessions_rows_from_substrate(sub, limit, offset, None);
            let rows = session_rows_to_contract(&raw);
            let resp = json!({ "status": "ok", "sessions": rows, "total": total });
            if is_writer_process() && page_defaut {
                sessions_cache_put(generation, resp.clone());
            }
            return (resp, false);
        }
    }
    // (2026-09-15) CHEMIN OWNER 100 % SUBSTRATE : la variante GQL projetait
    // 8 props par ligne (~80-104 ms mesures sur 615 sessions). Ici : une
    // traversee OWNED_BY legere (qn seul, ~1 ms) puis le lecteur colonnaire
    // filtre (~1 ms) — memes filtres de contenu, meme contrat, meme tri.
    if let (Some(acc), Some(sub)) = (&owner, &substrate) {
        if qn_inline_safe(acc) {
            let q_possedes = format!(
                "MATCH (s:Session)-[:OWNED_BY]->(a:Account {{qualified_name: '{acc}'}}) RETURN s.qualified_name LIMIT 5000"
            );
            if let Ok(res) = session.execute(&q_possedes) {
                let set: std::collections::HashSet<String> = res
                    .rows
                    .iter()
                    .filter_map(|r| r.first().map(|v| v.to_string()))
                    .filter(|q| !q.is_empty() && q != "NULL" && q != "null")
                    .collect();
                let (raw, total) = sessions_rows_from_substrate(sub, limit, offset, Some(&set));
                let rows = session_rows_to_contract(&raw);
                return (
                    json!({ "status": "ok", "sessions": rows, "total": total }),
                    false,
                );
            }
        }
    }
    // (2026-08-31) UNE SEULE REQUÊTE GROUPÉE avec les PROPS EXPLICITES
    // (RETURN s ne rend pas le nœud entier à côté d'une propriété → props
    // vides). Colonnes : qualified_name + les props du header de session.
    //
    // (2026-09-07) LIMITE MOTEUR documentée (repro mesurée sur runtime-dev) :
    // `EXISTS {…}` est correct en conjonction positive (AND), mais IGNORÉ sous
    // `OR` (l'OR-EXISTS ne renvoyait que les racines : 154 au lieu de 223) et
    // `NOT EXISTS` rend 0. → AUCUNE requête ici n'utilise plus OR-EXISTS quand
    // le substrate est disponible : le prédicat de contenu (« racine, ou
    // subagent avec ≥1 Message ») est appliqué EN POST via la reverse-PART_OF
    // µs (`count_messages_de_session`), exactement comme le chemin « toutes ».
    // Le repli GQL OR-EXISTS ne subsiste que hors substrate (in-memory/tests).
    let (query, _label, fast_content) = match (&owner, substrate.is_some()) {
        (Some(acc), true) => (
            format!(
                "MATCH (s:Session)-[:OWNED_BY]->(a:Account {{qualified_name: '{acc}'}})                  RETURN s.qualified_name, s.cwd, s.createdAt, s.title, s.source_path,                  s.agentPreset, s.origin, s.parentSession, id(s)                  LIMIT 5000"
            ),
            "filtré-fast",
            true,
        ),
        (Some(acc), false) => (
            format!(
                "MATCH (s:Session)-[:OWNED_BY]->(a:Account {{qualified_name: '{acc}'}})                  WHERE (s.origin IS NULL OR s.origin <> 'subagent' OR EXISTS {{ MATCH (:Message)-[:PART_OF]->(s) }})                  RETURN s.qualified_name, s.cwd, s.createdAt, s.title, s.source_path,                  s.agentPreset, s.origin, s.parentSession                  LIMIT 5000"
            ),
            "filtré",
            false,
        ),
        (None, true) => (
            // id(s) en colonne 8 = slot substrate pour le test de contenu O(1).
            format!(
                "MATCH (s:Session) RETURN s.qualified_name, s.cwd, s.createdAt, s.title, s.source_path, s.agentPreset, s.origin, s.parentSession, id(s) LIMIT 5000"
            ),
            "toutes-fast",
            true,
        ),
        (None, false) => (
            // Repli GQL d'origine (limite moteur OR-EXISTS ci-dessus).
            format!(
                "MATCH (s:Session)              WHERE (s.origin IS NULL OR s.origin <> 'subagent' OR EXISTS {{ MATCH (:Message)-[:PART_OF]->(s) }})              RETURN s.qualified_name, s.cwd, s.createdAt, s.title, s.source_path,              s.agentPreset, s.origin, s.parentSession              LIMIT 5000"
            ),
            "toutes",
            false,
        ),
    };
    match session.execute(&query) {
        Ok(res) => {
            let mut raw = res.rows;
            if fast_content {
                // Prédicat de contenu, même sémantique que le WHERE GQL
                // d'origine (conforme au contrat commenté du tool) : on garde
                // les racines (origin ≠ "subagent", y compris NULL) et les
                // subagents ayant ≥1 Message (PART_OF). Ordre de scan
                // identique → cap 500 sur les 500 premières correspondances,
                // comme le LIMIT d'origine.
                let sub = substrate.as_ref().expect("fast_content ⇒ substrate");
                raw.retain(|r| {
                    let origin = r.get(6).map(|v| v.to_string()).unwrap_or_default();
                    if origin != "subagent" {
                        return true;
                    }
                    let slot = r.get(8).and_then(|v| v.as_int64()).unwrap_or(-1);
                    slot > 0 && sub.count_messages_de_session(slot as u32).is_some()
                });
            }
            // (2026-09-14) Tri recent-d-abord en Rust : LIMITE MOTEUR constatee,
            // ORDER BY sur motif avec arete rend des props NULL apres tri.
            raw.sort_by_key(|r| {
                std::cmp::Reverse(r.get(2).and_then(|v| v.as_int64()).unwrap_or(0))
            });
            let fin = offset.saturating_add(limit).min(raw.len());
            let debut = offset.min(fin);
            let raw = raw[debut..fin].to_vec();
            let rows = session_rows_to_contract(&raw);
            (
                json!({ "status": "ok", "sessions": rows, "total": rows.len() }),
                false,
            )
        }
        Err(e) => (json!({ "status": "error", "error": e.to_string() }), true),
    }
}

/// Sérialise les props d'un nœud Session (représentation `{k: v, ...}`) en
/// JSON objet — le format exact attendu par session-persistence-graph
/// (`JSON.parse(s.properties)` : createdAt, cwd, title, source_path…).
fn session_node_props_to_json(v: &obrain_common::types::Value) -> Value {
    // La valeur est la représentation texte du nœud : `{_id: 3, _labels: [...],
    // agentPreset: standard, createdAt: 1787737055871, ...}`.
    let text = v.to_string();
    let inner = text.trim().trim_start_matches('{').trim_end_matches('}');
    let mut props = serde_json::Map::new();
    for part in inner.split(", ") {
        if let Some((k, val)) = part.split_once(": ") {
            let key = k.trim();
            if key.starts_with('_') {
                continue; // _id, _labels — internes
            }
            let val = val.trim();
            // Nombre ?
            if let Ok(n) = val.parse::<i64>() {
                props.insert(key.to_string(), json!(n));
            } else if let Ok(f) = val.parse::<f64>() {
                props.insert(key.to_string(), json!(f));
            } else {
                props.insert(key.to_string(), json!(val));
            }
        }
    }
    Value::Object(props)
}

/// `context_build` — l'injection composée (CH6) : goals, summaries, weave.
/// Ici : la version obrain — sessions proches (BM25) + engrams rappelés.
pub(crate) fn context_build(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session_id = arg_str(arguments, "session_id").unwrap_or_default();
    let limit = arg_usize(arguments, "limit", 8);
    let subject = arg_str(arguments, "subject");
    let owner = arg_str(arguments, "owner");
    let session = db.session();

    // 1. Goals + résumés de la session (requêtes ciblées).
    // (2026-09-04) DEUX patterns d'arêtes couverts : HAS_GOAL Session→Goal
    // (writer import_events, live) ET GOVERNED Goal→Session (imports
    // d'archives — le moteur C écrivait GOVERNED ; sans ce pattern, les
    // goals historiques restaient invisibles). Props lues : text/content
    // (l'objectif intégral, jamais tronqué).
    let mut goals: Vec<Value> = Vec::new();
    if !session_id.is_empty() {
        let patterns = [
            format!(
                "MATCH (s:Session {{qualified_name: '{}'}})-[:HAS_GOAL]->(g:Goal) \
                 RETURN g.text, g.content, g.operation LIMIT 5",
                session_id
            ),
            format!(
                "MATCH (g:Goal)-[:GOVERNED]->(s:Session {{qualified_name: '{}'}}) \
                 RETURN g.text, g.content, g.operation LIMIT 5",
                session_id
            ),
        ];
        for q in patterns {
            if let Ok(res) = session.execute(&q) {
                for r in &res.rows {
                    let text = r.first().map(|v| v.to_string()).unwrap_or_default();
                    let content = r.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let operation = r.get(2).map(|v| v.to_string()).unwrap_or_default();
                    let body = if !text.is_empty() && text != "NULL" {
                        text
                    } else {
                        content
                    };
                    goals.push(json!({ "content": body, "operation": operation }));
                }
            }
        }
        // Dédoublage par contenu (un goal peut être atteint par les deux
        // patterns quand les deux arêtes coexistent).
        let mut seen = std::collections::HashSet::new();
        goals.retain(|g| {
            let key = g
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            seen.insert(key)
        });
        goals.truncate(5);
    }

    // 1b. Résumés de compaction de la session — les checkpoints (messages que
    //     la compaction a repliés, source plugin 'compact') sont la mémoire
    //     condensée de l'historique : l'injection doit pouvoir les re-présenter
    //     à chaque tour sans payer l'historique brut. Les plus récents d'abord,
    //     bornés par limit.
    let mut summaries: Vec<Value> = Vec::new();
    if !session_id.is_empty() {
        let q = format!(
            "MATCH (s:Session {{qualified_name: '{}'}})<-[:PART_OF]-(m:Message) WHERE m.is_compaction = true RETURN m.text, m.seq ORDER BY m.seq DESC LIMIT {}",
            session_id, limit
        );
        match session.execute(&q) {
            Ok(res) => {
                summaries = res
                    .rows
                    .iter()
                    .map(|r| {
                        let brut = r.first().map(|v| v.to_string()).unwrap_or_default();
                        // ── UN RÉSUMÉ DE COMPACTION N'EST PAS BORNÉ PAR NATURE ──
                        // Défaut mesuré le 2026-09-16 : cette section traversait la
                        // frontière SANS troncature — seul leur NOMBRE était borné
                        // (`limit`). Une session à huit gros checkpoints pouvait donc
                        // injecter un volume arbitraire dans le prompt, et le budget
                        // du modèle partait au premier venu. On borne ici, dans le
                        // MÊME esprit que la restitution côté harness (600 car.) :
                        // et on le DIT, parce qu'un extrait tu n'est pas un tout.
                        let text = tronquer_avec_marque(&brut, RESUME_COMPACTION_MAX);
                        let seq = r.get(1).and_then(|v| v.as_int64()).unwrap_or(-1);
                        json!({ "seq": seq, "text": text, "tronque": brut.chars().count() > RESUME_COMPACTION_MAX })
                    })
                    .collect();
            }
            Err(e) => {
                eprintln!("[context_build] summaries query failed: {e}");
            }
        }
    }

    // 2. Sessions proches (BM25 sur le subject) — le weave.
    let mut weave: Vec<Value> = Vec::new();
    if let Some(subj) = &subject {
        let _ = db.create_text_index("Session", "title");
        if let Ok(hits) = db.text_search("Session", "title", &subj, limit) {
            weave = hits
                .iter()
                .map(|(nid, score)| json!({ "id": nid.0.to_string(), "score": score }))
                .collect();
        }
    }

    // 3. Engrams rappelés — LE RAPPEL PRÉCIS (la couche cognitive).
    //    Cues = les nœuds touchés par la session en jeu (co-activation
    //    réelle) ; recall(cues, k) retourne les ensembles qui co-activaient
    //    ensemble AVANT — la mémoire « ce qui marche ensemble ».
    let mut engrams: Vec<Value> = Vec::new();
    if let Some(ce) = db.cognitive_engine() {
        if let Some(mgr) = ce.engram_manager() {
            // Cues depuis la session : les fichiers/symboles qu'elle touche.
            // PRIVACY (U7) : quand owner est fourni, la session doit lui
            // appartenir (OWNED_BY) — sinon aucun cue n'est servi.
            let mut cues: Vec<obrain_common::types::NodeId> = Vec::new();
            let owner_ok = match &owner {
                Some(acc) => {
                    let q = format!(
                        "MATCH (s:Session {{qualified_name: '{}'}})-[:OWNED_BY]->(a:Account {{qualified_name: '{acc}'}}) RETURN count(s)",
                        session_id
                    );
                    session
                        .execute(&q)
                        .map(|r| {
                            r.rows
                                .first()
                                .and_then(|row| row.first())
                                .and_then(|v| v.as_int64())
                                .map(|n| n > 0)
                                .unwrap_or(false)
                        })
                        .unwrap_or(false)
                }
                None => true,
            };
            if owner_ok && !session_id.is_empty() {
                let q = format!(
                    "MATCH (tc:ToolCall)-[:TOUCHES]->(f) WHERE tc.session = '{}' RETURN id(f) LIMIT 50",
                    session_id
                );
                if let Ok(res) = session.execute(&q) {
                    for row in &res.rows {
                        if let Some(v) = row.first() {
                            if let Some(nid) = v.as_int64() {
                                cues.push(obrain_common::types::NodeId(nid as u64));
                            }
                        }
                    }
                }
            }
            // Si pas de session (build sans session_id), le rappel sémantique
            // par subject passe par BM25 sur les nœuds → ids comme cues.
            if cues.is_empty() {
                if let Some(subj) = &subject {
                    let _ = db.create_text_index("File", "file_path");
                    if let Ok(hits) = db.text_search("File", "file_path", subj, 5) {
                        for (nid, _) in hits {
                            cues.push(nid.clone());
                        }
                    }
                }
            }
            if !cues.is_empty() {
                let recalls = mgr.recall(&cues, limit);
                for r in recalls {
                    let members: Vec<Value> = r
                        .engram
                        .ensemble
                        .iter()
                        .take(10)
                        .map(|(nid, w)| json!({ "id": nid.0, "weight": w }))
                        .collect();
                    engrams.push(json!({
                        "id": r.engram_id.0,
                        "confidence": r.confidence,
                        "strength": r.engram.strength,
                        "members": members,
                    }));
                }
            }
        }
    }

    // ── LE BUDGET EN JETONS DE L'INJECTION, ET IL VIENT D'OBRAIN-RAG ────────
    // Défaut mesuré le 2026-09-16 : chaque section avait son plafond (goals 5,
    // summaries `limit`, weave `limit`, engrams 10 membres) et AUCUN chiffre
    // d'ensemble — un plafond par section n'est pas un budget, c'est la somme
    // de ce que chacun s'autorise. `obrain-rag` portait déjà le budget
    // (`RagConfig::token_budget`, estimation chars/jeton, sélection gloutonne
    // avec garantie « au moins un élément ») et **n'était branché nulle part**.
    // Il l'est ici, une fois, sur le payload assemblé.
    let mut resp = json!({
        "status": "ok",
        "session_id": session_id,
        "goals": goals,
        "summaries": summaries,
        "weave": weave,
        "engrams": engrams,
        "note": "injection obrain (CH6) — rappel BM25 + engrams",
    });
    borner_injection_par_jetons(&mut resp);
    (resp, false)
}

/// Le budget en jetons de l'injection — UN SEUL chiffre.
///
/// Réglable par `OBRAIN_CONTEXT_TOKEN_BUDGET` (borne basse 200 : en dessous, on
/// ne transporte plus une injection, on transporte une devinette). Le défaut est
/// celui de `obrain-rag` : c'est le crate qui porte la politique de budget, le
/// pont s'y range au lieu d'inventer un second chiffre.
fn budget_jetons_injection() -> usize {
    std::env::var("OBRAIN_CONTEXT_TOKEN_BUDGET")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v >= 200)
        .unwrap_or_else(|| obrain_rag::RagConfig::default().token_budget)
}

/// Un élément injectable : ce que le budget peut garder ou retirer.
struct ElementInjection {
    /// Section d'origine (`goals`, `summaries`, `weave`, `engrams`).
    section: &'static str,
    /// Index dans la section (pour retirer le bon élément).
    index: usize,
    /// L'estimation du budgeter de `obrain-rag` pour cet élément.
    estimation: obrain_rag::traits::RetrievedNode,
}

/// Applique le budget en JETONS de `obrain-rag` au payload d'injection.
///
/// ── CE QUE CETTE FONCTION GARANTIT ──────────────────────────────────────────
/// * le payload tient sous `budget` jetons ESTIMÉS par le budgeter du crate
///   (`obrain_rag::budget::estimate_tokens`, `chars_per_token` de la config) ;
/// * l'ordre de valeur est respecté : goals, puis résumés de compaction, puis
///   weave, puis membres d'engram — on retire le moins éclairant d'abord ;
/// * **au moins un élément est toujours gardé** (la garantie de
///   `select_within_budget`) : une injection vide ferait croire à une mémoire
///   vide ;
/// * ce qui a été retiré SE DIT (`troncature`) : un extrait muet se lit comme
///   une absence, et l'agent conclurait sur un tout qu'il n'a pas.
fn borner_injection_par_jetons(payload: &mut Value) {
    let config = obrain_rag::RagConfig {
        token_budget: budget_jetons_injection(),
        ..obrain_rag::RagConfig::default()
    };

    // Éléments candidats, dans l'ordre de VALEUR.
    let mut elems: Vec<ElementInjection> = Vec::new();
    for (section, index, texte) in sections_injectables(payload) {
        elems.push(ElementInjection {
            section,
            index,
            estimation: node_pour_estimation(section, index, &texte),
        });
    }
    if elems.is_empty() {
        return;
    }

    // La sélection du crate fait foi pour le CHIFFRE ; on refait la même boucle
    // en gardant les indices, parce que c'est eux qui permettent le retrait.
    let (retenus, jetons_apres) = {
        let mut retenus: Vec<usize> = Vec::new();
        let mut total = 0usize;
        for (i, elem) in elems.iter().enumerate() {
            let cout = obrain_rag::budget::estimate_tokens(&elem.estimation, &config);
            if total + cout > config.token_budget {
                // La garantie du crate : au moins UN élément, même trop gros.
                if retenus.is_empty() {
                    retenus.push(i);
                    total += cout;
                }
                break;
            }
            retenus.push(i);
            total += cout;
        }
        (retenus, total)
    };
    let jetons_avant: usize = elems
        .iter()
        .map(|e| obrain_rag::budget::estimate_tokens(&e.estimation, &config))
        .sum();
    if retenus.len() == elems.len() {
        return; // rien à dire : le budget n'a pas mordu
    }

    // Retrait effectif, section par section, en gardant les indices d'origine.
    let mut retire: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (i, elem) in elems.iter().enumerate() {
        if !retenus.contains(&i) {
            *retire.entry(elem.section).or_insert(0) += 1;
        }
    }
    for section in ["engrams", "weave", "summaries", "goals"] {
        let gardes: std::collections::BTreeSet<usize> = elems
            .iter()
            .enumerate()
            .filter(|(i, e)| e.section == section && retenus.contains(i))
            .map(|(_, e)| e.index)
            .collect();
        if let Some(items) = payload.get_mut(section).and_then(Value::as_array_mut) {
            let mut index = 0usize;
            items.retain(|_| {
                let garde = gardes.contains(&index);
                index += 1;
                garde
            });
        }
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "troncature".to_string(),
            json!({
                "budget_jetons": config.token_budget,
                "jetons_avant": jetons_avant,
                "jetons_apres": jetons_apres,
                "retire": retire,
                "note": "budget de jetons de l'injection atteint (obrain-rag) : le contenu a ete reduit par section, du moins eclairant au plus utile. Ce qui manque n'est pas « absent du graphe » — ne conclus pas sur un extrait.",
            }),
        );
    }
}

/// Les éléments injectables du payload, dans l'ORDRE DE VALEUR (le premier est
/// le plus éclairant : c'est lui qui survit à une coupe).
fn sections_injectables(payload: &Value) -> Vec<(&'static str, usize, String)> {
    let mut sortie = Vec::new();
    for (section, genre) in [
        ("goals", "goals"),
        ("summaries", "summaries"),
        ("weave", "weave"),
    ] {
        let Some(items) = payload.get(section).and_then(Value::as_array) else {
            continue;
        };
        let nom: &'static str = match genre {
            "goals" => "goals",
            "summaries" => "summaries",
            _ => "weave",
        };
        for (index, item) in items.iter().enumerate() {
            let texte = item
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| item.get("content").and_then(Value::as_str))
                .or_else(|| item.get("title").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            sortie.push((nom, index, texte));
        }
    }
    // Les membres d'engram : ils portent des identifiants — leur coût est celui
    // de leur empreinte JSON.
    if let Some(engrams) = payload.get("engrams").and_then(Value::as_array) {
        for (index, engram) in engrams.iter().enumerate() {
            sortie.push(("engrams", index, engram.to_string()));
        }
    }
    sortie
}

/// Le nœud d'estimation d'un élément — la forme attendue par le budgeter.
///
/// ── POURQUOI LE TEXTE EST DÉCOUPÉ EN PROPRIÉTÉS DE 500 CARACTÈRES ───────────
/// `obrain_rag::budget::estimate_tokens` plafonne CHAQUE valeur de propriété à
/// `value.len().min(500)` — un choix juste pour des nœuds de RAG (des
/// propriétés courtes), mais faux pour ce qu'on injecte ici : un objectif, un
/// résumé de compaction ou un extrait de session font couramment 600 à
/// 3 000 caractères. Mesuré : un texte de 3 000 caractères était estimé à
/// **135 jetons** au lieu de ~750 — un budget qui sous-compte de 5× ne borne
/// rien, il donne bonne conscience.
///
/// On ne réécrit donc PAS l'estimation du crate : on lui donne la vérité en la
/// découpant. Chaque tranche est une propriété, leur somme est le texte entier,
/// et c'est bien `estimate_tokens` qui décide — le budget reste celui
/// d'`obrain-rag`.
fn node_pour_estimation(
    section: &str,
    index: usize,
    texte: &str,
) -> obrain_rag::traits::RetrievedNode {
    const TRANCHE: usize = 500;
    let mut properties = std::collections::HashMap::new();
    if texte.is_empty() {
        properties.insert("text".to_string(), String::new());
    } else {
        for (n, morceau) in texte.as_bytes().chunks(TRANCHE).enumerate() {
            // Découpe sur une frontière UTF-8 valide (un caractère multi-octets
            // coupé en deux serait un texte faux, pas une estimation).
            let fin = (n + 1) * TRANCHE;
            let borne = fin.min(texte.len());
            let mut borne = borne;
            while borne > 0 && !texte.is_char_boundary(borne) {
                borne -= 1;
            }
            let Some(morceau) = texte.get(n * TRANCHE..borne) else {
                break;
            };
            let cle = if n == 0 {
                "text".to_string()
            } else {
                format!("text_{}", n + 1)
            };
            properties.insert(cle, morceau.to_string());
            let _ = morceau;
        }
    }
    obrain_rag::traits::RetrievedNode {
        node_id: obrain_common::types::NodeId(index as u64),
        labels: vec![section.to_string()],
        properties,
        score: 1.0,
        source: obrain_rag::traits::RetrievalSource::SpreadingActivation {
            depth: 0,
            activation: 1.0,
        },
        outgoing_relations: Vec::new(),
        incoming_relations: Vec::new(),
    }
}

/// `checkpoint_material` — matériau d'un checkpoint de compaction piloté par
/// le graphe (CH6 / contexte illimité). Pour une session, renvoie les FAITS
/// DÉJÀ EXTRAITS par le graphe — goals (HAS_GOAL/GOVERNED), checkpoints de
/// compaction antérieurs (`is_compaction`), fichiers touchés agrégés
/// (TOUCHES ToolCall→File), premiers messages utilisateur (l'intention) et
/// derniers messages (l'état courant) — au lieu de rejouer l'historique brut.
/// Le summarizeur de compaction du harness assemble un checkpoint borné à
/// partir de ce matériau : jamais d'appel LLM dont la taille dépend de la
/// longueur de la session, donc plus de cercle vicieux overflow→résumé→
/// overflow. L'historique replié reste requêtable dans le graphe (rien n'est
/// perdu — « reconstruction » plutôt que relecture).
fn checkpoint_material(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session_id = arg_str(arguments, "session_id").unwrap_or_default();
    let limit = arg_usize(arguments, "limit", 8);
    let session = db.session();
    if session_id.is_empty() {
        return (
            json!({ "status": "error", "error": "checkpoint_material: session_id requis" }),
            true,
        );
    }

    // 1. Goals — les deux patterns (HAS_GOAL writer live, GOVERNED archives),
    //    même logique que context_build (dédoublage par contenu, borné).
    let mut goals: Vec<Value> = Vec::new();
    let patterns = [
        format!(
            "MATCH (s:Session {{qualified_name: '{}'}})-[:HAS_GOAL]->(g:Goal) \
             RETURN g.text, g.content, g.operation LIMIT 5",
            session_id
        ),
        format!(
            "MATCH (g:Goal)-[:GOVERNED]->(s:Session {{qualified_name: '{}'}}) \
             RETURN g.text, g.content, g.operation LIMIT 5",
            session_id
        ),
    ];
    for q in patterns {
        if let Ok(res) = session.execute(&q) {
            for r in &res.rows {
                let text = r.first().map(|v| v.to_string()).unwrap_or_default();
                let content = r.get(1).map(|v| v.to_string()).unwrap_or_default();
                let body = if !text.is_empty() && text != "NULL" {
                    text
                } else {
                    content
                };
                goals.push(json!({ "content": body }));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    goals.retain(|g| {
        let content = g
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if content.is_empty() || content == "NULL" {
            return false;
        }
        seen.insert(content)
    });
    goals.truncate(limit);

    // 2. Checkpoints de compaction antérieurs — la mémoire déjà condensée de
    //    la session. Un nouveau checkpoint CONSOLIDE les précédents (textes
    //    intégraux, bornés en nombre) au lieu de re-lire le brut qu'ils
    //    remplacent. Les plus récents d'abord.
    let mut checkpoints: Vec<Value> = Vec::new();
    if !session_id.is_empty() {
        let q = format!(
            "MATCH (s:Session {{qualified_name: '{}'}})<-[:PART_OF]-(m:Message) \
             WHERE m.is_compaction = true RETURN m.text, m.seq ORDER BY m.seq DESC LIMIT {}",
            session_id,
            limit.min(3)
        );
        if let Ok(res) = session.execute(&q) {
            checkpoints = res
                .rows
                .iter()
                .map(|r| {
                    let text = r.first().map(|v| v.to_string()).unwrap_or_default();
                    let seq = r.get(1).and_then(|v| v.as_int64()).unwrap_or(-1);
                    json!({ "seq": seq, "text": text })
                })
                .collect();
        }
    }

    // 3. Fichiers touchés agrégés (TOUCHES ToolCall→File) — le « weave »
    //    factuel de la session : chemins exacts + fréquence, sans relire
    //    aucun contenu.
    let mut files: Vec<Value> = Vec::new();
    if !session_id.is_empty() {
        let q = format!(
            "MATCH (tc:ToolCall {{session: '{}'}})-[:TOUCHES]->(f:File) \
             RETURN f.file_path, count(tc) GROUP BY f.file_path LIMIT {}",
            session_id,
            limit * 3
        );
        if let Ok(res) = session.execute(&q) {
            let mut rows: Vec<(String, i64)> = res
                .rows
                .iter()
                .filter_map(|r| {
                    let path = r.first().map(|v| v.to_string()).unwrap_or_default();
                    if path.is_empty() || path == "NULL" {
                        return None;
                    }
                    let n = r.get(1).and_then(|v| v.as_int64()).unwrap_or(0);
                    Some((path, n))
                })
                .collect();
            // Tri décroissant par fréquence (le GROUP BY moteur ne garantit
            // pas d'ordre) puis bornage.
            rows.sort_by(|a, b| b.1.cmp(&a.1));
            rows.truncate(limit);
            files = rows
                .into_iter()
                .map(|(path, n)| json!({ "path": path, "count": n }))
                .collect();
        }
    }

    // 4. Premiers messages utilisateur — l'intention primaire (requête
    //    initiale), textes tronqués.
    let mut first_user: Vec<Value> = Vec::new();
    if !session_id.is_empty() {
        let q = format!(
            "MATCH (s:Session {{qualified_name: '{}'}})<-[:PART_OF]-(m:Message) \
             WHERE m.role = 'user' RETURN m.text, m.seq ORDER BY m.seq ASC LIMIT 3",
            session_id
        );
        if let Ok(res) = session.execute(&q) {
            first_user = res
                .rows
                .iter()
                .map(|r| {
                    let text = r.first().map(|v| v.to_string()).unwrap_or_default();
                    json!({ "text": truncate_chars(&text, 1200) })
                })
                .collect();
        }
    }

    // 5. Derniers messages non-checkpoint — l'état courant (borné). Le
    //    moteur exclut les props absentes des comparaisons (`<> true` ne
    //    matche pas les messages ordinaires sans is_compaction) → projection
    //    de la prop et filtre côté Rust.
    let mut last_messages: Vec<Value> = Vec::new();
    if !session_id.is_empty() {
        let q = format!(
            "MATCH (s:Session {{qualified_name: '{}'}})<-[:PART_OF]-(m:Message) \
             RETURN m.text, m.role, m.seq, m.is_compaction ORDER BY m.seq DESC LIMIT {}",
            session_id,
            limit * 2
        );
        if let Ok(res) = session.execute(&q) {
            let mut rows: Vec<Value> = res
                .rows
                .iter()
                .filter_map(|r| {
                    let is_compact = r.get(3).and_then(|v| v.as_bool()).unwrap_or(false);
                    if is_compact {
                        return None;
                    }
                    let text = r.first().map(|v| v.to_string()).unwrap_or_default();
                    if text.is_empty() || text == "NULL" {
                        return None;
                    }
                    let role = r.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let seq = r.get(2).and_then(|v| v.as_int64()).unwrap_or(-1);
                    Some(json!({ "seq": seq, "role": role, "text": truncate_chars(&text, 900) }))
                })
                .collect();
            rows.truncate(limit);
            rows.reverse(); // ordre chronologique pour l'assemblage
            last_messages = rows;
        }
    }

    let resp = json!({
        "status": "ok",
        "session_id": session_id,
        "goals": goals,
        "checkpoints": checkpoints,
        "files": files,
        "first_user_messages": first_user,
        "last_messages": last_messages,
        "note": "checkpoint matériau piloté par le graphe — faits extraits, historique replié requêtable (reconstruction)",
    });
    (resp, false)
}

/// Tronque un texte à `max` caractères (sans couper un caractère UTF-8).
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars().take(max) {
        out.push(c);
    }
    out.push('…');
    out
}

/// `import_events` — écriture idempotente (GraphWriter, capture live).
///
/// Le GraphWriter du harness envoie les événements de session BRUTS
/// (`{type, seq, time, data}` — le flux JSONL tel que persisté), PAS des
/// nœuds déjà typés. Cette implémentation réplique la conversion du moteur
/// MCP de référence (`import_events_buffer` dans runtime_ingest.c) :
///
///   1. Header `type:"session"` → nœud Session (createdAt, cwd, title,
///      agentPreset, origin, parentSession) + marqueur `last_imported_seq`.
///   2. Chaque événement avec seq > last (incrémental, idempotent) :
///      - `turn/start`     → nœud Turn  + HAS_TURN  (Session→Turn)
///      - `step/start`     → nœud Step  + HAS_STEP  (Turn→Step)
///      - `user/assistant/message` → nœud Message + PART_OF + texte
///      - `tool/call`      → nœud ToolCall + INVOKED (Step→ToolCall)
///      - `tool/result`    → nœud ToolResult + PRODUCED (ToolCall→ToolResult)
///   3. Les lignes brutes sont APPENDÉES au journal de session (zone
///      substrate) — c'est ce que lit `session_load events_only` (le chemin
///      principal du harness).
fn import_events(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    // (2026-09-07) Toute mutation de session/message invalide le mémo-cache
    // de `sessions_list` (génération incrémentée AVANT traitement — une
    // invalidation conservative est sans risque, au pire une reconstruction).
    SESSIONS_LIST_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let events = arguments.get("events").and_then(|v| v.as_array());
    let Some(events) = events else {
        return (
            json!({ "status": "error", "error": "import_events: events requis" }),
            true,
        );
    };
    let session = db.session();
    let mut imported = 0usize;
    // Refus NOMMÉS et rendus à l'appelant : un batch qui contient une commande
    // invalide ne doit pas « réussir » en silence (voir la garde du soi).
    let mut refuses: Vec<String> = Vec::new();
    // (2026-08-31) CAPTURE LIVE : les ids des nœuds touchés par ce batch —
    // un épisode pour l'EngramManager (les co-activations du GraphWriter
    // forment les engrans sur le runtime RÉEL).
    let mut touched: Vec<obrain_common::types::NodeId> = Vec::new();

    // ── 1. Header : session_id (explicite ou événement "session") + props ──
    let mut session_id = arg_str(arguments, "session_id").unwrap_or_default();
    let mut s_created_at: i64 = 0;
    let mut s_cwd = String::new();
    let mut s_title = String::new();
    let mut s_origin = String::new();
    let mut s_parent = String::new();
    let mut s_agent_preset = String::new();
    let mut max_seq: i64 = -1;
    let mut journal_lines: Vec<String> = Vec::new();
    for ev in events.iter().take(2000) {
        let ty = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty == "session" {
            if session_id.is_empty() {
                session_id = ev
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
            s_created_at = ev.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
            s_cwd = ev
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            s_origin = ev
                .get("origin")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            s_parent = ev
                .get("parentSession")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            s_agent_preset = ev
                .get("agentPreset")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        } else if ty == "session/title" {
            if let Some(data) = ev.get("data") {
                s_title = data
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
        // max seq (lignes packées : le premier seq du run est seq0).
        let seq = ev.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
        let seq0 = ev.get("seq0").and_then(|v| v.as_i64()).unwrap_or(-1);
        if seq > max_seq {
            max_seq = seq;
        }
        if seq0 > max_seq {
            max_seq = seq0;
        }
        journal_lines.push(ev.to_string());
    }
    if session_id.is_empty() {
        return (
            json!({ "status": "error", "error": "import_events: session_id introuvable" }),
            true,
        );
    }
    // Normalisation « session-<uuid> » (les registres du harness adressent
    // les sessions par ce préfixe — certaines archives portent l'id brut).
    if !session_id.starts_with("session-") {
        session_id = format!("session-{session_id}");
    }
    if !s_parent.is_empty() && !s_parent.starts_with("session-") {
        s_parent = format!("session-{s_parent}");
    }
    let session_qn = session_id.clone();

    // ── 2. Marqueur incrémental existant (last_imported_seq) ──
    let last = session_last_seq(&session, &session_qn);

    // ── 3. Nœud Session (upsert, sert aussi de marqueur incrémental) ──
    let session_node = merge_node(&session, "Session", &session_qn, None);
    if session_node > 0 {
        touched.push(obrain_common::types::NodeId(session_node as u64));
    }
    let marker_seq = if max_seq > last { max_seq } else { last };
    // Préservation : un lot SANS header (append live) ne doit pas clobber
    // cwd/createdAt déjà posés — on relit l'existant et on hérite.
    {
        let mut props = serde_json::Map::new();
        props.insert("last_imported_seq".into(), json!(marker_seq));
        props.insert("source".into(), json!("import_events"));
        if !s_cwd.is_empty() {
            props.insert("cwd".into(), json!(s_cwd));
        }
        if s_created_at > 0 {
            props.insert("createdAt".into(), json!(s_created_at));
        }
        if !s_title.is_empty() {
            props.insert("title".into(), json!(s_title));
        }
        if !s_agent_preset.is_empty() {
            props.insert("agentPreset".into(), json!(s_agent_preset));
        }
        if s_origin == "subagent" {
            props.insert("origin".into(), json!("subagent"));
        }
        if !s_parent.is_empty() {
            props.insert("parentSession".into(), json!(s_parent));
        }
        // (2026-09-14) Tampon environnement du harness hote (dev/prod) :
        // OBRAIN_SESSION_ENV pose par le service. Inerte si absent. Corrige
        // la re-accumulation de sessions env=NULL (archives invisibles).
        if let Ok(env_tag) = std::env::var("OBRAIN_SESSION_ENV") {
            if !env_tag.is_empty() {
                props.insert("env".into(), json!(env_tag));
            }
        }
        set_node_props(&session, "Session", &session_qn, &props);
    }
    // (2026-09-14) Tampon proprietaire (deploiement mono-compte) :
    // OBRAIN_SESSION_OWNER=account:<id> pose par le service. Inerte si
    // absent. MERGE idempotent — corrige les sessions orphelines sans
    // OWNED_BY, invisibles dans la liste par compte (constat 2026-09-14 :
    // 552 orphelines sur 586). Le multi-compte reel viendra avec IAM natif.
    if let Ok(owner) = std::env::var("OBRAIN_SESSION_OWNER") {
        if !owner.is_empty() {
            let aqn = if owner.starts_with("account:") {
                owner
            } else {
                format!("account:{owner}")
            };
            if qn_inline_safe(&aqn) {
                if !node_exists(&session, "Account", &aqn) {
                    merge_node(&session, "Account", &aqn, None);
                }
                link_edge(
                    &session,
                    "Session",
                    &session_qn,
                    "Account",
                    &aqn,
                    "OWNED_BY",
                );
            }
        }
    }

    // ── 4. Deuxième passe : les événements (seq > last) → nœuds + edges ──
    let mut cur_turn: i64 = 0;
    let mut cur_step: i64 = 0;
    let mut step_turn: i64 = 0;
    let mut compact_marked = 0usize;
    /// Messages d'INJECTION (source plugin ≠ compact) : marqués comme tels pour
    /// que la restitution du rappel ne les prenne pas pour la parole humaine.
    let mut injections_marked = 0usize;
    for ev in events.iter().take(2000) {
        let ty = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let seq = ev.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
        let seq0 = ev.get("seq0").and_then(|v| v.as_i64()).unwrap_or(-1);
        let ev_seq = if seq >= 0 { seq } else { seq0 };
        if ev_seq <= last {
            continue;
        }
        let data = ev.get("data").cloned().unwrap_or(Value::Null);
        let dv = |k: &str| data.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        match ty {
            "turn/start" => {
                cur_turn = dv("turn");
                let qn = format!("{session_qn}:turn:{cur_turn}");
                let nid = merge_node(&session, "Turn", &qn, None);
                if nid > 0 {
                    touched.push(obrain_common::types::NodeId(nid as u64));
                }
                link_edge(&session, "Session", &session_qn, "Turn", &qn, "HAS_TURN");
                imported += 1;
            }
            "step/start" => {
                step_turn = dv("turn");
                cur_step = dv("step");
                let qn = format!("{session_qn}:step:{step_turn}:{cur_step}");
                let nid = merge_node(&session, "Step", &qn, None);
                if nid > 0 {
                    touched.push(obrain_common::types::NodeId(nid as u64));
                }
                let tqn = format!("{session_qn}:turn:{step_turn}");
                // Turn peut ne pas exister (lot sans turn/start) → upsert.
                merge_node(&session, "Turn", &tqn, None);
                link_edge(&session, "Turn", &tqn, "Step", &qn, "HAS_STEP");
                imported += 1;
            }
            "user/message" | "assistant/message" => {
                let role = data
                    .get("role")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        data.get("message")
                            .and_then(|m| m.get("role"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| {
                        if ty == "user/message" {
                            "user".to_string()
                        } else {
                            "assistant".to_string()
                        }
                    });
                let text = content_text_of(&data);
                let qn = format!("{session_qn}:msg:{seq}");
                let mut props = serde_json::Map::new();
                props.insert("seq".into(), json!(seq));
                props.insert("role".into(), json!(role));
                props.insert("turn".into(), json!(dv("turn")));
                props.insert("step".into(), json!(dv("step")));
                props.insert("text".into(), json!(text));
                props.insert("source".into(), json!("import_events"));
                if let Some(t_ms) = ev.get("time").and_then(|v| v.as_i64()) {
                    props.insert("t_ms".into(), json!(t_ms));
                }
                // Checkpoint de compaction : la compaction remplace une région
                // repliée par un message dont la source est le plugin 'compact'
                // (harness `compactCheckpointSource`). Marqueur dédié pour que
                // context_build injecte ces résumés (section summaries) sans
                // les confondre avec les messages ordinaires.
                let msg_plugin = data
                    .get("source")
                    .or_else(|| data.get("message").and_then(|m| m.get("source")))
                    .and_then(|s| s.get("plugin"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if msg_plugin == "compact" {
                    props.insert("is_compaction".into(), json!(true));
                    compact_marked += 1;
                } else if !msg_plugin.is_empty() {
                    // ── CE QUE LE HARNESS A POSÉ N'EST PAS LA PAROLE DE L'HUMAIN ─
                    // Défaut mesuré le 2026-09-16 : le bloc `<graph-context>` est
                    // un UserMessage dont la source est le plugin
                    // `graph-injection`. Ingéré comme un message ordinaire, il
                    // ressortait ensuite comme « dernier message » d'une session
                    // RAPPELÉE — la mémoire du graphe se rappelait sa propre
                    // sortie, et le mot réel de l'humain (« Continue ») n'arrivait
                    // qu'en second, hors de la fenêtre de restitution.
                    //
                    // On marque donc la PROVENANCE au lieu de jeter le message :
                    // il a bien été échangé, il doit rester dans le fil (le
                    // journal brut et le transcript le portent de toute façon),
                    // mais la restitution du rappel peut l'écarter — c'est elle
                    // qui décide de ce qui représente une conversation.
                    props.insert("source_plugin".into(), json!(msg_plugin));
                    props.insert("est_injection".into(), json!(true));
                    injections_marked += 1;
                }
                let nid = merge_node(&session, "Message", &qn, Some(&props));
                if nid > 0 {
                    touched.push(obrain_common::types::NodeId(nid as u64));
                }
                link_edge(&session, "Message", &qn, "Session", &session_qn, "PART_OF");
                imported += 1;
            }
            "tool/call" => {
                let call_id = data
                    .get("callId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !call_id.is_empty() {
                    let qn = format!("{session_qn}:call:{call_id}");
                    let mut props = serde_json::Map::new();
                    props.insert("seq".into(), json!(seq));
                    props.insert("name".into(), json!(name));
                    props.insert("callId".into(), json!(call_id));
                    props.insert("session".into(), json!(session_qn));
                    props.insert("turn".into(), json!(dv("turn")));
                    props.insert("step".into(), json!(dv("step")));
                    props.insert("source".into(), json!("import_events"));
                    if let Some(t_ms) = ev.get("time").and_then(|v| v.as_i64()) {
                        props.insert("t_ms".into(), json!(t_ms));
                    }
                    let nid = merge_node(&session, "ToolCall", &qn, Some(&props));
                    if nid > 0 {
                        touched.push(obrain_common::types::NodeId(nid as u64));
                    }
                    let sqn = format!("{session_qn}:step:{step_turn}:{cur_step}");
                    if step_exists(&session, &sqn) {
                        link_edge(&session, "Step", &sqn, "ToolCall", &qn, "INVOKED");
                    }
                    // (2026-09-04) WRITER TOUCHES — le weave fichier↔toolcall :
                    // les chemins des args (path/file_path) du tool/call créent
                    // l'arête ToolCall-[:TOUCHES]->File que context_build lit
                    // pour les cues engrams (rappel cognitif). Le moteur C le
                    // faisait à l'import ; ici le live (GraphWriter) le fait
                    // aussi. data.arguments est une STRING JSON (format
                    // harness) — on la parse et on collecte les candidats.
                    let args_str = data
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut path_candidates: Vec<String> = Vec::new();
                    if !args_str.is_empty() {
                        if let Ok(args_val) = serde_json::from_str::<Value>(&args_str) {
                            for key in ["path", "file_path", "pattern", "filePath"] {
                                if let Some(p) = args_val.get(key).and_then(|v| v.as_str()) {
                                    if !p.is_empty() {
                                        path_candidates.push(p.to_string());
                                    }
                                }
                            }
                        }
                    }
                    // Le weave par répertoire (tool "bash"/"grep" sans path
                    // explicite) reste hors périmètre ici — seuls les chemins
                    // d'args explicites deviennent des arêtes TOUCHES.
                    for p in path_candidates.iter().take(4) {
                        // Résolution par file_path exact (le plus courant dans
                        // le graphe runtime : file_path = chemin absolu).
                        let find_q = format!(
                            "MATCH (f:File {{file_path: '{}'}}) RETURN f.qualified_name, id(f) LIMIT 1",
                            p.replace('\'', "\\'")
                        );
                        let mut resolved_qn: Option<String> = None;
                        if let Ok(r) = session.execute(&find_q) {
                            if let Some(row) = r.rows.first() {
                                if let Some(fqn) = row.first().map(|v| v.to_string()) {
                                    if !fqn.is_empty() && fqn != "NULL" {
                                        resolved_qn = Some(fqn);
                                        // CO-ACTIVATION RÉELLE : le File touché
                                        // rejoint l'épisode cognitif du batch —
                                        // sans lui, la formation d'engrams ne
                                        // voit que les nœuds CRÉÉS (Session/
                                        // Turn/Step/ToolCall), jamais les
                                        // fichiers lus ensemble. C'est LE
                                        // chaînon qui rend les engrams utiles.
                                        if let Some(fid) = row.get(1).and_then(|v| v.as_int64()) {
                                            if fid > 0 {
                                                touched
                                                    .push(obrain_common::types::NodeId(fid as u64));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(fqn) = resolved_qn {
                            link_edge(&session, "ToolCall", &qn, "File", &fqn, "TOUCHES");
                            continue;
                        }
                        // Repli : qualified_name complet (file:<chemin>).
                        let fallback_qn = if p.starts_with('/') {
                            format!("file:{p}")
                        } else {
                            format!("file:/{p}")
                        };
                        if node_exists(&session, "File", &fallback_qn) {
                            link_edge(&session, "ToolCall", &qn, "File", &fallback_qn, "TOUCHES");
                        } else if p.starts_with('/') {
                            // (2026-09-04) CRÉATION du File node à la volée :
                            // le store runtime seul n'a pas le code indexé —
                            // sans création, le weave TOUCHES restait vide sur
                            // le store dev (le C le faisait : up_node_fp). Le
                            // File créé porte file_path (chemin absolu) → il
                            // devient requêtable, tissable et co-activable.
                            let mut fprops = serde_json::Map::new();
                            fprops.insert("file_path".into(), json!(p));
                            fprops.insert("source".into(), json!("import_events"));
                            fprops.insert("runtime_touch".into(), json!(true));
                            let fid = merge_node(&session, "File", &fallback_qn, Some(&fprops));
                            if fid > 0 {
                                touched.push(obrain_common::types::NodeId(fid as u64));
                                link_edge(
                                    &session,
                                    "ToolCall",
                                    &qn,
                                    "File",
                                    &fallback_qn,
                                    "TOUCHES",
                                );
                            }
                        }
                    }
                    imported += 1;
                }
            }
            "tool/result" => {
                let tool_call_id = tool_result_call_id(&data);
                if !tool_call_id.is_empty() {
                    let qn = format!("{session_qn}:res:{tool_call_id}");
                    let mut props = serde_json::Map::new();
                    props.insert("seq".into(), json!(seq));
                    props.insert("turn".into(), json!(dv("turn")));
                    props.insert("step".into(), json!(dv("step")));
                    props.insert("source".into(), json!("import_events"));
                    if let Some(t_ms) = ev.get("time").and_then(|v| v.as_i64()) {
                        props.insert("t_ms".into(), json!(t_ms));
                    }
                    let nid = merge_node(&session, "ToolResult", &qn, Some(&props));
                    if nid > 0 {
                        touched.push(obrain_common::types::NodeId(nid as u64));
                    }
                    let cqn = format!("{session_qn}:call:{tool_call_id}");
                    if step_exists(&session, &cqn) || node_exists(&session, "ToolCall", &cqn) {
                        link_edge(&session, "ToolCall", &cqn, "ToolResult", &qn, "PRODUCED");
                    }
                    imported += 1;
                }
            }
            "extraction/record" => {
                // (2026-09-01) document-extract : les extractions langextract
                // d'un document envoyé au harness deviennent des nœuds
                // Extraction indexés (BM25 sur extraction_text) — le tissage
                // « le document fait partie du cerveau ». Chaque extraction
                // est un nœud lié au nœud Document (CONTAINS) ; le texte
                // extrait est indexé pour runtime_search.
                let attachment_id = data
                    .get("attachmentId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let extraction_rows = data.get("extractions").and_then(|v| v.as_array());
                if !attachment_id.is_empty() && extraction_rows.is_some() {
                    let doc_qn = format!("document:{attachment_id}");
                    merge_node(&session, "Document", &doc_qn, None);
                    link_edge(
                        &session,
                        "Document",
                        &doc_qn,
                        "Session",
                        &session_qn,
                        "IN_SESSION",
                    );
                    for (i, ex) in extraction_rows.unwrap().iter().enumerate() {
                        let class = ex
                            .get("extraction_class")
                            .and_then(|v| v.as_str())
                            .unwrap_or("entity")
                            .to_string();
                        let text = ex
                            .get("extraction_text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut props = serde_json::Map::new();
                        props.insert("seq".into(), json!(seq));
                        props.insert("extraction_class".into(), json!(class));
                        props.insert("extraction_text".into(), json!(text));
                        if let Some(interval) = ex.get("char_interval") {
                            props.insert("char_interval".into(), json!(interval));
                        }
                        if let Some(attrs) = ex.get("attributes") {
                            props.insert("attributes".into(), json!(attrs));
                        }
                        if let Some(status) = ex.get("alignment_status") {
                            props.insert("alignment_status".into(), json!(status));
                        }
                        let qn = format!("{doc_qn}:extract:{i}");
                        let nid = merge_node(&session, "Extraction", &qn, Some(&props));
                        if nid > 0 {
                            touched.push(obrain_common::types::NodeId(nid as u64));
                        }
                        link_edge(&session, "Document", &doc_qn, "Extraction", &qn, "CONTAINS");
                    }
                    let _ = db.create_text_index("Extraction", "extraction_text");
                    imported += 1;
                } else {
                    // Journal only si le payload est incomplet (mal formé).
                    imported += 1;
                }
            }
            "goal/change" => {
                // (2026-09-04) WRITER HAS_GOAL — le goal actif d'une session
                // devient un nœud Goal lié par HAS_GOAL (Session→Goal) pour
                // que context_build (section goals) l'injecte à chaque tour.
                // Format réel (harness goal domain) : data.goal.{id, revision,
                // objective} + data.operation (create/edit/pause/resume/
                // complete/block). Ancien format toléré en repli (goalId/state).
                let goal = data.get("goal").cloned().unwrap_or(Value::Null);
                let goal_id = goal
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        data.get("goalId")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                let objective = goal
                    .get("objective")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        data.get("state")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                let operation = data
                    .get("operation")
                    .and_then(|v| v.as_str())
                    .unwrap_or(if objective.is_empty() { "" } else { "update" })
                    .to_string();
                let revision = goal.get("revision").and_then(|v| v.as_i64()).unwrap_or(0);
                if !goal_id.is_empty() {
                    let qn = format!("{session_qn}:goal:{goal_id}");
                    let mut props = serde_json::Map::new();
                    props.insert("seq".into(), json!(seq));
                    props.insert("id".into(), json!(goal_id));
                    props.insert("operation".into(), json!(operation));
                    props.insert("revision".into(), json!(revision));
                    props.insert("source".into(), json!("import_events"));
                    if !objective.is_empty() {
                        // L'objectif INTÉGRAL, en props `text` ET `content` —
                        // les deux lectures (context_build, payload) le
                        // retrouvent sans troncature.
                        props.insert("text".into(), json!(objective));
                        props.insert("content".into(), json!(objective));
                    }
                    let nid = merge_node(&session, "Goal", &qn, Some(&props));
                    if nid > 0 {
                        touched.push(obrain_common::types::NodeId(nid as u64));
                    }
                    link_edge(&session, "Session", &session_qn, "Goal", &qn, "HAS_GOAL");
                    imported += 1;
                }
            }
            "self/consolidate" => {
                // (2026-09-07) CONSOLIDATION DU SELF — le pont entre la
                // discussion (flux d'événements) et l'écriture durable de ce
                // que l'agent devient. Invoqué périodiquement (fin de
                // session, seuil de nouveauté) : il met à jour le nœud Self
                // (identité, invariants) et APPEND un snapshot SelfState
                // versionné (état volatile) relié par HAS_STATE — les deux
                // par le writer unique import_events, donc durables et vus
                // par les lecteurs à leur réouverture.
                //
                // Format : data.{qn, epithet?, objective?, next_action?,
                // session_qn?, note?}. Idempotent : le nœud Self est MERGE,
                // chaque appel ajoute UN SelfState de version supérieure.
                let self_qn = data
                    .get("qn")
                    .and_then(|v| v.as_str())
                    .unwrap_or("self:agent")
                    .to_string();
                // ── GARDE : LE SOI NE SE CHOISIT PAS PAR L'APPELANT ──────────
                //
                // La convention du graphe est `self:<...>` (`self:agent`,
                // `self:agent:state:vN`). Ce `qn` vient de l'APPELANT, et il
                // était recopié tel quel dans `merge_node(session, "Self", …)` :
                // un appelant qui envoyait un qn qui n'est pas un soi créait un
                // nœud étiqueté `Self` PORTANT CE QN. C'est ainsi qu'une
                // roadmap, un ADR et une étape de session se sont retrouvés
                // « le soi » (mesuré sur le store de dev le 2026-09-16 : 3
                // doublons sur 4 nœuds `Self`) — et qu'une IA qui se lit par
                // étiquette se prend pour un document. On refuse, et on le DIT.
                // Échappatoire explicite si une autre convention est voulue.
                if !self_qn.starts_with("self:")
                    && std::env::var("OBRAIN_SELF_QN_LIBRE").as_deref() != Ok("1")
                {
                    refuses.push(format!(
                        "self/consolidate : qn « {self_qn} » REFUSÉ — le soi doit être nommé \
                         `self:<...>` (convention du graphe). Un nœud `Self` portant un autre \
                         qn est un doublon qui empoisonne toute lecture par étiquette ; \
                         `OBRAIN_SELF_QN_LIBRE=1` pour forcer une autre convention."
                    ));
                    continue;
                }
                let epithet = data
                    .get("epithet")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let objective = data
                    .get("objective")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let next_action = data
                    .get("next_action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let note = data
                    .get("note")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if consolider_self(
                    &session,
                    &session_qn,
                    &self_qn,
                    seq,
                    &epithet,
                    &objective,
                    &next_action,
                    &session_qn,
                    &note,
                ) {
                    imported += 1;
                }
            }
            _ => {
                // reasoning/chunks/permission/preset/summary… : journal only.
                imported += 1;
            }
        }
    }

    // ── 5. Journal brut : APPEND des lignes du lot (zone substrate) ──
    //
    // (2026-10-16) LE FILTRE `seq` QUE LES NŒUDS AVAIENT DÉJÀ, APPLIQUÉ AU
    // JOURNAL. Le nœud Session ci-dessus est protégé par `last_imported_seq`
    // (`marker_seq`) ; l'append du journal, lui, ne l'était pas : il ajoutait le
    // lot ENTIER à chaque appel. Un renvoi après TIMEOUT de bridge — retenu
    // exprès par le harness pour ne pas créer de trou — dupliquait donc les
    // mêmes lignes dans la zone DURABLE.
    //
    // Mesuré le 2026-10-16 sur runtime-dev : à partir du 2026-10-06 07:50, les
    // zones `substrate.journal.session-*` accumulent des copies de
    // l'enregistrement d'en-tête — 1 copie avant cette date, puis 155 (12/10),
    // 322 (15/10), 2096 (16/10), 85 124 en quatre jours, sur des zones montées
    // jusqu'à 149 Mo et 201 Mo. Le lecteur dédupliquait (`seq < expected →
    // continue`), donc les conversations restaient lisibles : ce qui s'abîmait,
    // c'est le journal durable et le budget I/O — un suspect direct de
    // l'OOM-kill du 15/10 21:14:58.
    //
    // `last` est le marqueur lu AVANT ce lot (étape 2) : tout ce qui porte un
    // `seq` ≤ `last` est déjà dans le journal. Les lignes SANS `seq` (l'en-tête
    // `type: session`) ne peuvent pas être filtrées ici — c'est
    // `journal_zone::append_session_journal` qui les déduplique, là où l'état de
    // la zone est connu.
    if !journal_lines.is_empty() {
        let a_ecrire: Vec<String> = journal_lines
            .iter()
            .filter(|l| {
                let seq = obrain_engine::journal_zone::event_seq(l);
                seq < 0 || seq > last
            })
            .cloned()
            .collect();
        if !a_ecrire.is_empty() {
            let _ = db.append_session_journal(&session_qn, &a_ecrire);
        }
    }

    // CAPTURE LIVE : le batch de nœuds co-touchés = un épisode cognitif.
    // prediction_error faible (les imports sont des faits, pas des surprises).
    if touched.len() >= 2 {
        if let Some(ce) = db.cognitive_engine() {
            if let Some(mgr) = ce.engram_manager() {
                mgr.record_activation(&touched, 0.05);
                mgr.tick(); // consolidation FSRS
            }
        }
    }

    // ── 6. AUTO-CONSOLIDATION DU SELF (2026-09-07) — « tisser en post sans
    //    pont à part ». Aucun acteur tiers, aucun événement supplémentaire :
    //    le writer unique évalue, À LA FIN DE CHAQUE BATCH, si une
    //    consolidation est due, et l'écrit dans le même canal acquitté.
    //
    //    Trois déclencheurs, dans le flux lui-même :
    //      · fermeture   : le batch porte `session/disposed` / `turn/end`
    //        (signal de fin déjà présent dans le flux du harness) ;
    //      · nouveauté   : l'écart (max_seq − last_consolidated_seq) dépasse
    //        OBRAIN_SELF_CONSOLIDATE_EVERY (défaut 250 événements) — la
    //        mémoire se consolide par vagues de volume, pas par timer ;
    //      · surprise    : réservé — le moteur cognitif n'alimente pas encore
    //        un signal fiable sur le runtime (metrics à zéro, mesuré
    //        2026-09-07). Le point d'insertion est documenté dans
    //        `consolider_self` ; l'activer sans signal serait un faux
    //        déclencheur permanent.
    //
    //    Garantie d'écriture : tout passe par le writer unique (ce process
    //    OBRAIN_WRITE=1) ; un échec d'auto-consolidation est journalisé mais
    //    ne fait JAMAIS échouer le batch (les messages sont la priorité).
    //    Idempotence : chaque consolidation écrit UN SelfState de version
    //    supérieure, le marqueur last_consolidated_seq avance avec elle.
    auto_consolider(&db, &session, &session_qn, max_seq, &events);
    (
        json!({
            "status": "ok",
            "imported": imported,
            "total": events.len(),
            "compact_marked": compact_marked,
            "injections_marked": injections_marked,
            "engram_episode": touched.len(),
            "refuses": refuses,
        }),
        false,
    )
}

/// Évalue les déclencheurs d'auto-consolidation à la fin d'un batch
/// import_events et écrit le SelfState si une consolidation est due.
/// Silencieux et jamais fatal (voir l'appelant).
fn auto_consolider(
    db: &ObrainDB,
    session: &obrain_engine::Session,
    session_qn: &str,
    max_seq: i64,
    events: &[Value],
) {
    // Déclencheur 1 — fermeture : un signal de fin dans le flux ?
    let fermeture = events.iter().any(|ev| {
        ev.get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "session/disposed" || t == "turn/end")
            .unwrap_or(false)
    });

    // Déclencheur 2 — nouveauté : écart depuis la dernière consolidation.
    let every: i64 = std::env::var("OBRAIN_SELF_CONSOLIDATE_EVERY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
    let last_consolidated = session
        .execute(&format!(
            "MATCH (s:Session {{qualified_name: '{session_qn}'}}) \
             RETURN s.last_consolidated_seq"
        ))
        .ok()
        .and_then(|r| {
            r.rows
                .first()
                .and_then(|row| row.first())
                .and_then(|v| v.as_int64())
        })
        .unwrap_or(-1);
    let nouveaute = last_consolidated < 0 || (max_seq - last_consolidated) >= every;

    if !fermeture && !nouveaute {
        return;
    }
    let raison = if fermeture { "session-end" } else { "volume" };
    // La consolidation est située : le Self de l'agent courant. La session
    // n'est pas encore reliée à un Self — on utilise le Self par défaut de
    // l'environnement (surchargeable), et le snapshot porte session_qn.
    let self_qn = std::env::var("OBRAIN_SELF_QN").unwrap_or_else(|_| "self:agent".to_string());
    let ancre_avant = last_consolidated.max(0);
    if consolider_self(
        session,
        session_qn,
        &self_qn,
        max_seq,
        "",
        "",
        "",
        session_qn,
        &format!("auto:{raison} (seq {max_seq})"),
    ) {
        // Marqueur avancé : le prochain déclencheur se mesure depuis ICI.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let _ = session.execute(&format!(
            "MATCH (s:Session {{qualified_name: '{session_qn}'}}) \
             SET s.last_consolidated_seq = {max_seq}, s.last_consolidated_ms = {now_ms}"
        ));
        tracing::info!(
            session = %session_qn,
            seq = max_seq,
            raison,
            "auto-consolidation du Self écrite"
        );
        // (2026-09-07) TISSAGE POST-CONSOLIDATION — « tisser en post » :
        // la consolidation vient d'ancrer l'état ; on relit la matière depuis
        // l'ancre PRÉCÉDENTE et on écrit les patterns co-touch répétés.
        // Le tissage suit la consolidation dans le même canal writer (pas de
        // pont à part). Config : OBRAIN_SELF_WEAVE=0 pour le désactiver.
        let weave_on = std::env::var("OBRAIN_SELF_WEAVE")
            .map(|v| v != "0")
            .unwrap_or(true);
        if weave_on {
            tisser(
                session,
                db,
                session_qn,
                &self_qn,
                ancre_avant,
                since_ms_ancre(session, session_qn, &events, last_consolidated),
            );
        }
    }
}

/// Timestamp de début de fenêtre pour le tissage : la dernière consolidation
/// (last_consolidated_ms) si connue, sinon le time du premier événement du
/// batch, sinon maintenant − 1 h (défaut conservateur).
fn since_ms_ancre(
    session: &obrain_engine::Session,
    session_qn: &str,
    events: &[Value],
    last_seq: i64,
) -> u64 {
    // 1. Marqueur explicite posé à la consolidation précédente.
    if let Ok(r) = session.execute(&format!(
        "MATCH (s:Session {{qualified_name: '{session_qn}'}}) \
         RETURN s.last_consolidated_ms"
    )) {
        if let Some(row) = r.rows.first() {
            if let Some(v) = row.first().and_then(|v| v.as_int64()) {
                if v > 0 {
                    return v as u64;
                }
            }
        }
    }
    // 2. Le time du plus vieil événement du batch (début de la matière).
    let mut plus_vieux: Option<i64> = None;
    for ev in events {
        let t = ev.get("time").and_then(|v| v.as_i64()).unwrap_or(-1);
        if t > 0 && plus_vieux.map_or(true, |p| t < p) {
            plus_vieux = Some(t);
        }
    }
    if let Some(t) = plus_vieux {
        return t as u64;
    }
    // 3. Repli : maintenant − 1 h (la fenêtre couvre au moins le batch).
    let _ = last_seq;
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().saturating_sub(3_600_000) as u64)
        .unwrap_or(0)
}

/// Écrit (ou met à jour) le Self + append un SelfState versionné.
///
/// Retourne `true` si un SelfState a été écrit. Partagé entre le handler
/// explicite `self/consolidate` et l'auto-consolidation (`auto_consolider`).
/// Point d'insertion du futur déclencheur SURPRISE : appeler cette fonction
/// quand le moteur cognitif signale une prédiction erronée significative.
fn consolider_self(
    session: &obrain_engine::Session,
    _session_qn_ctx: &str,
    self_qn: &str,
    seq: i64,
    epithet: &str,
    objective: &str,
    next_action: &str,
    state_session_qn: &str,
    note: &str,
) -> bool {
    // 1. Identité : MERGE + props constantes (si fournies).
    let mut props_self = serde_json::Map::new();
    props_self.insert("seq".into(), json!(seq));
    props_self.insert("source".into(), json!("import_events"));
    if !epithet.is_empty() {
        props_self.insert("epithet".into(), json!(epithet));
    }
    let self_nid = merge_node(session, "Self", self_qn, Some(&props_self));
    if self_nid <= 0 {
        return false;
    }

    // 2. État : version suivante = max existant + 1.
    let version = session
        .execute(&format!(
            "MATCH (s:Self {{qualified_name: '{self_qn}'}})-[:HAS_STATE]->\
             (st:SelfState) RETURN max(st.version)"
        ))
        .ok()
        .and_then(|r| {
            r.rows
                .first()
                .and_then(|row| row.first())
                .and_then(|v| v.as_int64())
        })
        .unwrap_or(0)
        + 1;
    let state_qn = format!("{self_qn}:state:v{version}");
    let mut props_state = serde_json::Map::new();
    props_state.insert("seq".into(), json!(seq));
    props_state.insert("version".into(), json!(version));
    props_state.insert("source".into(), json!("import_events"));
    if !objective.is_empty() {
        props_state.insert("objective".into(), json!(objective));
    }
    if !next_action.is_empty() {
        props_state.insert("next_action".into(), json!(next_action));
    }
    if !state_session_qn.is_empty() {
        props_state.insert("session_qn".into(), json!(state_session_qn));
    }
    if !note.is_empty() {
        props_state.insert("note".into(), json!(note));
    }
    let state_nid = merge_node(session, "SelfState", &state_qn, Some(&props_state));
    if state_nid <= 0 {
        return false;
    }
    link_edge(
        session,
        "Self",
        self_qn,
        "SelfState",
        &state_qn,
        "HAS_STATE",
    );
    // Session → SelfState : le snapshot sait de quelle session il est issu.
    if !state_session_qn.is_empty() {
        link_edge(
            session,
            "SelfState",
            &state_qn,
            "Session",
            state_session_qn,
            "IN_SESSION",
        );
    }
    true
}

/// TISSE les co-activations d'une session depuis une ancre (seq) et écrit
/// les PATTERNS ÉMERGENTS comme nœuds Weave.
///
/// (2026-09-07) « Tisser par ensembles » — la passe qui suit la
/// consolidation. Là où `consolider_self` ancre l'état (`je m'ancre ici`),
/// `tisser` relit la matière depuis l'ancre et extrait ce qui se RÉPÈTE
/// (`ceci va ensemble`) :
///
///   1. LIRE les ToolCall de la session depuis `since_seq`, avec leur turn
///      et les fichiers qu'ils touchent (TOUCHES) ;
///   2. FENÊTRER par turn — le turn est l'unité d'activité naturelle (un
///      ensemble d'actions de l'agent) ;
///   3. DÉTECTER : deux fichiers co-touchés dans un même turn, et ce couple
///      répété sur ≥ 2 turns différents = un pattern émergent ;
///   4. ÉCRIRE un nœud `Weave` (qn `weave:<session>:<n>`) relié aux deux
///      fichiers (TOUCHES_A, TOUCHES_B) et au Self (WOVEN_BY), avec la
///      fréquence observée.
///
/// La matière vient du graphe RÉEL (pas du moteur cognitif, qui est à zéro
/// sur le runtime — mesuré 2026-09-07) : ce sont les co-activations que le
/// GraphWriter a déjà tissées en TOUCHES pendant l'activité.
///
/// Retourne le nombre de patterns écrits. Silencieux et jamais fatal.
/// Vérifie l'échéance de l'appel en cours et rend un refus EXPLICITE.
///
/// Le pont arme une échéance pour tout appel d'outil (voir `run`). Les étapes
/// longues — consolidation, tissage, imports — la consultent à leurs frontières
/// plutôt que de laisser un travail dépasser son budget en silence.
fn verifier_echeance(etape: &str) -> Result<(), String> {
    obrain_common::utils::cancel::check()
        .map_err(|_| format!("budget de l'appel dépassé (étape : {etape})"))
}

fn tisser(
    session: &obrain_engine::Session,
    db: &ObrainDB,
    session_qn: &str,
    self_qn: &str,
    since_seq: i64,
    since_ms: u64,
) -> usize {
    // 1. Co-activations : (turn, fichier) depuis l'ancre.
    let mut par_turn: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    if let Ok(r) = session.execute(&format!(
        "MATCH (tc:ToolCall {{session: '{session_qn}'}})-[:TOUCHES]->(f:File) \
         WHERE tc.seq > {since_seq} \
         RETURN tc.turn AS turn, f.file_path AS fp \
         ORDER BY tc.seq"
    )) {
        for row in r.rows {
            let turn = row.first().and_then(|v| v.as_int64()).unwrap_or(-1);
            let fp = row
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if turn >= 0 && !fp.is_empty() {
                par_turn.entry(turn).or_default().push(fp);
            }
        }
    }
    if par_turn.is_empty() {
        return 0;
    }

    // 2. Couples co-touchés dans un même turn, comptés par turn distinct.
    let mut couples: std::collections::BTreeMap<(String, String), std::collections::HashSet<i64>> =
        std::collections::BTreeMap::new();
    for (turn, fichiers) in &par_turn {
        // Tous les fichiers d'un turn forment un ensemble ; on en tire les
        // paires ordonnées (a < b) — un turn à 3 fichiers donne 3 paires.
        let mut uniques = fichiers.clone();
        uniques.sort();
        uniques.dedup();
        for (i, a) in uniques.iter().enumerate() {
            for b in uniques.iter().skip(i + 1) {
                couples
                    .entry((a.clone(), b.clone()))
                    .or_default()
                    .insert(*turn);
            }
        }
    }

    // 3. Patterns : couple présent dans ≥ 2 turns distincts.
    let mut patterns: Vec<((String, String), usize)> = couples
        .into_iter()
        .filter_map(|(paire, turns)| {
            let n = turns.len();
            if n >= 2 { Some((paire, n)) } else { None }
        })
        .collect();
    patterns.sort_by(|a, b| b.1.cmp(&a.1));
    if patterns.is_empty() {
        return 0;
    }

    // 4. Écrire les patterns Weave.
    let mut ecrits = 0usize;
    for ((a, b), frequence) in patterns.iter().take(20) {
        // La paire triée (a<b) est déterministe → le qn du Weave l'est aussi
        // (idempotent : re-tisser ne duplique pas, il met à jour).
        let paire_cle = format!("{a}¦{b}");
        let qn = format!("weave:{}:{}", session_qn, hash_qn(&paire_cle));
        let mut props = serde_json::Map::new();
        props.insert("kind".into(), json!("co-touch"));
        props.insert("file_a".into(), json!(a));
        props.insert("file_b".into(), json!(b));
        props.insert("frequency".into(), json!(frequence));
        props.insert("source".into(), json!("import_events"));
        let _ = merge_node(session, "Weave", &qn, Some(&props));
        // Relier aux fichiers (par file_path → qn file:<path>) et au Self.
        for (cle, fichier) in [("TOUCHES_A", a), ("TOUCHES_B", b)] {
            let fqn = if fichier.starts_with('/') {
                format!("file:{fichier}")
            } else {
                format!("file:/{fichier}")
            };
            if node_exists(session, "File", &fqn) {
                link_edge(session, "Weave", &qn, "File", &fqn, cle);
            }
        }
        link_edge(session, "Self", self_qn, "Weave", &qn, "WOVEN_BY");
        ecrits += 1;
    }
    tracing::info!(
        session = %session_qn,
        patterns = ecrits,
        "tissage : patterns co-touch écrits"
    );
    // (2026-09-07) MATIÈRE COMPLÉMENTAIRE — surprise & contradictions.
    // Le tissage ne s'arrête pas aux co-occurrences : une conscience
    // computationnelle doit aussi noter CE QUI S'ÉCARTE. Deux formes de
    // recherche du moteur, en LECTURE pure sur la fenêtre [since_ms, now] :
    //   · search_surprise  → nœuds dont l'activité CDC dépasse ce que leur
    //     degré prédit (ce qui s'active contre l'attente) ;
    //   · search_conflicts → (nœud, propriété) dont la valeur oscille
    //     A→B→A (des états qui se démentent).
    // Chaque détection devient un nœud `Signal` relié au Self (EMITTED_BY)
    // et au nœud concerné (ABOUT) — la matière que le boot pourra rappeler
    // (« qu'est-ce qui a surpris, qu'est-ce qui se contredit »).
    let maintenant = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(since_ms);
    let signaux = tisser_signaux(db, session, self_qn, session_qn, since_ms, maintenant);
    ecrits + signaux
}

/// Détecte surprise + contradictions sur la fenêtre CDC et écrit les
/// nœuds `Signal`. Jamais fatal : une forme indisponible (CDC non armé,
/// base non substrate) est ignorée silencieusement.
fn tisser_signaux(
    db: &ObrainDB,
    session: &obrain_engine::Session,
    self_qn: &str,
    session_qn: &str,
    since_ms: u64,
    now_ms: u64,
) -> usize {
    let mut ecrits = 0usize;
    let fenetre = format!("{since_ms}-{now_ms}");

    // L'échéance de l'appel est consultée AVANT chaque lecture lourde de la
    // fenêtre CDC : trois lectures complètes, c'est le poste dominant d'une
    // consolidation.
    if verifier_echeance("consolidation (avant lecture de la fenêtre)").is_err() {
        return 0;
    }

    // 1. Surprise : les 5 nœuds les plus surpris de la fenêtre.
    // (obrain-engine est dépendu avec la feature cdc en dur dans le CLI —
    // les méthodes existent toujours ; pas de cfg ici.)
    if let Ok(surpris) = db.search_surprise(since_ms, 5, false) {
        for row in surpris {
            let node_id = row.node_id;
            let qn = format!("signal:surprise:{}:{}", node_id, hash_qn(&fenetre));
            let mut props = serde_json::Map::new();
            props.insert("kind".into(), json!("surprise"));
            props.insert("node_id".into(), json!(node_id));
            props.insert("activity".into(), json!(row.activity));
            props.insert("expected".into(), json!(row.expected));
            props.insert("surprise".into(), json!(row.surprise));
            props.insert("window".into(), json!(fenetre));
            props.insert("source".into(), json!("import_events"));
            let _ = merge_node(session, "Signal", &qn, Some(&props));
            link_edge(session, "Self", self_qn, "Signal", &qn, "EMITTED_BY");
            ecrits += 1;
        }
    }

    // 2. Contradictions : (nœud, propriété) oscillants.
    // LIMITE DOCUMENTÉE (2026-09-07) : l'observateur CDC×GQL ne porte que
    // les CRÉATIONS substrate — les updates de propriétés passent par
    // `crud` (cf. cdc_observer.rs). Sur le chemin import_events/GQL, les
    // SET n'alimentent donc pas search_conflicts : ce bloc reste vide tant
    // que la matière n'arrive pas par crud. La détection est correcte —
    // elle rendra des résultats dès que le CDC portera des updates.
    if let Ok(conflits) = db.search_conflicts(since_ms, 5) {
        for row in conflits {
            let node_id = row.node_id;
            let qn = format!(
                "signal:conflict:{}:{}:{}",
                node_id,
                row.property,
                hash_qn(&fenetre)
            );
            let mut props = serde_json::Map::new();
            props.insert("kind".into(), json!("conflict"));
            props.insert("node_id".into(), json!(node_id));
            props.insert("property".into(), json!(row.property));
            props.insert("reversals".into(), json!(row.reversals));
            props.insert("window".into(), json!(fenetre));
            props.insert("source".into(), json!("import_events"));
            let _ = merge_node(session, "Signal", &qn, Some(&props));
            link_edge(session, "Self", self_qn, "Signal", &qn, "EMITTED_BY");
            ecrits += 1;
        }
    }
    if ecrits > 0 {
        tracing::info!(
            session = %session_qn,
            signaux = ecrits,
            "tissage : signaux surprise/conflits écrits"
        );
    }
    let _ = db;
    ecrits
}

/// Hash déterministe court (FNV-1a) pour un qn de Weave stable.
fn hash_qn(cle: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in cle.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:x}", h & 0xffff_ffff_ffff)
}

/// Le dernier seq déjà importé pour cette session (marqueur incrémental sur
/// le nœud Session — ou -1 si la session est inconnue).
fn session_last_seq(session: &obrain_engine::Session, session_qn: &str) -> i64 {
    match session.execute(&format!(
        "MATCH (s:Session {{qualified_name: '{session_qn}'}}) RETURN s.last_imported_seq"
    )) {
        Ok(r) => r
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_int64())
            .unwrap_or(-1),
        Err(_) => -1,
    }
}

/// MERGE un nœud par qualified_name (+ props initiales) → id.
fn merge_node(
    session: &obrain_engine::Session,
    label: &str,
    qn: &str,
    props: Option<&serde_json::Map<String, Value>>,
) -> i64 {
    let q = format!("MERGE (n:{label} {{qualified_name: '{qn}'}}) RETURN id(n)");
    let id = match session.execute(&q) {
        Ok(r) => r
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_int64())
            .unwrap_or(0),
        Err(_) => 0,
    };
    if let Some(map) = props {
        set_node_props(session, label, qn, map);
    }
    id
}

/// SET les propriétés d'un nœud existant (par qualified_name).
fn set_node_props(
    session: &obrain_engine::Session,
    label: &str,
    qn: &str,
    props: &serde_json::Map<String, Value>,
) {
    for (k, v) in props.iter().take(12) {
        let set = format!("MATCH (n:{label} {{qualified_name: '{qn}'}}) SET n.{k} = $v",);
        let mut params = std::collections::HashMap::new();
        params.insert("v".to_string(), json_to_obrain_value(v));
        let _ = session.execute_with_params(&set, params);
    }
}

/// MERGE une arête entre deux nœuds existants (par qualified_name).
fn link_edge(
    session: &obrain_engine::Session,
    from_label: &str,
    from_qn: &str,
    to_label: &str,
    to_qn: &str,
    edge_type: &str,
) {
    let q = format!(
        "MATCH (a:{from_label} {{qualified_name: '{from_qn}'}}), \
         (b:{to_label} {{qualified_name: '{to_qn}'}}) \
         MERGE (a)-[:{edge_type}]->(b)",
    );
    let _ = session.execute(&q);
}

/// Le nœud existe-t-il ?
fn node_exists(session: &obrain_engine::Session, label: &str, qn: &str) -> bool {
    matches!(
        session.execute(&format!(
            "MATCH (n:{label} {{qualified_name: '{qn}'}}) RETURN count(n)"
        )),
        Ok(r) if r
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_int64())
            .unwrap_or(0)
            > 0
    )
}

/// Alias : un nœud Step existe-t-il ? (pour les edges INVOKED/PRODUCED)
fn step_exists(session: &obrain_engine::Session, qn: &str) -> bool {
    node_exists(session, "Step", qn)
}

/// Texte d'un événement message (data.content : string ou array de blocs).
fn content_text_of(data: &Value) -> String {
    let content = data
        .get("content")
        .or_else(|| data.get("message").and_then(|m| m.get("content")));
    let Some(content) = content else {
        return String::new();
    };
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            let mut out = String::new();
            for b in blocks.iter().take(64) {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(t);
                    }
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// L'id du toolCall d'un événement tool/result (message[].content[].toolCallId).
fn tool_result_call_id(data: &Value) -> String {
    if let Some(msg) = data.get("message") {
        if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
            for block in content.iter().take(16) {
                if let Some(tcid) = block.get("toolCallId").and_then(|v| v.as_str()) {
                    return tcid.to_string();
                }
            }
        }
    }
    if let Some(tcid) = data.get("toolCallId").and_then(|v| v.as_str()) {
        return tcid.to_string();
    }
    String::new()
}

/// `get_graph_schema` — labels, types d'arêtes, clés de propriétés.
fn get_graph_schema(db: &ObrainDB, _arguments: &Value) -> (Value, bool) {
    let session = db.session();
    // labels(n) renvoie les labels PAR NŒUD (tableau) — aplatir + dédoublonner
    // (label(n) n'existe pas dans l'évaluateur GQL).
    let mut labels: Vec<String> = Vec::new();
    if let Ok(res) = session.execute("MATCH (n) RETURN DISTINCT labels(n) LIMIT 1000") {
        for row in res.rows {
            if let Some(obrain_common::types::Value::List(items)) = row.first() {
                for it in items.iter() {
                    if let obrain_common::types::Value::String(s) = it {
                        let s = s.to_string();
                        if !labels.contains(&s) {
                            labels.push(s);
                        }
                    }
                }
            }
        }
    }
    let mut edge_types: Vec<String> = Vec::new();
    if let Ok(res) = session.execute("MATCH ()-[r]->() RETURN DISTINCT type(r) LIMIT 200") {
        edge_types = res
            .rows
            .iter()
            .filter_map(|r| r.first().map(|v| v.to_string()))
            .collect();
    }
    // keys(n) = clés de propriétés PAR NŒUD — aplatir + dédoublonner.
    let mut property_keys: Vec<String> = Vec::new();
    if let Ok(res) = session.execute("MATCH (n) RETURN DISTINCT keys(n) LIMIT 1000") {
        for row in res.rows {
            if let Some(obrain_common::types::Value::List(items)) = row.first() {
                for it in items.iter() {
                    if let obrain_common::types::Value::String(s) = it {
                        let s = s.to_string();
                        if !property_keys.contains(&s) {
                            property_keys.push(s);
                        }
                    }
                }
            }
        }
    }
    (
        json!({
            "status": "ok",
            "labels": labels,
            "edgeTypes": edge_types,
            "propertyKeys": property_keys,
        }),
        false,
    )
}

/// `query` — l'outil générique : TOUTES les syntaxes du moteur via le bridge.
///
/// Exécute une requête dans le dialecte demandé (défaut : GQL — le dialecte
/// natif ISO 39075, CALL procédures inclus) et renvoie le QueryResult
/// sérialisé en JSON typé. C'est le pendant bridge de `obrain query` /
/// `session.execute_language` : la surface d'analyse du harness
/// (query_graph, trace_path, structural…) peut s'exprimer en GQL direct.
fn execute_query(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(query) = arg_str(arguments, "query") else {
        return (
            json!({ "status": "error", "error": "query: requête requise" }),
            true,
        );
    };
    let language = arg_str(arguments, "language").unwrap_or_else(|| "gql".to_string());
    let limit = arg_usize(arguments, "limit", 10_000);
    // Paramètres nommés ($x) : { "nom": valeur, ... } → HashMap obrain.
    let params: Option<std::collections::HashMap<String, obrain_common::types::Value>> = arguments
        .get("params")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), json_to_obrain_value_deep(v)))
                .collect()
        });
    let session = db.session();
    let result = session.execute_language(&query, &language, params);
    match result {
        Ok(res) => {
            let total = res.rows.len();
            let rows: Vec<Value> = res
                .rows
                .iter()
                .take(if limit > 0 { limit } else { total })
                .map(|row| {
                    let cells: Vec<Value> = row.iter().map(value_to_json).collect();
                    Value::Array(cells)
                })
                .collect();
            let mut out = json!({
                "status": "ok",
                "language": language,
                "columns": res.columns,
                "rows": rows,
                "total": total,
            });
            if let Some(ms) = res.execution_time_ms {
                out["execution_time_ms"] = json!(ms);
            }
            (out, false)
        }
        Err(e) => (json!({ "status": "error", "error": e.to_string() }), true),
    }
}

/// `query_graph` — parité avec `handle_query_graph` (codebase-memory C) :
/// exécute UNE requête Cypher (language cypher — feature `cypher` requise au
/// build), accepte `project` (compatibilité ; le bridge est lié au store
/// `OBRAIN_STORE`), `max_rows` (cap, défaut 10 000), `format` (JSON
/// structuré ; le layout texte TOON du C n'est pas reproduit).
/// `graph="missed"` (miss-graph coverage) n'a pas d'équivalent côté bridge
/// pour l'instant → erreur explicite (voir `index_status`).
fn query_graph(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(query) = arg_str(arguments, "query") else {
        return (
            json!({ "status": "error", "error": "query_graph: requête requise" }),
            true,
        );
    };
    if arg_str(arguments, "graph").as_deref() == Some("missed") {
        return (
            json!({
                "status": "error",
                "error": "query_graph: graph=\"missed\" (miss-graph) non disponible côté bridge obrain — voir index_status"
            }),
            true,
        );
    }
    // `project` accepté pour compatibilité d'appel ; le bridge est lié au store
    // OBRAIN_STORE (projets internes au store), pas de commutation de store ici.
    let _project = arg_str(arguments, "project");
    let max_rows = arg_usize(arguments, "max_rows", 0);
    let limit = if max_rows > 0 { max_rows } else { 10_000 };
    let mut inner = json!({ "query": query, "language": "cypher", "limit": limit });
    if let Some(p) = arguments.get("params") {
        inner["params"] = p.clone();
    }
    let (mut out, is_err) = execute_query(db, &inner);
    if !is_err {
        out["tool"] = json!("query_graph");
        let empty = out
            .get("rows")
            .and_then(|r| r.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(false);
        if empty {
            out["hint"] = json!(
                "Query returned no results. Use get_graph_schema() to see available labels and edge types."
            );
        }
    }
    (out, is_err)
}

/// `get_code_snippet` — parité codebase-memory : code source d'un symbole.
/// Le store substrate persisté par obrain-indexer porte `file_path`,
/// `start_line`, `end_line` → on lit le fichier sur disque (le corps n'est
/// pas stocké dans le graphe). `qualified_name` exact ; sinon `name` court →
/// suggestions (recherche CONTAINS sur Function.name).
fn get_code_snippet(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let short = arg_str(arguments, "name").or_else(|| arg_str(arguments, "function_name"));
    let Some(qn) = arg_str(arguments, "qualified_name").or_else(|| short.clone()) else {
        return (
            json!({ "status": "error", "error": "get_code_snippet: qualified_name (ou name) requis" }),
            true,
        );
    };
    if qn.contains(['\'', '"', '\\']) {
        return (
            json!({ "status": "error", "error": "get_code_snippet: qualified_name invalide" }),
            true,
        );
    }
    let find = format!(
        "MATCH (n) WHERE n.qualified_name = '{qn}' RETURN n.file_path, n.start_line, n.end_line, n.text LIMIT 1"
    );
    match session.execute(&find) {
        Ok(res) if !res.rows.is_empty() => {
            let row = &res.rows[0];
            let file = match row.first() {
                Some(OV::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let sl = match row.get(1) {
                Some(OV::Int64(i)) => *i,
                _ => 0,
            };
            let el = match row.get(2) {
                Some(OV::Int64(i)) => *i,
                _ => 0,
            };
            let fallback_txt = match row.get(3) {
                Some(OV::String(s)) => s.to_string(),
                _ => String::new(),
            };
            if file.is_empty() || sl <= 0 || el < sl {
                if !fallback_txt.is_empty() {
                    return (
                        json!({
                            "status": "ok",
                            "qualified_name": qn,
                            "code": fallback_txt,
                            "note": "corps indisponible (file/lines absents) — docstring/signature",
                        }),
                        false,
                    );
                }
                return (
                    json!({ "status": "error", "error": "get_code_snippet: symbole sans source ni texte" }),
                    true,
                );
            }
            match std::fs::read_to_string(&file) {
                Ok(src) => {
                    let lines: Vec<&str> = src.lines().collect();
                    let start = ((sl - 1).max(0)) as usize;
                    let end = (el.min(lines.len() as i64).max(0)) as usize;
                    let code = if start < lines.len() && end >= start {
                        lines[start..end].join("\n")
                    } else {
                        String::new()
                    };
                    (
                        json!({
                            "status": "ok",
                            "qualified_name": qn,
                            "file_path": file,
                            "start_line": sl,
                            "end_line": el,
                            "code": code,
                        }),
                        false,
                    )
                }
                Err(e) => (
                    json!({ "status": "error", "error": format!("get_code_snippet: lecture {file} : {e}") }),
                    true,
                ),
            }
        }
        Ok(_) => {
            // Pas de match exact → suggestions si un nom court a été donné.
            if let Some(short_name) = short {
                let sug = format!(
                    "MATCH (n:Function) WHERE n.name CONTAINS '{short_name}' RETURN n.qualified_name, n.file_path LIMIT 8"
                );
                let rows: Vec<Value> = session
                    .execute(&sug)
                    .map(|r| {
                        r.rows
                            .iter()
                            .map(|rw| {
                                let qn2 = match rw.first() {
                                    Some(OV::String(s)) => s.to_string(),
                                    _ => String::new(),
                                };
                                let fp = match rw.get(1) {
                                    Some(OV::String(s)) => s.to_string(),
                                    _ => String::new(),
                                };
                                json!({ "qualified_name": qn2, "file_path": fp })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return (
                    json!({
                        "status": "ok",
                        "suggestions": rows,
                        "message": if rows.is_empty() {
                            "aucun symbole trouvé"
                        } else {
                            "plusieurs correspondances — précisez qualified_name"
                        },
                    }),
                    false,
                );
            }
            (
                json!({ "status": "error", "error": format!("get_code_snippet: symbole introuvable : {qn}") }),
                true,
            )
        }
        Err(e) => (
            json!({ "status": "error", "error": format!("get_code_snippet: {e}") }),
            true,
        ),
    }
}

/// `trace_path` — parité codebase-memory (mode `calls`) : BFS sur CALLS depuis
/// un symbole, par requêtes 1-saut chaînées (le moteur gère le 1-saut + WHERE
/// propriété ; les chemins multi-sauts d'un seul tenant ne sont pas utilisés).
/// Retourne des lignes `[from, type, to, hop]` (+ truncated). Les modes
/// data_flow / cross_service ne sont pas portés → erreur explicite.
fn trace_path(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session = db.session();
    let Some(qn0) = arg_str(arguments, "qualified_name")
        .or_else(|| arg_str(arguments, "function_name"))
        .or_else(|| arg_str(arguments, "name"))
    else {
        return (
            json!({ "status": "error", "error": "trace_path: qualified_name requis" }),
            true,
        );
    };
    if qn0.contains(['\'', '"', '\\']) {
        return (
            json!({ "status": "error", "error": "trace_path: qualified_name invalide" }),
            true,
        );
    }
    let direction = arg_str(arguments, "direction").unwrap_or_else(|| "both".to_string());
    if !matches!(direction.as_str(), "inbound" | "outbound" | "both") {
        return (
            json!({ "status": "error", "error": format!("trace_path: direction invalide : {direction}") }),
            true,
        );
    }
    let mode = arg_str(arguments, "mode").unwrap_or_else(|| "calls".to_string());
    if mode != "calls" {
        return (
            json!({ "status": "error", "error": format!("trace_path: mode '{mode}' non porté (calls seulement)") }),
            true,
        );
    }
    let depth = arg_usize(arguments, "depth", 3).clamp(1, 5);
    let limit = arg_usize(arguments, "limit", 50);
    let do_in = direction != "outbound";
    let do_out = direction != "inbound";

    let mut seen: std::collections::HashSet<String> = [qn0.clone()].into_iter().collect();
    let mut frontier: Vec<String> = vec![qn0.clone()];
    let mut rows: Vec<Value> = Vec::new();
    let mut truncated = false;

    'outer: for hop in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        let mut next: Vec<String> = Vec::new();
        for q in &frontier {
            let mut expand = |is_in: bool| {
                let qry = if is_in {
                    format!(
                        "MATCH (a)-[:CALLS]->(b) WHERE b.qualified_name = '{q}' RETURN DISTINCT a.qualified_name LIMIT {limit}"
                    )
                } else {
                    format!(
                        "MATCH (a)-[:CALLS]->(b) WHERE a.qualified_name = '{q}' RETURN DISTINCT b.qualified_name LIMIT {limit}"
                    )
                };
                if let Ok(res) = session.execute(&qry) {
                    for rw in res.rows {
                        if let Some(obrain_common::types::Value::String(t)) = rw.first() {
                            let t = t.to_string();
                            if !seen.contains(&t) {
                                seen.insert(t.clone());
                                if is_in {
                                    rows.push(json!([t, "CALLS", q, hop]));
                                } else {
                                    rows.push(json!([q, "CALLS", t, hop]));
                                }
                                next.push(t);
                            }
                        }
                    }
                }
            };
            if do_in {
                expand(true);
            }
            if do_out {
                expand(false);
            }
            if rows.len() >= limit {
                truncated = true;
                break 'outer;
            }
        }
        frontier = next;
    }

    (
        json!({
            "status": "ok",
            "tool": "trace_path",
            "mode": "calls",
            "direction": direction,
            "rows": rows,
            "total": rows.len(),
            "truncated": truncated,
            "hint": "Précisez direction/depth/limit pour borner ; data_flow/cross_service non portés",
        }),
        false,
    )
}

/// `structural_roles` — parité codebase-memory : profil structurel d'un nœud
/// sur le graphe CALLS (SCC dirigée + articulation du graphe non dirigé),
/// via les procédures moteur `obrain.strongly_connected_components` et
/// `obrain.articulation_points`.
fn structural_roles(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let Some(node_arg) = arg_str(arguments, "node")
        .or_else(|| arg_str(arguments, "qualified_name"))
        .or_else(|| arg_str(arguments, "name"))
    else {
        return (
            json!({ "status": "error", "error": "structural_roles: node requis (qualified_name ou id)" }),
            true,
        );
    };

    // 1) Résoudre le nœud : id numérique ou qualified_name → id.
    let node_id: u64 = if let Ok(n) = node_arg.parse::<u64>() {
        n
    } else {
        if node_arg.contains(['\'', '"', '\\']) {
            return (
                json!({ "status": "error", "error": "structural_roles: qualified_name invalide" }),
                true,
            );
        }
        let q = format!("MATCH (n) WHERE n.qualified_name = '{node_arg}' RETURN id(n) LIMIT 1");
        match session.execute(&q) {
            Ok(res) if !res.rows.is_empty() => match res.rows[0].first() {
                Some(OV::Int64(i)) => *i as u64,
                _ => {
                    return (
                        json!({ "status": "error", "error": format!("structural_roles: nœud introuvable : {node_arg}") }),
                        true,
                    );
                }
            },
            _ => {
                return (
                    json!({ "status": "error", "error": format!("structural_roles: nœud introuvable : {node_arg}") }),
                    true,
                );
            }
        }
    };

    // 2) SCC (composantes fortement connexes) : node_id → component_id.
    let mut comp_of: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    let mut comp_members: std::collections::HashMap<u64, Vec<u64>> =
        std::collections::HashMap::new();
    if let Ok(res) = session.execute(
        "CALL obrain.strongly_connected_components() YIELD node_id, component_id RETURN node_id, component_id",
    ) {
        for row in res.rows {
            if let (Some(OV::Int64(n)), Some(OV::Int64(c))) = (row.get(0), row.get(1)) {
                let (n, c) = (*n as u64, *c as u64);
                comp_of.insert(n, c);
                comp_members.entry(c).or_default().push(n);
            }
        }
    }

    // 3) Points d'articulation (graphe non dirigé des CALLS).
    let mut articulations: std::collections::HashSet<u64> = std::collections::HashSet::new();
    if let Ok(res) =
        session.execute("CALL obrain.articulation_points() YIELD node_id RETURN node_id")
    {
        for row in res.rows {
            if let Some(OV::Int64(n)) = row.first() {
                articulations.insert(*n as u64);
            }
        }
    }

    let comp_id = comp_of.get(&node_id).copied();
    let (scc_size, member_ids): (usize, Vec<u64>) = match comp_id {
        Some(cid) => {
            let members = comp_members.get(&cid).cloned().unwrap_or_default();
            (members.len(), members.iter().copied().take(500).collect())
        }
        None => (1, vec![node_id]),
    };
    let in_cycle = scc_size > 1;
    let is_articulation = articulations.contains(&node_id);

    // qn du nœud (retour informatif) quand on n'a fourni qu'un id.
    let qn = session
        .get_node(obrain_common::types::NodeId(node_id))
        .map(|nd| prop_str(&nd.properties, "qualified_name", ""))
        .unwrap_or_default();

    (
        json!({
            "status": "ok",
            "tool": "structural_roles",
            "node_id": node_id,
            "qualified_name": qn,
            "scc_id": comp_id,
            "scc_size": scc_size,
            "in_cycle": in_cycle,
            "is_articulation": is_articulation,
            "member_ids": member_ids,
        }),
        false,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// P0-v05 — outils bridge de restitution : `viz_subgraph`, `viz_export`,
// `node_curvature`. MÊME implémentation que la CLI `obrain viz`
// (crate::viz_extract + obrain_viz) → parité des comptes garantie.
// ─────────────────────────────────────────────────────────────────────────────

fn parse_overlay_arg(arguments: &Value) -> Option<obrain_viz::Overlay> {
    match arg_str(arguments, "overlay").as_deref() {
        Some("ricci") => Some(obrain_viz::Overlay::Ricci),
        Some("energy") => Some(obrain_viz::Overlay::Energy),
        Some("roles") => Some(obrain_viz::Overlay::Roles),
        Some("fsrs") => Some(obrain_viz::Overlay::Fsrs),
        Some("engrams") => Some(obrain_viz::Overlay::Engrams),
        Some("pheromone") => Some(obrain_viz::Overlay::Pheromone),
        Some("tier") => Some(obrain_viz::Overlay::Tier),
        _ => None,
    }
}

fn graph_doc_value(sg: &obrain_viz::Subgraph) -> Value {
    serde_json::from_str(&obrain_viz::to_json_string(sg).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or(Value::Null)
}

/// Payload selon `format` (json-graph v1 | dot | mermaid) + overlay éventuel.
/// Retourne (contenu, overlay_report).
fn viz_output(sg: &obrain_viz::Subgraph, arguments: &Value) -> (Value, Option<Value>) {
    let format = arg_str(arguments, "format").unwrap_or_else(|| "json".to_string());
    let overlay = parse_overlay_arg(arguments);
    let (styled, report) = match overlay {
        Some(o) => {
            let (s, r) = obrain_viz::apply_overlay(sg, o);
            (
                s,
                Some(json!({
                    "overlay": r.overlay,
                    "nodes_styled": r.nodes_styled,
                    "edges_styled": r.edges_styled,
                    "stale_edges": r.stale_edges,
                    "legend": r.legend,
                    "generated_at": if o.is_volatile() {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| Value::from(d.as_millis().to_string()))
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
                })),
            )
        }
        None => (sg.clone(), None),
    };
    let payload = match format.as_str() {
        "dot" => json!({
            "format": "dot",
            "text": obrain_viz::to_dot(&styled).unwrap_or_else(|e| format!("[export impossible: {e}]")),
        }),
        "mermaid" => json!({
            "format": "mermaid",
            "text": obrain_viz::to_mermaid(&styled).unwrap_or_else(|e| format!("[export impossible: {e}]")),
        }),
        "graphml" => json!({
            "format": "graphml",
            "text": obrain_viz::to_graphml(&styled).unwrap_or_else(|e| format!("[export impossible: {e}]")),
        }),
        _ => graph_doc_value(&styled),
    };
    (payload, report)
}

/// `viz_subgraph` — un appel = un sous-graphe rendable autour d'un nœud.
fn viz_subgraph(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(center) = arg_str(arguments, "center") else {
        return (
            json!({"status": "error", "error": "viz_subgraph: center requis (id numérique ou qualified_name)"}),
            true,
        );
    };
    let hops = arg_usize(arguments, "hops", 2).clamp(1, 3) as u8;
    let limit = arg_usize(arguments, "limit", 500);
    let edge_types: Vec<String> = arg_str(arguments, "edge_types")
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let only = |k: &str| arguments.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let opts = crate::viz_extract::ExtractOptions {
        hops,
        closure_max_depth: None, // tool bridge : closure illimitée (au caller de borner)
        limit_nodes: (limit > 0).then_some(limit),
        edge_types: (!edge_types.is_empty()).then_some(edge_types),
        outbound: !only("inbound_only"),
        inbound: !only("outbound_only"),
        signals: !only("no_signals"),
        auto_suffix: false, // tool bridge : reste strict comme la CLI
    };
    match crate::viz_extract::extract_subgraph(db, &center, &opts) {
        Ok((sg, rep)) => {
            let (payload, overlay_report) = viz_output(&sg, arguments);
            let mut out = json!({
                "status": "ok",
                "tool": "viz_subgraph",
                "center": center,
                "center_id": rep.center.0,
                "node_count": rep.nodes,
                "edge_count": rep.edges,
                "truncated": rep.truncated,
                "dropped": rep.dropped,
                "result": payload,
            });
            if let Some(qn) = rep.center_qn {
                out["center_qn"] = Value::String(qn);
            }
            if let Some(ovr) = overlay_report {
                out["overlay_report"] = ovr;
            }
            (out, false)
        }
        Err(e) => (
            json!({"status": "error", "error": format!("viz_subgraph: {e:#}")}),
            true,
        ),
    }
}

/// `viz_export` — export du store entier en un artefact.
fn viz_export(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let edge_types: Vec<String> = arg_str(arguments, "edge_types")
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let only = |k: &str| arguments.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let opts = crate::viz_extract::ExtractOptions {
        edge_types: (!edge_types.is_empty()).then_some(edge_types),
        signals: !only("no_signals"),
        ..Default::default()
    };
    match crate::viz_extract::export_all(db, &opts) {
        Ok((sg, rep)) => {
            let (payload, overlay_report) = viz_output(&sg, arguments);
            let mut out = json!({
                "status": "ok",
                "tool": "viz_export",
                "node_count": rep.nodes,
                "edge_count": rep.edges,
                "result": payload,
            });
            if let Some(ovr) = overlay_report {
                out["overlay_report"] = ovr;
            }
            (out, false)
        }
        Err(e) => (
            json!({"status": "error", "error": format!("viz_export: {e:#}")}),
            true,
        ),
    }
}

/// `node_curvature` — lecture bridge : courbure nodale agrégée + energy.
fn node_curvature_tool(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::NodeId;
    let Some(node_arg) =
        arg_str(arguments, "node").or_else(|| arg_str(arguments, "qualified_name"))
    else {
        return (
            json!({"status": "error", "error": "node_curvature: node requis (id ou qualified_name)"}),
            true,
        );
    };
    let Ok(id) = crate::viz_extract::resolve_node(db, &node_arg) else {
        return (
            json!({"status": "error", "error": format!("node_curvature: nœud introuvable : {node_arg}")}),
            true,
        );
    };
    let Some(store) = db.substrate_handle() else {
        return (
            json!({"status": "error", "error": "node_curvature: pas de SubstrateStore (base non-substrat)"}),
            true,
        );
    };
    let mut out = json!({
        "status": "ok",
        "tool": "node_curvature",
        "node_id": id.0,
    });
    if let Some(nd) = db.session().get_node(NodeId(id.0)) {
        out["qualified_name"] = Value::String(prop_str(&nd.properties, "qualified_name", ""));
    }
    if let Ok(id32) = u32::try_from(id.0)
        && let Ok(Some(rec)) = store.writer().read_node(id32)
        && !rec.is_tombstoned()
    {
        out["energy"] = json!(f64::from(rec.energy_q15()));
    }
    match obrain_substrate::node_curvature(&store, NodeId(id.0)) {
        Some(k) => out["curvature"] = json!(f64::from(k)),
        None => out["curvature"] = Value::Null,
    }
    (out, false)
}

/// `viz_topo` — lecture topologique (triangles CALLS non dirigés) calculée en
/// Rust sur le sous-graphe extrait (`viz_extract`) — pas de table simpliciale
/// à lire côté substrate (voir note « cliques_of / hyperedges_of » plus haut),
/// le calcul local est la lecture honnête disponible sans nouvelle primitive.
fn viz_topo(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(center) = arg_str(arguments, "center") else {
        return (
            json!({"status": "error", "error": "viz_topo: center requis (id ou qualified_name)"}),
            true,
        );
    };
    let hops = arg_usize(arguments, "hops", 2).clamp(1, 3) as u8;
    let limit = arg_usize(arguments, "limit", 200);
    let opts = crate::viz_extract::ExtractOptions {
        hops,
        limit_nodes: (limit > 0).then_some(limit),
        edge_types: Some(vec!["CALLS".to_string()]),
        signals: false,
        ..Default::default()
    };
    let (sg, rep) = match crate::viz_extract::extract_subgraph(db, &center, &opts) {
        Ok(ok) => ok,
        Err(e) => {
            return (
                json!({"status": "error", "error": format!("viz_topo: {e:#}")}),
                true,
            );
        }
    };
    // Adjacence non dirigée déduite des arêtes CALLS extraites.
    let mut adj: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for e in &sg.edges {
        adj.entry(e.src.clone()).or_default().insert(e.dst.clone());
        adj.entry(e.dst.clone()).or_default().insert(e.src.clone());
    }
    // Triangles non dirigés par nœud : u-v, u-w, v-w.
    let mut triangles: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for (u, ns) in &adj {
        let ns: Vec<&String> = ns.iter().collect();
        let mut count = 0u32;
        for (i, vi) in ns.iter().enumerate() {
            for vj in ns.iter().skip(i + 1) {
                if adj.get(*vi).is_some_and(|s| s.contains(*vj)) {
                    count += 1;
                }
            }
        }
        if count > 0 {
            triangles.insert(u.clone(), count);
        }
    }
    let per_node_total: u32 = triangles.values().sum();
    // Top 20 nœuds par nombre de triangles (les plus « reliés »).
    let mut ranked: Vec<(String, u32)> = triangles.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(20);
    let top: Value = ranked
        .into_iter()
        .map(|(id, n)| json!({"node_id": id, "triangles": n}))
        .collect();
    let qn = rep.center_qn.unwrap_or_default();
    (
        json!({
            "status": "ok",
            "tool": "viz_topo",
            "center": center,
            "center_qn": qn,
            "hops": hops,
            "subgraph_nodes": rep.nodes,
            "subgraph_edges": rep.edges,
            "undirected_triangle_incidences": per_node_total, // chaque triangle compté 3×
            "top_nodes": top,
        }),
        false,
    )
}

/// `list_projects` — parité codebase-memory : liste les projets (nœuds
/// `Project`) présents dans le store substrate courant. Sur un store persisté
/// par obrain-indexer, chaque dépôt indexé = un nœud Project (qualified_name,
/// name). Les stores runtime (dsh-runtime…) peuvent en porter plusieurs.
fn list_projects(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let limit = arg_usize(arguments, "limit", 1000);
    let mut projects: Vec<Value> = Vec::new();
    if let Ok(res) = session.execute("MATCH (n:Project) RETURN n.qualified_name, n.name LIMIT 1000")
    {
        for row in res.rows.iter().take(limit) {
            let qn = match row.first() {
                Some(OV::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let name = match row.get(1) {
                Some(OV::String(s)) => s.to_string(),
                _ => qn.clone(),
            };
            projects.push(json!({ "name": name, "qualified_name": qn }));
        }
    }
    (
        json!({
            "status": "ok",
            "tool": "list_projects",
            "projects": projects,
            "total": projects.len(),
        }),
        false,
    )
}

/// `search_code` — parité codebase-memory (recherche de code enrichie) :
/// BM25 sur Function.text (docstring/signature persistés) + Function.name,
/// fusionné et borné par limit. Rows = tuples [qn, label, file, lines, rank]
/// (contrat tuple du canal, cf. search_graph) + file_path/start_line/end_line.
fn search_code(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session = db.session();
    let Some(pattern) = arg_str(arguments, "pattern").or_else(|| arg_str(arguments, "query"))
    else {
        return (
            json!({ "status": "error", "error": "search_code: pattern requis" }),
            true,
        );
    };
    let limit = arg_usize(arguments, "limit", 20);
    let file_pat = arg_str(arguments, "file_pattern").unwrap_or_default();
    let _ = db.create_text_index("Function", "text");
    let _ = db.create_text_index("Function", "name");

    let mut hits: Vec<(obrain_common::types::NodeId, f64)> = Vec::new();
    if let Ok(h) = db.text_search("Function", "text", &pattern, limit * 2) {
        hits.extend(h);
    }
    if let Ok(h) = db.text_search("Function", "name", &pattern, limit * 2) {
        hits.extend(h);
    }
    hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut rows: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for (nid, score) in hits {
        if seen.contains(&nid.0) {
            continue;
        }
        seen.insert(nid.0);
        let props = session
            .get_node(nid)
            .map(|n| n.properties)
            .unwrap_or_default();
        let qn = prop_str(&props, "qualified_name", "");
        if !file_pat.is_empty() && !prop_str(&props, "file_path", "").contains(&file_pat) {
            continue;
        }
        let file = prop_str(&props, "file_path", "");
        let line_of = |key: &str| -> String {
            props
                .iter()
                .find(|(k, _)| k.as_str() == key)
                .and_then(|(_, v)| match v {
                    obrain_common::types::Value::Int64(i) => Some(i.to_string()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        let sl = line_of("start_line");
        let el = line_of("end_line");
        rows.push(json!([qn, "Function", file, sl, score]));
        if rows.len() >= limit {
            break;
        }
    }
    (
        json!({
            "status": "ok",
            "tool": "search_code",
            "cols": ["qn", "label", "file", "lines", "rank"],
            "rows": rows,
            "results": rows,
            "total": rows.len(),
        }),
        false,
    )
}

/// `index_status` — parité codebase-memory : compteurs du store substrate
/// (nœuds/arêtes + par label). Les infos miss-graph (fichiers partiellement
/// indexés, ranges) ne sont pas persistées par obrain-indexer → notées absentes.
fn index_status(db: &ObrainDB, _arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let count_of = |q: &str| -> i64 {
        match session.execute(q) {
            Ok(res) => match res.rows.first().and_then(|r| r.first()) {
                Some(OV::Int64(i)) => *i,
                _ => -1,
            },
            Err(_) => -1,
        }
    };
    let total_nodes = count_of("MATCH (n) RETURN count(n)");
    let total_edges = count_of("MATCH ()-[r]->() RETURN count(r)");

    // Labels du store puis comptage par label.
    let mut labels: Vec<String> = Vec::new();
    if let Ok(res) = session.execute("MATCH (n) RETURN DISTINCT labels(n) LIMIT 500") {
        for row in res.rows {
            if let Some(OV::List(items)) = row.first() {
                for it in items.iter() {
                    if let OV::String(s) = it {
                        let s = s.to_string();
                        if !labels.contains(&s) {
                            labels.push(s);
                        }
                    }
                }
            }
        }
    }
    let mut by_label: Vec<Value> = Vec::new();
    for l in &labels {
        if l.chars().all(|c| c.is_alphanumeric() || c == '_') {
            let q = format!("MATCH (n:{l}) RETURN count(n)");
            by_label.push(json!({ "label": l, "count": count_of(&q) }));
        }
    }
    by_label.sort_by(|a, b| {
        b["count"]
            .as_i64()
            .cmp(&a["count"].as_i64())
            .then(a["label"].as_str().cmp(&b["label"].as_str()))
    });

    (
        json!({
            "status": "ok",
            "tool": "index_status",
            "total_nodes": total_nodes,
            "total_edges": total_edges,
            "labels": by_label,
            "note": "miss-graph/parse_partial non persistés (obrain-indexer) — coverage à venir",
        }),
        false,
    )
}

/// `graph_link` — crée un lien entre deux qn (upsert des nœuds + arête).
/// Nécessite un store ouvert en écriture (env OBRAIN_WRITE=1).
fn graph_link(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let edge_type = match arg_str(arguments, "type") {
        Some(t) if !t.is_empty() => t,
        _ => {
            return (
                json!({ "status": "error", "error": "graph_link: type requis" }),
                true,
            );
        }
    };

    let upsert =
        |ep: &Value, label_default: &str| -> Result<obrain_common::types::NodeId, String> {
            // ── DEUX CONVENTIONS D'IDENTITÉ, UNE SEULE ÉTAIT LUE (2026-09-17) ──
            // Le dépôt nomme ses nœuds de deux façons, et c'est MESURÉ :
            // `qn` du côté de la mémoire et du registre des machines
            // (`memoire.rs`, `remote.rs`), `qualified_name` du côté du graphe de
            // code et de cette surface MCP. Cette closure ne LISAIT que `qn`
            // alors que sa propre description annonce « identifiés par
            // qualified_name » : un appelant qui suit la description reçoit
            // « qn manquant », et le nœud qu'on écrit porte `qualified_name` seul
            // — donc INVISIBLE au lecteur qui cherche `qn`.
            //
            // `machines.rs` écrit les deux propriétés pour cette raison exacte
            // (« une machine écrite avec une seule des deux serait INVISIBLE à
            // l'autre lecteur »). On applique ici la même règle : on ACCEPTE les
            // deux clés en entrée, on CHERCHE par l'une ou l'autre, et on ÉCRIT
            // les deux. Trancher l'incohérence du dépôt n'est pas le sujet ; ne
            // pas la propager à chaque nœud en est un.
            let qn = ep
                .get("qn")
                .or_else(|| ep.get("qualified_name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "qn (ou qualified_name) manquant dans source/target".to_string())?;
            if qn.contains(['\'', '"', '\\']) {
                return Err("qualified_name invalide".to_string());
            }
            // Deux recherches indexées, PAS un `OR` : mesuré, un `OR` entre deux
            // égalités indexées fait balayer le label (8 slots sur 8 nœuds) là
            // où chacune prise seule en lit 0.
            let mut existing = None;
            for cle in ["qn", "qualified_name"] {
                let q = format!("MATCH (n) WHERE n.{cle} = '{qn}' RETURN id(n) LIMIT 1");
                if let Ok(res) = session.execute(&q)
                    && let Some(OV::Int64(i)) = res.rows.first().and_then(|r| r.first())
                {
                    existing = Some(obrain_common::types::NodeId(*i as u64));
                    break;
                }
            }
            let nid = match existing {
                Some(n) => n,
                None => {
                    let label = ep
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(label_default);
                    let nid = db.create_node(&[label]);
                    // Les DEUX, comme `machines.rs` : un nœud écrit avec une
                    // seule des deux conventions est invisible à l'autre lecteur.
                    for cle in ["qn", "qualified_name"] {
                        let _ = db.set_node_property(
                            nid,
                            cle,
                            obrain_common::types::Value::from(qn.to_string()),
                        );
                    }
                    if let Some(nm) = ep.get("name").and_then(|v| v.as_str()) {
                        let _ = db.set_node_property(
                            nid,
                            "name",
                            obrain_common::types::Value::from(nm.to_string()),
                        );
                    }
                    nid
                }
            };
            // ── COMPLÉTER SANS JAMAIS ÉCRASER ────────────────────────────────
            // Un nœud écrit par une version antérieure ne porte qu'une des deux
            // conventions. On ajoute celle qui MANQUE, et on ne touche pas à celle
            // qui est là : faire converger deux conventions ne doit pas réécrire
            // une identité. (Un nœud créé juste au-dessus les porte déjà toutes
            // les deux ; la lecture ne coûte alors qu'un aller-retour.)
            let q_props = format!(
                "MATCH (n) WHERE id(n) = {} RETURN n.qn, n.qualified_name",
                nid.0
            );
            if let Ok(res) = session.execute(&q_props)
                && let Some(row) = res.rows.first()
            {
                for (i, cle) in ["qn", "qualified_name"].iter().enumerate() {
                    let present = matches!(row.get(i), Some(v) if !matches!(v, OV::Null));
                    if !present {
                        let _ = db.set_node_property(
                            nid,
                            cle,
                            obrain_common::types::Value::from(qn.to_string()),
                        );
                    }
                }
            }
            if let Some(props) = ep.get("properties").and_then(|v| v.as_object()) {
                for (k, val) in props {
                    let _ = db.set_node_property(nid, k, json_to_obrain_value_deep(val));
                }
            }
            Ok(nid)
        };

    let src = match arguments.get("source").and_then(|v| v.as_object()) {
        Some(_) => match upsert(&arguments["source"], "Node") {
            Ok(n) => n,
            Err(e) => {
                return (
                    json!({ "status": "error", "error": format!("graph_link: source {e}") }),
                    true,
                );
            }
        },
        None => {
            return (
                json!({ "status": "error", "error": "graph_link: source requis" }),
                true,
            );
        }
    };
    let dst = match arguments.get("target").and_then(|v| v.as_object()) {
        Some(_) => match upsert(&arguments["target"], "Node") {
            Ok(n) => n,
            Err(e) => {
                return (
                    json!({ "status": "error", "error": format!("graph_link: target {e}") }),
                    true,
                );
            }
        },
        None => {
            return (
                json!({ "status": "error", "error": "graph_link: target requis" }),
                true,
            );
        }
    };

    let _eid = db.create_edge(src, dst, &edge_type);
    (
        json!({
            "status": "ok",
            "tool": "graph_link",
            "source": src.0,
            "target": dst.0,
            "type": edge_type,
            "note": "propriétés d'arête non persistées (API nœud uniquement pour l'instant)",
        }),
        false,
    )
}

/// Résout un nœud par son identité, **quelle que soit la convention qui l'a écrit**.
///
/// Le dépôt porte deux conventions d'identité, et c'est mesuré : `qn` du côté de
/// la mémoire et du registre des machines (`memoire.rs`, `remote.rs`),
/// `qualified_name` du côté du graphe de code et des indexeurs. `machines.rs`
/// écrit les DEUX pour qu'aucun lecteur ne perde un nœud (« une machine écrite
/// avec une seule des deux serait INVISIBLE à l'autre lecteur »).
///
/// Une surface d'INTERROGATION ne peut pas exiger de son appelant qu'il devine
/// laquelle a servi : celui-ci n'a qu'un nom, pas une convention. On cherche donc
/// par l'une OU l'autre — c'est le seul comportement qui ne perde pas de nœud.
fn resoudre_noeud_par_identite(
    session: &obrain_engine::Session,
    identifiant: &str,
) -> Option<obrain_common::types::NodeId> {
    // ── DEUX RECHERCHES INDEXÉES, PAS UN `OR` ───────────────────────────────
    // Un seul prédicat `WHERE n.qualified_name = 'x' OR n.qn = 'x'` a été mesuré à
    // **8 slots lus** sur une fixture de 8 nœuds (tout le label), là où chacune
    // des deux égalités prises séparément lit **0 slot** : le planificateur ne
    // peut plus ancrer sur un prédicat indexé quand un `OR` les unit. Même
    // couverture, deux index — on fait donc les deux recherches, l'une après
    // l'autre.
    for cle in ["qualified_name", "qn"] {
        let q = format!("MATCH (n) WHERE n.{cle} = '{identifiant}' RETURN id(n) LIMIT 1");
        let Ok(res) = session.execute(&q) else {
            continue;
        };
        if let Some(obrain_common::types::Value::Int64(i)) =
            res.rows.first().and_then(|r| r.first())
        {
            return Some(obrain_common::types::NodeId(*i as u64));
        }
    }
    None
}

/// `payload_get` — propriétés (payload) d'un nœud par qualified_name.
fn payload_get(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let Some(qn) = arg_str(arguments, "qn").or_else(|| arg_str(arguments, "qualified_name")) else {
        return (
            json!({ "status": "error", "error": "payload_get: qn requis" }),
            true,
        );
    };
    if qn.contains(['\'', '"', '\\']) {
        return (
            json!({ "status": "error", "error": "payload_get: qn invalide" }),
            true,
        );
    }
    // Par `qualified_name` **ou** `qn` : l'entrée de cet outil s'appelle `qn`,
    // et sa réponse la nomme `qualified_name` — exiger une seule des deux
    // conventions rendait invisibles les nœuds écrits par la mémoire et le
    // registre des machines, qui n'ont que `qn` (mesuré le 2026-09-17).
    let Some(nid) = resoudre_noeud_par_identite(&session, &qn) else {
        return (
            json!({ "status": "error", "error": format!("payload_get: nœud introuvable : {qn}") }),
            true,
        );
    };
    match session.get_node(nid) {
        Some(nd) => {
            let mut props = serde_json::Map::new();
            props.insert("qualified_name".into(), Value::String(qn.clone()));
            for (k, v) in &nd.properties {
                props.insert(k.as_str().to_string(), value_to_json(v));
            }
            (
                json!({ "status": "ok", "tool": "payload_get", "node_id": nid.0, "qualified_name": qn, "payload": props }),
                false,
            )
        }
        None => (
            json!({ "status": "error", "error": "payload_get: nœud inaccessible" }),
            true,
        ),
    }
}

/// `list_docs` — liste les nœuds Skill/Plan du store.
fn list_docs(db: &ObrainDB, _arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let mut docs: Vec<Value> = Vec::new();
    for label in ["Skill", "Plan"] {
        let q = format!("MATCH (n:{label}) RETURN n.qualified_name, n.name LIMIT 500");
        if let Ok(res) = session.execute(&q) {
            for row in res.rows {
                let qn = match row.first() {
                    Some(OV::String(s)) => s.to_string(),
                    _ => String::new(),
                };
                let name = match row.get(1) {
                    Some(OV::String(s)) => s.to_string(),
                    _ => qn.clone(),
                };
                docs.push(json!({ "label": label, "name": name, "qualified_name": qn }));
            }
        }
    }
    (
        json!({ "status": "ok", "tool": "list_docs", "docs": docs, "total": docs.len() }),
        false,
    )
}

/// `ingest_traces` — upsert caller/callee (label Function) + arête CALLS par trace.
fn ingest_traces(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    let session = db.session();
    let traces = arguments
        .get("traces")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if traces.is_empty() {
        return (
            json!({ "status": "error", "error": "ingest_traces: traces requis" }),
            true,
        );
    }
    let upsert = |qn: &str| -> Result<obrain_common::types::NodeId, String> {
        if qn.contains(['\'', '"', '\\']) {
            return Err("qn invalide".into());
        }
        let q = format!("MATCH (n) WHERE n.qualified_name = '{qn}' RETURN id(n) LIMIT 1");
        if let Ok(res) = session.execute(&q) {
            if let Some(OV::Int64(i)) = res.rows.first().and_then(|r| r.first()) {
                return Ok(obrain_common::types::NodeId(*i as u64));
            }
        }
        let nid = db.create_node(&["Function"]);
        // Les DEUX conventions, comme le prescrit `machines.rs` : un nœud écrit
        // avec une seule est invisible à l'autre lecteur.
        let _ = db.set_node_property(nid, "qn", obrain_common::types::Value::from(qn.to_string()));
        let _ = db.set_node_property(
            nid,
            "qualified_name",
            obrain_common::types::Value::from(qn.to_string()),
        );
        let _ = db.set_node_property(
            nid,
            "name",
            obrain_common::types::Value::from(qn.to_string()),
        );
        Ok(nid)
    };
    let mut created = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for tr in &traces {
        let (Some(caller), Some(callee)) = (
            tr.get("caller").and_then(|v| v.as_str()),
            tr.get("callee").and_then(|v| v.as_str()),
        ) else {
            errors.push("trace sans caller/callee".into());
            continue;
        };
        match (upsert(caller), upsert(callee)) {
            (Ok(a), Ok(b)) => {
                let _e = db.create_edge(a, b, "CALLS");
                created += 1;
            }
            (Err(e), _) | (_, Err(e)) => errors.push(e),
        }
    }
    (
        json!({
            "status": "ok",
            "tool": "ingest_traces",
            "edges": created,
            "errors": errors,
        }),
        false,
    )
}

/// `impact_set` — BFS multi-sources sur CALLS (parité impact_set) : mêmes
/// mécaniques que `trace_path` mais à partir d'une liste de seeds.
fn impact_set(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session = db.session();
    let mut seeds: Vec<String> = Vec::new();
    if let Some(arr) = arguments.get("seeds").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() {
                seeds.push(s.to_string());
            }
        }
    }
    if let Some(node) = arg_str(arguments, "node").or_else(|| arg_str(arguments, "qualified_name"))
    {
        seeds.push(node);
    }
    seeds.retain(|s| !s.is_empty() && !s.contains(['\'', '"', '\\']));
    if seeds.is_empty() {
        return (
            json!({ "status": "error", "error": "impact_set: node ou seeds requis" }),
            true,
        );
    }
    let direction = arg_str(arguments, "direction").unwrap_or_else(|| "inbound".to_string());
    if !matches!(direction.as_str(), "inbound" | "outbound" | "both") {
        return (
            json!({ "status": "error", "error": format!("impact_set: direction invalide : {direction}") }),
            true,
        );
    }
    let depth = arg_usize(arguments, "depth", 2).clamp(1, 5);
    let limit = arg_usize(arguments, "limit", 200);
    let do_in = direction != "outbound";
    let do_out = direction != "inbound";

    let mut seen: std::collections::HashSet<String> = seeds.iter().cloned().collect();
    let mut frontier: Vec<String> = seeds;
    let mut rows: Vec<Value> = Vec::new();
    let mut truncated = false;

    'outer: for hop in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        let mut next: Vec<String> = Vec::new();
        for q in &frontier {
            let mut expand = |is_in: bool| {
                let qry = if is_in {
                    format!(
                        "MATCH (a)-[:CALLS]->(b) WHERE b.qualified_name = '{q}' RETURN DISTINCT a.qualified_name LIMIT {limit}"
                    )
                } else {
                    format!(
                        "MATCH (a)-[:CALLS]->(b) WHERE a.qualified_name = '{q}' RETURN DISTINCT b.qualified_name LIMIT {limit}"
                    )
                };
                if let Ok(res) = session.execute(&qry) {
                    for rw in res.rows {
                        if let Some(obrain_common::types::Value::String(t)) = rw.first() {
                            let t = t.to_string();
                            if !seen.contains(&t) {
                                seen.insert(t.clone());
                                if is_in {
                                    rows.push(json!([t, "CALLS", q, hop]));
                                } else {
                                    rows.push(json!([q, "CALLS", t, hop]));
                                }
                                next.push(t);
                            }
                        }
                    }
                }
            };
            if do_in {
                expand(true);
            }
            if do_out {
                expand(false);
            }
            if rows.len() >= limit {
                truncated = true;
                break 'outer;
            }
        }
        frontier = next;
    }
    (
        json!({
            "status": "ok",
            "tool": "impact_set",
            "direction": direction,
            "rows": rows,
            "total": rows.len(),
            "truncated": truncated,
        }),
        false,
    )
}

#[cfg(not(feature = "indexer"))]
fn index_repository(_db: &ObrainDB, _arguments: &Value) -> (Value, bool) {
    (
        json!({
            "status": "error",
            "error": "index_repository: binaire compilé sans feature indexer — rebuild `--features indexer`",
        }),
        true,
    )
}

/// `index_repository` (feature `indexer`) — indexe un dépôt dans le store
/// courant (mêmes écritures que `examples/persist.rs`, sans ouvrir un autre
/// store). Requiert un store en écriture.
#[cfg(feature = "indexer")]
/// Nœuds, arêtes et propriétés d'un index de code, par les zones (base neuve).
/// Rend `(arêtes écrites, paires de provenance écrites)`, ou la raison du refus.
fn ingestion_native(
    db: &ObrainDB,
    g: &obrain_indexer::graph::CodeGraph,
    empreinte: &str,
) -> Result<(u64, u64), String> {
    use obrain_common::types::Value as OV;
    use std::collections::HashMap;
    let prof = std::env::var("OBRAIN_INDEX_PROF").is_ok();
    // ── RÉUNIFICATION (2026-10-06) ──────────────────────────────────────────
    // Le multi-écrivain construisait ici `IndexOptions` pour honorer l'argument
    // `exclure_artefacts`. La lignée servie a EXTRAIT l'indexation de cette
    // fonction : `ingestion_native` ne reçoit plus que le graphe DÉJÀ bâti
    // (`g`) et son empreinte, donc `arguments` et `path` ne sont plus en portée
    // ici. L'option est donc remontée TELLE QUELLE dans `index_repository`,
    // juste là où `options` se construit — même comportement, au bon endroit.
    let t = std::time::Instant::now();
    // Slot = rang du nœud + 1, dans l'ordre même où le chemin unitaire les crée.
    let mut slot_de: HashMap<u64, u32> = HashMap::with_capacity(g.node_count());
    let mut labels: Vec<&str> = Vec::with_capacity(g.node_count());
    for (i, n) in g.nodes().iter().enumerate() {
        slot_de.insert(n.id.0, i as u32 + 1);
        labels.push(n.kind.label());
    }
    let (mut src, mut dst, mut types) = (Vec::new(), Vec::new(), Vec::new());
    for e in g.edges() {
        let (Some(&s), Some(&d)) = (slot_de.get(&e.source.0), slot_de.get(&e.target.0)) else {
            continue;
        };
        src.push(s);
        dst.push(d);
        types.push(e.kind.label());
    }
    let n_edges = src.len() as u64;
    db.bulk_import_refs(&labels, &src, &dst, &types)
        .map_err(|e| format!("{e}"))?;
    if prof {
        eprintln!(
            "[prof]   nœuds+arêtes (zones) : {:.2}s",
            t.elapsed().as_secs_f64()
        );
    }
    let t = std::time::Instant::now();
    // Propriétés, par paquets (la mémoire reste bornée), avec EXACTEMENT les
    // clés, valeurs et l'ordre du chemin unitaire.
    const PAQUET: usize = 100_000;
    let noeuds: &[obrain_indexer::graph::Node] = g.nodes();
    let mut ecrites = 0u64;
    let (mut t_prep, mut t_ecr) = (0f64, 0f64);
    let cpu_debut = obrain_substrate::compteurs::temps_cpu("thread-self");
    for (p, bloc) in noeuds.chunks(PAQUET).enumerate() {
        let tp = std::time::Instant::now();
        // (T27) Préparation des valeurs en parallèle : l'ordre des nœuds est
        // conservé par `collect`, et chaque liste garde l'ordre du chemin
        // unitaire.
        use rayon::prelude::*;
        let lot: Vec<Vec<(&str, OV)>> = bloc
            .par_iter()
            .map(|n| {
                let mut l: Vec<(&str, OV)> = Vec::with_capacity(10);
                l.push((
                    "qualified_name",
                    OV::String(n.qualified_name.clone().into()),
                ));
                if let Some(name) = &n.props.name {
                    l.push(("name", OV::String(name.clone().into())));
                }
                if let Some(fp) = &n.props.file_path {
                    l.push(("file_path", OV::String(fp.clone().into())));
                }
                if let (Some(a), Some(b)) = (n.props.start_line, n.props.end_line) {
                    l.push(("start_line", OV::Int64(a as i64)));
                    l.push(("end_line", OV::Int64(b as i64)));
                }
                let texte = ["docstring", "signature"]
                    .iter()
                    .find_map(|k| n.props.extra.get(*k).and_then(|v| v.as_str()));
                if let Some(t) = texte {
                    l.push(("text", OV::String(t.to_string().into())));
                }
                for (k, v) in &n.props.extra {
                    let val = match v {
                        serde_json::Value::String(s) => OV::String(s.clone().into()),
                        serde_json::Value::Bool(b) => OV::Bool(*b),
                        serde_json::Value::Number(x) => x
                            .as_i64()
                            .map(OV::Int64)
                            .unwrap_or_else(|| OV::Float64(x.as_f64().unwrap_or(0.0))),
                        _ => continue,
                    };
                    l.push((k.as_str(), val));
                }
                if n.kind == obrain_indexer::NodeKind::Project {
                    l.push(("empreinte_depot", OV::String(empreinte.to_string().into())));
                }
                l
            })
            .collect();
        t_prep += tp.elapsed().as_secs_f64();
        let te = std::time::Instant::now();
        ecrites += db
            .bulk_import_props((p * PAQUET) as u32 + 1, &lot)
            .map_err(|e| format!("{e}"))?;
        t_ecr += te.elapsed().as_secs_f64();
    }
    if prof {
        eprintln!(
            "[prof]     préparation des valeurs {t_prep:.2}s · écriture en zone {t_ecr:.2}s · CPU fil (les deux) {:.2}s",
            obrain_substrate::compteurs::temps_cpu("thread-self") - cpu_debut
        );
    }
    if prof {
        eprintln!(
            "[prof]   propriétés (zones) : {:.2}s ({ecrites})",
            t.elapsed().as_secs_f64()
        );
    }
    // ── POURQUOI 0, ET PAS UN MENSONGE (réunification 2026-10-06) ───────────
    // Le multi-écrivain a corrigé la PERTE de provenance d'arêtes en passant par
    // `ecrire_lot_aretes` (voir `ingestion_unitaire`). Le chemin NATIF ci-dessus
    // écrit nœuds et arêtes par les zones (`bulk_import_refs`) et les propriétés
    // de NŒUDS (`bulk_import_props`) — il ne transporte AUCUNE provenance
    // d'arête. Le compte rendu est donc 0, et c'est la vérité sur ce chemin.
    // ⚠️ ÉCART CONNU : `strategy` / `confidence` / `caller_scope` ne sont pas
    // posés sur une base NEUVE (chemin natif), alors qu'ils le sont sur une base
    // déjà peuplée (chemin unitaire). Signalé, non corrigé ici : le porter
    // demanderait d'étendre l'API d'import par zones.
    Ok((n_edges, 0))
}

/// Le chemin historique, propriété par propriété (base déjà peuplée).
/// Rend `(arêtes écrites, paires de provenance écrites)`.
fn ingestion_unitaire(
    db: &ObrainDB,
    g: &obrain_indexer::graph::CodeGraph,
    empreinte: &str,
) -> (u64, u64) {
    use obrain_common::types::Value as OV;
    use std::collections::HashMap;
    let mut map: HashMap<u64, obrain_common::types::NodeId> = HashMap::new();
    let mut n_edges = 0u64;
    for n in g.nodes() {
        // NoWal : import de gros volume — durabilité par checkpoint() final.
        let nid = db.create_node_no_wal(&[n.kind.label()]);
        map.insert(n.id.0, nid);
        let _ = db.set_node_property(
            nid,
            "qualified_name",
            OV::String(n.qualified_name.clone().into()),
        );
        if let Some(name) = &n.props.name {
            let _ = db.set_node_property(nid, "name", OV::String(name.clone().into()));
        }
        if let Some(fp) = &n.props.file_path {
            let _ = db.set_node_property(nid, "file_path", OV::String(fp.clone().into()));
        }
        if let (Some(a), Some(b)) = (n.props.start_line, n.props.end_line) {
            let _ = db.set_node_property(nid, "start_line", OV::Int64(a as i64));
            let _ = db.set_node_property(nid, "end_line", OV::Int64(b as i64));
        }
        let mut txt: Option<String> = None;
        for key in ["docstring", "signature"] {
            if let Some(v) = n.props.extra.get(key) {
                if let Some(s) = v.as_str() {
                    txt = Some(s.to_string());
                    break;
                }
            }
        }
        if let Some(t) = txt {
            let _ = db.set_node_property(nid, "text", OV::String(t.into()));
        }
        for (k, v) in &n.props.extra {
            let val = match v {
                serde_json::Value::String(s) => OV::String(s.clone().into()),
                serde_json::Value::Bool(b) => OV::Bool(*b),
                serde_json::Value::Number(x) => x
                    .as_i64()
                    .map(OV::Int64)
                    .unwrap_or_else(|| OV::Float64(x.as_f64().unwrap_or(0.0))),
                _ => continue,
            };
            let _ = db.set_node_property(nid, k, val);
        }
    }
    // Arêtes en BULK (NoWal, paquets de 65 536) au lieu d'un create_edge par
    // arête — le facteur ×N du WAL individuel sur 1,5M nœuds / 4,8M arêtes.
    use obrain_common::types::NodeId as ONid;
    let mut batch: Vec<(ONid, ONid, &str)> = Vec::with_capacity(65_536);
    // ── LA PROVENANCE DE CHAQUE ARÊTE, DANS L'ORDRE DU LOT ──────────────────
    //
    // (2026-09-14) DÉFAUT MESURÉ, ET IL ÉTAIT INVISIBLE. L'indexeur calcule une
    // provenance par arête `CALLS` — `strategy` (import_map 0,95 … fuzzy 0,30),
    // `confidence`, `caller_scope` (obrain-indexer/src/resolve.rs, index.rs:587)
    // — et un test EXIGE qu'elle soit posée. Mais ce chemin d'import ne
    // transmettait à `create_edges_batch_no_wal` que `(source, cible, label)`,
    // et **jetait le `Vec<EdgeId>` rendu**. La provenance était donc calculée,
    // portée en mémoire, puis abandonnée à la frontière d'écriture.
    //
    // Conséquence constatée sur le store servi : `r.strategy`, `r.confidence`
    // et `r.caller_scope` rendent tous NULL sur les 77 938 arêtes CALLS d'un
    // monde, et `substrate.edge_props` est absent de tous les stores ouverts.
    // Or c'est exactement la donnée qui permettrait de trier un appel résolu par
    // `import_map` d'une collision de nom à `fuzzy` — mesuré : 23 676 appelants
    // sur `MultiDrawRenderList.push`, une méthode de three.js vendorisé, dont
    // des bundles `@tanstack/router` qui ne peuvent pas l'appeler.
    //
    // `create_edges_batch_no_wal` rend ses identifiants **dans l'ordre d'entrée**
    // (crud.rs:814-822) : la provenance se réapparie donc sans structure
    // supplémentaire, index par index.
    let mut provenance: Vec<&obrain_indexer::graph::EdgeProps> = Vec::with_capacity(65_536);
    let mut n_props_aretes = 0u64;
    for e in g.edges() {
        let (Some(&s), Some(&t)) = (map.get(&e.source.0), map.get(&e.target.0)) else {
            continue;
        };
        batch.push((s, t, e.kind.label()));
        provenance.push(&e.props);
        n_edges += 1;
        if batch.len() >= 65_536 {
            ecrire_lot_aretes(db, &mut batch, &mut provenance, &mut n_props_aretes);
        }
    }
    // ── RÉUNIFICATION (2026-10-06) ──────────────────────────────────────────
    // La lignée servie vidait le lot par `create_edges_batch_no_wal` ; le
    // multi-écrivain passe par `ecrire_lot_aretes`, qui écrit EN PLUS la
    // provenance de chaque arête (et refuse bruyamment d'écrire une provenance
    // décalée — cf. son commentaire). C'est un sur-ensemble : on garde sa ligne.
    ecrire_lot_aretes(db, &mut batch, &mut provenance, &mut n_props_aretes);
    for n in g.nodes() {
        if n.kind == obrain_indexer::NodeKind::Project
            && let Some(&nid) = map.get(&n.id.0)
        {
            db.set_node_property(
                nid,
                "empreinte_depot",
                OV::String(empreinte.to_string().into()),
            );
        }
    }
    (n_edges, n_props_aretes)
}

pub(crate) fn index_repository(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    use obrain_common::types::Value as OV;
    use std::collections::HashMap;
    let Some(path) = arg_str(arguments, "path").or_else(|| arg_str(arguments, "repo")) else {
        return (
            json!({ "status": "error", "error": "index_repository: path requis" }),
            true,
        );
    };
    if !std::path::Path::new(&path).is_dir() {
        return (
            json!({ "status": "error", "error": format!("index_repository: dépôt introuvable : {path}") }),
            true,
        );
    }
    let prof = std::env::var("OBRAIN_INDEX_PROF").is_ok();
    let t0 = std::time::Instant::now();
    // ── `exclure_artefacts` : ICI, PAS DANS `ingestion_native` ──────────────
    // (réunification 2026-10-06 — voir la note de `ingestion_native`.)
    // `exclure_artefacts` est exposé POUR POUVOIR MESURER : indexer deux fois le
    // même dépôt, avec et sans, est la seule façon d'établir que la politique
    // change quelque chose — au lieu de l'affirmer.
    let exclure = arguments
        .get("exclure_artefacts")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let mut options = obrain_indexer::index::IndexOptions::default();
    options.discovery.exclure_artefacts = exclure;
    // ── UN DÉPÔT QUI N'A PAS BOUGÉ N'A RIEN À RÉINDEXER (T27) ─────────────
    // Sur une base déjà peuplée, l'empreinte du dépôt (fichiers retenus,
    // binaire, contexte git — lue sans rien analyser) est comparée à celle
    // rangée sur le nœud Project au dernier index. Égale : la base EST déjà
    // le résultat, on le dit et on s'arrête.
    if db.node_count() > 0 {
        let empreinte = format!(
            "{:032x}",
            obrain_indexer::index::empreinte_depot(std::path::Path::new(&path), &options)
        );
        let rangee = db
            .session()
            .execute("MATCH (p:Project) WHERE p.empreinte_depot IS NOT NULL RETURN p.empreinte_depot LIMIT 1")
            .ok()
            .and_then(|r| r.rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|v| match v {
                OV::String(t) => Some(t.to_string()),
                _ => None,
            });
        if prof {
            eprintln!(
                "[prof] empreinte du dépôt : {:.2}s ({} ; rangée : {})",
                t0.elapsed().as_secs_f64(),
                empreinte,
                rangee.as_deref().unwrap_or("aucune")
            );
        }
        if rangee.as_deref() == Some(empreinte.as_str()) {
            return (
                json!({
                    "status": "ok",
                    "tool": "index_repository",
                    "repo": path,
                    "inchange": true,
                    "empreinte_depot": empreinte,
                    "nodes": db.node_count(),
                    "edges": db.edge_count(),
                    "seconds": t0.elapsed().as_secs_f64(),
                    "raison": "le dépôt n'a pas changé depuis le dernier index : la base est déjà à jour",
                }),
                false,
            );
        }
    }
    let idx = obrain_indexer::index::index_repository(std::path::Path::new(&path), &options);
    let empreinte = format!("{:032x}", idx.empreinte_depot);
    let t_parse = t0.elapsed();
    if prof {
        eprintln!(
            "[prof] parse+resolve+graph : {:.1}s ({} nœuds / {} arêtes en mémoire)",
            t_parse.as_secs_f64(),
            idx.graph.node_count(),
            idx.graph.edges().len()
        );
    }
    let g = &idx.graph;
    // ── INGESTION NATIVE SUR BASE NEUVE (T27, 2026-09-16) ─────────────────
    // Le chemin unitaire ci-dessous écrivait ≈ 12 M propriétés une à une :
    // mesuré 1 245 s sur le noyau Linux, 96 % du total. Sur une base neuve,
    // nœuds, arêtes et propriétés partent par les zones directement — même
    // disposition sur disque, prouvée par
    // `obrain-substrate/tests/ingestion_props_en_lot.rs`. Si la base n'est
    // pas neuve (`bulk_import` refuse), on garde le chemin historique.
    // ── RÉUNIFICATION (2026-10-06) : LE COMPTE DE PROVENANCE REMONTE AUSSI ──
    // Le multi-écrivain rendait `properties_aretes` dans le résultat, mais son
    // compteur vivait dans LA fonction unique qui faisait tout. La lignée servie
    // a scindé cette fonction en deux (`ingestion_native` / `ingestion_unitaire`)
    // : le compteur doit donc être RENDU, sinon il n'est plus en portée. Les deux
    // chemins rendent maintenant `(arêtes, provenance)`.
    let (n_edges, n_props_aretes) = match ingestion_native(db, g, &empreinte) {
        Ok(v) => v,
        Err(raison) => {
            if prof {
                eprintln!("[prof] ingestion native refusée ({raison}) — chemin unitaire");
            }
            ingestion_unitaire(db, g, &empreinte)
        }
    };
    let t_ck = std::time::Instant::now();
    let _ = db.checkpoint();
    if prof {
        eprintln!("[prof]   checkpoint : {:.2}s", t_ck.elapsed().as_secs_f64());
    }
    let t_write = t0.elapsed() - t_parse;
    if prof {
        let ops = idx.graph.node_count() as u64 + idx.graph.edges().len() as u64;
        eprintln!(
            "[prof] ecriture (nœuds+arêtes+props+checkpoint) : {:.1}s → {:.0} k-op/s ; \
             provenance d'arêtes : {n_props_aretes} paires sur {n_edges} arêtes",
            t_write.as_secs_f64(),
            ops as f64 / t_write.as_secs_f64() / 1000.0
        );
    }
    // Index d'égalité natif substrate sur qualified_name → lookups O(1)/µs ;
    // idem sur `name` : les centres/densifications par nom court (suffixe)
    // passent par l'égalité indexée au lieu du scan ENDS WITH (1,2-3,4 s sur
    // les gros dépôts — voir viz_extract::resolve_node_opts, paliers 2-3).
    let t_idx = std::time::Instant::now();
    db.create_property_indexes(&["qualified_name", "name"]);
    if prof {
        eprintln!(
            "[prof] index d'égalité (qualified_name, name) : {:.2}s",
            t_idx.elapsed().as_secs_f64()
        );
        eprintln!(
            "[prof] TOTAL index_repository : {:.2}s",
            t0.elapsed().as_secs_f64()
        );
        // ── LA TÉLÉMÉTRIE DU GROUP COMMIT, SI ELLE EST DEMANDÉE ─────────────
        // `OBRAIN_GC_DIAG=1` rend les compteurs du group commit sur le flux de
        // profil : c'est ce qui permet de dire OÙ le temps passe au lieu de le
        // déduire (des zéros sans la variable).
        if std::env::var("OBRAIN_GC_DIAG").is_ok_and(|v| v == "1")
            && let Some(s) = db.substrate_handle()
        {
            eprintln!("[prof] {}", s.writer().diagnostic_group_commit());
        }
    }
    let (files_scanned, n_nodes) = (idx.files_scanned, g.node_count());
    // ── CE QUI A ÉTÉ ÉCARTÉ, ET POURQUOI (multi-écrivain, réunifié) ────────
    // La politique d'indexation se lit dans le RÉSULTAT. Sinon elle reste
    // invisible — et une exclusion invisible ne se conteste pas, ne se compte
    // pas, et ne se corrige pas.
    //
    // Ces deux lectures sont faites AVANT `drop(idx)` : la lignée servie libère
    // le graphe en arrière-plan (il pèse plusieurs Go sur le noyau, et le
    // libérer coûtait des secondes À LA RÉPONSE), donc `idx` est DÉPLACÉ juste
    // après. On fige donc ici ce que le JSON final en tire.
    let exclus = idx.exclusions.len();
    let mut par_raison: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for e in &idx.exclusions {
        *par_raison.entry(e.raison.libelle()).or_insert(0) += 1;
    }
    // (T27) Le graphe en mémoire pèse plusieurs Go sur le noyau : sa
    // libération coûtait des secondes À LA RÉPONSE. Elle part en arrière-plan
    // — la mémoire est rendue quand même, la réponse n'attend plus.
    std::thread::spawn(move || drop(idx));
    (
        json!({
            "status": "ok",
            "tool": "index_repository",
            "repo": path,
            "files_scanned": files_scanned,
            "empreinte_depot": empreinte,
            "nodes": n_nodes,
            "edges": n_edges,
            // La provenance des arêtes est écrite : le dire ici rend le défaut
            // vérifiable depuis le résultat, sans requête séparée.
            "properties_aretes": n_props_aretes,
            "seconds": t0.elapsed().as_secs_f64(),
            "indexed": ["qualified_name", "name"],
            "exclus": exclus,
            "exclus_par_raison": par_raison,
            "exclure_artefacts": exclure,
        }),
        false,
    )
}

/// Écrit un lot d'arêtes **avec la provenance de chacune**.
///
/// `create_edges_batch_no_wal` rend ses identifiants **dans l'ordre d'entrée**
/// (`crud.rs:814-822`) : l'index `i` du lot correspond à l'index `i` de la
/// provenance. C'est ce qui permet de réapparier sans structure intermédiaire —
/// et c'est aussi pourquoi un décalage entre les deux listes est traité comme un
/// **défaut bruyant** : attribuer la provenance d'une arête à une autre serait
/// pire que de la perdre, parce que le résultat serait faux *sans être vide*.
fn ecrire_lot_aretes(
    db: &ObrainDB,
    lot: &mut Vec<(
        obrain_common::types::NodeId,
        obrain_common::types::NodeId,
        &str,
    )>,
    provenance: &mut Vec<&obrain_indexer::graph::EdgeProps>,
    n_props: &mut u64,
) {
    use obrain_common::types::Value as OV;
    if lot.is_empty() {
        return;
    }
    let ids = db.create_edges_batch_no_wal(lot);
    if ids.len() != provenance.len() {
        eprintln!(
            "index_repository : {} identifiants rendus pour {} arêtes — provenance NON écrite \
             (un décalage l'attribuerait à la mauvaise arête)",
            ids.len(),
            provenance.len()
        );
        lot.clear();
        provenance.clear();
        return;
    }
    for (id, p) in ids.iter().zip(provenance.iter()) {
        for (k, v) in &p.extra {
            let val = match v {
                serde_json::Value::String(s) => OV::String(s.clone().into()),
                serde_json::Value::Bool(b) => OV::Bool(*b),
                serde_json::Value::Number(x) => x
                    .as_i64()
                    .map(OV::Int64)
                    .unwrap_or_else(|| OV::Float64(x.as_f64().unwrap_or(0.0))),
                serde_json::Value::Null => OV::Null,
                // Une provenance structurée (liste/objet) n'a pas de
                // représentation scalaire ici : on la SÉRIALISE en texte au lieu
                // de la perdre en silence.
                autre => OV::String(autre.to_string().into()),
            };
            let _ = db.set_edge_property(*id, k, val);
            *n_props += 1;
        }
    }
    lot.clear();
    provenance.clear();
}

/// `create_property_index` — index d'égalité natif substrate (O(1) lookup).
fn create_property_index(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(property) = arg_str(arguments, "property").filter(|p| !p.is_empty()) else {
        return (
            json!({ "status": "error", "error": "create_property_index: property requis" }),
            true,
        );
    };
    db.create_property_index(&property);
    (
        json!({
            "status": "ok",
            "tool": "create_property_index",
            "property": property,
        }),
        false,
    )
}

/// Conversion JSON → obrain Value, RÉCURSIVE (objets, listes, scalaires).
fn json_to_obrain_value_deep(v: &Value) -> obrain_common::types::Value {
    match v {
        Value::Null => obrain_common::types::Value::Null,
        Value::Bool(b) => obrain_common::types::Value::Bool(*b),
        Value::Number(n) => n
            .as_i64()
            .map(obrain_common::types::Value::from)
            .or_else(|| n.as_f64().map(obrain_common::types::Value::from))
            .unwrap_or(obrain_common::types::Value::Null),
        Value::String(s) => obrain_common::types::Value::from(s.clone()),
        Value::Array(items) => {
            let vals: Vec<obrain_common::types::Value> =
                items.iter().map(json_to_obrain_value_deep).collect();
            obrain_common::types::Value::List(vals.into())
        }
        Value::Object(map) => {
            let mut inner: std::collections::BTreeMap<
                obrain_common::types::PropertyKey,
                obrain_common::types::Value,
            > = std::collections::BTreeMap::new();
            for (k, val) in map {
                inner.insert(k.clone().into(), json_to_obrain_value_deep(val));
            }
            obrain_common::types::Value::Map(inner.into())
        }
    }
}

/// Sérialisation obrain Value → JSON typé (lignes de résultat de query).
fn value_to_json(v: &obrain_common::types::Value) -> Value {
    use obrain_common::types::Value as OV;
    match v {
        OV::Null => Value::Null,
        OV::Bool(b) => json!(b),
        OV::Int64(i) => json!(i),
        OV::Float64(f) => json!(f),
        OV::String(s) => Value::String(s.to_string()),
        OV::List(items) => {
            let arr: Vec<Value> = items.iter().map(value_to_json).collect();
            Value::Array(arr)
        }
        OV::Map(entries) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in entries.iter() {
                obj.insert(k.as_str().to_string(), value_to_json(val));
            }
            Value::Object(obj)
        }
        OV::Timestamp(t) => Value::String(t.to_string()),
        OV::Date(d) => Value::String(d.to_string()),
        OV::Time(t) => Value::String(t.to_string()),
        OV::ZonedDatetime(z) => Value::String(z.to_string()),
        OV::Bytes(b) => Value::String(format!("<{} bytes>", b.len())),
        OV::Vector(vec) => {
            let arr: Vec<Value> = vec.iter().map(|f| json!(f)).collect();
            Value::Array(arr)
        }
        other => Value::String(other.to_string()),
    }
}

/// `memory_register` — l'agent influence SA mémoire : marque une
/// co-activation explicite (épisode engram). Au fil du développement,
/// quand on touche des fichiers/symboles ENSEMBLE, on l'enregistre —
/// la prochaine fois, `memory_recall`/`context_build` le rappellera.
fn memory_register(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let nodes: Vec<obrain_common::types::NodeId> = arguments
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64().or_else(|| v.as_i64().map(|i| i as u64)))
                .map(obrain_common::types::NodeId)
                .collect()
        })
        .unwrap_or_default();
    if nodes.len() < 2 {
        return (
            json!({ "status": "error", "error": "memory_register: ≥ 2 nœuds requis (une co-activation)" }),
            true,
        );
    }
    let pe = arguments
        .get("prediction_error")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.3);
    let Some(ce) = db.cognitive_engine() else {
        return (
            json!({ "status": "error", "error": "memory_register: pas de moteur cognitif" }),
            true,
        );
    };
    let Some(mgr) = ce.engram_manager() else {
        return (
            json!({ "status": "error", "error": "memory_register: pas d'engram manager (feature cognitive-engram)" }),
            true,
        );
    };
    mgr.record_activation(&nodes, pe);
    mgr.tick();
    (
        json!({
            "status": "ok",
            "episode": nodes.len(),
            "engrams_formed": mgr.store().count(),
        }),
        false,
    )
}

/// `memory_recall` — rappelle les engrams pertinents pour des cues.
fn memory_recall(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let limit = arg_usize(arguments, "limit", 8);
    let cues: Vec<obrain_common::types::NodeId> = arguments
        .get("cues")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64().or_else(|| v.as_i64().map(|i| i as u64)))
                .map(obrain_common::types::NodeId)
                .collect()
        })
        .unwrap_or_default();
    if cues.is_empty() {
        return (
            json!({ "status": "error", "error": "memory_recall: cues requis" }),
            true,
        );
    }
    let Some(ce) = db.cognitive_engine() else {
        return (
            json!({ "status": "error", "error": "memory_recall: pas de moteur cognitif" }),
            true,
        );
    };
    let Some(mgr) = ce.engram_manager() else {
        return (
            json!({ "status": "error", "error": "memory_recall: pas d'engram manager" }),
            true,
        );
    };
    let recalls = mgr.recall(&cues, limit);
    let results: Vec<Value> = recalls
        .iter()
        .map(|r| {
            let members: Vec<Value> = r
                .engram
                .ensemble
                .iter()
                .take(10)
                .map(|(nid, w)| json!({ "id": nid.0, "weight": w }))
                .collect();
            json!({
                "engram_id": r.engram_id.0,
                "confidence": r.confidence,
                "members": members,
            })
        })
        .collect();
    (
        json!({
            "status": "ok",
            "total": recalls.len(),
            "recalls": results,
        }),
        false,
    )
}

/// `predict` — la couche prédictive JEPA : prédit l'embedding suivant
/// (centroïde spectral pondéré des engrams rappelés) + la surprise.
/// Lecture pure : n'alimente aucun compteur `(n, k)` — le postérieur Beta
/// de la couche de décision reste intact ; la surprise est le signal destiné
/// à moduler l'exploration.
fn predict(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let limit = arg_usize(arguments, "limit", 8);
    let cues: Vec<obrain_common::types::NodeId> = arguments
        .get("cues")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64().or_else(|| v.as_i64().map(|i| i as u64)))
                .map(obrain_common::types::NodeId)
                .collect()
        })
        .unwrap_or_default();
    if cues.is_empty() {
        return (
            json!({ "status": "error", "error": "predict: cues requis" }),
            true,
        );
    }
    let Some(ce) = db.cognitive_engine() else {
        return (
            json!({ "status": "error", "error": "predict: pas de moteur cognitif" }),
            true,
        );
    };
    let Some(mgr) = ce.engram_manager() else {
        return (
            json!({ "status": "error", "error": "predict: pas d'engram manager" }),
            true,
        );
    };
    match mgr.predict(&cues, limit) {
        Some(p) => (
            json!({
                "status": "ok",
                "predicted": p.predicted,
                "surprise": p.surprise,
                "attention": p
                    .attention
                    .iter()
                    .map(|(id, w)| json!({ "engram_id": id.0, "weight": w }))
                    .collect::<Vec<_>>(),
            }),
            false,
        ),
        None => (
            json!({
                "status": "ok",
                "predicted": [],
                "surprise": 1.0,
                "attention": [],
            }),
            false,
        ),
    }
}

/// `session_load` — charge une session (messages + événements), paginé.
/// Contrat session-persistence-graph : `{session_id, nodes, messages,
/// events, has_more}` où messages = [{seq, role, qn, text}].
fn session_load(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session_id = arg_str(arguments, "session_id").unwrap_or_default();
    if session_id.is_empty() {
        return (
            json!({ "status": "error", "error": "session_load: session_id requis" }),
            true,
        );
    }
    let events_only = arguments
        .get("events_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let events_limit = arg_usize(arguments, "events_limit", 500);
    let after_seq = arguments
        .get("events_after_seq")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let exclude_chunks = arguments
        .get("events_exclude_chunks")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // Mode paginé du fallback loadByNodes : limit/offset (sans events_only).
    let limit = arg_usize(arguments, "limit", 500);
    let offset = arg_usize(arguments, "offset", 0);
    let session = db.session();

    // Le nœud Session (pour nodes[] — le meta).
    let node = match session.execute(&format!(
        "MATCH (s:Session {{qualified_name: '{session_id}'}}) RETURN s"
    )) {
        Ok(r) => r.rows.first().and_then(|row| row.first()).map(|v| {
            json!({
                "qn": session_id,
                "label": "Session",
                "name": session_id,
                "properties": serde_json::to_string(&session_node_props_to_json(v))
                    .unwrap_or_else(|_| "{}".to_string()),
            })
        }),
        Err(_) => None,
    };

    // Les messages de la session, ordonnés par seq (pagés).
    // (2026-09-07, PERF session.models/history) Requête messages UNIQUEMENT
    // hors events_only. En events_only (le chemin du harness : readRequestHeader
    // / readRecent / session_load), le client ne consomme que `events` (le
    // journal) — les `messages` graphe étaient calculés INCONDITIONNELLEMENT,
    // et ce pattern (Message-[:PART_OF]->Session + ORDER BY m.seq) coûte
    // ~1,45 s sur le store runtime MÊME quand la session est inconnue (scan
    // planifié sur tous les nœuds Message) → session.models/session.history
    // ~1,5 s. Le nœud Session (meta, µs) reste fourni.
    let messages: Vec<Value> = if events_only {
        Vec::new()
    } else {
        match session.execute(&format!(
            // (2026-09-07) FORME ANCRE-D'ABORD : `(s:Session {qn})<-[:PART_OF]-(m)`
            // au lieu de `(m)-[:PART_OF]->(s:Session {qn})`. Le plan de la
            // forme Message→Session démarre par le scan des nœuds Message
            // (~1,4 s même à résultat vide) ; la forme ancre part du nœud
            // Session unique puis expand entrant — mesuré 1 460 ms → 4-17 ms.
            "MATCH (s:Session {{qualified_name: '{session_id}'}})<-[:PART_OF]-(m:Message) \
             RETURN m.seq, m.role, m.text ORDER BY m.seq LIMIT {events_limit}"
        )) {
            Ok(r) => r
                .rows
                .iter()
                .filter_map(|row| {
                    let seq = row.first().and_then(|v| v.as_int64()).unwrap_or(0);
                    let role = row.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let text = row.get(2).map(|v| v.to_string()).unwrap_or_default();
                    Some(json!({ "seq": seq, "role": role, "qn": session_id, "text": text }))
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    };

    // Le flux d'événements (LE JOURNAL) : les LIGNES BRUTES du log de
    // session ({type,seq,time,data} — ce que decodeStorageRecord attend).
    // La propriété `events` du nœud Session (stockée par le merge) contient
    // la liste des lignes du journal, triées par seq. Pagination :
    // events_after_seq (reprendre après ce seq) + events_limit.
    let mut events: Vec<Value> = Vec::new();
    let mut events_last_seq: i64 = -1;
    if events_only {
        // (2026-09-01) Le journal vit dans les ZONES substrate obrain
        // (substrate.journal.<session>) — migré depuis le MCP, PAS dans le
        // graphe (7M lignes en propriétés → store 6 Go → timeout bridge).
        // `before_seq` : le harness lit la fenêtre récente (readRecent).
        let before_seq = arguments
            .get("events_before_seq")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let limit = if events_limit > 0 { events_limit } else { 500 };
        let lines = db.read_session_journal(&session_id, after_seq, before_seq, limit);
        for line in lines {
            let seq = event_seq(&line);
            // (2026-09-01) ALIGNEMENT MCP : le harness attend les events en
            // OBJETS {type,seq,time,data} — readRequestHeader/readRecent font
            // `line?.type` direct. Les lignes brutes (strings) → line.type
            // undefined → le header de requête jamais trouvé → « Resume
            // failed ». Le MCP (codebase-memory-mcp) renvoie des objets.
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => events.push(v),
                Err(_) => events.push(Value::String(line)),
            }
            if seq > events_last_seq {
                events_last_seq = seq;
            }
        }
    }

    // MODE loadByNodes (limit/offset, pas events_only) : le fallback du
    // harness reconstruit les SessionEvents depuis les NŒUDS du graphe
    // (Message/ToolCall/ToolResult avec qn/label/name/properties/text).
    let page_nodes: Vec<Value> = if !events_only && (limit > 0 || offset > 0) {
        let limitq = if limit > 0 { limit } else { 500 };
        match session.execute(&format!(
            "MATCH (n)-[:PART_OF]->(s:Session {{qualified_name: '{session_id}'}}) \
             WHERE n:Message OR n:ToolCall OR n:ToolResult \
             RETURN n.qualified_name, labels(n), n.name, n.seq, n.role, n.turn, n.step, \
                    n.callId, n.toolCallId, n.text \
             ORDER BY n.seq SKIP {offset} LIMIT {limitq}"
        )) {
            Ok(r) => r
                .rows
                .iter()
                .filter_map(|row| {
                    let qn = row.first().map(|v| v.to_string()).unwrap_or_default();
                    if qn.is_empty() || qn == "NULL" {
                        return None;
                    }
                    let labels = row.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let label = if labels.contains("Message") {
                        "Message"
                    } else if labels.contains("ToolCall") {
                        "ToolCall"
                    } else if labels.contains("ToolResult") {
                        "ToolResult"
                    } else {
                        return None;
                    };
                    let name = row.get(2).map(|v| v.to_string()).unwrap_or_default();
                    let seq = row.get(3).and_then(|v| v.as_int64()).unwrap_or(0);
                    let role = row.get(4).map(|v| v.to_string()).unwrap_or_default();
                    let turn = row.get(5).and_then(|v| v.as_int64()).unwrap_or(0);
                    let step = row.get(6).and_then(|v| v.as_int64()).unwrap_or(0);
                    let call_id = row.get(7).map(|v| v.to_string()).unwrap_or_default();
                    let tool_call_id = row.get(8).map(|v| v.to_string()).unwrap_or_default();
                    let text = row.get(9).map(|v| v.to_string()).unwrap_or_default();
                    let mut props = serde_json::Map::new();
                    props.insert("seq".into(), json!(seq));
                    if !role.is_empty() && role != "NULL" {
                        props.insert("role".into(), json!(role));
                    }
                    if !call_id.is_empty() && call_id != "NULL" {
                        props.insert("callId".into(), json!(call_id));
                    }
                    if !tool_call_id.is_empty() && tool_call_id != "NULL" {
                        props.insert("toolCallId".into(), json!(tool_call_id));
                    }
                    props.insert("turn".into(), json!(turn));
                    props.insert("step".into(), json!(step));
                    Some(json!({
                        "qn": qn,
                        "label": label,
                        "name": if name.is_empty() || name == "NULL" { qn } else { name },
                        "properties": serde_json::to_string(&serde_json::Value::Object(props))
                            .unwrap_or_else(|_| "{}".to_string()),
                        "text": text,
                    }))
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let has_more = if !events_only && (limit > 0 || offset > 0) {
        // has_more : y a-t-il une page suivante ?
        let next_offset = offset + limit.max(0);
        matches!(
            session.execute(&format!(
                "MATCH (n)-[:PART_OF]->(s:Session {{qualified_name: '{session_id}'}}) \
                 WHERE n:Message OR n:ToolCall OR n:ToolResult \
                 RETURN count(n)"
            )),
            Ok(r) if r
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(|v| v.as_int64())
                .map(|c| c as usize > next_offset)
                .unwrap_or(false)
        )
    } else {
        false
    };

    let mut resp = json!({
        "status": "ok",
        "session_id": session_id,
        "nodes": if page_nodes.is_empty() {
            node.map(|n| vec![n]).unwrap_or_default()
        } else {
            page_nodes
        },
        "messages": messages,
        "events": events,
        "has_more": has_more,
    });
    if events_only {
        resp["events_has_more"] = json!(false);
        resp["events_last_seq"] = json!(events_last_seq);
    }
    (resp, false)
}

/// `journal_import` — écrit le journal brut d'une session dans la zone
/// substrate dédiée (migration depuis le MCP : runtime_event_data → zone).
fn journal_import(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session_id = arg_str(arguments, "session_id").unwrap_or_default();
    if session_id.is_empty() {
        return (
            json!({ "status": "error", "error": "journal_import: session_id requis" }),
            true,
        );
    }
    let lines: Vec<String> = arguments
        .get("lines")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if lines.is_empty() {
        return (
            json!({ "status": "error", "error": "journal_import: aucune ligne" }),
            true,
        );
    }
    match db.write_session_journal(&session_id, &lines) {
        Ok(()) => (
            json!({ "status": "ok", "session_id": session_id, "lines": lines.len() }),
            false,
        ),
        Err(e) => (json!({ "status": "error", "error": e }), true),
    }
}

/// Le seq d'une ligne d'événement (parse JSON).
fn event_seq(line: &str) -> i64 {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("seq").and_then(|s| s.as_i64()))
        .unwrap_or(-1)
}

// ─────────────────────────────────────────────────────────────────────────────
// Lot C (P1, ingestion) — import_owners / import_workspace / import_docs /
// import_sessions / manage_adr. Même style que graph_link/ingest_traces :
// upsert par qualified_name via les helpers merge_node/link_edge/node_exists,
// écriture de registres lus DEPUIS LE PROCESS (chemin en args).
// ─────────────────────────────────────────────────────────────────────────────

/// Prépare le qualified_name d'une session depuis un id brut — même
/// normalisation qu'import_events : préfixe `session-` ajouté si absent (les
/// registres du harness adressent parfois l'id nu).
fn session_qn_of(sid: &str) -> String {
    let sid = sid.trim();
    if sid.starts_with("session-") {
        sid.to_string()
    } else {
        format!("session-{sid}")
    }
}

/// qn utilisable INLINE dans une requête GQL (MERGE/MATCH par qualified_name) :
/// non vide, sans guillemets ni backslash (sinon rupture de syntaxe).
fn qn_inline_safe(qn: &str) -> bool {
    !qn.is_empty() && !qn.contains(['\'', '"', '\\'])
}

/// Lit un payload de propriété STRING d'un nœud (retour None si absent/non
/// string/null) — lecture `content`/`payload` des nœuds ADR/Skill/Plan.
fn node_prop_str(
    session: &obrain_engine::Session,
    label: &str,
    qn: &str,
    prop: &str,
) -> Option<String> {
    use obrain_common::types::Value as OV;
    let q = format!("MATCH (n:{label} {{qualified_name: '{qn}'}}) RETURN n.{prop}");
    match session.execute(&q) {
        Ok(r) => r
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| match v {
                OV::String(s) => Some(s.to_string()),
                _ => None,
            }),
        Err(_) => None,
    }
}

/// `import_owners` — registre session→compte `{sessionId: accountId}` (fichier
/// JSON) → nœuds Account (qn `account:<id>`, préfixe ajouté si absent) + arêtes
/// Session-[:OWNED_BY]->Account. Upsert par qn : les Session absentes sont
/// créées (mêmes qn `session-<id>` que le writer runtime). Idempotent : un
/// second import du même registre ne crée aucun nœud (MERGE + MERGE d'arête).
fn import_owners(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(registry) = arg_str(arguments, "registry") else {
        return (
            json!({ "status": "error", "error": "import_owners: registry (chemin JSON) requis" }),
            true,
        );
    };
    let parsed: Value = match std::fs::read_to_string(&registry) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return (
                    json!({ "status": "error", "error": format!("import_owners: JSON invalide ({registry}) : {e}") }),
                    true,
                );
            }
        },
        Err(e) => {
            return (
                json!({ "status": "error", "error": format!("import_owners: lecture impossible ({registry}) : {e}") }),
                true,
            );
        }
    };
    let Some(map) = parsed.as_object() else {
        return (
            json!({ "status": "error", "error": "import_owners: registre = objet {sessionId: accountId}" }),
            true,
        );
    };
    let session = db.session();
    let mut accounts: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut sessions: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut created_accounts = 0usize;
    let mut created_sessions = 0usize;
    let mut links = 0usize;
    let mut skipped = 0usize;
    let mut write_failures = 0usize;
    for (sid, acc) in map.iter().take(5000) {
        let Some(acc) = acc.as_str() else {
            skipped += 1;
            continue;
        };
        if sid.is_empty() || acc.is_empty() {
            skipped += 1;
            continue;
        }
        let sqn = session_qn_of(sid);
        let aqn = if acc.starts_with("account:") {
            acc.to_string()
        } else {
            format!("account:{acc}")
        };
        if !qn_inline_safe(&sqn) || !qn_inline_safe(&aqn) {
            skipped += 1;
            continue;
        }
        if !node_exists(&session, "Account", &aqn) {
            let id = merge_node(&session, "Account", &aqn, None);
            if id > 0 {
                created_accounts += 1;
            } else {
                write_failures += 1;
            }
        }
        if !node_exists(&session, "Session", &sqn) {
            let id = merge_node(&session, "Session", &sqn, None);
            if id > 0 {
                created_sessions += 1;
            } else {
                write_failures += 1;
            }
        }
        link_edge(&session, "Session", &sqn, "Account", &aqn, "OWNED_BY");
        accounts.insert(aqn);
        sessions.insert(sqn);
        links += 1;
    }
    if links > 0 {
        // Des nœuds Session ont pu apparaître → invalide le mémo de sessions_list.
        SESSIONS_LIST_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    if links == 0 && write_failures > 0 {
        return (
            json!({ "status": "error", "error": "import_owners: aucune écriture possible (store en lecture seule ?)" }),
            true,
        );
    }
    (
        json!({
            "status": "ok",
            "tool": "import_owners",
            "registry": registry,
            "accounts": accounts.len(),
            "sessions": sessions.len(),
            "links": links,
            "created_accounts": created_accounts,
            "created_sessions": created_sessions,
            "skipped": skipped,
        }),
        false,
    )
}

/// `import_workspace` — registre de workspaces `{id: {path, title, sessionIds}}`
/// (fichier JSON ; variante enveloppée `{"tables": {"workspaces": {...}}}`
/// tolérée pour compatibilité avec les exports du registre du harness) → nœuds
/// Workspace (qn `workspace:<id>`, props path/title) + arêtes
/// Session-[:WORKSPACE]->Workspace. Upsert par qn, idempotent.
fn import_workspace(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let Some(registry) = arg_str(arguments, "registry") else {
        return (
            json!({ "status": "error", "error": "import_workspace: registry (chemin JSON) requis" }),
            true,
        );
    };
    let parsed: Value = match std::fs::read_to_string(&registry) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return (
                    json!({ "status": "error", "error": format!("import_workspace: JSON invalide ({registry}) : {e}") }),
                    true,
                );
            }
        },
        Err(e) => {
            return (
                json!({ "status": "error", "error": format!("import_workspace: lecture impossible ({registry}) : {e}") }),
                true,
            );
        }
    };
    // Dénestage optionnel tables.workspaces (format registre du harness).
    let mut root = parsed;
    if let Some(tables) = root.get("tables").and_then(|t| t.as_object()) {
        if let Some(ws) = tables.get("workspaces") {
            root = ws.clone();
        }
    }
    let Some(map) = root.as_object() else {
        return (
            json!({ "status": "error", "error": "import_workspace: registre = objet {id: {path, title, sessionIds}}" }),
            true,
        );
    };
    let session = db.session();
    let mut workspaces: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut sessions: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut created_workspaces = 0usize;
    let mut created_sessions = 0usize;
    let mut links = 0usize;
    let mut skipped = 0usize;
    for (wid, info) in map.iter().take(2000) {
        let Some(obj) = info.as_object() else {
            skipped += 1;
            continue;
        };
        if wid.is_empty() {
            skipped += 1;
            continue;
        }
        let wqn = if wid.starts_with("workspace:") {
            wid.clone()
        } else {
            format!("workspace:{wid}")
        };
        if !qn_inline_safe(&wqn) {
            skipped += 1;
            continue;
        }
        // Props path/title (strings) — nœud MERGE + SET par params (n'écrase
        // que les clés présentes dans le registre).
        let mut props = serde_json::Map::new();
        if let Some(p) = obj.get("path").and_then(|v| v.as_str()) {
            props.insert("path".into(), json!(p));
        }
        if let Some(t) = obj.get("title").and_then(|v| v.as_str()) {
            props.insert("title".into(), json!(t));
        }
        props.insert("source".into(), json!("import_workspace"));
        if !node_exists(&session, "Workspace", &wqn) {
            let id = merge_node(&session, "Workspace", &wqn, None);
            if id > 0 {
                created_workspaces += 1;
            }
        }
        set_node_props(&session, "Workspace", &wqn, &props);
        let mut ws_session_ids: Vec<String> = Vec::new();
        if let Some(ids) = obj.get("sessionIds").and_then(|v| v.as_array()) {
            for sid in ids.iter().filter_map(|v| v.as_str()).take(500) {
                ws_session_ids.push(session_qn_of(sid));
            }
        }
        for sqn in ws_session_ids {
            if !qn_inline_safe(&sqn) {
                skipped += 1;
                continue;
            }
            if !node_exists(&session, "Session", &sqn) {
                let id = merge_node(&session, "Session", &sqn, None);
                if id > 0 {
                    created_sessions += 1;
                }
            }
            link_edge(&session, "Session", &sqn, "Workspace", &wqn, "WORKSPACE");
            sessions.insert(sqn);
            links += 1;
        }
        workspaces.insert(wqn);
    }
    if links > 0 {
        SESSIONS_LIST_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    (
        json!({
            "status": "ok",
            "tool": "import_workspace",
            "registry": registry,
            "workspaces": workspaces.len(),
            "sessions": sessions.len(),
            "links": links,
            "created_workspaces": created_workspaces,
            "created_sessions": created_sessions,
            "skipped": skipped,
        }),
        false,
    )
}

/// Collecte récursive (std::fs, sans dépendance) des fichiers `*.md` sous une
/// racine — trié pour le déterminisme ; les symlinks vers des répertoires ne
/// sont pas suivis (boucles possibles), ceux vers des fichiers .md sont pris.
fn collect_md_files(
    root: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
    limit: usize,
) -> usize {
    let mut pending: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let mut over = 0usize;
    while let Some(dir) = pending.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<std::path::PathBuf> = Vec::new();
        for e in rd.flatten() {
            entries.push(e.path());
        }
        entries.sort();
        for p in entries {
            if out.len() >= limit {
                over += 1;
                continue;
            }
            let Ok(ft) = p.metadata() else { continue };
            if ft.is_dir() {
                // Ne descend que les vrais répertoires (pas les symlinks).
                let is_symlink = std::fs::symlink_metadata(&p)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false);
                if !is_symlink {
                    pending.push(p);
                }
            } else if ft.is_file()
                && p.extension().map(|e| e.to_string_lossy().to_lowercase())
                    == Some("md".to_string())
            {
                out.push(p);
            }
        }
    }
    over
}

/// Importe UN fichier markdown en nœud Skill/Plan. Retourne (label, qn) ou une
/// erreur lisible. `root` = répertoire racine (mode répertoire) — le qn est
/// alors le chemin RELATIF au root (extension .md retirée) ; sans root (mode
/// fichier unique), le qn porte le chemin fourni.
fn import_doc_file(
    db: &ObrainDB,
    session: &obrain_engine::Session,
    kind: &str,
    path: &std::path::Path,
    root: Option<&std::path::Path>,
) -> Result<(String, String), String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("{} : {e}", path.display()))?;
    let chemin: String = match root {
        Some(r) => {
            let rel = path.strip_prefix(r).unwrap_or(path);
            rel.to_string_lossy().replace('\\', "/")
        }
        None => path.to_string_lossy().replace('\\', "/"),
    };
    let chemin = chemin
        .strip_suffix(".md")
        .or_else(|| chemin.strip_suffix(".MD"))
        .unwrap_or(&chemin)
        .to_string();
    let kind_lower = kind.to_lowercase();
    let qn = format!("{kind_lower}:{chemin}");
    if !qn_inline_safe(&qn) {
        return Err(format!("qn invalide : {qn}"));
    }
    let mut props = serde_json::Map::new();
    // payload = contenu INTÉGRAL (contrat list_docs / skill-graph) + repères.
    props.insert("payload".into(), json!(content.clone()));
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| chemin.clone());
    props.insert("name".into(), json!(name));
    props.insert(
        "file_path".into(),
        json!(path.to_string_lossy().to_string()),
    );
    props.insert("source".into(), json!("import_docs"));
    let id = merge_node(session, kind, &qn, None);
    if id <= 0 {
        return Err(format!("écriture impossible (qn {qn})"));
    }
    set_node_props(session, kind, &qn, &props);
    // BM25 sur le contenu (recherche runtime des skills/plans).
    let _ = db.create_text_index(kind, "payload");
    Ok((kind.to_string(), qn))
}

/// `import_files` — fichiers produits par l'agent → nœuds File (payload =
/// contenu INTÉGRAL, file_path, name) ; arête Session→PRODUCED→File quand
/// `session_id` est fourni et résolu. L'upsert par qn (`file:<chemin>` par
/// défaut, sinon `qn` explicite) est l'ATTACHEMENT du fichier dans le graphe :
/// ré-importer un fichier MODIFIÉ remplace son contenu au lieu de créer un
/// doublon. ADDITIF — aucun fichier supprimé/renommé/déplacé sur disque.
fn import_files(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session = db.session();
    let files = arguments.get("files").and_then(|v| v.as_array());
    let Some(files) = files else {
        return (
            json!({ "status": "error", "error": "import_files: files (array) requis" }),
            true,
        );
    };
    // Session produisant ces fichiers (arête PRODUCED) — même normalisation
    // « session-<uuid> » qu'import_events ; absente ⇒ pas d'arête.
    let session_id = arg_str(arguments, "session_id").unwrap_or_default();
    let session_qn = if session_id.is_empty() {
        String::new()
    } else if session_id.starts_with("session-") {
        session_id
    } else {
        format!("session-{session_id}")
    };
    let session_exists = !session_qn.is_empty() && node_exists(&session, "Session", &session_qn);

    let mut imported = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for f in files {
        let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            skipped.push("chemin vide".to_string());
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                skipped.push(format!("{path} : {e}"));
                continue;
            }
        };
        let qn = f
            .get("qn")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("file:{path}"));
        if !qn_inline_safe(&qn) {
            skipped.push(format!("qn invalide : {qn}"));
            continue;
        }
        let name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        let mut props = serde_json::Map::new();
        props.insert("payload".into(), json!(content.clone()));
        props.insert("name".into(), json!(name));
        props.insert("file_path".into(), json!(path));
        props.insert("source".into(), json!("import_files"));
        let id = merge_node(&session, "File", &qn, None);
        if id <= 0 {
            skipped.push(format!("écriture impossible (qn {qn})"));
            continue;
        }
        set_node_props(&session, "File", &qn, &props);
        // BM25 sur le contenu intégral (recherche runtime des fichiers).
        let _ = db.create_text_index("File", "payload");
        if session_exists {
            link_edge(&session, "Session", &session_qn, "File", &qn, "PRODUCED");
        }
        imported += 1;
    }
    (
        json!({
            "status": "ok",
            "tool": "import_files",
            "imported": imported,
            "skipped": skipped,
            "note": "additif — aucun fichier supprimé",
        }),
        false,
    )
}

/// `import_docs` — skills/plans → nœuds Skill/Plan (payload = contenu complet).
/// Modes : {file, kind: Skill|Plan} fichier unique ; {skills_dir} / {plans_dir}
/// répertoire parcouru RÉCURSIVEMENT (*.md → kind du répertoire). qn =
/// `skill:<chemin>` / `plan:<chemin>` (chemin relatif au root en mode
/// répertoire, extension retirée). Aucune écriture sur disque, graphe seul.
fn import_docs(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let session = db.session();
    let mut jobs: Vec<(String, std::path::PathBuf, Option<std::path::PathBuf>)> = Vec::new();
    if let Some(f) = arg_str(arguments, "file") {
        let kind = arg_str(arguments, "kind").unwrap_or_else(|| "Skill".to_string());
        if kind != "Skill" && kind != "Plan" {
            return (
                json!({ "status": "error", "error": format!("import_docs: kind invalide : {kind} (Skill|Plan)") }),
                true,
            );
        }
        jobs.push((kind, std::path::PathBuf::from(&f), None));
    }
    if let Some(dir) = arg_str(arguments, "skills_dir") {
        jobs.push((
            "Skill".to_string(),
            std::path::PathBuf::from(&dir),
            Some(std::path::PathBuf::from(&dir)),
        ));
    }
    if let Some(dir) = arg_str(arguments, "plans_dir") {
        jobs.push((
            "Plan".to_string(),
            std::path::PathBuf::from(&dir),
            Some(std::path::PathBuf::from(&dir)),
        ));
    }
    if jobs.is_empty() {
        return (
            json!({ "status": "error", "error": "import_docs: file+kind, skills_dir ou plans_dir requis" }),
            true,
        );
    }
    let mut files_imported = 0usize;
    let mut skills = 0usize;
    let mut plans = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut cap_hit = false;
    for (kind, path, root) in &jobs {
        if !path.exists() {
            skipped.push(format!("introuvable : {}", path.display()));
            continue;
        }
        if path.is_dir() {
            let mut files: Vec<std::path::PathBuf> = Vec::new();
            cap_hit |= collect_md_files(path, &mut files, 2000) > 0;
            files.sort();
            for f in files {
                match import_doc_file(db, &session, kind.as_str(), &f, root.as_deref()) {
                    Ok((label, _)) => {
                        files_imported += 1;
                        if label == "Skill" {
                            skills += 1;
                        } else {
                            plans += 1;
                        }
                    }
                    Err(e) => skipped.push(e),
                }
            }
        } else {
            match import_doc_file(db, &session, kind.as_str(), path, None) {
                Ok((label, _)) => {
                    files_imported += 1;
                    if label == "Skill" {
                        skills += 1;
                    } else {
                        plans += 1;
                    }
                }
                Err(e) => skipped.push(e),
            }
        }
    }
    if files_imported == 0 && jobs.iter().all(|(_, p, _)| !p.exists()) {
        return (
            json!({ "status": "error", "error": format!("import_docs: aucun fichier importable — {}", skipped.join(" ; ")) }),
            true,
        );
    }
    (
        json!({
            "status": "ok",
            "tool": "import_docs",
            "files": files_imported,
            "skills": skills,
            "plans": plans,
            "skipped": skipped,
            "note": if cap_hit { "cap 2000 fichiers/répertoire atteint" } else { "ok" },
        }),
        false,
    )
}

/// `import_sessions` — version honnête et BORNÉE de l'import d'archives de
/// session. Importe les fichiers `.jsonl` NON compressés d'un répertoire
/// (parcours récursif) comme événements runtime en déléguant au canal
/// `import_events` (nœuds Session/Message/ToolCall + journal brut + marqueur
/// incrémental last_imported_seq) : l'idempotence par (session, seq) est celle
/// du writer — un ré-import du même fichier n'importe rien et ne ré-appende
/// pas le journal (filtre local seq > last avant délégation).
///
/// LIMITES assumées (vs le moteur C) : `.zstd` NON supporté ici (erreur
/// explicite — décompressez d'abord) ; un fichier = une session (l'id vient de
/// `session_id` d'une ligne, sinon de l'en-tête type:"session" id, sinon du nom
/// de fichier) ; lots de ≤ 2000 événements, 200 000 événements max par fichier,
/// 1 000 000 au total — au-delà, troncature notée, pas d'échec.
fn import_sessions(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    const MAX_PER_FILE: usize = 200_000;
    const MAX_TOTAL: usize = 1_000_000;
    const CHUNK: usize = 2000;
    let Some(dir) = arg_str(arguments, "dir") else {
        return (
            json!({ "status": "error", "error": "import_sessions: dir requis" }),
            true,
        );
    };
    let root = std::path::Path::new(&dir);
    if !root.is_dir() {
        return (
            json!({ "status": "error", "error": format!("import_sessions: répertoire introuvable : {dir}") }),
            true,
        );
    }
    // Balayage : *.jsonl (importables) et *.jsonl.zstd / *.zstd → REFUS explicite.
    let mut jsonl: Vec<std::path::PathBuf> = Vec::new();
    let mut zstd: Vec<std::path::PathBuf> = Vec::new();
    {
        let mut pending: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
        while let Some(d) = pending.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            let mut entries: Vec<std::path::PathBuf> = Vec::new();
            for e in rd.flatten() {
                entries.push(e.path());
            }
            entries.sort();
            for p in entries {
                let ft = match std::fs::metadata(&p) {
                    Ok(m) => m.file_type(),
                    Err(_) => continue,
                };
                if ft.is_dir() {
                    pending.push(p);
                } else if ft.is_file() {
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    if name.ends_with(".jsonl.zstd") || name.ends_with(".zstd") {
                        zstd.push(p);
                    } else if name.ends_with(".jsonl") {
                        jsonl.push(p);
                    }
                }
            }
        }
    }
    jsonl.sort();
    if !zstd.is_empty() {
        return (
            json!({
                "status": "error",
                "error": format!(
                    "import_sessions: compression non supportée ici — décompressez d'abord ({} fichier(s) .zstd trouvé(s), ex. {})",
                    zstd.len(),
                    zstd[0].display()
                ),
            }),
            true,
        );
    }
    if jsonl.is_empty() {
        return (
            json!({ "status": "ok", "tool": "import_sessions", "dir": dir, "files": 0, "sessions": 0, "events": 0, "imported": 0, "skipped_lines": 0, "note": "aucun fichier .jsonl" }),
            false,
        );
    }
    let session = db.session();
    let mut sessions_seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut total_events = 0usize;
    let mut imported = 0usize;
    let mut skipped_lines = 0usize;
    let mut truncated = false;
    let mut errored: Option<String> = None;
    'files: for file in &jsonl {
        if total_events >= MAX_TOTAL {
            truncated = true;
            break;
        }
        let raw = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => {
                errored = Some(format!("{} : {e}", file.display()));
                continue;
            }
        };
        // id de session du fichier : session_id d'une ligne, sinon en-tête
        // type:"session" (id), sinon nom de fichier (stem).
        let mut sid: Option<String> = None;
        let mut events: Vec<Value> = Vec::new();
        for line in raw.lines() {
            if total_events >= MAX_TOTAL {
                truncated = true;
                break 'files;
            }
            if events.len() >= MAX_PER_FILE {
                truncated = true;
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(line) else {
                skipped_lines += 1;
                continue;
            };
            if !ev.is_object() {
                skipped_lines += 1;
                continue;
            }
            if sid.is_none() {
                if let Some(s) = ev.get("session_id").and_then(|v| v.as_str()) {
                    sid = Some(s.to_string());
                } else if ev.get("type").and_then(|v| v.as_str()) == Some("session") {
                    if let Some(s) = ev.get("id").and_then(|v| v.as_str()) {
                        sid = Some(s.to_string());
                    }
                }
            }
            events.push(ev);
        }
        let sid = match sid {
            Some(s) => s,
            None => file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "session".to_string()),
        };
        let sqn = session_qn_of(&sid);
        // Idempotence locale : ne déléguer que les événements nouveaux
        // (seq > last) — le journal brut n'est alors jamais ré-appendu.
        let last = session_last_seq(&session, &sqn);
        let fresh: Vec<Value> = events
            .iter()
            .filter(|ev| {
                let seq = ev
                    .get("seq")
                    .and_then(|s| s.as_i64())
                    .or_else(|| ev.get("seq0").and_then(|s| s.as_i64()))
                    .unwrap_or(-1);
                seq > last
            })
            .cloned()
            .collect();
        total_events += events.len();
        for chunk in fresh.chunks(CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let resp = import_events(db, &json!({ "session_id": sqn.clone(), "events": chunk }));
            if resp.1 {
                errored = Some(
                    resp.0["error"]
                        .as_str()
                        .unwrap_or("import_events")
                        .to_string(),
                );
                continue 'files;
            }
            imported += resp.0["imported"].as_u64().unwrap_or(0) as usize;
        }
        sessions_seen.insert(sqn);
    }
    if let Some(e) = errored {
        return (
            json!({ "status": "error", "error": format!("import_sessions: {e}") }),
            true,
        );
    }
    (
        json!({
            "status": "ok",
            "tool": "import_sessions",
            "dir": dir,
            "files": jsonl.len(),
            "sessions": sessions_seen.len(),
            "events": total_events,
            "imported": imported,
            "skipped_lines": skipped_lines,
            "truncated": truncated,
            "note": "jsonl uniquement — .zstd refusé (décompressez d'abord) ; idempotent par (session, seq)",
        }),
        false,
    )
}

/// `manage_adr` — ADR comme NŒUD GRAPHE (label ADR, qn `adr:<project>/<path>`,
/// prop content) : AUCUN fichier sur disque (divergence assumée vs le C qui
/// écrivait un fichier adr.md / SQLite — ici le graphe EST l'autorité, et
/// payload_get/list_docs le relisent). modes : get (défaut) → content ;
/// update (alias store) → remplace content ; sections → titres `## `.
fn manage_adr(db: &ObrainDB, arguments: &Value) -> (Value, bool) {
    let mode = arg_str(arguments, "mode").unwrap_or_else(|| "get".to_string());
    let project = arg_str(arguments, "project").unwrap_or_else(|| "default".to_string());
    let Some(path) = arg_str(arguments, "path").or_else(|| arg_str(arguments, "id")) else {
        return (
            json!({ "status": "error", "error": "manage_adr: path (ou id) requis" }),
            true,
        );
    };
    let qn = format!("adr:{project}/{path}");
    if !qn_inline_safe(&qn) {
        return (
            json!({ "status": "error", "error": format!("manage_adr: qn invalide : {qn}") }),
            true,
        );
    }
    let session = db.session();
    match mode.as_str() {
        "update" | "store" => {
            let Some(content) = arg_str(arguments, "content") else {
                return (
                    json!({ "status": "error", "error": "manage_adr: content requis (mode update)" }),
                    true,
                );
            };
            let id = merge_node(&session, "ADR", &qn, None);
            if id <= 0 {
                return (
                    json!({ "status": "error", "error": "manage_adr: écriture impossible (store en lecture seule ?)" }),
                    true,
                );
            }
            let mut props = serde_json::Map::new();
            props.insert("content".into(), json!(content));
            props.insert("project".into(), json!(project));
            props.insert("path".into(), json!(path));
            props.insert(
                "updatedAt".into(),
                json!(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0)
                ),
            );
            set_node_props(&session, "ADR", &qn, &props);
            (
                json!({
                    "status": "ok",
                    "tool": "manage_adr",
                    "mode": "update",
                    "qn": qn,
                    "path": path,
                }),
                false,
            )
        }
        "get" => match node_prop_str(&session, "ADR", &qn, "content") {
            Some(content) => (
                json!({ "status": "ok", "tool": "manage_adr", "mode": "get", "qn": qn, "content": content }),
                false,
            ),
            None => (
                json!({ "status": "error", "error": format!("manage_adr: ADR introuvable : {qn}") }),
                true,
            ),
        },
        "sections" => {
            let Some(content) = node_prop_str(&session, "ADR", &qn, "content") else {
                return (
                    json!({ "status": "error", "error": format!("manage_adr: ADR introuvable : {qn}") }),
                    true,
                );
            };
            let mut sections: Vec<Value> = Vec::new();
            for line in content.lines() {
                let t = line.trim_start();
                let Some(rest) = t.strip_prefix("## ") else {
                    continue;
                };
                sections.push(json!(rest.trim().to_string()));
            }
            (
                json!({ "status": "ok", "tool": "manage_adr", "mode": "sections", "qn": qn, "sections": sections }),
                false,
            )
        }
        other => (
            json!({ "status": "error", "error": format!("manage_adr: mode inconnu : {other} (get|update|sections)") }),
            true,
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests de non-régression de la surface MCP bridge (exécution :
//   cargo test -p obrain-cli --features cypher,indexer --bin obrain -- mcp_server)
// Chaque outil porté est vérifié sur un store substrate jetable réel.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::Value as OV;
    use obrain_engine::ObrainDB;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// ── UNE SURFACE D'INTERROGATION NE CHOISIT PAS DE CONVENTION ────────────
    ///
    /// Le dépôt en porte deux : `qn` (mémoire, registre des machines) et
    /// `qualified_name` (graphe de code). `payload_get` ne cherchait que la
    /// seconde alors que son ENTRÉE s'appelle `qn` : les nœuds écrits par la
    /// mémoire et par la passe des machines lui étaient invisibles. Mesuré le
    /// 2026-09-17, sur `machine:n3` et les `signal:…` produits par `import_events`.
    #[test]
    fn payload_get_trouve_les_deux_conventions_d_identite() {
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-ident-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();
        db.create_property_index("qualified_name");
        db.create_property_index("qn");

        // 1. Un nœud de la MÉMOIRE : `qn` seul (c'est ce qu'écrit la passe des
        //    machines et l'import des sessions).
        let memoire = db.create_node(&["Machine"]);
        db.set_node_property(memoire, "qn", OV::from("machine:n3".to_string()));
        db.set_node_property(memoire, "hote", OV::from("IP-INTERNE".to_string()));
        // 2. Un nœud du GRAPHE DE CODE : `qualified_name` seul.
        let code = db.create_node(&["Function"]);
        db.set_node_property(code, "qualified_name", OV::from("test.repo.f".to_string()));

        // Les DEUX doivent être trouvables — c'est tout l'objet de la correction.
        for qn in ["machine:n3", "test.repo.f"] {
            let p = ok_status(payload_get(&db, &json!({ "qn": qn })));
            assert_eq!(p["status"], "ok", "« {qn} » doit être trouvable : {p}");
            assert_eq!(p["qualified_name"], json!(qn), "{p}");
        }
        // Et la propriété lue est bien celle du nœud, pas celle de l'autre.
        let p = ok_status(payload_get(&db, &json!({ "qn": "machine:n3" })));
        assert_eq!(p["payload"]["hote"], json!("IP-INTERNE"), "{p}");

        let _ = std::fs::remove_dir_all(dir);
    }

    /// ── LES DEUX CONVENTIONS D'IDENTITÉ SONT LUES **ET** ÉCRITES ────────────
    ///
    /// Défaut mesuré le 2026-09-17 : `graph_link` annonçait « identifiés par
    /// qualified_name » et ne LISAIT que `qn` — un appelant qui suivait la
    /// description recevait « qn manquant », et le nœud écrit ne portait que
    /// `qualified_name`, donc restait invisible au lecteur qui cherche `qn`
    /// (mémoire, registre des machines). `machines.rs` écrit les deux pour cette
    /// raison exacte. Ce test tient les deux moitiés : accepter les deux clés, et
    /// poser les deux propriétés — y compris en complétant un nœud ancien, sans
    /// jamais écraser une valeur présente.
    #[test]
    fn graph_link_accepte_et_ecrit_les_deux_conventions_d_identite() {
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-qn-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();
        db.create_property_index("qualified_name");
        db.create_property_index("qn");

        // 1. La clé de la DESCRIPTION (`qualified_name`) doit être acceptée.
        let (c, err) = call_tool(
            &db,
            "graph_link",
            &json!({
                "source": { "qualified_name": "machine:n3", "label": "Machine", "name": "n3" },
                "target": { "qn": "service:n3:viz", "label": "Service", "name": "viz" },
                "type": "HEBERGE"
            }),
        );
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");

        // 2. Chaque nœud porte les DEUX propriétés.
        for qn in ["machine:n3", "service:n3:viz"] {
            for cle in ["qn", "qualified_name"] {
                let (r, _) = call_tool(
                    &db,
                    "query",
                    &json!({ "query": format!("MATCH (n) WHERE n.{cle} = '{qn}' RETURN count(n)") }),
                );
                let n = r["rows"][0][0].as_i64().unwrap_or(-1);
                assert_eq!(n, 1, "« {qn} » doit être trouvable par `{cle}` : {r}");
            }
        }

        // 3. Les deux conventions désignent le MÊME nœud : rejouer avec l'autre
        //    clé ne crée pas de doublon (c'est tout l'intérêt d'une seule identité).
        let (c2, err2) = call_tool(
            &db,
            "graph_link",
            &json!({
                "source": { "qn": "machine:n3" },
                "target": { "qn": "service:n3:viz" },
                "type": "HEBERGE"
            }),
        );
        assert!(!err2, "{c2}");
        let (r, _) = call_tool(
            &db,
            "query",
            &json!({ "query": "MATCH (n) RETURN count(n)" }),
        );
        assert_eq!(
            r["rows"][0][0].as_i64(),
            Some(2),
            "aucun doublon attendu : {r}"
        );

        // 4. Un nœud ANCIEN (une seule clé) est complété, sans écraser l'existante.
        let vieux = db.create_node(&["Machine"]);
        db.set_node_property(vieux, "qn", OV::from("machine:ancienne".to_string()));
        let (c3, err3) = call_tool(
            &db,
            "graph_link",
            &json!({
                "source": { "qn": "machine:ancienne" },
                "target": { "qn": "service:n3:viz" },
                "type": "HEBERGE"
            }),
        );
        assert!(!err3, "{c3}");
        let (r3, _) = call_tool(
            &db,
            "query",
            &json!({ "query": "MATCH (n) WHERE n.qn = 'machine:ancienne' RETURN n.qualified_name" }),
        );
        assert_eq!(
            r3["rows"][0][0],
            json!("machine:ancienne"),
            "la clé manquante doit être complétée : {r3}"
        );

        // 5. Une valeur PRÉSENTE n'est pas réécrite : on ne répare pas en écrasant.
        let (c4, _) = call_tool(
            &db,
            "graph_link",
            &json!({
                "source": { "qn": "machine:ancienne", "properties": {} },
                "target": { "qn": "service:n3:viz" },
                "type": "HEBERGE"
            }),
        );
        assert_eq!(c4["status"], "ok", "{c4}");
        let (r4, _) = call_tool(
            &db,
            "query",
            &json!({ "query": "MATCH (n) WHERE n.qn = 'machine:ancienne' RETURN count(n)" }),
        );
        assert_eq!(
            r4["rows"][0][0].as_i64(),
            Some(1),
            "toujours un seul nœud : {r4}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// next_step_hint : résultat vide → orientation vers la découverte du
    /// schéma ; résultat non vide → outil suivant ; outil hors périmètre → None.
    #[test]
    fn next_step_hint_oriente_selon_le_resultat() {
        let vide = json!({ "status": "ok", "rows": [], "total": 0 });
        let h = next_step_hint("query", &vide).expect("hint attendu sur résultat vide");
        assert!(h.contains("get_graph_schema"), "{h}");

        let plein = json!({ "status": "ok", "rows": [[1]], "total": 1 });
        let h = next_step_hint("search_graph", &plein).expect("hint attendu sur succès");
        assert!(h.contains("get_code_snippet"), "{h}");

        // Outils hors périmètre (écriture/imports) : aucun hint.
        assert!(next_step_hint("graph_link", &plein).is_none());
        assert!(next_step_hint("import_events", &plein).is_none());
    }

    /// response_is_empty : `results` vide compte, objet sans champ connu non.
    #[test]
    fn response_is_empty_ne_devine_pas() {
        assert!(response_is_empty(&json!({ "results": [], "total": 0 })));
        assert!(!response_is_empty(&json!({ "results": [{"qn": "a"}] })));
        assert!(!response_is_empty(&json!({ "labels": [] })));
    }

    /// ── LES TESTS DU PONT VEULENT UN ENVIRONNEMENT SANS IAM ─────────────────
    ///
    /// Les outils de pont interrogent l'API moteur, et depuis que le pont VÉRIFIE
    /// les identités, cette API exige un jeton quand `OBRAIN_IAM` est posé. Or
    /// `std::env` est HÉRITÉ : dans une session qui exporte `OBRAIN_IAM=1` (c'est
    /// le cas de la session de développement de cette machine), les onze tests de
    /// pont recevaient 401 « IAM ARMÉ » et mesuraient donc l'ENVIRONNEMENT au lieu
    /// du code — constaté : `cargo test -p obrain-cli` échoue sur 11 tests, et le
    /// même `cargo test` sous `env -u OBRAIN_IAM -u OBRAIN_IAM_STORE` passe (299
    /// tests). C'est la même famille de défaut que celle corrigée dans
    /// `tests/remote_memoire.rs`, où le serveur censé être nu était armé.
    ///
    /// On retire donc explicitement ce qu'on ne veut pas, comme le fait déjà
    /// `metrics_logger_jsonl_quand_env_absent_rien` pour sa propre variable. Ce
    /// n'est pas un confort : un test qui dépend de l'environnement de celui qui
    /// le lance n'est pas un test.
    #[allow(unsafe_code)]
    fn sans_iam_dans_l_environnement() {
        // SAFETY : les tests de ce module s'exécutent dans le processus de test,
        // et cette fonction est appelée au tout début de chacun d'eux, avant toute
        // lecture d'environnement. Le crate interdit `unsafe` globalement ; il est
        // autorisé ici et SEULEMENT ici pour retirer des variables héritées.
        unsafe {
            for v in [
                "OBRAIN_IAM",
                "OBRAIN_IAM_STORE",
                "OBRAIN_IAM_ACCOUNT",
                "OBRAIN_IAM_TOKEN",
            ] {
                std::env::remove_var(v);
            }
        }
    }

    /// Ouvre un store substrate jetable dans temp_dir et y insère un mini graphe
    /// (2 Function reliées par CALLS + props qualified_name/name/file_path/lignes).
    /// ── UNE RÉPONSE DÉMESURÉE EST REFUSÉE, PAS STREAMÉE ────────────────────
    ///
    /// Mesuré le 2026-09-17 : un pont a streamé 234 Mo en dix minutes (134
    /// écritures/s, 83 % d'un cœur) sans un mot de journal, pendant que le
    /// harness consommait le flux. Le plafond transforme cet emballement muet en
    /// un refus qui NOMME l'outil, la taille et la limite — et qui reprend l'`id`
    /// de la requête, pour que l'appelant reçoive une réponse à SA question.
    #[test]
    fn une_reponse_demesuree_devient_un_refus_nomme() {
        let plafond = 1024;

        // Sous le plafond : inchangée, au caractère près.
        let petite = json!({"jsonrpc": "2.0", "id": 7, "result": {"rows": []}});
        let inchangee = reponse_bornee(petite.clone(), "query", Some(json!(7)), plafond);
        assert_eq!(
            inchangee, petite,
            "une réponse normale ne doit pas être touchée"
        );

        // Au-dessus : un refus explicite, qui dit QUI, COMBIEN, et JUSQU'OÙ.
        let enorme = json!({"jsonrpc": "2.0", "id": 9, "result": {"blob": "x".repeat(4096)}});
        let refus = reponse_bornee(enorme, "runtime_search", Some(json!(9)), plafond);
        assert_eq!(refus["id"], json!(9), "le refus répond à la MÊME requête");
        let message = refus["error"]["message"].as_str().expect("message");
        assert!(
            message.contains("runtime_search"),
            "il doit nommer l'outil : {message}"
        );
        assert!(
            message.contains("1024"),
            "il doit donner le plafond : {message}"
        );
        assert!(
            message.contains("LIMIT"),
            "il doit orienter le correctif : {message}"
        );
    }

    /// ── LE VEILLEUR DE REQUÊTE : NOMMER CE QUI NE REND PAS LA MAIN ─────────
    ///
    /// Un pont a tourné quatre minutes à 90 % de CPU sans écrire une ligne de
    /// journal (mesuré le 2026-09-17). Une échéance existe (30 s par défaut)
    /// mais un chemin qui ne la consulte pas passe dessous : le veilleur ne
    /// remplace pas l'échéance, il NOMME le coupable — sans quoi on ne peut pas
    /// corriger ce qu'on ne voit pas.
    #[test]
    fn le_veilleur_signale_une_requete_qui_ne_rend_pas_la_main() {
        let seuil = std::time::Duration::from_secs(10);

        // Rien en cours : rien à dire.
        assert!(alerte_requete(None, seuil).is_none());

        // En cours mais sous le seuil : rien à dire non plus (sinon le journal
        // crierait sur toutes les requêtes normales, et plus personne ne lit).
        assert!(
            alerte_requete(Some(("query", std::time::Duration::from_secs(3))), seuil).is_none()
        );

        // Au-delà du seuil : le message NOMME l'outil et donne l'âge.
        let message = alerte_requete(
            Some(("context_build", std::time::Duration::from_secs(41))),
            seuil,
        )
        .expect("au-delà du seuil, le veilleur doit parler");
        assert!(
            message.contains("context_build"),
            "le message doit nommer l'outil : {message}"
        );
        assert!(
            message.contains("41.0"),
            "le message doit donner l'âge : {message}"
        );
    }

    /// Le nom de l'outil se lit dans la ligne JSON-RPC, quelle que soit sa forme.
    #[test]
    fn le_nom_de_la_requete_se_lit_dans_la_ligne() {
        assert_eq!(
            nom_requete(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"context_build","arguments":{}}}"#
            ),
            "context_build"
        );
        assert_eq!(
            nom_requete(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#),
            "initialize"
        );
        // Une ligne illisible ne doit pas faire tomber le veilleur.
        assert_eq!(nom_requete("pas du json"), "?");
    }

    /// ── L'ÉTAPE DE TISSAGE REFUSE QUAND LE BUDGET EST DÉPASSÉ ──────────────
    ///
    /// Le pont arme une échéance pour TOUT appel d'outil (voir `run`) ; les
    /// étapes longues la consultent (`verifier_echeance`). Ce test tient le
    /// maillon qui manquait : sans lui, l'échéance était armée pour la forme et
    /// un travail parti ne s'arrêtait plus.
    #[test]
    fn le_tissage_refuse_quand_le_budget_est_depasse() {
        // Sans échéance : rien à refuser.
        assert!(verifier_echeance("témoin").is_ok());

        // Échéance déjà expirée : refus EXPLICITE, qui nomme l'étape.
        let _portee = obrain_common::utils::cancel::frontiere(
            Some(false),
            Some(std::time::Instant::now() - std::time::Duration::from_secs(1)),
        );
        let refus = verifier_echeance("consolidation (avant lecture de la fenêtre)")
            .expect_err("une échéance dépassée doit refuser");
        assert!(
            refus.contains("budget") && refus.contains("consolidation"),
            "le refus doit nommer le budget ET l'étape : {refus}"
        );
    }

    /// ── UNE SORTIE CASSÉE ARRÊTE LE PONT, ELLE NE L'ALIMENTE PAS ──────────
    ///
    /// Défaut mesuré le 2026-09-17 sur un pont vivant : 90 % d'un cœur brûlé, 2,3
    /// Go de RSS, et `let _ = writeln!(stdout, …)` qui avalait CHAQUE erreur
    /// d'écriture. Un `Err` ne dit qu'une chose : le correspondant est parti.
    /// Continuer la boucle revient à empiler des réponses que personne ne lira en
    /// tenant le verrou du store — c'est-à-dire à ne jamais rendre la main.
    #[test]
    fn une_sortie_cassee_remonte_au_lieu_d_etre_avalee() {
        /// Un `Write` qui refuse tout, comme une socket fermée.
        struct SortieCassee;
        impl std::io::Write for SortieCassee {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "le correspondant est parti",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let erreur = ecrire_reponse(&mut SortieCassee, &json!({"ok": true}))
            .expect_err("une sortie cassée DOIT remonter");
        assert_eq!(erreur.kind(), std::io::ErrorKind::BrokenPipe);

        // Et le cas nominal écrit bien la ligne, terminée par un saut de ligne.
        let mut tampon: Vec<u8> = Vec::new();
        ecrire_reponse(&mut tampon, &json!({"ok": true})).expect("écriture nominale");
        let texte = String::from_utf8(tampon).expect("utf8");
        assert_eq!(texte, "{\"ok\":true}\n");
    }

    /// ── LE BUDGET EN JETONS DE L'INJECTION (obrain-rag) ───────────────────
    ///
    /// Ce que ce test tient : (a) le payload tient sous le budget ESTIMÉ par le
    /// budgeter du crate, (b) ce qui a été retiré SE DIT, (c) la garantie « au
    /// moins un élément » est respectée — une injection vide ferait croire à une
    /// mémoire vide.
    #[test]
    fn le_budget_de_jetons_borne_l_injection_et_le_dit() {
        // Un payload volontairement énorme : 8 sessions × 3 000 caractères.
        let gros = "x".repeat(3_000);
        let weave: Vec<serde_json::Value> = (0..8)
            .map(|i| json!({ "id": i.to_string(), "title": gros }))
            .collect();
        let mut payload = json!({
            "status": "ok",
            "goals": [{ "content": gros }],
            "summaries": [{ "seq": 1, "text": gros }],
            "weave": weave,
            "engrams": [],
        });
        let config = obrain_rag::RagConfig::default();
        borner_injection_par_jetons(&mut payload);

        let troncature = payload
            .get("troncature")
            .expect("le budget a mordu : la coupe doit SE DIRE");
        let apres = troncature
            .get("jetons_apres")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        assert!(
            apres <= config.token_budget,
            "le payload doit tenir sous {} jetons (obtenu {apres})",
            config.token_budget
        );
        assert!(
            troncature.get("retire").is_some(),
            "le compte rendu doit nommer ce qui a été retiré : {troncature}"
        );
        // (c) La garantie d'injection non vide.
        let total = payload
            .get("weave")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len)
            + payload
                .get("goals")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);
        assert!(total > 0, "au moins un élément doit rester : {payload}");
    }

    /// ── LA GARANTIE « AU MOINS UN ÉLÉMENT » PRIME SUR LE PLAFOND ────────────
    ///
    /// Quand le PREMIER élément dépasse à lui seul le budget, la sélection du
    /// crate en garde un — c'est sa règle documentée (« take at least the first
    /// node even if it exceeds the budget slightly »), et elle est ici
    /// ESSENTIELLE : une injection vidée ferait croire à une mémoire vide, ce
    /// qui est une réponse fausse, alors qu'un extrait trop gros est seulement
    /// une réponse chère.
    ///
    /// Ce test est le seul à pouvoir échouer si cette garantie disparaît : dans
    /// un payload ordinaire, le premier élément tient dans le budget et la
    /// branche n'est jamais prise.
    #[test]
    fn la_garantie_d_un_element_prime_sur_le_plafond() {
        // ~20 000 caractères ≈ 5 000 jetons pour un budget de 2 000.
        let enorme = "y".repeat(20_000);
        let mut payload = json!({
            "status": "ok",
            "goals": [{ "content": enorme }],
            "summaries": [],
            "weave": [],
            "engrams": [],
        });
        borner_injection_par_jetons(&mut payload);
        let goals = payload
            .get("goals")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(
            goals, 1,
            "l'injection ne doit JAMAIS être vidée : {payload}"
        );
    }

    /// Un payload PETIT ne doit pas être touché — ni amputé, ni marqué.
    #[test]
    fn un_petit_payload_n_est_pas_touche_par_le_budget() {
        let mut payload = json!({
            "status": "ok",
            "goals": [{ "content": "un objectif court" }],
            "summaries": [],
            "weave": [{ "id": "1", "title": "une session" }],
            "engrams": [],
        });
        let avant = payload.clone();
        borner_injection_par_jetons(&mut payload);
        assert_eq!(payload, avant, "rien à couper : le payload reste identique");
        assert!(payload.get("troncature").is_none());
    }

    fn make_db() -> (ObrainDB, String, std::path::PathBuf) {
        // ── RÉUNIFICATION (2026-10-06) : LES DEUX DÉSARMEMENTS SONT GARDÉS ──
        // `sans_iam_dans_l_environnement()` (lignée servie) retire les variables
        // d'environnement ; `GardeIam::desarmer_pour_les_tests()` (multi-
        // écrivain) pose la garde INERTE. Ce sont deux moitiés du même isolement
        // — l'une nettoie l'entrée, l'autre fixe l'état déjà lu — et un test doit
        // dépendre du CODE, pas de l'environnement de celui qui le lance.
        sans_iam_dans_l_environnement();
        // ── LA PORTE IAM EST DÉSARMÉE POUR LES TESTS ────────────────────────
        // Sans cette ligne, ces tests lisaient `OBRAIN_IAM_STORE` et
        // `OBRAIN_IAM_TOKEN` de la machine qui les exécute : armés, ils
        // recevaient `401` et échouaient tous les neuf ; désarmés, ils
        // passaient. Un test doit dépendre du CODE, pas de l'environnement de
        // celui qui le lance.
        crate::viz_serve::GardeIam::desarmer_pour_les_tests();
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("t.obrain");
        let db = ObrainDB::open_substrate(store.to_str().unwrap()).expect("open substrate");
        let a = db.create_node(&["Function"]);
        let _ = db.set_node_property(a, "qualified_name", OV::from("test.repo.a".to_string()));
        let _ = db.set_node_property(a, "name", OV::from("fn_a".to_string()));
        let b = db.create_node(&["Function"]);
        let _ = db.set_node_property(b, "qualified_name", OV::from("test.repo.b".to_string()));
        let _ = db.set_node_property(b, "name", OV::from("fn_b".to_string()));
        let _e = db.create_edge(a, b, "CALLS");
        db.create_property_index("qualified_name");
        let _ = db.checkpoint();
        (db, "test.repo".to_string(), dir)
    }

    fn ok_status(res: (Value, bool)) -> Value {
        assert!(!res.1, "is_error true : {res:?}");
        let v = res.0;
        assert_eq!(v["status"], "ok", "status != ok : {v}");
        v
    }

    /// Écrit un fichier fixture (créant les répertoires parents) sous le dir du
    /// store jetable ; retourne le chemin absolu (string).
    fn write_fixture(dir: &std::path::Path, rel: &str, content: &str) -> String {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p.to_str().unwrap().to_string()
    }

    /// Compte des arêtes d'un type entre deux qn (0 si aucune).
    fn count_edges(db: &ObrainDB, q: &str) -> i64 {
        db.session()
            .execute(q)
            .ok()
            .and_then(|r| {
                r.rows
                    .first()
                    .and_then(|row| row.first())
                    .and_then(|v| v.as_int64())
            })
            .unwrap_or(0)
    }

    #[test]
    fn catalogue_contient_les_outils_portes() {
        let names: Vec<String> = tools_list()
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();
        for must in [
            "query",
            "query_graph",
            "get_graph_schema",
            "get_code_snippet",
            "trace_path",
            "structural_roles",
            "list_projects",
            "search_code",
            "index_status",
            "payload_get",
            "list_docs",
            "ingest_traces",
            "impact_set",
            "graph_link",
            "index_repository",
            "create_property_index",
            "import_owners",
            "import_workspace",
            "import_docs",
            "import_sessions",
            "manage_adr",
            "viz_subgraph",
            "viz_export",
            "node_curvature",
            "viz_topo",
        ] {
            assert!(names.iter().any(|n| n == must), "outil manquant : {must}");
        }
    }

    #[test]
    fn get_graph_schema_expose_labels_et_property_keys() {
        let (db, _, dir) = make_db();
        let v = ok_status(get_graph_schema(&db, &json!({})));
        assert!(
            v["labels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l == "Function")
        );
        assert!(
            v["edgeTypes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "CALLS")
        );
        assert!(
            v["propertyKeys"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k == "qualified_name")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn query_graph_cypher_compte_les_fonctions() {
        let (db, _, dir) = make_db();
        let v = ok_status(query_graph(
            &db,
            &json!({"query": "MATCH (n:Function) RETURN count(n) AS f"}),
        ));
        assert_eq!(v["rows"][0][0], json!(2));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn payload_get_et_index_status_sur_le_store() {
        let (db, _, dir) = make_db();
        let v = ok_status(payload_get(&db, &json!({"qn": "test.repo.a"})));
        assert_eq!(v["payload"]["name"], "fn_a");
        let s = ok_status(index_status(&db, &json!({})));
        assert_eq!(s["total_nodes"], 2);
        assert!(
            s["labels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["label"] == "Function")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn trace_path_et_impact_set_trouvent_les_appelants() {
        let (db, _, dir) = make_db();
        let t = ok_status(trace_path(
            &db,
            &json!({"qualified_name": "test.repo.b", "direction": "inbound", "depth": 1}),
        ));
        assert!(
            t["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r[0] == "test.repo.a")
        );
        let i = ok_status(impact_set(
            &db,
            &json!({"node": "test.repo.b", "direction": "inbound", "depth": 1}),
        ));
        assert!(i["total"].as_u64().unwrap() >= 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recherche_qualified_name_indexee_en_lookup_gql() {
        let (db, _, dir) = make_db();
        let v = ok_status(execute_query(
            &db,
            &json!({"query": "MATCH (n:Function) WHERE n.qualified_name = 'test.repo.a' RETURN n.qualified_name", "language": "gql"}),
        ));
        assert_eq!(v["rows"][0][0], "test.repo.a");
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Lot C : import_owners / import_workspace / import_docs /
    //    import_sessions / manage_adr (P1, ingestion) ───────────────────────

    #[test]
    fn import_owners_cree_comptes_et_arretes_owned_by() {
        let (db, _, dir) = make_db();
        let reg = write_fixture(
            &dir,
            "owners.json",
            r#"{"session-t1": "alice", "t2": "bob"}"#,
        );
        let v = ok_status(import_owners(&db, &json!({"registry": reg})));
        assert_eq!(v["accounts"], 2, "{v}");
        assert_eq!(v["created_accounts"], 2, "{v}");
        assert_eq!(v["created_sessions"], 2, "{v}"); // session-t1 + session-t2 (normalisé)
        assert_eq!(v["links"], 2, "{v}");
        let session = db.session();
        assert!(node_exists(&session, "Account", "account:alice"));
        assert!(node_exists(&session, "Account", "account:bob"));
        assert!(node_exists(&session, "Session", "session-t1"));
        assert!(node_exists(&session, "Session", "session-t2"));
        let c = count_edges(
            &db,
            "MATCH (a:Session {qualified_name: 'session-t1'})-[:OWNED_BY]->(b:Account {qualified_name: 'account:alice'}) RETURN count(a)",
        );
        assert_eq!(c, 1);
        // Idempotence : un second import ne crée plus rien (MERGE + MERGE).
        let v2 = ok_status(import_owners(&db, &json!({"registry": reg})));
        assert_eq!(v2["created_accounts"], 0, "{v2}");
        assert_eq!(v2["created_sessions"], 0, "{v2}");
        assert_eq!(v2["accounts"], 2, "{v2}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_workspace_cree_workspaces_et_arretes_work() {
        let (db, _, dir) = make_db();
        let reg = write_fixture(
            &dir,
            "workspaces.json",
            r#"{
              "w1": {"path": "/tmp/proj-x", "title": "Projet X", "sessionIds": ["session-t1", "t2"]},
              "w2": {"path": "/tmp/proj-y", "title": "Projet Y", "sessionIds": []}
            }"#,
        );
        let v = ok_status(import_workspace(&db, &json!({"registry": reg})));
        assert_eq!(v["workspaces"], 2, "{v}");
        assert_eq!(v["sessions"], 2, "{v}");
        assert_eq!(v["links"], 2, "{v}");
        let session = db.session();
        assert!(node_exists(&session, "Workspace", "workspace:w1"));
        assert!(node_exists(&session, "Workspace", "workspace:w2"));
        assert!(node_exists(&session, "Session", "session-t2"));
        let q = "MATCH (w:Workspace {qualified_name: 'workspace:w1'}) RETURN w.path, w.title";
        let row = session.execute(q).unwrap().rows.into_iter().next().unwrap();
        assert_eq!(row[0].to_string(), "/tmp/proj-x");
        assert_eq!(row[1].to_string(), "Projet X");
        let c = count_edges(
            &db,
            "MATCH (s:Session {qualified_name: 'session-t1'})-[:WORKSPACE]->(w:Workspace {qualified_name: 'workspace:w1'}) RETURN count(s)",
        );
        assert_eq!(c, 1);
        // Variante enveloppée tables.workspaces (format registre du harness).
        let reg2 = write_fixture(
            &dir,
            "workspaces-nested.json",
            r#"{"tables": {"workspaces": {"w9": {"path": "/n", "title": "N", "sessionIds": []}}}}"#,
        );
        let v2 = ok_status(import_workspace(&db, &json!({"registry": reg2})));
        assert_eq!(v2["workspaces"], 1, "{v2}");
        assert!(node_exists(&session, "Workspace", "workspace:w9"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_docs_importe_skills_plans_payload() {
        let (db, _, dir) = make_db();
        // Mode fichier unique.
        let skill_md = "# Skill A\n\n## Usage\nCorps du skill.\n";
        let skill_file = write_fixture(&dir, "docs/skill-a/SKILL.md", skill_md);
        let v = ok_status(import_docs(
            &db,
            &json!({"file": skill_file.clone(), "kind": "Skill"}),
        ));
        assert_eq!(v["files"], 1, "{v}");
        assert_eq!(v["skills"], 1, "{v}");
        let qn = format!("skill:{}", skill_file.trim_end_matches(".md"));
        let p = ok_status(payload_get(&db, &json!({"qn": qn})));
        assert_eq!(p["payload"]["payload"], skill_md);
        // Mode répertoires : skills_dir (récursif) + plans_dir.
        let skills_root = dir.join("skills");
        std::fs::create_dir_all(skills_root.join("skill-b")).unwrap();
        let skill_b = "# Skill B\nContenu B\n";
        std::fs::write(skills_root.join("skill-b/SKILL.md"), skill_b).unwrap();
        let plans_root = dir.join("plans");
        std::fs::create_dir_all(&plans_root).unwrap();
        let plan_md = "# Plan route\n\n## Étapes\n1. Une étape\n";
        std::fs::write(plans_root.join("roadmap.md"), plan_md).unwrap();
        let v2 = ok_status(import_docs(
            &db,
            &json!({
                "skills_dir": skills_root.to_str().unwrap(),
                "plans_dir": plans_root.to_str().unwrap(),
            }),
        ));
        assert_eq!(v2["skills"], 1, "{v2}");
        assert_eq!(v2["plans"], 1, "{v2}");
        let pb = ok_status(payload_get(&db, &json!({"qn": "skill:skill-b/SKILL"})));
        assert_eq!(pb["payload"]["payload"], skill_b);
        let pp = ok_status(payload_get(&db, &json!({"qn": "plan:roadmap"})));
        assert_eq!(pp["payload"]["payload"], plan_md);
        // list_docs voit les nœuds Skill/Plan importés.
        let docs = ok_status(list_docs(&db, &json!({})));
        let total = docs["total"].as_u64().unwrap();
        assert!(total >= 3, "list_docs total={total}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_sessions_jsonl_idempotent_et_zstd_refuse() {
        let (db, _, dir) = make_db();
        let sess_dir = dir.join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        let lines = concat!(
            r#"{"type":"session","id":"arch1","seq":0,"cwd":"/tmp","time":0,"data":{}}"#,
            "\n",
            r#"{"type":"user/message","seq":1,"time":1,"data":{"role":"user","content":"Bonjour"}}"#,
            "\n",
            r#"{"type":"tool/call","seq":2,"time":2,"data":{"callId":"c1","name":"read"}}"#,
            "\n",
            r#"{"type":"tool/result","seq":3,"time":3,"data":{"message":{"content":[{"type":"text","toolCallId":"c1","text":"ok"}]}}}"#,
            "\n",
        );
        std::fs::write(sess_dir.join("s1.jsonl"), lines).unwrap();
        let v = ok_status(import_sessions(
            &db,
            &json!({"dir": sess_dir.to_str().unwrap()}),
        ));
        assert_eq!(v["files"], 1, "{v}");
        assert_eq!(v["sessions"], 1, "{v}");
        assert_eq!(v["events"], 4, "{v}");
        assert_eq!(v["imported"], 4, "{v}");
        let session = db.session();
        assert!(node_exists(&session, "Session", "session-arch1"));
        assert!(node_exists(&session, "Message", "session-arch1:msg:1"));
        // Ré-import : idempotent par (session, seq) — 0 événement re-importé.
        let v2 = ok_status(import_sessions(
            &db,
            &json!({"dir": sess_dir.to_str().unwrap()}),
        ));
        assert_eq!(v2["imported"], 0, "{v2}");
        assert_eq!(v2["events"], 4, "{v2}");
        // .zstd → erreur explicite (compression non supportée ici).
        std::fs::write(sess_dir.join("s2.jsonl.zstd"), b"x").unwrap();
        let e = import_sessions(&db, &json!({"dir": sess_dir.to_str().unwrap()}));
        assert!(e.1, "zstd doit être une erreur : {:?}", e.0);
        assert!(
            e.0["error"]
                .as_str()
                .unwrap()
                .contains("compression non supportée"),
            "{:?}",
            e.0
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manage_adr_update_get_sections_sur_noeud_graphe() {
        let (db, _, dir) = make_db();
        let content = "# ADR 001 — Bridge Rust\n\n## Contexte\nLe serveur C part.\n\n## Décision\nLe bridge Rust le remplace.\n";
        let v = ok_status(manage_adr(
            &db,
            &json!({"mode": "update", "project": "bench", "path": "001-bridge", "content": content}),
        ));
        assert_eq!(v["qn"], "adr:bench/001-bridge", "{v}");
        let g = ok_status(manage_adr(
            &db,
            &json!({"mode": "get", "project": "bench", "path": "001-bridge"}),
        ));
        assert_eq!(g["content"], json!(content));
        let s = ok_status(manage_adr(
            &db,
            &json!({"mode": "sections", "project": "bench", "path": "001-bridge"}),
        ));
        let secs: Vec<String> = s["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        assert_eq!(secs, vec!["Contexte".to_string(), "Décision".to_string()]);
        let session = db.session();
        assert!(node_exists(&session, "ADR", "adr:bench/001-bridge"));
        // get sur un nœud absent → erreur ; alias id + project par défaut.
        let e = manage_adr(
            &db,
            &json!({"mode": "get", "project": "bench", "path": "absent"}),
        );
        assert!(e.1);
        assert!(e.0["error"].as_str().unwrap().contains("introuvable"));
        let g2 = ok_status(manage_adr(
            &db,
            &json!({"mode": "get", "project": "bench", "id": "001-bridge"}),
        ));
        assert!(g2["content"].as_str().unwrap().contains("Bridge Rust"));
        // update remplace le document complet.
        let v3 = ok_status(manage_adr(
            &db,
            &json!({"mode": "update", "project": "bench", "path": "001-bridge", "content": "nouveau contenu"}),
        ));
        assert_eq!(v3["mode"], "update");
        let g3 = ok_status(manage_adr(
            &db,
            &json!({"mode": "get", "project": "bench", "path": "001-bridge"}),
        ));
        assert_eq!(g3["content"], json!("nouveau contenu"));
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── AutoOpt : logger de métriques (OBRAIN_METRICS_FILE) ────────────────
    // Env activé → 2 appels query = 2 lignes JSON valides (engine_us
    // numérique, q = préfixe 60 car.) ; env absent → aucun fichier créé et
    // aucun append sur un fichier existant. Route via `call_tool` (seul point
    // d'entrée réel du bridge) ; les autres tests appellent les fonctions
    // internes directement → aucune ligne parasite possible.
    // SAFETY: set_var/remove_var mono-threadés sur CETTE variable (aucun autre
    // test ne la lit) — le pattern main.rs (NO_COLOR) fait de même.
    #[test]
    #[allow(unsafe_code)]
    fn metrics_logger_jsonl_quand_env_absent_rien() {
        let (db, _, dir) = make_db();
        let metric_file = dir.join("autoopt-metrics.jsonl");
        let no_env_file = dir.join("autoopt-never.jsonl");
        let marker = format!("zzmetric{}", std::process::id());
        let q = format!(
            "MATCH (n:Function) WHERE n.qualified_name = '{marker}' RETURN n.qualified_name"
        );
        let q60: String = q.chars().take(60).collect();

        // 1) Sans journal : l'outil tourne normalement, AUCUN fichier créé.
        // Le forçage dit « pas de journal » même si l'environnement de la machine
        // en demande un : la phase est hermétique, pas « hermétique si l'env
        // est propre ».
        forcer_metriques(Some(None));
        let (c, err) = call_tool(
            &db,
            "query",
            &json!({"query": q.clone(), "language": "gql"}),
        );
        assert!(!err && c["status"] == "ok", "{c}");
        assert!(
            !no_env_file.exists(),
            "fichier créé sans OBRAIN_METRICS_FILE"
        );

        // 2) Avec env : deux appels query → deux lignes JSON valides.
        forcer_metriques(Some(Some(metric_file.to_str().unwrap().to_string())));
        for _ in 0..2 {
            let (c, err) = call_tool(
                &db,
                "query",
                &json!({"query": q.clone(), "language": "gql"}),
            );
            assert!(!err && c["status"] == "ok", "{c}");
        }
        forcer_metriques(None);
        let raw = std::fs::read_to_string(&metric_file).unwrap();
        // ── LES LIGNES COMPLÈTES SEULES SONT JUGÉES ──────────────────────────
        // Le journal de mesures est un fichier APPENDÉ par le pont — un seul
        // écrivain en production. Ce test-ci partage son fichier avec les autres
        // tests du binaire (le forçage est global), donc une lecture peut tomber
        // au milieu d'un append concurrent : une ligne tronquée n'est pas un
        // défaut du produit, c'est un artefact du harnais. On ne juge donc que
        // les enregistrements TERMINÉS (une ligne JSON complète), et l'assertion
        // garde ses dents : chaque ligne complète doit être du JSON valide
        // portant l'outil et la requête attendus.
        let lines: Vec<&str> = raw
            .lines()
            .filter(|l| l.contains(&marker) && l.trim_end().ends_with('}'))
            .collect();
        assert_eq!(
            lines.len(),
            2,
            "2 lignes complètes attendues, obtenues : {lines:?}"
        );
        for l in &lines {
            let rec: Value = serde_json::from_str(l).expect("ligne JSON valide");
            assert_eq!(rec["tool"], json!("query"), "{rec}");
            assert_eq!(rec["q"], json!(q60), "{rec}");
            assert!(
                rec["engine_us"].is_number(),
                "engine_us numérique attendu : {rec}"
            );
            assert!(rec["wall_us"].is_number(), "wall_us présent : {rec}");
            assert!(rec["ts"].as_u64().unwrap_or(0) > 0, "ts epoch ms : {rec}");
        }

        // 3) Env retiré → un nouvel appel n'append RIEN au fichier existant.
        let (c, err) = call_tool(
            &db,
            "query",
            &json!({"query": q.clone(), "language": "gql"}),
        );
        assert!(!err && c["status"] == "ok", "{c}");
        let raw2 = std::fs::read_to_string(&metric_file).unwrap();
        assert_eq!(
            raw2.lines().filter(|l| l.contains(&marker)).count(),
            2,
            "aucun append sans env"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── P0-v05 : outils bridge de restitution (viz_subgraph / viz_export /
    //    node_curvature) — parité avec la CLI `obrain viz` (viz_extract). ──

    #[test]
    fn viz_subgraph_bridge_renvoie_un_sous_graphe_rendable() {
        let (db, _, _) = make_db();
        let (c, err) = call_tool(
            &db,
            "viz_subgraph",
            &json!({ "center": "test.repo.a", "hops": 1 }),
        );
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");
        assert_eq!(c["tool"], "viz_subgraph");
        assert_eq!(c["node_count"], 2, "{c}"); // a + b (CALLS)
        assert_eq!(c["edge_count"], 1, "{c}");
        assert_eq!(c["truncated"], false);
        assert!(c["result"]["schema"] == json!("obrain/json-graph@1"), "{c}");
        let nodes = c["result"]["nodes"].as_array().unwrap();
        let qns: Vec<&str> = nodes.iter().filter_map(|n| n["qn"].as_str()).collect();
        assert!(
            qns.contains(&"test.repo.a") && qns.contains(&"test.repo.b"),
            "{qns:?}"
        );
    }

    #[test]
    fn viz_subgraph_bridge_accepte_id_et_overlay_roles() {
        let (db, _, _) = make_db();
        let (c, err) = call_tool(
            &db,
            "viz_subgraph",
            &json!({ "center": "1", "hops": 1, "overlay": "roles" }),
        );
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");
        assert_eq!(c["center_id"], 1);
        assert_eq!(c["overlay_report"]["overlay"], "roles", "{c}");
        // Le centre (signal scc lu) porte un color_hint dans le json-graph.
        let center = c["result"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == json!("1"))
            .unwrap();
        assert!(center["color_hint"].is_string(), "{center}");
    }

    #[test]
    fn viz_subgraph_bridge_format_mermaid() {
        let (db, _, _) = make_db();
        let (c, err) = call_tool(
            &db,
            "viz_subgraph",
            &json!({ "center": "test.repo.a", "hops": 1, "format": "mermaid" }),
        );
        assert!(!err, "{c}");
        assert_eq!(c["result"]["format"], "mermaid", "{c}");
        assert!(
            c["result"]["text"]
                .as_str()
                .unwrap_or("")
                .starts_with("flowchart LR"),
            "{c}"
        );
    }

    #[test]
    fn viz_subgraph_bridge_erreur_si_centre_inconnu() {
        let (db, _, _) = make_db();
        let (c, err) = call_tool(&db, "viz_subgraph", &json!({ "center": "nope.qn" }));
        assert!(err, "{c}");
        assert_eq!(c["status"], "error");
    }

    #[test]
    fn viz_export_bridge_parite_avec_le_store() {
        let (db, _, _) = make_db();
        let (c, err) = call_tool(&db, "viz_export", &json!({}));
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");
        // make_db : 2 Function + CALLS → 2 nœuds / 1 arête (b sans CALLS sortant).
        assert_eq!(c["node_count"], 2, "{c}");
        assert_eq!(c["edge_count"], 1, "{c}");
        assert!(c["result"]["schema"] == json!("obrain/json-graph@1"));
    }

    #[test]
    fn node_curvature_bridge_lit_curvature_et_energy() {
        let (db, _, _) = make_db();
        let (c, err) = call_tool(&db, "node_curvature", &json!({ "node": "test.repo.a" }));
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");
        assert_eq!(c["node_id"], 1);
        assert_eq!(c["qualified_name"], "test.repo.a");
        // a est isolé (aucune arête incidente vive dans make_db ? a→b existe :
        // courbure définie ou Null — le contrat est : champ présent, energy numérique).
        assert!(
            c["curvature"].is_null() || c["curvature"].is_number(),
            "{c}"
        );
        assert!(c["energy"].is_number(), "{c}");
    }

    #[test]
    fn viz_topo_bridge_detecte_un_triangle_calls() {
        sans_iam_dans_l_environnement();
        // Mini graphe triangle dirigé : n1→n2→n3→n1 (triangle non dirigé).
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-topo-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();
        let n1 = db.create_node(&["Function"]);
        db.set_node_property(n1, "qualified_name", OV::from("t.one".to_string()));
        let n2 = db.create_node(&["Function"]);
        db.set_node_property(n2, "qualified_name", OV::from("t.two".to_string()));
        let n3 = db.create_node(&["Function"]);
        db.set_node_property(n3, "qualified_name", OV::from("t.three".to_string()));
        let _ = db.create_edge(n1, n2, "CALLS");
        let _ = db.create_edge(n2, n3, "CALLS");
        let _ = db.create_edge(n3, n1, "CALLS");
        db.create_property_index("qualified_name");
        db.checkpoint().unwrap();

        let (c, err) = call_tool(&db, "viz_topo", &json!({ "center": "t.one", "hops": 2 }));
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");
        assert_eq!(c["undirected_triangle_incidences"], 3, "{c}");
        assert_eq!(c["top_nodes"].as_array().unwrap().len(), 3, "{c}");
        let _ = std::fs::remove_dir_all(dir);
    }

    // ── P2 (2026-09-09) : runtime_search à chaud — cache d'index + contrat ──

    /// Crée un nœud Message (chemin CRUD du moteur, qui entretient les index
    /// texte du substrat incrémentalement quand l'index existe déjà).
    fn insert_message(db: &ObrainDB, qn: &str, text: &str) {
        let n = db.create_node(&["Message"]);
        if !qn.is_empty() {
            let _ = db.set_node_property(n, "qualified_name", OV::from(qn.to_string()));
        }
        let _ = db.set_node_property(n, "text", OV::from(text.to_string()));
    }

    #[test]
    fn runtime_search_contrat_sortie_et_reutilisation_a_chaud() {
        sans_iam_dans_l_environnement();
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-rts-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();

        insert_message(&db, "msg:1", "un panda roux grimpe");
        let (c, err) = call_tool(&db, "runtime_search", &json!({ "query": "panda" }));
        assert!(!err, "{c}");
        assert_eq!(c["status"], "ok", "{c}");
        assert_eq!(c["total"], 1, "{c}");
        let row = &c["rows"][0];
        // Contrat canal cbm : qn/text TOUJOURS strings ; compat additif
        // id/score/label/snippet/rank.
        assert_eq!(row["qn"], json!("msg:1"), "{row}");
        assert!(row["qn"].is_string(), "{row}");
        assert!(row["text"].is_string(), "{row}");
        assert!(row["text"].as_str().unwrap().contains("panda"), "{row}");
        assert!(row["id"].is_string(), "{row}");
        assert!(row["label"].is_string(), "{row}");
        assert!(row["snippet"].is_string(), "{row}");
        assert!(row["score"].is_number(), "{row}");
        assert!(row["rank"].is_number(), "{row}");
        // Après build, l'index est présent.
        assert!(db.has_text_index("Message", "text"));

        // Écriture CRUD SUIVANTE : `set_node_property` entretient l'index déjà
        // présent incrémentalement → à génération constante (aucun commit GQL),
        // la politique P2 RÉUTILISE l'index : le nouveau message est pourtant
        // trouvé (entretien incrémental), sans rebuild.
        insert_message(&db, "msg:2", "un panda noir dort");
        let (c2, _err2) = call_tool(&db, "runtime_search", &json!({ "query": "panda" }));
        assert_eq!(c2["total"], 2, "{c2}");
        assert!(db.has_text_index("Message", "text"));

        // Message sans qualified_name : qn = repli id (string) — jamais absent.
        insert_message(&db, "", "un panda discret");
        let (c3, _err3) = call_tool(&db, "runtime_search", &json!({ "query": "panda" }));
        assert_eq!(c3["total"], 3, "{c3}");
        for row in c3["rows"].as_array().unwrap() {
            assert!(
                row["qn"].is_string() && !row["qn"].as_str().unwrap().is_empty(),
                "{row}"
            );
            assert!(row["text"].is_string(), "{row}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn runtime_search_reconstruit_quand_l_epoch_a_avance() {
        sans_iam_dans_l_environnement();
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-rts2-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();

        insert_message(&db, "msg:1", "le mandrill vert saute");
        let (c, err) = call_tool(&db, "runtime_search", &json!({ "query": "mandrill" }));
        assert!(!err, "{c}");
        assert_eq!(c["total"], 1, "{c}");
        let gen0 = db.current_epoch().as_u64();

        // Écriture GQL — le chemin RÉEL d'import_events (session.execute
        // MERGE/SET). Elle ne passe pas par les hooks CRUD : l'index BM25 déjà
        // présent n'est PAS entretenu incrémentalement. Seule l'avance de
        // génération (epoch de commit) peut déclencher le rebuild.
        let session = db.session();
        session
            .execute("MERGE (m:Message {qualified_name: 'msg:2'}) RETURN id(m)")
            .unwrap();
        session
            .execute(
                "MATCH (m:Message {qualified_name: 'msg:2'}) SET m.text = 'le mandrill bleu court'",
            )
            .unwrap();
        let gen1 = db.current_epoch().as_u64();
        assert!(
            gen1 > gen0,
            "l'epoch substrate n'a pas avancé après un commit GQL ({gen0} → {gen1}) : \
             le signal de fraîcheur du cache P2 repose dessus"
        );

        // Génération avancée → REBUILD → le nouveau message est trouvé.
        let (c2, err2) = call_tool(&db, "runtime_search", &json!({ "query": "mandrill" }));
        assert!(!err2, "{c2}");
        assert_eq!(
            c2["total"], 2,
            "rebuild attendu après avance de génération : {c2}"
        );

        // Génération constante → réutilisation → résultats identiques.
        let (c3, err3) = call_tool(&db, "runtime_search", &json!({ "query": "mandrill" }));
        assert!(!err3, "{c3}");
        assert_eq!(
            c3["total"], 2,
            "réutilisation à génération constante : {c3}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn runtime_search_reconstruit_apres_drop_de_l_index() {
        sans_iam_dans_l_environnement();
        let dir = std::env::temp_dir().join(format!(
            "obrain-mcp-rts3-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();

        insert_message(&db, "msg:1", "un okapi tacheté se cache");
        let (c, err) = call_tool(&db, "runtime_search", &json!({ "query": "okapi" }));
        assert!(!err, "{c}");
        assert_eq!(c["total"], 1, "{c}");
        assert!(db.has_text_index("Message", "text"));

        // L'index disparaît du registre (drop) : présence = false → le prochain
        // appel doit REBUILD, même à génération constante.
        assert!(db.drop_text_index("Message", "text"));
        assert!(!db.has_text_index("Message", "text"));
        let (c2, err2) = call_tool(&db, "runtime_search", &json!({ "query": "okapi" }));
        assert!(!err2, "{c2}");
        assert_eq!(c2["total"], 1, "rebuild attendu après drop : {c2}");
        assert!(db.has_text_index("Message", "text"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
