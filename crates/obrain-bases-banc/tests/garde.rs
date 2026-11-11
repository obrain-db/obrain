//! Garde de l'incident du 2026-11-11 : une base réelle ne s'ouvre que si une
//! variable la désigne — jamais parce qu'elle se trouve sous `$HOME` — et
//! seul `obrain_bases_banc` lit ces variables.
//!
//! C'est une garde TEXTUELLE : elle attrape les formes qu'un développeur écrit
//! (lecture de `HOME`, chemin `.obrain`, nom d'une variable de désignation,
//! `env::vars()`), pas un contournement délibéré (nom de variable reconstitué
//! à l'exécution). La barrière de fond est ailleurs : le nom des variables
//! n'est pas exporté par le helper, et la base désignée n'est jamais rendue,
//! seulement sa copie.

use std::fs;
use std::path::{Path, PathBuf};

use obrain_bases_banc::{empreinte, essayer_copie_de};

/// Une fausse base de fichiers factices (ces tests ne passent pas par le
/// moteur).
fn fausse_base(racine: &Path) -> PathBuf {
    let dossier = racine.join("po");
    fs::create_dir_all(&dossier).unwrap();
    fs::write(dossier.join("substrate.meta"), b"en-tete").unwrap();
    fs::write(dossier.join("substrate.nodes"), b"donnees de l'utilisateur").unwrap();
    dossier
}

#[test]
fn ecrire_dans_la_copie_ne_touche_pas_l_original() {
    let tmp = tempfile::tempdir().unwrap();
    let original = fausse_base(tmp.path());
    fs::create_dir_all(original.join("sous/dossier")).unwrap();
    fs::write(original.join("sous/dossier/wal"), b"journal").unwrap();
    let avant = empreinte(&original).unwrap();

    let copie = essayer_copie_de(&original, "essai").unwrap();
    let chemin = copie.chemin();
    assert_eq!(
        avant
            .keys()
            .filter(|k| !k.as_os_str().is_empty())
            .collect::<Vec<_>>(),
        empreinte(chemin)
            .unwrap()
            .keys()
            .filter(|k| !k.as_os_str().is_empty())
            .collect::<Vec<_>>(),
        "la copie n'a pas la même arborescence"
    );

    fs::write(chemin.join("substrate.nodes"), b"ecrase par le banc").unwrap();
    fs::write(chemin.join("nouveau"), b"cree par le banc").unwrap();
    fs::remove_file(chemin.join("sous/dossier/wal")).unwrap();

    assert_eq!(avant, empreinte(&original).unwrap());
}

