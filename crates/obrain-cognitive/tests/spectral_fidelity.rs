//! Contre-épreuve de FIDÉLITÉ de la signature spectrale : mesure la perte
//! d'information entre ensembles pondérés distincts.

#![cfg(feature = "engram")]

use obrain_common::types::NodeId;
use obrain_cognitive::engram::SpectralEncoder;

fn l2(x: &[f64], y: &[f64]) -> f64 {
    x.iter().zip(y).map(|(u, v)| (u - v).powi(2)).sum::<f64>().sqrt()
}
fn norm(x: &[f64]) -> f64 {
    x.iter().map(|u| u * u).sum::<f64>().sqrt()
}

#[test]
fn fidelite_signature_spectrale() {
    let enc = SpectralEncoder::new();
    let (a, b) = (NodeId(1), NodeId(2));

    let sA = enc.encode(&[(a, 0.4), (b, 0.2)]);
    let sB = enc.encode(&[(a, 0.8), (b, 0.4)]); // 2 × A (échelle uniforme)
    let sC = enc.encode(&[(a, 0.2), (b, 0.4)]); // poids échangés
    let sD = enc.encode(&[(b, 0.2), (a, 0.4)]); // réordonné == A

    println!("norm A  = {:.6}", norm(&sA));
    println!("norm B  = {:.6}", norm(&sB));
    println!("L2(A,B) = {:.6}", l2(&sA, &sB));
    println!("L2(A,C) = {:.6}", l2(&sA, &sC));
    println!("L2(A,D) = {:.6}", l2(&sA, &sD));

    // Vide et poids nuls.
    let s_empty = enc.encode(&[]);
    let s_zero = enc.encode(&[(a, 0.0), (b, 0.0)]);
    println!("norm(vide) = {:.6}", norm(&s_empty));
    println!("norm(zéro) = {:.6}", norm(&s_zero));

    assert!(l2(&sA, &sB) < 1e-9, "échelle uniforme → même signature (magnitude perdue)");
    assert!(l2(&sA, &sD) < 1e-9, "ordre des membres sans effet");
    assert!(l2(&sA, &sC) > 1e-9, "poids relatifs différents → signatures distinctes");
    assert!((norm(&sA) - 1.0).abs() < 1e-9, "norme unitaire");
    assert!(norm(&s_empty) < 1e-9, "vide → vecteur nul");
    assert!(norm(&s_zero) < 1e-9, "poids nuls → vecteur nul");
}
