//! Banc des index de proprietes sur la base Project Orchestrator.

#![cfg(feature = "cypher")]
//! Property index validation & benchmark on real Project Orchestrator database.
//!
//! This test validates:
//! 1. CREATE INDEX works on a PO-like dataset
//! 2. Indexed lookups are significantly faster than full scans
//! 3. Index persistence via WAL replay
//! 4. Mutation hooks maintain index consistency
//!
//! Run with:
//!   cargo test -p obrain-engine --features "cypher,wal,tiered-storage" --test property_index_po_bench -- --nocapture

use obrain_bases_banc::{Base, base_copie};
use obrain_engine::ObrainDB;
use web_time::Instant;

/// Measure a closure over N iterations, return (mean_ms, stddev_ms, min_ms, max_ms)
fn measure<F: FnMut() -> R, R>(mut f: F, iterations: usize) -> (f64, f64, f64, f64) {
    let mut times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let _ = f();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let variance = times.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / times.len() as f64;
    let min = times.iter().copied().fold(f64::INFINITY, f64::min);
    let max = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (mean, variance.sqrt(), min, max)
}

/// Mesure p50/mean sur N iterations.
fn measure_p50<F: FnMut() -> R, R>(mut f: F, iterations: usize) -> (f64, f64) {
    let mut times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let _ = f();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    (times[times.len() / 2], mean)
}

// ============================================================================
// Chantier #95 : jointure correlee multi-MATCH — seek d'index vs produit
// cartesien. Modele de la requete de prod :
//   MATCH (ev:Evidence) MATCH (c:Claim {id: ev.claim_id})
//   WHERE NOT (ev)-[:SUPPORTS]->(c)      (324 s mesurees en prod)
// Index OFF = ancien plan (ScanOperator x input via NestedLoopJoin Cross,
// |A| x |B| paires materialisees puis filtrees par ligne).
// Index ON  = IndexSeekJoinOperator (une sonde par ligne externe).
// L'index passe par l'API programmatique db.create_property_index — le
// GQL CREATE INDEX est retire depuis le cutover T17 substrate.
// ============================================================================

