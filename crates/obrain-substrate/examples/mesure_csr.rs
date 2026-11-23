//! # `mesure_csr` — le coût réel de `CsrAdjacency::build` (#292)
//!
//! Le code affirme « ~20 ns/edge […] ~160 ms of setup time » pour 8 M
//! d'arêtes. Avant d'ajouter un cache, il faut savoir ce que le build coûte
//! VRAIMENT, et combien de fois une opération le paie.
//!
//! Cet exemple le mesure sans rien changer au comportement.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-substrate --example mesure_csr -- <base> [builds]
//! ```

use std::path::PathBuf;
use std::time::Instant;

use obrain_core::graph::traits::GraphStore;
use obrain_substrate::SubstrateStore;
use obrain_substrate::geometric::CsrAdjacency;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), BoxErr> {
    let base: PathBuf = std::env::args()
        .nth(1)
        .ok_or_else(|| -> BoxErr { "usage: mesure_csr <base> [builds]".into() })?
        .into();
    let builds: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let store = SubstrateStore::open(base)?;
    let n = store.node_count();
    println!("base : {n} nœuds");

    let mut durees = Vec::with_capacity(builds);
    for i in 0..builds {
        let t = Instant::now();
        let csr = CsrAdjacency::build(&store);
        let d = t.elapsed();
        durees.push(d.as_secs_f64() * 1e3);
        println!(
            "  build {}/{} : {:.1} ms ({} lignes)",
            i + 1,
            builds,
            durees[i],
            csr.row_offsets.len() as u64
        );
    }
    durees.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mediane = durees[durees.len() / 2];
    println!("\nmédiane : {mediane:.1} ms par build (sans cache)");

    // PAS de mode « avec cache » : le cache a été implémenté puis RETIRÉ.
    // `create_edge` n'incrémente pas `feature_epoch`, donc un cache indexé sur
    // cet epoch servirait un CSR périmé sur toute mutation d'arête — et un CSR
    // périmé fausse silencieusement HKS, Ricci, le pas de chaleur, la
    // résistance effective et les géodésiques. 238 ms valent mieux qu'un
    // résultat faux. Voir la note #292 : il manque un compteur de révision
    // STRUCTURELLE, que rien n'incrémente aujourd'hui.
    println!(
        "\ncoût d'une opération qui appelle 3 fonctions géométriques : {:.0} ms",
        mediane * 3.0
    );

    Ok(())
}
