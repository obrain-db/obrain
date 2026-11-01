//! T8 harness natif (2026-09-15) — CONTRAT du Self par utilisateur.
//!
//! Prouve que le mecanisme memoire existant, route sur `self:<accountId>` avec
//! une base du Self DEDIEE au compte, constitue un Self par utilisateur sans
//! divergence du modele : journal (extraction auto simulee) -> memoire
//! --appliquer -> base du compte porte Self/Note/Goal/SelfState, idempotent.
//! La base dediee est un store SEPARE (verrou exclusif propre) : ecrire dedans
//! ne touche jamais le runtime — d ou le decouplage de la bascule T6.

use std::path::Path;
use std::process::Command;

fn obrain() -> Command {
    Command::new(env!("CARGO_BIN_EXE_obrain"))
}

fn query_json(base: &Path, cache: &Path, q: &str) -> serde_json::Value {
    let out = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", cache)
        .arg("query").arg(base).arg(q).arg("--format").arg("json")
        .output().expect("query");
    let txt = String::from_utf8_lossy(&out.stdout);
    // le stdout peut contenir des lignes WARN avant le JSON
    let debut = txt.find(['[', '{']).expect("json dans la sortie query");
    serde_json::from_str(&txt[debut..]).expect("parse json query")
}

fn rows(v: &serde_json::Value) -> usize {
    match v {
        serde_json::Value::Object(o) => o.get("rows").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0),
        serde_json::Value::Array(a) => a.len(),
        _ => 0,
    }
}

#[test]
fn self_par_utilisateur_cycle_complet_base_dediee() {
    let td = tempfile::tempdir().unwrap();
    let cache = td.path().join("cache");      // journal isole (OBRAIN_VIZ_CACHE_DIR)
    std::fs::create_dir_all(cache.join("memoire")).unwrap();
    let acc = "6fe5a183-test";
    let self_qn = format!("self:{acc}");
    let base = td.path().join(format!("self-{acc}.obrain"));

    // 1. base dediee du compte
    assert!(obrain().arg("init").arg(&base).output().unwrap().status.success());

    // 2. chemin du journal du compte (demande au binaire) puis extraction simulee
    let rapport = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", &cache)
        .arg("memoire").arg("--self").arg(&self_qn)
        .output().expect("memoire --self");
    let txt = String::from_utf8_lossy(&rapport.stdout);
    let d: serde_json::Value = serde_json::from_str(txt.trim()).expect("rapport json");
    let journal = d["journal"].as_str().expect("journal path").to_string();

    let lignes = [
        format!(r#"{{"t":1,"self":"{self_qn}","provenance":"extraction:auto","intention":{{"kind":"note","texte":"Prefere le francais concis","sujet":"preference-style"}}}}"#),
        format!(r#"{{"t":2,"self":"{self_qn}","provenance":"extraction:auto","intention":{{"kind":"objectif","nom":"harness-natif","texte":"harness Rust natif"}}}}"#),
        format!(r#"{{"t":3,"self":"{self_qn}","provenance":"extraction:auto","intention":{{"kind":"etat-self","epithet":"constructeur","objectif":"self par user","prochaine_action":"prouver","note":"T8"}}}}"#),
    ];
    std::fs::write(&journal, lignes.join("\n") + "\n").unwrap();

    // 3. application dans la base du compte
    let appl = obrain()
        .env("OBRAIN_VIZ_CACHE_DIR", &cache)
        .arg("memoire").arg("--store").arg(&base).arg("--self").arg(&self_qn).arg("--appliquer")
        .output().expect("appliquer");
    assert!(appl.status.success(), "appliquer: {}", String::from_utf8_lossy(&appl.stderr));

    // 4. la base du compte porte le Self et ses membres
    assert_eq!(rows(&query_json(&base, &cache, &format!("MATCH (s:Self {{qn: '{self_qn}'}}) RETURN s.qn"))), 1, "Self du compte");
    assert_eq!(rows(&query_json(&base, &cache, "MATCH (:Self)-[:HAS_NOTE]->(n:Note) RETURN n.sujet")), 1, "profil (note)");
    assert_eq!(rows(&query_json(&base, &cache, "MATCH (:Self)-[:HAS_GOAL]->(g:Goal) RETURN g.nom")), 1, "objectif");
    assert_eq!(rows(&query_json(&base, &cache, "MATCH (:Self)-[:HAS_STATE]->(st:SelfState) RETURN st.version")), 1, "etat versionne");

    // 5. idempotence : re-appliquer la MEME note ne duplique pas
    std::fs::write(&journal, &lignes[0]).unwrap();
    let _ = obrain().env("OBRAIN_VIZ_CACHE_DIR", &cache)
        .arg("memoire").arg("--store").arg(&base).arg("--self").arg(&self_qn).arg("--appliquer")
        .output().expect("appliquer 2");
    assert_eq!(rows(&query_json(&base, &cache, "MATCH (n:Note) RETURN n.sujet")), 1, "note non dupliquee (MERGE par qn)");
}
