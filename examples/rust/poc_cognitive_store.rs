//! poc_cognitive_store.rs — valide la couche cognitive sur un STORE RÉEL.
//!
//! Copie le store (jamais l'original), l'ouvre avec le moteur cognitif
//! (synapses + énergie), lance UN tick HilbertEnricher (64-72d) puis mesure
//! la flotte standard de thinkers sur un intervalle court. Sorties :
//!   - open + cognitive engine construit
//!   - couverture _hilbert_features avant/après le tick
//!   - temps du tick (compute + persist)
//!   - spread (CALL obrain.cognitive.spread, mode causal) — la primitive
//!     cognitive de projection sur le graphe réel
//!
//! Usage :
//!   cargo run --release -p obrain-examples --bin poc-cognitive-store \
//!     --features "storage cognitive-full enrichment" -- <file.obrain> [rep]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use obrain_cognitive::thinker::{
    HilbertEnricher, HilbertEnricherConfig, NeverOverloadedSensor, Thinker, ThinkersConfig,
    spawn_standard_fleet,
};
use obrain_common::types::{PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_engine::ObrainDB;
use obrain_substrate::SubstrateStore;

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

fn sample_coverage(store: &Arc<SubstrateStore>, key: &PropertyKey, sample_n: usize) -> f64 {
    let ids = store.node_ids();
    let n = ids.len().min(sample_n);
    if n == 0 {
        return 0.0;
    }
    let mut hits = 0usize;
    for nid in ids.iter().take(n) {
        if let Some(Value::Vector(_)) = store.get_node_property(*nid, key) {
            hits += 1;
        }
    }
    (hits as f64) / (n as f64)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let src = PathBuf::from(&args[1]);
    let rep: usize = args.get(2).map(|s| s.parse().unwrap_or(5)).unwrap_or(5);

    // ── Copie du store (on ne mute JAMAIS l'original) ──
    let dst = std::env::temp_dir().join("poc-cognitive-copy.obrain");
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    copy_dir(&src, &dst)?;
    println!("store copié : {} → {}", src.display(), dst.display());

    // ── 1. Open + moteur cognitif (synapses + énergie) ──
    let t_open = Instant::now();
    let db = ObrainDB::open(&dst)?;
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    let session = db.session();
    println!("\n=== POC couche cognitive — store réel ===");
    println!("open           : {:.1} ms", open_ms);
    println!("cognitive      : {}", db.cognitive_engine().is_some());

    // ── 2. Substrate + HilbertEnricher (tick complet) ──
    let store = Arc::new(SubstrateStore::open(&dst)?);
    let nodes: usize = store.node_count();
    let edges: usize = store.edge_count();
    println!("nœuds          : {}", nodes);
    println!("arêtes         : {}", edges);

    let key = PropertyKey::from("_hilbert_features");
    let before = sample_coverage(&store, &key, 1000);
    println!(
        "\n_hilbert_features coverage AVANT : {:.1}% (sample 1000)",
        before * 100.0
    );

    let enricher = HilbertEnricher::new(HilbertEnricherConfig::default());
    let t_tick = Instant::now();
    let report = enricher.tick(&store)?;
    let tick_ms = t_tick.elapsed().as_secs_f64() * 1000.0;
    let after = sample_coverage(&store, &key, 1000);
    println!(
        "tick Hilbert   : {:.1} ms (nodes_touched={})",
        tick_ms, report.nodes_touched
    );
    println!(
        "_hilbert_features coverage APRÈS : {:.1}% (sample 1000)",
        after * 100.0
    );

    // ── 3. Flotte standard de thinkers (intervalle court) ──
    let mut tcfg = ThinkersConfig {
        min_tick_interval_secs: 1,
        ..Default::default()
    };
    tcfg.recompute_runtime();
    tcfg.runtime.min_tick_interval = std::time::Duration::from_millis(50);
    tcfg.consolidator.inner_toml.interval_secs = Some(1);
    tcfg.warden.inner_toml.interval_secs = Some(1);
    tcfg.predictor.inner_toml.interval_secs = Some(1);
    tcfg.dreamer.inner_toml.interval_secs = Some(1);
    let sensor = Arc::new(NeverOverloadedSensor);
    let rt = spawn_standard_fleet(store.clone(), &tcfg, sensor);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    rt.shutdown();
    println!("\nflotte thinkers : 4 thinkers actifs 1,5 s (consolidator/warden/predictor/dreamer)");

    // ── 4. Spread (primitive cognitive de projection) ──
    let seeds: Vec<String> = session
        .execute("MATCH (n:Message) RETURN id(n) LIMIT 3")?
        .rows
        .iter()
        .filter_map(|r| r.first())
        .map(|v| v.to_string())
        .collect();
    if seeds.is_empty() {
        println!("\n⚠ aucune seed Message — spread impossible");
    } else {
        let q = format!(
            "CALL obrain.cognitive.spread([{}], {{hops: 2, causal_edge_types: ['INVOKED','PRODUCED'], causal_boost: 2.0}}) YIELD activated, energy RETURN activated, energy LIMIT 20",
            seeds.join(",")
        );
        let r = session.execute(&q)?;
        println!(
            "\nspread-causal (INVOKED/PRODUCED, hops=2) : {} lignes retournées",
            r.row_count()
        );
        let spread_ms = bench(rep, || {
            let _ = session.execute(&q);
        });
        println!("spread-causal  : {:.1} ms (médiane {})", spread_ms, rep);
    }

    // ── 5. Synapses présentes (le store en avait déjà) ──
    let syn = store
        .edge_counts_by_type()
        .map(|m| m.get("SYNAPSE").copied().unwrap_or(0))
        .unwrap_or(0);
    println!("synapses       : {}", syn);

    println!("\nPOC terminé — store de test : {}", dst.display());
    Ok(())
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}
