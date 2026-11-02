//! `selfd` — le micro-harness d'un nœud : il agit sur la machine, et il le dit.
//!
//! # Quatre façons d'atteindre le même nœud
//!
//! ```text
//! selfd serve [--socket P]      démon : socket Unix locale, plusieurs clients
//! selfd status                  la CLI de la machine qui exécute le nœud
//! selfd exec '{"cmd":"…"}'      idem — tout appel passe par la socket locale
//! selfd mcp                     serveur MCP sur stdio → à brancher sur le harness
//! selfd --stdio                 JSON-RPC natif sur stdio (le chemin `ssh` direct)
//! selfd --call <m> [json]       un appel, sans démon (diagnostic)
//! ```
//!
//! Les quatre appellent **le même** `protocole::executer_outil` : une capacité
//! ajoutée une fois est disponible partout, et il n'y a pas quatre
//! implémentations à garder d'accord.
//!
//! # Pourquoi en Rust
//!
//! Le harness Node a été essayé d'abord (`spike/node-distant/`). Il a échoué, et
//! pas sur le transport : sa composition de plugins doit se résoudre depuis un
//! répertoire portant ses `node_modules` —
//! `Cannot find package '@deepseek-ai/dsh-token-meter' imported from …/spike/node-distant/`.
//! Un second essai a buté sur l'interpréteur : un shell `ssh` non interactif
//! résout `/usr/bin/node` (v18) alors que le harness exige ≥ 20.12.
//!
//! Un nœud qu'on déploie sur N machines ne peut dépendre ni d'un arbre de 1,4 Go
//! de `node_modules`, ni du PATH d'un shell. `selfd` est un binaire de ~750 Ko :
//! on le copie, il démarre — en 4 ms.

mod agent;
mod attache;
mod chat;
mod ecoute;
mod identite;
mod mcp;
mod outils;
mod politique;
mod protocole;
mod serveur;
mod soi;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use politique::Politique;
use protocole::{Etat, analyser, executer_outil, repondre};
use serde_json::{Value, json};

const AIDE: &str = "\
selfd — micro-harness d'un nœud : il agit sur la machine, et il le dit

USAGE
  selfd serve [options]              démon local (socket Unix), plusieurs clients
  selfd <méthode> [json]             CLI locale : parle au démon, et sort
  selfd mcp [options]                serveur MCP sur stdio — pour le harness
  selfd --stdio [options]            JSON-RPC natif sur stdio (ssh direct)
  selfd --call <méthode> [json]      un appel direct, sans démon

MÉTHODES
  status                             identité du nœud, politique, charge
  chat {message}                     converser avec le nœud (un tour, sans outil)
  run {objectif, maxSteps?}          DÉROULER un objectif : le harness agit et rend compte
  caps | initialize                  identité et capacités
  exec {cmd, args[], cwd?, shell?, timeoutMs?}
  read {path, maxBytes?}             write {path, content}
  list {path?}                       shutdown

OPTIONS
  --root <dir>        racine du nœud ; aucun chemin n'en sort (défaut : cwd)
  --allow-exec        AUTORISE l'exécution de commandes (défaut : refusée)
  attacher            branche cette machine au moteur et attend du travail
                      (--moteur <url> --jeton-moteur <jeton> [--machine <nom>])
  --socket <chemin>   socket du démon   (défaut $SELFD_SOCKET, sinon XDG_RUNTIME_DIR)
  --state-dir <dir>   identité persistée (défaut $SELFD_STATE_DIR, sinon ~/.local/state/selfd)
  --base <url>        base commune du nœud (défaut $SELFD_BASE)
  --moteur <url>      moteur obrain : LE SOCLE que ce harness sert (défaut $SELFD_MOTEUR,
                      sinon 127.0.0.1:8432). Le self, ses mondes et sa base sont DÉCOUVERTS.
  --chat-port <n>     ouvre la surface HTTP de chat (EXIGE un jeton ; 0 = port libre)
  --chat-host <a>     adresse d'écoute du chat (défaut 127.0.0.1, jamais 0.0.0.0 par défaut)
  --llm <url>         modèle OpenAI-compatible du chat (défaut $SELFD_LLM, sinon 127.0.0.1:9901/v1)
  --token <secret>    jeton de la surface HTTP (OBLIGATOIRE pour --chat-port)
  --token-sur-socket  exige aussi le jeton sur la socket Unix (défaut : non, le 0600 suffit)
  --max-output <n>    octets gardés par flux (défaut 262144)
  --timeout-ms <n>    délai par défaut d'une commande (défaut 60000)
  --json              sortie JSON brute (mode CLI)
  -h, --help          cette aide

