//! Le démon local — une socket Unix, et la CLI qui lui parle.
//!
//! # Pourquoi une socket Unix, et pas un port
//!
//! Ce qui doit être atteignable, c'est **la CLI de la machine qui exécute le
//! nœud** — pas le réseau. Une socket Unix donne exactement cela, et rien de
//! plus :
//!
//! * **l'autorité vient des permissions du fichier**, pas d'un mot de passe
//!   qu'il faudrait distribuer. Mode `0600` : le propriétaire, et lui seul.
//!   C'est la doctrine déjà tenue par le CLI d'administration du moteur —
//!   *« l'autorité du CLI vient du système d'exploitation »* ;
//! * **rien n'est exposé sur le réseau** : un nœud n'ouvre aucun port, donc une
//!   machine compromise à côté ne peut pas lui parler. Le chemin distant reste
//!   `ssh` (ou MCP), explicitement ;
//! * **pas de port à choisir**, donc pas de collision avec le moteur (8432),
//!   les GUI (3082/3083) ni le LLM local (9901).
//!
//! # Un démon, plusieurs clients
//!
//! Chaque connexion est servie dans son propre fil et parle le même protocole
//! que stdio. Un client qui ferme ne dérange pas les autres ; le nœud ne meurt
//! pas avec son premier appelant — c'est ce qui distingue un nœud d'une
//! commande.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::protocole::{analyser, repondre, Etat};

/// Chemin par défaut de la socket.
///
/// Trois candidats, dans cet ordre — et **aucun appel système** :
///
/// 1. `$SELFD_SOCKET`, si l'opérateur l'a décidé ;
/// 2. `$XDG_RUNTIME_DIR/selfd.sock` : le répertoire que le système garantit privé
///    et effacé à la déconnexion ;
/// 3. `<état>/selfd.sock` (`~/.local/state/selfd/`) : déjà propre à
///    l'utilisateur, donc deux utilisateurs de la même machine ne se marchent
///    pas dessus sans qu'on ait à demander son uid au noyau.
///
/// Une première version appelait `getuid` pour construire `/tmp/selfd-<uid>.sock`.
/// C'était un `unsafe` — et le dépôt l'interdit (`-W unsafe-code`) à juste titre :
/// le répertoire d'état répond au même besoin sans franchir cette ligne.
pub fn socket_defaut() -> PathBuf {
    if let Ok(d) = std::env::var("SELFD_SOCKET") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(r) = std::env::var("XDG_RUNTIME_DIR") {
        if !r.is_empty() {
            return PathBuf::from(r).join("selfd.sock");
        }
    }
    crate::identite::state_dir_defaut().join("selfd.sock")
}

/// Sert la socket jusqu'à l'arrêt du processus.
pub fn servir(chemin: &Path, etat: Arc<Mutex<Etat>>, secret: Option<String>) -> Result<()> {
    // ── UNE SOCKET PÉRIMÉE SE REPREND, UNE SOCKET VIVANTE SE REFUSE ─────────
    // Un démon tué laisse son fichier derrière lui. Le supprimer sans regarder
    // volerait la socket d'un nœud qui tourne encore — et deux nœuds sur le
    // même chemin se croiraient chacun seul. On sonde donc : si quelqu'un
    // répond, on refuse de démarrer.
    if chemin.exists() {
        match UnixStream::connect(chemin) {
            Ok(_) => bail!(
                "un nœud écoute déjà sur {} — refus de le remplacer (arrêter l'autre, \
                 ou utiliser --socket <autre chemin>)",
                chemin.display()
            ),
            Err(_) => {
                std::fs::remove_file(chemin)
                    .with_context(|| format!("retrait de la socket périmée {}", chemin.display()))?;
            }
        }
    }
    if let Some(parent) = chemin.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let ecoute = UnixListener::bind(chemin)
        .with_context(|| format!("écoute sur {}", chemin.display()))?;
    // 0600 : le propriétaire seul. Posé APRÈS `bind` parce que le mode du
    // fichier n'existe qu'à partir de là — et avant d'accepter, pour qu'aucune
    // connexion ne soit servie pendant que la socket est encore ouverte.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(chemin, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("permissions de {}", chemin.display()))?;
    }

    // La réponse au client qui demande où est le nœud : c'est le PREMIER
    // endroit où un opérateur regarde, donc il doit dire la vérité utile.
    eprintln!(
        "selfd {} — nœud {} · socket {} · racine {} · exec {}",
        env!("CARGO_PKG_VERSION"),
        etat.lock().map(|e| e.identite.node_id.clone()).unwrap_or_default(),
        chemin.display(),
        etat.lock().map(|e| e.politique.racine.display().to_string()).unwrap_or_default(),
        if etat.lock().map(|e| e.politique.exec_autorise).unwrap_or(false) { "autorise" } else { "REFUSE" },
    );

    for flux in ecoute.incoming() {
        let Ok(flux) = flux else { continue };
        let etat = Arc::clone(&etat);
        let secret = secret.clone();
        // Un fil par client : un appel long (une commande de 60 s) ne doit pas
        // empêcher la CLI locale de demander `status` pendant ce temps.
        std::thread::spawn(move || {
            let _ = servir_client(flux, etat, secret.as_deref());
        });
    }
    Ok(())
}

