//! Un modèle ONNX **local** et **quantifié int8** (l'export ARM64 de all-MiniLM-L6-v2) se
//! charge sans Hugging Face et rend des vecteurs qui portent du sens. C'est le chemin iOS :
//! feature `embed` seule, ONNX Runtime lié statiquement, fichiers dans le conteneur de l'app.
//!
//! Ignoré par défaut : il exige `OBRAIN_TEST_MODELE_DIR`, un dossier contenant
//! `model_qint8_arm64.onnx` et `tokenizer.json` (23 Mo + 466 Ko).
#![cfg(feature = "embed")]

use obrain_engine::embedding::{EmbeddingModel, OnnxEmbeddingModel};

fn cosinus(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

#[test]
#[ignore = "exige OBRAIN_TEST_MODELE_DIR (model_qint8_arm64.onnx + tokenizer.json)"]
fn un_modele_int8_local_rend_des_vecteurs_qui_portent_du_sens() {
    let dir = std::env::var("OBRAIN_TEST_MODELE_DIR").expect("OBRAIN_TEST_MODELE_DIR");
    let debut = std::time::Instant::now();
    let modele = OnnxEmbeddingModel::from_files(
        "minilm-int8",
        format!("{dir}/model_qint8_arm64.onnx"),
        format!("{dir}/tokenizer.json"),
    )
    .expect("chargement du modèle int8 local");
    let chargement = debut.elapsed();
    assert_eq!(modele.dimensions(), 384);

    let debut = std::time::Instant::now();
    let v = modele
        .embed(&[
            "the car is parked outside the house",
            "an automobile needs fuel to run",
            "apple pie recipe with cinnamon",
        ])
        .expect("inférence");
    let inference = debut.elapsed();
    assert_eq!(v.len(), 3);
    assert!(v.iter().all(|x| x.len() == 384));
    assert!(v.iter().all(|x| x.iter().all(|f| f.is_finite())), "un vecteur porte NaN/inf");

    let proche = cosinus(&v[0], &v[1]);
    let loin = cosinus(&v[0], &v[2]);
    assert!(
        proche > loin,
        "car~automobile ({proche:.3}) doit battre car~pie ({loin:.3})"
    );
    eprintln!("MESURE int8 Mac : chargement {chargement:?} ; 3 textes {inference:?} ; cos proche {proche:.3} / loin {loin:.3}");
}
