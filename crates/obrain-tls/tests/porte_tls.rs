//! # La porte TLS — le critère de l'ADR, exécutable
//!
//! L'étape 0 de l'ADR s'énonce ainsi :
//!
//! > `tcpdump -A` sur une requête `/api/query` ne montre **ni** le jeton, **ni**
//! > la requête, **ni** le résultat.
//!
//! Ce fichier exécute ce critère sans `tcpdump` : un **relais enregistreur**
//! s'intercale entre le client et le serveur, et conserve **tous** les octets
//! qui passent, dans les deux sens. C'est exactement ce que verrait un
//! observateur du réseau, sans hypothèse sur ce qu'il sait décoder.
//!
//! ## Le témoin, sans lequel rien ne serait prouvé
//!
//! Un test de capture qui ne capturerait rien — relais mal branché, flux vide,
//! assertion sur un vecteur vide — passerait au vert en ne prouvant **rien**.
//! Le test `temoin_en_clair_la_capture_voit_bien_le_jeton` ferme cette porte :
//! le **même** montage, appliqué à un serveur en clair, **doit** retrouver le
//! jeton. Si le témoin cesse de voir le clair, c'est le montage qui est cassé,
//! et les autres tests ne veulent plus rien dire.
//!
//! Ce couple — le test et son témoin — est ce qui distingue une preuve d'une
//! décoration.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use obrain_tls::{ConfigTls, EcouteTls, ErreurTls};

// ---------------------------------------------------------------------------
// Le matériel de test
// ---------------------------------------------------------------------------

/// **Clé, certificat et autorité jetables**, engendrés pour ces tests et sans
/// aucune valeur : ils ne servent qu'à faire tourner une poignée de main sur la
/// boucle locale. Une clé privée versionnée est une mauvaise habitude ;
/// celles-ci sont l'exception assumée, et elles sont nommées pour qu'on ne s'y
/// trompe pas.
///
/// La chaîne est à deux niveaux — `Obrain Test CA` signe un feuillet
/// `CN=localhost` — parce qu'un certificat auto-signé marqué CA serait refusé
/// comme feuillet par webpki (`CaUsedAsEndEntity`), et à raison : c'est
/// exactement le genre de raccourci de test qui finit en production.
const CHEMIN_AUTORITE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/donnees/ca.pem");
const CHEMIN_CERTIFICAT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/donnees/cert-de-test.pem");
const CHEMIN_CLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/donnees/cle-de-test.pem");

/// Ce qu'un observateur ne doit **jamais** pouvoir lire. Ces trois chaînes sont
/// assez longues et assez distinctives pour qu'on ne les confonde pas avec du
/// bruit par hasard : une recherche d'occurrence a du sens sur elles.
const JETON: &str = "jeton-porteur-3f9a2b7c-1d4e-a-ne-jamais-traverser-le-reseau";
const REQUETE: &str = "graph:read MATCH (n:Client) WHERE n.iban = 'FR76-LE-SECRET-DU-CLIENT'";
const REPONSE: &str = "resultat-du-calcul-8c1d55aa-non-plus-jamais";

/// Ce que le serveur de test rapporte de la session établie.
type Negociation = (
    Option<rustls::ProtocolVersion>,
    Option<rustls::SupportedCipherSuite>,
);
type Rapport = Result<Negociation, String>;

fn contient(foin: &[u8], aiguille: &[u8]) -> bool {
    foin.windows(aiguille.len()).any(|f| f == aiguille)
}

/// La requête complète, avec son jeton : c'est ce qui passe sur le fil.
fn requete_du_client() -> Vec<u8> {
    format!(
        "GET /api/query?q={REQUETE} HTTP/1.1\r\nHost: localhost:8432\r\nAuthorization: Bearer {JETON}\r\n\r\n"
    )
    .into_bytes()
}

fn reponse_du_serveur() -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{REPONSE}",
        REPONSE.len()
    )
}

