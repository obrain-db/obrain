//! bench_croise.rs — bench adapté aux bases MCP réelles (sessions/workspace).
//!
//! Les requêtes de bench_head2head ciblent :Function/CALLS (graphes de code).
//! Les bases runtime/obrain-ws ont d'autres labels/arêtes (ToolCall, Message,
//! Step, PRODUCED, PART_OF, HAS_STEP). Ce bench utilise la structure RÉELLE
//! de chaque base pour mesurer le même workload : open, schema, search,
//! 1-saut, triangle (jointure fermée), K4, orderby, distinct + cognitif.
use std::path::PathBuf;
use std::time::Instant;

use obrain_engine::ObrainDB;

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn bench<F: FnMut()>(rep: usize, mut f: F) -> f64 {
    let mut t = Vec::with_capacity(rep);
    for _ in 0..rep {
        let s = Instant::now();
        f();
        t.push(s.elapsed().as_secs_f64() * 1000.0);
    }
    median(&mut t)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = PathBuf::from(&args[1]);
    let rep: usize = args.get(2).map(|s| s.parse().unwrap_or(7)).unwrap_or(7);
    let base = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    // Structure par base : label_noeud, label_result, arête de production,
    // et le pattern de jointure fermée (triangle).
    let (node_label, result_label, produce_edge) = if base.contains("runtime") {
        ("ToolCall", "ToolResult", "PRODUCED")
    } else if base.contains("obrain-ws") {
        ("ToolCall", "ToolResult", "PRODUCED")
    } else {
        ("Function", "Function", "CALLS")
    };

    let db = ObrainDB::open(&path)?;
    let session = db.session();

    println!("=== bench-croise ({base}) ===");
    println!("structure : :{node_label} -[:{produce_edge}]-> :{result_label}");

    let open_ms = bench(rep, || {
        let _ = ObrainDB::open(&path);
    });
    println!("open     {:8.1} ms   (open+close seul)", open_ms);

    // schema : distribution des labels
    let schema_ms = bench(rep, || {
        let _ = session.execute("MATCH (n) RETURN labels(n) AS l, COUNT(*) AS c LIMIT 20");
    });
    println!("schema   {:8.1} ms   (labels distribution)", schema_ms);

    // search : CONTAINS sur name
    let search_ms = bench(rep, || {
        let _ =
            session.execute("MATCH (n) WHERE toLower(n.name) CONTAINS 'x' RETURN n.name LIMIT 10");
    });
    println!("search   {:8.1} ms   (CONTAINS name, LIMIT 10)", search_ms);

    // 1-saut : le vrai edge de production
    let q_1hop = format!(
        "MATCH (a:{node_label})-[:{produce_edge}]->(b:{result_label}) RETURN a.name LIMIT 3"
    );
    let onehop_ms = bench(rep, || {
        let _ = session.execute(&q_1hop);
    });
    println!(
        "1hop     {:8.1} ms   (MATCH 1-saut {produce_edge}, LIMIT 3)",
        onehop_ms
    );

    // triangle : jointure fermée sur le même type
    let q_tri = format!(
        "MATCH (a:{node_label})-[:{produce_edge}]->(b:{result_label})-[:{produce_edge}]->(c:{node_label}) \
         MATCH (c)-[:{produce_edge}]->(a) RETURN a.name LIMIT 3"
    );
    let tri_ms = bench(rep, || {
        let _ = session.execute(&q_tri);
    });
    println!("triangle {:8.1} ms   (2-sauts + jointure fermée)", tri_ms);

    // K4 : 4-sauts + 3 jointures
    let q_k4 = format!(
        "MATCH (a:{node_label})-[:{produce_edge}]->(b:{result_label})-[:{produce_edge}]->(c:{node_label})-[:{produce_edge}]->(d:{result_label}) \
         MATCH (a)-[:{produce_edge}]->(c),(a)-[:{produce_edge}]->(d),(b)-[:{produce_edge}]->(d) RETURN a.name LIMIT 3"
    );
    let k4_ms = bench(rep, || {
        let _ = session.execute(&q_k4);
    });
    println!(
        "k4       {:8.1} ms   (4-sauts + 3 jointures fermées)",
        k4_ms
    );

    // orderby / distinct sur le label dominant
    let q_order = format!("MATCH (n:{node_label}) RETURN n.name ORDER BY n.name DESC LIMIT 50");
    let order_ms = bench(rep, || {
        let _ = session.execute(&q_order);
    });
    println!(
        "orderby  {:8.1} ms   (ORDER BY sur :{node_label}, LIMIT 50)",
        order_ms
    );

    let q_dist = format!("MATCH (n:{node_label}) RETURN DISTINCT n.name");
    let dist_ms = bench(rep, || {
        let _ = session.execute(&q_dist);
    });
    println!(
        "distinct {:8.1} ms   (RETURN DISTINCT sur :{node_label})",
        dist_ms
    );

    // ── Cognitif : bootstrap + spread sur la structure réelle ──
    if db.cognitive_engine().is_some() {
        // seeds : nœuds du label dominant
        let seeds: Vec<String> = session
            .execute(&format!("MATCH (n:{node_label}) RETURN id(n) LIMIT 5"))?
            .rows
            .iter()
            .filter_map(|r| r.first())
            .map(|v| v.to_string())
            .collect();
        if !seeds.is_empty() {
            let seeds_str = seeds.join(",");
            let q_syn = format!(
                "CALL obrain.cognitive.spread([{seeds_str}], {{hops: 3}}) YIELD activated, energy RETURN activated, energy LIMIT 20"
            );
            let r = session.execute(&q_syn)?;
            let syn_ms = bench(rep, || {
                let _ = session.execute(&q_syn);
            });
            println!(
                "spread-synapse {:5.2} ms   ({} lignes, seeds={})",
                syn_ms,
                r.row_count(),
                seeds.len()
            );
        }
    } else {
        println!("(moteur cognitif non construit — feature cognitive-full requise)");
    }

    Ok(())
}
