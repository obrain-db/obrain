//! L'en-tête COMMITÉ `obrain.h` est-il celui que cbindgen (version épinglée)
//! produit à partir du source ?
//!
//! - cbindgen présent à la version épinglée : génération dans un fichier
//!   temporaire et comparaison octet pour octet avec l'en-tête commité
//!   (`git show HEAD:…`, ou le fichier sur disque hors dépôt git). Rouge si
//!   un export a changé sans que l'en-tête régénéré soit commité.
//! - cbindgen présent à une AUTRE version : rouge, avec la commande
//!   d'installation (une autre version produit un autre texte).
//! - cbindgen absent : l'en-tête commité fait foi ; on vérifie qu'il déclare
//!   exactement les fonctions `extern "C"` du source (pas de diff textuel).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const VERSION_EPINGLEE: &str = env!("OBRAIN_C_CBINDGEN_VERSION");

fn version_cbindgen(cbindgen: &str) -> Option<String> {
    let sortie = Command::new(cbindgen).arg("--version").output().ok()?;
    if !sortie.status.success() {
        return None;
    }
    let texte = String::from_utf8_lossy(&sortie.stdout);
    Some(
        texte
            .trim()
            .trim_start_matches("cbindgen")
            .trim()
            .to_string(),
    )
}

/// Contenu de `obrain.h` tel que COMMITÉ (HEAD), ou tel que sur disque si le
/// crate n'est pas dans un dépôt git (archive `cargo package`).
fn en_tete_commite(manifeste: &Path) -> String {
    let git = Command::new("git")
        .arg("-C")
        .arg(manifeste)
        .args(["show", "HEAD:./obrain.h"])
        .output();
    match git {
        Ok(s) if s.status.success() => String::from_utf8(s.stdout).expect("obrain.h UTF-8"),
        _ => std::fs::read_to_string(manifeste.join("obrain.h")).expect("lire obrain.h"),
    }
}

fn exports_du_source(manifeste: &Path) -> BTreeSet<String> {
    let mut noms = BTreeSet::new();
    for entree in std::fs::read_dir(manifeste.join("src")).expect("lire src/") {
        let chemin = entree.expect("entrée").path();
        if chemin.extension().is_some_and(|e| e == "rs") {
            let texte = std::fs::read_to_string(&chemin).expect("lire source");
            for ligne in texte.lines() {
                if let Some(reste) = ligne.trim().strip_prefix("pub extern \"C\" fn ") {
                    let nom: String = reste
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    noms.insert(nom);
                }
            }
        }
    }
    noms
}

fn exports_de_l_en_tete(texte: &str) -> BTreeSet<String> {
    let mut noms = BTreeSet::new();
    for ligne in texte.lines() {
        // Une déclaration : `type nom(…` sans `#`, hors commentaires.
        let l = ligne.trim();
        if l.starts_with('#') || l.starts_with("//") || l.starts_with('*') || l.starts_with("/*") {
            continue;
        }
        if let Some(debut) = l.find("obrain_") {
            let nom: String = l[debut..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if l[debut + nom.len()..].starts_with('(') {
                noms.insert(nom);
            }
        }
    }
    noms
}

#[test]
fn header_a_jour() {
    let manifeste = Path::new(env!("CARGO_MANIFEST_DIR"));
    let commite = en_tete_commite(manifeste);
    let cbindgen = std::env::var("CBINDGEN").unwrap_or_else(|_| "cbindgen".to_string());

    match version_cbindgen(&cbindgen) {
        Some(version) if version == VERSION_EPINGLEE => {
            let tmp = tempfile::tempdir().expect("tempdir");
            let genere = tmp.path().join("obrain.h");
            let statut = Command::new(&cbindgen)
                .current_dir(manifeste)
                .args([
                    "--config",
                    "cbindgen.toml",
                    "--crate",
                    "obrain-c",
                    "--output",
                ])
                .arg(&genere)
                .status()
                .expect("lancer cbindgen");
            assert!(statut.success(), "cbindgen a échoué : {statut}");
            let genere = std::fs::read_to_string(&genere).expect("lire le généré");
            assert!(
                genere == commite,
                "obrain.h commité diverge du généré par cbindgen {VERSION_EPINGLEE} : \
                 `cargo build -p obrain-c` le régénère, commitez-le.\n--- premier écart ---\n{}",
                premier_ecart(&commite, &genere)
            );
            println!("header_a_jour : identique au généré par cbindgen {version}");
        }
        Some(version) => panic!(
            "cbindgen {version} trouvé, {VERSION_EPINGLEE} attendu : \
             `cargo install cbindgen --locked --version {VERSION_EPINGLEE}`"
        ),
        None => {
            let source = exports_du_source(manifeste);
            let en_tete = exports_de_l_en_tete(&commite);
            let manquants: Vec<_> = source.difference(&en_tete).collect();
            let en_trop: Vec<_> = en_tete.difference(&source).collect();
            assert!(
                manquants.is_empty() && en_trop.is_empty(),
                "cbindgen absent et obrain.h incohérent avec le source : \
                 absents de l'en-tête {manquants:?}, absents du source {en_trop:?}"
            );
            println!(
                "header_a_jour : cbindgen absent, {} exports déclarés et cohérents",
                source.len()
            );
        }
    }
}

fn premier_ecart(a: &str, b: &str) -> String {
    for (n, (la, lb)) in a.lines().zip(b.lines()).enumerate() {
        if la != lb {
            return format!("ligne {} :\n  commité : {la}\n  généré  : {lb}", n + 1);
        }
    }
    format!(
        "longueurs différentes : {} lignes commitées, {} générées",
        a.lines().count(),
        b.lines().count()
    )
}
