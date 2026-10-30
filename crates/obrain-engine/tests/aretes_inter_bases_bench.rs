//! Banc aretes inter-bases (V2) — traversee inter-store vs intra-store.
//!
//! Meme jeu de donnees des deux cotes : N notes, chacune reliee par
//! DOCUMENTS a un doc porteur de {id, titre}. Cote INTRA, docs et aretes
//! vivent dans la meme base ; cote INTER, les docs vivent dans une base
//! attachee et les aretes sont des aretes externes (resolution par seek
//! sur la propriete id, indexee T17b).
//!
//! ```bash
//! nice -19 cargo test -p obrain-engine --release \
//!   --features "gql,cypher,wal,text-index,vector-index,algos" \
//!   --test aretes_inter_bases_bench -- --nocapture
//! ```

#![cfg(feature = "gql")]

use obrain_engine::ObrainDB;
use web_time::Instant;

/// Mesure p50/mean (ms) sur N iterations.
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

#[test]
fn bench_traversee_inter_store_vs_intra_store() {
    println!("\n{}", "=".repeat(70));
    println!("ARETES INTER-BASES — traversee inter-store vs intra-store");
    println!("{}\n", "=".repeat(70));

    let n = 2000usize;
    let iterations = 30usize;

    // ── INTRA : tout dans une base ─────────────────────────────────────
    let intra = ObrainDB::new_in_memory();
    {
        let s = intra.session();
        for i in 0..n {
            s.execute(&format!(
                "CREATE (:Note {{name: 'note-{i}'}})-[:DOCUMENTS]->(:Doc {{id: 'doc-{i}', titre: 'T{i}'}})"
            ))
            .expect("seed intra");
        }
    }

    // ── INTER : notes + aretes externes dans alpha, docs dans beta ─────
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    {
        let sb = beta.session();
        for i in 0..n {
            sb.execute(&format!("INSERT (:Doc {{id: 'doc-{i}', titre: 'T{i}'}})"))
                .expect("seed beta");
        }
        beta.graph_store().create_property_index("id");
        let sa = alpha.session();
        for i in 0..n {
            sa.execute(&format!("INSERT (:Note {{name: 'note-{i}'}})"))
                .expect("seed alpha");
            sa.execute(&format!(
                "MATCH (a:Note {{name: 'note-{i}'}}) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc-{i}\"))"
            ))
            .expect("seed arete externe");
        }
        alpha.graph_store().create_property_index("name");
        alpha.attach_bases("alpha", None);
        alpha.attach_base("beta", &beta).expect("attach");
    }

    let q_traverse = "MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN count(b)";
    let q_filtre =
        "MATCH (a:Note)-[:DOCUMENTS]->(b) WHERE b.titre = 'T42' RETURN a.name, b.titre";

    // Verification de correction avant la mesure.
    let si = intra.session();
    let sx = alpha.session();
    let ri = si.execute(q_traverse).expect("intra");
    let rx = sx.execute(q_traverse).expect("inter");
    assert_eq!(ri.rows, rx.rows, "meme resultat des deux cotes");
    assert_eq!(
        si.execute(q_filtre).expect("intra").rows.len(),
        sx.execute(q_filtre).expect("inter").rows.len()
    );

    // (V4) Profondeur variable : meme frontiere, motif *1..2.
    let q_vle = "MATCH (a:Note)-[:DOCUMENTS*1..2]->(x) RETURN count(x)";
    let ri = si.execute(q_vle).expect("intra vle");
    let rx = sx.execute(q_vle).expect("inter vle");
    assert_eq!(ri.rows, rx.rows, "meme resultat VLE des deux cotes");

    println!("N = {n} notes -> docs ; {iterations} iterations, p50 (mean) en ms\n");
    for (nom, q) in [
        ("count(b) pleine traversee", q_traverse),
        ("WHERE b.titre cible", q_filtre),
        ("count profondeur variable *1..2", q_vle),
    ] {
        let (p50_i, mean_i) = measure_p50(|| si.execute(q).expect("intra"), iterations);
        let (p50_x, mean_x) = measure_p50(|| sx.execute(q).expect("inter"), iterations);
        println!(
            "  {nom:28} intra {p50_i:8.2} ({mean_i:.2})   inter {p50_x:8.2} ({mean_x:.2})   x{:.1}",
            p50_x / p50_i.max(0.0001)
        );
    }
    println!();
}