/// **Le corps de la boucle HTTP de `viz_serve.rs`, à la lettre** : lire jusqu'à
/// la fin des en-têtes, écrire une réponse.
///
/// Il est écrit sur un type **générique**, `S: Read + Write`, et non sur
/// `TcpStream` : c'est la démonstration que `FluxTls` s'y substitue sans qu'une
/// seule ligne de la boucle ne change. Le contrat d'intégration de l'étape 0
/// tient ou ne tient pas ici.
fn servir_un_echange<S: Read + Write>(flux: &mut S) -> std::io::Result<()> {
    let mut recu = Vec::new();
    let mut tampon = [0u8; 1024];
    loop {
        let n = flux.read(&mut tampon)?;
        if n == 0 {
            break;
        }
        recu.extend_from_slice(&tampon[..n]);
        if contient(&recu, b"\r\n\r\n") {
            break;
        }
    }
    flux.write_all(reponse_du_serveur().as_bytes())?;
    flux.flush()
}

fn config_de_test() -> ConfigTls {
    ConfigTls::depuis_fichiers(Path::new(CHEMIN_CERTIFICAT), Path::new(CHEMIN_CLE))
        .expect("la configuration de test doit se construire")
}

/// Un serveur chiffré qui sert **un** échange, puis rend son rapport.
fn serveur_tls() -> (u16, mpsc::Receiver<Rapport>) {
    let ecoute = EcouteTls::lier("127.0.0.1:0", config_de_test()).expect("écoute");
    let port = ecoute
        .adresse_locale()
        .expect("adresse locale")
        .port();

    let (emetteur, recepteur) = mpsc::channel();
    thread::spawn(move || {
        let rapport = (|| -> Rapport {
            let mut flux = ecoute
                .accepter()
                .map_err(|e| format!("poignée de main refusée : {e}"))?;
            let negociation = (flux.protocole(), flux.suite_de_chiffrement());
            servir_un_echange(&mut flux).map_err(|e| format!("échange : {e}"))?;
            Ok(negociation)
        })();
        let _ = emetteur.send(rapport);
    });
    (port, recepteur)
}

/// Un serveur **en clair**, pour le témoin. Même boucle, même réponse.
fn serveur_en_clair() -> (u16, mpsc::Receiver<Rapport>) {
    let ecoute = TcpListener::bind("127.0.0.1:0").expect("écoute");
    let port = ecoute.local_addr().expect("adresse locale").port();

    let (emetteur, recepteur) = mpsc::channel();
    thread::spawn(move || {
        let rapport = (|| -> Rapport {
            let (mut flux, _) = ecoute.accept().map_err(|e| e.to_string())?;
            servir_un_echange(&mut flux).map_err(|e| format!("échange : {e}"))?;
            Ok((None, None))
        })();
        let _ = emetteur.send(rapport);
    });
    (port, recepteur)
}

/// Le relais enregistreur : tout ce qui passe entre le client et le serveur est
/// conservé, dans les deux sens. C'est le `tcpdump` du test.
///
/// Les délais de lecture garantissent que le relais **se termine** : un test qui
/// peut se bloquer indéfiniment ne vaut pas mieux qu'un test absent.
fn relais_enregistreur(vers_port: u16) -> (u16, Arc<Mutex<Vec<u8>>>) {
    let ecoute = TcpListener::bind("127.0.0.1:0").expect("relais");
    let port = ecoute.local_addr().expect("adresse du relais").port();
    let vu = Arc::new(Mutex::new(Vec::new()));
    let vu_retour = Arc::clone(&vu);

    thread::spawn(move || {
        let (client, _) = ecoute.accept().expect("le relais doit accepter");
        let serveur = TcpStream::connect(("127.0.0.1", vers_port)).expect("le relais doit joindre");
        for flux in [&client, &serveur] {
            let _ = flux.set_read_timeout(Some(Duration::from_secs(5)));
        }

        let (mut client_aller, mut serveur_aller) =
            (client.try_clone().expect("clone"), serveur.try_clone().expect("clone"));
        let vu_aller = Arc::clone(&vu);
        let vers_serveur = thread::spawn(move || {
            copier(&mut client_aller, &mut serveur_aller, &vu_aller);
        });

        let (mut serveur_retour, mut client_retour) = (serveur, client);
        copier(&mut serveur_retour, &mut client_retour, &vu);
        let _ = vers_serveur.join();
    });

    (port, vu_retour)
}

fn copier(lecteur: &mut TcpStream, ecrivain: &mut TcpStream, vu: &Arc<Mutex<Vec<u8>>>) {
    let mut tampon = [0u8; 8192];
    loop {
        match lecteur.read(&mut tampon) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                vu.lock().expect("verrou").extend_from_slice(&tampon[..n]);
                if ecrivain.write_all(&tampon[..n]).is_err() {
                    break;
                }
                let _ = ecrivain.flush();
            }
        }
    }
    let _ = ecrivain.shutdown(std::net::Shutdown::Write);
}

