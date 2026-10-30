//! Client HTTP MINIMAL (std uniquement).
//!
//! `obrain-cli` n'embarque aucune dépendance HTTP — et n'en embarquera pas pour
//! joindre un service local : le serveur viz est lui-même écrit en `std::net`
//! (voir `viz_serve.rs`), un client symétrique de ~120 lignes suffit à parler à
//! un serveur OpenAI-compatible local (llama-server sur 9901).
//!
//! Portée volontairement étroite : POST/GET vers `http://hôte[:port]/chemin`,
//! corps JSON, réponse lue en entier, `Content-Length` OU `Transfer-Encoding:
//! chunked`. Pas de TLS (le LLM est local), pas de redirection, pas de
//! connexion persistante. Toute limitation est explicite, jamais silencieuse.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Décompose `http://hôte[:port]/chemin` en (hôte, port, chemin).
fn parse_url(url: &str) -> Result<(String, u16, String)> {
    let rest = url
        .strip_prefix("http://")
        .with_context(|| format!("http_min : seul http:// est supporté (reçu {url})"))?;
    let (autorite, chemin) = match rest.split_once('/') {
        Some((a, c)) => (a, format!("/{c}")),
        None => (rest, "/".to_string()),
    };
    let (hote, port) = match autorite.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(80)),
        None => (autorite.to_string(), 80),
    };
    if hote.is_empty() {
        bail!("http_min : hôte vide dans {url}");
    }
    Ok((hote, port, chemin))
}

/// Réponse brute : statut, en-têtes utiles, corps.
pub struct Reponse {
    pub status: u16,
    pub body: String,
}

/// Lit la réponse complète d'un flux (en-têtes puis corps), en gérant
/// `Content-Length` et `chunked`.
fn lire_reponse(stream: &mut TcpStream) -> Result<Reponse> {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    // L'en-tête peut arriver en plusieurs morceaux : on lit jusqu'au double CRLF.
    let mut tmp = [0u8; 8192];
    let fin_entetes = loop {
        let n = stream.read(&mut tmp).context("http_min : lecture des en-têtes")?;
        if n == 0 {
            bail!("http_min : connexion fermée sans en-têtes");
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            bail!("http_min : en-têtes trop volumineux");
        }
    };
    let entetes = String::from_utf8_lossy(&buf[..fin_entetes]).to_string();
    let status: u16 = entetes
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let lower = entetes.to_lowercase();
    let mut corps = buf[fin_entetes..].to_vec();

    if lower.contains("transfer-encoding: chunked") {
        // Reassemblage par blocs (taille hexadécimale, puis données, puis CRLF).
        let mut out: Vec<u8> = Vec::new();
        let mut rest = corps;
        loop {
            while rest.is_empty() {
                let n = stream.read(&mut tmp).context("http_min : lecture chunk")?;
                if n == 0 { break; }
                rest.extend_from_slice(&tmp[..n]);
            }
            let Some(p) = rest.windows(2).position(|w| w == b"\r\n") else { break };
            let taille = usize::from_str_radix(
                String::from_utf8_lossy(&rest[..p]).trim().split(';').next().unwrap_or("0"),
                16,
            ).unwrap_or(0);
            if taille == 0 { break; }
            let debut = p + 2;
            // On garantit d'avoir tout le bloc.
            while rest.len() < debut + taille + 2 {
                let n = stream.read(&mut tmp).context("http_min : lecture bloc")?;
                if n == 0 { break; }
                rest.extend_from_slice(&tmp[..n]);
            }
            if rest.len() < debut + taille { break; }
            out.extend_from_slice(&rest[debut..debut + taille]);
            rest.drain(..(debut + taille + 2).min(rest.len()));
        }
        corps = out;
    } else if let Some(len) = lower
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        while corps.len() < len {
            let n = stream.read(&mut tmp).context("http_min : lecture du corps")?;
            if n == 0 { break; }
            corps.extend_from_slice(&tmp[..n]);
        }
        corps.truncate(len);
    } else {
        // Ni longueur ni chunked (connexion fermée comme délimiteur).
        loop {
            let n = stream.read(&mut tmp).context("http_min : lecture jusqu'à fermeture")?;
            if n == 0 { break; }
            corps.extend_from_slice(&tmp[..n]);
        }
    }
    Ok(Reponse { status, body: String::from_utf8_lossy(&corps).to_string() })
}

