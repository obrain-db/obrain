//! Fabrique des bases JETABLES pour exercer les bancs sur « bases réelles »
//! sans jamais toucher à une vraie base.
//!
//! ```bash
//! # deux corpus simpliciaux identiques (CORPUS_A / CORPUS_B / CORPUS_DB)
//! cargo run --release -p obrain-substrate --example fabrique_bases_banc -- simplexes /tmp/x/a
//! # une base de N nœuds (mesure du coût de copie)
//! cargo run --release -p obrain-substrate --example fabrique_bases_banc -- noeuds 1000000 /tmp/x/n1m
//! ```
//!
//! Refuse un chemin qui existe déjà : on ne fabrique jamais par-dessus une base.

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;

/// Groupes de trois `Function` : chaque groupe porte un triangle `CALLS`
/// (a→b, a→c, b→c) et un site de dispatch (a→b et a→c depuis la même ligne).
/// 30 000 nœuds : `mesures_corpus` adresse les nœuds jusqu'à 25 001.
const NOEUDS_SIMPLEXES: u64 = 30_000;

fn simplexes(s: &SubstrateStore) {
    let ids: Vec<NodeId> = (0..NOEUDS_SIMPLEXES)
        .map(|_| s.create_node(&["Function"]))
        .collect();
    // `as_chunks` (stable 1.88, sous la MSRV) : clippy 1.99 refuse
    // `chunks_exact` à taille constante (`chunks_exact_to_as_chunks`).
    for (g, [a, b, c]) in ids.as_chunks::<3>().0.iter().enumerate() {
        let (a, b, c) = (*a, *b, *c);
        let ligne = Value::Int64(i64::try_from(g).unwrap_or(i64::MAX));
        let ab = s.create_edge(a, b, "CALLS");
        let ac = s.create_edge(a, c, "CALLS");
        s.create_edge(b, c, "CALLS");
        s.set_edge_property(ab, "line", ligne.clone());
        s.set_edge_property(ac, "line", ligne);
    }
}

fn noeuds(s: &SubstrateStore, n: u64) {
    let mut precedent: Option<NodeId> = None;
    for i in 0..n {
        let id = s.create_node(&["File"]);
        s.set_node_property(id, "i", Value::Int64(i64::try_from(i).unwrap_or(i64::MAX)));
        if let Some(p) = precedent {
            s.create_edge(p, id, "NEXT");
        }
        precedent = Some(id);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (genre, chemin, n) = match args.as_slice() {
        [g, c] if g == "simplexes" => ("simplexes", c.clone(), 0),
        [g, n, c] if g == "noeuds" => ("noeuds", c.clone(), n.parse::<u64>().expect("N")),
        _ => {
            eprintln!("usage : fabrique_bases_banc simplexes <dir> | noeuds <N> <dir>");
            std::process::exit(2);
        }
    };
    assert!(
        !std::path::Path::new(&chemin).exists(),
        "{chemin} existe déjà : on ne fabrique pas par-dessus"
    );
    let debut = std::time::Instant::now();
    let s = SubstrateStore::create(&chemin).expect("création");
    match genre {
        "simplexes" => simplexes(&s),
        _ => noeuds(&s, n),
    }
    s.flush().expect("flush");
    drop(s);
    println!(
        "base « {genre} » fabriquée dans {chemin} en {:.1} s",
        debut.elapsed().as_secs_f64()
    );
}
