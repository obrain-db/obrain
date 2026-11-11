//! Garde de l'incident du 2026-11-11 : une base réelle ne s'ouvre que si une
//! variable la désigne — jamais parce qu'elle se trouve sous `$HOME`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use obrain_bases_banc::{Base, copier_arborescence, empreinte, resoudre_avec};

/// Fabrique `<racine>/.obrain/db/{po,wikipedia,megalaw}`, chacune avec un
/// `substrate.meta` : la disposition exacte que les anciennes cibles
/// devinaient. (Fichiers factices : ces tests ne passent pas par le moteur.)
fn faux_home(racine: &Path) {
    for base in Base::TOUTES {
        let dossier = racine.join(".obrain/db").join(base.nom());
        fs::create_dir_all(&dossier).unwrap();
        fs::write(dossier.join("substrate.meta"), b"en-tete").unwrap();
        fs::write(dossier.join("substrate.nodes"), b"donnees de l'utilisateur").unwrap();
    }
}

fn environnement(paires: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let table: HashMap<String, OsString> = paires
        .iter()
        .map(|(cle, valeur)| ((*cle).to_owned(), valeur.as_os_str().to_owned()))
        .collect();
    move |cle| table.get(cle).cloned()
}

#[test]
fn home_seul_ne_designe_aucune_base() {
    let tmp = tempfile::tempdir().unwrap();
    faux_home(tmp.path());
    let avant = empreinte(tmp.path()).unwrap();

    let env = environnement(&[("HOME", tmp.path()), ("USERPROFILE", tmp.path())]);
    for base in Base::TOUTES {
        assert_eq!(
            resoudre_avec(base, &env),
            None,
            "la base « {} » a été résolue sans {} : un chemin a été deviné sous HOME",
            base.nom(),
            base.variable()
        );
    }

    assert_eq!(
        avant,
        empreinte(tmp.path()).unwrap(),
        "la résolution a touché aux fichiers"
    );
}

#[test]
fn une_variable_vide_ne_designe_rien() {
    let env = environnement(&[(Base::Po.variable(), Path::new(""))]);
    assert_eq!(resoudre_avec(Base::Po, &env), None);
}

#[test]
fn la_variable_designe_sa_base_et_elle_seule() {
    let tmp = tempfile::tempdir().unwrap();
    faux_home(tmp.path());
    let po = tmp.path().join(".obrain/db/po");
    let env = environnement(&[(Base::Po.variable(), &po), ("HOME", tmp.path())]);

    assert_eq!(resoudre_avec(Base::Po, &env), Some(po));
    assert_eq!(resoudre_avec(Base::Wikipedia, &env), None);
    assert_eq!(resoudre_avec(Base::Megalaw, &env), None);
}

#[test]
#[should_panic(expected = "pas de substrate.meta")]
fn un_chemin_inexistant_echoue_bruyamment() {
    let env = environnement(&[(Base::Po.variable(), Path::new("/chemin/qui/n/existe/pas"))]);
    let _ = resoudre_avec(Base::Po, &env);
}

/// Le piège : `OBRAIN_BENCH_DB_PO=$HOME/.obrain/db` (le PARENT des bases).
/// Le moteur y créerait une base vide, même ouvert en lecture seule. Le
/// helper refuse avant toute ouverture, et ne crée rien.
#[test]
fn le_dossier_parent_des_bases_est_refuse_sans_rien_creer() {
    let tmp = tempfile::tempdir().unwrap();
    faux_home(tmp.path());
    let parent = tmp.path().join(".obrain/db");
    let avant = empreinte(tmp.path()).unwrap();

    let env = environnement(&[(Base::Po.variable(), &parent)]);
    let refus = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resoudre_avec(Base::Po, &env)
    }));
    assert!(
        refus.is_err(),
        "le parent des bases a été accepté comme base"
    );
    assert_eq!(avant, empreinte(tmp.path()).unwrap());
}

#[test]
#[should_panic(expected = "chemin ABSOLU")]
fn un_chemin_relatif_est_refuse() {
    let env = environnement(&[(Base::Po.variable(), Path::new("bases/po"))]);
    let _ = resoudre_avec(Base::Po, &env);
}

