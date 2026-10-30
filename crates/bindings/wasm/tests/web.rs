//! WASM binding integration tests.
//!
//! Run via: `wasm-pack test crates/bindings/wasm --node`

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;

use obrain_wasm::Database;

/// Le message d'un `JsError`, lu comme le ferait un appelant JS.
///
/// `JsError` n'implémente pas `Display` (d'où l'ancien `format!("{err}")`, qui
/// ne compilait plus) et son `Debug` passe par `JsValue` : on lit donc la
/// propriété `message` de l'objet JS sous-jacent, seule forme stable.
fn js_error_message(err: wasm_bindgen::JsError) -> String {
    let value: wasm_bindgen::JsValue = err.into();
    js_sys::Reflect::get(&value, &wasm_bindgen::JsValue::from_str("message"))
        .ok()
        .and_then(|m| m.as_string())
        .unwrap_or_else(|| format!("{value:?}"))
}

#[wasm_bindgen_test]
fn test_database_creation() {
    let db = Database::new().expect("should create in-memory database");
    assert_eq!(db.node_count(), 0);
    assert_eq!(db.edge_count(), 0);
}

#[wasm_bindgen_test]
fn test_insert_and_query() {
    let db = Database::new().expect("create db");

    db.execute("CREATE (:Person {name: 'Alix', age: 30})")
        .expect("create node");
    assert_eq!(db.node_count(), 1);

    let result = db
        .execute("MATCH (n:Person) RETURN n.name, n.age")
        .expect("query");

    // Result is a JsValue (array of objects)
    assert!(!result.is_null());
    assert!(!result.is_undefined());
}

#[wasm_bindgen_test]
fn test_execute_raw_structure() {
    let db = Database::new().expect("create db");

    db.execute("CREATE (:N {x: 1}), (:N {x: 2})")
        .expect("create");

    let raw = db.execute_raw("MATCH (n:N) RETURN n.x").expect("raw query");

    // execute_raw returns { columns, rows, executionTimeMs }
    assert!(!raw.is_null());
    assert!(!raw.is_undefined());
}

#[wasm_bindgen_test]
fn test_execute_with_language_gql() {
    let db = Database::new().expect("create db");

    db.execute("CREATE (:Person {name: 'Alix'})")
        .expect("create");

    let result = db
        .execute_with_language("MATCH (n:Person) RETURN n.name", "gql")
        .expect("gql query");

    assert!(!result.is_null());
}

#[wasm_bindgen_test]
fn test_execute_with_unknown_language_error() {
    let db = Database::new().expect("create db");

    let result = db.execute_with_language("SELECT 1", "unknown_lang");
    assert!(result.is_err(), "unknown language should error");
}

/// Aller-retour entre DEUX bases par la surface RÉELLE du binding.
///
/// (2026-09-17) Ce test s'appelait `test_snapshot_roundtrip` et utilisait
/// `Database::export_snapshot()` / `Database::import_snapshot()`. Cette surface
/// a été **retirée** avec les formats legacy `.obrain` v1/v2 (commit `b0f5bd11`,
/// « T17 final cutover ») : la version substrat persiste par son propre
/// répertoire, et le moteur renvoyait explicitement « a substrate-native
/// snapshot API lands as T17b ». Le test était donc resté sur une API
/// disparue — ce qui, avec les deux autres erreurs de compilation, a laissé la
/// suite ENTIÈRE muette pendant que le module était inutilisable dans un
/// navigateur.
///
/// En attendant T17b, on éprouve l'aller-retour avec ce qui existe : on relit
/// les valeurs par une requête, on en reconstruit une charge LPG, et on la
/// réimporte dans une base neuve. Le jour où un snapshot natif arrive, c'est ce
/// test qu'il faudra renforcer — pas ressusciter.
#[wasm_bindgen_test]
fn test_roundtrip_entre_deux_bases() {
    let db = Database::new().expect("create db");

    db.execute("CREATE (:Person {name: 'Alix'})")
        .expect("create");
    assert_eq!(db.node_count(), 1);

    // Relecture par la surface publique : la valeur relue est la source de vérité.
    let rows = db.execute("MATCH (n:Person) RETURN n.name AS name").expect("read");
    let premiere = js_sys::Reflect::get(&rows, &wasm_bindgen::JsValue::from_f64(0.0))
        .expect("ligne 0");
    let nom = js_sys::Reflect::get(&premiere, &wasm_bindgen::JsValue::from_str("name"))
        .expect("colonne name")
        .as_string()
        .expect("name est une chaîne");
    assert_eq!(nom, "Alix");

    // Réimport de la valeur relue dans une base NEUVE.
    let data = lpg_input(serde_json::json!({
        "nodes": [{ "labels": ["Person"], "properties": { "name": nom } }]
    }));
    let restored = Database::new().expect("restore db");
    restored.import_lpg(data).expect("import");
    assert_eq!(restored.node_count(), 1);
}

