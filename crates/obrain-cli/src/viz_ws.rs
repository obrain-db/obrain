//! Primitives WebSocket **std-only** (NATIF-J9, transport vivant du viz).
//!
//! Le viz serve est un mini serveur HTTP sans dependance, un thread par
//! connexion (`TcpListener::incoming` -> `thread::spawn` -> `handle`), IO
//! bloquante, `Connection: close`. C'est le modele ideal pour un WS
//! serveur->client : on detourne le `TcpStream` apres le handshake, on ecrit
//! les frames, on ferme.
//!
//! On n'a besoin, pour le streaming du subgraph (Phase A), que de :
//!   1. le handshake RFC6455 (SHA-1 + base64 du `Sec-WebSocket-Key` + GUID) ;
//!   2. l'ecriture de frames texte NON masquees (serveur->client) ;
//!   3. une frame `close`.
//! Les parametres arrivent dans l'URL du GET d'upgrade : AUCUNE lecture de
//! frame client n'est requise. => zero dependance nouvelle, esprit std-only
//! preserve (cf. decision archi, docs/plan-j9.md).

use std::io::Write;

use anyhow::Result;

/// GUID magique RFC6455, concatene a la cle client avant SHA-1.
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

// ── SHA-1 (RFC 3174), implementation minimale ──────────────────────────────
// Utilisee UNIQUEMENT pour le `Sec-WebSocket-Accept` du handshake — jamais pour
// de la securite. Vecteur de test RFC6455 dans les tests plus bas.
fn sha1(msg: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let ml: u64 = (msg.len() as u64).wrapping_mul(8);
    let mut data = msg.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&ml.to_be_bytes());

    for chunk in data.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, hi) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&hi.to_be_bytes());
    }
    out
}

/// base64 standard (RFC 4648), avec padding `=`.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// `Sec-WebSocket-Accept` a partir du `Sec-WebSocket-Key` client (RFC6455 §4.2.2).
fn accept_key(sec_key: &str) -> String {
    let mut buf = sec_key.as_bytes().to_vec();
    buf.extend_from_slice(WS_GUID.as_bytes());
    base64_encode(&sha1(&buf))
}

/// Valeur d'un en-tete (insensible a la casse du NOM), premiere occurrence.
fn header<'a>(entetes: &'a str, name: &str) -> Option<&'a str> {
    let name_low = name.to_ascii_lowercase();
    entetes.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().to_ascii_lowercase() == name_low {
            Some(v.trim())
        } else {
            None
        }
    })
}

/// La requete est-elle un upgrade WebSocket bien forme ?
#[must_use]
pub(crate) fn is_websocket_upgrade(entetes: &str) -> bool {
    let upgrade_ws = header(entetes, "Upgrade").is_some_and(|v| {
        v.to_ascii_lowercase()
            .split(',')
            .any(|t| t.trim() == "websocket")
    });
    let conn_upgrade = header(entetes, "Connection").is_some_and(|v| {
        v.to_ascii_lowercase()
            .split(',')
            .any(|t| t.trim() == "upgrade")
    });
    upgrade_ws && conn_upgrade && header(entetes, "Sec-WebSocket-Key").is_some()
}

/// Handshake serveur. Ecrit la reponse `101 Switching Protocols` si la requete
/// est un upgrade WS valide (retourne `true`) ; sinon ecrit `400` (`false`).
/// Apres un `true`, le `stream` est un canal WS ouvert (frames a suivre).
pub(crate) fn handshake(stream: &mut impl Write, entetes: &str) -> Result<bool> {
    let Some(key) = header(entetes, "Sec-WebSocket-Key").filter(|_| is_websocket_upgrade(entetes))
    else {
        let body = b"attendu: upgrade WebSocket (RFC6455)";
        let resp = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes())?;
        stream.write_all(body)?;
        stream.flush()?;
        return Ok(false);
    };
    let accept = accept_key(key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()?;
    Ok(true)
}

