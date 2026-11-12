//! Régénère `obrain.h` par cbindgen quand l'outil est présent À LA VERSION
//! ÉPINGLÉE ; sinon l'en-tête commité fait foi (le test `header_a_jour`
//! vérifie alors sa cohérence avec les exports du source).
//!
//! Pourquoi une version épinglée : deux versions de cbindgen ne produisent pas
//! le même texte (ordre, commentaires, macros) et le test de fraîcheur compare
//! le généré au commité octet pour octet. La CI installe la même version
//! (`cargo install cbindgen --locked --version …`, job « Bindings C »).

use std::path::Path;
use std::process::Command;

/// Version de cbindgen qui a produit l'en-tête commité. À changer ici, dans
/// `.github/workflows/ci.yml` (job « Bindings C ») et dans `cbindgen.toml`
/// (commentaire d'en-tête), puis régénérer et commiter `obrain.h`.
const CBINDGEN_VERSION: &str = "0.29.4";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=CBINDGEN");
    // Source unique de vérité pour le test `header_a_jour`.
    println!("cargo:rustc-env=OBRAIN_C_CBINDGEN_VERSION={CBINDGEN_VERSION}");

    let cbindgen = std::env::var("CBINDGEN").unwrap_or_else(|_| "cbindgen".to_string());
    let Ok(sortie) = Command::new(&cbindgen).arg("--version").output() else {
        println!("cargo:warning=cbindgen absent : obrain.h commité fait foi (non régénéré)");
        return;
    };
    let version = String::from_utf8_lossy(&sortie.stdout);
    let version = version.trim().trim_start_matches("cbindgen").trim();
    if version != CBINDGEN_VERSION {
        println!(
            "cargo:warning=cbindgen {version} trouvé, {CBINDGEN_VERSION} attendu : obrain.h \
             commité fait foi (non régénéré) ; `cargo install cbindgen --locked --version \
             {CBINDGEN_VERSION}`"
        );
        return;
    }

    let manifeste = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifeste = Path::new(&manifeste);
    let en_tete = manifeste.join("obrain.h");
    let avant = std::fs::read(&en_tete).unwrap_or_default();
    let statut = Command::new(&cbindgen)
        .current_dir(manifeste)
        .args([
            "--config",
            "cbindgen.toml",
            "--crate",
            "obrain-c",
            "--output",
        ])
        .arg(&en_tete)
        .status()
        .expect("lancer cbindgen");
    assert!(statut.success(), "cbindgen a échoué ({statut})");
    let apres = std::fs::read(&en_tete).expect("relire obrain.h");
    if avant != apres {
        println!("cargo:warning=obrain.h régénéré par cbindgen {CBINDGEN_VERSION} : à commiter");
    }
}
