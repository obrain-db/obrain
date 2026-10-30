//! Validation du provider de position sur des bases réelles.
//!
//! Ouvre une base substrate **en lecture** et rapporte ce que le provider y
//! trouve : dimension, couverture, nœuds écartés. Aucune écriture.
//!
//! Usage :
//!   cargo run --release -p obrain-cognitive --features substrate \
//!     --example position_scan -- /chemin/vers/base.obrain

#[cfg(not(feature = "substrate"))]
fn main() {
    eprintln!("exemple à compiler avec --features substrate");
}

#[cfg(feature = "substrate")]
fn main() {
    use obrain_cognitive::engram::position_substrate::{
        DEFAULT_POSITION_KEY, SubstratePositionProvider,
    };
    use obrain_core::graph::GraphStore;
    use obrain_substrate::SubstrateStore;
    use web_time::Instant;

    let args: Vec<String> = std::env::args().collect();
    let paths: Vec<String> = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        eprintln!("usage : position_scan <base.obrain> [...]");
        std::process::exit(2);
    };

    println!(
        "{:<28} {:>9} {:>9} {:>7} {:>9} {:>10}",
        "base", "nœuds", "position", "dims", "écartés", "chargement"
    );
    for p in &paths {
        let t_open = Instant::now();
        // `open_read_only` : l'en-tête de cet exemple promet « aucune écriture »,
        // et `open` prenait le verrou d'écriture (et pouvait écrire un WAL) sur
        // une base de production. Une promesse de lecture seule se tient par
        // l'appel, pas par le commentaire.
        let store = match SubstrateStore::open_read_only(p) {
            Ok(s) => s,
            Err(e) => {
                println!("{:<28} ERREUR ouverture : {e}", short(p));
                continue;
            }
        };
        let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
        let n_nodes = store.all_node_ids().len();

        let t_load = Instant::now();
        let provider = match SubstratePositionProvider::load(&store, DEFAULT_POSITION_KEY) {
            Ok(v) => v,
            Err(e) => {
                println!("{:<28} {n_nodes:>9} ERREUR provider : {e}", short(p));
                continue;
            }
        };
        let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;

        println!(
            "{:<28} {n_nodes:>9} {:>9} {:>7} {:>9} {:>9.1}ms",
            short(p),
            provider.len(),
            provider.dim(),
            provider.skipped_zero(),
            load_ms,
        );
        if provider.is_empty() {
            println!(
                "  -> colonne `{DEFAULT_POSITION_KEY}` ABSENTE sur cette base : \
                 le provider dégrade proprement (vide, pas d'erreur). \
                 Ouverture {open_ms:.0} ms."
            );
        } else {
            let mb = provider.bytes() as f64 / (1024.0 * 1024.0);
            println!(
                "  -> {mb:.1} Mio en mémoire, couverture {:.1} %, ouverture {open_ms:.0} ms",
                100.0 * provider.len() as f64 / n_nodes.max(1) as f64
            );
        }
    }
}

#[cfg(feature = "substrate")]
fn short(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}