BRANCHER LE NŒUD SUR LE HARNESS (aucun code harness à écrire)
  - id: mcp-n3
    name: '@deepseek-ai/dsh-mcp-client'
    config:
      transport: stdio
      command: ssh
      args: ['n3', '/opt/selfd/selfd', 'mcp', '--root', '/srv', '--allow-exec']

PRINCIPE
  La politique est fixée au DÉMARRAGE, jamais négociée : un client peut la lire
  (status, caps), pas l'élargir. Sans --allow-exec, le nœud observe et n'agit pas.
";

/// Ce que la ligne de commande a décidé.
struct Options {
    politique: Politique,
    state_dir: PathBuf,
    socket: PathBuf,
    base: Option<String>,
    secret: Option<String>,
    /// Positionnel : `serve`, `mcp`, ou un nom de méthode (mode CLI).
    commande: Option<String>,
    params: Value,
    un_coup: Option<(String, Value)>,
    json: bool,
    /// Port de la surface HTTP. `None` = aucune surface réseau ouverte.
    chat_port: Option<u16>,
    chat_host: String,
    llm: String,
    /// Exiger le jeton aussi sur la socket Unix (défaut : non — le `0600` suffit).
    jeton_sur_socket: bool,
    /// URL du moteur : le SOCLE que ce harness sert.
    moteur: Option<String>,
    /// Jeton IAM présenté AU MOTEUR (mode `attacher`) — distinct du jeton qui
    /// garde la surface locale.
    jeton_moteur: Option<String>,
    /// Le nom sous lequel cette machine apparaît chez son compte.
    machine: Option<String>,
}

/// Les modes qui ne prennent pas de JSON de paramètres.
fn est_commande_de_mode(c: &str) -> bool {
    matches!(c, "serve" | "mcp" | "attacher")
}

/// Lit un éventuel objet JSON en argument, et de combien avancer.
fn parametres_suivants(args: &[String], index: usize) -> Result<(Value, usize)> {
    match args.get(index) {
        Some(s) if s.starts_with('{') => {
            let v = serde_json::from_str(s)
                .map_err(|e| anyhow::anyhow!("paramètres illisibles : {e}"))?;
            Ok((v, 1))
        }
        _ => Ok((json!({}), 0)),
    }
}

