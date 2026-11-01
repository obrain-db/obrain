//! bench_cognitive.rs — PREMIER bench réel du layer cognitif Rust d'obrain.
//!
//! Mesure, sur une base réelle :
//!   1. `CALL obrain.cognitive.spread` (spreading activation multi-seeds,
//!      hops=3, causal boost) — la primitive cognitive de projection.
//!   2. Le coût d'overhead du cognitive engine sur une requête chaude
//!      (1-saut) quand le moteur cognitif est construit (avec vs sans).
//!
//! Le stub `bench/substrate/benches/activation.rs` (noop) est remplacé par
//! cette mesure réelle. Usage :
//!   cargo run --release -p obrain-examples --bin bench-cognitive --features "storage cognitive-full" -- <file.obrain> [rep]
use std::path::PathBuf;
use std::time::Instant;

use obrain_engine::ObrainDB;

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
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

    let db = ObrainDB::open(&path)?;
    let session = db.session();

    println!("=== bench-cognitive ===");
    println!("File: {}", path.display());
    println!(
        "Cognitive engine construit: {}",
        db.cognitive_engine().is_some()
    );

    // ── 1. Spreading activation réelle via CALL ──
    // Seeds : 5 fonctions quelconques, hops=3, boost causal 2.0.
    let seeds: Vec<String> = session
        .execute("MATCH (n:Function) RETURN id(n) LIMIT 5")?
        .rows
        .iter()
        .filter_map(|r| r.first())
        .map(|v| v.to_string())
        .collect();
    if seeds.is_empty() {
        eprintln!("⚠ aucune fonction seed trouvée — spread impossible");
    } else {
        let seeds_str = seeds.join(",");
        // Mode causal : propage SUR LE GRAPHE (arêtes CALLS) — c'est le mode
        // qui fonctionne sans synapses pré-bootstrappées, et le plus proche
        // du workload « connaissance » (le graphe EST la mémoire).
        let q_causal = format!(
            "CALL obrain.cognitive.spread([{seeds_str}], {{hops: 3, causal_edge_types: ['CALLS'], causal_boost: 2.0}}) YIELD activated, energy RETURN activated, energy LIMIT 20"
        );
        let r = session.execute(&q_causal)?;
        println!(
            "spread-causal (résultats: {} lignes, seeds={})",
            r.row_count(),
            seeds.len()
        );
        let causal_ms = bench(rep, || {
            let _ = session.execute(&q_causal);
        });
        println!(
            "spread-causal {:6.2} ms   (spreading sur arêtes CALLS, hops=3, 5 seeds)",
            causal_ms
        );

        // Mode synapse : propage sur les synapses existantes (le layer
        // cognitif « pur »). Peut être vide sur une base fraîche sans
        // bootstrap — c'est un constat, pas une erreur.
        let q_syn = format!(
            "CALL obrain.cognitive.spread([{seeds_str}], {{hops: 3}}) YIELD activated, energy RETURN activated, energy LIMIT 20"
        );
        let r = session.execute(&q_syn)?;
        println!(
            "spread-synapse (résultats: {} lignes — synapses existantes)",
            r.row_count()
        );
        let syn_ms = bench(rep, || {
            let _ = session.execute(&q_syn);
        });
        println!("spread-synapse {:5.2} ms   (spreading sur synapses)", syn_ms);
    }

    // ── 2. Overhead du cognitif sur une requête chaude ──
    let q_hot = "MATCH (a:Function)-[:CALLS]->(b:Function) RETURN a.name LIMIT 3";
    let hot_ms = bench(rep, || {
        let _ = session.execute(q_hot);
    });
    println!("1hop     {:8.2} ms   (avec cognitive engine construit)", hot_ms);

    Ok(())
}
