//! CE QUE LE MOTEUR REFUSE, ET AU NOM DE QUOI — **dire vrai** (2026-09-16).
//!
//! Deux défauts de la même famille, tous deux MESURÉS sur des instances réelles
//! et non déduits par lecture. Ce fichier les rend VRAIS par la mesure, sur un
//! dispositif ISOLÉ (stores dans /tmp, cache dans /tmp, IAM temporaire, port
//! libre > 8400) — jamais sur un service de l'utilisateur.
//!
//! ── DÉFAUT 1 (manque 6) : une route inconnue se cachait derrière un refus ───
//! Constaté par le donneur d'ordre en mesurant, avec l'IAM armé :
//!
//! ```text
//! GET /api/harness/health   → 403 permission denied … tenant/<store>
//! GET /api/harness/machine  → 403 permission denied … tenant/<store>
//! GET /api/route-inexistante → 403 permission denied … tenant/<store>
//! ```
//!
//! Les trois corps ÉTAIENT identiques : impossible de distinguer « cette route
//! n'existe pas » de « tu n'as pas le droit », alors que les deux ne se
//! corrigent pas de la même façon. La garde IAM s'exécutait AVANT le routage.
//!
//! Le remède a une contrainte : il ne doit PAS transformer le serveur en ORACLE.
//! Si une route inconnue répondait `404` à qui n'a pas d'identité, un balayage
//! anonyme énumérerait la carte des routes. L'arbitrage retenu (commenté dans
//! `viz_serve.rs::route_api_connue`) est donc :
//!
//!   * SANS identité valide, le refus du `401` est RIGOUREUSEMENT le même pour
//!     une route connue et pour une route inexistante — ce que ce fichier
//!     vérifie par ÉGALITÉ DES CORPS, pas par ressemblance ;
//!   * AVEC une identité, les deux cas se distinguent : `404 route inconnue`
//!     contre `403 permission denied`, avec un champ `connue` lisible par un
//!     programme ;
//!   * sans IAM armé, la même phrase « route inconnue » — « sans IAM comme
//!     avec ».
//!
//! ── DÉFAUT 2 (défaillance 5.2) : la ressource jugée était un NOM DE FICHIER ─
//! `docs/DEVELOPPEMENT-SELF-PAR-COMPTE-2026-09-16.md` §5.2, mesuré :
//!
//! ```text
//! GET /api/auth/whoami        → {"authentifie":true,…}          identité VALIDE
//! GET /api/engine/memoire     → 403 permission denied: action=graph:read
//!                               resource=orn:obrain:graph:obrain:tenant/runtime-essai
//! ```
//!
//! Le compte s'appelait `alice`, son espace `esp-…`, et le refus parlait du NOM
//! DU STORE. Conséquence directe sur l'objectif « chaque utilisateur a un agent
//! sur SA base » : une base `selfs/<compte>.obrain` n'était pas lisible par son
//! propre compte dès que son espace ne portait pas le nom du fichier.
//!
//! Le remède juge le compte sur SON espace — et seulement quand la ressource est
//! VRAIMENT la sienne : `/api/engine/memoire` (sa mémoire) et la base que le
//! produit lui destine (`dirname(store)/selfs/<compte>.obrain`, la convention de
//! `engine.rs::profil_du_self`). Le dossier vient du SERVEUR et le nom de fichier
//! de l'IDENTITÉ : un compte ne peut donc pas se déclarer propriétaire de la base
//! du voisin. Ce fichier le vérifie dans les deux sens — ce qui passe ET ce qui
//! reste refusé.
//!
//! ── CE QUE CE FICHIER NE FAIT PAS ───────────────────────────────────────────
//! Il ne touche aucun service (ports 8432/8433/8455/8090/8144/3082/3083 refusés
//! par construction : on prend un port libre rendu par le noyau), il n'écrit dans
//! aucune base vivante, et il n'arme rien qui survive au test.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn obrain() -> Command {
    Command::new(env!("CARGO_BIN_EXE_obrain"))
}

