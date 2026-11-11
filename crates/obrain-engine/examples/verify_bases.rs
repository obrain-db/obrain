//! Ouvre EN LECTURE SEULE les bases passées en argument et imprime leurs
//! comptes de nœuds et d'arêtes.
//!
//!   cargo run --release -p obrain-engine --example verify_bases -- <base> [<base>…]
//!
//! Aucun chemin par défaut : sans argument, l'exemple n'ouvre rien (il
//! portait auparavant trois chemins `…/.obrain/db/*` en dur, ouverts en
//! lecture-écriture).

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