#[wasm_bindgen_test]
fn test_schema() {
    let db = Database::new().expect("create db");

    db.execute("CREATE (:Person {name: 'Alix'})-[:KNOWS]->(:Person {name: 'Gus'})")
        .expect("create");

    let schema = db.schema().expect("schema");
    assert!(!schema.is_null());
    assert!(!schema.is_undefined());
}

#[wasm_bindgen_test]
fn test_version() {
    let v = Database::version();
    assert!(!v.is_empty());
}

// ---------------------------------------------------------------------------
// importLpg tests
// ---------------------------------------------------------------------------

/// Construit la charge d'entrée d'un import LPG : un objet JS ORDINAIRE.
///
/// `serde_wasm_bindgen::to_value` sérialise les maps en `Map` JS, alors que le
/// binding lit un objet à champs nommés (`serde_wasm_bindgen::from_value` sur
/// `LpgImport`) : le `Map` faisait échouer 14 tests sur
/// « Invalid LPG data: missing field `nodes` » — un défaut de CONSTRUCTION DU
/// TEST, pas du binding, qui recevrait d'un appelant JS un objet littéral.
/// `json_compatible()` produit exactement cette forme.
fn lpg_input(v: serde_json::Value) -> wasm_bindgen::JsValue {
    use serde::Serialize as _;
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .expect("serialisation json_compatible")
}

fn lpg_result_counts(result: &wasm_bindgen::JsValue) -> (u32, u32) {
    let nodes = js_sys::Reflect::get(result, &wasm_bindgen::JsValue::from_str("nodes"))
        .unwrap()
        .as_f64()
        .unwrap() as u32;
    let edges = js_sys::Reflect::get(result, &wasm_bindgen::JsValue::from_str("edges"))
        .unwrap()
        .as_f64()
        .unwrap() as u32;
    (nodes, edges)
}

#[wasm_bindgen_test]
fn test_import_lpg_basic() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Person"], "properties": { "name": "Alix", "age": 30 } },
            { "labels": ["Person"], "properties": { "name": "Gus", "age": 25 } }
        ],
        "edges": [
            { "source": 0, "target": 1, "type": "KNOWS", "properties": { "since": 2020 } }
        ]
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, edges) = lpg_result_counts(&result);
    assert_eq!(nodes, 2);
    assert_eq!(edges, 1);
    assert_eq!(db.node_count(), 2);
    assert_eq!(db.edge_count(), 1);
}

#[wasm_bindgen_test]
fn test_import_lpg_nodes_only() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Tag"], "properties": { "name": "rust" } },
            { "labels": ["Tag"], "properties": { "name": "wasm" } },
            { "labels": ["Tag"], "properties": { "name": "graph" } }
        ]
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, edges) = lpg_result_counts(&result);
    assert_eq!(nodes, 3);
    assert_eq!(edges, 0);
    assert_eq!(db.node_count(), 3);
}

#[wasm_bindgen_test]
fn test_import_lpg_empty() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": []
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, edges) = lpg_result_counts(&result);
    assert_eq!(nodes, 0);
    assert_eq!(edges, 0);
}

#[wasm_bindgen_test]
fn test_import_lpg_no_properties() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["A"] },
            { "labels": ["B"] }
        ],
        "edges": [
            { "source": 0, "target": 1, "type": "LINKED" }
        ]
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, edges) = lpg_result_counts(&result);
    assert_eq!(nodes, 2);
    assert_eq!(edges, 1);
}

#[wasm_bindgen_test]
fn test_import_lpg_multiple_labels() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Person", "Employee", "Developer"], "properties": { "name": "Alix" } }
        ]
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, _) = lpg_result_counts(&result);
    assert_eq!(nodes, 1);
    assert_eq!(db.node_count(), 1);
}

#[wasm_bindgen_test]
fn test_import_lpg_self_loop() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [{ "labels": ["Node"] }],
        "edges": [{ "source": 0, "target": 0, "type": "SELF" }]
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, edges) = lpg_result_counts(&result);
    assert_eq!(nodes, 1);
    assert_eq!(edges, 1);
}

#[wasm_bindgen_test]
fn test_import_lpg_multiple_edges_same_pair() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Person"], "properties": { "name": "Alix" } },
            { "labels": ["Person"], "properties": { "name": "Gus" } }
        ],
        "edges": [
            { "source": 0, "target": 1, "type": "KNOWS" },
            { "source": 0, "target": 1, "type": "WORKS_WITH" },
            { "source": 1, "target": 0, "type": "KNOWS" }
        ]
    }));

    let result = db.import_lpg(data).expect("import");
    let (_, edges) = lpg_result_counts(&result);
    assert_eq!(edges, 3);
    assert_eq!(db.edge_count(), 3);
}

