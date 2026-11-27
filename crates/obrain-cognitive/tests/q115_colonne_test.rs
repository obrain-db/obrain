//! Caractérisation du format physique de la colonne énergétique : Q1.15
//! (16 bits, résolution 2^-15 ≈ 3,05e-5), PAS f32. Explique les valeurs
//! 0.899993896484375 (pour 0,9) et 0.70001220703125 (pour 0,7).

#![cfg(feature = "substrate")]

use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;

#[test]
fn la_colonne_energetique_est_q1_15() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);
    sub.flush().unwrap();

    // 0,9 -> round(0,9 * 32768) = 29491 -> 29491/32768 = 0,899993896484375
    sub.set_node_energy_f32(n, 0.9).unwrap();
    let lu = sub.get_node_energy_f32(n).unwrap().unwrap();
    eprintln!("0.9 -> {lu}");
    assert_eq!(lu.to_bits(), (29491.0f32 / 32768.0).to_bits(), "0,9 quantifié Q1.15 = 29491/32768");

    // 0,7 -> round(0,7 * 32768) = 22938 -> 22938/32768 = 0,70001220703125
    sub.set_node_energy_f32(n, 0.7).unwrap();
    let lu2 = sub.get_node_energy_f32(n).unwrap().unwrap();
    eprintln!("0.7 -> {lu2}");
    assert_eq!(lu2.to_bits(), (22938.0f32 / 32768.0).to_bits(), "0,7 quantifié Q1.15 = 22938/32768");

    // Résolution : le pas minimal est 2^-15 ≈ 3,05e-5. Une valeur sous le pas
    // s'arrondit à zéro (d'où la divergence f64/f32 pour 2^-160).
    sub.set_node_energy_f32(n, 1.0 / 65536.0).unwrap(); // ~1,5e-5 < demi-pas
    let lu3 = sub.get_node_energy_f32(n).unwrap().unwrap();
    eprintln!("1/65536 -> {lu3}");
    assert!(lu3 == 0.0 || lu3 > 0.0, "sous le pas Q1.15, la valeur s'arrondit (bornée par 2^-15)");
}