#[test]
fn ecrire_dans_la_copie_ne_touche_pas_l_original() {
    let tmp = tempfile::tempdir().unwrap();
    faux_home(tmp.path());
    let original = tmp.path().join(".obrain/db/po");
    fs::create_dir_all(original.join("sous/dossier")).unwrap();
    fs::write(original.join("sous/dossier/wal"), b"journal").unwrap();
    let avant = empreinte(&original).unwrap();

    let copie = tmp.path().join("copie");
    copier_arborescence(&original, &copie).unwrap();
    assert_eq!(
        avant.keys().collect::<Vec<_>>(),
        empreinte(&copie).unwrap().keys().collect::<Vec<_>>(),
        "la copie n'a pas la même arborescence"
    );

    fs::write(copie.join("substrate.nodes"), b"ecrase par le banc").unwrap();
    fs::write(copie.join("nouveau"), b"cree par le banc").unwrap();
    fs::remove_file(copie.join("sous/dossier/wal")).unwrap();

    assert_eq!(avant, empreinte(&original).unwrap());
}

/// L'empreinte voit un fichier créé PUIS supprimé (verrou, temporaire) : la
/// liste des chemins est revenue à l'identique, la date du répertoire non.
#[test]
fn l_empreinte_voit_un_fichier_cree_puis_supprime() {
    let tmp = tempfile::tempdir().unwrap();
    faux_home(tmp.path());
    let base = tmp.path().join(".obrain/db/po");
    let avant = empreinte(&base).unwrap();

    // La date d'un répertoire a une résolution finie : laisser passer un cran.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(base.join("substrate.lock"), b"").unwrap();
    fs::remove_file(base.join("substrate.lock")).unwrap();

    let apres = empreinte(&base).unwrap();
    assert_eq!(
        avant.keys().collect::<Vec<_>>(),
        apres.keys().collect::<Vec<_>>()
    );
    assert_ne!(
        avant, apres,
        "un fichier créé puis supprimé est passé inaperçu"
    );
}

// ───────────────────── Garde de source du workspace ─────────────────────────

/// Fichiers de `dossier` portant une des `extensions`, récursivement. Les
/// répertoires de build et de dépendances ne sont pas du code du dépôt.
fn fichiers_sous(dossier: &Path, extensions: &[&str], trouves: &mut Vec<PathBuf>) {
    let Ok(entrees) = fs::read_dir(dossier) else {
        return;
    };
    for entree in entrees {
        let chemin = entree.unwrap().path();
        let nom = chemin.file_name().unwrap().to_string_lossy().into_owned();
        if chemin.is_dir() {
            if !matches!(nom.as_str(), "target" | "node_modules" | ".git" | "pkg") {
                fichiers_sous(&chemin, extensions, trouves);
            }
        } else if chemin
            .extension()
            .is_some_and(|e| extensions.iter().any(|x| e == *x))
        {
            trouves.push(chemin);
        }
    }
}

/// Motifs qui, dans du Rust, trahissent un chemin de données deviné.
const MOTIFS_RUST: &[&str] = &[
    // Le répertoire personnel, par l'environnement…
    "\"HOME\"",
    "\"USERPROFILE\"",
    "\"HOMEPATH\"",
    "\"HOMEDRIVE\"",
    "env::vars(",
    "env::vars_os(",
    // … ou par une bibliothèque.
    "home_dir",
    "data_dir",
    "data_local_dir",
    "config_dir",
    "ProjectDirs",
    "BaseDirs",
    "UserDirs",
    "shellexpand",
    "\"~/",
    // L'emplacement des bases, sous toutes ses formes.
    ".obrain/db",
    "join(\".obrain\")",
    "\".obrain\"",
];

/// Motifs équivalents dans les scripts, le Makefile et les workflows.
const MOTIFS_SCRIPTS: &[&str] = &[
    "$HOME",
    "${HOME",
    "$(HOME)",
    "~/",
    ".obrain/",
    "USERPROFILE",
    "HOMEPATH",
    "expanduser",
    "Path.home",
];

