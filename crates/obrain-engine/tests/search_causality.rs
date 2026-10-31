#![cfg(feature = "algos")]
//! `obrain.search_causes` / `obrain.search_effects` — spread directionnel
//! causal (BFS amorti) : (1) chaîne A→B→C directionnelle ; (2) étoile ;
//! (3) cycle → terminaison ; (4) depth/limit ; (5) CALL ; (6) read-only.

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;

fn now_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .expect("horloge")
        .as_millis() as u64
}

fn e(db: &ObrainDB, a: u64, b: u64) {
    db.create_edge(NodeId(a), NodeId(b), "T");
}

#[test]
fn search_effects_suit_la_direction_des_aretes() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-caus-chain");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let a = db.create_node(&["L"]).0;
    let b = db.create_node(&["L"]).0;
    let c = db.create_node(&["L"]).0;
    e(&db, a, b);
    e(&db, b, c);
    let fx = db.search_effects(a, 10, 3).expect("effects(A)");
    assert_eq!(fx[0].node_id, b, "effet direct d'abord");
    assert_eq!(fx[0].level, 1);
    assert!(
        (fx[0].score - 0.5).abs() < 1e-9,
        "score γ=0.5, got {}",
        fx[0].score
    );
    assert_eq!(fx[1].node_id, c);
    assert_eq!(fx[1].level, 2);
    assert!(
        (fx[1].score - 0.25).abs() < 1e-9,
        "score γ²=0.25, got {}",
        fx[1].score
    );
    // Pas de retour arrière : causes(A) est vide (rien n'entre dans A).
    let cs = db.search_causes(a, 10, 3).expect("causes(A)");
    assert!(cs.is_empty(), "A n'a pas de conducteur: {cs:?}");
}

#[test]
fn search_causes_remonte_la_direction() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-caus-up");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let a = db.create_node(&["L"]).0;
    let b = db.create_node(&["L"]).0;
    let c = db.create_node(&["L"]).0;
    e(&db, a, b);
    e(&db, b, c);
    let cs = db.search_causes(c, 10, 3).expect("causes(C)");
    assert_eq!(cs[0].node_id, b, "conducteur direct d'abord");
    assert_eq!(cs[1].node_id, a, "conducteur indirect ensuite");
    assert!((cs[0].score - 0.5).abs() < 1e-9);
    assert!((cs[1].score - 0.25).abs() < 1e-9);
}

#[test]
fn search_causes_effects_etoile_et_cycle_terminent() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-caus-star");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    // Étoile : centre → 3 feuilles.
    let centre = db.create_node(&["L"]).0;
    let leaves: Vec<u64> = (0..3).map(|_| db.create_node(&["L"]).0).collect();
    for l in &leaves {
        e(&db, centre, *l);
    }
    let fx = db.search_effects(centre, 10, 3).expect("effects");
    assert_eq!(fx.len(), 3);
    for r in &fx {
        assert_eq!(r.level, 1);
        assert!((r.score - 0.5).abs() < 1e-9);
    }
    let cs = db.search_causes(leaves[0], 10, 3).expect("causes");
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].node_id, centre);

    // Cycle A→B→A : terminaison garantie, pas de boucle infinie.
    let x = db.create_node(&["L"]).0;
    let y = db.create_node(&["L"]).0;
    e(&db, x, y);
    e(&db, y, x);
    let fx2 = db.search_effects(x, 10, 6).expect("cycle effects");
    assert!(!fx2.is_empty());
    // Chaque nœud n'apparaît qu'UNE fois (première découverte).
    let mut seen: Vec<u64> = fx2.iter().map(|r| r.node_id).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), fx2.len(), "pas de doublon: {fx2:?}");
}

#[test]
fn search_causes_effects_depth_et_limite() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-caus-lim");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let a = db.create_node(&["L"]).0;
    let b = db.create_node(&["L"]).0;
    let c = db.create_node(&["L"]).0;
    let d = db.create_node(&["L"]).0;
    e(&db, a, b);
    e(&db, b, c);
    e(&db, c, d);
    // depth=1 : seulement le voisin direct.
    let fx = db.search_effects(a, 10, 1).expect("depth1");
    assert_eq!(fx.len(), 1);
    assert_eq!(fx[0].node_id, b);
    // limit=1.
    let fx = db.search_effects(a, 1, 3).expect("limit");
    assert_eq!(fx.len(), 1);
    assert_eq!(fx[0].node_id, b);
    // depth=3 : jusqu'à d.
    let fx = db.search_effects(a, 10, 3).expect("depth3");
    assert_eq!(fx.len(), 3);
    let _ = now_ms();
}

#[test]
fn call_search_causes_effects_exposent_le_classement() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-caus-call");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let a = db.create_node(&["L"]).0;
    let b = db.create_node(&["L"]).0;
    let c = db.create_node(&["L"]).0;
    e(&db, a, b);
    e(&db, b, c);
    let s = db.session();
    let r = s
        .execute("CALL obrain.search_effects(1, 5, 3) YIELD node_id, level, score RETURN node_id")
        .expect("CALL search_effects");
    assert_eq!(
        r.rows.len(),
        2,
        "effets de 1 sur la chaîne 1→2→3 : 2 nœuds, got {:?}",
        r.rows
    );
    let r2 = s
        .execute("CALL obrain.search_causes(3, 5, 3) YIELD node_id RETURN node_id")
        .expect("CALL search_causes");
    assert_eq!(r2.rows.len(), 2, "causes de 3 : 1 et 2, got {:?}", r2.rows);
}

#[test]
fn search_causes_effects_lecture_pure_et_noeud_invalide() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-caus-ro");
    {
        let w = ObrainDB::open_substrate(&path).expect("open rw");
        let a = w.create_node(&["L"]).0;
        let b = w.create_node(&["L"]).0;
        e(&w, a, b);
        let rows = w.search_effects(a, 10, 3).expect("rw ok");
        assert_eq!(rows.len(), 1);
        // Slot 0 invalide → erreur explicite.
        assert!(w.search_effects(0, 10, 3).is_err());
    }
    let db_ro = ObrainDB::open_read_only(&path).expect("open ro");
    let rows = db_ro.search_effects(1, 10, 3).expect("ro ok");
    assert!(!rows.is_empty(), "read-only rend le spread");
    let s = db_ro.session();
    let r = s
        .execute("CALL obrain.search_effects(1, 5, 2) YIELD node_id RETURN node_id")
        .expect("CALL read-only");
    assert!(!r.rows.is_empty(), "CALL read-only rend des lignes");
}
