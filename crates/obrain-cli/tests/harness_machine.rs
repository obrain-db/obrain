//! Auto-declaration de la machine (2026-09-15) — le contrat que le releve
//! `docs/MACHINES-CONNUES-2026-09-15.md` reclame.
//!
//! Ce que ce fichier protege n'est pas une route, c'est une CAPACITE : celle de
//! distinguer deux harnesses de la flotte par ce qu'ils disent d'eux-memes.
//!
//! Le releve du 2026-09-15 est le point de depart : n2 (192.168.1.102) et n3
//! (192.168.1.103) rendaient la MEME reponse `/api/harness/health`, et le nom de
//! l'hote n'y apparaissait pas une seule fois. Deux machines indiscernables par
//! leur propre contrat, donc non adressables : « on ne pilote pas une machine
//! qui n'a pas de nom ».
//!
//! Les tests lancent le BINAIRE REEL sur des stores temporaires et des ports
//! ephemeres, comme `tests/harness_serve.rs` : ce qui est verifie est ce que le
//! processus ecrit sur la socket, pas une fonction interne.
//!
//! Note de methode : les deux harnesses du premier test tournent sur le MEME
//! hote (c'est une machine de test, pas une flotte). Le test l'assume et
//! l'ecrit : le nom d'hote NE LES DISTINGUE PAS — ce sont le store, la racine et
//! l'instance qui le font, et c'est exactement pourquoi l'identite ne se reduit
//! pas au nom d'hote.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn port_libre() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("port ephemere")
        .local_addr()
        .unwrap()
        .port()
}

/// Requete HTTP brute : on ne teste pas un client, on teste ce que le serveur
/// repond. `entetes` permet de faire varier l'identite presentee (IAM).
fn requete(
    addr: &str,
    methode: &str,
    chemin: &str,
    corps: Option<&str>,
    entetes: &[(&str, &str)],
) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("connexion");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let corps = corps.unwrap_or("");
    let mut sup = String::new();
    for (k, v) in entetes {
        sup.push_str(&format!("{k}: {v}\r\n"));
    }
    let req = format!(
        "{methode} {chemin} HTTP/1.1\r\nHost: test\r\n{sup}Content-Length: {}\r\nConnection: close\r\n\r\n{corps}",
        corps.len()
    );
    s.write_all(req.as_bytes()).expect("envoi");
    let mut rep = String::new();
    s.read_to_string(&mut rep).expect("lecture");
    let statut: u16 = rep
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = rep.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (statut, body)
}

fn get(addr: &str, chemin: &str) -> (u16, serde_json::Value) {
    let (st, corps) = requete(addr, "GET", chemin, None, &[]);
    let v: serde_json::Value =
        serde_json::from_str(&corps).unwrap_or_else(|e| panic!("{chemin} non-JSON ({st}): {corps} / {e}"));
    (st, v)
}