/// L'empreinte voit un fichier créé PUIS supprimé (verrou, temporaire) : la
/// liste des chemins est revenue à l'identique, la date du répertoire non.
#[test]
fn l_empreinte_voit_un_fichier_cree_puis_supprime() {
    let tmp = tempfile::tempdir().unwrap();
    let base = fausse_base(tmp.path());
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

// ───────────────────── Garde de source du dépôt ─────────────────────────────

/// Un répertoire est-il un produit de build ou un dépôt de dépendances ?
/// Jamais sur son seul NOM : un dossier de sources nommé `target` ou `pkg`
/// serait une échappatoire. `target` n'est ignoré qu'à sa place (racine du
/// dépôt, racine d'un crate) ou s'il porte la marque de cargo ; `pkg` que s'il
/// porte celle de wasm-pack.
fn est_un_produit_de_build(racine: &Path, dossier: &Path) -> bool {
    let nom = dossier.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if dossier.join("CACHEDIR.TAG").is_file() {
        return true;
    }
    if nom == "target" {
        let parent = dossier.parent().unwrap_or(racine);
        let racine_de_crate = parent.join("Cargo.toml").is_file();
        if parent == racine || racine_de_crate {
            return true;
        }
    }
    if nom == "pkg" && fs::read_to_string(dossier.join(".gitignore")).is_ok_and(|g| g.trim() == "*")
    {
        return true;
    }
    // Dépendances installées, métadonnées git : pas du code du dépôt.
    matches!(nom, "node_modules" | ".git" | "__pycache__" | ".venv")
}

/// Tous les fichiers sous `dossier`, récursivement. Un répertoire qu'on ne
/// peut pas lire n'est pas un répertoire vérifié : la garde échoue.
fn fichiers_sous(racine: &Path, dossier: &Path, trouves: &mut Vec<PathBuf>) {
    let entrees = fs::read_dir(dossier).unwrap_or_else(|e| {
        panic!(
            "répertoire illisible, donc non vérifié : {} : {e}",
            dossier.display()
        )
    });
    for entree in entrees {
        let chemin = entree
            .unwrap_or_else(|e| panic!("entrée illisible sous {} : {e}", dossier.display()))
            .path();
        let meta = fs::symlink_metadata(&chemin)
            .unwrap_or_else(|e| panic!("entrée illisible {} : {e}", chemin.display()));
        if meta.is_dir() {
            if !est_un_produit_de_build(racine, &chemin) {
                fichiers_sous(racine, &chemin, trouves);
            }
        } else if meta.is_file() {
            trouves.push(chemin);
        }
    }
}

/// Familles de fichiers : leurs motifs, et le préfixe d'une ligne de
/// commentaire (`None` : le format n'a pas de commentaire de ligne).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Famille {
    Rust,
    Script(Option<&'static str>),
}

/// Famille d'un fichier, ou `None` s'il n'est pas du code ni de la
/// configuration (documentation, données, images, verrous…).
fn famille(fichier: &Path) -> Option<Famille> {
    let nom = fichier.file_name()?.to_str()?;
    let extension = fichier.extension().and_then(|e| e.to_str()).unwrap_or("");
    let famille = match extension {
        "rs" => Famille::Rust,
        "sh" | "bash" | "zsh" | "ps1" | "py" | "yml" | "yaml" | "mk" | "toml" | "cfg" | "ini" => {
            Famille::Script(Some("#"))
        }
        "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" => Famille::Script(Some("//")),
        "bat" | "cmd" => Famille::Script(Some("REM ")),
        "json" => Famille::Script(None),
        _ if matches!(nom, "Makefile" | "makefile" | "justfile" | "Justfile") => {
            Famille::Script(Some("#"))
        }
        // Script sans extension : reconnu à son shebang.
        "" if fs::read(fichier).is_ok_and(|o| o.starts_with(b"#!")) => Famille::Script(Some("#")),
        _ => return None,
    };
    Some(famille)
}

/// Motifs qui, dans du Rust, trahissent un chemin de données deviné. Ce sont
/// des formes d'APPEL (`::data_dir(`), pas de simples mots : une variable
/// locale nommée `data_dir` ou `config_dir` n'est pas une faute.
const MOTIFS_RUST: &[&str] = &[
    // Le répertoire personnel, par l'environnement…
    "\"HOME\"",
    "\"USERPROFILE\"",
    "\"HOMEPATH\"",
    "\"HOMEDRIVE\"",
    // … par la libc…
    "getpwuid",
    // … ou par une bibliothèque (`dirs`, `directories`, `home`, `homedir`).
    "home_dir(",
    "homedir",
    "::data_dir(",
    "::data_local_dir(",
    "::config_dir(",
    "ProjectDirs",
    "BaseDirs",
    "UserDirs",
    "shellexpand",
    "\"~/",
    // L'emplacement des bases : relatif, composé, ou absolu en dur.
    "/.obrain",
    "\".obrain/",
    "join(\".obrain\")",
    "\".obrain\"",
    // Le nom d'une variable de désignation : seul le helper le connaît.
    "OBRAIN_BENCH_DB_",
];

/// Motifs équivalents dans les scripts, manifestes, Makefile et workflows.
const MOTIFS_SCRIPTS: &[&str] = &[
    "$HOME",
    "${HOME",
    "$(HOME)",
    "%USERPROFILE%",
    "~/",
    "/.obrain",
    "USERPROFILE",
    "HOMEPATH",
    "expanduser",
    "Path.home",
    "homedir",
    "getpwuid",
];

/// Répertoires d'OUTILS sous le répertoire personnel : y faire référence
/// (`$HOME/.cargo/bin` dans un workflow, `~/.cargo/advisory-db` dans
/// `deny.toml`) n'est pas deviner un chemin de données. Retirés de la ligne
/// avant la recherche des motifs, pour que la garde ne produise pas de faux
/// positifs qui pousseraient à la désactiver.
const OUTILS_TOLERES: &[&str] = &[
    "$HOME/.cargo",
    "${HOME}/.cargo",
    "~/.cargo",
    "$HOME/.rustup",
    "${HOME}/.rustup",
    "~/.rustup",
    "$HOME/.local/bin",
    "${HOME}/.local/bin",
    "~/.local/bin",
];

/// `env::vars()` / `vars_os()` sous TOUTES ses formes : appel qualifié ou non
/// (`use std::env::vars; vars()`), import renommé (`use std::env::vars as
/// v`). Énumérer l'environnement, c'est lire une désignation sans la nommer.
fn enumere_l_environnement(code: &str) -> bool {
    let mot_entier = |mot: &str| {
        code.match_indices(mot).any(|(i, _)| {
            let avant = code[..i].chars().next_back();
            let apres = code[i + mot.len()..].chars().next();
            let bord = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            bord(avant) && bord(apres)
        })
    };
    let appel = ["vars(", "vars_os("].iter().any(|appel| {
        code.match_indices(appel).any(|(i, _)| {
            code[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
        })
    });
    let import = code.starts_with("use ")
        && code.contains("env")
        && (mot_entier("vars") || mot_entier("vars_os"));
    appel || import
}

/// Exceptions NOMMÉES, à l'occurrence près : (fin du chemin, motif, LIGNE
/// exacte sans son indentation, nombre d'occurrences attendu, justification).
/// Toute autre occurrence — y compris une répétition de la même ligne
/// au-delà du nombre attendu — fait échouer la garde.
const EXCEPTIONS: &[(&str, &str, &str, usize, &str)] = &[
    (
        "crates/obrain-engine/tests/wal_directory.rs",
        "\".obrain\"",
        "!path.to_string_lossy().ends_with(\".obrain\"),",
        1,
        "refuse l'EXTENSION `.obrain` sur un chemin de tempdir ; ne construit aucun chemin",
    ),
    (
        "crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs",
        "\"HOME\"",
        ".env(\"HOME\", home)",
        1,
        "la preuve de l'incident : pose HOME d'un SOUS-PROCESSUS sur un faux HOME temporaire",
    ),
    (
        "crates/obrain-engine/tests/bases_reelles_non_ouvertes.rs",
        "\".obrain/",
        "let db = racine.join(\".obrain/db\");",
        1,
        "la preuve de l'incident : fabrique `.obrain/db` dans un faux HOME temporaire",
    ),
    // ── Le helper lui-même : c'est LUI qui lit les désignations. ──
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "OBRAIN_BENCH_DB_",
        "Self::Po => \"OBRAIN_BENCH_DB_PO\",",
        1,
        "la table des variables de désignation, privée au crate",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "OBRAIN_BENCH_DB_",
        "Self::Wikipedia => \"OBRAIN_BENCH_DB_WIKIPEDIA\",",
        1,
        "la table des variables de désignation, privée au crate",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "OBRAIN_BENCH_DB_",
        "Self::Megalaw => \"OBRAIN_BENCH_DB_MEGALAW\",",
        1,
        "la table des variables de désignation, privée au crate",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "\"HOME\"",
        "let env = environnement(&[(\"HOME\", tmp.path()), (\"USERPROFILE\", tmp.path())]);",
        1,
        "test unitaire : un HOME FOURNI (faux) ne désigne aucune base",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "\"USERPROFILE\"",
        "let env = environnement(&[(\"HOME\", tmp.path()), (\"USERPROFILE\", tmp.path())]);",
        1,
        "test unitaire : un USERPROFILE FOURNI (faux) ne désigne aucune base",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "\"HOME\"",
        "let env = environnement(&[(Base::Po.variable(), &po), (\"HOME\", tmp.path())]);",
        1,
        "test unitaire : un HOME FOURNI (faux) ne change pas la base désignée",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "\".obrain/",
        "let dossier = racine.join(\".obrain/db\").join(base.nom());",
        1,
        "test unitaire : fabrique la disposition devinée jadis, dans un tempdir",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "\".obrain/",
        "let po = tmp.path().join(\".obrain/db/po\");",
        1,
        "test unitaire : désigne la fausse base du tempdir",
    ),
    (
        "crates/obrain-bases-banc/src/lib.rs",
        "\".obrain/",
        "let parent = tmp.path().join(\".obrain/db\");",
        1,
        "test unitaire : le dossier PARENT des bases, dans un tempdir, doit être refusé",
    ),
];

/// Les fautes d'une ligne de code.
fn fautes_de_la_ligne(famille: Famille, code: &str) -> Vec<&'static str> {
    match famille {
        Famille::Rust => {
            let mut fautes: Vec<&str> = MOTIFS_RUST
                .iter()
                .copied()
                .filter(|m| code.contains(m))
                .collect();
            if enumere_l_environnement(code) {
                fautes.push("énumération de l'environnement (env::vars)");
            }
            fautes
        }
        Famille::Script(_) => {
            let mut nettoye = code.to_owned();
            for outil in OUTILS_TOLERES {
                nettoye = nettoye.replace(outil, "");
            }
            MOTIFS_SCRIPTS
                .iter()
                .copied()
                .filter(|m| nettoye.contains(m))
                .collect()
        }
    }
}

