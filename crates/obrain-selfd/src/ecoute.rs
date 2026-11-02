//! La surface HTTP du nœud — sa page de chat, et les routes qui la servent.
//!
//! # Pourquoi HTTP ici, alors que la socket Unix suffit en local
//!
//! Les deux surfaces ne servent pas la même chose, et les confondre serait une
//! faute :
//!
//! * la **socket Unix** est l'interface de la machine : son autorité est le
//!   système d'exploitation (permissions `0600`), elle ne s'expose pas et ne
//!   demande donc aucun secret ;
//! * la **surface HTTP** est faite pour être atteinte **d'ailleurs** — un
//!   navigateur, un autre poste, un pilotage supérieur. Là, il n'y a plus de
//!   permissions de fichier pour tenir la porte : il faut un **jeton**.
//!
//! # Deux règles, et elles sont fail-closed
//!
//! 1. **Pas de jeton, pas de surface.** Sans `--token` (ou `$SELFD_TOKEN`), le
//!    nœud **refuse de démarrer** l'écouteur HTTP. Un chat de machine exposé sans
//!    authentification est une porte ouverte sur l'exécution de commandes, et
//!    « je n'y avais pas pensé » n'est pas une politique.
//! 2. **Écoute locale par défaut.** `127.0.0.1` sauf `--chat-host` explicite.
//!    Exposer est un acte, pas un défaut.
//!
//! # Ce qui est public, et ce qui ne l'est pas
//!
//! `GET /` rend la **page** sans authentification : c'est un formulaire de
//! connexion, il doit pouvoir s'afficher. Elle ne contient aucune donnée du nœud.
//! Tout ce qui **révèle** ou **agit** (`/api/status`, `/api/chat`) exige le
//! jeton, comparé en temps constant.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use obrain_garde::{Garde, Motif, Reglages};
use serde_json::{Value, json};

use crate::agent;
use crate::chat;
use crate::identite::Identite;
use crate::politique::Politique;
use crate::protocole::secret_egal;

/// Ce qu'il faut pour servir la surface.
pub struct Ecoute {
    pub host: String,
    pub port: u16,
}

/// L'état partagé par toutes les connexions HTTP.
pub struct Surface {
    pub politique: Politique,
    pub identite: Identite,
    pub debut: Instant,
    pub llm: String,
    pub secret: String,
    pub historique: chat::Historique,
    /// URL du moteur : le socle que le harness sert.
    pub moteur: String,
    /// Notes locales du nœud (le self n'accepte pas d'écriture).
    pub notes: PathBuf,
}

const TYPE_HTML: &str = "text/html; charset=utf-8";
const TYPE_JSON: &str = "application/json; charset=utf-8";
const TYPE_SSE: &str = "text/event-stream; charset=utf-8";

/// Une requête HTTP analysée.
struct Requete {
    methode: String,
    chemin: String,
    entetes: Vec<(String, String)>,
    corps: String,
}

impl Requete {
    fn entete(&self, nom: &str) -> Option<&str> {
        self.entetes
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(nom))
            .map(|(_, v)| v.as_str())
    }

    /// Le jeton présenté, s'il y en a un.
    fn jeton(&self) -> Option<&str> {
        let brut = self.entete("authorization")?;
        brut.strip_prefix("Bearer ")
            .or_else(|| brut.strip_prefix("bearer "))
            .map(str::trim)
    }
}

/// Lit une requête complète depuis la connexion.
fn lire_requete(flux: &mut TcpStream) -> Result<Requete> {
    let mut lecteur = BufReader::new(flux.try_clone()?);
    let mut ligne = String::new();
    lecteur.read_line(&mut ligne).context("ligne de requête")?;
    let mut mots = ligne.split_whitespace();
    let methode = mots.next().unwrap_or_default().to_string();
    let chemin = mots.next().unwrap_or("/").to_string();

    let mut entetes = Vec::new();
    let mut longueur = 0usize;
    loop {
        let mut h = String::new();
        if lecteur.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((n, v)) = h.split_once(':') {
            let (n, v) = (n.trim().to_string(), v.trim().to_string());
            if n.eq_ignore_ascii_case("content-length") {
                longueur = v.parse().unwrap_or(0);
            }
            entetes.push((n, v));
        }
    }
    // Borne dure sur le corps : accepter une longueur annoncée par le client sans
    // plafond est une façon simple de faire tomber un nœud.
    let longueur = longueur.min(1 << 20);
    let mut tampon = vec![0u8; longueur];
    let mut lu = 0;
    while lu < longueur {
        let n = lecteur
            .read(&mut tampon[lu..])
            .context("corps de la requête")?;
        if n == 0 {
            break;
        }
        lu += n;
    }
    Ok(Requete {
        methode,
        chemin,
        entetes,
        corps: String::from_utf8_lossy(&tampon[..lu]).to_string(),
    })
}

