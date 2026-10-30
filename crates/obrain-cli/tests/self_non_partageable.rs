//! DEUX PERSONNES, UN MÊME NOM D'UTILISATEUR : DEUX MÉMOIRES (2026-09-16).
//!
//! ── CE QUI ÉTAIT FAUX ───────────────────────────────────────────────────────
//! `ancrage::self_qn_pour` coupait l'identité rendue par la garde
//! (`Garde::identite` → `utilisateur@compte`) au premier `@` et rendait
//! `self:<utilisateur>` : `alice@compteA` et `alice@compteB` obtenaient le MÊME
//! self. Or `memoire::journal` est une fonction du nom du self (un fichier
//! par self) et `scripts/memoire-appliquer-comptes.sh` applique dans
//! `selfs/<nom>.obrain` : deux personnes écrivaient donc dans un seul journal,
//! et leurs souvenirs atterrissaient dans une seule base. Le commentaire de la
//! fonction interdisait exactement cela depuis le premier jour.
//!
//! ── CE QUE CE TEST PROUVE, ET PAR QUOI IL LE PROUVE ─────────────────────────
//! Rien d'ici n'est simulé : deux magasins IAM, deux serveurs `viz serve` réels
//! (comme le service 8432, mais sur des ports LIBRES > 8400), des jetons réels,
//! et la route qui passe la forme documentée (`GET /api/engine/resonance`, la
//! seule surface qui donne à `self_qn_pour` l'identité ENTIÈRE — vérifié par
//! lecture de `viz_serve.rs`). On y lit le self que l'appelant s'est vu
//! attribuer, et on compare les JOURNAUX que le binaire désigne lui-même.
//!
//!   1. `alice@compte-a` et `alice@compte-b` ne partagent PAS de journal (deux
//!      chemins distincts, et une écriture dans l'un ne touche pas l'autre) ;
//!   2. `alice@compte-a` est STABLE dans le temps : même nom, même journal,
//!      y compris après relance du serveur (aucun état, aucune horloge) ;
//!   3. le chemin qui passe un compte NU (`POST /api/remote/memoire`, celui qui
//!      est EN SERVICE) garde EXACTEMENT son comportement d'avant :
//!      `self:<utilisateur>`, le même journal qu'avant ce correctif ;
//!   4. le nom NU et le nom QUALIFIÉ ne désignent pas la même mémoire — c'est
//!      la séparation, mesurée sur les fichiers, pas sur les chaînes.
//!
//! ── ISOLATION ───────────────────────────────────────────────────────────────
//! Tout vit sous un répertoire temporaire : magasins IAM, stores servis, cache
//! de mémoire. Aucune base vivante n'est ouverte, aucun service n'est touché
//! (les serveurs de ce test sont des enfants du test, tués à la sortie).

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

/// Le serveur de test, tué à la sortie — même sur un `panic!`.
struct Serveur(Child);