/// Applique la garde à `racine` et rend les fautes (vide = propre), plus le
/// nombre de fichiers Rust et autres examinés.
fn garder(
    racine: &Path,
    exceptions: &[(&str, &str, &str, usize, &str)],
) -> (Vec<String>, usize, usize) {
    let mut fichiers = Vec::new();
    fichiers_sous(racine, racine, &mut fichiers);
    // La liste des motifs ci-dessus nomme forcément ce qu'elle interdit.
    let ce_fichier = Path::new(file!());
    fichiers.retain(|f| !f.ends_with(ce_fichier));

    let mut fautes = Vec::new();
    let mut vues = vec![0usize; exceptions.len()];
    let (mut rust, mut autres) = (0, 0);
    for fichier in &fichiers {
        let Some(famille) = famille(fichier) else {
            continue;
        };
        if famille == Famille::Rust {
            rust += 1;
        } else {
            autres += 1;
        }
        // Un fichier qu'on ne sait pas lire n'est pas un fichier vérifié.
        let texte = fs::read_to_string(fichier).unwrap_or_else(|e| {
            panic!(
                "fichier illisible, donc non vérifié : {} : {e}",
                fichier.display()
            )
        });
        let commentaire = match famille {
            Famille::Rust => Some("//"),
            Famille::Script(c) => c,
        };
        let relatif = fichier.strip_prefix(racine).unwrap_or(fichier);
        for (numero, ligne) in texte.lines().enumerate() {
            let code = ligne.trim();
            if commentaire.is_some_and(|c| code.starts_with(c) || code.starts_with("::")) {
                continue;
            }
            for motif in fautes_de_la_ligne(famille, code) {
                let exception = exceptions.iter().position(|(fin, m, exacte, _, _)| {
                    relatif.ends_with(fin) && *m == motif && *exacte == code
                });
                if let Some(i) = exception {
                    vues[i] += 1;
                    if vues[i] <= exceptions[i].3 {
                        continue;
                    }
                }
                fautes.push(format!(
                    "{}:{} — {motif} : {code}",
                    relatif.display(),
                    numero + 1
                ));
            }
        }
    }
    // Une exception qui ne sert plus (ou plus autant) est corrigée, pas gardée
    // « au cas où ».
    for (i, (fin, motif, exacte, attendu, _)) in exceptions.iter().enumerate() {
        if vues[i] < *attendu {
            fautes.push(format!(
                "exception périmée : {fin} contient {} fois (au lieu de {attendu}) la ligne « {exacte} » \
                 ({motif})",
                vues[i]
            ));
        }
    }
    (fautes, rust, autres)
}

