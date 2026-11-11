//! Ouvre EN LECTURE SEULE les bases passées en argument et imprime leurs
//! comptes de nœuds et d'arêtes.
//!
//!   cargo run --release -p obrain-engine --example verify_bases -- <base> [<base>…]
//!
//! Aucun chemin par défaut : sans argument, l'exemple n'ouvre rien (il
//! portait auparavant trois chemins de bases en dur, ouverts en
//! lecture-écriture).
//!
//! ATTENTION : `open_read_only` refuse les mutations de la session mais peut
//! écrire sur le disque (rejeu du WAL, colonnes reconstruites), et CRÉE une
//! base dans un répertoire qui n'en est pas une — d'où le refus ci-dessous
//! d'un chemin sans `substrate.meta`. Lui passer une copie, pas une base vivante
//! (docs/BENCHMARKS.md, « Bancs sur bases réelles »).

use obrain_engine::ObrainDB;

fn main() {
    let bases: Vec<String> = std::env::args().skip(1).collect();
    if bases.is_empty() {
        eprintln!(
            "usage : verify_bases <base> [<base>…] — aucune base désignée, rien n'est ouvert"
        );
        std::process::exit(2);
    }
    for path in &bases {
        if !std::path::Path::new(path).join("substrate.meta").is_file() {
            println!("{path} : pas de substrate.meta, ce n'est pas une base — non ouvert");
            continue;
        }
        print!("Opening {path} (lecture seule)... ");
        match ObrainDB::open_read_only(path) {
            Ok(db) => {
                let store = db.store();
                let n = store.node_count();
                let e = store.edge_count();
                println!("OK nodes={n} edges={e}");
            }
            Err(err) => println!("FAIL: {err}"),
        }
    }
}
