//! Édition de liens de la cdylib `obrain` (module d'extension CPython).
//!
//! Avec la feature `extension-module`, pyo3 ne lie pas libpython : les symboles
//! `_Py*` restent indéfinis à l'édition de liens et sont résolus à l'import par
//! l'interpréteur. Sous Linux c'est le comportement par défaut ; sous macOS, `ld`
//! refuse les symboles indéfinis sans ce drapeau. maturin l'ajoute lui-même lors
//! d'un build de wheel ; `cargo build -p obrain-python` ne le fait pas, d'où ce
//! fichier : même résultat dans les deux chemins.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }
}
