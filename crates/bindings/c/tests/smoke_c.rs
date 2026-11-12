//! Fumée bout en bout : `tests/smoke.c` est compilé par le compilateur C de la
//! machine (`CC`, sinon `cc`), lié dynamiquement à `libobrain_c`, puis lancé
//! sur une base temporaire. Le programme C ouvre en écriture, INSERT + MATCH
//! en GQL, lit `obrain_result_json` / `obrain_result_row_count`, vérifie le
//! bail, ferme, relit en lecture seule et libère tout.
//!
//! La bibliothèque est celle que `cargo test` vient de construire (le crate
//! est aussi `rlib`, donc sa cdylib/staticlib sortent de la même compilation) ;
//! elle est cherchée à côté du binaire de test.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bibliotheque_dynamique() -> PathBuf {
    // La cdylib est dans `target/<profil>/`, mais l'exécutable de test peut être ailleurs
    // selon le runner (`out/`, `deps/`) : on remonte l'arborescence jusqu'à la trouver.
    let exe = std::env::current_exe().expect("current_exe");
    let nom = format!(
        "{}obrain_c{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    for dir in exe.ancestors().skip(1) {
        let candidat = dir.join(&nom);
        if candidat.is_file() {
            return candidat;
        }
    }
    panic!("{nom} introuvable en remontant depuis {}", exe.display());
}

#[test]
fn smoke_c() {
    let manifeste = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib = bibliotheque_dynamique();
    let lib_dir = lib.parent().expect("dossier de la bibliothèque");
    let sortie = tempfile::tempdir().expect("tempdir");
    let binaire = sortie.path().join("smoke");

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let compilation = Command::new(&cc)
        .args(["-std=c99", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(manifeste)
        .arg("-o")
        .arg(&binaire)
        .arg(manifeste.join("tests").join("smoke.c"))
        .arg(format!("-L{}", lib_dir.display()))
        .arg("-lobrain_c")
        .arg(format!("-Wl,-rpath,{}", lib_dir.display()))
        .output()
        .unwrap_or_else(|e| panic!("lancer {cc} : {e}"));
    assert!(
        compilation.status.success(),
        "compilation C échouée ({}):\n{}",
        compilation.status,
        String::from_utf8_lossy(&compilation.stderr)
    );

    let base = sortie.path().join("base");
    let execution = Command::new(&binaire)
        .arg(&base)
        .output()
        .expect("lancer smoke");
    let stdout = String::from_utf8_lossy(&execution.stdout);
    let stderr = String::from_utf8_lossy(&execution.stderr);
    println!("--- smoke.c stdout ---\n{stdout}--- stderr ---\n{stderr}");
    assert!(
        execution.status.success(),
        "smoke.c a échoué ({})",
        execution.status
    );
    assert!(stdout.contains("lignes=2"), "deux lignes attendues");
    assert!(
        stdout.contains("bail refusé"),
        "le bail doit refuser le second écrivain"
    );
    assert!(
        stdout.contains("SMOKE_C_OK"),
        "fin du programme non atteinte"
    );
}
