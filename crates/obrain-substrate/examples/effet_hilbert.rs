//! # `effet_hilbert` — le réordonnancement change-t-il le coût de HKS ?
//!
//! `apply` du noyau de Chebyshev est dominé par des accès mémoire ALÉATOIRES
//! (`x[v]` pour un `v` quelconque). Paralléliser les boucles autour n'a rien
//! donné (mesuré : 2724 ms contre 2768, dans le bruit). La question devient
//! donc : l'ORDRE des slots influence-t-il le coût ?
//!
//! Cet exemple mesure sur une COPIE : HKS avant, `repermuter_par_hilbert`,
//! HKS après. Aucune base d'origine n'est touchée.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-substrate --example effet_hilbert -- <copie> [order] [max_degree]
//! ```

use std::path::PathBuf;
use std::time::Instant;

use obrain_substrate::SubstrateStore;
use obrain_substrate::geometric::heat_kernel_signature;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn mesure(label: &str, store: &SubstrateStore) -> Result<f64, BoxErr> {
    let t = Instant::now();
    let _ = heat_kernel_signature(store, 1.0)?;
    let d = t.elapsed().as_secs_f64() * 1e3;
    println!("  {label} : {d:.0} ms");
    Ok(d)
}

fn main() -> Result<(), BoxErr> {
    let base: PathBuf = std::env::args()
        .nth(1)
        .ok_or_else(|| -> BoxErr { "usage: effet_hilbert <copie> [order] [max_degree]".into() })?
        .into();
    let order: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let max_degree: u32 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    // Mesure 1 : ordre courant.
    let avant = {
        let store = SubstrateStore::open_read_only(&base)?;
        println!("ordre courant");
        let a = mesure("HKS", &store)?;
        let b = mesure("HKS (2e)", &store)?;
        (a + b) / 2.0
    };

    // Réordonnancement : écriture, sur la copie.
    {
        let store = SubstrateStore::open(&base)?;
        let t = Instant::now();
        store.repermuter_par_hilbert(order, max_degree)?;
        store.flush()?;
        println!(
            "\nrepermuter_par_hilbert(order={order}, max_degree={max_degree}) : {:.0} ms",
            t.elapsed().as_secs_f64() * 1e3
        );
    }

    // Mesure 2 : après réordonnancement.
    let apres = {
        let store = SubstrateStore::open_read_only(&base)?;
        println!("\naprès réordonnancement");
        let a = mesure("HKS", &store)?;
        let b = mesure("HKS (2e)", &store)?;
        (a + b) / 2.0
    };

    println!(
        "\nmoyenne : {avant:.0} ms avant, {apres:.0} ms après → {:.2}×",
        avant / apres.max(0.001)
    );
    Ok(())
}
