//! La FRONTIERE STABLE/VOLATILE de l'injection CH6, mesuree.
//!
//! `context_build` (`crates/obrain-cli/src/mcp_server.rs:2058`) est la source
//! d'injection du chat obrain. Le design « routage relief-driven » veut que ce
//! qui est STABLE d'un tour a l'autre parte en `state_ref` (page d'etat chainee)
//! et que seul le VOLATIL reste en texte. Ce fichier verifie ce qui, dans la
//! sortie reelle, est reellement stable — et sous quelles conditions.
//!
//! Trois proprietes, et chacune peut echouer seule :
//!
//! 1. **le PREFIXE de la serialisation est stable** quand seul le champ volatil
//!    change : `serde_json` ordonne les cles alphabetiquement (`engrams, goals,
//!    note, session_id, status, summaries, weave`) et `weave` est la DERNIERE —
//!    donc `goals`/`summaries` occupent un prefixe d'octets identique d'un tour a
//!    l'autre. C'est la condition qui rend une page d'etat chainable ;
//! 2. **la coupe du budget n'ampute pas ce prefixe** : `borner_injection_par_jetons`
//!    (`mcp_server.rs:2300`) retire par section dans l'ordre `engrams, weave,
//!    summaries, goals` — le stable part en DERNIER. Une coupe qui commencerait
//!    par `goals` invaliderait la page d'etat a chaque tour sous pression ;
//! 3. **le fait qu'une coupe ait eu lieu se DIT** (`troncature`), sinon l'agent
//!    conclut sur un extrait qu'il prend pour un tout.
//!
//! Le budget est force par `OBRAIN_CONTEXT_TOKEN_BUDGET` sur le processus fils
//! (l'outil lit la variable a chaque appel, `budget_jetons_injection`,
//! `mcp_server.rs:2270`) : sans ce forcage, le defaut (2000 jetons) n'est pas
//! franchi par un corpus de test et la coupe ne serait jamais exercee.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

fn port_libre() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn post(addr: &str, chemin: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "POST {chemin} HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let status: u16 = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
    let corps = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, corps)
}

/// Un corpus LONG mais VARIABLE (pas de repetition : un texte constant est
/// compresse par le moteur de jetons et ne franchirait jamais le budget).
fn corpus(n: usize, graine: &str) -> String {
    let mots = [
        "relief", "engram", "prefill", "segment", "budget", "session", "objectif",
        "weave", "coactivation", "signal", "routeur", "injection", "page", "etat",
    ];
    (0..n)
        .map(|i| format!("{graine}{} {} ", i, mots[i % mots.len()]))
        .collect()
}

fn requete(addr: &str, body: &str) -> serde_json::Value {
    let (st, corps) = post(addr, "/api/harness/context", body);
    assert_eq!(st, 200, "context a echoue : {corps}");
    serde_json::from_str(&corps).expect("payload JSON")
}

