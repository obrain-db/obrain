//! Garde de l'incident du 2026-11-11 : une base réelle ne s'ouvre que si une
//! variable la désigne — jamais parce qu'elle se trouve sous `$HOME`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use obrain_bases_banc::{Base, copier_arborescence, empreinte, resoudre_avec};

/// Fabrique `<racine>/.obrain/db/{po,wikipedia,megalaw}` avec un fichier
/// chacune : la disposition exacte que les anciennes cibles devinaient.
fn faux_home(racine: &Path) {
    for base in Base::TOUTES {
        let dossier = racine.join(".obrain/db").join(base.nom());
        fs::create_dir_all(&dossier).unwrap();
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
#[should_panic(expected = "n'est pas un répertoire")]
fn une_designation_erronee_echoue_bruyamment() {
    let env = environnement(&[(Base::Po.variable(), Path::new("/chemin/qui/n/existe/pas"))]);
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

/// Fichiers `.rs` sous `dossier`, récursivement.
fn sources(dossier: &Path, trouves: &mut Vec<PathBuf>) {
    let Ok(entrees) = fs::read_dir(dossier) else {
        return;
    };
    for entree in entrees {
        let chemin = entree.unwrap().path();
        if chemin.is_dir() {
            sources(&chemin, trouves);
        } else if chemin.extension().is_some_and(|e| e == "rs") {
            trouves.push(chemin);
        }
    }
}

/// Garde de non-régression sur le code lui-même : aucune cible de test, de
/// banc ou d'exemple du workspace ne lit `HOME` ni ne construit un chemin
/// `.obrain/db`. Une base réelle passe par `obrain_bases_banc` ou par un
/// argument explicite.
#[test]
fn aucune_cible_ne_devine_un_chemin_sous_home() {
    let racine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ce_crate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let mut fichiers = Vec::new();
    for crate_ in fs::read_dir(racine.join("crates")).unwrap() {
        let crate_ = crate_.unwrap().path();
        if crate_.canonicalize().unwrap() == ce_crate {
            continue;
        }
        for cible in ["tests", "benches", "examples"] {
            sources(&crate_.join(cible), &mut fichiers);
        }
        fichiers.push(crate_.join("build.rs"));
    }
    sources(&racine.join("examples"), &mut fichiers);
    sources(&racine.join("scripts"), &mut fichiers);
    assert!(
        fichiers.len() > 100,
        "le parcours n'a trouvé que {} fichiers : la garde ne regarde plus le workspace",
        fichiers.len()
    );

    let interdits = ["\"HOME\"", "\"USERPROFILE\"", "home_dir", ".obrain/db"];
    let mut fautes = Vec::new();
    for fichier in &fichiers {
        // La preuve de l'incident fabrique justement un faux HOME (temporaire).
        if fichier.ends_with("obrain-engine/tests/bases_reelles_non_ouvertes.rs") {
            continue;
        }
        let Ok(texte) = fs::read_to_string(fichier) else {
            continue;
        };
        for (numero, ligne) in texte.lines().enumerate() {
            let code = ligne.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if let Some(motif) = interdits.iter().find(|m| code.contains(**m)) {
                fautes.push(format!(
                    "{}:{} contient {motif} : {}",
                    fichier.display(),
                    numero + 1,
                    code
                ));
            }
        }
    }
    assert!(
        fautes.is_empty(),
        "des cibles devinent encore un chemin de base sous HOME — passer par \
         obrain_bases_banc (OBRAIN_BENCH_DB_*) :\n{}",
        fautes.join("\n")
    );
}
