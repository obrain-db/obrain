//! Ad-hoc search in Wikipedia corpus for "Incendie du bar Le Constellation".
//!
//! ```bash
//! cargo test -p obrain-engine --release --features cypher \
//!   --test search_wikipedia_constellation -- --nocapture --ignored
//! ```

#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
#[ignore = "ad-hoc search — read-only on prod base"]
fn search_constellation_bar_fire() {
    let home = std::env::var("HOME").unwrap_or_default();
    let wiki_path = std::path::PathBuf::from(&home).join(".obrain/db/wikipedia");
    if !wiki_path.exists() {
        eprintln!("⏭  Wikipedia not present");
        return;
    }
    let db = ObrainDB::open(&wiki_path).expect("open Wiki");
    let store = db.store();

    println!("\n=== Wiki stats ===");
    println!("  nodes = {}", store.node_count());
    println!("  edges = {}", store.edge_count());

    let labels = store.all_labels();
    let prop_keys: Vec<String> = store.all_property_keys().into_iter().collect();
    println!("  labels = {:?}", &labels[..labels.len().min(20)]);
    println!(
        "  prop_keys (first 20) = {:?}",
        &prop_keys[..prop_keys.len().min(20)]
    );

    // Sample Article title property names — Wiki uses various schemas.
    let articles = store.nodes_by_label("Article");
    println!("\n  Article count = {}", articles.len());

    for (idx, nid) in articles.iter().take(5).enumerate() {
        println!("\n  Sample Article #{} (NodeId={}) properties:", idx, nid.0);
        // First try via get_node
        if let Some(rec) = store.get_node(*nid) {
            if rec.properties.is_empty() {
                println!("    get_node.properties = EMPTY");
            } else {
                for (k, v) in rec.properties.iter() {
                    let display = format!("{:?}", v);
                    let short = if display.len() > 100 {
                        &display[..100]
                    } else {
                        &display
                    };
                    println!("    get_node.{} = {}", k, short);
                }
            }
        } else {
            println!("    get_node = None");
        }
        // Try individual property getters
        for key_str in ["title", "abstract", "name", "url", "lang"] {
            let key = obrain_common::PropertyKey::from(key_str);
            if let Some(v) = store.get_node_property(*nid, &key) {
                let display = format!("{:?}", v);
                let short = if display.len() > 100 {
                    &display[..100]
                } else {
                    &display
                };
                println!("    get_node_property({}) = {}", key_str, short);
            }
        }
    }

    // Direct-API scan (Cypher WHERE on properties is broken on Wiki).
    println!("\n=== Direct-API scan on Article.title ===");
    let key = obrain_common::PropertyKey::from("title");
    let t0 = std::time::Instant::now();
    let mut total = 0u64;
    let mut bar_constellation: Vec<(u64, String)> = Vec::new();
    let mut only_constellation: Vec<(u64, String)> = Vec::new();
    let mut incendie_bar: Vec<(u64, String)> = Vec::new();
    for nid in &articles {
        total += 1;
        if let Some(obrain_common::types::Value::String(title)) =
            store.get_node_property(*nid, &key)
        {
            let t = title.as_str();
            let tl = t.to_lowercase();
            let has_bar = tl.contains(" bar ")
                || tl.starts_with("bar ")
                || tl.ends_with(" bar")
                || tl == "bar"
                || tl.contains(" bar,")
                || tl.contains(" bar-");
            let has_constellation = tl.contains("constellation");
            let has_incendie = tl.contains("incendie");
            if has_constellation && (has_bar || has_incendie) {
                bar_constellation.push((nid.0, t.to_string()));
            } else if has_constellation {
                only_constellation.push((nid.0, t.to_string()));
            } else if has_incendie && has_bar {
                incendie_bar.push((nid.0, t.to_string()));
            }
        }
        if total.is_multiple_of(500_000) {
            println!(
                "  ... scanned {} in {:.1}s",
                total,
                t0.elapsed().as_secs_f64()
            );
        }
    }
    println!(
        "  SCAN DONE {} in {:.2}s",
        total,
        t0.elapsed().as_secs_f64()
    );
    println!(
        "\n  *** Articles with 'constellation' AND ('bar' OR 'incendie') = {} ***",
        bar_constellation.len()
    );
    for (nid, title) in &bar_constellation {
        println!("    NodeId={} title={:?}", nid, title);
    }
    println!(
        "\n  Articles with 'incendie' AND 'bar' (no constellation) = {}",
        incendie_bar.len()
    );
    for (nid, title) in incendie_bar.iter().take(30) {
        println!("    NodeId={} title={:?}", nid, title);
    }
    println!(
        "\n  Articles with 'constellation' alone = {} (showing first 30)",
        only_constellation.len()
    );
    for (nid, title) in only_constellation.iter().take(30) {
        println!("    NodeId={} title={:?}", nid, title);
    }

    // Read the target article in full
    println!("\n=== Target article: NodeId=2542284 ===");
    let target = obrain_common::NodeId(2542284);
    for key_str in [
        "title",
        "abstract",
        "url",
        "lang",
        "word_count",
        "section_count",
        "infobox_type",
    ] {
        let pk = obrain_common::PropertyKey::from(key_str);
        if let Some(v) = store.get_node_property(target, &pk) {
            println!("  {} = {:?}", key_str, v);
        }
    }

    // Check whether Wikipedia has ONNX embeddings for semantic search.
    println!("\n=== _st_embedding coverage check ===");
    let st_key = obrain_common::PropertyKey::from("_st_embedding");
    let mut with_embedding = 0;
    for nid in articles.iter().take(1000) {
        if store.get_node_property(*nid, &st_key).is_some() {
            with_embedding += 1;
        }
    }
    println!(
        "  first 1000 Articles: {} have _st_embedding ({}%)",
        with_embedding,
        with_embedding * 100 / 1000
    );

    let session = db.session();

    println!("\n=== Cypher searches (known broken on Wiki, kept for diagnosis) ===");
    for q in [
        "MATCH (a:Article) WHERE a.title CONTAINS 'Constellation' RETURN a.title LIMIT 20",
        "MATCH (a:Article) WHERE a.title CONTAINS 'constellation' RETURN a.title LIMIT 20",
        "MATCH (a:Article) WHERE a.title CONTAINS 'Incendie' RETURN a.title LIMIT 20",
        "MATCH (a:Article) WHERE a.title CONTAINS 'Le Constellation' RETURN a.title LIMIT 20",
        "MATCH (a:Article) WHERE a.name CONTAINS 'Constellation' RETURN a.name LIMIT 20",
        "MATCH (a:Article) WHERE a.label CONTAINS 'Constellation' RETURN a.label LIMIT 20",
    ] {
        match session.execute_cypher(q) {
            Ok(r) => {
                println!("\n  {}", q);
                println!("    → {} rows", r.rows.len());
                for row in r.rows.iter().take(20) {
                    println!("      {:?}", row);
                }
            }
            Err(e) => println!("\n  {} → ERR: {:?}", q, e),
        }
    }
}
