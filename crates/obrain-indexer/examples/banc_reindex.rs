//! T27 — banc de l'indexeur SEUL sur deux passages : le premier remplit le
//! cache d'extraction, le second doit en être servi. Usage :
//!   OBRAIN_INDEX_PROF=1 banc_reindex <depot> <dossier-cache>
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let depot = a
        .next()
        .expect("usage: banc_reindex <depot> <dossier-cache>");
    let cache = a
        .next()
        .expect("usage: banc_reindex <depot> <dossier-cache>");
    let opts = obrain_indexer::index::IndexOptions {
        cache_dir: Some(cache.into()),
        ..Default::default()
    };
    for passage in 1..=2 {
        let t = Instant::now();
        let r = obrain_indexer::index::index_repository(std::path::Path::new(&depot), &opts);
        println!(
            "passage {passage} : {:.2}s — {} nœuds, {} arêtes, {} fichiers dont {} depuis le cache",
            t.elapsed().as_secs_f64(),
            r.graph.node_count(),
            r.graph.edges().len(),
            r.files_scanned,
            r.fichiers_depuis_cache
        );
    }
}
