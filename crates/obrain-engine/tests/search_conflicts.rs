#![cfg(all(feature = "cdc", feature = "algos"))]
//! `obrain.search_conflicts` — dissonance temporelle d'états (A→B→A…) :
//! (1) méthode `ObrainDB::search_conflicts` ; (2) procédure CALL ;
//! (3) lecture pure (read-only) ; (4) monotonie → 0 ; (5) fenêtre vide.

use std::time::Duration;
use web_time::SystemTime;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("horloge")
        .as_millis() as u64
}

#[test]
fn search_conflicts_compte_les_reversals_aba() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let n = db.create_node(&["L"]);
    // État initial distinct (0), puis A→B→A→B→A : 3 reversals attendus.
    db.set_node_property(n, "x", Value::Int64(0));
    for v in [1i64, 2, 1, 2, 1] {
        db.set_node_property(n, "x", Value::Int64(v));
    }
    let rows = db.search_conflicts(0, 100).expect("conflicts");
    assert_eq!(rows.len(), 1, "un seul couple conflictuel: {rows:?}");
    assert_eq!(rows[0].node_id, n.0);
    assert_eq!(rows[0].property, "x");
    assert_eq!(rows[0].reversals, 3, "A,B,A,B,A → 3 reversals");
}

#[test]
fn search_conflicts_monotone_rend_zero() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf-mono");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let n = db.create_node(&["L"]);
    // Valeur qui change SANS jamais revenir → aucun reversal.
    for v in [1i64, 2, 3, 4, 5] {
        db.set_node_property(n, "x", Value::Int64(v));
    }
    let rows = db.search_conflicts(0, 100).expect("conflicts");
    assert!(
        rows.is_empty(),
        "monotone = pas de dissonance, got {rows:?}"
    );
}

#[test]
fn search_conflicts_classe_plusieurs_couples() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf-ord");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let a = db.create_node(&["L"]);
    let b = db.create_node(&["L"]);
    // a.x oscille fort (3 reversals) ; b.y oscille peu (1) ; b.z monotone (0).
    db.set_node_property(a, "x", Value::Int64(0));
    db.set_node_property(b, "y", Value::Int64(0));
    for v in [1i64, 2, 1, 2, 1] {
        db.set_node_property(a, "x", Value::Int64(v));
    }
    for v in [10i64, 20, 10] {
        db.set_node_property(b, "y", Value::Int64(v));
    }
    for v in [100i64, 200, 300] {
        db.set_node_property(b, "z", Value::Int64(v));
    }
    let rows = db.search_conflicts(0, 100).expect("conflicts");
    assert_eq!(rows.len(), 2, "deux couples conflictuels: {rows:?}");
    // Tri par reversals décroissant.
    assert_eq!(rows[0].node_id, a.0);
    assert_eq!(rows[0].reversals, 3);
    assert_eq!(rows[1].node_id, b.0);
    assert_eq!(rows[1].property, "y");
    assert_eq!(rows[1].reversals, 1);
    // limit respectée.
    let top = db.search_conflicts(0, 1).expect("limit");
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].node_id, a.0);
}

#[test]
fn call_search_conflicts_expose_le_classement() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf-call");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let n = db.create_node(&["L"]);
    db.set_node_property(n, "x", Value::Int64(0));
    for v in [1i64, 2, 1, 2, 1] {
        db.set_node_property(n, "x", Value::Int64(v));
    }
    let s = db.session();
    let r = s
        .execute("CALL obrain.search_conflicts(0, 10) YIELD node_id, property, reversals RETURN reversals")
        .expect("CALL search_conflicts");
    assert!(!r.rows.is_empty(), "CALL rend des lignes");
    assert!(
        matches!(r.rows[0][0], Value::Int64(3)),
        "3 reversals: {:?}",
        r.rows[0]
    );
}

