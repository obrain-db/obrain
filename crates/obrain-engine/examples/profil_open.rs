//! Profil de `open` sur une base synthétique — combien de temps coûte
//! l'ouverture à chaud, et ce temps dépend-il du nombre d'arêtes ?
//! (obrain-db/obrain#264)
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example profil_open -- <noeuds> <aretes>
//! ```

use obrain_engine::ObrainDB;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let aretes: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(n * 5);

    let dir = std::env::temp_dir().join(format!("profil-open-{n}-{aretes}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // 1) Fabrique — API directe du substrat : une requête Cypher par nœud
    // coûterait des minutes (parse + plan + transaction à chaque ligne).
    {
        use obrain_core::graph::GraphStoreMut;
        let t = Instant::now();
        let s = obrain_substrate::SubstrateStore::create(&dir).expect("création");
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            ids.push(s.create_node(&["N"]));
        }
        for i in 0..aretes {
            s.create_edge(ids[i % n], ids[(i * 7 + 3) % n], "R");
        }
        // Sans flush, le WAL garde toutes les trames : c'est l'état d'une base
        // importée qu'on n'a jamais compactée — celui de l'export PO.
        if std::env::var("SANS_FLUSH").is_err() {
            s.flush().expect("flush");
        } else {
            s.fsync_wal().ok();
        }
        println!("fabrication : {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    let taille = |p: &std::path::Path| -> u64 { std::fs::metadata(p).map_or(0, |m| m.len()) };
    for f in [
        "substrate.wal",
        "substrate.edges",
        "substrate.nodes",
        "substrate.props.v2",
    ] {
        println!("{f:<22} {:>12} octets", taille(&dir.join(f)));
    }
    let total: u64 = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.metadata().map_or(0, |m| m.len()))
        .sum();
    println!("base totale : {:.1} Mio", total as f64 / (1024.0 * 1024.0));

    // 2) Ouvertures à chaud
    let mut durees = Vec::new();
    for i in 0..4 {
        let t = Instant::now();
        let db = ObrainDB::open(&dir).expect("open");
        let d = t.elapsed().as_secs_f64() * 1000.0;
        // Un accès réel, pour distinguer « open » de « premier accès ».
        let t2 = Instant::now();
        let _ = db
            .session()
            .execute_cypher("MATCH (n:N) RETURN count(n)")
            .map(|r| r.rows.len());
        let d2 = t2.elapsed().as_secs_f64() * 1000.0;
        durees.push((d, d2));
        drop(db);
        println!("open {i} : {d:.1} ms (premier accès {d2:.1} ms)");
    }
    let hot: f64 = durees.iter().skip(1).map(|(d, _)| *d).sum::<f64>() / 3.0;
    println!("moyenne à chaud : {hot:.1} ms");
}
