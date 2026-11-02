//! # Le serveur témoin de l'étape 0
//!
//! Un HTTP/1.1 minimal — **le même dessin que la boucle de `viz_serve.rs`** :
//! lire jusqu'à la fin des en-têtes, décider, écrire une réponse avec sa
//! `Content-Length`. Il existe pour deux raisons :
//!
//! 1. **prouver l'étape 0 avec des outils indépendants** — `curl` et
//!    `openssl s_client` comme clients, `grep` comme vérificateur
//!    (`scripts/acceptation-tls.sh`) ;
//! 2. **montrer le contrat d'intégration** : `servir()` ci-dessous est
//!    exactement ce que `run_serve()` doit devenir — une poignée de main, puis
//!    la même boucle, avec `traiter()` écrit sur `S: Read + Write` et non sur
//!    `TcpStream`.
//!
//! ## Les deux modes
//!
//! ```text
//!   serveur_tls servir --port 0 --port-ecrit /tmp/p --cert … --cle … --jeton …
//!   serveur_tls servir --port 0 --clair          # LE TÉMOIN, jamais en vrai
//!   serveur_tls relais --ecoute 0 --vers 8443 --capture /tmp/fil.bin
//! ```
//!
//! Le mode `relais` est le `tcpdump` que la porte ne peut pas employer : cet
//! utilisateur n'a pas le droit de capturer sur `lo`. Il enregistre **tous** les
//! octets qui passent, dans les deux sens, et les écrit dans un fichier que la
//! porte interroge ensuite avec `grep`.
//!
//! Le mode `--clair` n'existe que pour le **témoin** de la porte : montrer que
//! la capture sait voir un jeton quand il passe en clair. Sans ce témoin, une
//! capture vide ferait passer la vérification pour de mauvaises raisons.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use obrain_tls::{ConfigTls, EcouteTls};