/// Sert une connexion : la même boucle de lignes que stdio.
fn servir_client(flux: UnixStream, etat: Arc<Mutex<Etat>>, secret: Option<&str>) -> Result<()> {
    flux.set_read_timeout(None).ok();
    let lecteur = BufReader::new(flux.try_clone()?);
    let mut sortie = flux;
    for ligne in lecteur.lines() {
        let Ok(ligne) = ligne else { break };
        if ligne.trim().is_empty() {
            continue;
        }
        let reponse = match analyser(&ligne) {
            Ok(appel) => {
                let mut g = etat.lock().unwrap_or_else(|e| e.into_inner());
                repondre(&mut g, &appel, secret)
            }
            Err(e) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": e }
            })),
        };
        if let Some(r) = reponse {
            writeln!(sortie, "{r}")?;
            sortie.flush()?;
        }
    }
    Ok(())
}

/// La CLI locale : un appel, une réponse, on sort.
///
/// C'est ce qui rend le nœud atteignable **depuis le terminal de la machine qui
/// l'exécute** : `selfd status`, `selfd exec '{"cmd":"hostname"}'`.
pub fn appeler(
    chemin: &Path,
    methode: &str,
    params: &Value,
    delai: Duration,
    jeton: Option<&str>,
) -> Result<Value> {
    let flux = UnixStream::connect(chemin).map_err(|e| {
        anyhow::anyhow!(
            "aucun nœud n'écoute sur {} ({e}).\n\
             Démarrer le démon : selfd serve [--root <dir>] [--allow-exec]",
            chemin.display()
        )
    })?;
    flux.set_read_timeout(Some(delai)).ok();
    flux.set_write_timeout(Some(delai)).ok();
    let mut ecriture = flux.try_clone()?;
    let mut lecteur = BufReader::new(flux);

    // Le jeton, s'il y en a un, voyage DANS les paramètres de l'appel : la
    // socket n'a pas d'en-tête, et l'authentification reste sans état.
    let mut params = params.clone();
    if let (Some(t), Some(obj)) = (jeton, params.as_object_mut()) {
        obj.insert("token".to_string(), json!(t));
    }
    let appel = json!({ "jsonrpc": "2.0", "id": 1, "method": methode, "params": params });
    writeln!(ecriture, "{appel}")?;
    ecriture.flush()?;

    let mut ligne = String::new();
    lecteur.read_line(&mut ligne).context("lecture de la réponse du nœud")?;
    if ligne.trim().is_empty() {
        bail!("le nœud a fermé la connexion sans répondre");
    }
    let v: Value = serde_json::from_str(&ligne).context("réponse illisible du nœud")?;
    if let Some(e) = v.get("error") {
        bail!("{}", e.get("message").and_then(|m| m.as_str()).unwrap_or("erreur du nœud"));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}
