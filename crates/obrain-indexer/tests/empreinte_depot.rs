//! T27 — l'empreinte de dépôt dit si une réindexation changerait quelque chose.

use obrain_indexer::index::{IndexOptions, empreinte_depot, index_repository};
use std::path::Path;

fn ecrire(racine: &Path, rel: &str, contenu: &str) {
    let p = racine.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contenu).unwrap();
}

fn depot(racine: &Path) {
    ecrire(
        racine,
        "src/a.c",
        "int aide(int x) { return x; }\nint appel(void) { return aide(3); }\n",
    );
    ecrire(racine, "src/b.c", "int b(void) { return appel(); }\n");
}

fn e(racine: &Path) -> u128 {
    empreinte_depot(racine, &IndexOptions::default())
}

#[test]
fn l_empreinte_seule_egale_celle_de_l_indexation() {
    let d = tempfile::tempdir().unwrap();
    depot(d.path());
    let r = index_repository(d.path(), &IndexOptions::default());
    assert_eq!(e(d.path()), r.empreinte_depot);
}

#[test]
fn rien_ne_change_l_empreinte_ne_change_pas() {
    let d = tempfile::tempdir().unwrap();
    depot(d.path());
    let avant = e(d.path());
    assert_eq!(avant, e(d.path()), "deux calculs sur le même état");
    ecrire(d.path(), "notes/lisez-moi.xyz", "hors indexation");
    assert_eq!(avant, e(d.path()), "un fichier non indexé ne compte pas");
}

#[test]
fn un_contenu_ou_un_fichier_change_l_empreinte() {
    let d = tempfile::tempdir().unwrap();
    depot(d.path());
    let avant = e(d.path());
    ecrire(d.path(), "src/b.c", "int b(void) { return 0; }\n");
    let apres_modif = e(d.path());
    assert_ne!(avant, apres_modif, "contenu modifié");
    ecrire(d.path(), "src/c.c", "int c(void) { return 1; }\n");
    assert_ne!(apres_modif, e(d.path()), "fichier ajouté");
}

#[test]
fn un_commit_change_l_empreinte_meme_a_contenu_egal() {
    let d = tempfile::tempdir().unwrap();
    depot(d.path());
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(d.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) {
        return; // pas de git sur cette machine : rien à prouver ici
    }
    assert!(git(&["add", "."]) && git(&["commit", "-q", "-m", "un"]));
    let apres_un = e(d.path());
    assert!(git(&["commit", "-q", "--allow-empty", "-m", "deux"]));
    assert_ne!(
        apres_un,
        e(d.path()),
        "nouvelle tête git : la branche et l'historique indexés bougent"
    );
    let r = index_repository(d.path(), &IndexOptions::default());
    assert_eq!(
        e(d.path()),
        r.empreinte_depot,
        "égalité aussi dans un dépôt git"
    );
}