fn init_store(store: &Path) {
    let o = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("init")
        .arg(store)
        .output()
        .expect("init");
    assert!(
        o.status.success(),
        "init {}: {}",
        store.display(),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn semer_session(store: &Path, qn: &str) {
    let q = format!("CREATE (:Session {{qualified_name: '{qn}', cwd: '/tmp/{qn}', createdAt: 1}})");
    let o = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("query")
        .arg(store)
        .arg(&q)
        .output()
        .expect("query");
    assert!(
        o.status.success(),
        "semis {qn}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Un harness de test : binaire reel, store temporaire, port ephemere, racine
/// declaree explicite. Tue au `Drop` — aucun daemon ne survit au test.
struct Harness {
    enfant: Child,
    addr: String,
    store: PathBuf,
    _td: tempfile::TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.enfant.kill();
        let _ = self.enfant.wait();
    }
}

/// Lance le binaire, attend qu'il accepte une connexion, et rend la main.
///
/// `racine` est passee par `OBRAIN_HARNESS_ROOT` : c'est la SEULE facon de
/// declarer la racine de donnees d'un harness — rien ne la devine a partir du
/// chemin du store.
fn lancer(store: PathBuf, td: tempfile::TempDir, racine: &Path) -> Harness {
    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    let enfant = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("harness-serve")
        .arg(&store)
        .arg("--addr")
        .arg(&addr)
        .env("OBRAIN_HARNESS_ROOT", racine)
        // ── LE CACHE EST ISOLÉ, ET C'EST UNE CORRECTION, PAS UN CONFORT ─────
        // Depuis que le harness journalise sa propre déclaration de machine au
        // démarrage, un test qui ne pose pas cette variable écrit dans le cache
        // VIVANT de la machine (/tmp/obrain-viz-cache/machines). Constaté : six
        // fichiers `kind:machine` y portaient des stores de test disparus
        // (…/kb-harness, session-alpha.obrain…), et une passe
        // `machines --appliquer` avec le cache par défaut aurait écrit des nœuds
        // `machine:` pour des machines de test. Un test ne doit pas laisser de
        // trace dans l'état partagé.
        .env("OBRAIN_VIZ_CACHE_DIR", td.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn harness-serve");
    let fin = Instant::now() + Duration::from_secs(30);
    while Instant::now() < fin {
        if TcpStream::connect(&addr).is_ok() {
            return Harness {
                enfant,
                addr,
                store,
                _td: td,
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("harness jamais pret sur {addr} (store {})", store.display());
}

/// Un harness pret : store initialise et peuple d'une session nommee.
fn harness_avec_session(nom_session: &str) -> (Harness, PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let store = td.path().join(format!("{nom_session}.obrain"));
    init_store(&store);
    semer_session(&store, nom_session);
    let racine = td.path().join(format!("racine-{nom_session}"));
    std::fs::create_dir_all(&racine).expect("racine");
    (lancer(store, td, &racine), racine)
}

/// L'identite, moins ce qui bouge a chaque reponse (`presence_ms`, le pid, et
/// l'adresse d'ecoute qui depend du port ephemere). C'est ce reste-la qui doit
/// distinguer deux machines.
fn identite_stable(v: &serde_json::Value) -> serde_json::Value {
    let mut c = v.clone();
    if let Some(o) = c.as_object_mut() {
        o.remove("presence_ms");
        o.remove("pid");
        o.remove("ecoute");
        o.remove("demarre_le_ms");
    }
    c
}

// ── (a) DEUX HARNESSES, DEUX STORES, DEUX RACINES : DEUX IDENTITES ──────────
//
// C'est LE test que le releve reclame : « deux harnesses lances sur deux stores
// et deux racines differentes rendent deux identites distinguables ». Il est
// ecrit de facon a ne PAS passer par accident : les deux tournent sur le meme
// hote, donc si l'identite se reduisait au nom d'hote, ce test echouerait.
#[test]
fn deux_harnesses_sur_deux_stores_se_disent_differents() {
    let (a, racine_a) = harness_avec_session("session-alpha");
    let (b, racine_b) = harness_avec_session("session-beta");

    let (sa, ia) = get(&a.addr, "/api/harness/machine");
    let (sb, ib) = get(&b.addr, "/api/harness/machine");
    assert_eq!(sa, 200, "machine A: {ia}");
    assert_eq!(sb, 200, "machine B: {ib}");

    // 1. Le nom d'hote est le MEME — et c'est assume : ce test ne prouve pas
    //    que deux hotes se distinguent, il prouve que deux instances se
    //    distinguent, ce qui est le cas reel qui a casse (n2 sans store).
    assert_eq!(
        ia["machine"], ib["machine"],
        "meme hote de test : {ia} / {ib}"
    );
    assert_ne!(
        ia["machine"], "inconnu",
        "le nom d'hote doit etre LU, pas laisse a inconnu : {ia}"
    );
    assert_eq!(
        ia["machine_qn"],
        format!("machine:{}", ia["machine"].as_str().unwrap()),
        "le noeud de premier ordre porte le nom de la machine : {ia}"
    );
    assert_eq!(ia["nom_source"], "/proc/sys/kernel/hostname");

    // 2. Ce qui les distingue VRAIMENT.
    assert_ne!(ia["store"], ib["store"], "deux stores : {ia} / {ib}");
    assert_ne!(
        ia["store_racine"], ib["store_racine"],
        "deux dossiers de store : {ia} / {ib}"
    );
    assert_eq!(ia["store_racine"], a.store.parent().unwrap().display().to_string());
    assert_eq!(ia["racine_declaree"], racine_a.display().to_string());
    assert_eq!(ib["racine_declaree"], racine_b.display().to_string());
    assert_ne!(ia["instance"], ib["instance"], "instance : {ia} / {ib}");
    assert_ne!(ia["ecoute"], ib["ecoute"], "deux ports : {ia} / {ib}");
    assert!(
        ia["instance"].as_str().unwrap().starts_with("machine:"),
        "l'instance est ancree sur le nom de machine : {ia}"
    );

    // 3. L'identite ENTIERE differe — pas seulement un champ qu'on aurait
    //    ajoute pour faire passer le test.
    assert_ne!(
        identite_stable(&ia),
        identite_stable(&ib),
        "deux harnesses sur deux stores doivent rendre deux identites : {ia} / {ib}"
    );

    // 4. Ce que l'identite dit du binaire est l'estampille REELLE de
    //    compilation (`build.rs`), pas une constante ecrite a la main : la
    //    constante `version` `0.0.1`, elle, est identique sur toutes les
    //    machines — c'est precisement ce que le releve reprochait.
    assert_eq!(ia["build"]["commit"], env!("OBRAIN_GIT_COMMIT"));
    assert_eq!(ia["build"]["describe"], env!("OBRAIN_GIT_DESCRIBE"));
    assert_eq!(ia["build"]["arbre"], env!("OBRAIN_GIT_ARBRE"));
    assert_eq!(ia["build"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ia["os"], std::env::consts::OS);
    assert_eq!(ia["arch"], std::env::consts::ARCH);
    assert_eq!(ia["mode"], "ro", "le mode offert est ro : {ia}");
    assert_eq!(ia["status"], "ok");
    assert!(ia["presence_ms"].as_i64().unwrap_or(0) > 0, "{ia}");
}

// ── (c) L'IDENTITE ANNONCEE EST LE STORE REELLEMENT SERVI ──────────────────
//
// Annoncer un store n'est pas le servir. Un harness qui repondrait « je sers A »
// en servant B enverrait les actions au mauvais endroit — c'est la faute que
// « annoncer est decider » interdit. On le verifie donc par le COMPORTEMENT :
// le contenu du store annonce doit etre celui qu'on interroge, et l'autre
// harness ne doit PAS voir la session du premier.
#[test]
fn l_identite_annonce_le_store_reellement_servi() {
    let (a, _ra) = harness_avec_session("session-alpha");
    let (b, _rb) = harness_avec_session("session-beta");

    let (_, ia) = get(&a.addr, "/api/harness/machine");
    assert_eq!(
        ia["store"].as_str().unwrap(),
        a.store.display().to_string(),
        "le store annonce est celui qu'on a demande de servir"
    );

    let (st, va) = get(&a.addr, "/api/harness/sessions?limit=50");
    assert_eq!(st, 200, "{va}");
    let (_, vb) = get(&b.addr, "/api/harness/sessions?limit=50");
    let qns = |v: &serde_json::Value| -> Vec<String> {
        v["sessions"]
            .as_array()
            .expect("sessions[]")
            .iter()
            .map(|s| s["qn"].as_str().unwrap_or("").to_string())
            .collect()
    };
    assert!(
        qns(&va).contains(&"session-alpha".to_string()),
        "A sert bien son store : {va}"
    );
    assert!(
        !qns(&va).contains(&"session-beta".to_string()),
        "A ne sert PAS le store de B : {va}"
    );
    assert!(
        qns(&vb).contains(&"session-beta".to_string()),
        "B sert bien son store : {vb}"
    );
}

// ── (b) LA ROUTE REPOND SANS IAM COMME AVEC ────────────────────────────────
//
// `/api/harness/machine` decrit une machine, pas une personne : elle ne doit
// dependre d'aucun jeton. Ce test l'E PROUVE au lieu de l'affirmer — une
// identite presente (ou absente) ne change pas la reponse, et aucun 401
// n'apparait.
#[test]
fn la_route_machine_repond_sans_iam_comme_avec() {
    let (a, _r) = harness_avec_session("session-iam");

    let (sans, corps_sans) = requete(&a.addr, "GET", "/api/harness/machine", None, &[]);
    let (avec, corps_avec) = requete(
        &a.addr,
        "GET",
        "/api/harness/machine",
        None,
        &[
            ("Authorization", "Bearer jeton-bidon"),
            ("X-Obrain-Identite", "compte-inexistant"),
        ],
    );
    assert_eq!(sans, 200, "sans en-tete d'identite : {corps_sans}");
    assert_eq!(avec, 200, "avec une identite bidon : {corps_avec}");

    let vs: serde_json::Value = serde_json::from_str(&corps_sans).unwrap();
    let va: serde_json::Value = serde_json::from_str(&corps_avec).unwrap();
    assert_eq!(
        identite_stable(&vs),
        identite_stable(&va),
        "l'identite rendue ne depend pas de l'identite presentee"
    );

    // La sante suit la meme regle, et garde ses champs d'origine : des scripts
    // les lisent (`scripts/verifier-integration-tissage.sh` cherche
    // `"status":"ok"` et `mode` ; `scripts/flotte-auto-connaissance.sh` cherche
    // le nom de l'hote dans le corps).
    let (sh, vh) = get(&a.addr, "/api/harness/health");
    assert_eq!(sh, 200, "{vh}");
    assert_eq!(vh["status"], "ok");
    assert_eq!(vh["mode"], "ro");
    assert_eq!(vh["store"].as_str().unwrap(), a.store.display().to_string());
    assert_eq!(vh["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        vh["machine"].is_string(),
        "le champ `machine` reste une chaine (contrat existant) : {vh}"
    );
    assert_eq!(
        vh["identite"]["machine"], vh["machine"],
        "l'identite ajoutee nomme la meme machine : {vh}"
    );
    assert_eq!(vh["identite"]["build"]["commit"], env!("OBRAIN_GIT_COMMIT"));
}

// ── (4) LE VIDE N'A PLUS LA FORME D'UNE BONNE REPONSE ─────────────────────
//
// Releve qui a ouvert ce chantier : `/api/harness/context` sans session rendait
// `{"engrams":[],"goals":[],"session_id":"","weave":[],"summaries":[],
// "status":"ok"}` — un « ok » sur un vide integral. Un appelant ne pouvait pas
// distinguer « le graphe n'a rien a dire » de « on ne m'a pas dit pour qui
// regarder ».
#[test]
fn le_contexte_sans_session_ne_se_declare_pas_ok() {
    let (a, _r) = harness_avec_session("session-ctx");

    // 1. Demande LISIBLE qui ne nomme aucune session : refusee, motivee, 400.
    let (st, corps) = requete(&a.addr, "POST", "/api/harness/context", Some("{}"), &[]);
    assert_eq!(st, 400, "une demande lisible sans session est refusee : {corps}");
    let v: serde_json::Value = serde_json::from_str(&corps).expect("corps JSON");
    assert_ne!(v["status"], "ok", "plus de ok sur un vide : {v}");
    assert_eq!(v["status"], "aucune_session");
    assert_eq!(v["corps_lisible"], true);
    assert!(
        v["raison"].as_str().map(|r| !r.is_empty()).unwrap_or(false),
        "le refus doit etre MOTIVE : {v}"
    );
    assert!(v.get("tissage").is_some(), "le fil reste publie : {v}");

    // 2. Demande ILLISIBLE : statut honnete, mais le fil survit. Un consommateur
    //    qui envoie n'importe quoi doit quand meme apprendre si le tissage
    //    tourne — c'est dans les cas degrades qu'on en a besoin.
    let (st, corps) = requete(
        &a.addr,
        "POST",
        "/api/harness/context",
        Some("ceci n'est pas du JSON"),
        &[],
    );
    assert_eq!(st, 200, "{corps}");
    let v: serde_json::Value = serde_json::from_str(&corps).expect("corps JSON");
    assert_eq!(v["status"], "aucune_session");
    assert_eq!(v["corps_lisible"], false);
    assert!(v.get("tissage").is_some(), "{v}");

    // 3. Le chemin NORMAL n'a pas bouge : une session nommee rend ok, et ses
    //    sections sont presentes.
    let (st, corps) = requete(
        &a.addr,
        "POST",
        "/api/harness/context",
        Some(r#"{"session_id":"session-ctx","subject":"x"}"#),
        &[],
    );
    assert_eq!(st, 200, "session nommee : {corps}");
    let v: serde_json::Value = serde_json::from_str(&corps).expect("corps JSON");
    assert_eq!(v["status"], "ok", "{v}");
    for champ in ["goals", "summaries", "weave", "engrams", "tissage"] {
        assert!(v.get(champ).is_some(), "section '{champ}' absente : {corps}");
    }
}
