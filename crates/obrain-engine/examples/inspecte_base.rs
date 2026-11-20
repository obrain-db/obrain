//! Ouvre une base obrain en lecture seule et imprime sa taille réelle.
//!
//! Les deux ouvertures (le store brut, puis `ObrainDB` pour la requête
//! optionnelle) sont en lecture seule : l'outil n'écrit jamais dans la base.
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example inspecte_base -- /chemin/vers/base
//! ```
use obrain_core::graph::traits::GraphStore;
use obrain_substrate::SubstrateStore;
use std::path::Path;

fn main() {
    let chemin = std::env::args()
        .nth(1)
        .expect("usage : inspecte_base <repertoire>");
    let repertoire = Path::new(&chemin);

    let mut total = 0u64;
    let mut lignes: Vec<(String, u64)> = Vec::new();
    for entree in std::fs::read_dir(repertoire).expect("lecture du répertoire") {
        let entree = entree.expect("entrée");
        let taille = entree.metadata().map_or(0, |m| m.len());
        if taille > 0 {
            total += taille;
            lignes.push((entree.file_name().to_string_lossy().into_owned(), taille));
        }
    }
    lignes.sort_by_key(|(_, taille)| std::cmp::Reverse(*taille));

    println!(
        "fichiers : {}   total : {:.1} Go",
        lignes.len(),
        total as f64 / 1e9
    );
    for (nom, taille) in lignes.iter().take(8) {
        println!("  {:<32} {:>12} o", nom, taille);
    }

    let store = SubstrateStore::open_read_only(repertoire).expect("ouverture en lecture seule");
    println!();
    println!("nœuds    : {}", store.node_count());
    println!("relations: {}", store.edge_count());
    drop(store);

    // Requête optionnelle, passée en second argument : permet d'interroger une
    // base réelle sans écrire un exemple à chaque fois.
    if let Some(requete) = std::env::args().nth(2) {
        use std::time::Instant;
        let db = obrain_engine::ObrainDB::open_read_only(repertoire)
            .expect("ouverture en lecture seule");
        let session = db.session();
        let depart = Instant::now();
        match session.execute_cypher(&requete) {
            Ok(resultat) => {
                println!();
                println!("requête : {requete}");
                println!(
                    "durée   : {:.1} ms",
                    depart.elapsed().as_secs_f64() * 1000.0
                );
                println!("lignes  : {}", resultat.rows.len());
                for ligne in resultat.rows.iter().take(15) {
                    println!("  {ligne:?}");
                }
            }
            Err(erreur) => {
                println!();
                println!("requête : {requete}");
                println!(
                    "ERREUR  : {}",
                    format!("{erreur}").lines().next().unwrap_or("")
                );
            }
        }
    }
}
