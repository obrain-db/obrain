#![cfg(all(feature = "cdc", feature = "algos"))]
//! `obrain.search_delta` — diff CDC graphe « ce qui a changé depuis ma
//! dernière vue » (méthode ObrainDB + procédure CALL).
//!
//! Vérifie : (1) la méthode `ObrainDB::search_delta(since_ms)` retourne les
//! changements depuis un instant ; (2) la procédure CALL expose les mêmes ;
//! (3) le filtre node/edge fonctionne ; (4) lecture pure (read-only).

use obrain_engine::ObrainDB;

fn setup_db() -> ObrainDB {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    std::mem::forget(td); // garde le tempdir vivant pour la durée du test
    ObrainDB::open_substrate(path).expect("open_substrate")
}

#[test]
fn search_delta_retourne_les_changements_tries() {
    let db = setup_db();

    // Deux créations de nœuds + une propriété + une arête.
    let n1 = db.create_node(&["Doc"]);
    db.set_node_property(
        n1,
        "title",
        obrain_common::types::Value::String("alpha".into()),
    );
    let n2 = db.create_node(&["Doc"]);
    db.set_node_property(
        n2,
        "title",
        obrain_common::types::Value::String("beta".into()),
    );
    db.create_edge(n1, n2, "LINK");

    // since=0 → toutes les mutations enregistrées.
    let all = db.search_delta(0, false, false).expect("search_delta(0)");
    assert!(
        all.len() >= 5,
        "≥ 2 create nœuds + 2 create props + 1 create arête, got {}",
        all.len()
    );

    // Tri par timestamp croissant.
    let ts: Vec<u64> = all.iter().map(|e| e.timestamp).collect();
    let mut ts_sorted = ts.clone();
    ts_sorted.sort_unstable();
    assert_eq!(ts, ts_sorted, "tri par timestamp croissant");

    // since futur → vide.
    let future = all
        .last()
        .map(|e| e.timestamp + 1_000_000)
        .unwrap_or(u64::MAX);
    let empty = db.search_delta(future, false, false).expect("futur");
    assert!(empty.is_empty(), "aucun changement après le dernier");

    // Filtre only_nodes=true (exclut les arêtes) : strictement moins d'events.
    let nodes_only = db.search_delta(0, true, false).expect("only_nodes");
    let edges_only = db.search_delta(0, false, true).expect("only_edges");
    assert!(
        nodes_only.len() < all.len() && !edges_only.is_empty(),
        "nodes {} < total {} et edges {} > 0",
        nodes_only.len(),
        all.len(),
        edges_only.len()
    );
}

#[test]
fn call_search_delta_expose_le_diff() {
    let db = setup_db();
    let a = db.create_node(&["Doc"]);
    db.set_node_property(a, "title", obrain_common::types::Value::String("x".into()));
    let b = db.create_node(&["Doc"]);
    db.create_edge(a, b, "LINK");

    let session = db.session();
    let r = session
        .execute("CALL obrain.search_delta(0) YIELD entity_id, kind RETURN entity_id, kind")
        .expect("call search_delta");
    assert!(
        r.rows.len() >= 3,
        "2 create nœuds + 1 arête, got {}",
        r.rows.len()
    );
    for row in &r.rows {
        match row.get(1) {
            Some(obrain_common::types::Value::String(s)) => {
                assert!(
                    s == "create" || s == "update",
                    "mutations initiales = create/update (jamais delete), got {s}"
                )
            }
            other => panic!("kind inattendu: {other:?}"),
        }
    }

    // Read-only : la procédure est une lecture pure.
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("ro");
    {
        let w = ObrainDB::open_substrate(&path).expect("open rw");
        w.create_node(&["Doc"]);
    }
    std::mem::forget(td);
    let db_ro = ObrainDB::open_read_only(&path).expect("open ro");
    let s_ro = db_ro.session();
    let r_ro = s_ro
        .execute("CALL obrain.search_delta(0) YIELD entity_id RETURN entity_id")
        .expect("search_delta en read-only");
    assert!(
        !r_ro.rows.is_empty(),
        "read-only doit retourner les changements"
    );
}

/// (2026-09-05) RECHARGEMENT PARESSEUX — l'ouverture d'un store ne décode
/// plus la zone CDC (régression d'ouverture 2026-09-05 : ~0,7 s / ~1 Go sur
/// stores runtime) ; l'historique persisté n'est rechargé qu'à la première
/// lecture, sans doublon, et les écritures vives de la session suivent.
#[test]
fn cdc_recharge_paresseusement_a_la_premiere_lecture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-cdc-lazy");
    // Session 1 : mutations (enregistrées + persistées en zone).
    {
        let w = ObrainDB::open_substrate(&path).expect("open rw");
        let n = w.create_node(&["Doc"]);
        w.set_node_property(
            n,
            "title",
            obrain_common::types::Value::String("session-1".into()),
        );
        w.create_edge(n, n, "SELF");
        w.checkpoint().expect("checkpoint");
    }
    // Réouverture : RIEN n'est chargé (lazy) → l'ouverture reste légère.
    let db = ObrainDB::open_substrate(&path).expect("reopen");
    assert_eq!(
        db.cdc_log().event_count(),
        0,
        "aucun événement CDC décodé au boot (lazy)"
    );
    // Première lecture : recharge la fenêtre persistée (create node + prop +
    // edge + edge prop éventuels) puis sert l'historique.
    let all = db.search_delta(0, false, false).expect("search_delta(0)");
    assert!(
        all.len() >= 3,
        "≥ create nœud + create prop + create arête persistés, got {}",
        all.len()
    );
    assert!(db.cdc_log().event_count() >= all.len());
    // Relire ne double rien (load exactement une fois).
    let all2 = db.search_delta(0, false, false).expect("relecture");
    assert_eq!(all.len(), all2.len(), "pas de doublon à la relecture");
    // Écriture vive APRÈS le load : visible, sans doublon des persistées.
    let n3 = db.create_node(&["Doc"]);
    let after = db.search_delta(0, false, false).expect("après écriture");
    assert_eq!(
        after.len(),
        all.len() + 1,
        "1 nouvelle mutation, aucune des persistées dupliquée"
    );
    let _ = n3;
    // Early-exit futur : vide (max_timestamp à jour après le load).
    let future = all
        .last()
        .map(|e| e.timestamp + 1_000_000)
        .unwrap_or(u64::MAX);
    assert!(
        db.search_delta(future, false, false)
            .expect("futur")
            .is_empty()
    );
}

/// (2026-09-05) MÊME PROCESS écrivain puis lecteur : les événements déjà en
/// mémoire (enregistrés par la session) ne sont pas dupliqués par le
/// rechargement de la zone (dédup par (entity_id, epoch, timestamp)).
#[test]
fn cdc_pas_de_doublon_ecrivain_puis_lecteur_meme_process() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-cdc-dedup");
    let db = ObrainDB::open_substrate(&path).expect("open rw");
    let n1 = db.create_node(&["Doc"]);
    let n2 = db.create_node(&["Doc"]);
    db.create_edge(n1, n2, "LINK");
    // Avant la première lecture : 3 événements en mémoire (jamais rechargés).
    let mem = db.cdc_log().event_count();
    assert_eq!(mem, 3, "3 mutations en mémoire, got {mem}");
    // Première lecture : recharge la zone (qui contient ces 3 événements,
    // appendés par la session) → la dédup doit les écarter.
    let all = db.search_delta(0, false, false).expect("search_delta(0)");
    assert_eq!(all.len(), 3, "aucun doublon des événements de la session");
    assert_eq!(db.cdc_log().event_count(), 3);
}
