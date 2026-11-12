//! Mesure ce que coûte la copie d'une base avant de la désigner à un banc :
//! durée et octets logiques (imprimés), disque réellement consommé (à lire
//! avec `df` avant et après, la copie étant conservée si
//! `OBRAIN_BENCH_GARDER_COPIE=1`).
//!
//!   cargo run --release -p obrain-bases-banc --example cout_copie -- /chemin/absolu/vers/la/base
//!
//! La base n'est que lue, fichier par fichier ; elle n'est pas ouverte.

use std::path::PathBuf;

fn main() {
    let Some(base) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage : cout_copie <chemin absolu d'une base> — rien n'est copié");
        std::process::exit(2);
    };
    let debut = std::time::Instant::now();
    let copie = obrain_bases_banc::copie_de(&base, "l'argument");
    println!(
        "copie : {} en {:.3} s",
        copie.chemin().display(),
        debut.elapsed().as_secs_f64()
    );
}
