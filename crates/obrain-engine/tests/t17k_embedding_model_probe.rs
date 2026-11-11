//! T17k T5 — what embedding_model names are used?
#![cfg(feature = "cypher")]

use obrain_bases_banc::{Base, base_lecture};
use obrain_common::PropertyKey;
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

#[test]
#[ignore = "read-only probe"]
fn probe_embedding_model_names() {
    for (name, base, labels) in [
        ("po", Base::Po, &["File", "Function", "Decision"][..]),
        ("wikipedia", Base::Wikipedia, &["Article"][..]),
        ("megalaw", Base::Megalaw, &["Concept"][..]),
    ] {
        let Some(path) = base_lecture(base) else {
            continue;
        };
        let db = ObrainDB::open_read_only(&path).expect("open");
        let store = db.store();
        println!("\n═══ {name} ═══");
        let model_key = PropertyKey::from("embedding_model");
        let embedded_at_key = PropertyKey::from("embedded_at");
        for label in labels {
            let nodes = store.nodes_by_label(label);
            if nodes.is_empty() {
                continue;
            }
            let mut models: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            let mut dates: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            for nid in nodes.iter().take(1000) {
                if let Some(Value::String(m)) = store.get_node_property(*nid, &model_key) {
                    *models.entry(m.to_string()).or_insert(0) += 1;
                }
                if let Some(Value::String(d)) = store.get_node_property(*nid, &embedded_at_key) {
                    let short = if d.len() >= 10 {
                        d[..10].to_string()
                    } else {
                        d.to_string()
                    };
                    *dates.entry(short).or_insert(0) += 1;
                }
            }
            println!("  :{label} ({} total) embedding_model:", nodes.len());
            for (m, c) in &models {
                println!("    {} × {}", c, m);
            }
            println!("  :{label} embedded_at (day):");
            let mut date_list: Vec<_> = dates.iter().collect();
            date_list.sort();
            for (d, c) in date_list.iter().take(10) {
                println!("    {} × {}", c, d);
            }
        }
    }
}
