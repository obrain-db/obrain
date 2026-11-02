//! Les outils du nœud — ce par quoi il agit sur la machine.
//!
//! Trois principes, et ils viennent de défauts connus, pas du goût :
//!
//! 1. **Aucun shell implicite.** `exec` prend un programme et ses arguments. Une
//!    exécution qui passe par `/bin/sh -c` doit être demandée (`shell: true`) :
//!    interpoler sans le dire transforme une donnée en code.
//! 2. **On ne laisse jamais un enfant bloquer sur un tuyau plein.** La sortie est
//!    lue jusqu'à EOF par deux fils, et seule la tête est conservée au-delà du
//!    plafond. Cesser de lire pour économiser la mémoire figerait le processus
//!    enfant sur son `write` — le nœud paraîtrait alors en panne, sans erreur.
//! 3. **Un délai est un délai.** À l'échéance, l'enfant est tué et le résultat
//!    dit `timeout: true` — jamais un succès partiel déguisé.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::politique::Politique;

/// Lit un flux en gardant au plus `plafond` octets, mais en le DRAINANT jusqu'à
/// EOF. Rend (octets gardés, total lu, tronqué).
fn lire_borne(mut flux: impl Read, plafond: usize) -> (Vec<u8>, usize, bool) {
    let mut garde: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut tampon = [0u8; 8192];
    loop {
        match flux.read(&mut tampon) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if garde.len() < plafond {
                    let reste = plafond - garde.len();
                    garde.extend_from_slice(&tampon[..n.min(reste)]);
                }
            }
            Err(_) => break,
        }
    }
    let tronque = total > garde.len();
    (garde, total, tronque)
}

