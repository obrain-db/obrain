//! LE CHEMIN D'ÉCRITURE (2026-09-15) — une machine attachée NOURRIT la base de
//! SON compte.
//!
//! ── CE QUI ÉTAIT FAUX, ET QUE CE TEST REND VRAI ─────────────────────────────
//! Jusqu'ici une machine attachée LISAIT le self de son compte et ne pouvait
//! rien y écrire (`obrain-selfd/src/soi.rs` le disait à l'agent, et le nœud
//! gardait ses notes en local). Donc « chaque utilisateur a un agent qui se
//! développe réellement sur SA base de connaissances » était faux.
//!
//! Ce test déroule la chaîne COMPLÈTE, sur des serveurs et des bases
//! TEMPORAIRES, et vérifie ce qu'elle REFUSE autant que ce qu'elle fait :
//!
//!   1. deux comptes réels (IAM armé, deux comptes, deux jetons) attachent
//!      chacun leur machine → deux `client_id` ;
//!   2. le compte A publie des intentions par `POST /api/remote/memoire` ;
//!   3. elles apparaissent dans le JOURNAL DU SELF DU COMPTE A — et le `self`
//!      que le corps nommait est IGNORÉ (c'est le point de sécurité : sans lui,
//!      une machine écrirait chez le voisin) ;
//!   4. la passe d'application (`obrain memoire --appliquer`) les fait atterrir
//!      dans la BASE du compte A ;
//!   5. la base du compte B n'a PAS bougé — empreinte des octets ET nombre de
//!      nœuds, mesurés avant et après ;
//!   6. aucun refus (client d'un autre compte, intention informe, kind inconnu,
//!      sans identité) n'écrit une seule ligne.
//!
//! ── POURQUOI LE SERVEUR EST CELUI DU BINAIRE, PAS UNE MAQUETTE ──────────────
//! Une maquette testerait la maquette. On lance `obrain viz serve` comme le
//! service 8432 le fait, avec un port LIBRE et `--enrich-ricci 0` : le store
//! servi est un store temporaire, et les bases des deux comptes ne sont JAMAIS
//! servies — c'est ce qui permet à la passe d'application de prendre le verrou
//! EXCLUSIF sans arrêter quoi que ce soit (le motif exact que `scripts/memoire.sh`
//! résout en arrêtant le service).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn obrain() -> Command {
    Command::new(env!("CARGO_BIN_EXE_obrain"))
}

fn port_libre() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
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

/// Le serveur de test, tué à la sortie — même sur un `panic!` (sinon un port
/// resterait tenu par un processus orphelin après un test rouge).
struct Serveur(Child);