fn parser(args: &[String]) -> Result<Options> {
    let mut politique = Politique::default();
    let mut state_dir = identite::state_dir_defaut();
    // La précédence : un drapeau EXPLICITE bat une variable d'environnement,
    // qui bat un défaut. `--state-dir` déplace donc la socket **sauf** si
    // `--socket` ou `$SELFD_SOCKET` l'ont dit autrement — sans quoi déplacer
    // l'état laisserait la socket ailleurs, et deux nœuds se croiraient seuls.
    let mut state_dir_explicite = false;
    let mut socket_explicite = std::env::var("SELFD_SOCKET")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let mut socket = socket_explicite
        .clone()
        .unwrap_or_else(serveur::socket_defaut);
    let mut base = std::env::var("SELFD_BASE").ok().filter(|s| !s.is_empty());
    // Le secret par environnement d'abord : un argument est visible dans `ps`.
    let mut secret = std::env::var("SELFD_TOKEN").ok().filter(|s| !s.is_empty());
    let mut commande: Option<String> = None;
    let mut params = json!({});
    let mut un_coup: Option<(String, Value)> = None;
    let mut json_flag = false;
    let mut chat_port: Option<u16> = None;
    // 127.0.0.1 par défaut : exposer est un acte, pas un défaut.
    let mut chat_host = "127.0.0.1".to_string();
    let mut llm = chat::llm_par_defaut();
    let mut jeton_sur_socket = false;
    let mut moteur: Option<String> = std::env::var("SELFD_MOTEUR").ok().filter(|s| !s.is_empty());
    let mut jeton_moteur: Option<String> = std::env::var("SELFD_MOTEUR_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    let mut machine: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{AIDE}");
                std::process::exit(0);
            }
            // Mode par défaut : accepté pour pouvoir être écrit explicitement.
            "--stdio" => {}
            "--json" => json_flag = true,
            "--token-sur-socket" => jeton_sur_socket = true,
            // ── SE BRANCHER AU MOTEUR : QUI ON EST, ET SOUS QUEL NOM ────────
            // Le jeton du MOTEUR n'est pas celui de la surface locale : l'un
            // dit à un serveur distant de quel compte est cette machine,
            // l'autre garde la porte du démon sur cette machine. Les confondre
            // reviendrait à envoyer la clé de la maison à qui demande son nom.
            "--jeton-moteur" => {
                jeton_moteur = args.get(i + 1).cloned();
                i += 1;
            }
            "--machine" => {
                machine = args.get(i + 1).cloned();
                i += 1;
            }
            "--moteur" => {
                i += 1;
                let Some(u) = args.get(i) else {
                    bail!("--moteur attend une URL")
                };
                moteur = Some(u.clone());
            }
            "--chat-port" => {
                i += 1;
                let Some(p) = args.get(i) else {
                    bail!("--chat-port attend un port")
                };
                chat_port = Some(p.parse().map_err(|_| {
                    anyhow::anyhow!("--chat-port attend un entier (0 = port libre)")
                })?);
            }
            "--chat-host" => {
                i += 1;
                let Some(h) = args.get(i) else {
                    bail!("--chat-host attend une adresse")
                };
                chat_host = h.clone();
            }
            "--llm" => {
                i += 1;
                let Some(u) = args.get(i) else {
                    bail!("--llm attend une URL")
                };
                llm = u.clone();
            }
            "--allow-exec" => politique.exec_autorise = true,
            "--root" => {
                i += 1;
                let Some(d) = args.get(i) else {
                    bail!("--root attend un répertoire")
                };
                politique.racine = std::fs::canonicalize(d)
                    .map_err(|e| anyhow::anyhow!("--root inexistant ({d}) : {e}"))?;
            }
            "--socket" => {
                i += 1;
                let Some(p) = args.get(i) else {
                    bail!("--socket attend un chemin")
                };
                socket = PathBuf::from(p);
                socket_explicite = Some(socket.clone());
            }
            "--state-dir" => {
                i += 1;
                let Some(p) = args.get(i) else {
                    bail!("--state-dir attend un chemin")
                };
                state_dir = PathBuf::from(p);
                state_dir_explicite = true;
            }
            "--base" => {
                i += 1;
                let Some(u) = args.get(i) else {
                    bail!("--base attend une URL")
                };
                base = Some(u.clone());
            }
            "--token" => {
                i += 1;
                let Some(t) = args.get(i) else {
                    bail!("--token attend un secret")
                };
                eprintln!(
                    "selfd : AVERTISSEMENT — un secret en argument est visible dans `ps`. \
                     Préférer SELFD_TOKEN dans l'environnement."
                );
                secret = Some(t.clone());
            }
            "--max-output" => {
                i += 1;
                politique.plafond_octets = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("--max-output attend un entier"))?;
            }
            "--timeout-ms" => {
                i += 1;
                politique.delai_ms = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("--timeout-ms attend un entier"))?;
            }
            "--call" => {
                i += 1;
                let Some(m) = args.get(i) else {
                    bail!("--call attend une méthode")
                };
                let (valeur, avance) = parametres_suivants(args, i + 1)?;
                i += avance;
                un_coup = Some((m.clone(), valeur));
            }
            a if a.starts_with("--") => bail!("option inconnue : {a} (voir --help)"),
            a => {
                if commande.is_none() {
                    commande = Some(a.to_string());
                } else if commande
                    .as_deref()
                    .map(est_commande_de_mode)
                    .unwrap_or(false)
                {
                    bail!("argument inattendu : {a}");
                } else {
                    // En mode CLI, le second positionnel est le JSON des paramètres.
                    let (valeur, avance) = parametres_suivants(args, i)?;
                    params = valeur;
                    i += avance.saturating_sub(1);
                }
            }
        }
        i += 1;
    }

    // `--state-dir` explicite, et rien d'autre qui tranche : la socket suit.
    if socket_explicite.is_none() && state_dir_explicite {
        socket = state_dir.join("selfd.sock");
    }

    Ok(Options {
        politique,
        state_dir,
        socket,
        base,
        secret,
        commande,
        params,
        un_coup,
        json: json_flag,
        chat_port,
        chat_host,
        llm,
        jeton_sur_socket,
        moteur,
        jeton_moteur,
        machine,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match parser(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("selfd : {e:#}");
            std::process::exit(2);
        }
    };
    if let Err(e) = executer(o) {
        eprintln!("selfd : {e:#}");
        std::process::exit(1);
    }
}

