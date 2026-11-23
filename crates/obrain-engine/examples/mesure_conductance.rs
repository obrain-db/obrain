//! # `mesure_conductance` — les communautés réelles sont-elles bien séparées ?
//!
//! `conductances()` (#294) existait mais n'avait jamais tourné sur un graphe
//! réel : ses tests n'utilisent que des graphes fabriqués. Cet exemple comble
//! le trou en recalculant les communautés par Louvain sur une base, puis en
//! mesurant la conductance de chacune.
//!
//! On recalcule plutôt que de lire la partition persistée : les zones sont
//! indexées par slot, et les lire par offset de nœud est le piège qui a déjà
//! trompé deux analyses de cette session. Passer par l'API ne peut pas se
//! tromper.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-engine --example mesure_conductance -- <base> [resolution]
//! ```

use std::path::PathBuf;

use obrain_adapters::plugins::algorithms::{conductances, louvain};
use obrain_substrate::SubstrateStore;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), BoxErr> {
    let base: PathBuf = std::env::args()
        .nth(1)
        .ok_or_else(|| -> BoxErr { "usage: mesure_conductance <base> [resolution]".into() })?
        .into();
    let resolution: f64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);

    let store = SubstrateStore::open_read_only(&base)?;

    println!("communautés par Louvain (resolution {resolution}) …");
    let t = std::time::Instant::now();
    let part = louvain(&store, resolution);
    println!(
        "  {} nœuds, {} communautés, {:.1} s",
        part.communities.len(),
        part.num_communities,
        t.elapsed().as_secs_f64()
    );

    let t = std::time::Instant::now();
    let certs = conductances(&store, &part.communities);
    println!("  conductance en {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    let mut vals: Vec<f64> = certs.iter().filter_map(|c| c.conductance).collect();
    if vals.is_empty() {
        println!("aucune conductance définie");
        return Ok(());
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = vals.len();
    let moy = vals.iter().sum::<f64>() / n as f64;
    let q = |p: f64| vals[((n as f64 - 1.0) * p) as usize];
    let mal = vals.iter().filter(|v| **v > 0.5).count();
    let taille_moy = certs.iter().map(|c| c.taille).sum::<usize>() / certs.len();

    println!("\ncommunautés avec conductance définie : {n}");
    println!("  taille moyenne        : {taille_moy} nœuds");
    println!("  conductance moyenne   : {moy:.4}");
    println!("  médiane               : {:.4}", q(0.5));
    println!("  p05 / p95             : {:.4} / {:.4}", q(0.05), q(0.95));
    println!(
        "  mal séparées (> 0,5)  : {mal} ({:.1} %)",
        100.0 * mal as f64 / n as f64
    );
    let mut tailles: Vec<usize> = certs.iter().map(|c| c.taille).collect();
    tailles.sort_unstable();
    let singletons = tailles.iter().filter(|t| **t == 1).count();
    let tq = |p: f64| tailles[((tailles.len() as f64 - 1.0) * p) as usize];
    println!(
        "  tailles               : médiane {}, p95 {}, max {} ; singletons {} ({:.1} %)",
        tq(0.5),
        tq(0.95),
        tailles.last().copied().unwrap_or(0),
        singletons,
        100.0 * singletons as f64 / tailles.len() as f64
    );
    // ── HORS SINGLETONS : la seule mesure utile ──────────────────────
    // Un singleton a conductance 1,0 par construction (volume = degré,
    // coupe = degré) et n'est pas une « communauté ». Les inclure noie le
    // résultat : 81 % du total ici.
    let mut utiles: Vec<f64> = certs
        .iter()
        .filter(|c| c.taille >= 2)
        .filter_map(|c| c.conductance)
        .collect();
    utiles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !utiles.is_empty() {
        let nu = utiles.len();
        let moy_u = utiles.iter().sum::<f64>() / nu as f64;
        let bad = utiles.iter().filter(|v| **v > 0.5).count();
        let uq = |p: f64| utiles[((nu as f64 - 1.0) * p) as usize];
        println!("\nHORS SINGLETONS ({nu} communautés de taille >= 2) :");
        println!("  conductance moyenne   : {moy_u:.4}");
        println!("  médiane / p95         : {:.4} / {:.4}", uq(0.5), uq(0.95));
        println!(
            "  mal séparées (> 0,5)  : {bad} ({:.1} %)",
            100.0 * bad as f64 / nu as f64
        );
    }
    let vides: usize = certs.iter().filter(|c| c.coupe == 0).count();
    println!(
        "  communautés SANS arête sortante (coupe = 0) : {vides} ({:.1} %)",
        100.0 * vides as f64 / certs.len() as f64
    );
    Ok(())
}
