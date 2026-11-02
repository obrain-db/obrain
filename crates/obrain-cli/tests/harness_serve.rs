//! T1 harness natif (2026-09-15) — contrat d integration du daemon.
//! Lance le BINAIRE reel sur un port ephemere avec un store temporaire :
//! health, contrat sessions (pagine recent-d-abord), et arret propre.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

fn port_libre() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn get(addr: &str, chemin: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("connect");
    write!(
        s,
        "GET {chemin} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let status: u16 = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
    let corps = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, corps)
}

#[test]
fn harness_serve_health_et_sessions() {
    // 1. store temporaire peuple via le BINAIRE (init + CREATE) : aucun verrou
    // in-process ne subsiste quand le daemon ouvrira le store.
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-harness");
    let init = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("init")
        .arg(&store)
        .output()
        .expect("init");
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    for (qn, cwd, ts) in [
        ("session-a", "/tmp/a", 100),
        ("session-b", "/tmp/b", 300),
        ("session-c", "/tmp/c", 200),
    ] {
        let q =
            format!("CREATE (:Session {{qualified_name: '{qn}', cwd: '{cwd}', createdAt: {ts}}})");
        let o = Command::new(env!("CARGO_BIN_EXE_obrain"))
            .arg("query")
            .arg(&store)
            .arg(&q)
            .output()
            .expect("query create");
        assert!(
            o.status.success(),
            "create {qn}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    // 2. daemon sur port ephemere
    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    let mut enfant = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("harness-serve")
        .arg(&store)
        .arg("--addr")
        .arg(&addr)
        // ── CACHE ISOLÉ ────────────────────────────────────────────────────
        // Le harness journalise sa déclaration de machine au démarrage : sans
        // cette variable, ce test écrirait un fait `machine:` — pour un store de
        // test — dans le cache VIVANT de la machine, et une passe
        // `machines --appliquer` le ferait entrer dans le graphe. Un test ne
        // laisse pas de trace dans l'état partagé.
        .env("OBRAIN_VIZ_CACHE_DIR", td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn harness-serve");

    // 3. readiness bornee
    let fin = Instant::now() + Duration::from_secs(20);
    let mut pret = false;
    while Instant::now() < fin {
        if TcpStream::connect(&addr).is_ok() {
            pret = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !pret {
        let _ = enfant.kill();
        let sortie = enfant.wait_with_output().unwrap();
        panic!(
            "daemon jamais pret sur {addr}\nstderr:\n{}",
            String::from_utf8_lossy(&sortie.stderr)
        );
    }

    // 4. health
    let (st, corps) = get(&addr, "/api/harness/health");
    assert_eq!(st, 200, "health: {corps}");
    assert!(
        corps.contains("\"mode\":\"ro\""),
        "mode ro attendu: {corps}"
    );

    // 5. sessions : contrat pagine recent-d-abord
    let (st, corps) = get(&addr, "/api/harness/sessions?limit=2");
    assert_eq!(st, 200, "sessions: {corps}");
    let v: serde_json::Value = serde_json::from_str(&corps).unwrap();
    let sess = v["sessions"].as_array().expect("sessions[]");
    assert_eq!(sess.len(), 2, "limit=2");
    assert_eq!(sess[0]["qn"], "session-b", "plus recente d abord (300)");
    assert_eq!(sess[1]["qn"], "session-c", "puis 200");

    let (st, corps) = get(&addr, "/api/harness/sessions?limit=2&offset=2");
    assert_eq!(st, 200);
    let v: serde_json::Value = serde_json::from_str(&corps).unwrap();
    let sess = v["sessions"].as_array().unwrap();
    assert_eq!(sess.len(), 1, "offset=2 -> reste 1");
    assert_eq!(sess[0]["qn"], "session-a", "la plus ancienne (100)");

    // 6. route inconnue
    let (st, _) = get(&addr, "/api/harness/inconnu");
    assert_eq!(st, 404);

    let _ = enfant.kill();
    let _ = enfant.wait();
}
