//! T17k T5 — réel audit de coverage embeddings sur les 3 bases.
//! Probe TOUTES les clés enregistrées dans prop_keys + pour chaque clé,
//! vérifie le type (Vector vs autre) et mesure le % de nodes qui l'ont.
#![cfg(feature = "cypher")]

use obrain_common::PropertyKey;
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

#[test]
#[ignore = "ad-hoc audit — read-only on prod bases"]
fn audit_embedding_coverage_on_all_bases() {
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
        println!("\n═══ {name} — {} nodes ═══", store.node_count());

        // 1. List ALL property keys
        let keys: Vec<String> = store.all_property_keys().into_iter().collect();
        println!("  all_property_keys total = {}", keys.len());

        // 2. For each label (Article / File / Function / Decision / Concept), sample 1000
        //    nodes and compute coverage per key. Also track which are Vector vs scalar.
        for label in ["Article", "File", "Function", "Decision", "Concept"] {
            let nodes = store.nodes_by_label(label);
            if nodes.is_empty() {
                continue;
            }
            let sample_size = nodes.len().min(1000);
            println!(
                "\n  :{} ({} total, sampling {}):",
                label,
                nodes.len(),
                sample_size
            );

            // For each key, count how many sampled nodes have it + note Vector dim
            let mut per_key_counts: std::collections::HashMap<
                String,
                (usize, Option<(String, usize)>),
            > = std::collections::HashMap::new();

            for nid in nodes.iter().take(sample_size) {
                for k in &keys {
                    let pk = PropertyKey::from(k.as_str());
                    if let Some(v) = store.get_node_property(*nid, &pk) {
                        let entry = per_key_counts.entry(k.clone()).or_insert((0, None));
                        entry.0 += 1;
                        if entry.1.is_none() {
                            let kind_dim = match &v {
                                Value::Vector(vec) => ("Vector".to_string(), vec.len()),
                                Value::String(_) => ("String".to_string(), 0),
                                Value::Int64(_) => ("Int64".to_string(), 0),
                                Value::Float64(_) => ("Float64".to_string(), 0),
                                Value::Bool(_) => ("Bool".to_string(), 0),
                                Value::Null => ("Null".to_string(), 0),
                                _ => ("Other".to_string(), 0),
                            };
                            entry.1 = Some(kind_dim);
                        }
                    }
                }
            }

            // Sort by coverage descending. Highlight Vector-typed keys.
            let mut rows: Vec<(String, usize, Option<(String, usize)>)> = per_key_counts
                .into_iter()
                .map(|(k, (c, kind))| (k, c, kind))
                .collect();
            rows.sort_by(|a, b| b.1.cmp(&a.1));
            for (k, count, kind) in rows.iter().take(20) {
                let pct = (*count as f64) * 100.0 / (sample_size as f64);
                let kind_str = match kind {
                    Some((kind_name, dim)) if kind_name == "Vector" => {
                        format!("{}[{}]", kind_name, dim)
                    }
                    Some((kind_name, _)) => kind_name.clone(),
                    None => "?".to_string(),
                };
                let marker = if let Some((k, _)) = kind {
                    if k == "Vector" { " ⭐" } else { "" }
                } else {
                    ""
                };
                println!(
                    "      {:<30}  {:>4}/{:<4}  ({:>5.1}%)  {}{}",
                    k, count, sample_size, pct, kind_str, marker
                );
            }
        }
    }
}
