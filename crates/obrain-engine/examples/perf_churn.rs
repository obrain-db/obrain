//! Banc de churn : le coût de lecture d'un nœud croît-il avec son NOMBRE DE
//! RÉÉCRITURES ?
//!
//! Hypothèse testée (revue moteur 2026-08-17) : `PropsZone` v2 est append-only
//! sans compaction, et `get_latest_for_key` (props_zone.rs:595-630) remonte la
//! chaîne de pages pour appliquer le last-write-wins. Le coût d'une LECTURE de
//! propriété serait donc O(nombre d'écritures HISTORIQUES de ce nœud), et non
//! O(1). Idem pour les arènes `substrate.blobcol.*`, append-only sans GC.
//!
//! C'est le motif exact des singletons d'obrain-tmn — AgentSelfState,
//! ChainStoreSnapshot{singleton:"chains"}, AttributionBaseline — réécrits à
//! chaque tick, et qui coûtent 1,5 à 10,6 s en production alors qu'un nœud
//! écrit UNE fois se lit en ~15 µs.
//!
//! Usage :
//!   cargo run --release -p obrain-engine --example perf_churn -- [n_rewrites] [n_probes]
//!
//! Lecture du résultat :
//!   - latence de lecture PLATE quand le nombre de réécritures monte → hypothèse fausse
//!   - latence qui CROÎT avec le nombre de réécritures → hypothèse confirmée,
//!     et la pente donne le coût par version morte.

use obrain_engine::ObrainDB;
use web_time::Instant;

fn pct(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

/// Mesure la latence de lecture d'un nœud après `rewrites` réécritures.
fn measure_after_rewrites(db: &ObrainDB, key: &str, rewrites: usize, probes: usize) -> (u128, u128) {
    let session = db.session();

    // Phase de churn : on réécrit le MÊME nœud, encore et encore.
    for i in 0..rewrites {
        let q = format!(
            "MERGE (n:Churn {{id: '{key}'}}) SET n.counter = {i}, n.payload = 'v{i}', n.at = {i}"
        );
        session.execute(&q).expect("churn merge");
    }

    // Rodage (sort le nœud du cache CPU froid, pas du cache moteur).
    for _ in 0..5 {
        let _ = session.execute(&format!("MATCH (n:Churn {{id: '{key}'}}) RETURN n.counter"));
    }

    let mut samples = Vec::with_capacity(probes);
    for _ in 0..probes {
        let q = format!("MATCH (n:Churn {{id: '{key}'}}) RETURN n.counter");
        let t = Instant::now();
        let r = session.execute(&q).expect("read");
        samples.push(t.elapsed().as_micros());
        assert_eq!(r.rows.len(), 1, "le nœud doit rester unique après churn");
    }
    samples.sort_unstable();
    (pct(&samples, 0.50), pct(&samples, 0.95))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let max_rewrites: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let probes: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);

    let dir = tempfile::tempdir().expect("tempdir");
    println!("=== banc de churn : lecture vs nombre de réécritures ===");
    println!("base : {}\n", dir.path().display());

    let db = ObrainDB::open_substrate(dir.path()).expect("open");
    db.create_property_index("id");

    // Un peu de fond de base, pour que le nœud ciblé ne soit pas seul au monde.
    {
        let session = db.session();
        let mut stmt = String::new();
        for i in 0..5_000 {
            stmt.push_str(&format!("INSERT (:Filler {{id: 'fill_{i:06}', v: {i}}})\n"));
            if (i + 1) % 500 == 0 {
                session.execute(stmt.trim_end()).expect("fill");
                stmt.clear();
            }
        }
    }

    println!(
        "{:>10}  {:>12}  {:>12}  {:>14}  {:>12}",
        "réécrit.", "lecture p50", "lecture p95", "µs/réécriture", "taille base"
    );
    println!("{}", "-".repeat(70));

    let mut first_p50: Option<f64> = None;
    // Paliers cumulatifs : chaque étape ajoute des réécritures au MÊME nœud.
    let steps = [1usize, 100, 250, 500, 1000, 2000, 4000, 8000];
    let mut done = 0usize;
    for &target in steps.iter() {
        if target > max_rewrites {
            break;
        }
        let delta = target.saturating_sub(done);
        let (p50, p95) = measure_after_rewrites(&db, "hot_singleton", delta, probes);
        done = target;

        // Taille sur disque : révèle l'accumulation append-only.
        let size: u64 = std::fs::read_dir(dir.path())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0);

        if first_p50.is_none() {
            first_p50 = Some(p50 as f64);
        }
        let per_rewrite = if done > 0 {
            (p50 as f64 - first_p50.unwrap()) / done as f64
        } else {
            0.0
        };
        println!(
            "{done:>10}  {p50:>9} µs  {p95:>9} µs  {per_rewrite:>13.3}  {:>9.1} Mo",
            size as f64 / 1_048_576.0
        );
    }

    println!("\nVERDICT :");
    println!("  latence PLATE  → PropsZone/blobcol ne sont pas en cause sur ce chemin.");
    println!("  latence QUI CROÎT → chaîne append-only confirmée ; la colonne");
    println!("  « µs/réécriture » donne le coût unitaire d'une version morte.");

    std::mem::drop(db);
    let _ = dir.close();
}
