//! Collisions, annulations et discrimination du DefaultSpectralEncoder (64 dims).
//! Corpus déterministe ; résultats numériques bruts, pas d'interprétation au-delà.

#![cfg(feature = "engram")]

use std::collections::HashMap;

use obrain_cognitive::engram::SpectralEncoder;
use obrain_common::types::NodeId;

fn norm(x: &[f64]) -> f64 {
    x.iter().map(|u| u * u).sum::<f64>().sqrt()
}
fn l2(x: &[f64], y: &[f64]) -> f64 {
    x.iter()
        .zip(y)
        .map(|(u, v)| (u - v).powi(2))
        .sum::<f64>()
        .sqrt()
}
fn key(x: &[f64]) -> Vec<u64> {
    x.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn collisions_annulations_discrimination() {
    let enc = SpectralEncoder::new();

    // 1. Nœuds individuels : collisions exactes + annulations (signature nulle).
    let n = 2000u64;
    let mut uniques: HashMap<Vec<u64>, NodeId> = HashMap::new();
    let mut collisions = 0usize;
    let mut null_single = 0usize;
    for i in 1..=n {
        let s = enc.encode(&[(NodeId(i), 1.0)]);
        if norm(&s) < 1e-9 {
            null_single += 1;
        }
        if uniques.insert(key(&s), NodeId(i)).is_some() {
            collisions += 1;
        }
    }
    println!(
        "nœuds individuels = {}, collisions exactes = {}, signatures nulles = {}",
        n, collisions, null_single
    );

    // 2. Annulations sur paires {i,j} (ensemble non vide -> vecteur nul).
    let p = 300u64;
    let mut null_pairs = 0usize;
    for i in 1..=p {
        for j in (i + 1)..=p {
            let s = enc.encode(&[(NodeId(i), 1.0), (NodeId(j), 1.0)]);
            if norm(&s) < 1e-9 {
                null_pairs += 1;
            }
        }
    }
    println!("paires {{i,j}} (1..{}) -> nulles = {}", p, null_pairs);

    // 3. Discrimination vs taille : L2 min entre ensembles DISJOINTS de k nœuds.
    for k in [1u64, 2, 4, 8, 16] {
        let mut min_d = f64::INFINITY;
        let mut cnt = 0u64;
        let mut base = 1u64;
        while base + 2 * k <= 512 {
            let e1: Vec<(NodeId, f64)> = (base..base + k).map(|x| (NodeId(x), 1.0)).collect();
            let e2: Vec<(NodeId, f64)> =
                (base + k..base + 2 * k).map(|x| (NodeId(x), 1.0)).collect();
            let d = l2(&enc.encode(&e1), &enc.encode(&e2));
            if d < min_d {
                min_d = d;
            }
            base += 2 * k;
            cnt += 1;
        }
        println!(
            "k={}: L2 min (ensembles disjoints, {} paires) = {:.6}",
            k, cnt, min_d
        );
    }

    assert_eq!(collisions, 0, "aucune collision exacte sur ce corpus");
    assert_eq!(null_single, 0, "aucun nœud seul annulé");
    assert_eq!(null_pairs, 0, "aucune paire annulée");
}