fn executer(o: Options) -> Result<()> {
    // ── MODE CLI : on parle au démon local, on ne fait rien soi-même ────────
    // C'est ce qui rend le nœud atteignable depuis le terminal de sa machine, et
    // c'est aussi ce qui garantit qu'un opérateur lit la MÊME politique que
    // celle réellement appliquée : celle du démon, pas celle de son shell.
    if let Some(m) = o.commande.clone() {
        if !est_commande_de_mode(&m) {
            // Le délai de lecture dépasse celui de la commande pour que le
            // client attende le nœud au lieu de conclure à sa mort.
            let delai = std::time::Duration::from_millis(o.politique.delai_ms.max(60_000) + 30_000);
            // La CLI présente le jeton de son environnement, s'il y en a un : un
            // opérateur qui l'a exporté n'a pas à le répéter à chaque commande.
            let resultat = serveur::appeler(&o.socket, &m, &o.params, delai, o.secret.as_deref())?;
            afficher(&m, &resultat, o.json);
            return Ok(());
        }
    }

    let mode: &'static str = match o.commande.as_deref() {
        Some("serve") => "socket",
        Some("mcp") => "mcp",
        Some("attacher") => "attache",
        _ => "stdio",
    };

    // L'identité est chargée AVANT tout service : un nœud qui ne saurait pas qui
    // il est ne devrait pas accepter de travail.
    let id = identite::Identite::charger_ou_creer(&o.state_dir, o.base.as_deref())?;

    // En mode MCP, le lanceur EST le client (voir l'en-tête de `mcp.rs`) : un
    // jeton n'y garde rien, et le dire vaut mieux que de le simuler.
    if mode == "mcp" && o.secret.is_some() {
        eprintln!(
            "selfd : en mode MCP le jeton n'est pas transmis par appel — la frontière est \
             le lanceur (configuration du harness, accès ssh, permissions du fichier)."
        );
    }
    let verrou = if mode == "mcp" {
        None
    } else {
        o.secret.as_deref()
    };

    // ── MODE UN-COUP (diagnostic) ───────────────────────────────────────────
    let historique = chat::Historique::nouveau(&o.state_dir);

    if let Some((methode, params)) = o.un_coup.clone() {
        let mut etat = Etat::nouveau(
            o.politique.clone(),
            id,
            "call",
            verrou,
            o.llm.clone(),
            historique,
        );
        etat.notes = o.state_dir.join("notes.jsonl");
        if let Some(m) = &o.moteur {
            etat.moteur = m.clone();
        }
        match executer_outil(&mut etat, &methode, &params, o.secret.as_deref()) {
            Ok(v) => {
                afficher(&methode, &v, o.json);
                Ok(())
            }
            Err(e) => bail!(
                "{}",
                e.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("erreur")
            ),
        }
    } else if mode == "attache" {
        // ── SE BRANCHER AU MOTEUR ───────────────────────────────────────────
        // Le jeton présenté ici dit à un serveur DISTANT de quel compte est
        // cette machine. Il n'est pas le verrou local : la politique, elle,
        // reste celle du démarrage, et aucune demande venue du moteur ne peut
        // l'élargir. On passe donc `None` comme secret de surface — exiger un
        // jeton local sur un canal déjà authentifié n'ajouterait aucune
        // frontière, seulement une confusion entre deux clés distinctes.
        let Some(moteur) = o.moteur.clone() else {
            bail!(
                "« attacher » demande l'adresse du moteur : --moteur <url> \
                 (ou la variable SELFD_MOTEUR)"
            );
        };
        // ── UN JETON EN LIGNE DE COMMANDE SE LIT DANS `ps` ──────────────────
        // Tout utilisateur de la machine voit les arguments des processus des
        // autres. On ne refuse pas — il faut bien pouvoir essayer une fois —
        // mais on le dit, parce qu'un secret qu'on croit privé et qui ne l'est
        // pas est pire qu'un secret qu'on sait exposé.
        if std::env::args().any(|a| a == "--jeton-moteur") {
            eprintln!(
                "selfd : le jeton passé en argument est visible dans `ps` par tout utilisateur \
                 de cette machine. Préférez la variable SELFD_MOTEUR_TOKEN."
            );
        }
        let Some(jeton) = o.jeton_moteur.clone() else {
            bail!(
                "« attacher » demande le jeton de VOTRE compte : --jeton-moteur <jeton> \
                 (ou la variable SELFD_MOTEUR_TOKEN). Il s'obtient sur le 8432."
            );
        };
        let machine = o.machine.clone().unwrap_or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|s| String::from_utf8(s.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "machine sans nom".to_string())
        });
        let mut etat = Etat::nouveau(o.politique, id, "attache", None, o.llm.clone(), historique);
        etat.notes = o.state_dir.join("notes.jsonl");
        etat.moteur = moteur.clone();
        attache::boucle(&mut etat, &moteur, &jeton, &machine)
    } else if mode == "mcp" {
        let mut etat = Etat::nouveau(o.politique, id, "mcp", None, o.llm.clone(), historique);
        etat.notes = o.state_dir.join("notes.jsonl");
        if let Some(m) = &o.moteur {
            etat.moteur = m.clone();
        }
        mcp::boucle(&mut etat, None)
    } else if mode == "socket" {
        // ── PAS DE JETON, PAS DE CHAT ──────────────────────────────────────
        // Vérifié ICI, pas dans le fil d'écoute : un échec dans un fil lancé
        // laisserait le démon tourner sans sa surface, et l'opérateur croirait
        // l'avoir ouverte. On refuse AVANT, et fort.
        if o.chat_port.is_some() && o.secret.is_none() {
            bail!(
                "refus d'ouvrir la surface HTTP de chat sans jeton : ajouter \
                 --token <secret> ou $SELFD_TOKEN.\n\
                 Un chat de nœud non authentifié est une porte ouverte sur \
                 l'exécution de commandes sur cette machine."
            );
        }
        // ── L'AUTORITÉ, LÀ OÙ ELLE EXISTE ──────────────────────────────────
        // La socket est déjà gardée par ses permissions (`0600`) : exiger en plus
        // un secret serait une cérémonie, pas une frontière — même raisonnement
        // que pour MCP, où le lanceur est la frontière. `--token-sur-socket`
        // reste offert à qui veut la ceinture ET les bretelles.
        let verrou_socket = if o.jeton_sur_socket { verrou } else { None };
        let mut etat = Etat::nouveau(
            o.politique.clone(),
            id.clone(),
            "socket",
            verrou_socket,
            o.llm.clone(),
            historique,
        );
        etat.notes = o.state_dir.join("notes.jsonl");
        if let Some(m) = &o.moteur {
            etat.moteur = m.clone();
        }

        // La surface HTTP tourne À CÔTÉ de la socket : deux publics, deux
        // autorités — permissions de fichier en local, jeton pour le réseau.
        if let Some(port) = o.chat_port {
            let ecoute = ecoute::Ecoute {
                host: o.chat_host.clone(),
                port,
            };
            let surface = ecoute::Surface {
                politique: o.politique.clone(),
                identite: id.clone(),
                debut: std::time::Instant::now(),
                llm: o.llm.clone(),
                // `is_none` a été refusé juste au-dessus : le déballage est sûr.
                secret: o.secret.clone().unwrap_or_default(),
                historique: chat::Historique::nouveau(&o.state_dir),
                moteur: etat.moteur.clone(),
                notes: o.state_dir.join("notes.jsonl"),
            };
            std::thread::spawn(move || {
                if let Err(e) = ecoute::servir(&ecoute, surface) {
                    eprintln!("selfd : surface HTTP arrêtée : {e:#}");
                }
            });
        }

        let partage = Arc::new(Mutex::new(etat));
        serveur::servir(&o.socket, partage, o.secret.clone())
    } else {
        let mut etat = Etat::nouveau(o.politique, id, "stdio", verrou, o.llm.clone(), historique);
        etat.notes = o.state_dir.join("notes.jsonl");
        if let Some(m) = &o.moteur {
            etat.moteur = m.clone();
        }
        boucle_stdio(&mut etat, o.secret.as_deref())
    }
}