#[test]
fn search_conflicts_est_une_lecture_pure() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf-ro");
    {
        let w = ObrainDB::open_substrate(&path).expect("open rw");
        let n = w.create_node(&["L"]);
        w.set_node_property(n, "x", Value::Int64(0));
        for v in [1i64, 2, 1] {
            w.set_node_property(n, "x", Value::Int64(v));
        }
        let rows = w.search_conflicts(0, 100).expect("rw ok");
        assert_eq!(rows.len(), 1);
    }
    let db_ro = ObrainDB::open_read_only(&path).expect("open ro");
    let rows = db_ro.search_conflicts(0, 100).expect("ro ok");
    assert!(
        !rows.is_empty(),
        "read-only : la fenêtre persistée est relue"
    );
    assert_eq!(rows[0].reversals, 1);
    let s = db_ro.session();
    let r = s
        .execute("CALL obrain.search_conflicts(0, 5) YIELD node_id RETURN node_id")
        .expect("CALL read-only");
    assert!(!r.rows.is_empty(), "CALL read-only rend des lignes");
}

/// PROPRIÉTÉ : oracle naïf indépendant du moteur — pour une séquence de
/// valeurs aléatoires par (nœud, propriété), le nombre de reversals détecté
/// par `search_conflicts` doit être identique au comptage direct A→B→A.
#[test]
fn search_conflicts_concorde_avec_l_oracle_naif() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf-prop");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    // Oracle naïf (indépendant de l'implémentation) sur les valeurs distinctes
    // consécutives : reversal = retour à l'avant-dernier état.
    fn naive(seq: &[i64]) -> usize {
        let mut rev = 0usize;
        let mut last: Option<i64> = None;
        let mut before: Option<i64> = None;
        for v in seq {
            if Some(*v) == last {
                continue;
            }
            if before == Some(*v) {
                rev += 1;
            }
            before = last;
            last = Some(*v);
        }
        rev
    }
    // RNG déterministe (xorshift).
    let mut st: u64 = 0xdead_beef_cafe_1234;
    let mut rnd = || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        st
    };
    let n1 = db.create_node(&["L"]);
    let n2 = db.create_node(&["L"]);
    for graine in 0..30u64 {
        let mut seq: Vec<i64> = Vec::new();
        for _ in 0..(3 + (rnd() % 25)) {
            seq.push((rnd() % 5) as i64);
        }
        // Valeur initiale distincte avant la séquence (l'état « absent » n'est
        // pas un reversal, comme dans les tests existants).
        let node = if graine % 2 == 0 { n1 } else { n2 };
        // Clé UNIQUE par graine : chaque graine est une chaîne isolée (évite
        // le cumul inter-graines du moteur sur la même (nœud, propriété)).
        let key = format!("k{graine}");
        db.set_node_property(node, &key, Value::Int64(99 + graine as i64));
        for v in &seq {
            db.set_node_property(node, &key, Value::Int64(*v));
        }
        // Oracle sur la chaîne COMPLÈTE (l'état initial 99 participe à la
        // parité de la chaîne des valeurs distinctes du moteur).
        let mut full: Vec<i64> = Vec::with_capacity(seq.len() + 1);
        full.push(99 + graine as i64);
        full.extend_from_slice(&seq);
        let expected = naive(&full);
        let rows = db.search_conflicts(0, 10_000).expect("conflicts");
        let found = rows
            .iter()
            .find(|r| r.node_id == node.0 && r.property == key)
            .map_or(0, |r| r.reversals);
        assert_eq!(
            found, expected,
            "graine {graine} (seq {seq:?}) : oracle {expected} vs moteur {found}"
        );
    }
}

#[test]
fn search_conflicts_fenetre_sans_update_rend_vide() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-conf-vide");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let n = db.create_node(&["L"]);
    for v in [1i64, 2, 1] {
        db.set_node_property(n, "x", Value::Int64(v));
    }
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    let rows = db.search_conflicts(since, 100).expect("vide");
    assert!(rows.is_empty(), "aucun update après since → vide");
}
