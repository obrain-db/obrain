//! Contre-épreuve de FIDÉLITÉ de la signature spectrale : mesure la perte
//! d'information entre ensembles pondérés distincts.

#![cfg(feature = "engram")]

use obrain_cognitive::engram::SpectralEncoder;
use obrain_common::types::NodeId;

fn l2(x: &[f64], y: &[f64]) -> f64 {
    x.iter()
        .zip(y)
        .map(|(u, v)| (u - v).powi(2))
        .sum::<f64>()
        .sqrt()
}
fn norm(x: &[f64]) -> f64 {
    x.iter().map(|u| u * u).sum::<f64>().sqrt()
}

#[test]
fn fidelite_signature_spectrale() {
    let enc = SpectralEncoder::new();
    let (a, b) = (NodeId(1), NodeId(2));

    let s_a = enc.encode(&[(a, 0.4), (b, 0.2)]);
    let s_b = enc.encode(&[(a, 0.8), (b, 0.4)]); // 2 × A (échelle uniforme)
    let s_c = enc.encode(&[(a, 0.2), (b, 0.4)]); // poids échangés
    let s_d = enc.encode(&[(b, 0.2), (a, 0.4)]); // réordonné == A

    println!("norm A  = {:.6}", norm(&s_a));
    println!("norm B  = {:.6}", norm(&s_b));
    println!("L2(A,B) = {:.6}", l2(&s_a, &s_b));
    println!("L2(A,C) = {:.6}", l2(&s_a, &s_c));
    println!("L2(A,D) = {:.6}", l2(&s_a, &s_d));

    // Vide et poids nuls.
    let s_empty = enc.encode(&[]);
    let s_zero = enc.encode(&[(a, 0.0), (b, 0.0)]);
    println!("norm(vide) = {:.6}", norm(&s_empty));
    println!("norm(zéro) = {:.6}", norm(&s_zero));

    assert!(
        l2(&s_a, &s_b) < 1e-9,
        "échelle uniforme → même signature (magnitude perdue)"
    );
    assert!(l2(&s_a, &s_d) < 1e-9, "ordre des membres sans effet");
    assert!(
        l2(&s_a, &s_c) > 1e-9,
        "poids relatifs différents → signatures distinctes"
    );
    assert!((norm(&s_a) - 1.0).abs() < 1e-9, "norme unitaire");
    assert!(norm(&s_empty) < 1e-9, "vide → vecteur nul");
    assert!(norm(&s_zero) < 1e-9, "poids nuls → vecteur nul");
}
