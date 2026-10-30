//! RFC zorder-superposition — capacité C1 : `ObrainDB::window_query_2d`.
//!
//! Test de DÉLÉGATION au niveau moteur : la méthode doit exposer le scan
//! EXACT du substrat (`SubstrateStore::window_query_2d`) sur une base
//! substrate temporaire, élargir les slots u32 en ids u64, et refuser
//! explicitement une base non adossée au substrate. La sémantique fine du
//! scan (bornes inclusives, tombstones, communautés étrangères) est déjà
//! couverte par `obrain-substrate/tests/window_query_2d.rs` — on ne la
//! re-teste pas ici, on vérifie le câblage.

use std::sync::Arc;

use obrain_core::graph::GraphStoreMut;
use obrain_engine::ObrainDB;
use obrain_engine::config::Config;

/// Crée un nœud de `cid` avec une centralité cible (Q0.16 approchée via f32,
/// comme le fait la suite substrate) et rend son slot (== id u64).
fn create_with_centrality(db: &ObrainDB, cid: u32, q16: u16) -> u64 {
    let store = db.substrate_handle().expect("base substrate");
    let node = store.create_node_in_community(&["L"], cid);
    store
        .update_centrality_batch_f32(vec![(node, q16 as f32 / u16::MAX as f32)])
        .expect("mise a jour de centralite");
    node.0
}

#[test]
fn window_query_2d_delegue_au_scan_substrat() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).expect("open_substrate");

    // Communauté 7 : deux centralités très écartées. Communauté 8 : une
    // seule, entre les deux. Aucune arête : les degrés sortants valent 0.
    let mut c7: Vec<u64> = vec![
        create_with_centrality(&db, 7, 1000),
        create_with_centrality(&db, 7, 60000),
    ];
    let mut c8: Vec<u64> = vec![create_with_centrality(&db, 8, 30000)];
    c7.sort_unstable();
    c8.sort_unstable();

    // Fenêtre ouverte sur chaque communauté : exactement ses nœuds vivants.
    let got = db
        .window_query_2d(7, 0, u16::MAX, 0, u32::MAX)
        .expect("fenetre ouverte cid=7");
    assert_eq!(got, c7, "communauté 7, fenêtre ouverte");
    let got = db
        .window_query_2d(8, 0, u16::MAX, 0, u32::MAX)
        .expect("fenetre ouverte cid=8");
    assert_eq!(got, c8, "communauté 8, fenêtre ouverte");

    // Fenêtre de centralité étroite : seul le nœud à haute centralité de la
    // communauté 7 répond (drift de quantification ±1 autour de 60000).
    let got = db
        .window_query_2d(7, 50000, u16::MAX, 0, u32::MAX)
        .expect("fenetre haute centralite");
    assert_eq!(got, vec![c7[1]], "communauté 7, haute centralité seulement");

    // Bornes inversées normalisées : équivalent de la fenêtre ouverte.
    let got = db
        .window_query_2d(7, u16::MAX, 0, u32::MAX, 0)
        .expect("bornes inversees");
    assert_eq!(got, c7, "bornes inversées normalisées");

    // Communauté inconnue : vide, sans erreur.
    let got = db
        .window_query_2d(42, 0, u16::MAX, 0, u32::MAX)
        .expect("communaute inconnue");
    assert!(got.is_empty(), "communauté 42 n'existe pas");
}

#[test]
fn window_query_2d_refuse_une_base_non_substrate() {
    let td = tempfile::tempdir().unwrap();
    // `with_store` ne renseigne PAS `substrate_store` (champ `None`) : c'est
    // le chemin legacy/externe, exactement la configuration que la méthode
    // moteur doit refuser avec un message explicite.
    let raw = obrain_substrate::SubstrateStore::create(td.path().join("kb")).unwrap();
    let erased: Arc<dyn GraphStoreMut> = Arc::new(raw);
    let db = ObrainDB::with_store(erased, Config::in_memory()).expect("with_store");

    let err = db
        .window_query_2d(1, 0, u16::MAX, 0, u32::MAX)
        .expect_err("base non substrate doit echouer");
    assert!(
        err.to_string()
            .contains("requires a substrate-backed database"),
        "message d'erreur inattendu: {err}"
    );
}