/// Un port RÉELLEMENT libre : le noyau le rend, on ne le devine pas.
fn port_libre() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Une requête HTTP brute — le test parle au serveur RÉEL, par le réseau.
fn http(
    addr: &str,
    methode: &str,
    chemin: &str,
    jeton: Option<&str>,
    corps: Option<&str>,
) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap_or_else(|e| panic!("connexion {addr} : {e}"));
    let mut req = format!("{methode} {chemin} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n");
    if let Some(j) = jeton {
        req.push_str(&format!("Authorization: Bearer {j}\r\n"));
    }
    let corps = corps.map(str::to_string);
    if let Some(c) = &corps {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", c.len()));
    }
    req.push_str("\r\n");
    if let Some(c) = &corps {
        req.push_str(c);
    }
    s.write_all(req.as_bytes()).expect("envoi de la requête");
    let mut buf = String::new();
    s.read_to_string(&mut buf).expect("lecture de la réponse");
    let status: u16 = buf
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("réponse illisible : {buf}"));
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn json(corps: &str) -> serde_json::Value {
    serde_json::from_str(corps).unwrap_or_else(|e| panic!("JSON illisible ({e}) : {corps}"))
}

/// Pourcent-encodage minimal : suffisant pour un chemin de store et une requête
/// GQL passés en paramètres d'URL.
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `GET /api/query?store=…&query=…` — la route par laquelle on LIT une base.
fn lire_base(addr: &str, jeton: Option<&str>, base: &Path, gql: &str) -> (u16, String) {
    let chemin = format!(
        "/api/query?store={}&query={}",
        enc(&base.to_string_lossy()),
        enc(gql)
    );
    http(addr, "GET", &chemin, jeton, None)
}

/// Le serveur de test, tué à la sortie — même sur un `panic!` (sinon un port
/// resterait tenu par un processus orphelin après un test rouge).
struct Serveur(Child);

