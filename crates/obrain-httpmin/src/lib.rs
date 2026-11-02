//! Client HTTP minimal (std uniquement), partagé par `selflink` et `selfd`.
//!
//! # Le parti pris, et d'où il vient
//!
//! `obrain-cli` a établi la règle pour joindre le serveur viz : *« obrain-cli
//! n'embarque aucune dépendance HTTP — et n'en embarquera pas pour joindre un
//! service local »*. Le serveur viz est lui-même écrit en `std::net` ; un client
//! symétrique de quelques centaines de lignes suffit.
//!
//! Portée étroite et **explicite** : `GET`/`POST` vers `http://hôte[:port]/chemin`,
//! `Content-Length` **ou** `Transfer-Encoding: chunked` **ou** corps à EOF. Pas
//! de TLS, pas de redirection, pas de connexion persistante. Toute limitation
//! est écrite, jamais silencieuse.
//!
//! # Pourquoi ce crate existe
//!
//! `selflink` et `selfd` en avaient chacun une copie dérivée du même modèle. La
//! première correction aurait divergé entre les deux — le défaut que ce dépôt
//! combat ailleurs : deux chemins pour un même travail ne restent pas d'accord.
//!
//! Il sert à deux choses différentes :
//!
//! * `selflink` interroge le **moteur** (self, mémoire, requêtes, conversation) ;
//! * `selfd` interroge le **modèle** (un serveur OpenAI-compatible) pour le chat
//!   local du nœud.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Délai d'une requête. Le moteur répond en quelques millisecondes ; une requête
/// lourde dépasse la seconde, et un tour de conversation passe par un modèle —
/// d'où une marge large plutôt qu'un faux « timeout ».
pub const DELAI_DEFAUT: Duration = Duration::from_secs(180);

/// Décompose `http://hôte[:port]/chemin` en (hôte, port, chemin).
pub fn parse_url(url: &str) -> Result<(String, u16, String)> {
    let reste = url
        .strip_prefix("http://")
        .with_context(|| format!("httpmin : seul http:// est supporté (reçu {url})"))?;
    let (autorite, chemin) = match reste.split_once('/') {
        Some((a, c)) => (a, format!("/{c}")),
        None => (reste, "/".to_string()),
    };
    let (hote, port) = match autorite.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(80)),
        None => (autorite.to_string(), 80),
    };
    if hote.is_empty() {
        bail!("httpmin : hôte vide dans {url}");
    }
    Ok((hote, port, chemin))
}

/// Échappe une valeur pour un paramètre de requête (RFC 3986, jeu `unreserved`).
///
/// Une requête GQL contient guillemets, espaces et accolades : passée brute, elle
/// couperait l'URL au premier `&`.
pub fn echapper(valeur: &str) -> String {
    let mut sortie = String::with_capacity(valeur.len());
    for octet in valeur.bytes() {
        match octet {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                sortie.push(octet as char);
            }
            _ => sortie.push_str(&format!("%{octet:02X}")),
        }
    }
    sortie
}

/// Construit `chemin?clé=valeur&…`, valeurs échappées.
pub fn avec_params(chemin: &str, params: &[(&str, &str)]) -> String {
    if params.is_empty() {
        return chemin.to_string();
    }
    let mut out = format!("{chemin}?");
    for (i, (cle, valeur)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(cle);
        out.push('=');
        out.push_str(&echapper(valeur));
    }
    out
}

/// En-tête de réponse : ce qui commande la lecture du corps.
struct Entete {
    status: u16,
    longueur: Option<usize>,
    chunked: bool,
}

/// Réponse lue en entier.
pub struct Reponse {
    /// Statut HTTP (`200`, `401`, `500`…).
    pub status: u16,
    /// Corps de la réponse, tel que reçu (déjà dé-chunké si nécessaire).
    pub corps: String,
}

impl Reponse {
    /// Le corps, désérialisé. Le statut est vérifié **avant** : un corps d'erreur
    /// n'est pas un objet exploitable, et le confondre avec un résultat est la
    /// façon la plus sûre de faire passer une panne pour une réponse vide.
    pub fn json(&self) -> Result<serde_json::Value> {
        if !(200..300).contains(&self.status) {
            bail!(
                "HTTP {} — {}",
                self.status,
                self.corps.trim().chars().take(400).collect::<String>()
            );
        }
        serde_json::from_str(&self.corps)
            .with_context(|| format!("réponse non-JSON ({})", extrait(&self.corps)))
    }
}

