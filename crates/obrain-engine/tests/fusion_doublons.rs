//! Fusion des doublons (mission perf, feature 2.2) : derniere-version-
//! gagne, aretes internes ET inter-bases REMONTEES, dry-run sans toucher,
//! backup avant fusion.

#![cfg(feature = "gql")]

use obrain_core::graph::GraphStore;
use obrain_engine::ObrainDB;

fn base_avec_doublons() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Deux doublons du meme id logique + un noeud sain.
    s.execute("CREATE (:Obj {id: \x27dup\x27, updated_at: \x272026-08-10\x27, v: 1})")
        .expect("vieux");
    s.execute("CREATE (:Obj {id: \x27dup\x27, updated_at: \x272026-08-16\x27, v: 2})")
        .expect("recent");
    s.execute("CREATE (:Obj {id: \x27sain\x27, updated_at: \x272026-08-01\x27})")
        .expect("sain");
    // Aretes du PERDANT (le vieux) : sortante, entrante, inter-bases.
    s.execute("CREATE (:Voisin {id: \x27x\x27})").expect("x");
    s.execute("CREATE (:Voisin {id: \x27y\x27})").expect("y");
    s.execute("MATCH (a:Obj {v: 1}), (x:Voisin {id: \x27x\x27}) CREATE (a)-[:VERS]->(x)")
        .expect("sortante");
    s.execute("MATCH (a:Obj {v: 1}), (y:Voisin {id: \x27y\x27}) CREATE (y)-[:DEPUIS]->(a)")
        .expect("entrante");
    s.execute("MATCH (a:Obj {v: 1}) CREATE (a)-[:CITE_EXT]->(ref(\"autre_base\", \"doc_9\"))")
        .expect("externe");
    db
}

#[test]
fn dry_run_compte_sans_toucher() {
    let db = base_avec_doublons();
    let rapport = db
        .fusionner_doublons("Obj", "id", false, None)
        .expect("dry run");
    assert!(!rapport.execute);
    assert_eq!(rapport.groupes.len(), 1, "un seul groupe en doublon");
    assert_eq!(rapport.groupes[0].id_logique, "dup");
    assert_eq!(rapport.perdants, 1);
    assert_eq!(
        db.graph_store().nodes_by_label("Obj").len(),
        3,
        "dry-run : RIEN ne doit avoir ete touche"
    );
}

#[test]
fn la_fusion_garde_le_plus_recent_et_remonte_tout() {
    let db = base_avec_doublons();
    let rapport = db
        .fusionner_doublons("Obj", "id", true, None)
        .expect("fusion");
    assert_eq!(rapport.perdants, 1);
    let s = db.session();
    // 1:1 : plus de doublon, le survivant est le PLUS RECENT.
    let r = s
        .execute("MATCH (n:Obj {id: \x27dup\x27}) RETURN n.v")
        .expect("verif");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], obrain_common::types::Value::Int64(2));
    // Les aretes du perdant ont ete REMONTEES.
    let r = s
        .execute("MATCH (n:Obj {id: \x27dup\x27})-[:VERS]->(x) RETURN x.id")
        .expect("sortante remontee");
    assert_eq!(r.rows.len(), 1);
    let r = s
        .execute("MATCH (y)-[:DEPUIS]->(n:Obj {id: \x27dup\x27}) RETURN y.id")
        .expect("entrante remontee");
    assert_eq!(r.rows.len(), 1);
    let survivant = db
        .graph_store()
        .find_nodes_by_property("id", &obrain_common::types::Value::String("dup".into()));
    let externes = db.graph_store().external_edges_from(survivant[0]);
    assert_eq!(externes.len(), 1, "l arete inter-bases doit etre remontee");
    assert_eq!(externes[0].to_store, "autre_base");
    assert_eq!(rapport.groupes[0].aretes_remontees, 2);
    assert_eq!(rapport.groupes[0].externes_remontees, 1);
}

#[test]
fn le_backup_est_ecrit_avant_la_fusion() {
    let db = base_avec_doublons();
    let tmp = std::env::temp_dir().join(format!("fusion_backup_{}.json", std::process::id()));
    let rapport = db
        .fusionner_doublons("Obj", "id", true, Some(&tmp))
        .expect("fusion avec backup");
    assert_eq!(rapport.backup.as_deref(), Some(tmp.to_str().unwrap()));
    let contenu = std::fs::read_to_string(&tmp).expect("backup lisible");
    assert!(contenu.contains("dup") && contenu.contains("externes"));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn backup_impossible_empeche_la_fusion() {
    let db = base_avec_doublons();
    let impossible = std::path::PathBuf::from("/proc/impossible/backup.json");
    let err = db.fusionner_doublons("Obj", "id", true, Some(&impossible));
    assert!(err.is_err(), "backup impossible => la fusion est REFUSEE");
    assert_eq!(
        db.graph_store().nodes_by_label("Obj").len(),
        3,
        "rien ne doit avoir ete fusionne sans backup"
    );
}