/// Ecrit une frame WebSocket brute (FIN=1, non masquee).
fn write_frame(stream: &mut impl Write, opcode: u8, payload: &[u8]) -> Result<()> {
    let mut hdr: Vec<u8> = Vec::with_capacity(10);
    hdr.push(0x80 | (opcode & 0x0F)); // FIN=1
    let len = payload.len();
    if len < 126 {
        hdr.push(len as u8);
    } else if len < 65536 {
        hdr.push(126);
        hdr.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        hdr.push(127);
        hdr.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&hdr)?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

/// Envoie une frame texte (opcode 0x1). Le payload doit etre de l'UTF-8 valide.
pub(crate) fn send_text(stream: &mut impl Write, payload: &str) -> Result<()> {
    write_frame(stream, 0x1, payload.as_bytes())
}

/// Envoie une frame `close` (opcode 0x8, sans corps) et laisse le TCP se fermer.
pub(crate) fn send_close(stream: &mut impl Write) -> Result<()> {
    write_frame(stream, 0x8, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_vecteurs_connus() {
        // RFC 3174 / vecteurs classiques.
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }

    #[test]
    fn base64_vecteurs_connus() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn accept_vecteur_rfc6455() {
        // RFC6455 §1.3 : la cle d'exemple donne cet accept.
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn detecte_upgrade_websocket() {
        let ok = "GET /x HTTP/1.1\r\nHost: h\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: abc\r\nSec-WebSocket-Version: 13";
        assert!(is_websocket_upgrade(ok));
        // Connection combine (navigateurs) : "keep-alive, Upgrade".
        let combo = "GET /x HTTP/1.1\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Key: abc";
        assert!(is_websocket_upgrade(combo));
        // GET ordinaire : pas un upgrade.
        let plain = "GET /x HTTP/1.1\r\nHost: h\r\nAccept: */*";
        assert!(!is_websocket_upgrade(plain));
        // Upgrade sans cle : refuse.
        let sanscle = "GET /x HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade";
        assert!(!is_websocket_upgrade(sanscle));
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn frame_texte_courte_non_masquee() {
        let mut out: Vec<u8> = Vec::new();
        send_text(&mut out, "hi").unwrap();
        // FIN(0x80)|opcode texte(0x1) = 0x81 ; len=2 (< 126) ; pas de bit de masque.
        assert_eq!(out, vec![0x81, 0x02, b'h', b'i']);
    }

    #[test]
    fn frame_longueur_etendue_16bits() {
        let payload = "x".repeat(200); // >= 126 -> longueur sur 2 octets
        let mut out: Vec<u8> = Vec::new();
        send_text(&mut out, &payload).unwrap();
        assert_eq!(out[0], 0x81);
        assert_eq!(out[1], 126);
        assert_eq!(u16::from_be_bytes([out[2], out[3]]), 200);
        assert_eq!(out.len(), 4 + 200);
    }

    #[test]
    fn frame_close() {
        let mut out: Vec<u8> = Vec::new();
        send_close(&mut out).unwrap();
        assert_eq!(out, vec![0x88, 0x00]); // FIN|close, len 0
    }

    #[test]
    fn handshake_repond_101_et_accept() {
        let entetes = "GET /api/viz/subgraph/stream HTTP/1.1\r\nHost: h\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13";
        let mut out: Vec<u8> = Vec::new();
        let ok = handshake(&mut out, entetes).unwrap();
        assert!(ok);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 101 Switching Protocols\r\n"), "{s}");
        assert!(
            s.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"),
            "{s}"
        );
    }

    #[test]
    fn handshake_refuse_get_ordinaire() {
        let entetes = "GET /api/viz/subgraph HTTP/1.1\r\nHost: h\r\nAccept: */*";
        let mut out: Vec<u8> = Vec::new();
        let ok = handshake(&mut out, entetes).unwrap();
        assert!(!ok);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{s}");
    }
}