fn extrait(s: &str) -> String {
    s.trim().chars().take(200).collect()
}

/// Envoie la requête et lit l'en-tête de réponse.
fn envoyer(
    url: &str,
    methode: &str,
    entetes: &[(&str, &str)],
    corps: Option<&str>,
) -> Result<(Entete, BufReader<TcpStream>)> {
    envoyer_avec_delai(url, methode, entetes, corps, DELAI_DEFAUT)
}

/// Comme [`envoyer`], avec un délai de lecture **choisi par l'appelant**.
///
/// Le délai d'une lecture de flux n'est pas celui d'une requête : un tour de
/// modèle en flux peut légitimement durer des minutes, alors qu'une requête de
/// lecture du moteur qui dépasse la seconde est déjà lente. Le figer dans une
/// constante obligeait les appelants de flux à accepter le délai des autres.
fn envoyer_avec_delai(
    url: &str,
    methode: &str,
    entetes: &[(&str, &str)],
    corps: Option<&str>,
    delai: Duration,
) -> Result<(Entete, BufReader<TcpStream>)> {
    let (hote, port, chemin) = parse_url(url)?;
    let mut flux = TcpStream::connect((hote.as_str(), port))
        .with_context(|| format!("connexion impossible à {hote}:{port}"))?;
    flux.set_read_timeout(Some(delai)).ok();
    flux.set_write_timeout(Some(delai)).ok();

    let mut requete =
        format!("{methode} {chemin} HTTP/1.1\r\nHost: {hote}:{port}\r\nConnection: close\r\n");
    for (nom, valeur) in entetes {
        requete.push_str(&format!("{nom}: {valeur}\r\n"));
    }
    match corps {
        Some(c) => {
            requete.push_str(&format!("Content-Length: {}\r\n\r\n", c.len()));
            requete.push_str(c);
        }
        None => requete.push_str("\r\n"),
    }
    flux.write_all(requete.as_bytes())
        .context("écriture de la requête")?;
    flux.flush().ok();

    let mut lecteur = BufReader::new(flux);
    let mut ligne = String::new();
    lecteur
        .read_line(&mut ligne)
        .context("lecture de la ligne de statut")?;
    let status = ligne
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .with_context(|| format!("statut illisible : {}", ligne.trim()))?;

    let (mut longueur, mut chunked) = (None, false);
    loop {
        let mut h = String::new();
        let n = lecteur.read_line(&mut h).context("lecture des en-têtes")?;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        let bas = h.to_ascii_lowercase();
        if bas.starts_with("content-length:") {
            longueur = h[15..].trim().parse::<usize>().ok();
        } else if bas.starts_with("transfer-encoding:") && bas.contains("chunked") {
            chunked = true;
        }
    }
    Ok((
        Entete {
            status,
            longueur,
            chunked,
        },
        lecteur,
    ))
}

/// Lit le corps entier selon ce que l'en-tête annonce.
fn lire_corps(mut lecteur: BufReader<TcpStream>, e: &Entete) -> Result<String> {
    let mut brut = Vec::new();
    match e.longueur {
        // Lecture bornée : `read_to_end` attendrait une fermeture qui, pour un
        // corps à longueur connue, n'est pas le signal de fin.
        Some(l) => {
            brut.resize(l, 0);
            lecteur.read_exact(&mut brut).context("lecture du corps")?;
        }
        None => {
            lecteur.read_to_end(&mut brut).context("lecture du corps")?;
        }
    }
    let texte = String::from_utf8_lossy(&brut).to_string();
    Ok(if e.chunked {
        decoder_chunked(&texte)
    } else {
        texte
    })
}

/// Décode un corps `Transfer-Encoding: chunked`.
///
/// Le serveur viz n'en émet pas, mais `llama-server` en émet — et un proxy
/// placé devant pourrait en ajouter. Sans ce décodage on lirait les tailles
/// hexadécimales comme du JSON.
fn decoder_chunked(corps: &str) -> String {
    let mut sortie = String::new();
    let mut reste = corps;
    loop {
        let Some((taille_hex, apres)) = reste.split_once("\r\n") else {
            break;
        };
        let Ok(taille) = usize::from_str_radix(taille_hex.trim(), 16) else {
            return corps.to_string();
        };
        if taille == 0 {
            break;
        }
        if apres.len() < taille {
            break;
        }
        sortie.push_str(&apres[..taille]);
        reste = apres[taille..].trim_start_matches("\r\n");
    }
    if sortie.is_empty() {
        corps.to_string()
    } else {
        sortie
    }
}

