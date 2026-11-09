//! RFC zorder-superposition — capacité C5 : `CALL obrain.window_2d(...)` en
//! syntaxe GQL.
//!
//! Vérifie que la fenêtre 2D par communauté (plan centrality × degree),
//! jusqu'alors réservée à l'API moteur (`ObrainDB::window_query_2d`) et au
//! binding NAPI (`windowQuery2d`), est accessible comme procédure CALL :
//! résultats identiques à la méthode moteur, classée LECTURE (exécutable sur
//! session read-only), et erreur explicite sur base non adossée au substrate.

#![cfg(feature = "algos")]

use obrain_engine::ObrainDB;
use obrain_engine::config::Config;

/// Crée un nœud de `cid` avec une centralité cible (Q0.16 approchée via f32)
/// et rend son id u64 (slot substrate).
fn create_with_centrality(db: &ObrainDB, cid: u32, q16: u16) -> u64 {
    let store = db.substrate_handle().expect("base substrate");
    let node = store.create_node_in_community(&["L"], cid);
    store
        .update_centrality_batch_f32(vec![(node, q16 as f32 / u16::MAX as f32)])
        .expect("mise a jour de centralite");
    node.0
}

/// Pose l'énergie d'un nœud (colonne inline Q1.15).
fn set_energy(db: &ObrainDB, node: u64, energy: f32) {
    let store = db.substrate_handle().expect("base substrate");
    store
        .set_node_energy_f32(obrain_common::types::NodeId(node), energy)
        .expect("pose energie");
}

/// Extrait les valeurs entières d'une colonne de résultat GQL.
fn int_column(rows: Vec<Vec<obrain_common::types::Value>>, col: usize) -> Vec<i64> {
    let mut out: Vec<i64> = rows
        .iter()
        .map(|r| match r.get(col) {
            Some(obrain_common::types::Value::Int64(v)) => *v,
            other => panic!("colonne {col} inattendue: {other:?}"),
        })
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn call_window_2d_egal_scan_moteur() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).expect("open_substrate");

    // Communauté 7 : deux centralités écartées. Communauté 8 : une seule.
    let mut c7: Vec<u64> = vec![
        create_with_centrality(&db, 7, 1000),
        create_with_centrality(&db, 7, 60000),
    ];
    let mut c8: Vec<u64> = vec![create_with_centrality(&db, 8, 30000)];
    c7.sort_unstable();
    c8.sort_unstable();

    let session = db.session();

    // Fenêtre ouverte — la procédure CALL doit rendre exactement la méthode.
    let r = session
        .execute("CALL obrain.window_2d(7, 0, 65535, 0, 4294967295) YIELD node_id RETURN node_id")
        .expect("call window ouverte cid=7");
    let got = int_column(r.rows, 0);
    let want: Vec<i64> = c7.iter().map(|&x| x as i64).collect();
    assert_eq!(
        got, want,
        "CALL window communauté 7, fenêtre ouverte == scan moteur"
    );

    // Alias court `window_2d` (résolution par dernier segment) sur cid=8.
    let r = session
        .execute("CALL window_2d(8, 0, 65535, 0, 4294967295) YIELD node_id RETURN node_id")
        .expect("call window alias court cid=8");
    let got = int_column(r.rows, 0);
    let want: Vec<i64> = c8.iter().map(|&x| x as i64).collect();
    assert_eq!(got, want, "CALL window_2d (alias court) communauté 8");

    // Fenêtre de centralité étroite : seul le nœud à haute centralité répond.
    let r = session
        .execute(
            "CALL obrain.window_2d(7, 50000, 65535, 0, 4294967295) YIELD node_id RETURN node_id",
        )
        .expect("call window haute centralite");
    let got = int_column(r.rows, 0);
    assert_eq!(
        got,
        vec![c7[1] as i64],
        "communauté 7, haute centralité seulement"
    );

    // Communauté inconnue : vide, sans erreur.
    let r = session
        .execute("CALL obrain.window_2d(42, 0, 65535, 0, 4294967295) YIELD node_id RETURN node_id")
        .expect("communaute inconnue");
    assert!(r.rows.is_empty(), "communauté 42 n'existe pas");
}

#[test]
fn call_window_2d_classe_lecture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    {
        // Base RW : créer le nœud puis laisser le scope fermer la base
        // (substrate : le drop libère le verrou d'écriture).
        let db = ObrainDB::open_substrate(&path).expect("open_substrate");
        create_with_centrality(&db, 3, 20000);
    }
    // Réouverture READ-ONLY (verrou partagé) : la fenêtre est une pure
    // lecture (même exigence que `spread`) — si la classification échoue,
    // la session read-only refuse avec Transaction(ReadOnly).
    let db = ObrainDB::open_read_only(&path).expect("open_read_only");
    let session = db.session();
    let r = session
        .execute("CALL obrain.window_2d(3, 0, 65535, 0, 4294967295) YIELD node_id RETURN node_id")
        .expect("window 2d doit s'executer sur session read-only");
    assert_eq!(r.rows.len(), 1, "un nœud attendu en read-only");
}