fn racines_locales() -> rustls::RootCertStore {
    let mut racines = rustls::RootCertStore::empty();
    // L'autorité de test, et **elle seule** : le feuillet présenté par le
    // serveur ne s'auto-légitime pas, il est vérifié par la chaîne.
    let pem = std::fs::read(CHEMIN_AUTORITE).expect("autorité de test");
    let mut lecteur = std::io::BufReader::new(&pem[..]);
    for certificat in rustls_pemfile::certs(&mut lecteur) {
        racines
            .add(certificat.expect("certificat PEM"))
            .expect("ajout à la racine");
    }
    racines
}

fn client_tls(
    port: u16,
    racines: rustls::RootCertStore,
) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    let config = rustls::ClientConfig::builder_with_provider(obrain_tls::fournisseur())
        .with_safe_default_protocol_versions()
        .expect("versions de protocole")
        .with_root_certificates(racines)
        .with_no_client_auth();
    let nom = rustls::pki_types::ServerName::try_from("localhost").expect("nom de serveur");
    let connexion = rustls::ClientConnection::new(Arc::new(config), nom).expect("connexion cliente");
    let socket = TcpStream::connect(("127.0.0.1", port)).expect("connexion TCP");
    rustls::StreamOwned::new(connexion, socket)
}

// ---------------------------------------------------------------------------
// Le critère de l'ADR
// ---------------------------------------------------------------------------

/// **Le test central : rien du calcul ne traverse le réseau en clair.**
#[test]
fn rien_du_calcul_ne_traverse_le_reseau_en_clair() {
    let (port_serveur, rapport_serveur) = serveur_tls();
    let (port_relais, vu) = relais_enregistreur(port_serveur);

    {
        let mut client = client_tls(port_relais, racines_locales());
        client.write_all(&requete_du_client()).expect("écriture");
        client.flush().expect("flush");

        let mut reponse = Vec::new();
        client.read_to_end(&mut reponse).expect("lecture");
        let texte = String::from_utf8_lossy(&reponse);
        assert!(
            texte.contains(REPONSE),
            "la réponse doit arriver entière par le tunnel : {texte}"
        );
    }

    let negociation = rapport_serveur
        .recv_timeout(Duration::from_secs(5))
        .expect("le serveur doit rendre son rapport")
        .expect("l'échange doit réussir");
    let vu = vu.lock().expect("verrou").clone();

    // La capture n'est pas vide, et c'est bien du TLS : sans ces deux
    // assertions, tout ce qui suit pourrait être vrai d'un fichier vide.
    assert!(
        vu.len() > 200,
        "capture de {} octets : le test ne prouverait rien",
        vu.len()
    );
    assert_eq!(
        vu[0], 0x16,
        "le premier octet doit ouvrir un enregistrement de poignée de main TLS"
    );

    // Le critère, dans l'ordre où l'ADR l'énonce.
    for (nom, secret) in [
        ("le jeton porteur", JETON),
        ("la requête", REQUETE),
        ("le résultat", REPONSE),
        ("l'en-tête HTTP", "Authorization: Bearer"),
    ] {
        assert!(
            !contient(&vu, secret.as_bytes()),
            "{nom} est lisible sur le fil — l'étape 0 n'est pas tenue"
        );
    }

    // Et la négociation est celle qu'on croit avoir eue.
    assert_eq!(
        negociation.0,
        Some(rustls::ProtocolVersion::TLSv1_3),
        "TLS 1.3 doit être négocié"
    );
    let suite = negociation.1.expect("une suite négociée");
    // Toute suite de TLS 1.3 est AEAD par construction ; l'assertion qui compte
    // est donc le protocole, ci-dessus. Celle-ci nomme la suite négociée pour
    // qu'un changement de fournisseur cryptographique se voie.
    assert!(
        format!("{:?}", suite.suite()).starts_with("TLS13_"),
        "suite inattendue pour TLS 1.3 : {:?}",
        suite.suite()
    );
}