impl Drop for Serveur {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Le dispositif : deux comptes aux espaces NOMMÉS AUTREMENT que les fichiers,
/// la base propre de chacun, un store servi, un moteur ARMÉ sur un port libre.
///
/// ── POURQUOI LES ESPACES NE PORTENT PAS LE NOM DES BASES ────────────────────
/// C'est TOUT le sujet du défaut 2. La porte d'acceptation de la flotte nomme son
/// espace comme son store (« sinon toutes les routes inconnues répondent 403 et
/// masquent les 404 ») : ce contournement est légitime là-bas, mais il rend le
/// défaut invisible. Ici, l'espace d'`alice` est `esp-alice` et le store servi
/// s'appelle `service.obrain` : si la ressource jugée est un NOM DE FICHIER, le
/// refus tombe — et c'est précisément ce qu'on veut voir tomber.
struct Dispositif {
    _td: tempfile::TempDir,
    /// Gardé pour que le processus meure avec le test (voir `Drop`), jamais lu.
    _serveur: Serveur,
    addr: String,
    jeton_alice: String,
    jeton_bob: String,
    /// Le store SERVI par le moteur (celui du `--store`).
    store_service: PathBuf,
    /// `selfs/alice.obrain` — la base que le produit destine au compte `alice`.
    base_alice: PathBuf,
    /// `selfs/bob.obrain` — celle de `bob`.
    base_bob: PathBuf,
}

impl Dispositif {
    fn monter(armer: bool) -> Self {
        let td = tempfile::tempdir().unwrap();
        let cache = td.path().join("cache");
        std::fs::create_dir_all(cache.join("memoire")).unwrap();
        let iam = td.path().join("iam.obrain");
        let store_service = td.path().join("service.obrain");
        let selfs = td.path().join("selfs");
        std::fs::create_dir_all(&selfs).unwrap();
        let base_alice = selfs.join("alice.obrain");
        let base_bob = selfs.join("bob.obrain");

        for b in [&iam, &store_service, &base_alice, &base_bob] {
            let o = obrain().arg("init").arg(b).output().expect("init");
            assert!(
                o.status.success(),
                "init {} : {}",
                b.display(),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        // ── LA BASE D'ALICE N'EST PAS VIDE ─────────────────────────────────
        // Une base vide ne prouverait rien : « lue » et « ouverte sans contenu »
        // seraient indiscernables. Ce repère rend la lecture MESURABLE.
        let o = obrain()
            .arg("query")
            .arg(&base_alice)
            .arg("CREATE (:Repere {qn: 'repere-alice'})")
            .output()
            .expect("peuplement de la base d'alice");
        assert!(
            o.status.success(),
            "peuplement : {}",
            String::from_utf8_lossy(&o.stderr)
        );

        // ── LES COMPTES EXISTENT AVANT QUE LE MOTEUR N'OUVRE LE MAGASIN ────
        // `obrain iam` exige le verrou EXCLUSIF : créés après le démarrage, ils
        // échoueraient sur « substrate … already open by another process », et le
        // test mesurerait cet accident d'ordonnancement au lieu de son sujet.
        let o = obrain()
            .arg("iam")
            .arg("amorcer")
            .arg(&iam)
            .arg("esp-alice")
            .arg("--utilisateur")
            .arg("alice")
            .arg("--mot-de-passe")
            .arg(MDP_ALICE)
            .output()
            .expect("iam amorcer");
        assert!(
            o.status.success(),
            "amorçage : {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let o = obrain()
            .arg("iam")
            .arg("ajouter")
            .arg(&iam)
            .arg("bob")
            .arg("--espace")
            .arg("esp-bob")
            .arg("--mot-de-passe")
            .arg(MDP_BOB)
            .output()
            .expect("iam ajouter");
        assert!(
            o.status.success(),
            "ajout de bob : {}",
            String::from_utf8_lossy(&o.stderr)
        );

        let port = port_libre();
        let addr = format!("127.0.0.1:{port}");
        let journal = td.path().join("serveur.log");
        let mut c = Command::new(env!("CARGO_BIN_EXE_obrain"));
        c.arg("viz")
            .arg("serve")
            .arg("--store")
            .arg(&store_service)
            .arg("--enrich-ricci")
            .arg("0")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .env("OBRAIN_VIZ_CACHE_DIR", &cache)
            // ── AUCUN MONDE ÉTRANGER DANS CE DISPOSITIF ────────────────────
            // Le registre des mondes vit sous `OBRAIN_WORLDS_DIR` (défaut
            // `/tmp/obrain-worlds`) : le laisser par défaut ferait entrer dans ce
            // test les bases ajoutées par d'autres sessions, et le test
            // dépendrait de l'état d'une machine au lieu de son propre décor.
            .env("OBRAIN_WORLDS_DIR", td.path().join("mondes"))
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&journal).expect("log du serveur"));
        if armer {
            c.env("OBRAIN_IAM", "1")
                .env("OBRAIN_IAM_STORE", &iam)
                .env("OBRAIN_IAM_ACCOUNT", "obrain");
        } else {
            c.env_remove("OBRAIN_IAM").env_remove("OBRAIN_IAM_STORE");
        }
        let serveur = Serveur(c.spawn().expect("spawn viz serve"));

        let fin = Instant::now() + Duration::from_secs(90);
        let mut pret = false;
        while Instant::now() < fin {
            if TcpStream::connect(&addr).is_ok() {
                pret = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !pret {
            panic!(
                "serveur jamais prêt sur {addr}\n{}",
                std::fs::read_to_string(&journal).unwrap_or_default()
            );
        }

        let texte = std::fs::read_to_string(&journal).unwrap_or_default();
        if armer {
            // La garde ARMÉE est la condition de tout ce qui suit : sans elle, il
            // n'y aurait ni refus ni jugement, et le test ne prouverait rien.
            assert!(
                texte.contains("IAM ARMÉ"),
                "le serveur de test devait être armé :\n{texte}"
            );
        } else {
            assert!(
                !texte.contains("IAM ARMÉ"),
                "ce dispositif devait être NON armé (c'est le sujet de l'assertion) :\n{texte}"
            );
        }
        eprintln!(
            "── GARDE DU SERVEUR DE TEST ──\n{}",
            texte
                .lines()
                .filter(|l| l.contains("viz serve :"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let connexion = |nom: &str, mdp: &str| -> String {
            let corps = serde_json::json!({ "username": nom, "password": mdp }).to_string();
            let (st, c) = http(&addr, "POST", "/api/auth/login", None, Some(&corps));
            assert_eq!(st, 200, "connexion de {nom} : {c}");
            json(&c)["jeton"]
                .as_str()
                .unwrap_or_else(|| panic!("jeton absent : {c}"))
                .to_string()
        };
        let (jeton_alice, jeton_bob) = if armer {
            (connexion("alice", MDP_ALICE), connexion("bob", MDP_BOB))
        } else {
            (String::new(), String::new())
        };

        Dispositif {
            _td: td,
            _serveur: serveur,
            addr,
            jeton_alice,
            jeton_bob,
            store_service,
            base_alice,
            base_bob,
        }
    }
}

const MDP_ALICE: &str = "mdp-alice-ch6";
const MDP_BOB: &str = "mdp-bob-ch6";

/// ── DÉFAUT 1 : LES TROIS NOMS DU CONSTAT DU DONNEUR D'ORDRE ──────────────────
///
/// `GET /api/harness/health`, `GET /api/harness/machine` et un nom inventé
/// disaient tous les trois « permission refusée ». Avec une identité, ils doivent
/// maintenant dire « route inconnue » — et une route CONNUE sans droit doit
/// continuer de dire « permission refusée ». C'est le CONTRASTE qui fait la
/// preuve : sans lui, on ne saurait pas que le `403` existe encore.
#[test]
fn une_route_inconnue_se_dit_inconnue_avec_une_identite() {
    let d = Dispositif::monter(true);

    for chemin in [
        "/api/harness/health",
        "/api/harness/machine",
        "/api/route-absente-ch6",
    ] {
        let (code, corps) = http(&d.addr, "GET", chemin, Some(&d.jeton_alice), None);
        assert_eq!(
            code, 404,
            "GET {chemin} devrait dire « inconnue » : {corps}"
        );
        let v = json(&corps);
        assert!(
            v["error"]
                .as_str()
                .is_some_and(|e| e.contains("route inconnue")),
            "GET {chemin} : le corps doit NOMMER « route inconnue », pas « permission denied » : {corps}"
        );
        assert!(
            v["error"]
                .as_str()
                .is_some_and(|e| !e.contains("permission denied")),
            "GET {chemin} : un refus de droit ne doit pas se déguiser en route inconnue : {corps}"
        );
        assert_eq!(
            v["connue"],
            serde_json::json!(false),
            "GET {chemin} : `connue` doit être lisible par un programme : {corps}"
        );
        assert_eq!(
            v["espace"],
            serde_json::Value::Null,
            "GET {chemin} : une route inconnue n'a jugé AUCUN espace — en nommer un \
             désignerait une ressource que la requête n'a jamais visée : {corps}"
        );
    }

    // ── LE CONTRASTE : UNE ROUTE QUI EXISTE, ET QUI EST REFUSÉE ─────────────
    // `/api/query` existe, et `alice` n'a aucun droit sur l'espace du store
    // servi (`service`). Elle doit donc recevoir un refus de DROIT — pas un 404.
    let (code, corps) = lire_base(
        &d.addr,
        Some(&d.jeton_alice),
        &d.store_service,
        "MATCH (n) RETURN n",
    );
    assert_eq!(
        code, 403,
        "un refus de droit reste un refus de droit : {corps}"
    );
    let v = json(&corps);
    assert!(
        v["error"]
            .as_str()
            .is_some_and(|e| e.contains("permission denied")),
        "le refus doit être MOTIVÉ par le droit : {corps}"
    );
    assert_eq!(
        v["connue"],
        serde_json::json!(true),
        "cette route existe : {corps}"
    );
    assert_eq!(
        v["espace"],
        serde_json::json!("service"),
        "un refus de droit NOMME l'espace jugé (le stem du store servi) : {corps}"
    );
}

/// ── DÉFAUT 1, LA CONTRE-ÉPREUVE : PAS D'ORACLE SANS IDENTITÉ ────────────────
///
/// Le remède serait pire que le mal s'il apprenait à un balayage anonyme quelles
/// routes existent. On exige donc l'ÉGALITÉ DES CORPS entre une route connue et
/// une route inexistante, sans identité : même code, même texte, même corps
/// d'octets. C'est une assertion stricte, et c'est voulu — une ressemblance ne
/// prouve rien.
#[test]
fn sans_identite_le_refus_ne_dit_pas_quelles_routes_existent() {
    let d = Dispositif::monter(true);

    let (c_connue, corps_connue) = http(&d.addr, "GET", "/api/query", None, None);
    let (c_inconnue, corps_inconnue) = http(&d.addr, "GET", "/api/route-absente-ch6", None, None);
    assert_eq!(
        c_connue, 401,
        "sans identité, une route connue se refuse : {corps_connue}"
    );
    assert_eq!(
        c_inconnue, 401,
        "sans identité, une route inconnue se refuse AUSSI : {corps_inconnue}"
    );
    assert_eq!(
        corps_connue, corps_inconnue,
        "LE CORPS DU 401 DOIT ÊTRE IDENTIQUE — sinon le refus énumère la carte des routes \
         à qui n'a pas encore d'identité (l'oracle qu'on refuse de créer)"
    );

    // Et un jeton qui ne vaut rien ne doit pas ouvrir la distinction non plus.
    let (c_faux, corps_faux) = http(
        &d.addr,
        "GET",
        "/api/route-absente-ch6",
        Some("jeton-faux"),
        None,
    );
    let (c_faux_connue, corps_faux_connue) =
        http(&d.addr, "GET", "/api/query", Some("jeton-faux"), None);
    assert_eq!(
        (c_faux, c_faux_connue),
        (401, 401),
        "un jeton invalide ne vaut pas mieux que pas de jeton"
    );
    assert_eq!(
        corps_faux, corps_faux_connue,
        "même règle pour un jeton invalide : le corps ne trahit pas l'existence de la route"
    );

    // ── LA MÊME SITUATION SE DIT PARTOUT DE LA MÊME FAÇON ──────────────────
    // `/api/engine/memoire` est jugée sur les espaces du compte (elle sert SA
    // mémoire) : sans identité, elle doit refuser avec le MÊME corps que le reste
    // du serveur. Un texte différent aurait suffi à deviner la route par la forme
    // de son refus — le défaut qu'on vient de fermer, par la porte d'à côté.
    let (c_memoire, corps_memoire) = http(&d.addr, "GET", "/api/engine/memoire", None, None);
    assert_eq!(
        c_memoire, 401,
        "sans identité, la mémoire du compte se refuse : {corps_memoire}"
    );
    assert_eq!(
        corps_memoire, corps_connue,
        "le refus d'identité doit être le MÊME sur toutes les routes de données — \
         sinon sa forme énumère les routes"
    );
}

/// ── « SANS IAM COMME AVEC » ─────────────────────────────────────────────────
///
/// La même phrase, sur un moteur NON armé : une route inconnue se dit inconnue.
/// Le code était déjà `404` avant ce chantier, mais son corps était `not found`,
/// en clair et sans dire que ce n'était PAS un refus — deux formes pour une même
/// vérité. Ici les deux régimes répondent la même chose.
#[test]
fn sans_iam_une_route_inconnue_se_dit_aussi_inconnue() {
    let d = Dispositif::monter(false);

    let (code, corps) = http(&d.addr, "GET", "/api/route-absente-ch6", None, None);
    assert_eq!(
        code, 404,
        "sans IAM, une route inconnue se dit inconnue : {corps}"
    );
    let v = json(&corps);
    assert!(
        v["error"]
            .as_str()
            .is_some_and(|e| e.contains("route inconnue")),
        "la phrase doit être la MÊME qu'avec l'IAM armé : {corps}"
    );
    assert_eq!(v["connue"], serde_json::json!(false), "{corps}");

    // Une route connue, elle, RÉPOND (le dispositif non armé n'a pas de droits à
    // juger) : le test ne doit pas confondre « inconnue » et « plus rien ne marche ».
    let (code, corps) = lire_base(&d.addr, None, &d.store_service, "MATCH (n) RETURN n");
    assert_eq!(
        code, 200,
        "une route connue répond sur un moteur non armé : {corps}"
    );
    assert!(
        json(&corps)["rows"].is_array(),
        "et elle rend des rangées : {corps}"
    );
}

/// ── DÉFAUT 2 : LE COMPTE EST JUGÉ SUR SON ESPACE, PAS SUR UN NOM DE FICHIER ──
///
/// Le cas mesuré par le rapport : identité VALIDE, espace `esp-alice`, store
/// servi nommé `service.obrain`, base du compte `selfs/alice.obrain`. La politique
/// d'`alice` porte sur `esp-alice` — jamais sur `alice` ni sur `service`.
///
/// Ce qui doit devenir vrai : elle lit SA base et SA mémoire. Ce qui doit RESTER
/// refusé : la base de bob, et l'espace du store servi — sinon la correction
/// aurait remplacé un refus injuste par un droit injuste, ce qui n'est pas un
/// progrès.
#[test]
fn un_compte_lit_la_base_qui_lui_appartient_malgre_son_espace() {
    let d = Dispositif::monter(true);

    // 1. LA BASE QUI LUI APPARTIENT — et l'on vérifie que c'est bien SON contenu.
    let (code, corps) = lire_base(
        &d.addr,
        Some(&d.jeton_alice),
        &d.base_alice,
        "MATCH (n:Repere) RETURN n.qn",
    );
    assert_eq!(
        code,
        200,
        "alice doit lire sa propre base `{}` (espace `esp-alice`, store servi `{}`) : {corps}\n\
         AVANT ce chantier, la ressource jugée était le nom du fichier (`tenant/alice`), \
         et le compte était refusé chez lui.",
        d.base_alice.display(),
        d.store_service.display()
    );
    assert!(
        corps.contains("repere-alice"),
        "elle doit lire SON contenu, pas seulement obtenir un 200 : {corps}"
    );

    // 2. SA MÉMOIRE — le cas exact de la défaillance 5.2.
    let (code, corps) = http(
        &d.addr,
        "GET",
        "/api/engine/memoire",
        Some(&d.jeton_alice),
        None,
    );
    assert_eq!(
        code, 200,
        "alice doit lire SA mémoire ; le refus mesuré parlait de \
         `resource=orn:…:tenant/service`, un espace qu'elle n'a jamais visé : {corps}"
    );
    let v = json(&corps);
    assert_eq!(
        v["self"].as_str(),
        Some("self:alice"),
        "et cette mémoire est bien la SIENNE : {corps}"
    );

    // 3. LA BASE DU VOISIN RESTE FERMÉE — la propriété vient de l'IDENTITÉ, jamais
    //    d'un chemin que l'appelant déclare.
    let (code, corps) = lire_base(
        &d.addr,
        Some(&d.jeton_bob),
        &d.base_alice,
        "MATCH (n:Repere) RETURN n.qn",
    );
    assert_eq!(
        code, 403,
        "bob ne doit PAS lire la base d'alice en nommant son chemin : {corps}"
    );
    assert!(
        corps.contains("permission denied"),
        "et le refus doit être motivé : {corps}"
    );

    // 4. L'ESPACE DU STORE SERVI N'EST PAS LE SIEN — la règle ne doit rien ouvrir
    //    de plus que la base du compte.
    let (code, corps) = lire_base(
        &d.addr,
        Some(&d.jeton_alice),
        &d.store_service,
        "MATCH (n) RETURN n",
    );
    assert_eq!(
        code, 403,
        "alice ne doit pas lire le store servi (`service`) : {corps}"
    );
    assert_eq!(
        json(&corps)["espace"],
        serde_json::json!("service"),
        "et le refus dit sur quel espace il a été prononcé : {corps}"
    );

    // 5. SANS IDENTITÉ, AUCUNE BASE DE COMPTE N'EST SERVIE.
    let (code, corps) = lire_base(&d.addr, None, &d.base_alice, "MATCH (n:Repere) RETURN n.qn");
    assert_eq!(
        code, 401,
        "sans identité, la base d'un compte reste fermée : {corps}"
    );

    // 6. BOB LIT LA SIENNE — deux comptes, deux bases, aucune confusion : c'est le
    //    fond de l'objectif « chaque utilisateur a un agent sur SA base ».
    let (code, corps) = lire_base(
        &d.addr,
        Some(&d.jeton_bob),
        &d.base_bob,
        "MATCH (n) RETURN n",
    );
    assert_eq!(
        code,
        200,
        "bob doit lire SA propre base `{}` : {corps}",
        d.base_bob.display()
    );
}

/// ── LA GARDE DE LA GARDE : LE PRÉDICAT COUVRE LE ROUTEUR ────────────────────
///
/// `route_api_connue` est une SECONDE liste à côté du `match path` du routeur —
/// exactement ce que ce fichier de serveur dénonce ailleurs (« deux sources pour
/// une même vérité finissent par diverger »). Le prédicat a été écrit comme un
/// SUR-ENSEMBLE justement pour que l'erreur se voie : un chemin déclaré connu qui
/// n'existe pas retombe sur le comportement d'avant, alors qu'une route SERVIE
/// qui manquerait au prédicat répondrait `404` — elle cesserait de marcher.
///
/// Cette erreur-là ne doit pas attendre un utilisateur pour se voir. Ce test LIT
/// LA SOURCE du routeur et compare les deux tables, littéralement : le jour où
/// quelqu'un ajoute une route sans la déclarer, il échoue en la NOMMANT. Aucune
/// troisième liste n'est entretenue à la main — les deux tables sont extraites du
/// fichier réel, donc ce contrôle ne peut pas diverger à son tour.
#[test]
fn le_predicat_de_routes_couvre_toutes_les_routes_servies() {
    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/viz_serve.rs"));

    /// Tous les littéraux `/api/…` d'un fragment de source, dans l'ordre.
    fn litteraux_api(fragment: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut reste = fragment;
        while let Some(i) = reste.find("\"/api/") {
            let apres = &reste[i + 1..]; // commence par `/api/`
            let Some(k) = apres.find('"') else { break };
            out.push(apres[..k].to_string());
            reste = &apres[k + 1..];
        }
        out
    }

    // La table du ROUTEUR : le `match path` de `router_requete`, jusqu'à son
    // repli `_ =>`.
    let debut = source.find("fn router_requete").expect("router_requete");
    let dispatch = source[debut..].find("match path {").expect("match path") + debut;
    let repli = source[dispatch..]
        .find("_ => respond(&mut stream, 404, \"text/plain\", \"not found\")")
        .expect("repli du routeur")
        + dispatch;
    let servies = litteraux_api(&source[dispatch..repli]);

    // La table du PRÉDICAT.
    let p_debut = source
        .find("fn route_api_connue")
        .expect("route_api_connue");
    let p_fin = source[p_debut..].find("\n}\n").expect("fin du prédicat") + p_debut;
    let declarees = litteraux_api(&source[p_debut..p_fin]);

    // ── LES ROUTES EXACTES, DÉDOUBLONNÉES ─────────────────────────────────
    // Le routeur nomme deux fois `/api/worlds` et `/api/engine/resonance` (GET et
    // POST) ; le filet `/api/` n'est pas une route, c'est le filet.
    let mut exactes: Vec<&String> = servies
        .iter()
        .filter(|c| *c != "/api/" && !c.ends_with('/'))
        .collect();
    exactes.sort();
    exactes.dedup();
    assert_eq!(
        exactes.len(),
        29,
        "le routeur ne sert plus 29 routes `/api/…` nommées : si ce nombre a changé, relisez ce \
         test — il est là pour VOUS prévenir que la table a bougé, pas pour vous contrarier. \
         Servies : {exactes:?}"
    );

    // Les deux préfixes sous-routés ont leur propre repli, qui dit déjà
    // « route inconnue » : ils sont déclarés connus en bloc.
    let sous_router = |c: &str| c.starts_with("/api/remote/") || c.starts_with("/api/iam/");
    let manquantes: Vec<&&String> = exactes
        .iter()
        .filter(|c| !declarees.contains(c) && !sous_router(c))
        .collect();
    assert!(
        manquantes.is_empty(),
        "ces routes sont SERVIES par le routeur mais absentes du prédicat `route_api_connue` : \
         {manquantes:?} — telles quelles, elles répondraient « route inconnue » alors qu'elles \
         existent. Déclarez-les dans `viz_serve.rs`, à côté de leurs sœurs."
    );
}
