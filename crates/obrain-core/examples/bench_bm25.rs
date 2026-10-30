//! Micro-banc BM25 (mesure brute, hors criterion) : ns/posting et µs/requête.
//!
//! Usage :
//! ```text
//! CARGO_TARGET_DIR=/tmp/opt-bm25 cargo run --release -p obrain-core \
//!   --features text-index --example bench_bm25
//! ```

use std::hint::black_box;
use std::time::Instant;

use obrain_common::types::NodeId;
use obrain_core::index::text::{BM25Config, InvertedIndex};

fn main() {
    let n = 20_000u64;
    let mut index = InvertedIndex::new(BM25Config::default());
    for i in 0..n {
        // « tokens » apparaît dans TOUS les documents : la requête visite
        // exactement n postings, ce qui donne un coût par posting honnête.
        index.insert(
            NodeId::new(i),
            &format!("tokens common document number {i}"),
        );
    }

    // Warm-up : la première recherche paie l'allocation du scratch/accumulateur.
    for _ in 0..200 {
        black_box(index.search("tokens", 10));
    }

    let iters = 2_000usize;
    let start = Instant::now();
    for _ in 0..iters {
        black_box(index.search("tokens", 10));
    }
    let elapsed = start.elapsed();
    let ns = elapsed.as_nanos() as f64 / iters as f64;
    println!(
        "n={n} query='tokens' iters={iters} total={elapsed:?} \
         avg_per_query={ns:.1} ns ({:.1} µs)  ns_per_posting={:.2}",
        ns / 1000.0,
        ns / n as f64
    );
}
