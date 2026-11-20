//! Mesure du chemin de longueur variable vers une cible exacte (obrain#189).
//!
//! Forme PO : `Note -LINKED_TO-> source -[:CONTAINS|IMPORTS|CALLS*0..2]-> File{path}`.
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example mesure_cible_exacte -- 300 8 2000 3
//! ```
#![allow(clippy::many_single_char_names)]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::time::Instant;

fn rss_mo() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        / 1024
}

fn main() {
    let arg = |i: usize, d: u64| {
        std::env::args()
            .nth(i)
            .and_then(|s| s.parse().ok())
            .unwrap_or(d)
    };
    let fichiers = arg(1, 300) as usize;
    let fonctions = arg(2, 8) as usize;
    let notes = arg(3, 2000) as usize;
    let degre = arg(4, 3) as usize;
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = move |m: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as usize
    };
    let f: Vec<_> = (0..fichiers)
        .map(|i| {
            s.create_node_with_props(
                &["File"],
                [("path", Value::String(format!("src/f{i}.rs").into()))],
            )
        })
        .collect();
    let mut sources = f.clone();
    let mut fns = vec![];
    for &fi in &f {
        for j in 0..fonctions {
            let n = s.create_node_with_props(
                &["Function"],
                [("name", Value::String(format!("g{j}").into()))],
            );
            s.create_edge(fi, n, "CONTAINS");
            fns.push(n);
            sources.push(n);
        }
    }
    for &fi in &f {
        for _ in 0..3 {
            s.create_edge(fi, f[rnd(f.len())], "IMPORTS");
        }
    }
    for &n in &fns {
        for _ in 0..degre {
            s.create_edge(n, fns[rnd(fns.len())], "CALLS");
        }
    }
    for i in 0..notes {
        let n = s.create_node_with_props(&["Note"], [("status", Value::String("active".into()))]);
        let _ = i;
        for _ in 0..3 {
            s.create_edge(n, sources[rnd(sources.len())], "LINKED_TO");
        }
    }
    println!(
        "fichiers={fichiers} fonctions/fichier={fonctions} notes={notes} rss={} Mo",
        rss_mo()
    );
    let q = |h: &str| {
        format!(
            "MATCH (target:File {{path: 'src/f7.rs'}}) MATCH path=(n:Note)-[:LINKED_TO]->(source)-[:CONTAINS|IMPORTS|CALLS{h}]->(target) WHERE n.status='active' RETURN count(path) AS c"
        )
    };
    for h in ["*0..1", "*1..2", "*0..2"] {
        let t = Instant::now();
        let r = s.execute(&q(h)).unwrap();
        println!(
            "{h:8} {:>10.1?} rss={} Mo  {:?}",
            t.elapsed(),
            rss_mo(),
            r.rows
        );
    }
}
