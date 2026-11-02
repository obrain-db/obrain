//! La surface MCP — ce qui permet de **brancher un nœud sur le harness**.
//!
//! # Pourquoi MCP, et pourquoi ce n'est pas un détour
//!
//! Le harness monte déjà des serveurs MCP par **configuration**
//! (`packages/mcp/mcp-client`) : transport `stdio` avec un `command` et des
//! `args` arbitraires, ou `streamable-http` avec une URL. Il n'y a donc **rien à
//! écrire dans le harness** pour brancher une machine : il suffit que `selfd`
//! parle MCP.
//!
//! ```yaml
//! - id: mcp-n3
//!   name: '@deepseek-ai/dsh-mcp-client'
//!   config:
//!     transport: stdio
//!     command: ssh                       # le transport, c'est ssh
//!     args: ['n3', '/opt/selfd/selfd', 'mcp', '--root', '/srv', '--allow-exec']
//! ```
//!
//! Le harness croit lancer un serveur MCP local ; c'est une machine distante.
//! C'est exactement la même ruse que pour les sous-agents : **le transport est
//! un exécutable**, et `ssh` en est un.
//!
//! # Le cadrage
//!
//! JSON-RPC 2.0, **une trame par ligne** — ce que le SDK MCP emploie sur stdio
//! (`StdioClientTransport`). C'est le même cadrage que le reste de `selfd` : rien
//! à adapter, la boucle de lecture est identique.
//!
//! # La confiance, en mode MCP
//!
//! Le client MCP **est celui qui a lancé le processus** : il a déjà le droit de
//! faire tourner un programme sur la machine, donc exiger de lui un jeton
//! n'ajouterait pas de frontière — ce serait une cérémonie. La frontière, ici,
//! est le lanceur (configuration du harness, accès `ssh`, permissions du
//! fichier). Le jeton garde tout son sens sur les modes où un client se
//! **connecte** à un nœud déjà vivant (socket, stdio) ; en MCP il n'est pas
//! transmis par appel, et le dire vaut mieux que de laisser croire à une garde
//! qui ne garde rien.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{Value, json};

use crate::protocole::{Etat, analyser, executer_outil};

/// Version du protocole annoncée quand le client n'en propose aucune.
/// On **renvoie celle du client** quand il en propose une : refuser une version
/// qu'on sait servir sur un simple numéro serait une rigidité gratuite.
const VERSION_DEFAUT: &str = "2025-06-18";

/// Les outils exposés au harness.
///
/// Les noms sont ceux qu'un modèle lira. Ils décrivent des actes sur **la
/// machine où le nœud tourne**, pas des appels d'API : d'où le vocabulaire
/// direct, et la mention de la politique dans chaque description — un modèle qui
/// sait qu'`exec` peut être refusé n'insiste pas.
pub fn outils() -> Value {
    json!([
        {
            "name": "exec",
            "description": "Exécute un programme SUR LA MACHINE DE CE NŒUD et rend son code de sortie, sa sortie standard et sa sortie d'erreur. \
                            Aucun shell n'est utilisé sauf si `shell` est vrai. Peut être refusé si le nœud a été démarré sans --allow-exec.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "Le programme à exécuter (ex. `uname`), pas une ligne de shell." },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments, passés tels quels." },
                    "cwd": { "type": "string", "description": "Répertoire de travail, sous la racine du nœud." },
                    "shell": { "type": "boolean", "description": "Exécuter via /bin/sh -c (à demander explicitement)." },
                    "timeoutMs": { "type": "integer", "description": "Délai avant mise à mort." }
                },
                "required": ["cmd"]
            }
        },
        {
            "name": "read",
            "description": "Lit un fichier sur la machine de ce nœud. Le chemin doit rester sous la racine du nœud.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "maxBytes": { "type": "integer" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "write",
            "description": "Écrit un fichier sur la machine de ce nœud, en créant les répertoires parents. Le chemin doit rester sous la racine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "list",
            "description": "Liste un répertoire de la machine de ce nœud.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }
        },
        {
            "name": "node_chat",
            "description": "Pose une question AU NŒUD LUI-MÊME : sa réponse vient de son identité, de sa politique et de son état local — jamais du graphe global ni du self.                             À utiliser pour lui demander où il en est, ce qu'il voit de sa machine, ce qu'il a le droit de faire.",
            "inputSchema": {
                "type": "object",
                "properties": { "message": { "type": "string", "description": "La question posée au nœud." } },
                "required": ["message"]
            }
        },
        {
            "name": "node_run",
            "description": "CONFIE UN OBJECTIF au harness de ce nœud : il déroule une boucle (le modèle décide, le nœud exécute, il recommence) ancrée sur le self, et rend sa réponse avec le détail de ses actes. \
                            C'est ce qui fait d'un nœud un harness : on ne lui dicte pas une commande, on lui donne un but.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objectif": { "type": "string", "description": "Le but à atteindre sur cette machine." },
                    "maxSteps": { "type": "integer", "description": "Budget d'étapes (défaut 12, max 40)." }
                },
                "required": ["objectif"]
            }
        },
        {
            "name": "node_status",
            "description": "Identité du nœud (nodeId stable, nom d'hôte, système), politique effective et charge. À lire avant d'agir pour savoir sur quelle machine on travaille.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

/// Traduit un appel `tools/call` vers l'outil natif correspondant.
fn methode_native(nom: &str) -> Option<&'static str> {
    match nom {
        "exec" => Some("exec"),
        "read" => Some("read"),
        "write" => Some("write"),
        "list" => Some("list"),
        "node_chat" => Some("chat"),
        "node_run" => Some("run"),
        "node_status" => Some("status"),
        _ => None,
    }
}

