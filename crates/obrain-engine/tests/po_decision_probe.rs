//! Deep probe: what properties exist on Decision nodes on PO?
//! Also probe whether vec_columns have ANY data on any base.
#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
#[ignore = "ad-hoc audit — read-only"]
fn probe_decision_and_vec_columns() {
    let home = std::env::var("HOME").unwrap_or_default();
    let po = std::path::PathBuf::from(&home).join(".obrain/db/po");
    let db = ObrainDB::open(&po).expect("open");
    let store = db.store();

    println!("\n=== all_property_keys on PO ===");
    let mut keys: Vec<String> = store.all_property_keys().into_iter().collect();
    keys.sort();
    println!("  total property keys = {}", keys.len());
    for k in &keys {
        println!("    {}", k);
    }

    // For each label, take 3 samples and probe ALL discovered keys
    println!("\n=== Probe Decision nodes with ALL keys ===");
    let decisions = store.nodes_by_label("Decision");
    println!("  Decision count = {}", decisions.len());
    for nid in decisions.iter().take(3) {
        println!("  Decision NodeId={}", nid.0);
        let mut found = 0;
        for k in &keys {
            let pk = obrain_common::PropertyKey::from(k.as_str());
            if let Some(v) = store.get_node_property(*nid, &pk) {
                let preview = format!("{:?}", v);
                let short = &preview[..preview.len().min(80)];
                println!("    {} = {}", k, short);
                found += 1;
            }
        }
        if found == 0 {
            println!("    *** NO PROPERTIES FOUND ***");
        }
    }

    println!("\n=== Probe File with ALL keys (first 2 samples) ===");
    let files = store.nodes_by_label("File");
    for nid in files.iter().take(2) {
        let mut found: Vec<(String, String)> = Vec::new();
        for k in &keys {
            let pk = obrain_common::PropertyKey::from(k.as_str());
            if let Some(v) = store.get_node_property(*nid, &pk) {
                found.push((k.clone(), format!("{:?}", v)));
            }
        }
        println!("  File NodeId={} → {} props found", nid.0, found.len());
        for (k, v) in found.iter().take(20) {
            let short = &v[..v.len().min(60)];
            println!("    {} = {}", k, short);
        }
    }
}