#[test]
fn call_window_2d_refuse_arguments_invalides() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).expect("open_substrate");
    let session = db.session();

    // Borne de centralité hors u16.
    let err = session
        .execute("CALL obrain.window_2d(0, 0, 70000, 0, 10) YIELD node_id RETURN node_id")
        .expect_err("borne centralite hors u16 doit echouer");
    assert!(
        err.to_string().contains("centrality bounds"),
        "message inattendu: {err}"
    );

    // Argument manquant.
    let err = session
        .execute("CALL obrain.window_2d(0, 0, 100) YIELD node_id RETURN node_id")
        .expect_err("argument manquant doit echouer");
    assert!(
        err.to_string().contains("non-negative integer"),
        "message inattendu: {err}"
    );
}

#[test]
fn call_window_2d_refuse_une_base_non_substrate() {
    let td = tempfile::tempdir().unwrap();
    // `with_store` ne renseigne PAS `substrate_store` : chemin legacy/externe.
    let raw = obrain_substrate::SubstrateStore::create(td.path().join("kb")).unwrap();
    let erased: std::sync::Arc<dyn obrain_core::graph::GraphStoreMut> = std::sync::Arc::new(raw);
    let db = ObrainDB::with_store(erased, Config::in_memory()).expect("with_store");

    let err = db
        .session()
        .execute("CALL obrain.window_2d(0, 0, 65535, 0, 4294967295) YIELD node_id RETURN node_id")
        .expect_err("base non substrate doit echouer");
    assert!(
        err.to_string()
            .contains("requires a substrate-backed database"),
        "message d'erreur inattendu: {err}"
    );
}

#[test]
#[allow(clippy::many_single_char_names)] // nœuds a, b, c… du scénario, r : résultat lu
fn call_window_2d_energy_filtre_par_energie() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).expect("open_substrate");

    // Communauté 7 : trois nœuds à haute centralité, énergies distinctes.
    let a = create_with_centrality(&db, 7, 60000);
    let b = create_with_centrality(&db, 7, 60000);
    let c = create_with_centrality(&db, 7, 60000);
    set_energy(&db, a, 0.9);
    set_energy(&db, b, 0.5);
    set_energy(&db, c, 0.1);

    let session = db.session();

    // Fenêtre de base : les trois nœuds.
    let r = session
        .execute(
            "CALL obrain.window_2d(7, 50000, 65535, 0, 4294967295) YIELD node_id RETURN node_id",
        )
        .expect("window 2d");
    assert_eq!(r.rows.len(), 3, "fenêtre de base = 3 nœuds");

    // Fenêtre cognitive : seuil Q1.15 ~ 0.6 → seuls a (0.9) et b (0.5≥0.5? non).
    // Q1.15 : 0.6 → 0.6*32767 ≈ 19660 ; énergies 0.9→29490, 0.5→16384, 0.1→3277.
    // Seuil 20000 → a (29490) passe, b (16384) non, c (3277) non → 1 résultat.
    let r = session
        .execute("CALL obrain.window_2d_energy(7, 50000, 65535, 0, 4294967295, 20000) YIELD node_id, energy RETURN node_id, energy")
        .expect("window 2d energy");
    assert_eq!(r.rows.len(), 1, "seuil 20000 → un seul nœud (a)");
    // La colonne energy doit être cohérente (≈ 0.9 pour a).
    let e = match r.rows[0].get(1) {
        Some(obrain_common::types::Value::Float64(v)) => *v,
        other => panic!("energy inattendue: {other:?}"),
    };
    assert!((e - 0.9).abs() < 0.02, "énergie de a ≈ 0.9, got {e}");

    // Seuil 0 → les trois nœuds (fenêtre entière, énergie ≥ 0).
    let r = session
        .execute("CALL obrain.window_2d_energy(7, 50000, 65535, 0, 4294967295, 0) YIELD node_id RETURN node_id")
        .expect("window 2d energy seuil 0");
    assert_eq!(r.rows.len(), 3, "seuil 0 → fenêtre entière");

    // Seuil maximal → vide.
    let r = session
        .execute("CALL obrain.window_2d_energy(7, 50000, 65535, 0, 4294967295, 32767) YIELD node_id RETURN node_id")
        .expect("window 2d energy seuil max");
    assert!(r.rows.is_empty(), "seuil max → aucun nœud (énergie < 1.0)");
}

#[test]
fn call_window_2d_energy_read_only() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    {
        let db = ObrainDB::open_substrate(&path).expect("open_substrate");
        let a = create_with_centrality(&db, 3, 60000);
        set_energy(&db, a, 0.8);
    }
    let db = ObrainDB::open_read_only(&path).expect("open_read_only");
    let session = db.session();
    let r = session
        .execute("CALL obrain.window_2d_energy(3, 0, 65535, 0, 4294967295, 10000) YIELD node_id RETURN node_id")
        .expect("window_2d_energy doit s'executer en read-only (pure lecture)");
    assert_eq!(r.rows.len(), 1, "un nœud vivant attendu en read-only");
}