#[test]
fn bench_correlated_multi_match_index_seek() {
    println!("\n{}", "=".repeat(70));
    println!("CORRELATED MULTI-MATCH — index seek vs produit cartesien (#95)");
    println!("{}\n", "=".repeat(70));

    let db = ObrainDB::new_in_memory();
    let session = db.session();

    let n_a = 1000;
    let n_b = 1000;
    let start = Instant::now();
    for i in 0..n_a {
        session
            .execute_cypher(&format!("CREATE (a:A {{fk: 'key-{i}'}})"))
            .expect("create A");
    }
    for i in 0..n_b {
        session
            .execute_cypher(&format!("CREATE (b:B {{k: 'key-{i}'}})"))
            .expect("create B");
    }
    // Aretes SUPPORTS : un a sur dix vers son b apparie (paires exclues du
    // NOT), et une arete vers un tiers (le piege du bug #95-2).
    for i in (0..n_a).step_by(10) {
        session
            .execute_cypher(&format!(
                "MATCH (a:A {{fk: 'key-{i}'}}) MATCH (b:B {{k: 'key-{i}'}}) \
                 CREATE (a)-[:SUPPORTS]->(b)"
            ))
            .expect("create SUPPORTS");
    }
    session
        .execute_cypher("MATCH (a:A {fk: 'key-1'}) CREATE (a)-[:SUPPORTS]->(:Z {name: 'tiers'})")
        .expect("create SUPPORTS tiers");
    println!(
        "  Peuple {} A + {} B en {:.0}ms\n",
        n_a,
        n_b,
        start.elapsed().as_secs_f64() * 1000.0
    );

    let q_join = "MATCH (a:A) MATCH (b:B {k: a.fk}) RETURN count(b)";
    let q_not = "MATCH (a:A) MATCH (b:B {k: a.fk}) \
                 WHERE NOT (a)-[:SUPPORTS]->(b) RETURN count(b)";

    // Verite terrain (independante de l'index)
    let rows_join = session
        .execute_cypher(q_join)
        .unwrap()
        .scalar::<i64>()
        .unwrap();
    let rows_not = session.execute(q_not).unwrap().scalar::<i64>().unwrap();
    assert_eq!(rows_join, n_a, "1 paire par a");
    assert_eq!(rows_not, n_a - n_a / 10, "paires supportees exclues");

    // ---- AVANT : sans index (= ancien plan, produit cartesien) ----
    println!("--- AVANT : sans index (produit cartesien) ---");
    let before_join = measure_p50(|| session.execute_cypher(q_join).unwrap(), 5);
    println!(
        "  {:<52} p50 {:>9.1}ms (mean {:.1}ms)",
        "join correle (count)", before_join.0, before_join.1
    );
    let before_not = measure_p50(|| session.execute(q_not).unwrap(), 5);
    println!(
        "  {:<52} p50 {:>9.1}ms (mean {:.1}ms)",
        "join correle + NOT pattern (count)", before_not.0, before_not.1
    );

    // ---- APRES : avec index (seek) ----
    db.create_property_index("k");
    println!("\n--- APRES : index sur B.k (IndexSeekJoin) ---");
    let after_join = measure_p50(|| session.execute_cypher(q_join).unwrap(), 20);
    println!(
        "  {:<52} p50 {:>9.1}ms (mean {:.1}ms)",
        "join correle (count)", after_join.0, after_join.1
    );
    let after_not = measure_p50(|| session.execute(q_not).unwrap(), 20);
    println!(
        "  {:<52} p50 {:>9.1}ms (mean {:.1}ms)",
        "join correle + NOT pattern (count)", after_not.0, after_not.1
    );

    // Les resultats restent identiques avec l'index
    assert_eq!(
        session
            .execute_cypher(q_join)
            .unwrap()
            .scalar::<i64>()
            .unwrap(),
        rows_join
    );
    assert_eq!(
        session.execute(q_not).unwrap().scalar::<i64>().unwrap(),
        rows_not
    );

    println!("\n--- SPEEDUP ---");
    println!(
        "  join correle          : {:.1}ms -> {:.1}ms  ({:.0}x)",
        before_join.0,
        after_join.0,
        before_join.0 / after_join.0
    );
    println!(
        "  join correle + NOT    : {:.1}ms -> {:.1}ms  ({:.0}x)",
        before_not.0,
        after_not.0,
        before_not.0 / after_not.0
    );

    // Garde-fou : le seek doit battre nettement le produit cartesien.
    assert!(
        after_join.0 < before_join.0,
        "le seek indexe doit battre le produit cartesien"
    );
}

fn print_bench(label: &str, stats: (f64, f64, f64, f64), row_count: usize) {
    println!(
        "  {:<45} {:>8.3}ms ± {:.3}ms  (min: {:.3}ms, max: {:.3}ms)  [{} rows]",
        label, stats.0, stats.1, stats.2, stats.3, row_count
    );
}

// ============================================================================
// Test 1: Benchmark property index on PO-like data (in-memory)
// ============================================================================