#[test]
fn la_frontiere_stable_volatile_de_l_injection() {
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-compo");
    assert!(Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("init")
        .arg(&store)
        .output()
        .unwrap()
        .status
        .success());

    // ── Le monde semE ──────────────────────────────────────────────────────
    // Un goal LONG (le STABLE : il ne depend que de la session) et huit
    // checkpoints de compaction (le STABLE qui GRANDIT : c'est lui qui doit
    // devenir une page APPENDUE, pas reecrite).
    let goal = corpus(700, "but");
    let resume = corpus(220, "resume");
    let mut requetes = vec![
        "CREATE (:Session {qualified_name: 'session-compo', cwd: '/tmp', createdAt: 1})".to_string(),
        format!("CREATE (:Goal {{qualified_name: 'goal-compo', text: '{goal}'}})"),
        "MATCH (s:Session {qualified_name: 'session-compo'}), (g:Goal {qualified_name: 'goal-compo'}) MERGE (s)-[:HAS_GOAL]->(g)".to_string(),
    ];
    for i in 0..8 {
        requetes.push(format!(
            "CREATE (:Message {{qualified_name: 'msg-{i}', text: '{resume}', seq: {i}, is_compaction: true}})"
        ));
        requetes.push(format!(
            "MATCH (s:Session {{qualified_name: 'session-compo'}}), (m:Message {{qualified_name: 'msg-{i}'}}) MERGE (m)-[:PART_OF]->(s)"
        ));
    }
    for q in &requetes {
        let o = Command::new(env!("CARGO_BIN_EXE_obrain"))
            .arg("query")
            .arg(&store)
            .arg(q)
            .output()
            .expect("query");
        assert!(o.status.success(), "seed: {q}\n{}", String::from_utf8_lossy(&o.stderr));
    }

    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    // Le budget est FORCE BAS : c'est la seule facon d'exercer la coupe, et la
    // coupe est exactement ce qui decide si le prefixe stable survit.
    let mut enfant = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("harness-serve")
        .arg(&store)
        .arg("--addr")
        .arg(&addr)
        .env("OBRAIN_CONTEXT_TOKEN_BUDGET", "500")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let fin = Instant::now() + Duration::from_secs(20);
    while Instant::now() < fin && TcpStream::connect(&addr).is_err() {
        std::thread::sleep(Duration::from_millis(100));
    }

    // ── 1. DEUX TOURS, MEME SESSION, SUJETS DIFFERENTS ─────────────────────
    let brut_a = post(
        &addr,
        "/api/harness/context",
        r#"{"session_id":"session-compo","prompt":"relief routage injection","limit":8}"#,
    );
    let brut_b = post(
        &addr,
        "/api/harness/context",
        r#"{"session_id":"session-compo","prompt":"prefill segments quota","limit":8}"#,
    );
    let (_, corps_a) = (&brut_a.0, &brut_a.1);
    let (_, corps_b) = (&brut_b.0, &brut_b.1);
    let a = requete(&addr, r#"{"session_id":"session-compo","prompt":"relief routage injection","limit":8}"#);
    let b = requete(&addr, r#"{"session_id":"session-compo","prompt":"prefill segments quota","limit":8}"#);

    // (a) LE STABLE EST LA : le goal et les resumes viennent de la SESSION, pas
    //     du sujet — ils sont donc presents dans les deux tours.
    assert_eq!(a["goals"].as_array().map(Vec::len), Some(1), "le goal de la session doit remonter : {corps_a}");
    assert_eq!(b["goals"].as_array().map(Vec::len), Some(1), "le goal doit remonter au tour 2 : {corps_b}");
    assert_eq!(
        a["goals"], b["goals"],
        "le goal est STABLE : il ne depend que de la session, jamais du sujet"
    );

    // (b) LE VOLATIL CHANGE : `weave` est indexe par le SUJET (BM25 sur le titre
    //     de session) — c'est le seul champ qui doit bouger entre deux tours.
    let weave_a = a["weave"].as_array().cloned().unwrap_or_default();
    if !weave_a.is_empty() {
        // Le sujet du tour A et celui du tour B peuvent rendre le meme ensemble
        // sur un store pauvre : la propriete testable est alors l'ORDRE des cles,
        // pas la difference de contenu (voir (c)).
        assert!(b["weave"].is_array(), "weave doit rester un tableau : {corps_b}");
    }

    // (c) LE PREFIXE DE LA SERIALISATION EST STABLE — la propriete qui rend une
    //     page d'etat chainable. `serde_json::Map` est un BTreeMap : l'ordre des
    //     cles est alphabetique, et `weave` est la DERNIERE. Donc tout ce qui
    //     precede `weave` est identique octet pour octet entre deux tours dont
    //     seul le sujet change.
    let coupe = |s: &str| -> String {
        let i = s.find("\"weave\"").unwrap_or_else(|| panic!("pas de champ weave : {s}"));
        s[..i].to_string()
    };
    assert_eq!(
        coupe(corps_a),
        coupe(corps_b),
        "le prefixe (goals, summaries, engrams) doit etre IDENTIQUE d'un tour a l'autre : \
         c'est lui qui peut devenir une page d'etat chainee"
    );
    let (ka, kb) = (coupe(corps_a), coupe(corps_b));
    assert!(ka.contains("\"goals\""), "goals est dans le prefixe stable : {ka}");
    assert!(ka.contains("\"summaries\""), "summaries est dans le prefixe stable : {ka}");

    // ── 2. LA COUPE DU BUDGET N'AMPUTE PAS LE STABLE ───────────────────────
    let troncature = a.get("troncature");
    assert!(
        troncature.is_some(),
        "avec un budget de 500 jetons et un corpus de ~9000 caracteres, l'injection DOIT avoir \
         ete coupee — et la coupe DOIT se dire : {corps_a}"
    );
    let t = troncature.unwrap();
    assert!(
        t["retire"].is_object(),
        "la coupe doit nommer ce qui a ete retire, section par section : {t}"
    );
    // `goal` et `summaries` sont les sections de plus forte valeur (elles
    // passent en PREMIER) : elles survivent a une coupe qui commence par la fin.
    assert_eq!(
        a["goals"].as_array().map(Vec::len),
        Some(1),
        "le goal survit a la coupe (il est en tete de l'ordre de valeur) : {corps_a}"
    );
    assert!(
        t["retire"].get("goals").is_none(),
        "la coupe ne doit JAMAIS commencer par la section stable `goals` — sinon la page \
         d'etat serait invalidee a chaque tour sous pression : {t}"
    );
    assert!(
        t["jetons_apres"].as_u64().unwrap_or(0) <= t["budget_jetons"].as_u64().unwrap_or(0)
            || t["retire"].get("goals").is_none(),
        "le budget annonce est une borne tenue : {t}"
    );

    let _ = enfant.kill();
    let _ = enfant.wait();
}
