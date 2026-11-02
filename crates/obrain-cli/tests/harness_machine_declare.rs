//! AUTO-DÉCLARATION DE LA MACHINE — point 1 du relevé `docs/MACHINES-CONNUES-2026-09-15.md`.
//!
//! Ce que ce fichier protège n'est pas une route, c'est une CHAÎNE, et elle a
//! deux moitiés qu'il faut prouver ensemble :
//!
//!   1. au démarrage, le harness **journalise sa propre identité** — le nom
//!      d'hôte lu du noyau, la plateforme, le store qu'il sert RÉELLEMENT, le
//!      mode offert, l'instant de la déclaration. C'est la moitié que le daemon
//!      peut faire : il ouvre la base en LECTURE (verrou partagé) et n'a donc pas
//!      le droit d'y écrire quoi que ce soit ;
//!   2. la passe `obrain machines --appliquer` **applique** ces faits, en
//!      exclusif, dans une base — le motif en deux temps de `memoire.rs`.
//!
//! Avant ce chaînon, une machine n'entrait dans le graphe que si un `selfd` s'y
//! était attaché. Un moteur qui tourne seul — le cas le plus fréquent, et le seul
//! qui existe sur une machine fraîchement provisionnée (n2 dans le relevé) —
//! restait innommable.
//!
//! **Ce que le test refuse de laisser passer**, et c'est le point délicat du
//! chantier : qu'une machine sans `selfd` se retrouve avec un CATALOGUE DE
//! CAPACITÉS inventé. « Annoncer est décider » — une liste ne doit jamais
//! contenir une ressource dont on n'a pas vérifié le droit. Un moteur seul
//! apparaît donc SANS capacité, et cela se vérifie par REQUÊTE, pas par lecture
//! de code.
//!
//! Méthode : le BINAIRE RÉEL, sur des stores, des caches et des ports temporaires,
//! comme `tests/harness_machine.rs`. Rien n'est simulé, et le cache vivant de la
//! machine (`/tmp/obrain-viz-cache`) n'est jamais touché : un test qui y écrirait
//! déclarerait au graphe des moteurs servant des bases de test, et la passe
//! suivante y créerait des nœuds `machine:` pour des stores disparus.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn binaire() -> &'static str {
    env!("CARGO_BIN_EXE_obrain")
}

/// Un port LIBRE, et jamais l'un de ceux que le protocole interdit de toucher :
/// 8432/8433 (le moteur et son voisin), 8090 (le harness en service), 8144
/// (this-rs), 3082/3083 (le harness du poste). Un test qui tomberait sur l'un
/// d'eux remplacerait un service vivant — le noyau ne prévient pas.
fn port_libre() -> u16 {
    for _ in 0..50 {
        let p = TcpListener::bind("127.0.0.1:0")
            .expect("port éphémère")
            .local_addr()
            .unwrap()
            .port();
        if p > 8400 && ![8432, 8433, 8455, 8090, 8144, 3082, 3083].contains(&p) {
            return p;
        }
    }
    panic!("aucun port libre hors de ceux qui sont interdits");
}