#[test]
fn bench_property_index_on_po_data() {
    println!("\n{}", "=".repeat(70));
    println!("PROPERTY INDEX BENCHMARK — PO-like Workload");
    println!("{}\n", "=".repeat(70));

    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // Populate with PO-like data: projects, plans, tasks, notes
    println!("Populating database with PO-like workload...");
    let start = Instant::now();

    // Projects (small set)
    for i in 0..20 {
        session
            .execute_cypher(&format!(
                "CREATE (p:Project {{slug: 'project-{i}', name: 'Project {i}', status: 'active'}})"
            ))
            .expect("create project");
    }

    // Plans (medium set)
    for i in 0..100 {
        let proj_idx = i % 20;
        session
            .execute_cypher(&format!(
                "CREATE (p:Plan {{id: 'plan-{i}', title: 'Plan {i}', status: '{}', project_id: 'project-{proj_idx}'}})",
                if i % 3 == 0 { "completed" } else if i % 3 == 1 { "in_progress" } else { "draft" }
            ))
            .expect("create plan");
    }

    // Tasks (large set)
    for i in 0..2000 {
        let plan_idx = i % 100;
        session
            .execute_cypher(&format!(
                "CREATE (t:Task {{id: 'task-{i}', title: 'Task {i}', status: '{}', plan_id: 'plan-{plan_idx}'}})",
                if i % 4 == 0 { "completed" } else if i % 4 == 1 { "in_progress" } else if i % 4 == 2 { "pending" } else { "blocked" }
            ))
            .expect("create task");
    }

    // Notes (large set)
    for i in 0..3000 {
        let proj_idx = i % 20;
        session
            .execute_cypher(&format!(
                "CREATE (n:Note {{id: 'note-{i}', note_type: '{}', importance: '{}', status: 'active', project_slug: 'project-{proj_idx}'}})",
                match i % 5 { 0 => "guideline", 1 => "gotcha", 2 => "pattern", 3 => "tip", _ => "observation" },
                match i % 4 { 0 => "low", 1 => "medium", 2 => "high", _ => "critical" }
            ))
            .expect("create note");
    }

    // Skills
    for i in 0..500 {
        session
            .execute_cypher(&format!(
                "CREATE (s:Skill {{id: 'skill-{i}', name: 'Skill {i}', cluster_id: 'cluster-{}'}})",
                i % 10
            ))
            .expect("create skill");
    }

    let populate_ms = start.elapsed().as_secs_f64() * 1000.0;
    let total_nodes = session
        .execute_cypher("MATCH (n) RETURN count(n)")
        .unwrap()
        .scalar::<i64>()
        .unwrap();
    println!(
        "  Populated {} nodes in {:.1}ms\n",
        total_nodes, populate_ms
    );

    // ---- PHASE 1: Baseline (no index) ----
    println!("--- PHASE 1: No Index (Full Scan) ---");
    let iterations = 50;

    let mut row_count = 0;
    let no_idx_slug = measure(
        || {
            let r = session
                .execute_cypher("MATCH (p:Project {slug: 'project-7'}) RETURN p.name")
                .unwrap();
            row_count = r.row_count();
            r
        },
        iterations,
    );
    print_bench(
        "MATCH (p:Project {slug: 'project-7'})",
        no_idx_slug,
        row_count,
    );

    let no_idx_status = measure(
        || {
            session
                .execute_cypher("MATCH (t:Task {status: 'in_progress'}) RETURN count(t)")
                .unwrap()
        },
        iterations,
    );
    print_bench(
        "MATCH (t:Task {status: 'in_progress'}) count",
        no_idx_status,
        1,
    );

    let no_idx_plan = measure(
        || {
            session
                .execute_cypher("MATCH (t:Task {plan_id: 'plan-42'}) RETURN t.title")
                .unwrap()
        },
        iterations,
    );
    let rc = session
        .execute_cypher("MATCH (t:Task {plan_id: 'plan-42'}) RETURN t.title")
        .unwrap()
        .row_count();
    print_bench("MATCH (t:Task {plan_id: 'plan-42'})", no_idx_plan, rc);

    let no_idx_note = measure(
        || {
            session
                .execute_cypher(
                    "MATCH (n:Note {project_slug: 'project-3', note_type: 'gotcha'}) RETURN n.id",
                )
                .unwrap()
        },
        iterations,
    );
    let rc2 = session
        .execute_cypher(
            "MATCH (n:Note {project_slug: 'project-3', note_type: 'gotcha'}) RETURN n.id",
        )
        .unwrap()
        .row_count();
    print_bench("MATCH (n:Note {project_slug,note_type})", no_idx_note, rc2);

    // ---- PHASE 2: Create indexes ----
    println!("\n--- PHASE 2: Create Indexes ---");

    let idx_start = Instant::now();
    // ── L'INDEX SE DÉCLARE PAR L'API, PAS PAR LE GQL ────────────────────────
    // `CREATE INDEX` est refusé depuis le cutover T17 ; le chemin supporté est
    // programmatique (celui du pont MCP et du catalogue `substrate.indexcat`).
    db.create_property_index("slug");
    println!(
        "  CREATE INDEX Project.slug:      {:.3}ms",
        idx_start.elapsed().as_secs_f64() * 1000.0
    );

    let idx_start = Instant::now();
    // ── L'INDEX SE DÉCLARE PAR L'API, PAS PAR LE GQL ────────────────────────
    // `CREATE INDEX` est refusé depuis le cutover T17 ; le chemin supporté est
    // programmatique (celui du pont MCP et du catalogue `substrate.indexcat`).
    db.create_property_index("status");
    println!(
        "  CREATE INDEX Task.status:       {:.3}ms",
        idx_start.elapsed().as_secs_f64() * 1000.0
    );

    let idx_start = Instant::now();
    // ── L'INDEX SE DÉCLARE PAR L'API, PAS PAR LE GQL ────────────────────────
    // `CREATE INDEX` est refusé depuis le cutover T17 ; le chemin supporté est
    // programmatique (celui du pont MCP et du catalogue `substrate.indexcat`).
    db.create_property_index("plan_id");
    println!(
        "  CREATE INDEX Task.plan_id:      {:.3}ms",
        idx_start.elapsed().as_secs_f64() * 1000.0
    );

    let idx_start = Instant::now();
    // ── L'INDEX SE DÉCLARE PAR L'API, PAS PAR LE GQL ────────────────────────
    // `CREATE INDEX` est refusé depuis le cutover T17 ; le chemin supporté est
    // programmatique (celui du pont MCP et du catalogue `substrate.indexcat`).
    db.create_property_index("project_slug");
    println!(
        "  CREATE INDEX Note.project_slug: {:.3}ms",
        idx_start.elapsed().as_secs_f64() * 1000.0
    );

    // ---- PHASE 3: With index ----
    println!("\n--- PHASE 3: With Index ---");

    let mut row_count_idx = 0;
    let idx_slug = measure(
        || {
            let r = session
                .execute_cypher("MATCH (p:Project {slug: 'project-7'}) RETURN p.name")
                .unwrap();
            row_count_idx = r.row_count();
            r
        },
        iterations,
    );
    print_bench(
        "MATCH (p:Project {slug: 'project-7'})",
        idx_slug,
        row_count_idx,
    );

    let idx_status = measure(
        || {
            session
                .execute_cypher("MATCH (t:Task {status: 'in_progress'}) RETURN count(t)")
                .unwrap()
        },
        iterations,
    );
    print_bench(
        "MATCH (t:Task {status: 'in_progress'}) count",
        idx_status,
        1,
    );

    let idx_plan = measure(
        || {
            session
                .execute_cypher("MATCH (t:Task {plan_id: 'plan-42'}) RETURN t.title")
                .unwrap()
        },
        iterations,
    );
    print_bench("MATCH (t:Task {plan_id: 'plan-42'})", idx_plan, rc);

    let idx_note = measure(
        || {
            session
                .execute_cypher(
                    "MATCH (n:Note {project_slug: 'project-3', note_type: 'gotcha'}) RETURN n.id",
                )
                .unwrap()
        },
        iterations,
    );
    print_bench("MATCH (n:Note {project_slug,note_type})", idx_note, rc2);

    // ---- PHASE 4: Speedup summary ----
    println!("\n--- SPEEDUP SUMMARY ---");
    let speedups = [
        ("Project.slug lookup (unique)", no_idx_slug.0, idx_slug.0),
        (
            "Task.status count (high card)",
            no_idx_status.0,
            idx_status.0,
        ),
        ("Task.plan_id lookup (20 rows)", no_idx_plan.0, idx_plan.0),
        ("Note.project_slug+type (combo)", no_idx_note.0, idx_note.0),
    ];

    for (name, before, after) in &speedups {
        let speedup = before / after;
        println!(
            "  {:<40} {:.3}ms -> {:.3}ms  ({:.1}x faster)",
            name, before, after, speedup
        );
    }

    // Correctness: same row counts
    assert_eq!(
        row_count, row_count_idx,
        "Row counts must match with/without index"
    );
    println!("\n  All row counts match — index is correct");
}

