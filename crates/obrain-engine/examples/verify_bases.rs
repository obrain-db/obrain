//! Imprime les comptes de nœuds et d'arêtes des bases passées en argument,
//! SANS les ouvrir : chaque base est copiée (clonée sur APFS) et c'est la
//! copie qui est ouverte, en lecture seule.
//!
//!   cargo run --release -p obrain-engine --example verify_bases -- <base> [<base>…]
//!
//! Chemins ABSOLUS de répertoires contenant `substrate.meta`. Aucun chemin par
//! défaut : sans argument, l'exemple n'ouvre rien (il portait auparavant trois
//! chemins de bases en dur, ouverts en lecture-écriture).
//!
//! Pourquoi une copie : `open_read_only` refuse les mutations de la session,
//! mais écrit sur le disque (rejeu du WAL, colonnes reconstruites) et crée une
//! base dans un répertoire qui n'en est pas une. Voir docs/BENCHMARKS.md,
//! « Bancs sur bases réelles ».

use obrain_engine::ObrainDB;

fn main() {
    let bases: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    if bases.is_empty() {
        eprintln!(
            "usage : verify_bases <base> [<base>…] — aucune base désignée, rien n'est ouvert"
        );
        std::process::exit(2);
    }
    for path in &bases {
        // Une base refusée (chemin relatif, pas de `substrate.meta`, base tenue
        // par un processus…) est SAUTÉE et nommée ; on passe à la suivante.
        let copie = match obrain_bases_banc::essayer_copie_de(path, "l'argument") {
            Ok(copie) => copie,
            Err(refus) => {
                println!("{} : SAUTÉE — {refus}", path.display());
                continue;
            }
        };
        print!("{} (copie, lecture seule)... ", path.display());
        match ObrainDB::open_read_only(copie.chemin()) {
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