impl Drop for Serveur {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Le chemin du journal de mémoire d'un self, demandé au binaire.
fn journal_de(cache: &Path, self_qn: &str) -> PathBuf {
    let out = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        .arg("memoire")
        .arg("--self")
        .arg(self_qn)
        .output()
        .expect("memoire --self");
    let txt = String::from_utf8_lossy(&out.stdout);
    let d = json(txt.trim());
    PathBuf::from(d["journal"].as_str().unwrap_or_else(|| panic!("journal absent : {txt}")))
}

fn lignes(chemin: &Path) -> usize {
    match std::fs::read_to_string(chemin) {
        Ok(t) => t.lines().filter(|l| !l.trim().is_empty()).count(),
        Err(_) => 0,
    }
}

/// Le nombre de lignes que rend une requête GQL, sur une base donnée.
fn compte(base: &Path, cache: &Path, gql: &str) -> usize {
    let out = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        .arg("query")
        .arg(base)
        .arg(gql)
        .arg("--format")
        .arg("json")
        .output()
        .expect("query");
    let txt = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    // Le stdout peut porter des avertissements avant le JSON.
    let debut = txt
        .find(['[', '{'])
        .unwrap_or_else(|| panic!("aucun JSON dans « {txt} » (statut {:?}, stderr : {err})", out.status));
    let v = json(&txt[debut..]);
    match &v {
        serde_json::Value::Object(o) => {
            o.get("rows").and_then(|r| r.as_array()).map(Vec::len).unwrap_or(0)
        }
        serde_json::Value::Array(a) => a.len(),
        _ => 0,
    }
}

/// Empreinte d'une base : chemins, tailles et dates de TOUT ce qu'elle contient.
///
/// Un compte de nœuds ne suffit pas à dire « intact » : un nœud peut être
/// modifié sans que le nombre change. On compare donc aussi les octets et les
/// dates, ce qui attrape une écriture comme une suppression.
fn empreinte(base: &Path) -> Vec<String> {
    fn parcourir(d: &Path, prefixe: &str, sortie: &mut Vec<String>) {
        let Ok(lectures) = std::fs::read_dir(d) else {
            return;
        };
        for e in lectures.flatten() {
            let p = e.path();
            let nom = format!("{prefixe}{}", e.file_name().to_string_lossy());
            let m = e.metadata().expect("métadonnées");
            if m.is_dir() {
                sortie.push(format!("{nom}/"));
                parcourir(&p, &format!("{nom}/"), sortie);
            } else {
                let date = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                sortie.push(format!("{nom} {} {date}", m.len()));
            }
        }
    }
    let mut v = Vec::new();
    parcourir(base, "", &mut v);
    v.sort();
    v
}

/// Lance `viz serve` comme le service le fait — même binaire, port LIBRE,
/// `--enrich-ricci 0` (le store servi est temporaire, on ne veut aucun entretien
/// qui écrive), et un store SERVI qui n'est ni la base d'Alice ni celle de Bob :
/// c'est ce qui laisse la passe d'application prendre le verrou exclusif.
///
/// `iam: None` = serveur SANS identité (le cas de l'occupant unique), pour
/// vérifier qu'il refuse d'écrire plutôt que de deviner.
fn lancer_serveur(
    store: &Path,
    cache: &Path,
    iam: Option<&Path>,
    log: &Path,
) -> (Serveur, String) {
    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    let mut c = Command::new(env!("CARGO_BIN_EXE_obrain"));
    c.arg("viz").arg("serve")
        .arg("--store").arg(store)
        .arg("--enrich-ricci").arg("0")
        .arg("--host").arg("127.0.0.1")
        .arg("--port").arg(port.to_string())
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(log).expect("log du serveur"));
    if let Some(i) = iam {
        c.env("OBRAIN_IAM", "1").env("OBRAIN_IAM_STORE", i).env("OBRAIN_IAM_ACCOUNT", "obrain");
    } else {
        // ── « SANS GARDE » DOIT VOULOIR DIRE SANS GARDE ────────────────────
        // Le cas `iam: None` veut un serveur NU, pour éprouver qu'il refuse
        // d'écrire une mémoire de self sans savoir quel self nourrir. Mais
        // `Command` HÉRITE de l'environnement : dans une session qui exporte
        // `OBRAIN_IAM=1` et `OBRAIN_IAM_STORE=<iam.obrain>`, le serveur censé
        // être nu était ARMÉ, et l'assertion échouait en 401 au lieu de 200 —
        // un test qui mesurait l'état de la machine au lieu du code. Mesuré
        // dans les deux sens : avec l'environnement ambiant il échoue, avec
        // `env -u OBRAIN_IAM -u OBRAIN_IAM_STORE` il passe. On retire donc
        // explicitement ce qu'on ne veut pas, au lieu de compter sur un
        // environnement propre.
        c.env_remove("OBRAIN_IAM")
            .env_remove("OBRAIN_IAM_STORE")
            .env_remove("OBRAIN_IAM_ACCOUNT");
    }
    let enfant = c.spawn().expect("spawn viz serve");
    let serveur = Serveur(enfant);
    let fin = Instant::now() + Duration::from_secs(90);
    while Instant::now() < fin {
        if TcpStream::connect(&addr).is_ok() {
            return (serveur, addr);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "serveur jamais prêt sur {addr}\n{}",
        std::fs::read_to_string(log).unwrap_or_default()
    );
}

/// Le serveur est-il ARMÉ ? C'est la condition de tout ce qui suit : sans la
/// garde, les deux comptes seraient le même occupant et le test ne prouverait
/// rien de l'isolation.
fn exiger_garde_armee(log: &Path) {
    let texte = std::fs::read_to_string(log).unwrap_or_default();
    assert!(texte.contains("IAM ARMÉ"), "le serveur de test devait être armé :\n{texte}");
    eprintln!(
        "── GARDE DU SERVEUR DE TEST ──\n{}",
        texte.lines().filter(|l| l.contains("viz serve :")).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn une_machine_attachee_nourrit_la_base_de_son_compte() {
    let td = tempfile::tempdir().unwrap();
    let cache = td.path().join("cache");
    std::fs::create_dir_all(cache.join("memoire")).unwrap();
    let iam = td.path().join("iam.obrain");
    // Le store SERVI par le serveur : ni la base d'Alice ni celle de Bob. C'est
    // ce qui permet à la passe d'application de prendre l'exclusif sur la base
    // d'Alice sans arrêter le serveur (invariant mono-écrivain).
    let store_service = td.path().join("service.obrain");
    let base_a = td.path().join("self-alice.obrain");
    let base_b = td.path().join("self-bob.obrain");

    for b in [&iam, &store_service, &base_a, &base_b] {
        let o = obrain().arg("init").arg(b).output().expect("init");
        assert!(o.status.success(), "init {} : {}", b.display(), String::from_utf8_lossy(&o.stderr));
    }

    // ── 1. DEUX COMPTES RÉELS, DEUX JETONS ─────────────────────────────────
    // Sans IAM armé, il n'y a qu'un occupant (« local ») : on ne pourrait PAS
    // prouver qu'un compte n'écrit pas chez l'autre. C'est donc la garde armée,
    // comme le service déployé.
    let (mdp_a, mdp_b) = ("alice-mdp-de-test", "bob-mdp-de-test");
    let o = obrain()
        .arg("iam").arg("amorcer").arg(&iam).arg("service")
        .arg("--utilisateur").arg("alice").arg("--mot-de-passe").arg(mdp_a)
        .output().expect("iam amorcer");
    assert!(o.status.success(), "amorçage : {}", String::from_utf8_lossy(&o.stderr));
    let o = obrain()
        .arg("iam").arg("ajouter").arg(&iam).arg("bob")
        .arg("--espace").arg("service").arg("--mot-de-passe").arg(mdp_b)
        .output().expect("iam ajouter");
    assert!(o.status.success(), "ajout de bob : {}", String::from_utf8_lossy(&o.stderr));

    // ── 2. LA BASE DE BOB EXISTE DÉJÀ, ET PORTE DES NŒUDS ──────────────────
    // Une base VIDE ne prouverait rien : « intacte » et « pas encore écrite »
    // seraient indiscernables. On y met donc des nœuds, et on mesure.
    for qn in ["repere-bob-1", "repere-bob-2", "repere-bob-3"] {
        let q = format!("CREATE (:Repere {{qn: '{qn}'}})");
        let o = obrain().arg("query").arg(&base_b).arg(&q).output().expect("create repere");
        assert!(o.status.success(), "peuplement de B : {}", String::from_utf8_lossy(&o.stderr));
    }
    let noeuds_b_avant = compte(&base_b, &cache, "MATCH (n) RETURN n");
    assert_eq!(noeuds_b_avant, 3, "la base de Bob part de trois nœuds");
    // L'empreinte est prise APRÈS la lecture ci-dessus : si une lecture laissait
    // une trace, elle serait déjà dans la référence.
    let empreinte_b_avant = empreinte(&base_b);
    eprintln!("── BASE DU COMPTE B (avant) : nœuds = {noeuds_b_avant} ; empreinte = {empreinte_b_avant:?}");

    // ── 3. LE SERVEUR RÉEL, ARMÉ, SUR UN PORT LIBRE ────────────────────────
    let journal_serveur = td.path().join("serveur.log");
    let (_serveur, addr) = lancer_serveur(&store_service, &cache, Some(&iam), &journal_serveur);
    exiger_garde_armee(&journal_serveur);

    // ── 4. SE CONNECTER, ET ATTACHER CHAQUE MACHINE ────────────────────────
    let connexion = |nom: &str, mdp: &str| -> String {
        let corps = serde_json::json!({ "username": nom, "password": mdp }).to_string();
        let (st, c) = http(&addr, "POST", "/api/auth/login", None, Some(&corps));
        assert_eq!(st, 200, "connexion de {nom} : {c}");
        json(&c)["jeton"].as_str().unwrap_or_else(|| panic!("jeton absent : {c}")).to_string()
    };
    let jeton_a = connexion("alice", mdp_a);
    let jeton_b = connexion("bob", mdp_b);

    let attacher = |jeton: &str, machine: &str| -> String {
        let corps = serde_json::json!({ "machine": machine, "capacites": ["read", "write"] }).to_string();
        let (st, c) = http(&addr, "POST", "/api/remote/attacher", Some(jeton), Some(&corps));
        assert_eq!(st, 200, "attachement de {machine} : {c}");
        json(&c)["client_id"].as_str().unwrap_or_else(|| panic!("client_id absent : {c}")).to_string()
    };
    let client_a = attacher(&jeton_a, "portable-alice");
    let client_b = attacher(&jeton_b, "tour-bob");

    let journal_a = journal_de(&cache, "self:alice");
    let journal_b = journal_de(&cache, "self:bob");
    assert!(!journal_a.exists(), "aucune mémoire d'Alice avant qu'elle publie");
    assert!(!journal_b.exists(), "aucune mémoire de Bob avant qu'il publie");

    // ── 5. LE COMPTE A PUBLIE — PAR SA MACHINE, ET PAR ELLE SEULE ──────────
    let envoi = serde_json::json!({
        "client_id": client_a,
        // Ce self est un PIÈGE : s'il était obéi, la note partirait chez Bob.
        "self": "self:bob",
        "intentions": [
            { "kind": "note", "texte": "Le disque de la machine d'Alice est à 91 %", "sujet": "disque" },
            { "kind": "objectif", "nom": "Surveiller le disque", "texte": "Alerter au-delà de 90 %" }
        ]
    })
    .to_string();
    let (st, c) = http(&addr, "POST", "/api/remote/memoire", Some(&jeton_a), Some(&envoi));
    assert_eq!(st, 200, "publication d'Alice : {c}");
    let r = json(&c);
    assert_eq!(
        r["self"].as_str(),
        Some("self:alice"),
        "le self EST CELUI DU COMPTE DU JETON, jamais celui du corps : {c}"
    );
    assert_eq!(
        r["self_du_corps_ignore"].as_str(),
        Some("self:bob"),
        "le self du corps est signalé IGNORÉ — le taire laisserait croire qu'il a servi : {c}"
    );
    assert_eq!(r["dans_la_base"], serde_json::json!(false), "le serveur ne peut pas écrire dans une base qu'il tient en lecture");
    eprintln!("── RÉPONSE DE POST /api/remote/memoire ──\n{}", serde_json::to_string_pretty(&r).unwrap());
    let journal_a = PathBuf::from(r["journal"].as_str().expect("journal rendu par la route"));
    let contenu = std::fs::read_to_string(&journal_a).expect("journal d'Alice");
    assert_eq!(lignes(&journal_a), 2, "deux intentions journalisées :\n{contenu}");
    eprintln!("── JOURNAL DE self:alice ({}) ──\n{contenu}", journal_a.display());
    assert!(contenu.contains("\"self\":\"self:alice\""), "chaque ligne porte le self :\n{contenu}");
    assert!(contenu.contains("Le disque de la machine d'Alice est à 91 %"), "le texte est intact :\n{contenu}");
    assert!(
        contenu.contains("remote:portable-alice:"),
        "la provenance nomme la machine qui a publié (une mémoire sans provenance n'est pas vérifiable) :\n{contenu}"
    );
    assert!(
        !contenu.contains("self:bob"),
        "rien de la publication d'Alice ne nomme le self de Bob :\n{contenu}"
    );
    assert!(!journal_b.exists(), "la publication d'Alice n'a PAS créé de mémoire chez Bob");

    // ── LE MÊME SELF QUE LA VUE MÉMOIRE DU COMPTE ──────────────────────────
    // La route d'ÉCRITURE et celle que l'écran lit (`/api/engine/memoire`)
    // résolvent le self SÉPARÉMENT, par deux chemins de code. Si elles
    // divergeaient, la note du nœud serait journalisée dans un self que personne
    // ne regarde — une écriture sans lecteur, c'est-à-dire perdue. On vérifie
    // donc qu'elles nomment le même self, et que la vue du compte voit les deux
    // intentions EN ATTENTE.
    let (st, c) = http(&addr, "GET", "/api/engine/memoire", Some(&jeton_a), None);
    assert_eq!(st, 200, "vue mémoire du compte : {c}");
    let vue = json(&c);
    assert_eq!(
        vue["self"].as_str(),
        Some("self:alice"),
        "l'écriture et la lecture doivent nommer LE MÊME self : {c}"
    );
    assert_eq!(vue["en_attente"].as_i64(), Some(2), "la vue du compte voit les deux intentions : {c}");

    // ── 6. CE QUI EST REFUSÉ NE S'ÉCRIT PAS ────────────────────────────────
    // « ANNONCER EST DECIDER » : un refus qui laisserait une ligne derrière lui
    // serait une promesse non tenue, et le journal ne se viderait jamais.
    let refus = |corps: serde_json::Value, jeton: Option<&str>, code: u16, extrait: &str| {
        let (st, c) = http(&addr, "POST", "/api/remote/memoire", jeton, Some(&corps.to_string()));
        assert_eq!(st, code, "attendu {code} pour {corps} — reçu : {c}");
        assert!(c.contains(extrait), "le refus doit dire « {extrait} » : {c}");
    };
    // a) le client d'un AUTRE compte, avec un jeton pourtant valide
    refus(
        serde_json::json!({ "client_id": client_b, "intentions": [{ "kind": "note", "texte": "x" }] }),
        Some(&jeton_a),
        403,
        "pas attaché à votre compte",
    );
    // b) aucune machine nommée
    refus(
        serde_json::json!({ "intentions": [{ "kind": "note", "texte": "x" }] }),
        Some(&jeton_a),
        403,
        "client_id",
    );
    // c) une intention que la passe d'application ne saurait pas écrire
    refus(
        serde_json::json!({ "client_id": client_a, "intentions": [{ "kind": "note" }] }),
        Some(&jeton_a),
        400,
        "texte",
    );
    // d) une forme inventée
    refus(
        serde_json::json!({ "client_id": client_a, "intentions": [{ "kind": "humeur", "texte": "x" }] }),
        Some(&jeton_a),
        400,
        "inconnu",
    );
    // e) un envoi vide n'est pas un succès
    refus(
        serde_json::json!({ "client_id": client_a, "intentions": [] }),
        Some(&jeton_a),
        400,
        "vide",
    );
    // f) plus d'intentions que le plafond
    let trop: Vec<serde_json::Value> = (0..40)
        .map(|i| serde_json::json!({ "kind": "note", "texte": format!("note {i}") }))
        .collect();
    refus(
        serde_json::json!({ "client_id": client_a, "intentions": trop }),
        Some(&jeton_a),
        400,
        "plafond",
    );
    // g) sans identité du tout
    refus(
        serde_json::json!({ "client_id": client_a, "intentions": [{ "kind": "note", "texte": "x" }] }),
        None,
        401,
        "identité",
    );
    assert_eq!(lignes(&journal_a), 2, "AUCUN refus n'a écrit une ligne :\n{}", std::fs::read_to_string(&journal_a).unwrap());

    // ── 7. LE COMPTE B PUBLIE — CHEZ LUI, PAS CHEZ ALICE ──────────────────
    let (st, c) = http(
        &addr,
        "POST",
        "/api/remote/memoire",
        Some(&jeton_b),
        Some(&serde_json::json!({
            "client_id": client_b,
            "intentions": [{ "kind": "note", "texte": "Repère propre à Bob", "sujet": "bob" }]
        }).to_string()),
    );
    assert_eq!(st, 200, "publication de Bob : {c}");
    let r_b = json(&c);
    assert_eq!(r_b["self"].as_str(), Some("self:bob"));
    assert_eq!(PathBuf::from(r_b["journal"].as_str().unwrap()), journal_b, "le journal de Bob est le sien");
    assert_eq!(lignes(&journal_a), 2, "la publication de Bob n'ajoute rien chez Alice");

    // ── 8. LA PASSE D'APPLICATION FAIT ATTERRIR, DANS LA BASE DU COMPTE A ──
    // Le serveur ne tient PAS la base d'Alice : la passe peut donc prendre le
    // verrou exclusif sans arrêter de service (c'est tout l'intérêt du journal).
    let o = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", &cache)
        .arg("memoire").arg("--store").arg(&base_a)
        .arg("--self").arg("self:alice").arg("--appliquer")
        .output().expect("appliquer");
    assert!(o.status.success(), "application : {}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(compte(&base_a, &cache, "MATCH (s:Self {qn: 'self:alice'}) RETURN s"), 1, "le Self du compte A");
    assert_eq!(compte(&base_a, &cache, "MATCH (:Self)-[:HAS_NOTE]->(n:Note) RETURN n"), 1, "la note publiée est dans la base");
    assert_eq!(compte(&base_a, &cache, "MATCH (:Self)-[:HAS_GOAL]->(g:Goal) RETURN g"), 1, "l'objectif publié est dans la base");
    let (st, c) = http(&addr, "GET", "/api/engine/memoire", Some(&jeton_a), None);
    assert_eq!(st, 200, "vue mémoire après application : {c}");
    assert_eq!(
        json(&c)["en_attente"].as_i64(),
        Some(0),
        "la passe a CONSOMMÉ le journal : plus rien en attente, tout est dans la base : {c}"
    );
    let texte = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", &cache)
        .arg("query").arg(&base_a)
        .arg("MATCH (n:Note) RETURN n.text").arg("--format").arg("json")
        .output().expect("query note");
    let txt = String::from_utf8_lossy(&texte.stdout);
    assert!(
        txt.contains("Le disque de la machine d'Alice est à 91 %"),
        "le texte appliqué est celui qui a été publié : {txt}"
    );

    // ── 9. LA BASE DU COMPTE B EST INTACTE — LES DEUX MESURES ─────────────
    // (1) l'empreinte d'abord, et AVANT toute nouvelle lecture — défaut mesuré
    //     en écrivant ce test : `obrain query` ouvre le store en lecture-ÉCRITURE
    //     (rejeu du WAL, puis `flush`), donc une simple LECTURE fait grossir
    //     `substrate.wal` et change des dates. Une sonde posée avant la mesure
    //     mesurerait sa propre trace : c'est la même famille de défaut que
    //     « un contrôle indirect valide ce qu'il ne mesure pas ».
    let empreinte_b_apres = empreinte(&base_b);
    eprintln!("── BASE DU COMPTE B (après) : empreinte = {empreinte_b_apres:?}");
    assert_eq!(
        empreinte_b_apres, empreinte_b_avant,
        "les octets et les dates de la base de Bob n'ont pas bougé\navant : {empreinte_b_avant:?}\naprès : {empreinte_b_apres:?}"
    );
    // (2) puis le contenu, sur la même base (la lecture ci-dessus écrit dans le
    //     WAL, mais l'empreinte était déjà prise : elle ne mesure que la chaîne
    //     de publication, pas la sonde).
    let noeuds_b_apres = compte(&base_b, &cache, "MATCH (n) RETURN n");
    eprintln!("── BASE DU COMPTE B (après) : nœuds = {noeuds_b_apres}");
    assert_eq!(noeuds_b_apres, noeuds_b_avant, "le nombre de nœuds de Bob n'a pas bougé");
    assert_eq!(compte(&base_b, &cache, "MATCH (n:Note) RETURN n"), 0, "aucune note d'Alice n'est allée chez Bob");
    assert_eq!(compte(&base_b, &cache, "MATCH (g:Goal) RETURN g"), 0, "ni aucun objectif");
}

/// ── SANS IDENTITÉ, LE MOTEUR REFUSE D'ÉCRIRE — IL NE DEVINE PAS ────────────
///
/// Quand l'IAM n'est pas armé, il n'y a qu'un occupant (« local ») et aucune
/// identité : le serveur ne peut pas dire QUEL self nourrir. Une lecture peut se
/// permettre de résoudre le self de l'instance ; une ÉCRITURE, non — un self
/// deviné, c'est une mémoire rangée chez quelqu'un d'autre. On vérifie donc les
/// deux choses ensemble : le refus est MOTIVÉ, et RIEN n'a été journalisé.
#[test]
fn sans_identite_le_moteur_refuse_de_deviner_le_self() {
    let td = tempfile::tempdir().unwrap();
    let cache = td.path().join("cache");
    std::fs::create_dir_all(cache.join("memoire")).unwrap();
    let store = td.path().join("service.obrain");
    let o = obrain().arg("init").arg(&store).output().expect("init");
    assert!(o.status.success(), "init : {}", String::from_utf8_lossy(&o.stderr));

    let journal_serveur = td.path().join("serveur.log");
    // `iam: None` — le serveur tourne sans garde, comme un poste de travail.
    let (_serveur, addr) = lancer_serveur(&store, &cache, None, &journal_serveur);

    // Ici, aucune identité n'est exigée : l'attachement passe sans jeton.
    let (st, c) = http(
        &addr,
        "POST",
        "/api/remote/attacher",
        None,
        Some(&serde_json::json!({ "machine": "poste-sans-iam" }).to_string()),
    );
    assert_eq!(st, 200, "attachement sans IAM : {c}");
    let client = json(&c)["client_id"].as_str().expect("client_id").to_string();

    let (st, c) = http(
        &addr,
        "POST",
        "/api/remote/memoire",
        None,
        Some(&serde_json::json!({
            "client_id": client,
            "intentions": [{ "kind": "note", "texte": "chez qui cette note atterrit-elle ?" }]
        }).to_string()),
    );
    assert_eq!(st, 403, "un moteur sans identité ne peut pas placer une écriture : {c}");
    assert!(
        c.contains("IAM non armé") && c.contains("deviner"),
        "le refus doit dire POURQUOI et quoi faire : {c}"
    );

    // ── LA PARTIE QUI COMPTE : RIEN N'A ÉTÉ ÉCRIT ─────────────────────────
    let journaux: Vec<String> = std::fs::read_dir(cache.join("memoire"))
        .expect("répertoire des journaux")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    assert!(
        journaux.is_empty(),
        "un refus doit laisser le journal VIDE (sinon la mémoire part dans un self deviné) : {journaux:?}"
    );
}
