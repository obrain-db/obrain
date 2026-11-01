//! T2 harness natif (2026-09-15) — route /api/harness/context (injection CH6).
//! Verifie que le POST in-process rend le meme contrat que l outil context_build
//! (champs engrams/goals/summaries/weave) sans respawn de bridge.

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
    ).unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let status: u16 = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
    let corps = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, corps)
}

#[test]
fn harness_context_injection() {
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-ctx");
    assert!(Command::new(env!("CARGO_BIN_EXE_obrain")).arg("init").arg(&store).output().unwrap().status.success());
    // une session avec un goal (pour peupler l injection)
    for q in [
        "CREATE (:Session {qualified_name: 'session-ctx', cwd: '/tmp', createdAt: 1})",
        "CREATE (:Goal {qualified_name: 'goal-1', text: 'objectif de test'})",
        "MATCH (s:Session {qualified_name: 'session-ctx'}), (g:Goal {qualified_name: 'goal-1'}) MERGE (s)-[:HAS_GOAL]->(g)",
    ] {
        assert!(Command::new(env!("CARGO_BIN_EXE_obrain")).arg("query").arg(&store).arg(q).output().unwrap().status.success(), "seed: {q}");
    }

    let port = port_libre();
    let addr = format!("127.0.0.1:{port}");
    let mut enfant = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("harness-serve").arg(&store).arg("--addr").arg(&addr)
        // ── CACHE ISOLÉ : LE HARNESS JOURNALISE SA MACHINE AU DÉMARRAGE ──────
        // Sans cette variable, ce test écrivait une déclaration `machine:` dans
        // le REGISTRE VIVANT de la machine (/tmp/obrain-viz-cache/machines), pour
        // un store de test situé sous /tmp/.tmpXXXX — un chemin qui n'existe plus
        // à la fin du test. Constaté par la porte d'atelier (famille 3), qui a
        // nommé les journaux fautifs ; les autres tests qui lancent un moteur
        // isolent déjà leur cache, celui-ci ne le faisait pas.
        //
        // Ce n'est pas une trace inoffensive : une passe `machines --appliquer`
        // avec le cache par défaut inscrirait dans une VRAIE base des nœuds
        // `machine:` pour des machines de test — « annoncer est décider » violé
        // par un effet de bord de test.
        .env("OBRAIN_VIZ_CACHE_DIR", td.path())
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped())
        .spawn().unwrap();
    let fin = Instant::now() + Duration::from_secs(20);
    while Instant::now() < fin && TcpStream::connect(&addr).is_err() {
        std::thread::sleep(Duration::from_millis(100));
    }

    let (st, corps) = post(&addr, "/api/harness/context", r#"{"session_id":"session-ctx","prompt":"objectif"}"#);
    assert_eq!(st, 200, "context: {corps}");
    let v: serde_json::Value = serde_json::from_str(&corps).unwrap();
    // contrat CH6 : les sections attendues sont presentes
    for champ in ["goals", "summaries", "weave", "engrams"] {
        assert!(v.get(champ).is_some(), "section '{champ}' absente: {corps}");
    }
    // le goal seme doit remonter
    let goals = v["goals"].as_array().expect("goals[]");
    assert!(!goals.is_empty(), "le goal seme doit apparaitre: {corps}");

    let _ = enfant.kill();
    let _ = enfant.wait();
}
