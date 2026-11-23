//! # `gain_cache_reel` — ce que le cache CSR change pour un usage réel (#292)
//!
//! `mesure_csr` mesure le build ; cet exemple mesure une **opération** qui
//! l'utilise. `heat_kernel_signature` passe par `store.csr_cached()` depuis
//! #327 ; la variante `_with_csr` prend le CSR qu'on lui donne.
//!
//! On compare trois appels successifs dans les deux modes, sur une vraie base.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-substrate --example gain_cache_reel -- <base> [appels]
//! ```

use std::path::PathBuf;
use std::time::Instant;

use obrain_substrate::SubstrateStore;
use obrain_substrate::geometric::{
    CsrAdjacency, heat_kernel_signature, heat_kernel_signature_with_csr,
};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), BoxErr> {
    let base: PathBuf = std::env::args()
        .nth(1)
        .ok_or_else(|| -> BoxErr { "usage: gain_cache_reel <base> [appels]".into() })?
        .into();
    let appels: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    let store = SubstrateStore::open(base)?;
    println!("base : {} nœuds", {
        use obrain_core::graph::traits::GraphStore;
        store.node_count()
    });

    // Sans cache : un build par appel, comme avant #327.
    let mut sans = Vec::new();
    for i in 0..appels {
        let t = Instant::now();
        let csr = CsrAdjacency::build(&store);
        let _ = heat_kernel_signature_with_csr(&csr, 1.0)?;
        sans.push(t.elapsed().as_secs_f64() * 1e3);
        println!("  sans cache, appel {} : {:.1} ms", i + 1, sans[i]);
    }

    // Avec cache : forme exacte du chemin de production.
    let mut avec = Vec::new();
    for i in 0..appels {
        let t = Instant::now();
        let _ = heat_kernel_signature(&store, 1.0)?;
        avec.push(t.elapsed().as_secs_f64() * 1e3);
        println!("  avec cache, appel {} : {:.1} ms", i + 1, avec[i]);
    }

    let total_sans: f64 = sans.iter().sum();
    let total_avec: f64 = avec.iter().sum();
    println!(
        "\n{appels} appels : {total_sans:.0} ms sans cache, {total_avec:.0} ms avec → gain {:.1}×",
        total_sans / total_avec.max(0.001)
    );
    Ok(())
}