/// **Le témoin.** Le même montage, avec un serveur en clair : la capture doit
/// voir le jeton. Sans ce test, celui du dessus pourrait passer parce que la
/// capture ne capte rien.
#[test]
fn temoin_en_clair_la_capture_voit_bien_le_jeton() {
    let (port_serveur, rapport_serveur) = serveur_en_clair();
    let (port_relais, vu) = relais_enregistreur(port_serveur);

    {
        let mut client = TcpStream::connect(("127.0.0.1", port_relais)).expect("connexion");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("délai");
        client.write_all(&requete_du_client()).expect("écriture");
        client.flush().expect("flush");
        let mut reponse = Vec::new();
        let _ = client.read_to_end(&mut reponse);
    }

    let _ = rapport_serveur.recv_timeout(Duration::from_secs(5));
    let vu = vu.lock().expect("verrou").clone();

    assert!(
        contient(&vu, JETON.as_bytes()),
        "le témoin ne voit pas le jeton dans une capture EN CLAIR : le montage est cassé, \
         et le test chiffré ne prouve donc rien"
    );
    assert!(
        contient(&vu, REQUETE.as_bytes()),
        "le témoin ne voit pas la requête en clair : le montage est cassé"
    );
}

// ---------------------------------------------------------------------------
// Les refus, qui comptent autant que le succès
// ---------------------------------------------------------------------------

/// Un pair qui connecte puis se tait doit être **renvoyé**, pas attendu. Sans
/// délai, c'est le déni de service le moins cher du monde : mille connexions
/// muettes, mille fils immobilisés, zéro octet envoyé.
#[test]
fn un_pair_muet_est_renvoye_au_bout_du_delai() {
    let ecoute = EcouteTls::lier("127.0.0.1:0", config_de_test()).expect("écoute");
    let port = ecoute.adresse_locale().expect("adresse").port();

    let (emetteur, recepteur) = mpsc::channel();
    thread::spawn(move || {
        let debut = Instant::now();
        let resultat = ecoute.accepter_avec_delai(Duration::from_millis(300));
        let _ = emetteur.send((resultat.is_err(), debut.elapsed()));
    });

    // Le pair muet : il ouvre la connexion et n'envoie jamais rien.
    let _muet = TcpStream::connect(("127.0.0.1", port)).expect("connexion muette");

    let (refuse, ecoule) = recepteur
        .recv_timeout(Duration::from_secs(10))
        .expect("le serveur doit rendre la main");
    assert!(refuse, "un pair muet ne doit pas obtenir de session");
    assert!(
        ecoule < Duration::from_secs(5),
        "le délai doit être respecté, écoulé : {ecoule:?}"
    );
}

/// **Aucune descente en clair.** Un client qui parle HTTP en clair sur le port
/// chiffré ne doit obtenir **aucune** réponse HTTP : ni la sienne, ni une
/// réponse d'erreur qui trahirait le contenu.
#[test]
fn du_http_en_clair_sur_le_port_chiffre_ne_rend_aucune_reponse() {
    let (port, rapport_serveur) = serveur_tls();

    let mut brut = TcpStream::connect(("127.0.0.1", port)).expect("connexion");
    brut.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("délai");
    let _ = brut.write_all(&requete_du_client());
    let _ = brut.flush();

    let mut recu = Vec::new();
    let _ = brut.read_to_end(&mut recu);
    let texte = String::from_utf8_lossy(&recu);

    assert!(
        !texte.contains("HTTP/1.1"),
        "une réponse HTTP est sortie d'un port chiffré : {texte}"
    );
    assert!(
        !texte.contains(REPONSE),
        "le résultat a fui en clair vers un client qui n'a pas fait de TLS"
    );

    // Le serveur, lui, a bien refusé — et il le dit.
    let rapport = rapport_serveur
        .recv_timeout(Duration::from_secs(5))
        .expect("rapport du serveur");
    assert!(
        rapport.is_err(),
        "le serveur ne doit pas avoir servi cette connexion : {rapport:?}"
    );
}

/// Un client qui ne croit pas le certificat doit échouer : c'est la moitié
/// « vérifiable par la partie qui la subit » du critère de garantie de l'ADR.
/// Un client qui accepterait n'importe quel certificat ne serait pas protégé
/// d'un intermédiaire, et croire le contraire serait pire que de ne rien faire.
#[test]
fn un_client_qui_ne_croit_pas_le_certificat_est_refuse() {
    let (port, rapport_serveur) = serveur_tls();

    let mut racines = rustls::RootCertStore::empty();
    racines.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let mut client = client_tls(port, racines);
    let ecriture = client
        .write_all(&requete_du_client())
        .and_then(|()| client.flush());
    let mut tampon = [0u8; 64];
    let lecture = client.read(&mut tampon);

    assert!(
        ecriture.is_err() || lecture.is_err(),
        "un certificat non cru doit faire échouer la session, pas passer"
    );

    let rapport = rapport_serveur
        .recv_timeout(Duration::from_secs(5))
        .expect("rapport du serveur");
    assert!(
        rapport.is_err(),
        "aucun échange n'aurait dû aboutir : {rapport:?}"
    );
}

