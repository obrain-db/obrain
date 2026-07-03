//! Audit where node properties actually live on PO: DashMap / blob_columns
//! / vec_columns / PropsZone v2. Uses get_node_property probe per zone.
#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
#[ignore = "ad-hoc audit — read-only"]
fn audit_where_po_properties_live() {
    let home = std::env::var("HOME").unwrap_or_default();
    for (name, path) in [
        ("po", std::path::PathBuf::from(&home).join(".obrain/db/po")),
        (
            "wikipedia",
            std::path::PathBuf::from(&home).join(".obrain/db/wikipedia"),
        ),
        (
            "megalaw",
            std::path::PathBuf::from(&home).join(".obrain/db/megalaw"),
        ),
    ] {
        if !path.exists() {
            eprintln!("⏭  {name}: not present");
            continue;
        }
        let db = ObrainDB::open(&path).expect("open");
        let store = db.store();
        println!("\n═══ {name} ═══");
        println!("  total nodes = {}", store.node_count());

        // Find nodes for each label, probe their properties
        for label in ["File", "Function", "Article", "Concept", "Decision"] {
            let nodes = store.nodes_by_label(label);
            if nodes.is_empty() {
                continue;
            }
            let first = nodes[0];

            // Try via get_node (DashMap-only path)
            let via_get_node = store.get_node(first);
            let dashmap_props_count = via_get_node.as_ref().map_or(0, |n| n.properties.len());

            // Probe each candidate property key via get_node_property (all zones)
            let probes = [
                "title",
                "name",
                "path",
                "abstract",
                "url",
                "lang",
                "word_count",
                "content",
                "body",
                "_st_embedding",
                "_hilbert_features",
                "_kernel_embedding",
                "created_at",
                "updated_at",
                "line",
                "kind",
                "size",
                "author",
            ];
            let mut found: Vec<(&str, &'static str)> = Vec::new();
            for k in probes {
                let pk = obrain_common::PropertyKey::from(k);
                if let Some(v) = store.get_node_property(first, &pk) {
                    let kind = match v {
                        obrain_common::types::Value::String(_) => "String (→blob)",
                        obrain_common::types::Value::Vector(_) => "Vector (→vec)",
                        obrain_common::types::Value::Int64(_) => "Int64",
                        obrain_common::types::Value::Float64(_) => "Float64",
                        obrain_common::types::Value::Bool(_) => "Bool",
                        _ => "Other",
                    };
                    found.push((k, kind));
                }
            }

            println!(
                "  :{:<10} first={:<10} get_node.props.len={} get_node_property-found={}",
                label,
                first.0,
                dashmap_props_count,
                found.len()
            );
            for (k, kind) in &found {
                println!("      {} = {}", k, kind);
            }
        }
    }
}