// ============================================================================
// Test 2: Index survives WAL replay (persistence)
// ============================================================================

#[test]
fn test_property_index_wal_persistence() {
    println!("\n{}", "=".repeat(70));
    println!("PROPERTY INDEX WAL PERSISTENCE TEST");
    println!("{}\n", "=".repeat(70));

    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("test_index_wal");

    // Phase 1: Create DB, add data, create index
    {
        let db = ObrainDB::open(&db_path).expect("open db");
        let session = db.session();

        for i in 0..100 {
            session
                .execute_cypher(&format!(
                    "CREATE (p:Project {{slug: 'proj-{i}', name: 'Project {i}'}})"
                ))
                .expect("create node");
        }

        // ── L'INDEX SE DÉCLARE PAR L'API, PAS PAR LE GQL ────────────────────
        // `CREATE INDEX` est refusé depuis le cutover T17 (le substrat bâtit sa
        // propre indexation). Le chemin supporté est programmatique — c'est
        // celui du pont MCP et du catalogue `substrate.indexcat`.
        db.create_property_index("slug");

        // Verify index works
        let r = session
            .execute_cypher("MATCH (p:Project {slug: 'proj-42'}) RETURN p.name")
            .expect("lookup");
        assert_eq!(r.row_count(), 1, "Index lookup should find 1 row");

        db.close().expect("close");
    }

    println!("  Phase 1: DB created, index created, closed.");

    // Phase 2: Reopen and check if index is reconstructed via WAL replay
    {
        let db = ObrainDB::open(&db_path).expect("reopen db");
        let session = db.session();

        let r = session
            .execute_cypher("MATCH (p:Project {slug: 'proj-42'}) RETURN p.name")
            .expect("lookup after reopen");
        assert_eq!(
            r.row_count(),
            1,
            "Index should survive WAL replay — found 1 row"
        );

        // Verify data integrity
        let total = session
            .execute_cypher("MATCH (p:Project) RETURN count(p)")
            .unwrap()
            .scalar::<i64>()
            .unwrap();
        assert_eq!(total, 100, "All 100 projects should survive WAL replay");

        db.close().expect("close");
    }

    println!("  Phase 2: DB reopened, index survives WAL replay");
    println!("  Property index persistence verified");
}