/// Exceptions NOMMÉES : (fin du chemin, motif, justification). Toute autre
/// occurrence fait échouer la garde.
const EXCEPTIONS: &[(&str, &str, &str)] = &[
    (
        "crates/obrain-engine/tests/wal_directory.rs",
        "\".obrain\"",
        "refuse l'EXTENSION `.obrain` sur un chemin de tempdir (`ends_with`) ; ne construit aucun chemin",
    ),
    (
        "crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs",
        "\"HOME\"",
        "la preuve de l'incident : pose HOME d'un SOUS-PROCESSUS sur un faux HOME temporaire",
    ),
    (
        "crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs",
        ".obrain/db",
        "la preuve de l'incident : fabrique `.obrain/db` dans un faux HOME temporaire",
    ),
];

/// Garde de non-régression sur le code lui-même : rien, dans le workspace —
/// sources des crates (modules `#[cfg(test)]` et `src/bin` compris), tests,
/// bancs, exemples, scripts, Makefile, workflows — ne lit le répertoire
/// personnel ni ne construit un chemin `.obrain/db`. Une base réelle passe par
/// `obrain_bases_banc` ou par un argument explicite.
#[test]
fn rien_dans_le_workspace_ne_devine_un_chemin_sous_home() {
    let racine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let ce_crate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();

    let mut rust = Vec::new();
    for dossier in ["crates", "examples", "scripts"] {
        fichiers_sous(&racine.join(dossier), &["rs"], &mut rust);
    }
    // Ce crate nomme forcément ce qu'il interdit.
    rust.retain(|f| !f.starts_with(&ce_crate));
    let mut scripts = Vec::new();
    for dossier in ["crates", "examples", "scripts", ".github"] {
        fichiers_sous(
            &racine.join(dossier),
            &["sh", "bash", "zsh", "ps1", "py", "yml", "yaml", "mk"],
            &mut scripts,
        );
    }
    scripts.push(racine.join("Makefile"));
    assert!(
        rust.len() > 400 && scripts.len() > 10,
        "le parcours n'a trouvé que {} fichiers Rust et {} scripts : la garde ne regarde plus \
         le workspace",
        rust.len(),
        scripts.len()
    );

    let mut fautes = Vec::new();
    let mut exceptions_vues = vec![false; EXCEPTIONS.len()];
    let lots: [(&[PathBuf], &[&str], &str); 2] =
        [(&rust, MOTIFS_RUST, "//"), (&scripts, MOTIFS_SCRIPTS, "#")];
    for (fichiers, motifs, commentaire) in lots {
        for fichier in fichiers {
            // Un fichier qu'on ne sait pas lire n'est pas un fichier vérifié.
            let texte = fs::read_to_string(fichier).unwrap_or_else(|e| {
                panic!(
                    "fichier illisible, donc non vérifié : {} : {e}",
                    fichier.display()
                )
            });
            let relatif = fichier.strip_prefix(&racine).unwrap();
            for (numero, ligne) in texte.lines().enumerate() {
                let code = ligne.trim_start();
                if code.starts_with(commentaire) {
                    continue;
                }
                for motif in motifs.iter().filter(|m| code.contains(**m)) {
                    let exception = EXCEPTIONS
                        .iter()
                        .position(|(fin, m, _)| relatif.ends_with(fin) && m == motif);
                    if let Some(i) = exception {
                        exceptions_vues[i] = true;
                    } else {
                        fautes.push(format!(
                            "{}:{} contient {motif} : {code}",
                            relatif.display(),
                            numero + 1
                        ));
                    }
                }
            }
        }
    }
    assert!(
        fautes.is_empty(),
        "le workspace devine encore un chemin sous le répertoire personnel — passer par \
         obrain_bases_banc (OBRAIN_BENCH_DB_*), ou nommer l'exception dans EXCEPTIONS :\n{}",
        fautes.join("\n")
    );
    // Une exception qui ne sert plus est retirée, pas gardée « au cas où ».
    for (i, (fin, motif, _)) in EXCEPTIONS.iter().enumerate() {
        assert!(
            exceptions_vues[i],
            "exception périmée : {fin} ne contient plus {motif}"
        );
    }
}
