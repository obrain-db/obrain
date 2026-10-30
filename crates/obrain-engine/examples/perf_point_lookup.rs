//! Banc de non-régression : lookup de point sur clé indexée, et concurrence de lecture.
//!
//! Reproduit la pathologie de production mesurée le 2026-08-17 sur obrain-tmn :
//! `MERGE (n:Label {id: "…"})` et `MATCH (n:Label {id: "…"}) RETURN …` coûtent
//! 1,5 à 10,6 s alors que `id` EST indexé, parce que le planificateur matérialise
//! l'intégralité du label pour intersecter (filter.rs) via `nodes_by_label`, qui
//! balaie `1..high_water` en prenant le mutex de zone à chaque slot.
//!
//! Usage :
//!   cargo run --release -p obrain-engine --example perf_point_lookup -- [n_nodes] [n_probes] [threads]
//!
//! Ce que le banc doit montrer une fois le moteur corrigé :
//!   - latence de lookup INDÉPENDANTE de `n_nodes` (O(k), pas O(high_water))
//!   - débit qui MONTE avec le nombre de threads (lectures concurrentes)

use obrain_engine::ObrainDB;
use std::sync::Arc;
use web_time::Instant;
use std::time::Duration;

fn pct(sorted_us: &[u128], p: f64) -> u128 {
    if sorted_us.is_empty() {
        return 0;
    }
    let idx = ((sorted_us.len() as f64 - 1.0) * p).round() as usize;
    sorted_us[idx]
}

