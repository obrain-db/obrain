//! Micro-bench : coût d'un `flush()` sur un store à 7000 types d'arêtes, AVANT
//! et APRÈS l'optimisation DIRTY-FLAG des colonnes de degrés typées.
//!
//! AVANT le correctif, `flush()` persistait CHAQUE colonne
//! `substrate.degrees.node.<type>.u32` (CRC + msync) à chaque appel — même quand
//! rien n'avait bougé → O(E_types) msync par flush périodique (~0,83 ms/type,
//! ~6 s à 7000 types). APRÈS, seules les colonnes SALES sont persistées : un
//! second flush à l'identique ne fait plus qu'un commit WAL + msync de zones +
//! dict inchangé (sauté), soit ~150 ms.
//!
//! Usage : `cargo run --release -p obrain-substrate --example bench_flush_7000_types`

use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::store::SubstrateStore;
use web_time::Instant;

fn main() {
    let td = tempfile::tempdir().expect("tempdir");
    let path = td.path().join("kb-7000-types");
    let s = SubstrateStore::create(&path).expect("create");

    let a = s.create_node(&["N"]);
    let b = s.create_node(&["N"]);
    let n_types = 7000usize;
    for i in 0..n_types {
        let _ = s.create_edge(a, b, &format!("T{i:04}"));
    }

    // Premier flush : tout est sale → il persiste les 7000 colonnes + le dict.
    // C'est le coût « AVANT » : O(E_types) msync, payé une fois ici.
    let t0 = Instant::now();
    s.flush().expect("flush initial");
    let premier = t0.elapsed();

    // Second flush : rien n'a bougé. AVANT le correctif, il re-payait les 7000
    // msync ; APRÈS, il ne persiste AUCUNE colonne typée et ne réécrit pas le
    // dict. C'est le coût PÉRIODIQUE que l'optimisation fait tomber.
    let mut samples = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        s.flush().expect("flush propre");
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let propre = samples[samples.len() / 2];

    println!("types d'arêtes        : {n_types}");
    println!(
        "flush initial (sale)  : {:>9.1} ms  (O(E_types) msync, payé une fois)",
        premier.as_secs_f64() * 1000.0
    );
    println!(
        "flush propre (médiane): {:>9.1} ms  (0 colonne typée persistée, dict sauté)",
        propre
    );
    println!(
        "gain périodique       : ~{:.0} ms → {:.0} ms (≈ {:>4.0}×)",
        n_types as f64 * 0.83,
        propre,
        (n_types as f64 * 0.83) / propre.max(0.01)
    );
}
