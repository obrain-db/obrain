//! Integration tests for `obrain_core::execution::operators::MergeOperator`
//! against the substrate backend.
//!
//! ## Why this lives in obrain-substrate and not in obrain-core
//!
//! Relocated as part of T17 Step 3 W2/Class-2 follow-up (decision
//! `b1dfe229`). `obrain-core` cannot take `obrain-substrate` as a
//! dev-dependency without creating two distinct compilation units of
//! `obrain-core` (gotcha `598dda40`). The forward direction
//! (`obrain-substrate → obrain-core`) has no cycle, so moving operator
//! tests into an integration test of `obrain-substrate` is the
//! post-T17-cutover home for the LPG-era fixtures.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p obrain-substrate --test operators_merge
//! ```

use obrain_common::types::{LogicalType, PropertyKey, Value};
use obrain_core::execution::operators::{MergeConfig, MergeOperator, Operator, PropertySource};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;

fn const_props(props: Vec<(&str, Value)>) -> Vec<(String, PropertySource)> {
    props
        .into_iter()
        .map(|(k, v)| (k.to_string(), PropertySource::Constant(v)))
        .collect()
}

#[test]
fn test_merge_creates_new_node() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    // MERGE should create a new node since none exists
    let mut merge = MergeOperator::new(
        Arc::clone(&store),
        None,
        MergeConfig {
            variable: "n".to_string(),
            labels: vec!["Person".to_string()],
            match_properties: const_props(vec![("name", Value::String("Alix".into()))]),
            on_create_properties: vec![],
            on_match_properties: vec![],
            output_schema: vec![LogicalType::Node],
            output_column: 0,
            bound_variable_column: None,
        },
    );

    let result = merge.next().unwrap();
    assert!(result.is_some());

    // Verify node was created
    let nodes = store.nodes_by_label("Person");
    assert_eq!(nodes.len(), 1);

    let node = store.get_node(nodes[0]).unwrap();
    assert!(node.has_label("Person"));
    assert_eq!(
        node.properties.get(&PropertyKey::new("name")),
        Some(&Value::String("Alix".into()))
    );
}

#[test]
fn test_merge_matches_existing_node() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    // Create an existing node
    store.create_node_with_props(
        &["Person"],
        &[(PropertyKey::new("name"), Value::String("Gus".into()))],
    );

    // MERGE should find the existing node
    let mut merge = MergeOperator::new(
        Arc::clone(&store),
        None,
        MergeConfig {
            variable: "n".to_string(),
            labels: vec!["Person".to_string()],
            match_properties: const_props(vec![("name", Value::String("Gus".into()))]),
            on_create_properties: vec![],
            on_match_properties: vec![],
            output_schema: vec![LogicalType::Node],
            output_column: 0,
            bound_variable_column: None,
        },
    );

    let result = merge.next().unwrap();
    assert!(result.is_some());

    // Verify only one node exists (no new node created)
    let nodes = store.nodes_by_label("Person");
    assert_eq!(nodes.len(), 1);
}

#[test]
fn test_merge_with_on_create() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    // MERGE with ON CREATE SET
    let mut merge = MergeOperator::new(
        Arc::clone(&store),
        None,
        MergeConfig {
            variable: "n".to_string(),
            labels: vec!["Person".to_string()],
            match_properties: const_props(vec![("name", Value::String("Vincent".into()))]),
            on_create_properties: const_props(vec![("created", Value::Bool(true))]),
            on_match_properties: vec![],
            output_schema: vec![LogicalType::Node],
            output_column: 0,
            bound_variable_column: None,
        },
    );

    let _ = merge.next().unwrap();

    // Verify node has both match properties and on_create properties
    let nodes = store.nodes_by_label("Person");
    let node = store.get_node(nodes[0]).unwrap();
    assert_eq!(
        node.properties.get(&PropertyKey::new("name")),
        Some(&Value::String("Vincent".into()))
    );
    assert_eq!(
        node.properties.get(&PropertyKey::new("created")),
        Some(&Value::Bool(true))
    );
}

#[test]
fn test_merge_with_on_match() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    // Create an existing node
    let node_id = store.create_node_with_props(
        &["Person"],
        &[(PropertyKey::new("name"), Value::String("Jules".into()))],
    );

    // MERGE with ON MATCH SET
    let mut merge = MergeOperator::new(
        Arc::clone(&store),
        None,
        MergeConfig {
            variable: "n".to_string(),
            labels: vec!["Person".to_string()],
            match_properties: const_props(vec![("name", Value::String("Jules".into()))]),
            on_create_properties: vec![],
            on_match_properties: const_props(vec![("updated", Value::Bool(true))]),
            output_schema: vec![LogicalType::Node],
            output_column: 0,
            bound_variable_column: None,
        },
    );

    let _ = merge.next().unwrap();

    // Verify node has the on_match property added
    let node = store.get_node(node_id).unwrap();
    assert_eq!(
        node.properties.get(&PropertyKey::new("updated")),
        Some(&Value::Bool(true))
    );
}

/// T17b (2026-08-05) : avec un index de propriété enregistré sur la clé de
/// match, MERGE matche le nœud existant via l'index (candidats 0/1) au lieu
/// du scan de label — même résultat, pas de doublon, et un second MERGE sur
/// une valeur absente crée bien un nouveau nœud.
#[test]
fn t17b_merge_uses_property_index_and_stays_correct() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
    store.create_property_index("id");
    assert!(store.has_property_index("id"));

    let existing = store.create_node_with_props(
        &["Rec"],
        &[(PropertyKey::new("id"), Value::String("pa_1".into()))],
    );

    let merge_for = |value: &str| {
        MergeOperator::new(
            Arc::clone(&store),
            None,
            MergeConfig {
                variable: "n".to_string(),
                labels: vec!["Rec".to_string()],
                match_properties: const_props(vec![("id", Value::String(value.into()))]),
                on_create_properties: vec![],
                on_match_properties: vec![],
                output_schema: vec![LogicalType::Node],
                output_column: 0,
                bound_variable_column: None,
            },
        )
    };

    // Match d'un existant : pas de création.
    let mut m1 = merge_for("pa_1");
    let _ = m1.next().unwrap();
    assert_eq!(store.nodes_by_label("Rec").len(), 1);

    // Valeur absente : création d'un nouveau nœud, indexé à son tour.
    let mut m2 = merge_for("pa_2");
    let _ = m2.next().unwrap();
    let recs = store.nodes_by_label("Rec");
    assert_eq!(recs.len(), 2);
    assert!(recs.contains(&existing));

    // Rejouer le MERGE sur pa_2 ne duplique pas (l'index a bien vu la
    // création faite PAR le MergeOperator lui-même).
    let mut m3 = merge_for("pa_2");
    let _ = m3.next().unwrap();
    assert_eq!(store.nodes_by_label("Rec").len(), 2);
}