fn entetes_auth(jeton: Option<&str>) -> Vec<(&str, &str)> {
    match jeton {
        Some(t) => vec![("Authorization", t)],
        None => Vec::new(),
    }
}

/// `GET` avec paramètres, réponse lue en entier.
pub fn get(url: &str, params: &[(&str, &str)], jeton: Option<&str>) -> Result<Reponse> {
    let (entete, lecteur) = envoyer(&avec_params(url, params), "GET", &entetes_auth(jeton), None)?;
    let corps = lire_corps(lecteur, &entete)?;
    Ok(Reponse {
        status: entete.status,
        corps,
    })
}

/// `POST` d'un corps JSON.
pub fn post_json(url: &str, corps: &str, jeton: Option<&str>) -> Result<Reponse> {
    let mut entetes = vec![("Content-Type", "application/json")];
    entetes.extend(entetes_auth(jeton));
    let (entete, lecteur) = envoyer(url, "POST", &entetes, Some(corps))?;
    let corps = lire_corps(lecteur, &entete)?;
    Ok(Reponse {
        status: entete.status,
        corps,
    })
}

/// Lit un flux SSE sur un `GET` et rappelle `sur_evenement(nom, données)`.
///
/// La fin est détectée par l'EOF. Le battement (`: ping`) est ignoré
/// **volontairement** : c'est un commentaire SSE, pas un événement — le remonter
/// ferait croire à une réponse.
///
/// Une ligne `data:` **sans** `event:` est rendue avec un nom vide : c'est la
/// forme qu'emploient les serveurs OpenAI-compatibles (`data: {...}`).
pub fn lire_sse<F>(
    url: &str,
    params: &[(&str, &str)],
    jeton: Option<&str>,
    sur_evenement: F,
) -> Result<()>
where
    F: FnMut(&str, &str),
{
    let (entete, lecteur) = envoyer(&avec_params(url, params), "GET", &entetes_auth(jeton), None)?;
    lire_flux(entete, lecteur, sur_evenement)
}

/// Lit un flux SSE en réponse à un `POST` JSON.
///
/// Indispensable pour un serveur OpenAI-compatible : `/v1/chat/completions` en
/// flux s'obtient par **POST** avec un corps. Un `GET` y répondrait une erreur —
/// et une erreur lue comme un flux donnerait une conversation vide, sans que
/// rien ne dise pourquoi.
pub fn post_sse<F>(url: &str, corps: &str, jeton: Option<&str>, sur_evenement: F) -> Result<()>
where
    F: FnMut(&str, &str),
{
    let mut entetes = vec![
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
    ];
    entetes.extend(entetes_auth(jeton));
    let (entete, lecteur) = envoyer(url, "POST", &entetes, Some(corps))?;
    lire_flux(entete, lecteur, sur_evenement)
}

/// Lit un flux SSE en réponse à un POST, **avec le pouvoir d'arrêter la lecture**.
///
/// ── POURQUOI CETTE FONCTION EXISTE (`post_sse` ne suffisait pas) ────────────
///
/// Défaut mesuré le 2026-09-16 : le rappel de `post_sse` rendait `()`, donc
/// **aucun appelant ne pouvait cesser de lire**. Un modèle qui se répète
/// écrivait jusqu'au plafond de génération, un client qui fermait son onglet
/// laissait le nœud générer (600 s par appel), et la fin d'un flux interrompu
/// était indiscernable d'une réponse complète. Un appelant qui ne peut pas dire
/// « stop » ne peut pas garantir qu'une génération s'arrête.
///
/// Deux façons de s'arrêter, et elles se distinguent au retour :
///
/// * `sur_ligne` rend `false` — la boucle appelante a jugé ce qu'elle venait de
///   lire (répétition détectée, texte suffisant, erreur à remonter) ;
/// * `continuer` rend `false` — l'état extérieur a changé (le client est parti) ;
///   il est consulté **avant chaque lecture**, pas seulement entre deux trames.
///
/// # Retour
///
/// `Ok(true)` : la lecture s'est arrêtée **par nous** (rappel ou commande).
/// `Ok(false)` : le flux est allé à son terme (EOF du service).
///
/// La distinction est le cœur de la correction : elle rend la fin d'un flux
/// interrompu impossible à confondre avec une réponse complète.
///
/// # Erreurs
///
/// Erreur si la connexion, l'en-tête ou une lecture échoue — y compris quand le
/// délai `delai` expire **pendant** l'attente d'une ligne : un flux qui cesse
/// d'émettre sans fermer ne bloque pas l'appelant indéfiniment.
pub fn post_sse_supervise<F, C>(
    url: &str,
    corps: &str,
    jeton: Option<&str>,
    delai: Duration,
    sur_ligne: F,
    continuer: C,
) -> Result<bool>
where
    F: FnMut(&str, &str) -> bool,
    C: FnMut() -> bool,
{
    let mut entetes = vec![
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
    ];
    entetes.extend(entetes_auth(jeton));
    let (entete, lecteur) = envoyer_avec_delai(url, "POST", &entetes, Some(corps), delai)?;
    lire_flux_supervise(entete, lecteur, sur_ligne, continuer)
}