#[wasm_bindgen_test]
fn test_import_lpg_edge_source_out_of_bounds() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [{ "labels": ["A"] }],
        "edges": [{ "source": 5, "target": 0, "type": "BAD" }]
    }));

    let err = db.import_lpg(data).unwrap_err();
    let msg = js_error_message(err);
    assert!(
        msg.contains("source index 5 out of bounds"),
        "unexpected error: {msg}"
    );
}

#[wasm_bindgen_test]
fn test_import_lpg_edge_target_out_of_bounds() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [{ "labels": ["A"] }],
        "edges": [{ "source": 0, "target": 99, "type": "BAD" }]
    }));

    let err = db.import_lpg(data).unwrap_err();
    let msg = js_error_message(err);
    assert!(
        msg.contains("target index 99 out of bounds"),
        "unexpected error: {msg}"
    );
}

#[wasm_bindgen_test]
fn test_import_lpg_invalid_shape() {
    let db = Database::new().expect("create db");

    // Missing required 'nodes' field
    let data = lpg_input(serde_json::json!({
        "edges": []
    }));

    let err = db.import_lpg(data).unwrap_err();
    let msg = js_error_message(err);
    assert!(msg.contains("Invalid LPG data"), "unexpected error: {msg}");
}

#[wasm_bindgen_test]
fn test_import_lpg_queryable_after_import() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["City"], "properties": { "name": "Amsterdam", "population": 905234 } },
            { "labels": ["City"], "properties": { "name": "Berlin", "population": 3748148 } }
        ],
        "edges": [
            { "source": 0, "target": 1, "type": "CONNECTED_TO", "properties": { "distance_km": 577 } }
        ]
    }));

    db.import_lpg(data).expect("import");

    // Verify nodes are queryable
    let result = db
        .execute("MATCH (c:City) RETURN c.name ORDER BY c.name")
        .expect("query cities");
    assert!(!result.is_null());

    // Verify edges are queryable
    let result = db
        .execute("MATCH (:City)-[e:CONNECTED_TO]->(:City) RETURN e.distance_km")
        .expect("query edges");
    assert!(!result.is_null());
}

#[wasm_bindgen_test]
fn test_import_lpg_mixed_property_types() {
    let db = Database::new().expect("create db");

    let data = lpg_input(serde_json::json!({
        "nodes": [{
            "labels": ["Thing"],
            "properties": {
                "str_val": "hello",
                "int_val": 42,
                "float_val": 3.14,
                "bool_val": true,
                "null_val": null,
                "list_val": [1, 2, 3]
            }
        }]
    }));

    let result = db.import_lpg(data).expect("import");
    let (nodes, _) = lpg_result_counts(&result);
    assert_eq!(nodes, 1);
}

#[wasm_bindgen_test]
fn test_import_lpg_incremental() {
    let db = Database::new().expect("create db");

    // First batch
    let data1 = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Person"], "properties": { "name": "Alix" } }
        ]
    }));
    db.import_lpg(data1).expect("import 1");
    assert_eq!(db.node_count(), 1);

    // Second batch
    let data2 = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Person"], "properties": { "name": "Gus" } }
        ]
    }));
    db.import_lpg(data2).expect("import 2");
    assert_eq!(db.node_count(), 2);
}

/// Même aller-retour, mais AVEC une arête — le cas que le test précédent ne
/// couvre pas, et que l'ancien `test_import_lpg_snapshot_roundtrip` visait par
/// l'intermédiaire du snapshot bincode retiré (voir le commentaire de
/// `test_roundtrip_entre_deux_bases` pour la raison).
#[wasm_bindgen_test]
fn test_roundtrip_avec_arete() {
    let db = Database::new().expect("create db");

    db.execute("CREATE (:Person {name: 'Alix'})-[:KNOWS]->(:Person {name: 'Gus'})")
        .expect("create");
    assert_eq!(db.node_count(), 2);
    assert_eq!(db.edge_count(), 1);

    // La relation doit être RELISIBLE — c'est elle qu'on réimporte.
    let rows = db
        .execute("MATCH (:Person)-[r:KNOWS]->(:Person) RETURN type(r) AS t")
        .expect("read edge");
    let ligne = js_sys::Reflect::get(&rows, &wasm_bindgen::JsValue::from_f64(0.0))
        .expect("ligne 0");
    let t = js_sys::Reflect::get(&ligne, &wasm_bindgen::JsValue::from_str("t"))
        .expect("colonne t")
        .as_string()
        .expect("type(r) est une chaîne");
    assert_eq!(t, "KNOWS");

    let data = lpg_input(serde_json::json!({
        "nodes": [
            { "labels": ["Person"], "properties": { "name": "Alix" } },
            { "labels": ["Person"], "properties": { "name": "Gus" } }
        ],
        "edges": [{ "source": 0, "target": 1, "type": t }]
    }));

    let restored = Database::new().expect("restore db");
    restored.import_lpg(data).expect("import");
    assert_eq!(restored.node_count(), 2);
    assert_eq!(restored.edge_count(), 1);
}