/// Requête POST JSON. `delai` borne l'attente (un LLM local peut être lent).
pub fn post_json(url: &str, payload: &str, delai: Duration) -> Result<Reponse> {
    let (hote, port, chemin) = parse_url(url)?;
    let addr = format!("{hote}:{port}");
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("http_min : connexion à {addr}"))?;
    stream.set_read_timeout(Some(delai)).ok();
    stream.set_write_timeout(Some(delai)).ok();
    let requete = format!(
        "POST {chemin} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(requete.as_bytes()).context("http_min : envoi")?;
    stream.flush().ok();
    lire_reponse(&mut stream)
}

/// Requête GET (utile pour les points de contrôle de santé du service).
pub fn get(url: &str, delai: Duration) -> Result<Reponse> {
    let (hote, port, chemin) = parse_url(url)?;
    let addr = format!("{hote}:{port}");
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("http_min : connexion à {addr}"))?;
    stream.set_read_timeout(Some(delai)).ok();
    let requete =
        format!("GET {chemin} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(requete.as_bytes()).context("http_min : envoi")?;
    stream.flush().ok();
    lire_reponse(&mut stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_avec_et_sans_port() {
        assert_eq!(
            parse_url("http://127.0.0.1:9901/v1/chat").unwrap(),
            ("127.0.0.1".into(), 9901, "/v1/chat".into())
        );
        assert_eq!(
            parse_url("http://exemple.test").unwrap(),
            ("exemple.test".into(), 80, "/".into())
        );
        assert!(parse_url("https://exemple.test").is_err());
    }
}

/// Lit la suite du flux dans `restant`. Rend `false` si la connexion est close
/// ou le délai dépassé — c'est-à-dire s'il n'y a plus rien à attendre.
///
/// POURQUOI CE HELPER EXISTE : la boucle de corps de `post_sse` ne relit la
/// socket que si `restant` est VIDE. Les deux `continue` d'origine (taille de
/// bloc incomplète, corps de bloc incomplet) tournaient donc **à vide sans
/// jamais lire** : la boucle repassait sur les mêmes octets, indéfiniment, en
/// brûlant un cœur — pendant que la réponse attendue, elle, ne serait jamais
/// arrivée. Il suffisait qu'un en-tête de bloc (`1a2\r\n`) ou un corps soit
/// coupé entre deux segments TCP, ce qui est courant sur un flux qui pousse des
/// milliers d'événements. Le chat restait alors muet, connexion ouverte, sans
/// erreur : le pire des symptômes, parce qu'il ressemble à un modèle lent.
fn lire_suite(stream: &mut TcpStream, restant: &mut Vec<u8>, tmp: &mut [u8; 8192]) -> bool {
    match stream.read(tmp) {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            restant.extend_from_slice(&tmp[..n]);
            true
        }
    }
}