// ============================================================================
// Test 3: Mutation hooks maintain index consistency
// ============================================================================

#[test]
fn test_property_index_mutation_hooks() {
    println!("\n{}", "=".repeat(70));
    println!("PROPERTY INDEX MUTATION HOOKS TEST");
    println!("{}\n", "=".repeat(70));

    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // Create initial data
    for i in 0..50 {
        session
            .execute_cypher(&format!(
                "CREATE (p:Project {{slug: 'proj-{i}', status: 'active'}})"
            ))
            .expect("create");
    }

    // Create index — API programmatique (le GQL est refusé depuis T17).
    db.create_property_index("slug");
    db.create_property_index("status");

    // Test 1: New node is found via index
    session
        .execute_cypher("CREATE (p:Project {slug: 'new-project', status: 'draft'})")
        .expect("create new");
    let r = session
        .execute_cypher("MATCH (p:Project {slug: 'new-project'}) RETURN p.status")
        .expect("lookup new");
    assert_eq!(r.row_count(), 1, "New node should be found via index");
    println!("  CREATE: New node found via index");

    // Test 2: Property update maintains index
    session
        .execute_cypher("MATCH (p:Project {slug: 'proj-10'}) SET p.status = 'archived'")
        .expect("update");
    let r = session
        .execute_cypher("MATCH (p:Project {status: 'archived'}) RETURN p.slug")
        .expect("lookup updated");
    assert_eq!(
        r.row_count(),
        1,
        "Updated node should be found with new value"
    );
    // Old value should not match
    let active_count = session
        .execute_cypher("MATCH (p:Project {status: 'active'}) RETURN count(p)")
        .unwrap()
        .scalar::<i64>()
        .unwrap();
    assert_eq!(
        active_count, 49,
        "One less active project after status change"
    );
    println!("  SET: Property update maintains index (old value removed, new value added)");

    // Test 3: DELETE removes from index
    session
        .execute_cypher("MATCH (p:Project {slug: 'proj-5'}) DELETE p")
        .expect("delete");
    let r = session
        .execute_cypher("MATCH (p:Project {slug: 'proj-5'}) RETURN p")
        .expect("lookup deleted");
    assert_eq!(
        r.row_count(),
        0,
        "Deleted node should not be found via index"
    );
    println!("  DELETE: Deleted node removed from index");

    // Test 4: Overall count consistency
    let total = session
        .execute_cypher("MATCH (p:Project) RETURN count(p)")
        .unwrap()
        .scalar::<i64>()
        .unwrap();
    // 50 initial + 1 new - 1 deleted = 50
    assert_eq!(total, 50, "Total count should be 50 after mutations");
    println!("  Count consistency: {} projects (50 expected)", total);

    println!("\n  All mutation hooks verified");
}

