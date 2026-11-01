//! Persistance du registre des machines — prouvée de bout en bout, sur le
//! BINAIRE réel, avec un VRAI redémarrage.
//!
//! ── CE QUE CE TEST PROUVE, ET POURQUOI IL FALLAIT LE BINAIRE ────────────────
//! Le défaut d'origine n'était pas dans une fonction : il était dans la vie du
//! processus. `crates/obrain-cli/src/remote.rs` tenait ses clients dans un
//! `HashMap` de processus, donc « attacher une machine » ne laissait AUCUNE
//! trace — et un redémarrage du moteur effaçait la flotte en silence. Un test
//! qui appellerait les fonctions du module dans le même processus ne pourrait
//! pas distinguer « ça persiste » de « ça se souvient encore ».
//!
//! D'où la forme de ce test : le vrai `obrain viz serve`, sur un port
//! éphémère, avec un `OBRAIN_VIZ_CACHE_DIR` temporaire, armé par un vrai
//! magasin d'identités IAM (deux comptes, deux jetons). On y attache deux
//! machines, ON ARRÊTE LE SERVEUR, ON EN RELANCE UN AUTRE, et on interroge le
//! nouveau : c'est exactement le geste de l'incident.
//!
//! Trois précautions, apprises des tests voisins (`harness_serve.rs`) :
//!   * le serveur est tué par un GARDE (`Drop`) : un `assert!` qui échoue ne
//!     doit pas laisser un port occupé derrière lui ;
//!   * ses journaux partent dans un FICHIER, jamais dans un tube non lu (un
//!     `stderr` plein bloque le serveur, et l'échec ressemblerait à une panne
//!     de l'application au lieu d'un défaut du test) ;
//!   * il tourne sur un port libre pris dynamiquement — jamais 8432.
//!
//! Ce que le test mesure, dans l'ordre du contrat : (a) attacher ÉCRIT un
//! journal par compte ; (b) un registre neuf relu du même répertoire retrouve
//! la machine, avec le MÊME identifiant ; (c) le cloisonnement par compte
//! survit à cette relecture ; (d) une machine sans battement récent est
//! annoncée connue mais PAS vivante, et un ordre vers elle est refusé avec un
//! motif au lieu d'expirer en silence.

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

/// L'horloge du test — la même que celle du registre (`activation::now_ms`),
/// pour que « il y a deux heures » veuille dire la même chose des deux côtés.
fn maintenant_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Le premier objet JSON d'une sortie de commande.
///
/// Les commandes de ce dépôt écrivent des `WARN` sur la même sortie que leur
/// résultat : on cherche la première LIGNE qui ouvre un objet, pas le premier
/// caractère `{` (qui peut apparaître dans un message).
fn json_de(txt: &str) -> serde_json::Value {
    let mut offset = 0usize;
    for ligne in txt.split_inclusive('\n') {
        let reste = ligne.trim_start();
        if reste.starts_with('{') {
            let debut = offset + (ligne.len() - reste.len());
            return serde_json::from_str(&txt[debut..])
                .unwrap_or_else(|e| panic!("JSON illisible ({e}) :\n{txt}"));
        }
        offset += ligne.len();
    }
    panic!("aucun objet JSON dans la sortie :\n{txt}");
}

fn requete(
    addr: &str,
    methode: &str,
    chemin: &str,
    jeton: Option<&str>,
    corps: Option<&str>,
) -> (u16, String) {
    tenter(addr, methode, chemin, jeton, corps)
        .unwrap_or_else(|| panic!("serveur de test injoignable sur {addr}"))
}

/// La même chose, mais rend `None` si le serveur n'écoute pas ENCORE.
///
/// L'attente de démarrage en a besoin : `viz serve` ouvre ses presets avant de
/// lier son port, et un `connect` trop tôt est un `ECONNREFUSED` — pas un échec
/// du test, seulement un serveur pas encore là.
fn tenter(
    addr: &str,
    methode: &str,
    chemin: &str,
    jeton: Option<&str>,
    corps: Option<&str>,
) -> Option<(u16, String)> {
    let mut s = TcpStream::connect(addr).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut req = format!("{methode} {chemin} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n");
    if let Some(j) = jeton {
        req.push_str(&format!("Authorization: Bearer {j}\r\n"));
    }
    match corps {
        Some(c) => {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n\r\n{c}", c.len()));
        }
        None => req.push_str("\r\n"),
    }
    s.write_all(req.as_bytes()).expect("écriture de la requête");
    let mut buf = String::new();
    s.read_to_string(&mut buf).expect("lecture de la réponse");
    let statut: u16 = buf.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let corps = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some((statut, corps))
}

fn lire(chemin: &Path) -> String {
    std::fs::read_to_string(chemin).unwrap_or_else(|e| format!("(journal illisible : {e})"))
}