fn summarize(label: &str, mut samples_us: Vec<u128>) {
    samples_us.sort_unstable();
    let n = samples_us.len();
    let sum: u128 = samples_us.iter().sum();
    println!(
        "  {label:<34} n={n:<6} moy={:>9.1} µs  p50={:>9} µs  p95={:>9} µs  max={:>10} µs",
        sum as f64 / n.max(1) as f64,
        pct(&samples_us, 0.50),
        pct(&samples_us, 0.95),
        samples_us.last().copied().unwrap_or(0),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n_nodes: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let n_probes: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200);
    let threads: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);

    let dir = tempfile::tempdir().expect("tempdir");
    println!("=== banc lookup de point ===");
    println!("nœuds={n_nodes}  sondes={n_probes}  threads={threads}");
    println!("base : {}", dir.path().display());

    let db = ObrainDB::open_substrate(dir.path()).expect("open");

    // ---- Remplissage -----------------------------------------------------
    // Un label unique, une clé primaire textuelle : la forme exacte utilisée
    // par obrain-tmn pour AgentDirective / SupervisorObjective / etc.
    let t0 = Instant::now();
    {
        let session = db.session();
        const BATCH: usize = 500;
        let mut stmt = String::new();
        for i in 0..n_nodes {
            stmt.push_str(&format!(
                "INSERT (:Obj {{id: 'obj_{i:08}', v: {i}, singleton: 'obj_{i:08}'}})\n"
            ));
            if (i + 1) % BATCH == 0 || i + 1 == n_nodes {
                session.execute(stmt.trim_end()).expect("insert batch");
                stmt.clear();
            }
        }
    }
    let fill = t0.elapsed();
    println!(
        "remplissage : {n_nodes} nœuds en {:.2} s ({:.0} nœuds/s)",
        fill.as_secs_f64(),
        n_nodes as f64 / fill.as_secs_f64().max(1e-9)
    );

    // ---- Index de propriété ---------------------------------------------
    // Exactement ce que fait obrain-tmn à l'ouverture (obrain_db_pool.rs).
    let t0 = Instant::now();
    db.create_property_index("id");
    db.create_property_index("singleton");
    println!("create_property_index x2 : {:.2} s", t0.elapsed().as_secs_f64());

    // Cibles réparties sur tout l'espace de clés (évite un biais de localité).
    let targets: Vec<String> = (0..n_probes)
        .map(|k| format!("obj_{:08}", (k * n_nodes.max(1) / n_probes.max(1)) % n_nodes.max(1)))
        .collect();

    // ---- A. MATCH par clé indexée (mono-thread) -------------------------
    println!("\n-- mono-thread --");
    {
        let session = db.session();
        // rodage
        for t in targets.iter().take(10) {
            let _ = session.execute(&format!("MATCH (n:Obj {{id: '{t}'}}) RETURN n.v"));
        }
        let mut samples = Vec::with_capacity(n_probes);
        for t in &targets {
            let q = format!("MATCH (n:Obj {{id: '{t}'}}) RETURN n.v");
            let start = Instant::now();
            let r = session.execute(&q).expect("match");
            samples.push(start.elapsed().as_micros());
            debug_assert_eq!(r.rows.len(), 1, "le lookup doit rendre exactement 1 ligne");
        }
        summarize("MATCH (n:Obj {id}) RETURN n.v", samples);
    }

    // ---- B. MATCH par clé indexée SANS label ----------------------------
    // Isole la contribution de l'intersection de label : même sonde d'index,
    // mais sans `nodes_by_label`. L'écart A - B est le coût du balayage.
    {
        let session = db.session();
        let mut samples = Vec::with_capacity(n_probes);
        for t in &targets {
            let q = format!("MATCH (n {{id: '{t}'}}) RETURN n.v");
            let start = Instant::now();
            let _ = session.execute(&q).expect("match sans label");
            samples.push(start.elapsed().as_micros());
        }
        summarize("MATCH (n {id}) RETURN n.v  [sans label]", samples);
    }

    // ---- C. MERGE par clé indexée (chemin d'écriture chaud) -------------
    {
        let session = db.session();
        let mut samples = Vec::with_capacity(n_probes.min(100));
        for t in targets.iter().take(n_probes.min(100)) {
            let q = format!("MERGE (n:Obj {{id: '{t}'}}) SET n.touched = 1");
            let start = Instant::now();
            let _ = session.execute(&q).expect("merge");
            samples.push(start.elapsed().as_micros());
        }
        summarize("MERGE (n:Obj {id}) SET n.touched", samples);
    }

    // ---- D. Concurrence de lecture --------------------------------------
    // Le test décisif pour « gérer beaucoup de concurrence » : le débit
    // agrégé doit MONTER avec le nombre de threads. S'il stagne, les lectures
    // sont sérialisées par un verrou exclusif.
    println!("\n-- concurrence de lecture (débit agrégé) --");
    let db = Arc::new(db);
    let mut baseline = 0.0f64;
    for &nt in &[1usize, 2, 4, threads] {
        if nt > threads && nt != 1 {
            continue;
        }
        let per_thread = (n_probes / 2).max(20);
        let start = Instant::now();
        let mut handles = Vec::new();
        for tid in 0..nt {
            let db = Arc::clone(&db);
            let targets = targets.clone();
            handles.push(std::thread::spawn(move || {
                let session = db.session();
                for i in 0..per_thread {
                    let t = &targets[(i + tid * 7) % targets.len()];
                    let _ = session.execute(&format!("MATCH (n:Obj {{id: '{t}'}}) RETURN n.v"));
                }
            }));
        }
        for h in handles {
            h.join().expect("join");
        }
        let elapsed = start.elapsed();
        let total = (nt * per_thread) as f64;
        let qps = total / elapsed.as_secs_f64().max(1e-9);
        if nt == 1 {
            baseline = qps;
        }
        println!(
            "  {nt:>3} thread(s) : {qps:>10.1} req/s   (accélération ×{:.2} vs 1 thread)",
            qps / baseline.max(1e-9)
        );
    }

    // ---- E. Sensibilité à la taille de la base --------------------------
    // Un lookup indexé DOIT être insensible au nombre de nœuds. Ce chiffre
    // est le vrai juge : on le compare entre deux tailles de base.
    println!("\nRÉFÉRENCE À RETENIR : p50 du MATCH indexé ci-dessus, à comparer");
    println!("entre n_nodes=50000 et n_nodes=200000. Un moteur sain donne le");
    println!("MÊME chiffre ; un balayage O(high_water) le multiplie par 4.");

    std::mem::drop(db);
    let _ = dir.close();
}

fn _unused(_: Duration) {}
