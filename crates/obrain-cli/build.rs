//! Estampille la provenance du binaire au moment de la compilation.
//!
//! Sans cela, `obrain version` ne rend que la version du manifeste Cargo —
//! figée à 0.0.1 — et rien ne relie un binaire qui tourne à un commit. Le
//! 2026-09-01, sept binaires de secours coexistaient dans le répertoire
//! d'installation de la prod, sept tailles différentes, aucun manifeste :
//! impossible de dire lequel correspondait à quoi. Le moteur C voisin avait
//! cette traçabilité (MANIFEST.txt + cohorte publiée au journal) ; obrain non.
//!
//! `arbre` est le champ qui compte : un binaire bâti sur un arbre sale n'est
//! pas reconstructible. Il vaut mieux le lire dans `obrain version` que le
//! découvrir après coup.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let sortie = Command::new("git").args(args).output().ok()?;
    if !sortie.status.success() {
        return None;
    }
    let s = String::from_utf8(sortie.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn main() {
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "inconnu".to_string());
    let describe =
        git(&["describe", "--tags", "--always", "--dirty"]).unwrap_or_else(|| "inconnu".to_string());
    let sale = git(&["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    println!("cargo:rustc-env=OBRAIN_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=OBRAIN_GIT_DESCRIBE={describe}");
    println!(
        "cargo:rustc-env=OBRAIN_GIT_ARBRE={}",
        if sale { "SALE" } else { "propre" }
    );

    // ── LA VERSION DE RUSTC, ESTAMPILLÉE (2026-09-16) ───────────────────────
    // Le harness publiait `"rustc":"non_estampee"` : un champ d'identité qui ne
    // pouvait rien dire, alors que la version du compilateur est précisément ce
    // qui explique qu'un MÊME source produise deux binaires différents — donc
    // qu'un comportement change sans que le code ait bougé. `rust-version` du
    // manifeste n'est PAS la réponse : c'est une déclaration de compatibilité
    // (« ce crate compile à partir de 1.91.1 »), pas la version qui a compilé.
    let rustc = Command::new("rustc")
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "inconnu".to_string());
    println!("cargo:rustc-env=RUSTC_VERSION={rustc}");
    // `RUSTC` peut désigner un autre compilateur que celui du PATH : on recompile
    // si la variable change, sinon l'estampille mentirait par inertie.
    println!("cargo:rerun-if-env-changed=RUSTC");

    // Recompiler quand HEAD bouge, sinon l'estampille se fige au premier build.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
}