/// Une réponse MCP de succès : le contenu est du TEXTE, et c'est du JSON.
///
/// Le modèle lit du texte : rendre un objet structuré obligerait le client à le
/// sérialiser, et chaque client le ferait autrement. Le JSON en texte est la
/// seule forme qui traverse tous les clients sans convention.
fn reponse_texte(valeur: &Value, erreur: bool) -> Value {
    let texte = serde_json::to_string_pretty(valeur).unwrap_or_else(|_| valeur.to_string());
    let mut r = json!({ "content": [{ "type": "text", "text": texte }] });
    if erreur {
        r["isError"] = json!(true);
    }
    r
}

/// La boucle MCP : une trame JSON-RPC par ligne sur stdin/stdout.
pub fn boucle(etat: &mut Etat, _secret: Option<&str>) -> Result<()> {
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

        // Une notification ne reçoit jamais de réponse — pas même un accusé.
        let id = match appel.id.clone() {
            Some(i) => i,
            None => continue,
        };

        let reponse = match appel.methode.as_str() {
            "initialize" => json!({
                "protocolVersion": appel
                    .params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or(VERSION_DEFAUT),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "selfd", "version": env!("CARGO_PKG_VERSION") },
            }),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": outils() }),
            "tools/call" => {
                let nom = appel
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let arguments = appel
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match methode_native(nom) {
                    Some(m) => {
                        // Le nœud en mode MCP n'est pas verrouillé : le lanceur
                        // est la frontière (voir l'en-tête de ce module).
                        match executer_outil(etat, m, &arguments, None) {
                            Ok(v) => reponse_texte(&v, false),
                            // Un refus est un RÉSULTAT, pas une erreur de
                            // protocole : `isError` le dit au modèle, qui peut
                            // alors changer d'approche au lieu de croire le
                            // serveur cassé.
                            Err(e) => reponse_texte(
                                &json!({ "error": e.get("message").cloned().unwrap_or(e) }),
                                true,
                            ),
                        }
                    }
                    None => {
                        reponse_texte(&json!({ "error": format!("outil inconnu : {nom}") }), true)
                    }
                }
            }
            autre => {
                json!({
                    "error": { "code": -32601, "message": format!("méthode inconnue : {autre}") }
                })
            }
        };

        let enveloppe = json!({ "jsonrpc": "2.0", "id": id, "result": reponse });
        let _ = writeln!(sortie, "{enveloppe}");
        let _ = sortie.flush();
    }
    Ok(())
}