impl Drop for Serveur {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Lance `viz serve` comme le service le fait — même binaire, port LIBRE,
/// `--enrich-ricci 0` (rien n'entretient un store temporaire), `OBRAIN_IAM_ACCOUNT`
/// = le COMPTE de cette instance (c'est le paramètre du chantier : deux
/// instances, deux comptes, un même nom d'utilisateur, un même cache de mémoire).
fn lancer_serveur(
    store: &Path,
    cache: &Path,
    iam: &Path,
    compte: &str,
    log: &Path,
) -> (Serveur, String) {
    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    let enfant = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("viz")
        .arg("serve")
        .arg("--store")
        .arg(store)
        .arg("--enrich-ricci")
        .arg("0")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        .env("OBRAIN_IAM", "1")
        .env("OBRAIN_IAM_STORE", iam)
        .env("OBRAIN_IAM_ACCOUNT", compte)
        // Le magasin d'identités est ÉCRIT par le serveur (jetons) : il vit sous
        // /tmp, jamais sur un store vivant.
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(log).expect("log du serveur"))
        .spawn()
        .expect("spawn viz serve");
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

/// Le serveur est-il ARMÉ ? Sans la garde, il n'y a pas d'identité du tout, donc
/// rien à séparer : le test ne prouverait rien.
fn exiger_garde_armee(log: &Path) {
    let texte = std::fs::read_to_string(log).unwrap_or_default();
    assert!(texte.contains("IAM ARMÉ"), "le serveur de test devait être armé :\n{texte}");
}

/// Le chemin du journal de mémoire d'un self, DEMANDÉ AU BINAIRE (le nom du
/// fichier est un hachage du self : il ne se devine pas, il se lit).
fn journal_de(cache: &Path, self_qn: &str) -> PathBuf {
    let out = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        .arg("memoire")
        .arg("--self")
        .arg(self_qn)
        .arg("--format")
        .arg("json")
        .output()
        .expect("memoire --self");
    let txt = String::from_utf8_lossy(&out.stdout);
    let d = json(txt.trim());
    PathBuf::from(
        d["journal"].as_str().unwrap_or_else(|| panic!("journal absent pour {self_qn} : {txt}")),
    )
}

fn lignes(chemin: &Path) -> usize {
    match std::fs::read_to_string(chemin) {
        Ok(t) => t.lines().filter(|l| !l.trim().is_empty()).count(),
        Err(_) => 0,
    }
}

/// Le self que le serveur attribue à l'appelant, lu sur la seule route qui
/// reçoit l'identité ENTIÈRE (`utilisateur@compte`) : celle de la résonance.
fn self_de_l_appelant(addr: &str, jeton: &str) -> String {
    let (st, c) = http(addr, "GET", "/api/engine/resonance", Some(jeton), None);
    assert_eq!(st, 200, "résonance : {c}");
    let v = json(&c);
    v["moi"].as_str().unwrap_or_else(|| panic!("« moi » absent de la résonance : {c}")).to_string()
}

fn connexion(addr: &str, nom: &str, mdp: &str) -> String {
    let corps = serde_json::json!({ "username": nom, "password": mdp }).to_string();
    let (st, c) = http(addr, "POST", "/api/auth/login", None, Some(&corps));
    assert_eq!(st, 200, "connexion de {nom} : {c}");
    json(&c)["jeton"].as_str().unwrap_or_else(|| panic!("jeton absent : {c}")).to_string()
}

/// Un magasin d'identités portant UN compte : c'est la forme réelle du défaut
/// (deux moteurs, deux comptes). Deux personnes de même nom ne peuvent pas
/// cohabiter dans un seul magasin — les utilisateurs y sont identifiés par
/// `u-<nom>` (`obrain-iam/src/admin.rs`) — donc l'isolation se joue bien entre
/// deux comptes, et c'est ce que ce test met en scène.
///
/// `--compte` est le LOCATAIRE manipulé, et c'est aussi celui que la garde
/// exigera : les politiques d'amorçage sont posées SOUS SON NOM. Les poser sous
/// le défaut (« obrain ») alors que le serveur tourne avec `compte-a` produit un
/// `permission denied: action=graph:read resource=orn:obrain:graph:compte-a:…`
/// sur toutes les routes — mesuré en écrivant ce test, et déjà payé une fois par
/// la porte d'acceptation (`acceptation-self-par-compte.sh`, étape 0).
fn magasin_iam(chemin: &Path, compte: &str, espace: &str, utilisateur: &str, mdp: &str) {
    let o = obrain()
        .arg("init")
        .arg(chemin)
        .output()
        .expect("init IAM");
    assert!(o.status.success(), "init IAM : {}", String::from_utf8_lossy(&o.stderr));
    let o = obrain()
        .arg("iam")
        .arg("amorcer")
        .arg(chemin)
        .arg(espace)
        .arg("--compte")
        .arg(compte)
        .arg("--utilisateur")
        .arg(utilisateur)
        .arg("--mot-de-passe")
        .arg(mdp)
        .output()
        .expect("iam amorcer");
    assert!(
        o.status.success(),
        "amorçage de {utilisateur} : {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Un store servi par le serveur de test (jamais la base d'un compte : c'est ce
/// qui laisse la passe d'application prendre l'exclusif plus tard).
fn store_neuf(chemin: &Path) {
    let o = obrain().arg("init").arg(chemin).output().expect("init store");
    assert!(o.status.success(), "init {} : {}", chemin.display(), String::from_utf8_lossy(&o.stderr));
}

#[test]
fn deux_comptes_de_meme_utilisateur_ne_partagent_pas_de_memoire() {
    let td = tempfile::tempdir().unwrap();
    // LE CACHE EST COMMUN : c'est le cas réel — un même `OBRAIN_VIZ_CACHE_DIR`
    // (défaut /tmp/obrain-viz-cache) partagé par deux moteurs d'une même machine.
    let cache = td.path().join("cache");
    std::fs::create_dir_all(cache.join("memoire")).unwrap();

    let (iam_a, iam_b) = (td.path().join("iam-a.obrain"), td.path().join("iam-b.obrain"));
    let (store_a, store_b) = (td.path().join("service-a.obrain"), td.path().join("service-b.obrain"));
    store_neuf(&store_a);
    store_neuf(&store_b);
    let (mdp_a, mdp_b) = ("mdp-compte-a", "mdp-compte-b");
    // ── L'ESPACE ACCORDÉ PORTE LE NOM DU STORE, SINON LA PORTE MESURE UN REFUS
    // DE DROITS ET L'APPELLE UNE PANNE ─────────────────────────────────────
    // La ressource jugée par la politique est le tenant, tiré du NOM DU STORE
    // (`…/service-a.obrain` ⇒ `orn:…:tenant/service-a`) : un espace accordé sous
    // un autre nom produit un `permission denied: action=graph:read` sur TOUTES
    // les routes, avec une identité pourtant valide (défaut déjà payé une fois,
    // voir l'en-tête de `scripts/acceptation-self-par-compte.sh`).
    magasin_iam(&iam_a, "compte-a", "service-a", "alice", mdp_a);
    magasin_iam(&iam_b, "compte-b", "service-b", "alice", mdp_b);

    // ── 1. DEUX INSTANCES, DEUX COMPTES, LE MÊME NOM D'UTILISATEUR ──────────
    let log_a = td.path().join("serveur-a.log");
    let log_b = td.path().join("serveur-b.log");
    let (serveur_a, addr_a) = lancer_serveur(&store_a, &cache, &iam_a, "compte-a", &log_a);
    let (serveur_b, addr_b) = lancer_serveur(&store_b, &cache, &iam_b, "compte-b", &log_b);
    exiger_garde_armee(&log_a);
    exiger_garde_armee(&log_b);
    let jeton_a = connexion(&addr_a, "alice", mdp_a);
    let jeton_b = connexion(&addr_b, "alice", mdp_b);

    let self_a = self_de_l_appelant(&addr_a, &jeton_a);
    let self_b = self_de_l_appelant(&addr_b, &jeton_b);
    eprintln!("── SELF RÉSOLU ── compte-a : {self_a} · compte-b : {self_b}");
    assert_eq!(self_a, "self:alice@compte-a", "le self doit porter l'utilisateur ET son compte");
    assert_eq!(self_b, "self:alice@compte-b", "le self doit porter l'utilisateur ET son compte");
    assert_ne!(self_a, self_b, "DEUX COMPTES NE DOIVENT JAMAIS CONVERGER SUR LE MÊME SELF");

    // ── 2. LES JOURNAUX SONT DEUX FICHIERS ─────────────────────────────────
    let journal_a = journal_de(&cache, &self_a);
    let journal_b = journal_de(&cache, &self_b);
    let journal_nu = journal_de(&cache, "self:alice");
    eprintln!(
        "── JOURNAUX ── {} · {} · (nom nu) {}",
        journal_a.display(),
        journal_b.display(),
        journal_nu.display()
    );
    assert_ne!(journal_a, journal_b, "deux selfs distincts, donc deux journaux distincts");
    assert_ne!(journal_a, journal_nu, "le self qualifié ne partage pas le journal du nom nu");
    assert_ne!(journal_b, journal_nu, "le self qualifié ne partage pas le journal du nom nu");
    assert!(!journal_a.exists(), "aucune mémoire avant qu'Alice écrive");

    // ── 3. LE CHEMIN EN SERVICE : UN NOM NU RESTE UN NOM NU ────────────────
    // `POST /api/remote/memoire` déduit le self du compte du JETON et le passe
    // NU (`compte_appelant` rend `Identite::username()`). Ce chemin est en
    // service : il doit garder exactement son comportement d'avant, sur une
    // instance dont le compte EST `compte-b` — donc sans qualification.
    let (st, c) = http(
        &addr_b,
        "POST",
        "/api/remote/attacher",
        Some(&jeton_b),
        Some(&serde_json::json!({ "machine": "tour-alice", "capacites": ["write"] }).to_string()),
    );
    assert_eq!(st, 200, "attachement : {c}");
    let client_b = json(&c)["client_id"].as_str().expect("client_id").to_string();
    let envoi = serde_json::json!({
        "client_id": client_b,
        "intentions": [
            { "kind": "note", "texte": "note du compte-b, par la machine attachee", "sujet": "separation" }
        ]
    })
    .to_string();
    let (st, c) = http(&addr_b, "POST", "/api/remote/memoire", Some(&jeton_b), Some(&envoi));
    assert_eq!(st, 200, "publication : {c}");
    let r = json(&c);
    assert_eq!(
        r["self"].as_str(),
        Some("self:alice"),
        "le chemin nu garde EXACTEMENT son comportement d'avant (self:alice) : {c}"
    );
    let journal_publie = PathBuf::from(r["journal"].as_str().expect("journal"));
    assert_eq!(journal_publie, journal_nu, "le nom nu désigne toujours le même journal qu'avant");
    assert_eq!(lignes(&journal_nu), 1, "la note est bien dans le journal du nom nu");
    assert_eq!(lignes(&journal_b), 0, "…et PAS dans celui du self qualifié : deux mémoires séparées");

    // ── 4. STABILITÉ DANS LE TEMPS ─────────────────────────────────────────
    for _ in 0..3 {
        assert_eq!(self_de_l_appelant(&addr_a, &jeton_a), self_a, "le self ne doit pas bouger");
    }
    // Relance du serveur : même self, même journal. Un nom qui dépendrait d'un
    // état de processus (ou d'une horloge) changerait ici.
    drop(serveur_a);
    let log_a2 = td.path().join("serveur-a2.log");
    let (_serveur_a2, addr_a2) = lancer_serveur(&store_a, &cache, &iam_a, "compte-a", &log_a2);
    let jeton_a2 = connexion(&addr_a2, "alice", mdp_a);
    let self_a2 = self_de_l_appelant(&addr_a2, &jeton_a2);
    assert_eq!(self_a2, self_a, "le self est STABLE d'un processus à l'autre");
    assert_eq!(journal_de(&cache, &self_a2), journal_a, "…et son journal aussi");

    drop(serveur_b);
    assert_eq!(lignes(&journal_nu), 1, "rien n'a été ajouté au journal du nom nu entre-temps");

    // ── 5. LE COMPTE HISTORIQUE GARDE SON NOM — LA MÉMOIRE N'EST PAS ORPHELINÉE
    // `obrain` est le compte sous lequel la ligne déployée porte déjà du contenu
    // réel sous `Self {qn: 'self:<utilisateur>'}` (selfs/{achaptal,admin,triviere}
    // .obrain, mesuré le 2026-09-16). Une instance armée sur CE compte doit
    // continuer de nommer `self:alice` : c'est l'arbitrage qui empêche le
    // correctif de couper en deux la mémoire de gens qui la lisent déjà. Ce n'est
    // pas une tolérance cachée : c'est la liste `COMPTES_A_SELF_HISTORIQUE`
    // d'`ancrage.rs`, et elle est faite pour être relue.
    let mdp_h = "mdp-compte-historique";
    let (iam_h, store_h) = (td.path().join("iam-h.obrain"), td.path().join("service-h.obrain"));
    store_neuf(&store_h);
    magasin_iam(&iam_h, "obrain", "service-h", "alice", mdp_h);
    let log_h = td.path().join("serveur-h.log");
    let (_serveur_h, addr_h) = lancer_serveur(&store_h, &cache, &iam_h, "obrain", &log_h);
    exiger_garde_armee(&log_h);
    let jeton_h = connexion(&addr_h, "alice", mdp_h);
    let self_h = self_de_l_appelant(&addr_h, &jeton_h);
    assert_eq!(
        self_h, "self:alice",
        "sur le compte historique, l'identité alice@obrain garde le nom NU — sinon la mémoire \
         déjà écrite dans selfs/alice.obrain et dans le journal de self:alice serait orpheline"
    );
    assert_eq!(
        journal_de(&cache, &self_h),
        journal_nu,
        "…et c'est bien LE MÊME journal qu'avant le correctif"
    );
}