fn racine_du_depot() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Garde de non-régression sur le dépôt entier — sources des crates (modules
/// `#[cfg(test)]` et `src/bin` compris, ce crate compris), tests, bancs,
/// exemples, paquets, scripts (avec ou sans extension), manifestes et
/// configurations (`Cargo.toml`, `.cargo/config.toml`, `deny.toml`,
/// `codecov.yml`, `.pre-commit-config.yaml`, `package.json`…), Makefile,
/// workflows : rien ne lit le répertoire personnel, ne construit un chemin
/// `.obrain`, ni ne nomme ou n'énumère une variable de désignation hors du
/// helper.
#[test]
fn rien_dans_le_depot_ne_devine_un_chemin_ni_ne_lit_une_designation() {
    let racine = racine_du_depot();
    let (fautes, rust, autres) = garder(&racine, EXCEPTIONS);
    assert!(
        rust > 400 && autres > 30,
        "le parcours n'a trouvé que {rust} fichiers Rust et {autres} scripts ou manifestes : la \
         garde ne regarde plus le dépôt"
    );
    assert!(
        fautes.is_empty(),
        "le dépôt devine un chemin sous le répertoire personnel, ou lit une désignation hors du \
         helper — passer par obrain_bases_banc (base_lecture, base_copie, copie_de), ou nommer \
         l'exception, à la ligne et au nombre près, dans EXCEPTIONS :\n{}",
        fautes.join("\n")
    );
}

/// Chaque contournement connu, planté dans un faux dépôt, doit être refusé.
#[test]
fn chaque_contournement_plante_est_refuse() {
    let plantes: &[(&str, &str, &str)] = &[
        // Lecture d'une désignation par un intermédiaire.
        (
            "crates/x/tests/a.rs",
            "const V: &str = \"OBRAIN_BENCH_DB_PO\";\nfn f() { let _ = std::env::var_os(V); }\n",
            "OBRAIN_BENCH_DB_",
        ),
        (
            "crates/x/tests/b.rs",
            "fn f() { let v = String::from(\"OBRAIN_BENCH_DB_\") + \"PO\"; }\n",
            "OBRAIN_BENCH_DB_",
        ),
        // Énumération de l'environnement, sous toutes ses formes.
        (
            "crates/x/tests/c.rs",
            "use std::env::{self, vars};\n",
            "env::vars",
        ),
        (
            "crates/x/tests/d.rs",
            "fn f() { for _ in vars() {} }\n",
            "env::vars",
        ),
        (
            "crates/x/tests/e.rs",
            "use std::env::vars_os as tout;\n",
            "env::vars",
        ),
        (
            "crates/x/src/f.rs",
            "fn f() { std::env::vars(); }\n",
            "env::vars",
        ),
        // Répertoire personnel et chemins de bases.
        (
            "crates/x/src/target/g.rs",
            "fn f() { let _ = std::env::var(\"HOME\"); }\n",
            "\"HOME\"",
        ),
        (
            "crates/x/pkg/h.rs",
            "fn f() { let _ = \"/Users/x/.obrain/db/po\"; }\n",
            "/.obrain",
        ),
        (
            "crates/x/src/i.rs",
            "fn f() { let _ = base.join(\".obrain\").join(\"db\"); }\n",
            "join(\".obrain\")",
        ),
        (
            "crates/x/src/j.rs",
            "fn f() { unsafe { libc::getpwuid(0); } }\n",
            "getpwuid",
        ),
        (
            "crates/x/src/k.rs",
            "fn f() { let _ = dirs::data_dir(); }\n",
            "::data_dir(",
        ),
        // Scripts, manifestes, configurations.
        (
            "packages/p/l.py",
            "import os\np = os.path.expanduser(\"x\")\n",
            "expanduser",
        ),
        (
            "packages/p/package.json",
            "{\"x\": \"$HOME/.obrain/db\"}\n",
            "$HOME",
        ),
        (".cargo/config.toml", "[env]\nX = \"~/.obrain/db\"\n", "~/"),
        (
            ".pre-commit-config.yaml",
            "- run: ls $HOME/donnees\n",
            "$HOME",
        ),
        (
            "scripts/sans-extension",
            "#!/bin/sh\nls \"$HOME/donnees\"\n",
            "$HOME",
        ),
        (
            "scripts/m.bat",
            "dir %USERPROFILE%\\donnees\n",
            "%USERPROFILE%",
        ),
    ];
    for (chemin, contenu, motif) in plantes {
        let tmp = tempfile::tempdir().unwrap();
        let fichier = tmp.path().join(chemin);
        fs::create_dir_all(fichier.parent().unwrap()).unwrap();
        fs::write(&fichier, contenu).unwrap();
        let (fautes, _, _) = garder(tmp.path(), &[]);
        assert!(
            fautes
                .iter()
                .any(|f| f.contains(chemin) && f.contains(motif)),
            "contournement NON refusé : {chemin} ({motif}) — fautes : {fautes:?}"
        );
    }
}

/// Ce qui n'est pas une faute ne doit pas en être une : sans quoi on finit
/// par désactiver la garde.
#[test]
fn ce_qui_n_est_pas_une_faute_passe() {
    let propres: &[(&str, &str)] = &[
        (
            "crates/x/src/a.rs",
            "fn f(data_dir: &Path, config_dir: &Path) {}\n",
        ),
        (
            "crates/x/src/b.rs",
            "fn f() { let env_vars = 1; let _ = varsity(); }\n",
        ),
        (
            ".github/workflows/w.yml",
            "- run: echo \"$HOME/.cargo/bin\" >> $GITHUB_PATH\n",
        ),
        ("deny.toml", "db-path = \"~/.cargo/advisory-db\"\n"),
        (
            "crates/x/src/c.rs",
            "// commentaire : ~/.obrain/db et $HOME\n",
        ),
    ];
    for (chemin, contenu) in propres {
        let tmp = tempfile::tempdir().unwrap();
        let fichier = tmp.path().join(chemin);
        fs::create_dir_all(fichier.parent().unwrap()).unwrap();
        fs::write(&fichier, contenu).unwrap();
        let (fautes, _, _) = garder(tmp.path(), &[]);
        assert!(fautes.is_empty(), "faux positif sur {chemin} : {fautes:?}");
    }
}

/// Une exception ne couvre que le NOMBRE d'occurrences qu'elle annonce.
#[test]
fn une_exception_ne_couvre_pas_une_repetition() {
    let tmp = tempfile::tempdir().unwrap();
    let fichier = tmp.path().join("crates/x/tests/t.rs");
    fs::create_dir_all(fichier.parent().unwrap()).unwrap();
    let ligne = "let _ = std::env::var(\"HOME\");";
    fs::write(&fichier, format!("{ligne}\n{ligne}\n")).unwrap();
    let exception = [("crates/x/tests/t.rs", "\"HOME\"", ligne, 1, "essai")];
    let (fautes, _, _) = garder(tmp.path(), &exception);
    assert_eq!(fautes.len(), 1, "{fautes:?}");
    assert!(fautes[0].contains(":2 "), "{fautes:?}");
}

/// Un répertoire illisible n'est pas un répertoire vérifié.
#[cfg(unix)]
#[test]
fn un_repertoire_illisible_fait_echouer_la_garde() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ferme = tmp.path().join("crates/ferme");
    fs::create_dir_all(&ferme).unwrap();
    fs::set_permissions(&ferme, fs::Permissions::from_mode(0o000)).unwrap();
    let echec = std::panic::catch_unwind(|| garder(tmp.path(), &[]));
    fs::set_permissions(&ferme, fs::Permissions::from_mode(0o755)).unwrap();
    let message = echec
        .expect_err("la garde a sauté un répertoire illisible")
        .downcast::<String>()
        .map(|m| *m)
        .unwrap_or_default();
    assert!(message.contains("répertoire illisible"), "{message}");
}