/// La boucle stdio : EOF termine le processus, ce qui fait qu'un `ssh`
/// interrompu n'abandonne pas un nœud orphelin sur la machine distante.
fn boucle_stdio(etat: &mut Etat, secret: Option<&str>) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut sortie = stdout.lock();
    for ligne in stdin.lock().lines() {
        let Ok(ligne) = ligne else { break };
        if ligne.trim().is_empty() {
            continue;
        }
        let appel = match analyser(&ligne) {
            Ok(a) => a,
            Err(e) => {
                let _ = writeln!(
                    sortie,
                    "{}",
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e}})
                );
                let _ = sortie.flush();
                continue;
            }
        };
        let fin = appel.methode == "shutdown";
        if let Some(r) = repondre(etat, &appel, secret) {
            let _ = writeln!(sortie, "{r}");
            let _ = sortie.flush();
        }
        if fin {
            break;
        }
    }
    Ok(())
}

/// Rend un résultat : `status` en clair (c'est ce qu'un humain lit d'abord), le
/// reste en JSON.
fn afficher(methode: &str, v: &Value, json_flag: bool) {
    if json_flag || methode != "status" {
        println!(
            "{}",
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        );
        return;
    }
    let n = &v["node"];
    println!("nœud      : {}", n["nodeId"].as_str().unwrap_or("?"));
    println!(
        "machine   : {}",
        n["hostnameCourant"].as_str().unwrap_or("?")
    );
    if n["hostnameChange"].as_bool().unwrap_or(false) {
        println!(
            "            (identité créée sous « {} » — machine renommée depuis)",
            n["hostname"].as_str().unwrap_or("?")
        );
    }
    if let Some(b) = n["base"].as_str() {
        println!("base      : {b}");
    } else {
        println!("base      : (aucune — --base ou $SELFD_BASE)");
    }
    println!("atteint   : {}", v["mode"].as_str().unwrap_or("?"));
    // ── OÙ IL EST : le socle, et sa place dessus ────────────────────────────
    // Le statut disait qui le nœud est (identifiant, machine, racine) et pas du
    // tout OÙ il est : quel moteur il sert, s'il y est joignable, s'il s'y est
    // rattaché, sous quel compte. Un nœud qui agit sur une machine sans pouvoir
    // dire à quel socle il appartient n'est pas situé.
    if let Some(p) = v.get("place") {
        let joignable = p["moteur_joignable"].as_bool().unwrap_or(false);
        println!(
            "moteur    : {} {}",
            p["moteur"].as_str().unwrap_or("?"),
            if joignable {
                "· joignable"
            } else {
                "· INJOIGNABLE"
            }
        );
        if let Some(ailleurs) = p["moteur_trouve_ailleurs"].as_str() {
            println!("            un moteur répond en {ailleurs} — poser SELFD_MOTEUR={ailleurs}");
        }
        if let Some(pos) = p["position"].as_str() {
            println!("place     : {pos}");
        }
    }
    println!(
        "racine    : {}",
        v["politique"]["racine"].as_str().unwrap_or("?")
    );
    println!(
        "exec      : {}",
        if v["capabilities"]["exec"].as_bool().unwrap_or(false) {
            "AUTORISÉ"
        } else {
            "refusé (démarrer avec --allow-exec)"
        }
    );
    if let Some(u) = v["uptimeMs"].as_u64() {
        println!("en marche : {:.1} s", u as f64 / 1000.0);
    }
    let c = &v["charge"];
    if c.is_object() {
        println!("charge    : {} {} {}", c["1min"], c["5min"], c["15min"]);
    }
}