/// Un serveur de test, tué par le `Drop` quoi qu'il arrive.
struct Serveur {
    enfant: Child,
    addr: String,
    log: PathBuf,
}

impl Drop for Serveur {
    fn drop(&mut self) {
        let _ = self.enfant.kill();
        let _ = self.enfant.wait();
    }
}

impl Serveur {
    /// Lance `viz serve` sur un magasin temporaire, armé par l'IAM du test, et
    /// attend qu'il RÉPONDE (pas seulement qu'il écoute).
    fn demarrer(store: &Path, cache: &Path, iam: &Path, log: &Path, jeton: &str) -> Self {
        let port = port_libre();
        let addr = format!("127.0.0.1:{port}");
        let sortie = std::fs::File::create(log).expect("fichier de log");
        let enfant = obrain()
            .arg("viz")
            .arg("serve")
            .arg("--store")
            .arg(store)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            // Le serveur ne doit RIEN enrichir : ce test mesure un registre, et
            // un entretien qui démarre en fond rendrait la mesure instable.
            .arg("--enrich-ricci")
            .arg("0")
            .env("OBRAIN_IAM", "1")
            .env("OBRAIN_IAM_STORE", iam)
            .env("OBRAIN_IAM_ACCOUNT", "obrain")
            .env("OBRAIN_VIZ_CACHE_DIR", cache)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(sortie))
            .spawn()
            .expect("spawn viz serve");