/// Le corps commun aux deux : on ne lit le flux que si le statut l'autorise.
fn lire_flux<F>(entete: Entete, lecteur: BufReader<TcpStream>, mut sur_evenement: F) -> Result<()>
where
    F: FnMut(&str, &str),
{
    lire_flux_supervise(
        entete,
        lecteur,
        |nom, donnees| {
            sur_evenement(nom, donnees);
            true
        },
        || true,
    )
    .map(|_| ())
}

/// Le cœur de la lecture SSE, avec les deux points d'arrêt.
fn lire_flux_supervise<F, C>(
    entete: Entete,
    mut lecteur: BufReader<TcpStream>,
    mut sur_ligne: F,
    mut continuer: C,
) -> Result<bool>
where
    F: FnMut(&str, &str) -> bool,
    C: FnMut() -> bool,
{
    if entete.status != 200 {
        let mut reste = String::new();
        lecteur.read_to_string(&mut reste).ok();
        bail!("HTTP {} sur le flux — {}", entete.status, extrait(&reste));
    }
    let (mut nom, mut donnees) = (String::new(), String::new());
    loop {
        // ── LE CLIENT EST-IL ENCORE LÀ ? ────────────────────────────────────
        // Consulté AVANT la lecture : sans cette place, un client parti
        // n'était remarqué qu'à la trame suivante — c'est-à-dire jamais, si le
        // service s'était tu en même temps.
        if !continuer() {
            return Ok(true);
        }
        let mut ligne = String::new();
        let n = lecteur
            .read_line(&mut ligne)
            .context("lecture du flux SSE")?;
        if n == 0 {
            break;
        }
        let l = ligne.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if !donnees.is_empty() && !sur_ligne(&nom, &donnees) {
                return Ok(true);
            }
            nom.clear();
            donnees.clear();
        } else if let Some(v) = l.strip_prefix("event:") {
            nom = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("data:") {
            donnees.push_str(v.trim_start());
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    /// Un service SSE minimal, sur un port éphémère, qui répond une fois.
    ///
    /// `apres` est écrit après les trames : il sert à tenir la connexion
    /// OUVERTE (un vrai service en flux ne ferme pas entre deux jetons). C'est
    /// ce qui rend les tests d'arrêt probants — sans lui, la fin du flux
    /// suffirait à sortir de la boucle et l'arrêt ne serait pas mesuré.
    fn serveur_sse(statut: String, trames: String, apres: Duration) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("port éphémère");
        let port = listener.local_addr().expect("adresse locale").port();
        let ecrites = Arc::new(AtomicUsize::new(0));
        let compteur = Arc::clone(&ecrites);
        thread::spawn(move || {
            let Ok((mut flux, _)) = listener.accept() else {
                return;
            };
            // Lire la requête jusqu'à la ligne vide (le corps ne nous intéresse pas).
            let mut lecteur = BufReader::new(match flux.try_clone() {
                Ok(c) => c,
                Err(_) => return,
            });
            let mut ligne = String::new();
            while lecteur.read_line(&mut ligne).unwrap_or(0) > 0 {
                if ligne == "\r\n" || ligne == "\n" {
                    break;
                }
                ligne.clear();
            }
            let reponse = format!(
                "HTTP/1.1 {statut}\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{trames}"
            );
            let _ = flux.write_all(reponse.as_bytes());
            let _ = flux.flush();
            compteur.store(trames.matches("\n\n").count(), Ordering::Relaxed);
            thread::sleep(apres);
        });
        (
            format!("http://127.0.0.1:{port}/v1/chat/completions"),
            ecrites,
        )
    }

    const TROIS_TRAMES: &str =
        "data: {\"a\":1}\n\ndata: {\"a\":2}\n\ndata: {\"a\":3}\n\ndata: [DONE]\n\n";

    #[test]
    fn le_flux_nominal_est_lu_entierement_et_rend_false() {
        // `apres = 0` : le service ferme, donc le flux va à son terme.
        let (url, _) = serveur_sse("200 OK".into(), TROIS_TRAMES.into(), Duration::ZERO);
        let mut vus = Vec::new();
        let arrete = post_sse_supervise(
            &url,
            "{}",
            None,
            Duration::from_secs(5),
            |_nom, donnees| {
                vus.push(donnees.to_string());
                true
            },
            || true,
        )
        .expect("le flux doit être lisible");
        assert!(
            !arrete,
            "un flux terminé ne doit pas se dire « arrêté par nous »"
        );
        assert_eq!(vus, vec!["{\"a\":1}", "{\"a\":2}", "{\"a\":3}", "[DONE]"]);
    }

    #[test]
    fn le_rappel_peut_arreter_la_lecture_et_le_dit() {
        // Le service tient la connexion ouverte APRÈS ses trames : sans arrêt,
        // cette lecture attendrait le délai. C'est le défaut mesuré — le rappel
        // rendait `()` et l'appelant ne pouvait pas cesser de lire.
        let (url, _) = serveur_sse(
            "200 OK".into(),
            TROIS_TRAMES.into(),
            Duration::from_millis(300),
        );
        let mut vus = 0usize;
        let arrete = post_sse_supervise(
            &url,
            "{}",
            None,
            Duration::from_secs(5),
            |_nom, _donnees| {
                vus += 1;
                vus < 2 // on s'arrête APRÈS la deuxième trame
            },
            || true,
        )
        .expect("l'arrêt est un succès, pas une erreur");
        assert!(arrete, "l'arrêt par le rappel doit se dire");
        assert_eq!(vus, 2, "la troisième trame ne doit pas avoir été lue");
    }

    #[test]
    fn la_commande_du_client_arrete_la_lecture_avant_la_trame_suivante() {
        let (url, _) = serveur_sse(
            "200 OK".into(),
            TROIS_TRAMES.into(),
            Duration::from_millis(300),
        );
        let lectures = Arc::new(AtomicUsize::new(0));
        let compteur = Arc::clone(&lectures);
        let mut vus = 0usize;
        let arrete = post_sse_supervise(
            &url,
            "{}",
            None,
            Duration::from_secs(5),
            |_nom, _donnees| {
                vus += 1;
                true
            },
            || compteur.fetch_add(1, Ordering::Relaxed) < 2,
        )
        .expect("l'arrêt est un succès");
        assert!(arrete);
        assert!(
            vus <= 4,
            "on ne lit pas au-delà de l'ordre d'arrêt (vus = {vus})"
        );
        assert!(
            lectures.load(Ordering::Relaxed) <= 3,
            "l'état extérieur est consulté à chaque tour"
        );
    }

    #[test]
    fn un_statut_non_200_est_une_erreur_qui_montre_le_corps() {
        let (url, _) = serveur_sse(
            "500 Internal Server Error".into(),
            "{\"error\":\"modele absent\"}\n\n".into(),
            Duration::ZERO,
        );
        let err = post_sse_supervise(
            &url,
            "{}",
            None,
            Duration::from_secs(5),
            |_, _| true,
            || true,
        )
        .expect_err("un 500 doit être une erreur, jamais un flux vide");
        let message = format!("{err:#}");
        assert!(message.contains("500"), "{message}");
        assert!(
            message.contains("modele absent"),
            "le corps doit être montré : {message}"
        );
    }

    #[test]
    fn le_delai_borne_l_attente_d_une_trame_qui_ne_vient_pas() {
        // En-tête envoyé, puis silence : sans délai de lecture, l'appelant
        // attendrait pour toujours la trame suivante.
        let listener = TcpListener::bind("127.0.0.1:0").expect("port éphémère");
        let port = listener.local_addr().expect("adresse locale").port();
        thread::spawn(move || {
            let Ok((mut flux, _)) = listener.accept() else {
                return;
            };
            let _ = flux.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let _ = flux.flush();
            thread::sleep(Duration::from_secs(2));
        });
        let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        let debut = std::time::Instant::now();
        let err = post_sse_supervise(
            &url,
            "{}",
            None,
            Duration::from_millis(200),
            |_, _| true,
            || true,
        )
        .expect_err("le silence doit rendre la main, pas bloquer");
        assert!(format!("{err:#}").contains("flux"), "{err:#}");
        assert!(
            debut.elapsed() < Duration::from_secs(2),
            "l'attente a dépassé le délai demandé"
        );
    }
}
