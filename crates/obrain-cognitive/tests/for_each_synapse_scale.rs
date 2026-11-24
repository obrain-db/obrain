//! (2026-11-24) Benchmark de la déduplication de `for_each_synapse` à degré élevé.
//!
//! Mesure le coût de la déduplication (SmallVec linéaire vs HashSet) sur une
//! étoile de degré d (500 / 2000 / 5000), base fermée puis rouverte depuis le
//! substrat — le scénario où le cache chaud (`node_to_keys` + DashMap) et
//! lindex dincidence couvrent exactement le même ensemble, donc celui où la
//! déduplication travaille à plein régime.
//!
//! Ce test est VOLONTAIREMENT un banc : il n'assert pas le chronomètre
//! (non reproductible), il vérifie l'INVARIANT fonctionnel — chaque voisin
//! émis exactement une fois, bon poids, ordre d'émission déterministe — et
//! imprime (ordre, hash d'ordre, chrono) pour comparaison avant/après.
//!
//! Run: cargo test --release -p obrain-cognitive --test for_each_synapse_scale
//!        --features synapse,substrate -- --nocapture
#![cfg(all(feature = "synapse", feature = "substrate"))]

use std::sync::Arc;
use std::time::Instant;

use obrain_cognitive::synapse::{SynapseConfig, SynapseStore};
use obrain_common::types::NodeId;
use obrain_substrate::SubstrateStore;

/// FNV-1a 64 bits, déterministe — pour comparer l'ordre d'émission complet
/// entre deux exécutions sans imprimer 5000 entiers.
fn fnv1a(xs: &[u64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in xs {
        h ^= x;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Construit une étoile hub—{d feuilles} sur substrat, ferme, et rouvre.
/// Retourne le TempDir (gardé vivant), le hub et la liste des feuilles.
fn etoile_rouverte(deg: usize) -> (tempfile::TempDir, NodeId, Vec<NodeId>) {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");
    let (hub, leaves) = {
        let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
        let hub = gs.create_node(&["n"]);
        let leaves: Vec<NodeId> = (0..deg).map(|_| gs.create_node(&["n"])).collect();
        let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
        let entrees: Vec<(NodeId, NodeId, f64)> = leaves.iter().map(|&l| (hub, l, 1.0)).collect();
        let created = store.bootstrap_batch(&entrees);
        assert_eq!(created, deg, "bootstrap_batch doit créer {deg} synapses");
        (hub, leaves)
    };
    (td, hub, leaves)
}

#[test]
fn echelle_dedup_500_2000_5000() {
    for deg in [500usize, 2000usize, 5000usize] {
        let (td, hub, _leaves) = etoile_rouverte(deg);
        let substrate = Arc::new(SubstrateStore::open(&td.path().join("kb")).expect("open"));
        let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
        let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);

        // Échauffement + capture de l'ordre d'émission et des poids.
        let mut ordre: Vec<u64> = Vec::new();
        let mut poids: Vec<f64> = Vec::new();
        store.for_each_synapse(hub, |n, w| {
            ordre.push(n.0);
            poids.push(w);
        });

        // INVARIANT fonctionnel : d voisins distincts, chacun une fois.
        assert_eq!(
            ordre.len(),
            deg,
            "degré attendu {deg}, émis {}",
            ordre.len()
        );
        let mut vus = std::collections::HashSet::new();
        for n in &ordre {
            assert!(vus.insert(*n), "voisin {n} émis PLUS d'une fois");
        }
        // INVARIANT poids : chaque arête émise porte le poids de la colonne
        // (1.0 ± tolérance Q0.16).
        let min_w = poids.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_w = poids.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (min_w - 1.0).abs() < 1e-3 && (max_w - 1.0).abs() < 1e-3,
            "poids attendu 1.0 ± 1e-3, mesuré min={min_w} max={max_w}"
        );

        // Chronométrage : min de N itérations.
        let n = 11usize;
        let mut best = f64::INFINITY;
        for _ in 0..n {
            let t0 = Instant::now();
            let mut c = 0usize;
            store.for_each_synapse(hub, |_, _| c += 1);
            let dt = t0.elapsed().as_secs_f64() * 1000.0;
            best = best.min(dt);
            assert_eq!(c, deg);
        }
        println!(
            "deg={deg} order_hash={:016x} poids=[{min_w:.6},{max_w:.6}] best={best:.3} ms (min de {n} it.)",
            fnv1a(&ordre),
        );
    }
}
