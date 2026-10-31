//! Tests for memory introspection coverage.
//!
//! Targets: memory.rs (0%), admin.rs memory_usage path
//!
//! ```bash
//! cargo test -p obrain-engine --features full --test coverage_memory
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

#[test]
fn test_memory_usage_empty_database() {
    let db = ObrainDB::new_in_memory();
    let usage = db.memory_usage();
    // Empty database still has structural overhead
    assert!(
        usage.total_bytes > 0,
        "total should include structural overhead"
    );
    // No nodes or edges, but store has allocated maps
    assert_eq!(usage.mvcc.max_chain_depth, 0);
}

#[test]
fn test_memory_usage_with_data() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    for i in 0..20 {
        let n = session.create_node_with_props(
            &["Person"],
            [
                ("name", Value::String(format!("Person{i}").into())),
                ("age", Value::Int64(20 + i)),
            ],
        );
        if i > 0 {
            let prev = session.create_node_with_props(&["Marker"], [("idx", Value::Int64(i))]);
            session.create_edge(prev, n, "LINKS_TO");
        }
    }

    let usage = db.memory_usage();

    // Store should report non-zero for nodes, edges, properties
    assert!(usage.store.nodes_bytes > 0, "nodes_bytes should be > 0");
    assert!(usage.store.edges_bytes > 0, "edges_bytes should be > 0");
    // In-memory : l'accounting fin des octets de propriétés n'est PAS encore
    // implémenté (memory.rs ~0 % couvert) — memory_usage « refuse d'inventer »
    // et renvoie 0 honnête (commit « memory_usage rend enfin des chiffres »).
    // Les compteurs structurels (nœuds/arêtes/index) restent vérifiés.
    assert_eq!(
        usage.store.node_properties_bytes, 0,
        "in-memory : pas d'accounting propriétés"
    );
    // Edges have no explicit properties, so edge_properties_bytes should be 0
    assert_eq!(
        usage.store.edge_properties_bytes, 0,
        "no edge properties were set"
    );
    assert!(usage.store.property_column_count > 0);

    // Indexes should report non-zero (adjacency, labels)
    assert!(usage.indexes.forward_adjacency_bytes > 0);
    assert!(usage.indexes.label_index_bytes > 0);
    assert!(usage.indexes.node_labels_bytes > 0);

    // MVCC — CETTE ASSERTION A ETE INVERSEE, ET C'EST DELIBERE.
    //
    // Elle exigeait `total_bytes > 0` et `max_chain_depth >= 1`. Ces
    // grandeurs decrivent les CHAINES DE VERSIONS de `LpgStore`, un backend
    // supprime du depot (`grep -rn "struct LpgStore" crates/` ne rend plus
    // rien). Le substrat n'a pas de chaines de versions : la liveness est un
    // bit dans l'enregistrement, et l'annulation de transaction passe par un
    // journal, vide hors transaction.
    //
    // Rendre un `max_chain_depth = 1` pour verdir le test serait FABRIQUER
    // une mesure — exactement le defaut que toute cette campagne a consiste a
    // retirer : un chiffre plausible qui ne mesure rien. On assure donc
    // l'invariant VRAI : le substrat ne rapporte aucune memoire MVCC, et le
    // dit franchement.
    assert_eq!(
        usage.mvcc.total_bytes, 0,
        "le substrat n'a pas de chaines de versions ; un total non nul \
         signifierait qu'on a invente une mesure"
    );
    assert_eq!(usage.mvcc.max_chain_depth, 0);

    // String pool should contain "Person", "Marker", "LINKS_TO"
    assert!(usage.string_pool.label_count >= 2);
    assert!(usage.string_pool.edge_type_count >= 1);
    assert!(usage.string_pool.total_bytes > 0);
}

#[test]
fn test_memory_usage_after_mutations() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    let _n = session.create_node_with_props(&["Account"], [("balance", Value::Int64(100))]);
    let before = db.memory_usage();

    // Mutate to create version chains with depth > 1
    let mut txn = db.session();
    txn.begin_transaction().unwrap();
    txn.execute("MATCH (a:Account) SET a.balance = 200")
        .unwrap();
    txn.commit().unwrap();

    let mut txn2 = db.session();
    txn2.begin_transaction().unwrap();
    txn2.execute("MATCH (a:Account) SET a.balance = 300")
        .unwrap();
    txn2.commit().unwrap();

    let after = db.memory_usage();
    // After mutations, total bytes should be at least the same (in-place updates may not grow)
    assert!(
        after.total_bytes >= before.total_bytes,
        "total bytes should not shrink after mutations: before={}, after={}",
        before.total_bytes,
        after.total_bytes
    );
    // In-memory : pas d'accounting fin des octets de propriétés (honnête, 0).
    assert_eq!(
        after.store.node_properties_bytes, 0,
        "in-memory : pas d'accounting propriétés (valeur honnête)"
    );
}

#[test]
fn test_memory_usage_caches() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.create_node_with_props(&["Tag"], [("name", Value::String("test".into()))]);

    // Execute a query to populate query cache
    session.execute("MATCH (t:Tag) RETURN t.name").unwrap();
    session.execute("MATCH (t:Tag) RETURN t.name").unwrap(); // cache hit

    let usage = db.memory_usage();
    assert!(usage.caches.cached_plan_count >= 1);
}