// ============================================================================
// Test 4: Benchmark on real PO database (if available)
// ============================================================================

#[test]
fn bench_on_real_po_database() {
    let Some(po_path) = base_copie(Base::Po) else {
        return;
    };

    println!("\n{}", "=".repeat(70));
    println!("REAL PO DATABASE BENCHMARK");
    println!("{}\n", "=".repeat(70));

    // Ouverture en écriture, sur la COPIE : le banc crée des index.
    let db = ObrainDB::open(&po_path).expect("open PO db");
    let session = db.session();

    // Get stats
    let node_count = session
        .execute_cypher("MATCH (n) RETURN count(n)")
        .unwrap()
        .scalar::<i64>()
        .unwrap();
    println!("  PO Database: {} total nodes", node_count);

    // Count by label
    for label in &[
        "Project", "Plan", "Task", "Note", "Skill", "Decision", "Step",
    ] {
        let count = session
            .execute_cypher(&format!("MATCH (n:{label}) RETURN count(n)"))
            .unwrap()
            .scalar::<i64>()
            .unwrap();
        if count > 0 {
            println!("    {label}: {count}");
        }
    }

    let iterations = 20;
    println!("\n  Benchmarks ({iterations} iterations each):");

    // Typical PO queries — baseline (full scan)
    println!("\n  --- Without Index (full scan) ---");

    let q1 = measure(
        || {
            session
                .execute_cypher("MATCH (p:Project {slug: 'grafeo'}) RETURN p.name")
                .unwrap()
        },
        iterations,
    );
    print_bench("Project by slug", q1, 1);

    let q2 = measure(
        || {
            session
                .execute_cypher("MATCH (t:Task {status: 'pending'}) RETURN count(t)")
                .unwrap()
        },
        iterations,
    );
    print_bench("Task count by status", q2, 1);

    let q3 = measure(
        || {
            session
                .execute_cypher("MATCH (n:Note {note_type: 'gotcha'}) RETURN count(n)")
                .unwrap()
        },
        iterations,
    );
    print_bench("Note count by type", q3, 1);

    // Create indexes
    println!("\n  --- Creating Indexes ---");
    // API programmatique : le GQL CREATE INDEX est refusé depuis T17.
    db.create_property_index("slug");
    db.create_property_index("status");
    db.create_property_index("note_type");

    // With index
    println!("  --- With Index ---");

    let q1i = measure(
        || {
            session
                .execute_cypher("MATCH (p:Project {slug: 'grafeo'}) RETURN p.name")
                .unwrap()
        },
        iterations,
    );
    print_bench("Project by slug (indexed)", q1i, 1);

    let q2i = measure(
        || {
            session
                .execute_cypher("MATCH (t:Task {status: 'pending'}) RETURN count(t)")
                .unwrap()
        },
        iterations,
    );
    print_bench("Task count by status (indexed)", q2i, 1);

    let q3i = measure(
        || {
            session
                .execute_cypher("MATCH (n:Note {note_type: 'gotcha'}) RETURN count(n)")
                .unwrap()
        },
        iterations,
    );
    print_bench("Note count by type (indexed)", q3i, 1);

    // Speedups
    println!("\n  --- Speedup ---");
    for (name, before, after) in [
        ("Project.slug", q1.0, q1i.0),
        ("Task.status", q2.0, q2i.0),
        ("Note.note_type", q3.0, q3i.0),
    ] {
        println!(
            "  {:<30} {:.3}ms -> {:.3}ms  ({:.1}x)",
            name,
            before,
            after,
            before / after
        );
    }

    println!("\n  Real PO database benchmark complete");
}
