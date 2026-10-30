#![cfg(all(feature = "cdc", feature = "algos"))]
//! CDC×GQL (2026-09-05) : les mutations GQL/executor (CREATE/MERGE — le chemin
//! réel du runtime via `session.execute`/import_events) doivent être
//! journalisées dans le CDC via l'observateur substrate, et :
//! (1) `search_delta` les retourne ; (2) `search_surprise` voit l'activité ;
//! (3) l'API `crud` ne produit PAS de doublon (anti-doublon activé).

use obrain_engine::ObrainDB;

fn count_entity_create(db: &ObrainDB, node_id: u64) -> usize {
    let events = db.search_delta(0, false, false).expect("search_delta");
    events
        .iter()
        .filter(|e| {
            e.kind == obrain_engine::cdc::ChangeKind::Create
                && matches!(e.entity_id, obrain_engine::cdc::EntityId::Node(n) if n.0 == node_id)
        })
        .count()
}

#[test]
fn cdc_capture_les_creations_gql() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-cdc-gql");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let s = db.session();

    // Chemin réel du runtime (import_events) : GQL CREATE puis MERGE d'arête.
    s.execute("CREATE (:A {qualified_name: 'x1'})")
        .expect("create x1");
    s.execute("CREATE (:A {qualified_name: 'x2'})")
        .expect("create x2");
    s.execute(
        "MATCH (a:A {qualified_name: 'x1'}), (b:A {qualified_name: 'x2'}) MERGE (a)-[:T]->(b)",
    )
    .expect("merge edge");

    let events = db.search_delta(0, false, false).expect("search_delta");
    // 3 créations attendues (2 nœuds + 1 arête) — avant l'observateur : 0.
    let creates = events
        .iter()
        .filter(|e| e.kind == obrain_engine::cdc::ChangeKind::Create)
        .count();
    assert!(
        creates >= 3,
        "créations GQL journalisées ≥3, got {creates} ({})",
        events.len()
    );
    // Une arête figure parmi les événements (kind Create).
    assert!(
        events.iter().any(|e| {
            e.kind == obrain_engine::cdc::ChangeKind::Create
                && matches!(e.entity_id, obrain_engine::cdc::EntityId::Edge(_))
        }),
        "l'arête MERGE est journalisée"
    );

    // search_surprise voit l'activité de la fenêtre.
    let rows = db.search_surprise(0, 50, false).expect("surprise");
    assert!(!rows.is_empty(), "activité détectée par search_surprise");
}

#[test]
fn cdc_pas_de_doublon_api_crud() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-cdc-dedup-api");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let id = db.create_node(&["B"]);
    assert_eq!(
        count_entity_create(&db, id.0),
        1,
        "API create_node → exactement 1 événement (pas de doublon observateur+crud)"
    );
}