/// Une erreur de configuration a lieu **avant toute écoute** : le serveur n'a
/// jamais accepté la moindre connexion, donc rien n'a pu être servi en clair.
/// C'est ce qui autorise une surface à annoncer « je ne suis pas démarré »
/// plutôt que de laisser croire qu'elle sert quelque chose.
#[test]
fn sans_certificat_aucune_ecoute_n_existe() {    let erreur = ConfigTls::depuis_fichiers(
        Path::new("/inexistant/certificat.pem"),
        Path::new(CHEMIN_CLE),
    )
    .expect_err("une configuration sans certificat doit être refusée");

    assert!(
        matches!(erreur, ErreurTls::CertificatAbsent { .. }),
        "attendu CertificatAbsent, obtenu {erreur:?}"
    );
    assert!(
        erreur.avant_toute_ecoute(),
        "une erreur de configuration précède toute écoute"
    );

    // Et il n'existe aucun chemin pour écouter sans configuration : `lier` en
    // exige une, **par valeur**. C'est une propriété de signature, donc
    // vérifiée à la compilation ; ce test ne peut que la constater, en montrant
    // qu'une configuration valide, elle, produit bien une écoute.
    let ecoute = EcouteTls::lier("127.0.0.1:0", config_de_test()).expect("écoute");
    assert!(ecoute.adresse_locale().is_ok());
    assert_eq!(ecoute.config().nb_certificats(), 1);
}

/// **La protection contre le slowloris survit à la migration vers le chiffré.**
///
/// `viz_serve.rs` pose un délai de lecture sur chaque connexion : ce n'est pas
/// un confort, c'est ce qui empêche un client d'annoncer une requête et de ne
/// jamais l'envoyer. Si `FluxTls` ne l'exposait pas, passer au TLS perdrait
/// cette protection **en silence** — la boucle aurait l'air identique, et le
/// serveur deviendrait attaquable d'une façon qu'il ne l'était pas.
///
/// Ce test est donc un garde-fou de **non-régression** sur la migration, pas
/// une vérification du chiffrement.
#[test]
fn un_delai_de_lecture_reste_posable_apres_la_poignee_de_main() {
    let ecoute = EcouteTls::lier("127.0.0.1:0", config_de_test()).expect("écoute");
    let port = ecoute.adresse_locale().expect("adresse").port();

    let (emetteur, recepteur) = mpsc::channel();
    thread::spawn(move || {
        let rapport = (|| -> Result<String, String> {
            let mut flux = ecoute.accepter().map_err(|e| e.to_string())?;
            flux.set_read_timeout(Some(Duration::from_millis(300)))
                .map_err(|e| e.to_string())?;
            flux.set_write_timeout(Some(Duration::from_millis(300)))
                .map_err(|e| e.to_string())?;
            flux.set_nodelay(true).map_err(|e| e.to_string())?;

            let debut = Instant::now();
            let mut tampon = [0u8; 64];
            match flux.read(&mut tampon) {
                Ok(n) => Err(format!("un client muet a envoyé {n} octets")),
                Err(e) => Ok(format!("{:?} après {:?}", e.kind(), debut.elapsed())),
            }
        })();
        let _ = emetteur.send(rapport);
    });

    // Le client mène sa poignée de main **puis se tait** : c'est exactement le
    // client que le délai doit renvoyer.
    let mut client = client_tls(port, racines_locales());
    client
        .conn
        .complete_io(&mut client.sock)
        .expect("la poignée de main cliente doit aboutir");

    let motif = recepteur
        .recv_timeout(Duration::from_secs(10))
        .expect("le serveur doit rendre la main")
        .expect("le serveur devait rendre un motif, pas un échec de test");

    assert!(
        motif.starts_with("TimedOut") || motif.starts_with("WouldBlock"),
        "un client muet doit être renvoyé par le délai, motif rendu : {motif}"
    );
}