        let mut serveur = Serveur { enfant, addr, log: log.to_path_buf() };
        let fin = Instant::now() + Duration::from_secs(90);
        while Instant::now() < fin {
            if let Ok(Some(statut)) = serveur.enfant.try_wait() {
                panic!(
                    "viz serve est mort ({statut}) — journal :\n{}",
                    lire(&serveur.log)
                );
            }
            // Un port qui écoute ne veut pas dire un serveur qui a fini de
            // démarrer : on exige une VRAIE réponse authentifiée.
            let reponse = tenter(&serveur.addr, "GET", "/api/remote/clients", Some(jeton), None);
            if matches!(reponse, Some((200, _))) {
                return serveur;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("viz serve jamais prêt sur {} — journal :\n{}", serveur.addr, lire(&serveur.log));
    }
}

/// Les journaux du registre : un fichier par compte, sous le cache.
fn journaux(cache: &Path) -> Vec<PathBuf> {
    let dir = cache.join("machines");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|l| {
            l.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Les machines annoncées pour un compte, telles que l'interface les verrait.
fn machines(addr: &str, jeton: &str) -> Vec<serde_json::Value> {
    let (code, corps) = requete(addr, "GET", "/api/remote/clients", Some(jeton), None);
    assert_eq!(code, 200, "clients : {corps}");
    json_de(&corps)["clients"].as_array().cloned().unwrap_or_default()
}

fn noms(v: &[serde_json::Value]) -> Vec<String> {
    v.iter()
        .map(|c| c["machine"].as_str().unwrap_or("?").to_string())
        .collect()
}

/// IAM de test : un magasin dédié, deux comptes, deux jetons.
///
/// C'est ce qui rend le cloisonnement MESURABLE : sans IAM armé, `viz serve`
/// nomme tout le monde « local » (et le dit) — un seul compte, donc aucune
/// frontière à prouver.
fn amorcer_iam(td: &Path) -> (PathBuf, String, String) {
    let iam = td.join("iam.obrain");
    let init = obrain().arg("init").arg(&iam).output().expect("init iam");
    assert!(init.status.success(), "init iam : {}", String::from_utf8_lossy(&init.stderr));

    let admin = obrain()
        .arg("iam")
        .arg("amorcer")
        .arg(&iam)
        .arg("espace-test")
        .arg("--utilisateur")
        .arg("admin")
        .arg("--mot-de-passe")
        .arg("Motdepasse!1")
        .arg("--json")
        .output()
        .expect("iam amorcer");
    assert!(
        admin.status.success(),
        "iam amorcer : {}",
        String::from_utf8_lossy(&admin.stderr)
    );
    let _ = json_de(&String::from_utf8_lossy(&admin.stdout))["jeton"].as_str().expect("jeton admin");

    let mut jetons = Vec::new();
    for (nom, mdp) in [("alice", "Motdepasse!2"), ("bob", "Motdepasse!3")] {
        let a = obrain()
            .arg("iam")
            .arg("ajouter")
            .arg(&iam)
            .arg(nom)
            .arg("--espace")
            .arg("espace-test")
            .arg("--mot-de-passe")
            .arg(mdp)
            .arg("--json")
            .output()
            .expect("iam ajouter");
        assert!(a.status.success(), "iam ajouter {nom} : {}", String::from_utf8_lossy(&a.stderr));
        let j = obrain()
            .arg("iam")
            .arg("jeton")
            .arg(&iam)
            .arg(nom)
            .arg("--json")
            .output()
            .expect("iam jeton");
        assert!(j.status.success(), "iam jeton {nom} : {}", String::from_utf8_lossy(&j.stderr));
        let v = json_de(&String::from_utf8_lossy(&j.stdout));
        jetons.push(v["jeton"].as_str().expect("jeton").to_string());
    }
    (iam, jetons[0].clone(), jetons[1].clone())
}

#[test]
fn le_registre_des_machines_survit_a_un_redemarrage_et_ne_trahit_pas_les_comptes() {
    let td = tempfile::tempdir().unwrap();
    let cache = td.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let store = td.path().join("kb.obrain");
    let init = obrain().arg("init").arg(&store).output().expect("init kb");
    assert!(init.status.success(), "init kb : {}", String::from_utf8_lossy(&init.stderr));

    let (iam, jeton_alice, jeton_bob) = amorcer_iam(td.path());

    // ── 1. PREMIER MOTEUR : les deux machines s'annoncent ───────────────────
    let s1 = Serveur::demarrer(&store, &cache, &iam, &td.path().join("moteur-1.log"), &jeton_alice);

    // Sans identité, le canal ne parle à personne : même une liste vide serait
    // une réponse, et ce serait une fuite déguisée en service.
    let (code, _) = requete(&s1.addr, "GET", "/api/remote/clients", None, None);
    assert_eq!(code, 401, "identité exigée");

    let (code, corps) = requete(
        &s1.addr,
        "POST",
        "/api/remote/attacher",
        Some(&jeton_alice),
        Some(r#"{"machine":"portable-alice","capacites":["read","exec"]}"#),
    );
    assert_eq!(code, 200, "attacher alice : {corps}");
    let id_alice = json_de(&corps)["client_id"].as_str().expect("client_id").to_string();

    let (code, corps) = requete(
        &s1.addr,
        "POST",
        "/api/remote/attacher",
        Some(&jeton_bob),
        Some(r#"{"machine":"tour-bob","capacites":["read"]}"#),
    );
    assert_eq!(code, 200, "attacher bob : {corps}");
    let id_bob = json_de(&corps)["client_id"].as_str().expect("client_id").to_string();

    // (a) ATTACHER ÉCRIT QUELQUE PART — et pas n'importe où : un journal par
    // compte, qui nomme la machine ET son propriétaire.
    let js = journaux(&cache);
    assert_eq!(js.len(), 2, "un journal par compte : {js:?}");
    let textes: Vec<String> = js.iter().map(|p| lire(p)).collect();
    let chez_alice = textes.iter().find(|t| t.contains("portable-alice")).expect("journal alice");
    let chez_bob = textes.iter().find(|t| t.contains("tour-bob")).expect("journal bob");
    assert!(chez_alice.contains(id_alice.as_str()), "l'identifiant est écrit : {chez_alice}");
    assert!(chez_alice.contains("\"compte\":\"alice\""), "et le compte : {chez_alice}");
    assert!(!chez_alice.contains("tour-bob"), "le journal d'alice ne nomme pas bob");
    assert!(!chez_bob.contains("portable-alice"), "et réciproquement");

    // Vivante, la machine est annoncée vivante (le battement vient d'avoir lieu).
    let chez_alice_vu = machines(&s1.addr, &jeton_alice);
    assert_eq!(noms(&chez_alice_vu), vec!["portable-alice"]);
    assert_eq!(chez_alice_vu[0]["vivant"], serde_json::json!(true), "elle vient de s'annoncer");
    assert_eq!(
        noms(&machines(&s1.addr, &jeton_bob)),
        vec!["tour-bob"],
        "avant redémarrage, chacun ne voit que la sienne"
    );

    // ── 2. REDÉMARRAGE : le premier moteur s'arrête (le `Drop` le tue) ──────
    drop(s1);
    let s2 = Serveur::demarrer(&store, &cache, &iam, &td.path().join("moteur-2.log"), &jeton_alice);

    // (b) LA MACHINE EST RETROUVÉE — avec son identifiant, ses capacités et sa
    // date de première attache : le registre neuf ne réinvente rien.
    let apres = machines(&s2.addr, &jeton_alice);
    assert_eq!(apres.len(), 1, "une liste vide après redémarrage serait le défaut d'origine");
    assert_eq!(apres[0]["machine"], serde_json::json!("portable-alice"));
    assert_eq!(apres[0]["id"], serde_json::json!(id_alice), "le MÊME identifiant");
    assert_eq!(apres[0]["capacites"], serde_json::json!(["read", "exec"]));
    assert!(
        apres[0]["attache_ms"].as_i64().unwrap_or(0) > 0,
        "la première attache est datée"
    );

    // CONNUE N'EST PAS JOIGNABLE : personne n'a battu depuis CE processus, donc
    // elle n'est pas annoncée vivante — même si sa dernière présence écrite est
    // récente (le redémarrage vient d'avoir lieu).
    assert_eq!(
        apres[0]["vivant"],
        serde_json::json!(false),
        "relue d'un journal, elle n'est pas vivante pour autant : {apres:?}"
    );

    // Et un ordre vers elle est REFUSÉ, tout de suite, avec un motif — pas une
    // attente de 30 s qui ressemblerait à une panne.
    let debut = Instant::now();
    let (code, corps) = requete(
        &s2.addr,
        "POST",
        "/api/remote/appel",
        Some(&jeton_alice),
        Some(&format!(
            r#"{{"client_id":"{id_alice}","outil":"read","arguments":{{}},"delai_s":30}}"#
        )),
    );
    assert_eq!(code, 409, "un ordre vers une machine absente est refusé : {corps}");
    assert!(
        corps.contains("pas joignable"),
        "le refus est MOTIVÉ (le silence ressemble à une panne) : {corps}"
    );
    assert!(debut.elapsed() < Duration::from_secs(5), "et il ne fait pas attendre le délai");

    // (c) LE CLOISONNEMENT SURVIT À LA RELECTURE.
    let bob_apres = machines(&s2.addr, &jeton_bob);
    assert_eq!(noms(&bob_apres), vec!["tour-bob"], "bob relit la sienne, et rien d'autre");
    assert_eq!(bob_apres[0]["id"], serde_json::json!(id_bob));
    // Un identifiant relu et DEVINÉ n'ouvre toujours rien.
    let (code, corps) = requete(
        &s2.addr,
        "POST",
        "/api/remote/appel",
        Some(&jeton_bob),
        Some(&format!(r#"{{"client_id":"{id_alice}","outil":"read","arguments":{{}}}}"#)),
    );
    assert_eq!(code, 409, "bob ne commande pas la machine d'alice : {corps}");
    assert!(corps.contains("pas à vous"), "et le refus le dit : {corps}");

    // Et la machine revient d'elle-même : elle redemande du travail (204 = rien
    // pour toi, rappelle), et ce geste la rend vivante de nouveau.
    let (code, _) = requete(
        &s2.addr,
        "GET",
        &format!("/api/remote/ordres?client={id_alice}&attente=1"),
        Some(&jeton_alice),
        None,
    );
    assert_eq!(code, 204, "aucun ordre en attente : le rythme normal du canal");
    assert_eq!(
        machines(&s2.addr, &jeton_alice)[0]["vivant"],
        serde_json::json!(true),
        "elle a redemandé du travail : elle est vivante"
    );

    // ── 3. UNE MACHINE QUI SE TAIT DEPUIS LONGTEMPS ─────────────────────────
    // On laisse le registre tel quel et on recule sa dernière présence, comme
    // après une nuit de panne du moteur : le journal dit « connue depuis
    // longtemps, vue il y a deux heures ».
    drop(s2);
    let vieux = maintenant_ms() - 2 * 3_600_000;
    let mut reculee = None;
    for j in journaux(&cache) {
        let t = lire(&j);
        if !t.contains("portable-alice") {
            continue;
        }
        let ligne = format!(
            "{}\n",
            serde_json::json!({
                "kind": "attache", "id": id_alice, "compte": "alice",
                "machine": "portable-alice", "capacites": ["read", "exec"],
                "attache_ms": vieux, "vue_ms": vieux,
            })
        );
        std::fs::write(&j, ligne).unwrap();
        reculee = Some(j);
    }
    assert!(reculee.is_some(), "le journal d'alice est retrouvé pour le reculer");

    let s3 = Serveur::demarrer(&store, &cache, &iam, &td.path().join("moteur-3.log"), &jeton_alice);
    // (d) ELLE EST CONNUE, MAIS PAS ANNONCÉE VIVANTE — et sa liste n'est pas
    // vide pour autant : une réponse vide aurait la forme d'une bonne réponse.
    let ancienne = machines(&s3.addr, &jeton_alice);
    assert_eq!(noms(&ancienne), vec!["portable-alice"], "elle reste connue");
    assert_eq!(ancienne[0]["vivant"], serde_json::json!(false), "mais pas vivante");
    assert!(
        ancienne[0]["silence_ms"].as_i64().unwrap_or(0) >= 3_600_000,
        "et son silence est dit en clair : {}",
        ancienne[0]["silence_ms"]
    );
    let (code, corps) = requete(
        &s3.addr,
        "POST",
        "/api/remote/appel",
        Some(&jeton_alice),
        Some(&format!(r#"{{"client_id":"{id_alice}","outil":"read","arguments":{{}}}}"#)),
    );
    assert_eq!(code, 409, "on ne commande pas une machine muette : {corps}");
    assert!(corps.contains("pas joignable"), "refus motivé : {corps}");
}
