//! bench-head2head.rs — benchmark obrain équivalent au bench_db.c du moteur cbm.
//!
//! DB ouverte UNE seule fois, médiane sur `rep` répétitions, mêmes 7 opérations
//! que bench_db.c : open, schema, search (BM25), 1hop, triangle, k4, orderby,
//! distinct. Requêtes GQL/cypher identiques aux requêtes Cypher du bench cbm.
//!
//! Usage: cargo run -p obrain-examples --bin bench-head2head --features storage -- <file.obrain> [rep]

use std::path::PathBuf;
use std::time::Instant;

use obrain::{Config, ObrainDB};

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
    if args.len() < 2 {
        eprintln!("usage: bench-head2head <file.obrain> [rep]");
        std::process::exit(1);
    }
    let db_path = PathBuf::from(&args[1]);
    let rep: usize = args.get(2).map(|s| s.parse().unwrap_or(5)).unwrap_or(5);

    let file_size = std::fs::metadata(&db_path)?.len();
    println!("=== bench-head2head (obrain) ===");
    println!(
        "File: {} ({:.2} MB)",
        db_path.display(),
        file_size as f64 / 1_048_576.0
    );
    println!("Project nodes/edges: (voir info)");

    // (2026-08-31) Mode read-only par défaut pour le bench : le vrai coût de
    // requête d'une session serveur/www qui ne mute rien. Sans cela, le
    // close flushait le substrat (~10-22 ms) — le seul point où obrain
    // perdait vs cbm sur `open`. Passer `RW=1` pour l'ancien comportement.
    let rw = std::env::var("RW").map(|v| v == "1").unwrap_or(false);

    // ── open (open+close — le "fixe" de la CLI) ──
    let open_ms = bench(rep, || {
        let _ = if rw {
            ObrainDB::open(&db_path)
        } else {
            ObrainDB::with_config(Config::read_only(&db_path))
        };
    });
    println!("open     {:8.1} ms   (open+close seul)", open_ms);

    // ── Store ouvert UNE FOIS : les mesures du moteur pur ──
    let db = if rw {
        ObrainDB::open(&db_path)?
    } else {
        ObrainDB::with_config(Config::read_only(&db_path))?
    };
    let session = db.session();

    // schema (labels + edge types, comme get_schema_counts)
    let schema_ms = bench(rep, || {
        let _ = session.execute("MATCH (n) RETURN labels(n) AS l, COUNT(*) AS c LIMIT 20");
    });
    println!("schema   {:8.1} ms   (labels distribution)", schema_ms);

    // search (BM25 sur name — équivalent de cbm_store_search name_pattern)
    let search_ms = bench(rep, || {
        let _ = session
            .execute("MATCH (n) WHERE toLower(n.name) CONTAINS 'handler' RETURN n.name LIMIT 10");
    });
    println!(
        "search   {:8.1} ms   (toLower CONTAINS name, LIMIT 10)",
        search_ms
    );

    // search-BM25 (vrai index inversé BM25 d'obrain, construit une fois puis
    // interrogé — le même geste que create_text_index + text_search, qui est
    // l'équivalent exact du BM25 indexé de cbm_store_search).
    let bm25_ms = {
        // Construire l'index sur :Function(name) hors mesure.
        db.create_text_index("Function", "name")?;
        let t0 = Instant::now();
        let _ = db.text_search("Function", "name", "handler", 10)?;
        let build_us = t0.elapsed().as_micros();
        // Mesurer UNIQUEMENT la recherche (index déjà construit).
        let q = bench(rep, || {
            let _ = db.text_search("Function", "name", "handler", 10);
        });
        println!(
            "bm25idx  {:8.1} ms   (build index Function.name)",
            build_us as f64 / 1000.0
        );
        q
    };
    println!("searchBM25 {:6.1} ms   (BM25 indexé, LIMIT 10)", bm25_ms);

    // 1-saut (stocké)
    let q_1hop = "MATCH (a:Function)-[:CALLS]->(b:Function) RETURN a.name LIMIT 3";
    let onehop_ms = bench(rep, || {
        let _ = session.execute(q_1hop);
    });
    println!("1hop     {:8.1} ms   (MATCH 1-saut, LIMIT 3)", onehop_ms);

    // triangle (2-sauts + jointure fermée)
    let q_tri = "MATCH (a:Function)-[:CALLS]->(b:Function)-[:CALLS]->(c:Function) \
                 MATCH (c)-[:CALLS]->(a) RETURN a.name LIMIT 3";
    let tri_ms = bench(rep, || {
        let _ = session.execute(q_tri);
    });
    println!("triangle {:8.1} ms   (2-sauts + jointure fermée)", tri_ms);

    // K4 (4-sauts + 3 jointures fermées)
    let q_k4 = "MATCH (a:Function)-[:CALLS]->(b:Function)-[:CALLS]->(c:Function)-[:CALLS]->(d:Function) \
                MATCH (a)-[:CALLS]->(c),(a)-[:CALLS]->(d),(b)-[:CALLS]->(d) RETURN a.name LIMIT 3";
    let k4_ms = bench(rep, || {
        let _ = session.execute(q_k4);
    });
    println!(
        "k4       {:8.1} ms   (4-sauts + 3 jointures fermées)",
        k4_ms
    );

    // ORDER BY (tri de toutes les fonctions)
    let q_order = "MATCH (n:Function) RETURN n.name ORDER BY n.name DESC LIMIT 50";
    let order_ms = bench(rep, || {
        let _ = session.execute(q_order);
    });
    println!(
        "orderby  {:8.1} ms   (ORDER BY sur toutes les fonctions, LIMIT 50)",
        order_ms
    );

    // DISTINCT (dédup de toutes les fonctions)
    let q_dist = "MATCH (n:Function) RETURN DISTINCT n.name";
    let dist_ms = bench(rep, || {
        let _ = session.execute(q_dist);
    });
    println!(
        "distinct {:8.1} ms   (RETURN DISTINCT sur toutes les fonctions)",
        dist_ms
    );

    Ok(())
}