/// Délai de lecture d'une session établie : la même protection que
/// `viz_serve.rs` pose sur ses connexions, et que le TLS ne doit pas faire
/// perdre.
const DELAI_LECTURE: Duration = Duration::from_secs(5);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(mode) = args.first() else {
        eprintln!("usage : serveur_tls <servir|relais> [options]");
        std::process::exit(2);
    };

    let resultat = match mode.as_str() {
        "servir" => servir(&args),
        "relais" => relais(&args),
        autre => Err(format!("mode inconnu : {autre}")),
    };

    if let Err(e) = resultat {
        eprintln!("serveur_tls : {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Les arguments, sans dépendance
// ---------------------------------------------------------------------------

fn argument(args: &[String], nom: &str) -> Option<String> {
    let position = args.iter().position(|a| a == nom)?;
    args.get(position + 1).cloned()
}

fn drapeau(args: &[String], nom: &str) -> bool {
    args.iter().any(|a| a == nom)
}

fn argument_requis(args: &[String], nom: &str) -> Result<String, String> {
    argument(args, nom).ok_or_else(|| format!("{nom} est requis"))
}

/// Écrit le port réellement lié, pour que la porte n'ait pas à le deviner.
fn ecrire_port(args: &[String], port: u16) -> Result<(), String> {
    println!("port={port}");
    if let Some(chemin) = argument(args, "--port-ecrit") {
        std::fs::write(&chemin, port.to_string())
            .map_err(|e| format!("écriture de {chemin} : {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Le serveur
// ---------------------------------------------------------------------------

fn servir(args: &[String]) -> Result<(), String> {
    let port: u16 = argument(args, "--port")
        .unwrap_or_else(|| "8443".to_owned())
        .parse()
        .map_err(|e| format!("--port : {e}"))?;
    let jeton = argument_requis(args, "--jeton")?;

    if drapeau(args, "--clair") {
        // ── LE TÉMOIN ───────────────────────────────────────────────────────
        // Servir en clair n'existe ici que pour vérifier que la capture de la
        // porte *sait* voir un secret quand il n'est pas chiffré. Ce chemin ne
        // doit jamais être celui d'un service réel : rien dans `obrain-tls` ne
        // permet de l'atteindre, il faut écrire `TcpListener` à la main — et
        // c'est précisément ce que fait cette branche, visiblement.
        let ecoute = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        let port_reel = ecoute.local_addr().map_err(|e| e.to_string())?.port();
        ecrire_port(args, port_reel)?;
        for flux in ecoute.incoming() {
            let Ok(mut flux) = flux else { continue };
            let _ = flux.set_read_timeout(Some(DELAI_LECTURE));
            let jeton = jeton.clone();
            thread::spawn(move || {
                let _ = traiter(&mut flux, &jeton, "clair");
            });
        }
        return Ok(());
    }

    let certificat = PathBuf::from(argument_requis(args, "--cert")?);
    let cle = PathBuf::from(argument_requis(args, "--cle")?);
    let config = ConfigTls::depuis_fichiers(&certificat, &cle).map_err(|e| e.to_string())?;

    let ecoute = EcouteTls::lier(("127.0.0.1", port), config).map_err(|e| e.to_string())?;
    let port_reel = ecoute.adresse_locale().map_err(|e| e.to_string())?.port();
    ecrire_port(args, port_reel)?;

    // ── LA BOUCLE, TELLE QUE `run_serve` DOIT DEVENIR ───────────────────────
    // `accepter()` mène la poignée de main (et refuse un pair muet au bout du
    // délai) ; ce qui en sort se lit et s'écrit comme un `TcpStream`.
    loop {
        match ecoute.accepter() {
            Ok(mut flux) => {
                // Le protocole négocié est connu **après** la poignée de main :
                // c'est ce qui permet de le dire dans la réponse plutôt que de
                // le supposer.
                let transport = format!("{:?}", flux.protocole());
                let _ = flux.set_read_timeout(Some(DELAI_LECTURE));
                let jeton = jeton.clone();
                thread::spawn(move || {
                    let _ = traiter(&mut flux, &jeton, &transport);
                });
            }
            Err(e) => eprintln!("serveur_tls : poignée de main refusée : {e}"),
        }
    }
}

/// La boucle de traitement, **générique** — c'est le point de l'étape 0.
fn traiter<S: Read + Write>(flux: &mut S, jeton: &str, transport: &str) -> std::io::Result<()> {
    let mut recu = Vec::new();
    let mut tampon = [0u8; 4096];
    loop {
        let n = flux.read(&mut tampon)?;
        if n == 0 {
            return Ok(());
        }
        recu.extend_from_slice(&tampon[..n]);
        if recu.windows(4).any(|f| f == b"\r\n\r\n") {
            break;
        }
        if recu.len() > 64 * 1024 {
            return repondre(flux, 431, "en-tetes trop longs");
        }
    }

    let texte = String::from_utf8_lossy(&recu).to_string();
    let premiere = texte.lines().next().unwrap_or_default().to_owned();
    let chemin = premiere.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let autorise = texte
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
        .is_some_and(|l| l.contains(jeton));

    if chemin.starts_with("/api/info") {
        return repondre(
            flux,
            200,
            &format!("{{\"etape\":\"0\",\"transport\":\"{transport}\"}}"),
        );
    }

    if chemin.starts_with("/api/query") {
        // Le jeton est exigé **dans** le tunnel : le chiffrement protège le
        // trajet, il ne remplace pas l'authentification du porteur.
        if !autorise {
            return repondre(flux, 401, "{\"erreur\":\"jeton requis\"}");
        }
        let requete = chemin.split_once("q=").map_or("(sans requete)", |(_, q)| q);
        return repondre(
            flux,
            200,
            &format!("{{\"resultat\":\"RESULTAT-DU-CALCUL pour {requete}\"}}"),
        );
    }

    repondre(flux, 404, "{\"erreur\":\"chemin inconnu\"}")
}

fn repondre<S: Write>(flux: &mut S, code: u16, corps: &str) -> std::io::Result<()> {
    let motif = match code {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let reponse = format!(
        "HTTP/1.1 {code} {motif}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corps}",
        corps.len()
    );
    flux.write_all(reponse.as_bytes())?;
    flux.flush()
}

// ---------------------------------------------------------------------------
// Le relais enregistreur — le `tcpdump` que la porte ne peut pas employer
// ---------------------------------------------------------------------------

fn relais(args: &[String]) -> Result<(), String> {
    let ecoute_port: u16 = argument(args, "--ecoute")
        .unwrap_or_else(|| "0".to_owned())
        .parse()
        .map_err(|e| format!("--ecoute : {e}"))?;
    let vers: u16 = argument_requis(args, "--vers")?
        .parse()
        .map_err(|e| format!("--vers : {e}"))?;
    let capture = PathBuf::from(argument_requis(args, "--capture")?);

    let ecoute = TcpListener::bind(("127.0.0.1", ecoute_port)).map_err(|e| e.to_string())?;
    let port_reel = ecoute.local_addr().map_err(|e| e.to_string())?.port();
    ecrire_port(args, port_reel)?;

    let (client, _) = ecoute.accept().map_err(|e| e.to_string())?;
    let serveur = TcpStream::connect(("127.0.0.1", vers)).map_err(|e| e.to_string())?;
    for flux in [&client, &serveur] {
        let _ = flux.set_read_timeout(Some(Duration::from_secs(10)));
    }

    let vu = Arc::new(Mutex::new(Vec::new()));
    let vu_aller = Arc::clone(&vu);
    let (mut client_aller, mut serveur_aller) = (
        client.try_clone().map_err(|e| e.to_string())?,
        serveur.try_clone().map_err(|e| e.to_string())?,
    );
    let aller = thread::spawn(move || copier(&mut client_aller, &mut serveur_aller, &vu_aller));

    let (mut serveur_retour, mut client_retour) = (serveur, client);
    copier(&mut serveur_retour, &mut client_retour, &vu);
    let _ = aller.join();

    let octets = vu.lock().map_err(|e| e.to_string())?.clone();
    std::fs::write(&capture, &octets).map_err(|e| format!("capture : {e}"))?;
    println!("capture={} octets", octets.len());
    Ok(())
}

fn copier(lecteur: &mut TcpStream, ecrivain: &mut TcpStream, vu: &Arc<Mutex<Vec<u8>>>) {
    let mut tampon = [0u8; 8192];
    loop {
        match lecteur.read(&mut tampon) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Ok(mut vu) = vu.lock() {
                    vu.extend_from_slice(&tampon[..n]);
                }
                if ecrivain.write_all(&tampon[..n]).is_err() {
                    break;
                }
                let _ = ecrivain.flush();
            }
        }
    }
    let _ = ecrivain.shutdown(std::net::Shutdown::Write);
}