/// Écrit une réponse complète.
fn repondre(flux: &mut TcpStream, statut: u16, type_mime: &str, corps: &str) -> Result<()> {
    let raison = match statut {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let entete = format!(
        "HTTP/1.1 {statut} {raison}\r\nContent-Type: {type_mime}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        corps.len()
    );
    flux.write_all(entete.as_bytes())?;
    flux.write_all(corps.as_bytes())?;
    flux.flush()?;
    Ok(())
}

/// Écrit une trame SSE **entière**, en un seul `write_all`.
///
/// Une trame découpée en plusieurs écritures peut être entrelacée par le
/// battement d'un autre fil — et comme une ligne vide TERMINE un événement, un
/// fragment glissé au mauvais endroit couperait la trame. Le défaut serait
/// invisible : le flux resterait bien formé, il manquerait juste des mots.
fn pousser(flux: &mut TcpStream, nom: &str, donnees: &Value) -> Result<()> {
    let trame = format!("event: {nom}\ndata: {donnees}\n\n");
    flux.write_all(trame.as_bytes())?;
    flux.flush()?;
    Ok(())
}

/// La page de chat — servie sans authentification (c'est un formulaire).
fn page(s: &Surface) -> String {
    // Le nom du nœud est injecté pour que l'écran dise À QUI on parle : sans
    // cela, un opérateur qui a trois nœuds ouverts ne saurait pas lequel répond.
    format!(
        r##"<!doctype html>
<html lang="fr"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>nœud {node}</title>
<style>
 :root {{ color-scheme: dark }}
 body {{ margin:0; font:14px/1.5 ui-monospace,monospace; background:#111; color:#ddd }}
 header {{ padding:10px 14px; background:#1b1b1b; border-bottom:1px solid #333;
           display:flex; gap:12px; align-items:baseline; flex-wrap:wrap }}
 .id {{ color:#7fd; font-weight:600 }}
 .dim {{ color:#888 }}
 main {{ padding:14px; max-width:900px }}
 #journal {{ white-space:pre-wrap; min-height:200px }}
 .q {{ color:#7cf }} .r {{ color:#ddd }}
 #bar {{ display:flex; gap:8px; margin-top:12px }}
 input,button,textarea {{ font:inherit; background:#1b1b1b; color:#ddd;
   border:1px solid #333; border-radius:6px; padding:8px }}
 textarea {{ flex:1; min-height:56px; resize:vertical }}
 button {{ cursor:pointer }} button:disabled {{ opacity:.5; cursor:default }}
 .err {{ color:#f88 }}
</style></head><body>
<header>
  <span>nœud <span class="id">{node}</span></span>
  <span class="dim">{hote} · {os}/{arch}</span>
  <span class="dim">racine {racine} · exec {exec}</span>
</header>
<main>
  <div id="jeton" style="margin-bottom:12px">
    <input id="tok" type="password" placeholder="jeton du nœud" style="width:320px">
    <button onclick="garder()">garder</button>
    <span class="dim">— mémorisé dans ce navigateur, jamais envoyé ailleurs</span>
  </div>
  <div id="journal"></div>
  <div id="bar">
    <textarea id="q" placeholder="Parler à CE nœud ({node}) — Entrée envoie"></textarea>
    <button id="ok" onclick="envoyer()">envoyer</button>
    <label class="dim" style="align-self:center">
      <input type="checkbox" id="agir"> mode harness (agit)
    </label>
  </div>
  <p class="dim">Ce chat répond de la place du nœud : ni graphe global, ni self.
  Pour AGIR sur la machine, c'est la surface MCP (exec), gouvernée par la politique.</p>
</main>
<script>
const j = document.getElementById('journal'), tok = document.getElementById('tok');
tok.value = localStorage.getItem('selfd_token') || '';
function garder() {{ localStorage.setItem('selfd_token', tok.value); }}
function dire(classe, texte) {{
  const d = document.createElement('div'); d.className = classe; d.textContent = texte;
  j.appendChild(d); window.scrollTo(0, document.body.scrollHeight); return d;
}}
const q = document.getElementById('q');
q.addEventListener('keydown', e => {{
  if (e.key === 'Enter' && !e.shiftKey) {{ e.preventDefault(); envoyer(); }}
}});
async function envoyer() {{
  const texte = q.value.trim(); if (!texte) return;
  garder(); q.value = ''; document.getElementById('ok').disabled = true;
  dire('q', '› ' + texte);
  const cible = dire('r', '‹ ');
  try {{
    const agir = document.getElementById('agir').checked;
    const r = await fetch(agir ? '/api/run' : '/api/chat', {{
      method: 'POST',
      headers: {{ 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + tok.value }},
      body: JSON.stringify(agir ? {{ objectif: texte }} : {{ message: texte }}),
    }});
    if (!r.ok) {{ cible.className = 'err'; cible.textContent = '‹ HTTP ' + r.status + ' — ' + (await r.text()); return; }}
    const lecteur = r.body.getReader(), dec = new TextDecoder();
    let tampon = '', nom = '';
    for (;;) {{
      const {{ value, done }} = await lecteur.read(); if (done) break;
      tampon += dec.decode(value, {{ stream: true }});
      const lignes = tampon.split('\n'); tampon = lignes.pop();
      for (const l of lignes) {{
        if (l.startsWith('event:')) nom = l.slice(6).trim();
        else if (l.startsWith('data:')) {{
          const d = JSON.parse(l.slice(5).trim());
          if (nom === 'texte') cible.textContent += d.t;
          else if (nom === 'acte') dire('q', '  ⟳ ' + d.outil + ' ' + d.argument);
          else if (nom === 'rend') dire('dim', '    ' + (d.ok ? '✓' : '✗') + ' ' + d.apercu);
          else if (nom === 'socle') dire('dim', '  socle : ' + (d.joignable ? d.self : 'INJOIGNABLE'));
          else if (nom === 'borne') dire('err', '  ⚠ budget épuisé après ' + d.etapes + ' étapes');
          else if (nom === 'erreur') {{ cible.className = 'err'; cible.textContent += ' [' + d.error + ']'; }}
        }}
      }}
    }}
  }} catch (e) {{ cible.className = 'err'; cible.textContent = '‹ ' + e.message; }}
  finally {{ document.getElementById('ok').disabled = false; q.focus(); }}
}}
tok.addEventListener('change', garder);
q.focus();
</script></body></html>"##,
        node = s.identite.node_id,
        hote = s.identite.hostname_courant,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        racine = s.politique.racine.display(),
        exec = if s.politique.exec_autorise {
            "autorisé"
        } else {
            "refusé"
        },
    )
}

/// Traite une connexion.
fn traiter(mut flux: TcpStream, s: Arc<Surface>) -> Result<()> {
    let req = match lire_requete(&mut flux) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let chemin = req.chemin.split('?').next().unwrap_or("/").to_string();

    // La page est le seul contenu public : c'est le formulaire de connexion.
    if req.methode == "GET" && (chemin == "/" || chemin == "/index.html") {
        return repondre(&mut flux, 200, TYPE_HTML, &page(&s));
    }

    // ── LA GARDE, EN UN SEUL POINT ──────────────────────────────────────────
    // Toute route qui RÉVÈLE ou AGIT passe par ici, et une route ajoutée demain
    // l'hérite sans qu'on ait à y penser. La placer sur chaque route serait la
    // laisser s'oublier sur la prochaine.
    let autorise = req
        .jeton()
        .map(|t| secret_egal(t, &s.secret))
        .unwrap_or(false);
    if !autorise {
        return repondre(
            &mut flux,
            401,
            TYPE_JSON,
            &json!({
                "error": "jeton requis ou invalide",
                "aide": "en-tête `Authorization: Bearer <jeton du nœud>`"
            })
            .to_string(),
        );
    }

    match (req.methode.as_str(), chemin.as_str()) {
        ("GET", "/api/status") => {
            let v = crate::outils::statut(&s.politique, &s.identite, s.debut, "http", &s.moteur);
            let mut v = v;
            v["chat"] = json!({
                "modele": s.llm,
                "disponible": chat::modele_disponible(&s.llm),
                "tours_memorises": s.historique.derniers(500).len(),
            });
            repondre(&mut flux, 200, TYPE_JSON, &v.to_string())
        }

        ("POST", "/api/chat") => {
            let corps: Value = match serde_json::from_str(&req.corps) {
                Ok(v) => v,
                Err(e) => {
                    return repondre(
                        &mut flux,
                        400,
                        TYPE_JSON,
                        &json!({ "error": format!("corps JSON illisible : {e}") }).to_string(),
                    );
                }
            };
            let question = corps
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if question.is_empty() {
                return repondre(
                    &mut flux,
                    400,
                    TYPE_JSON,
                    &json!({ "error": "« message » requis" }).to_string(),
                );
            }
            let question = question.to_string();

            // Le contexte est RELU ici, à chaque tour : une charge ou un nom
            // d'hôte qui change doit apparaître.
            let systeme = chat::contexte(&s.politique, &s.identite, s.debut, "http");
            let historique = s.historique.derniers(12);
            s.historique.ajouter("user", &question);

            flux.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {TYPE_SSE}\r\n\
                         Cache-Control: no-cache\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )?;
            flux.flush()?;

            let debut = Instant::now();
            // Le flux est écrit AU FUR ET À MESURE : c'est ce qui distingue une
            // réponse qui s'écrit d'un bloc reçu après une longue attente.
            //
            // ── LE CLIENT PARTI ARRÊTE LA GÉNÉRATION ────────────────────────
            // Le drapeau est le MÊME pour l'écrivain et pour la garde : dès que
            // l'écriture vers la page échoue, la garde cesse de lire le flux du
            // modèle. Sans ce partage, un onglet fermé laissait le nœud générer
            // jusqu'au plafond, et la boucle s'arrêtait « au tour suivant » —
            // c'est-à-dire jamais, puisqu'il n'y en avait pas.
            let client_parti = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut garde =
                Garde::avec_client(Reglages::default(), Some(Arc::clone(&client_parti)));
            let mut erreur_flux: Option<anyhow::Error> = None;
            let resultat = chat::converser(
                &s.llm,
                &systeme,
                &historique,
                &question,
                &mut garde,
                |frag| match pousser(&mut flux, "texte", &json!({ "t": frag })) {
                    Ok(()) => None,
                    Err(e) => {
                        client_parti.store(true, std::sync::atomic::Ordering::Relaxed);
                        erreur_flux = Some(e);
                        Some(Motif::ClientParti)
                    }
                },
            );

            match resultat {
                Ok(tour) => {
                    s.historique.ajouter("assistant", &tour.texte);
                    // Un arrêt se dit — au client, pas seulement dans un journal.
                    match &tour.arret {
                        None => {
                            let _ = pousser(
                                &mut flux,
                                "fin",
                                &json!({ "ms_llm": debut.elapsed().as_millis() as u64 }),
                            );
                        }
                        Some(motif) => {
                            // Le motif part au client quand il est encore là ;
                            // quand c'est SON départ qui a arrêté, l'écriture
                            // échouera (et c'est normal) — alors on le dit au
                            // journal du service, qui est le seul à écouter
                            // encore. Un arrêt qui ne se dit nulle part serait
                            // indiscernable d'une réponse tronquée.
                            if pousser(
                                &mut flux,
                                "arret",
                                &json!({ "code": motif.code(), "phrase": motif.phrase() }),
                            )
                            .is_err()
                            {
                                let detail = erreur_flux
                                    .take()
                                    .map(|e| format!(" ({e:#})"))
                                    .unwrap_or_default();
                                eprintln!("[chat] {}{detail}", motif.phrase());
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = pousser(&mut flux, "erreur", &json!({ "error": format!("{e:#}") }));
                }
            }
            Ok(())
        }

        // ── LE HARNESS À L'ŒUVRE ────────────────────────────────────────────
        // Chaque acte est poussé À MESURE : l'opérateur voit ce que la machine
        // fait, pas seulement ce qu'elle répond. C'est la différence entre
        // regarder travailler et lire un compte rendu.
        ("POST", "/api/run") => {
            let corps: Value = match serde_json::from_str(&req.corps) {
                Ok(v) => v,
                Err(e) => {
                    return repondre(
                        &mut flux,
                        400,
                        TYPE_JSON,
                        &json!({ "error": format!("corps JSON illisible : {e}") }).to_string(),
                    );
                }
            };
            let objectif = corps
                .get("objectif")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if objectif.is_empty() {
                return repondre(
                    &mut flux,
                    400,
                    TYPE_JSON,
                    &json!({ "error": "« objectif » requis" }).to_string(),
                );
            }
            let objectif = objectif.to_string();
            let max_etapes = corps
                .get("maxSteps")
                .and_then(|v| v.as_u64())
                .unwrap_or(12)
                .clamp(1, 40) as usize;

            flux.write_all(
                format!(
                    "HTTP/1.1 200 OK
Content-Type: {TYPE_SSE}
                         Cache-Control: no-cache
Connection: close

"
                )
                .as_bytes(),
            )?;
            flux.flush()?;

            // Le socle est relu ICI : une identité ou un état du self qui a
            // changé depuis le démarrage doit entrer dans ce tour.
            let socle = crate::soi::Socle::lire(&s.moteur, 10_000);
            let _ = pousser(
                &mut flux,
                "socle",
                &json!({
                    "self": socle.self_qn,
                    "joignable": socle.joignable,
                    "mondes": socle.mondes.len(),
                }),
            );

            // Le drapeau est partagé entre l'écrivain du flux et la garde : une
            // écriture refusée cesse la LECTURE du modèle, au lieu d'attendre le
            // tour suivant pour s'en apercevoir.
            let ferme = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut garde = Garde::avec_client(Reglages::default(), Some(Arc::clone(&ferme)));
            let mut cb = |ev: agent::Evenement| -> anyhow::Result<()> {
                if ferme.load(std::sync::atomic::Ordering::Relaxed) {
                    anyhow::bail!("flux client fermé");
                }
                let r = match ev {
                    agent::Evenement::Texte(t) => pousser(&mut flux, "texte", &json!({ "t": t })),
                    agent::Evenement::Acte {
                        etape,
                        outil,
                        argument,
                    } => pousser(
                        &mut flux,
                        "acte",
                        &json!({ "etape": etape, "outil": outil, "argument": argument }),
                    ),
                    agent::Evenement::Rend { etape, ok, apercu } => pousser(
                        &mut flux,
                        "rend",
                        &json!({ "etape": etape, "ok": ok, "apercu": apercu }),
                    ),
                    agent::Evenement::Fin { etapes, ms } => {
                        pousser(&mut flux, "fin", &json!({ "etapes": etapes, "ms": ms }))
                    }
                    agent::Evenement::Borne { etapes } => {
                        pousser(&mut flux, "borne", &json!({ "etapes": etapes }))
                    }
                    agent::Evenement::Arret { code, phrase } => pousser(
                        &mut flux,
                        "arret",
                        &json!({ "code": code, "phrase": phrase }),
                    ),
                };
                if r.is_err() {
                    ferme.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                r
            };

            match agent::derouler(
                &s.llm,
                &socle,
                &s.politique,
                &s.identite,
                &s.moteur,
                &s.notes,
                &objectif,
                max_etapes,
                &mut garde,
                &mut cb,
            ) {
                Ok(reponse) => {
                    s.historique.ajouter("user", &objectif);
                    s.historique.ajouter("assistant", &reponse);
                    let _ = pousser(&mut flux, "reponse", &json!({ "texte": reponse }));
                }
                Err(e) => {
                    let _ = pousser(&mut flux, "erreur", &json!({ "error": format!("{e:#}") }));
                }
            }
            Ok(())
        }

        ("GET", _) | ("POST", _) => repondre(
            &mut flux,
            404,
            TYPE_JSON,
            &json!({ "error": format!("route inconnue : {chemin}") }).to_string(),
        ),
        _ => repondre(
            &mut flux,
            405,
            TYPE_JSON,
            &json!({ "error": "méthode non supportée" }).to_string(),
        ),
    }
}

/// Sert la surface HTTP jusqu'à l'arrêt du processus.
///
/// `secret` est **obligatoire** : la fonction refuse un secret vide. Ce n'est pas
/// une vérification de confort, c'est la règle 1 de l'en-tête — un chat de
/// machine sans authentification est une porte ouverte sur l'exécution de
/// commandes.
pub fn servir(e: &Ecoute, surface: Surface) -> Result<()> {
    if surface.secret.is_empty() {
        anyhow::bail!(
            "réfus d'ouvrir une surface HTTP sans jeton — démarrer avec --token <secret> \
             ou $SELFD_TOKEN. Un chat de nœud non authentifié est une exécution de \
             commandes ouverte."
        );
    }
    let adresse = format!("{}:{}", e.host, e.port);
    let ecoute =
        TcpListener::bind(&adresse).with_context(|| format!("écoute HTTP sur {adresse}"))?;
    let port_reel = ecoute.local_addr().map(|a| a.port()).unwrap_or(e.port);

    eprintln!(
        "selfd : chat http://{}:{port_reel}/  (jeton requis · modèle {})",
        e.host, surface.llm
    );

    let partage = Arc::new(surface);
    for flux in ecoute.incoming() {
        let Ok(flux) = flux else { continue };
        let s = Arc::clone(&partage);
        std::thread::spawn(move || {
            let _ = traiter(flux, s);
        });
    }
    Ok(())
}