/// ── POST EN FLUX (SSE) ──────────────────────────────────────────────────────
///
/// `post_json` attend la réponse ENTIÈRE : pour un modèle de raisonnement qui
/// réfléchit 60 s, le premier mot n'apparaîtrait qu'à la fin. Or on veut voir le
/// *think* et le texte ARRIVER. On lit donc ici la réponse au fil de l'eau et on
/// rend chaque ligne complète à l'appelant (`data: {...}`), sans jamais
/// rassembler le corps entier.
/// ── LE RAPPEL PEUT ARRÊTER LA LECTURE (2026-09-16) ──────────────────────────
///
/// Il rendait `()` : un appelant qui DÉTECTAIT un défaut — un modèle qui se répète,
/// un client parti — n'avait aucun moyen d'arrêter le flux. Le service continuait
/// donc d'écrire, et l'appel payait jusqu'au bout (mesuré : délai de 600 s par
/// appel de modèle). Le rappel rend désormais `true` pour CONTINUER et `false`
/// pour arrêter ; `post_sse` rend alors `Ok(true)` — « arrêté par le rappel » —
/// pour que l'appelant sache que la fin du flux n'est PAS une réponse complète.
pub fn post_sse<F>(url: &str, payload: &str, delai: Duration, mut on_ligne: F) -> Result<bool>
where
    F: FnMut(&str) -> bool,
{
    let (hote, port, chemin) = parse_url(url)?;
    let addr = format!("{hote}:{port}");
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("http_min : connexion à {addr}"))?;
    stream.set_read_timeout(Some(delai)).ok();
    stream.set_write_timeout(Some(delai)).ok();
    let requete = format!(
        "POST {chemin} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Accept: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(requete.as_bytes()).context("http_min : envoi")?;
    stream.flush().ok();

    // En-têtes jusqu'au double CRLF (ils peuvent arriver en morceaux).
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    let fin_entetes = loop {
        let n = stream.read(&mut tmp).context("http_min : lecture des en-têtes")?;
        if n == 0 {
            return Ok(false); // connexion fermée avant tout corps : rien à diffuser
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            bail!("http_min : en-têtes trop volumineux");
        }
    };
    let entetes_brutes = String::from_utf8_lossy(&buf[..fin_entetes]).to_string();
    let entetes = entetes_brutes.to_lowercase();
    let status: u16 = entetes_brutes
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let chunked = entetes.contains("transfer-encoding: chunked");
    let mut restant: Vec<u8> = buf[fin_entetes..].to_vec();

    // ── UN REFUS DOIT SE VOIR ───────────────────────────────────────────────
    // Sans ce contrôle, un refus du service (fenêtre de contexte dépassée,
    // requête invalide) était IGNORÉ : le corps d'une erreur n'est pas du SSE,
    // ses lignes ne commencent pas par « data: », donc rien n'était diffusé et
    // l'appel se terminait « normalement ». Le chat annonçait alors le contexte
    // puis restait MUET, sans erreur : une panne présentée comme une réponse
    // vide. Mesuré le 2026-09-11 sur la base « Runtime dev (engrams) » :
    // 29,6 s d'attente, zéro événement, aucune erreur affichée.
    if status != 200 {
        // Le corps d'erreur est court : on le lit avec un délai BREF, sinon on
        // attendrait le délai de génération (600 s) pour un message de 80 octets.
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let mut corps = restant.clone();
        while corps.len() < 8 * 1024 {
            match stream.read(&mut tmp) {
                Ok(0) | Err(_) => break,
                Ok(n) => corps.extend_from_slice(&tmp[..n]),
            }
        }
        let message: String = String::from_utf8_lossy(&corps)
            .trim()
            .chars()
            .take(400)
            .collect();
        bail!("http_min : le service a répondu {status} — {message}");
    }

    // Corps au fil de l'eau. En « chunked », on retire l'encadrement au fur et à
    // mesure ; sinon tout ce qui arrive est du contenu.
    let mut lignes: Vec<u8> = Vec::new();
    let mut taille_bloc: Option<usize> = None;
    loop {
        if restant.is_empty() {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => restant.extend_from_slice(&tmp[..n]),
                Err(_) => break, // délai dépassé : on rend ce qu'on a déjà
            }
        }
        if chunked {
            if taille_bloc.is_none() {
                if let Some(p) = restant.windows(2).position(|w| w == b"\r\n") {
                    let t = String::from_utf8_lossy(&restant[..p])
                        .trim()
                        .split(';')
                        .next()
                        .unwrap_or("0")
                        .to_string();
                    let n = usize::from_str_radix(&t, 16).unwrap_or(0);
                    restant.drain(..p + 2);
                    if n == 0 {
                        break;
                    }
                    taille_bloc = Some(n);
                } else if lire_suite(&mut stream, &mut restant, &mut tmp) {
                    continue;
                } else {
                    break;
                }
            }
            if let Some(n) = taille_bloc {
                if restant.len() < n + 2 {
                    if lire_suite(&mut stream, &mut restant, &mut tmp) {
                        continue;
                    }
                    break;
                }
                lignes.extend_from_slice(&restant[..n]);
                restant.drain(..n + 2);
                taille_bloc = None;
            }
        } else {
            lignes.extend_from_slice(&restant);
            restant.clear();
        }
        // On rend chaque ligne COMPLÈTE, et on garde le reliquat.
        while let Some(p) = lignes.iter().position(|&c| c == b'\n') {
            let ligne: Vec<u8> = lignes.drain(..=p).collect();
            let texte = String::from_utf8_lossy(&ligne).trim().to_string();
            if !texte.is_empty() && !on_ligne(&texte) {
                return Ok(true);   // arrêt demandé par le rappel : on ne lit plus
            }
        }
    }
    if !lignes.is_empty() {
        let texte = String::from_utf8_lossy(&lignes).trim().to_string();
        if !texte.is_empty() && !on_ligne(&texte) {
            return Ok(true);
        }
    }
    Ok(false)
}