/// Une requête HTTP brute : on ne teste pas un client, on teste ce que le serveur
/// écrit sur la socket.
fn requete(addr: &str, chemin: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("connexion");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let req = format!(
        "GET {chemin} HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).expect("envoi");
    let mut rep = String::new();
    s.read_to_string(&mut rep).expect("lecture");
    let statut: u16 = rep
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (
        statut,
        rep.split("\r\n\r\n").nth(1).unwrap_or("").to_string(),
    )
}

fn obrain(args: &[&str], cache: &Path) -> Output {
    Command::new(binaire())
        .args(args)
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        // Aucun store vivant n'est visé : ces commandes travaillent sur des bases
        // temporaires, jamais sur celles des services.
        .output()
        .unwrap_or_else(|e| panic!("obrain {args:?} : {e}"))
}

fn init_store(store: &Path) {
    let o = Command::new(binaire())
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
    let o = Command::new(binaire())
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

/// Une requête de lecture sur un store : le rendu texte est celui que lit la
/// porte d'acceptation (`[]` pour un résultat vide).
fn interroger(store: &Path, cache: &Path, gql: &str) -> String {
    let o = obrain(
        &["query", &store.display().to_string(), "--read-only", gql],
        cache,
    );
    assert!(
        o.status.success(),
        "query « {gql} » : {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Le journal des auto-déclarations d'un cache : une ligne par instance.
fn declarations(cache: &Path) -> Vec<serde_json::Value> {
    let dir = cache.join("machines");
    let lectures = std::fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!(
            "journal des machines introuvable dans {} : {e}",
            dir.display()
        )
    });
    let mut fichiers: Vec<PathBuf> = lectures
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .collect();
    fichiers.sort();
    let mut lignes = Vec::new();
    for f in fichiers {
        let txt = std::fs::read_to_string(&f).expect("journal lisible");
        for l in txt.lines().filter(|l| !l.trim().is_empty()) {
            lignes.push(
                serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("{} n'est pas du JSON : {e}", f.display())),
            );
        }
    }
    lignes
}

struct Harness {
    enfant: Child,
    addr: String,
    store: PathBuf,
    pid: u32,
    arrete: bool,
    _td: tempfile::TempDir,
}

impl Harness {
    /// Arrête le harness et rend le verrou PARTAGÉ qu'il tient sur le store.
    ///
    /// C'est le geste que la porte d'acceptation fait avant sa passe : la passe
    /// ouvre en EXCLUSIF, donc un lecteur encore vivant la ferait échouer — et
    /// l'échec serait imputé à la passe, pas au verrou.
    fn arreter(&mut self) {
        if !self.arrete {
            let _ = self.enfant.kill();
            let _ = self.enfant.wait();
            self.arrete = true;
        }
    }

    /// La sortie d'erreur du harness, après son arrêt.
    fn stderr(&mut self) -> String {
        let Some(mut e) = self.enfant.stderr.take() else {
            return String::new();
        };
        let mut s = String::new();
        let _ = e.read_to_string(&mut s);
        s
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.arreter();
    }
}

/// Lance le binaire réel sur un store, un cache et une racine donnés.
///
/// `OBRAIN_HARNESS_ROOT` est la SEULE façon de déclarer la racine de données d'un
/// harness : rien ne la devine depuis le chemin du store, et le journal doit
/// porter `non_declaree` quand personne ne l'a dite.
fn lancer(store: PathBuf, cache: &Path, racine: Option<&Path>, td: tempfile::TempDir) -> Harness {
    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    let mut cmd = Command::new(binaire());
    cmd.arg("harness-serve")
        .arg(&store)
        .arg("--addr")
        .arg(&addr);
    cmd.env("OBRAIN_VIZ_CACHE_DIR", cache);
    match racine {
        Some(r) => cmd.env("OBRAIN_HARNESS_ROOT", r),
        None => cmd.env_remove("OBRAIN_HARNESS_ROOT"),
    };
    let enfant = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn harness-serve");
    let pid = enfant.id();
    let fin = Instant::now() + Duration::from_secs(30);
    while Instant::now() < fin {
        if TcpStream::connect(&addr).is_ok() {
            return Harness {
                enfant,
                addr,
                store,
                pid,
                arrete: false,
                _td: td,
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("harness jamais prêt sur {addr} (store {})", store.display());
}

/// Un store initialisé et peuplé d'une session nommée, dans son propre
/// répertoire temporaire.
fn store_avec_session(base: &str, nom_session: &str) -> (PathBuf, tempfile::TempDir) {
    let td = tempfile::tempdir().expect("tempdir");
    let store = td.path().join(format!("{base}.obrain"));
    init_store(&store);
    semer_session(&store, nom_session);
    (store, td)
}

// ============================================================================
// (1) LE HARNESS JOURNALISE EXACTEMENT CE QU'IL SERT
// ============================================================================

/// « Journaliser ce que LUI SEUL sait » — et rien d'autre. Le journal doit porter
/// la même identité que la route `/api/harness/machine` : si les deux pouvaient
/// diverger, on ne saurait plus laquelle croire, et une action partirait sur la
/// foi d'un fait que le moteur n'a jamais annoncé.
#[test]
fn un_harness_journalise_exactement_ce_qu_il_sert() {
    let cache = tempfile::tempdir().expect("tempdir cache");
    let racine = cache.path().join("racine-declaree");
    std::fs::create_dir_all(&racine).expect("racine");
    let (store, td) = store_avec_session("kb-alpha", "session-alpha");
    let h = lancer(store.clone(), cache.path(), Some(&racine), td);

    let (code, corps) = requete(&h.addr, "/api/harness/machine");
    assert_eq!(code, 200, "{corps}");
    let annonce: serde_json::Value = serde_json::from_str(&corps).expect("identité JSON");

    let lignes = declarations(cache.path());
    assert_eq!(lignes.len(), 1, "un harness, une déclaration : {lignes:?}");
    let l = &lignes[0];

    // 1. Le fait est daté et GENRÉ : c'est ce qui le distingue, dans le même
    //    répertoire, des attaches du registre.
    assert_eq!(l["kind"], "machine");
    assert!(
        l["declaree_ms"].as_i64().unwrap_or(0) > 0,
        "déclaration non datée : {l}"
    );

    // 2. Le nom vient du NOYAU, et sa provenance est dite. Un nom lu dans
    //    `$HOSTNAME` et un nom lu dans le noyau ne se valent pas.
    let noyau = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .expect("hostname")
        .trim()
        .to_string();
    assert_eq!(
        l["machine"],
        noyau.as_str(),
        "le journal doit nommer l'hôte : {l}"
    );
    assert_eq!(l["nom_source"], "/proc/sys/kernel/hostname");
    assert_ne!(
        l["machine"], "inconnu",
        "un nom non lu serait pire qu'aucun : {l}"
    );

    // 3. **LE STORE RÉELLEMENT SERVI**, et pas celui qu'on aurait pu croire : le
    //    chemin journalisé est celui qui répond, et il contient bien la session
    //    qu'on y a semée.
    assert_eq!(l["store"], store.display().to_string(), "{l}");
    assert_eq!(l["base"], "kb-alpha.obrain");
    assert_eq!(
        l["instance"],
        format!("machine:{}#kb-alpha.obrain", noyau),
        "l'instance porte le store servi : deux moteurs d'un même hôte restent distincts"
    );
    let (sc, sessions) = requete(&h.addr, "/api/harness/sessions?limit=50");
    assert_eq!(sc, 200, "{sessions}");
    assert!(
        sessions.contains("session-alpha"),
        "le store journalisé est bien celui qui est servi : {sessions}"
    );

    // 4. La plateforme et le mode offert : mesurés, jamais devinés.
    assert_eq!(l["os"], std::env::consts::OS);
    assert_eq!(l["arch"], std::env::consts::ARCH);
    assert_eq!(
        l["mode"], "ro",
        "le harness ne propose que la lecture : {l}"
    );
    assert_eq!(l["racine_declaree"], racine.display().to_string());
    assert_eq!(
        l["ecoute"], h.addr,
        "l'adresse d'écoute est celle RÉELLEMENT liée"
    );
    assert_eq!(
        l["pid"].as_u64().unwrap_or(0),
        u64::from(h.pid),
        "le processus se nomme"
    );
    assert_eq!(l["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(l["commit"], env!("OBRAIN_GIT_COMMIT"));
    assert_eq!(l["arbre"], env!("OBRAIN_GIT_ARBRE"));

    // 5. **CE QUI N'EST PAS JOURNALISÉ** : ni compte, ni capacité, ni `id`. Un
    //    moteur seul n'appartient à personne ; une ligne qui porterait un `id`
    //    serait relue par le registre comme l'attache d'un `selfd`.
    for absent in ["id", "compte", "capacites", "vivant", "joignable"] {
        assert!(
            l.get(absent).is_none(),
            "« {absent} » ne doit pas être journalisé : {l}"
        );
    }

    // 6. Le journal et la route disent la MÊME chose, sur les faits qui ne
    //    bougent pas d'une réponse à l'autre.
    for champ in [
        "machine",
        "instance",
        "store",
        "store_racine",
        "mode",
        "os",
        "arch",
    ] {
        assert_eq!(
            l[champ], annonce[champ],
            "journal et route divergent sur « {champ} »"
        );
    }
}

/// Deux harnesses, deux stores, deux racines — et le journal doit porter DEUX
/// faits distincts, jusque dans le graphe.
///
/// Le test dit son hypothèse au lieu de la cacher : les deux tournent sur le MÊME
/// hôte (c'est une machine de test, pas une flotte). Le nom d'hôte ne les
/// distingue donc PAS — ce sont le store servi et l'instance qui le font, et
/// c'est exactement pourquoi l'identité ne se réduit pas au nom d'hôte.
///
/// Puis la PASSE, puis la REQUÊTE du récit. La question posée au graphe est celle
/// d'une IA : `qualified_name` — pas `qn`. Un nœud écrit avec une seule des deux
/// propriétés serait invisible au lecteur qu'on veut servir.
#[test]
fn deux_harnesses_deux_stores_deux_faits_et_la_passe_les_nomme() {
    let cache = tempfile::tempdir().expect("tempdir cache");
    let (store_a, td_a) = store_avec_session("kb-a", "session-a");
    let (store_b, td_b) = store_avec_session("kb-b", "session-b");
    let racine_a = cache.path().join("racine-a");
    let racine_b = cache.path().join("racine-b");
    std::fs::create_dir_all(&racine_a).unwrap();
    std::fs::create_dir_all(&racine_b).unwrap();

    let mut ha = lancer(store_a.clone(), cache.path(), Some(&racine_a), td_a);
    let mut hb = lancer(store_b.clone(), cache.path(), Some(&racine_b), td_b);

    // ── 1. Le journal porte deux faits, et ils sont distincts ───────────────
    let lignes = declarations(cache.path());
    assert_eq!(lignes.len(), 2, "deux moteurs, deux faits : {lignes:?}");
    let noyau = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap()
        .trim()
        .to_string();
    let stores: Vec<String> = lignes
        .iter()
        .map(|l| l["store"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        stores.contains(&store_a.display().to_string()),
        "{lignes:?}"
    );
    assert!(
        stores.contains(&store_b.display().to_string()),
        "{lignes:?}"
    );
    let mut instances: Vec<String> = lignes
        .iter()
        .map(|l| l["instance"].as_str().unwrap_or("").to_string())
        .collect();
    // L'ORDRE n'est pas un contrat : chaque instance écrit dans son propre
    // fichier (le hachage de son nom), donc l'ordre de relecture suit les noms de
    // fichiers. Ce qui est un contrat, c'est l'ENSEMBLE des instances.
    instances.sort();
    assert_eq!(
        instances,
        vec![
            format!("machine:{noyau}#kb-a.obrain"),
            format!("machine:{noyau}#kb-b.obrain")
        ],
        "l'instance porte le store servi, et c'est ce qui rend les deux moteurs discernables"
    );

    // ── 2. « ANNONCER EST DÉCIDER » : une déclaration n'est pas un client ───
    //    Le registre ne doit PAS voir naître une machine de ce journal : sinon
    //    la liste d'un compte se peuplerait d'un moteur qui ne lui appartient
    //    pas, et d'une machine dont personne n'a vérifié le droit.
    //
    //    Depuis que le LISTING interroge les deux sources (le registre ET les
    //    auto-déclarations), l'assertion ne peut plus être « la liste est vide » :
    //    elle doit dire ce qui est vrai — la machine est NOMMÉE, mais AUCUN client
    //    ne s'y est rattaché. C'est le fait qui compte, et il est plus précis
    //    qu'avant : `rattachee` distingue « une machine sur laquelle quelqu'un
    //    s'est attaché » de « une machine sur laquelle un moteur tourne ».
    let listing = obrain(&["machines"], cache.path());
    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&listing.stdout).expect("rapport JSON de `obrain machines`");
    assert_eq!(v["rattachees"], 0, "AUCUN client ne s'est rattaché : {v}");
    assert_eq!(
        v["declarees_seulement"], 1,
        "un hôte vu seulement par des moteurs : {v}"
    );
    assert_eq!(v["machines"], 1, "l'hôte est NOMMÉ, une seule fois : {v}");
    let m0 = &v["connues"][0];
    assert_eq!(m0["rattachee"], false, "et il se dit NON rattaché : {v}");
    assert!(
        m0["comptes"].as_array().unwrap().is_empty(),
        "aucun compte : {v}"
    );
    assert!(
        m0["capacites"].as_array().unwrap().is_empty(),
        "aucune capacité inventée : {v}"
    );
    let moteurs = m0["moteurs"].as_array().expect("les moteurs déclarés");
    assert_eq!(
        moteurs.len(),
        2,
        "les DEUX moteurs sont nommés, avec leur store : {v}"
    );
    assert!(
        moteurs
            .iter()
            .any(|m| m["instance"] == format!("machine:{noyau}#kb-b.obrain")),
        "l'instance porte le store servi : {v}"
    );

    // ── 3. La passe (le verrou exclusif exige qu'aucun lecteur ne tienne la base)
    ha.arreter();
    hb.arreter();
    let passe = obrain(
        &[
            "machines",
            "--store",
            &store_a.display().to_string(),
            "--appliquer",
        ],
        cache.path(),
    );
    assert!(
        passe.status.success(),
        "passe : {}",
        String::from_utf8_lossy(&passe.stderr)
    );
    let rapport: serde_json::Value = serde_json::from_slice(&passe.stdout).expect("rapport JSON");
    assert_eq!(
        rapport["connues"], 1,
        "un seul hôte, donc une seule machine : {rapport}"
    );
    assert_eq!(
        rapport["machines"], 3,
        "un nœud machine:<hôte> ET un nœud par instance déclarée : {rapport}"
    );
    assert!(
        rapport["refusees"].as_array().unwrap().is_empty(),
        "{rapport}"
    );

    // ── 4. LA REQUÊTE DU RÉCIT ─────────────────────────────────────────────
    let qn = interroger(
        &store_a,
        cache.path(),
        "MATCH (n) WHERE n.qualified_name STARTS WITH 'machine:' RETURN n.qualified_name",
    );
    assert!(
        qn.contains(&format!("machine:{noyau}")),
        "la machine est nommée : {qn}"
    );
    for base in ["kb-a.obrain", "kb-b.obrain"] {
        assert!(
            qn.contains(&format!("machine:{noyau}#{base}")),
            "l'instance « {base} » manque — c'est pourtant son store qui la nomme : {qn}"
        );
    }

    // ── 5. Le store servi est un FAIT du nœud, pas une convention ──────────
    let details = interroger(
        &store_a,
        cache.path(),
        "MATCH (m:Machine) WHERE m.qn STARTS WITH 'machine:' AND m.store_servi <> '' RETURN m.qn, m.store_servi",
    );
    assert!(
        details.contains(&store_a.display().to_string()),
        "{details}"
    );
    assert!(
        details.contains(&store_b.display().to_string()),
        "{details}"
    );

    // ── 6. **AUCUNE CAPACITÉ INVENTÉE** ────────────────────────────────────
    //    Rien ne s'est attaché ici : le nœud doit porter une liste VIDE, et
    //    aucun nœud ne doit porter de capacité non vide. C'est la vérification
    //    qui compte : une machine annoncée sans qu'on sache ce qu'elle sait
    //    faire doit le dire, pas hériter d'un catalogue.
    let vides = interroger(
        &store_a,
        cache.path(),
        "MATCH (m:Machine) WHERE m.capacites_annoncees = '' RETURN m.qn",
    );
    assert!(
        vides.contains(&format!("machine:{noyau}")),
        "la machine sans selfd est là : {vides}"
    );
    let non_vides = interroger(
        &store_a,
        cache.path(),
        "MATCH (m:Machine) WHERE m.capacites_annoncees <> '' RETURN m.qn",
    );
    assert!(
        non_vides.contains("[]"),
        "un nœud annonce des capacités que personne n'a annoncées : {non_vides}"
    );

    // ── 7. Les faits de RATTACHEMENT ne sont pas écrits du tout ────────────
    //    Un zéro écrit se lirait comme une date (1er janvier 1970) : « aucun
    //    selfd ne s'est attaché ici » doit être une absence, pas un zéro.
    let attaches = interroger(
        &store_a,
        cache.path(),
        "MATCH (m:Machine) WHERE m.qn STARTS WITH 'machine:' AND m.derniere_presence_ms IS NOT NULL RETURN m.qn",
    );
    assert!(
        attaches.contains("[]"),
        "aucun rattachement n'a eu lieu : le nœud ne doit porter aucune date d'attache : {attaches}"
    );

    // ── 8. Le cache vivant n'a pas été touché ──────────────────────────────
    let vivant = Path::new("/tmp/obrain-viz-cache/machines");
    let propres = std::fs::read_dir(vivant)
        .map(|l| {
            l.flatten()
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter(|t| t.contains(&cache.path().display().to_string()))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(
        propres, 0,
        "le cache de test a fui dans le cache vivant {vivant:?}"
    );
}

// ============================================================================
// (2) UNE JOURNALISATION IMPOSSIBLE NE FAIT PAS ÉCHOUER LE HARNESS
// ============================================================================

/// Même contrat que le journal d'activation et celui de la mémoire : la
/// journalisation est **best-effort assumée**. Un cache en lecture seule, un
/// disque plein, une variable qui pointe sur un fichier — aucun de ces cas ne
/// doit empêcher un moteur de servir sa base.
///
/// Mais le dépôt ne laisse pas l'échec muet (« le silence ressemble à une
/// panne ») : la sortie d'erreur doit le DIRE. Ici le journal est rendu
/// impossible en plaçant un FICHIER là où il faudrait un répertoire — cas réel
/// d'une variable mal posée, et impossible à contourner par des droits.
#[test]
fn une_journalisation_impossible_ne_fait_pas_echouer_le_harness() {
    let cache = tempfile::tempdir().expect("tempdir cache");
    let obstacle = cache.path().join("pas-un-repertoire");
    std::fs::write(&obstacle, b"un fichier, pas un dossier").unwrap();
    let (store, td) = store_avec_session("kb-obstacle", "session-obstacle");
    let mut h = lancer(store.clone(), &obstacle, None, td);

    let (code, corps) = requete(&h.addr, "/api/harness/health");
    assert_eq!(
        code, 200,
        "le harness doit servir MALGRÉ la journalisation perdue : {corps}"
    );
    assert!(corps.contains("\"status\":\"ok\""), "{corps}");
    let (sc, sessions) = requete(&h.addr, "/api/harness/sessions?limit=50");
    assert_eq!(sc, 200, "{sessions}");
    assert!(
        sessions.contains("session-obstacle"),
        "et il sert vraiment son store : {sessions}"
    );

    h.arreter();
    let err = h.stderr();
    assert!(
        err.contains("NON journalisee") || err.contains("NON journalisée"),
        "l'échec de journalisation doit REMONTER, il ne doit pas être silencieux : {err}"
    );

    // Et la passe ne doit pas inventer de machine pour un moteur qui n'a rien pu
    // déclarer : « rien à appliquer » est un fait, pas une panne.
    let passe = obrain(
        &[
            "machines",
            "--store",
            &store.display().to_string(),
            "--appliquer",
        ],
        &obstacle,
    );
    assert!(
        passe.status.success(),
        "{}",
        String::from_utf8_lossy(&passe.stderr)
    );
    let rapport: serde_json::Value = serde_json::from_slice(&passe.stdout).expect("rapport JSON");
    assert_eq!(rapport["connues"], 0, "aucun fait à appliquer : {rapport}");
    assert_eq!(rapport["machines"], 0, "{rapport}");
}
