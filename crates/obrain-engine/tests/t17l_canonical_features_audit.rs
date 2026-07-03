//! T17l — Audit présence des 3 couches canoniques :
//! - `_kernel_embedding` (Φ₀, 80d)
//! - `_hilbert_features` (64-72d)
//! - `_st_embedding` (MiniLM, 384d)
//!
//! Et aussi la clé obsolète `embedding` qui traîne sur PO/Megalaw.
#![cfg(feature = "cypher")]

use obrain_common::PropertyKey;
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

#[test]
#[ignore = "read-only audit on prod bases"]
fn audit_canonical_features_coverage() {
    let home = std::env::var("HOME").unwrap_or_default();
    let canonical_keys = [
        "_kernel_embedding", // expected 80d
        "_hilbert_features", // expected 64-72d
        "_st_embedding",     // expected 384d
    ];
    let legacy_keys = ["embedding"]; // artefact à cartographier

    for (name, path, labels) in [
        (
            "po",
            std::path::PathBuf::from(&home).join(".obrain/db/po"),
            &["File", "Function", "Decision", "Struct", "Note"][..],
        ),
        (
            "wikipedia",
            std::path::PathBuf::from(&home).join(".obrain/db/wikipedia"),
            &["Article"][..],
        ),
        (
            "megalaw",
            std::path::PathBuf::from(&home).join(".obrain/db/megalaw"),
            &["Concept"][..],
        ),
    ] {
        if !path.exists() {
            continue;
        }
        let db = ObrainDB::open(&path).expect("open");
        let store = db.store();
        println!("\n═══ {name} — {} nodes ═══", store.node_count());

        for label in labels {
            let nodes = store.nodes_by_label(label);
            if nodes.is_empty() {
                continue;
            }
            let sample = nodes.len().min(1000);

            println!("\n  :{} ({} total, sample {}):", label, nodes.len(), sample);
            let mut report =
                std::collections::HashMap::<&'static str, (usize, Option<usize>)>::new();

            for key in canonical_keys.iter().chain(legacy_keys.iter()) {
                let pk = PropertyKey::from(*key);
                let mut count = 0usize;
                let mut dim: Option<usize> = None;
                for nid in nodes.iter().take(sample) {
                    if let Some(Value::Vector(v)) = store.get_node_property(*nid, &pk) {
                        count += 1;
                        if dim.is_none() {
                            dim = Some(v.len());
                        }
                    }
                }
                report.insert(key, (count, dim));
            }

            // Pretty table
            for key in canonical_keys.iter().chain(legacy_keys.iter()) {
                let (count, dim) = report[key];
                let pct = (count as f64) * 100.0 / (sample as f64);
                let dim_s = dim.map_or_else(|| "—".into(), |d| format!("dim={}", d));
                let marker = if canonical_keys.contains(key) {
                    "✓ canonical"
                } else {
                    "  legacy   "
                };
                let status = if count == 0 {
                    "❌ MISSING"
                } else if count == sample {
                    "✅ FULL"
                } else {
                    "⚠ partial"
                };
                println!(
                    "    {} {:<22}  {:>4}/{:<4} ({:>5.1}%)  {:<10}  {}",
                    marker, key, count, sample, pct, dim_s, status
                );
            }
        }
    }
}