/// Exécute une commande sous la politique du nœud.
pub fn exec(p: &Politique, args: &serde_json::Value) -> Result<serde_json::Value> {
    if !p.exec_autorise {
        bail!(
            "exécution NON AUTORISÉE sur ce nœud : il a été démarré sans --allow-exec. \
             Un nœud qui n'a pas été autorisé à agir observe ; il n'agit pas."
        );
    }
    let Some(prog) = args.get("cmd").and_then(|v| v.as_str()) else {
        bail!("exec : « cmd » requis (le programme, pas une ligne de shell)");
    };
    let liste: Vec<String> = args
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let shell = args.get("shell").and_then(|v| v.as_bool()).unwrap_or(false);
    let cwd = match args.get("cwd").and_then(|v| v.as_str()) {
        Some(c) => Some(p.resoudre(c)?),
        None => None,
    };
    let delai = Duration::from_millis(
        args.get("timeoutMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(p.delai_ms),
    );

    // `shell: true` est un acte EXPLICITE : c'est le seul chemin où une chaîne
    // fournie par un client devient du code exécuté.
    let mut commande = if shell {
        let ligne = std::iter::once(prog.to_string())
            .chain(liste.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(ligne);
        c
    } else {
        let mut c = Command::new(prog);
        c.args(&liste);
        c
    };

    if let Some(d) = &cwd {
        commande.current_dir(d);
    }
    let debut = Instant::now();
    let mut enfant = commande
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("démarrage de « {prog} »"))?;

    let sortie = enfant.stdout.take().expect("stdout pipé");
    let erreur = enfant.stderr.take().expect("stderr pipé");
    let cap = p.plafond_octets;
    let f_sortie = thread::spawn(move || lire_borne(sortie, cap));
    let f_erreur = thread::spawn(move || lire_borne(erreur, cap));

    // Attente SONDÉE : `std::process` n'offre pas de délai d'attente, et
    // `wait()` bloquerait indéfiniment — exactement ce qu'un nœud distant ne
    // doit pas faire.
    let mut tue = false;
    let statut = loop {
        if let Some(s) = enfant.try_wait().context("attente de l'enfant")? {
            break Some(s);
        }
        if debut.elapsed() >= delai {
            let _ = enfant.kill();
            let _ = enfant.wait();
            tue = true;
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };

    let (o, o_total, o_tronque) = f_sortie.join().unwrap_or_default();
    let (e, e_total, e_tronque) = f_erreur.join().unwrap_or_default();
    let duree = debut.elapsed().as_millis() as u64;

    // Sur Unix, un enfant tué par un signal n'a pas de code de sortie : rendre
    // `null` sans le dire ferait passer une mort violente pour une fin propre.
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        statut.and_then(|s| s.signal())
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;

    Ok(serde_json::json!({
        "exitCode": statut.and_then(|s| s.code()),
        "signal": signal,
        "timeout": tue,
        "durationMs": duree,
        "stdout": String::from_utf8_lossy(&o),
        "stderr": String::from_utf8_lossy(&e),
        "stdoutBytes": o_total,
        "stderrBytes": e_total,
        "tronque": o_tronque || e_tronque,
    }))
}

/// Lit un fichier de la machine.
pub fn read(p: &Politique, args: &serde_json::Value) -> Result<serde_json::Value> {
    let Some(chemin) = args.get("path").and_then(|v| v.as_str()) else {
        bail!("read : « path » requis");
    };
    let cible = p.resoudre(chemin)?;
    let max = args
        .get("maxBytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(p.plafond_octets as u64) as usize;
    let flux =
        std::fs::File::open(&cible).with_context(|| format!("ouverture de {}", cible.display()))?;
    let (gardes, total, tronque) = lire_borne(flux, max);
    Ok(serde_json::json!({
        "path": cible.display().to_string(),
        "bytes": total,
        "tronque": tronque,
        "content": String::from_utf8_lossy(&gardes),
    }))
}

/// Écrit un fichier sur la machine.
pub fn write(p: &Politique, args: &serde_json::Value) -> Result<serde_json::Value> {
    let Some(chemin) = args.get("path").and_then(|v| v.as_str()) else {
        bail!("write : « path » requis");
    };
    let Some(contenu) = args.get("content").and_then(|v| v.as_str()) else {
        bail!("write : « content » requis");
    };
    let cible = p.resoudre(chemin)?;
    if let Some(parent) = cible.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("création de {}", parent.display()))?;
    }
    std::fs::write(&cible, contenu).with_context(|| format!("écriture de {}", cible.display()))?;
    Ok(serde_json::json!({
        "path": cible.display().to_string(),
        "bytes": contenu.len(),
    }))
}

/// Liste un répertoire.
pub fn list(p: &Politique, args: &serde_json::Value) -> Result<serde_json::Value> {
    let demande = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let cible = p.resoudre(demande)?;
    let mut entrees = Vec::new();
    for e in std::fs::read_dir(&cible).with_context(|| format!("lecture de {}", cible.display()))? {
        let e = e?;
        let md = e.metadata().ok();
        entrees.push(serde_json::json!({
            "name": e.file_name().to_string_lossy(),
            "kind": if md.as_ref().map(|m| m.is_dir()).unwrap_or(false) { "dir" } else { "file" },
            "size": md.as_ref().map(|m| m.len()).unwrap_or(0),
        }));
    }
    entrees.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(serde_json::json!({
        "path": cible.display().to_string(),
        "count": entrees.len(),
        "entries": entrees,
    }))
}

/// Rend l'identité et la politique effectives du nœud.
///
/// C'est la réponse à « sur quel nœud suis-je, et que m'autorise-t-il ? ». Elle
/// porte l'identité **persistée** (`nodeId` stable) *et* le nom d'hôte courant,
/// parce que les deux peuvent diverger : une machine renommée garde son
/// identifiant, et le taire ferait chercher longtemps pourquoi un nœud s'appelle
/// comme une machine qui n'existe plus.
pub fn identite(p: &Politique, id: &crate::identite::Identite, mode: &str) -> serde_json::Value {
    serde_json::json!({
        "serverInfo": { "name": "selfd", "version": env!("CARGO_PKG_VERSION") },
        "node": id.en_json(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "pid": std::process::id(),
        "mode": mode,
        "capabilities": {
            "exec": p.exec_autorise,
            "fsRead": true,
            "fsWrite": true,
            "list": true,
        },
        "politique": p.en_json(),
    })
}

/// L'état de marche du nœud : identité, disponibilité de la base commune, temps
/// écoulé. C'est ce qu'un opérateur veut lire d'un `selfd status` local.
pub fn statut(
    p: &Politique,
    id: &crate::identite::Identite,
    debut: std::time::Instant,
    mode: &str,
    moteur: &str,
) -> serde_json::Value {
    let mut v = identite(p, id, mode);
    v["uptimeMs"] = serde_json::json!(debut.elapsed().as_millis() as u64);
    v["charge"] = charge();
    v["place"] = place(moteur);
    v
}

/// Y a-t-il un moteur AILLEURS, sur CETTE machine ?
///
/// On essaie les adresses par lesquelles le moteur D'ICI est joignable : la
/// boucle locale, l'adresse primaire de la machine, son nom d'hôte. La première
/// qui répond gagne, et son URL est rendue.
///
/// # Comment l'adresse primaire est obtenue, sans dépendance
///
/// Un `connect` **UDP** vers une adresse publique ne fait partir AUCUN paquet :
/// il demande au noyau **quelle adresse source il choisirait** pour cette
/// destination. C'est la seule façon d'obtenir l'adresse primaire de la machine
/// sans dépendance, sans résolution DNS et sans lancer un sous-processus
/// (`hostname -I` est un programme, et un nœud ne suppose pas le PATH).
///
/// Rien n'est envoyé à l'extérieur : c'est une question posée au noyau local.
fn moteur_a_proximite(configure: &str) -> Option<String> {
    let mut candidats: Vec<String> = Vec::new();
    candidats.push("http://127.0.0.1:8432".to_string());
    if let Some(ip) = ip_primaire() {
        candidats.push(format!("http://{ip}:8432"));
    }
    let hote = crate::identite::nom_hote();
    if !hote.is_empty() && hote != "inconnu" {
        candidats.push(format!("http://{hote}:8432"));
    }
    for c in candidats {
        if c.trim_end_matches('/') == configure {
            continue; // déjà essayé, et il n'a pas répondu
        }
        if let Ok(r) = obrain_httpmin::get(&format!("{c}/api/harness/health"), &[], None) {
            if r.status < 500 {
                return Some(c);
            }
        }
    }
    None
}

/// L'adresse primaire de cette machine, telle que le noyau la choisirait.
///
/// Voir `moteur_a_proximite` : un `connect` UDP ne transmet rien, il interroge
/// la table de routage. Rend `None` quand la machine n'a aucune route (un
/// conteneur `--network none`, par exemple) — et c'est une réponse honnête.
fn ip_primaire() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("1.1.1.1:53").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// OÙ CE NŒUD EST — sa PLACE dans la flotte, découverte et non déclarée.
///
/// # Le manque que ce bloc comble
///
/// Le nœud disait qui il est (son identifiant, sa machine, sa racine) et **pas du
/// tout où il est** : rien ne nommait le moteur qu'il sert, ni s'il y est
/// joignable, ni s'il s'y est rattaché, ni sous quel compte. Un nœud qui agit sur
/// une machine sans pouvoir dire à quel socle il appartient n'est pas situé — et
/// c'est pourtant ce qu'on lui demande : « qu'il sache instinctivement qui il est
/// **où il est** ».
///
/// # Ce qui est DÉCOUVERT, et ce qui est DIT comme inconnu
///
/// Le moteur vient de `--moteur`, sinon `$SELFD_MOTEUR`, sinon `127.0.0.1:8432` —
/// la MÊME découverte que `soi.rs` utilise pour lire le self : une seule vérité,
/// deux lecteurs. La joignabilité est SONDÉE (courte), parce qu'annoncer un
/// moteur qu'on ne joint pas serait annoncer sans décider.
///
/// La POSITION (rattaché, sous quel compte) demande un jeton : sans lui, le
/// moteur ne parle qu'à une identité — et le nœud le DIT au lieu de laisser croire
/// qu'il ne s'est pas rattaché. C'est la distinction que ce projet applique
/// partout : « je ne peux pas savoir » n'est pas « non ».
fn place(moteur: &str) -> serde_json::Value {
    let base = moteur.trim_end_matches('/');
    // Le jeton sert UNIQUEMENT à interroger le moteur : il n'est jamais RENDU.
    let jeton = std::env::var("SELFD_MOTEUR_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());

    let sante = obrain_httpmin::get(&format!("{base}/api/harness/health"), &[], None);
    let joignable = sante.as_ref().map(|r| r.status < 500).unwrap_or(false);
    let mut v = serde_json::json!({
        "moteur": base,
        "moteur_joignable": joignable,
        "moteur_reponse": sante.as_ref().map(|r| r.status).unwrap_or(0),
    });

    // ── LE MOTEUR N'EST PAS LÀ OÙ ON LE CHERCHE : ON CHERCHE OÙ IL EST ──────
    // Défaut mesuré le 2026-09-16, sur la machine même : le défaut codé est
    // `127.0.0.1:8432`, or le moteur écoute sur l'adresse du RÉSEAU
    // (IP-INTERNE:8432) et ne répond PAS sur la boucle locale. Un nœud sans
    // `SELFD_MOTEUR` ne trouvait donc pas le moteur de sa propre machine, et
    // annonçait « injoignable » — vrai, mais inutile.
    //
    // On CHERCHE, et on DIT ce qu'on a trouvé. On ne BASCULE PAS : la politique
    // et le socle sont fixés au démarrage et jamais négociés (c'est la propriété
    // qui fait qu'un moteur ne peut pas desserrer un nœud). Changer d'adresse
    // dans le dos de l'exploitant serait décider à sa place — on annonce, et on
    // donne la commande qui répare.
    if !joignable {
        if let Some(trouve) = moteur_a_proximite(base) {
            v["moteur_trouve_ailleurs"] = serde_json::json!(&trouve);
            v["moteur_conseille"] = serde_json::json!(format!(
                "un moteur répond en {trouve} sur CETTE machine. Le socle ne se change pas tout \
                 seul : posez SELFD_MOTEUR={trouve} (ou --moteur {trouve}) et redémarrez le nœud."
            ));
        }
    }

    match jeton {
        None => {
            v["position"] = serde_json::json!(
                "inconnue — aucun jeton (SELFD_MOTEUR_TOKEN) : le moteur ne parle qu'à une \
                 identité, donc ce nœud ne peut pas savoir s'il est rattaché. Ce n'est PAS \
                 « non rattaché »."
            );
            v["position_connue"] = serde_json::json!(false);
        }
        Some(t) => {
            // On demande au moteur SA vue de cette machine. Le nom d'hôte est celui
            // que le nœud publie déjà (`identite`) : on ne le recalcule pas ici —
            // deux calculs pour une même vérité finissent par se contredire.
            let nom = crate::identite::nom_hote();
            let q = [("token", t.as_str())];
            let r = obrain_httpmin::get(&format!("{base}/api/remote/machines"), &q, None);
            v["position_connue"] = serde_json::json!(true);
            match r {
                Ok(r) if r.status == 200 => {
                    let corps = r.json().unwrap_or(serde_json::Value::Null);
                    let vide = Vec::new();
                    let machines = corps
                        .get("machines")
                        .and_then(|m| m.as_array())
                        .unwrap_or(&vide);
                    let mienne = machines
                        .iter()
                        .find(|m| m.get("machine").and_then(|x| x.as_str()) == Some(nom.as_str()));
                    match mienne {
                        Some(m) => {
                            let vivante =
                                m.get("vivant").and_then(|x| x.as_bool()).unwrap_or(false);
                            v["position"] = serde_json::json!(format!(
                                "RATTACHÉE sous le compte « {} » — le moteur la voit{}",
                                m.get("compte").and_then(|c| c.as_str()).unwrap_or("?"),
                                if vivante {
                                    ", et elle est joignable"
                                } else {
                                    ", mais elle ne répond pas encore"
                                }
                            ));
                            v["rattachee"] = serde_json::json!(true);
                            v["compte"] =
                                m.get("compte").cloned().unwrap_or(serde_json::Value::Null);
                            v["capacites_vues_par_le_moteur"] = m
                                .get("capacites")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                        }
                        None => {
                            v["position"] = serde_json::json!(format!(
                                "PAS encore rattachée : le moteur répond, mais ne connaît aucune \
                                 machine nommée « {nom} » sous ce compte ({} machine(s) connue(s)). \
                                 Le rattachement est une connexion SORTANTE : `selfd attacher`.",
                                machines.len()
                            ));
                            v["rattachee"] = serde_json::json!(false);
                        }
                    }
                }
                Ok(r) if r.status == 401 || r.status == 403 => {
                    v["position"] = serde_json::json!(
                        "inconnue — le moteur a REFUSÉ le jeton : il ne peut donc pas dire où ce \
                         nœud en est. Vérifier le jeton de VOTRE compte."
                    );
                    v["position_connue"] = serde_json::json!(false);
                }
                _ => {
                    v["position"] = serde_json::json!(
                        "inconnue — le moteur est injoignable : ce nœud ne peut pas savoir s'il est \
                         rattaché. Il continue de travailler sans socle, et le dit."
                    );
                    v["position_connue"] = serde_json::json!(false);
                }
            }
        }
    }
    v
}

/// Charge système en 1/5/15 minutes, si le noyau la donne.
///
/// Lue dans `/proc/loadavg` plutôt que par une dépendance : un nœud dont on
/// veut savoir s'il est occupé n'a pas besoin d'une bibliothèque pour cela.
fn charge() -> serde_json::Value {
    let Ok(txt) = std::fs::read_to_string("/proc/loadavg") else {
        return serde_json::Value::Null;
    };
    let champs: Vec<&str> = txt.split_whitespace().collect();
    if champs.len() < 3 {
        return serde_json::Value::Null;
    }
    serde_json::json!({
        "1min": champs[0].parse::<f64>().ok(),
        "5min": champs[1].parse::<f64>().ok(),
        "15min": champs[2].parse::<f64>().ok(),
    })
}
