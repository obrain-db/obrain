//! Preuve de l'incident du 2026-11-11 : un `cargo test` ordinaire ne doit ni
//! ouvrir ni modifier une base qui se trouve sous `$HOME/.obrain/db/` — et
//! une base DÉSIGNÉE ne doit pas être modifiée non plus, pas d'un octet.
//!
//! Le test fabrique de VRAIES bases Obrain dans un faux `HOME` temporaire,
//! dont une **sale** — la `po`, laissée par un écrivain interrompu : fenêtre
//! WAL non vide, arêtes écrites depuis le dernier checkpoint, un graphe
//! nommé. C'est l'état dans lequel l'incident a pu laisser les bases réelles.
//! Il en prend l'empreinte (liste des fichiers, tailles, dates — répertoires
//! compris —, condensés), puis relance ce binaire en sous-processus (`HOME`
//! est global au processus : on ne le modifie jamais ici) pour y jouer ce que
//! font les bancs.
//!
//! * sans variable `OBRAIN_BENCH_DB_*` : rien n'est ouvert, empreinte
//!   identique (rouge avant le correctif : le banc écrivait dans la base) ;
//! * base désignée, cible lectrice ou écrivaine : la cible travaille sur une
//!   copie, l'empreinte de la base désignée est identique, `substrate.alloc`
//!   compris.
//!
//! # Le défaut moteur que la copie contourne
//!
//! `defaut_moteur_lecture_seule_ecrit_dans_la_base` et
//! `defaut_moteur_lecture_seule_cree_une_base` sont la reproduction minimale
//! de « `open_read_only` écrit sur le disque ». Rouges aujourd'hui, ils sont
//! derrière `cfg(defauts_connus)` (convention du dépôt, voir `Cargo.toml`) :
//!
//! ```bash
//! RUSTFLAGS="--cfg defauts_connus" cargo test -p obrain-engine \
//!   --test bases_reelles_non_ouvertes defaut_moteur -- --nocapture
//! ```
//!
//! Ils passeront au vert, sans y toucher, le jour où le moteur sera corrigé.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use obrain_bases_banc::{Base, Empreinte, base_copie, base_lecture, empreinte};
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Rôle de l'enfant : absent dans un lancement normal.
const ROLE: &str = "OBRAIN_TEST_ENFANT_BASES_BANC";
/// Base que l'enfant « salisseur » doit laisser sale (un tempdir du parent).
const A_SALIR: &str = "OBRAIN_TEST_ENFANT_BASE_A_SALIR";
/// Nœuds de chaque fausse base à son dernier checkpoint.
const NOEUDS: usize = 3;

/// Le corps d'un banc, joué dans le sous-processus. Sans [`ROLE`], ne fait
/// rien : c'est le cas de tout lancement normal de la suite.
#[test]
fn enfant_joue_un_banc() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    if role == "salisseur" {
        salir(Path::new(&std::env::var(A_SALIR).expect(A_SALIR)));
    }
    for base in Base::TOUTES {
        match role.as_str() {
            // Ce que font les bancs qui ne font que lire — deux fois de suite,
            // comme deux tests du même binaire : une seule copie doit servir.
            "lecteur" => {
                for _ in 0..2 {
                    let Some(designee) = base_lecture(base) else {
                        println!("SAUTE {}", base.nom());
                        break;
                    };
                    let db = designee.ouvrir();
                    println!("OUVERT {} noeuds={}", base.nom(), db.store().node_count());
                    let substrat = designee.ouvrir_substrat();
                    drop(substrat);
                }
            }
            // Ce que faisait `po_vs_neo4j_bench` : ouvrir en écriture et créer.
            "ecrivain" => {
                let Some(copie) = base_copie(base) else {
                    println!("SAUTE {}", base.nom());
                    continue;
                };
                let db = ObrainDB::open(copie.chemin()).expect("ouverture en écriture");
                let avant = db.store().node_count();
                db.session()
                    .create_node_with_props(&["_BenchNode"], [("id", Value::Int64(0))]);
                db.checkpoint().expect("checkpoint");
                println!(
                    "OUVERT {} ecrit={}",
                    base.nom(),
                    db.store().node_count() - avant
                );
            }
            autre => panic!("rôle inconnu : {autre}"),
        }
    }
}

/// L'écrivain interrompu : écrit dans la base (nœuds, arêtes, un graphe
/// nommé), ne fait NI checkpoint NI fermeture, et sort sans dérouler la pile —
/// le WAL garde une fenêtre non rejouée, exactement comme après un `kill`.
fn salir(chemin: &Path) -> ! {
    let db = ObrainDB::open(chemin).expect("ouverture de la base à salir");
    let session = db.session();
    let a = session.create_node_with_props(&["File"], [("i", Value::Int64(100))]);
    let b = session.create_node_with_props(&["File"], [("i", Value::Int64(101))]);
    session.create_edge(a, b, "IMPORTS");
    session.create_edge(b, a, "IMPORTS");
    db.create_graph("annexe").expect("graphe nommé");
    let annexe = db.session();
    annexe.use_graph("annexe");
    annexe.create_node_with_props(&["Note"], [("i", Value::Int64(1))]);
    println!("SALIE {}", chemin.display());
    // Ni `drop(db)`, ni checkpoint : `exit` ne déroule pas la pile.
    std::process::exit(0);
}

/// Faux `HOME` : `<racine>/.obrain/db/{po,wikipedia,megalaw}`, trois vraies
/// bases de [`NOEUDS`] nœuds fermées proprement — puis la `po` est salie par
/// un écrivain interrompu.
fn faux_home(racine: &Path) -> PathBuf {
    let db = racine.join(".obrain/db");
    for base in Base::TOUTES {
        let chemin = db.join(base.nom());
        std::fs::create_dir_all(&chemin).unwrap();
        let base_ouverte = ObrainDB::open(&chemin).expect("création de la fausse base");
        let session = base_ouverte.session();
        for i in 0..NOEUDS {
            session.create_node_with_props(&["File"], [("i", Value::Int64(i as i64))]);
        }
        base_ouverte.checkpoint().expect("checkpoint");
    }
    let po = db.join("po");
    let sortie = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "enfant_joue_un_banc", "--nocapture"])
        .env(ROLE, "salisseur")
        .env(A_SALIR, &po)
        .output()
        .expect("lancement du salisseur");
    assert!(
        sortie.status.success() && texte(&sortie.stdout).contains("SALIE"),
        "le salisseur a échoué :\n{}\n{}",
        texte(&sortie.stdout),
        texte(&sortie.stderr)
    );
    let wal = std::fs::metadata(po.join("substrate.wal")).map_or(0, |m| m.len());
    assert!(wal > 0, "la base « sale » n'a pas de fenêtre WAL");
    assert!(
        po.join("graphs/annexe").is_dir(),
        "la base « sale » n'a pas de graphe nommé"
    );
    db
}

/// Relance ce binaire sur le seul test enfant, avec `HOME` = `home`, son
/// propre `TMPDIR` (`tmp`) et les désignations `designations` — aucune autre.
fn lancer_enfant(role: &str, home: &Path, tmp: &Path, designations: &[(Base, &Path)]) -> Output {
    let mut commande = Command::new(std::env::current_exe().unwrap());
    commande
        .args(["--exact", "enfant_joue_un_banc", "--nocapture"])
        .env(ROLE, role)
        .env("HOME", home)
        .env("TMPDIR", tmp)
        .env_remove(obrain_bases_banc::VARIABLE_GARDER_COPIE);
    for base in Base::TOUTES {
        commande.env_remove(base.variable());
    }
    for (base, chemin) in designations {
        commande.env(base.variable(), chemin);
    }
    let sortie = commande.output().expect("lancement du sous-processus");
    assert!(
        sortie.status.success(),
        "l'enfant « {role} » a échoué :\n{}\n{}",
        texte(&sortie.stdout),
        texte(&sortie.stderr)
    );
    sortie
}

fn texte(octets: &[u8]) -> String {
    String::from_utf8_lossy(octets).into_owned()
}

/// Dit ce qui diffère entre deux empreintes, chemin par chemin.
fn ecarts(avant: &Empreinte, apres: &Empreinte) -> Vec<String> {
    let mut lignes = Vec::new();
    for (chemin, etat) in avant {
        let nom = if chemin.as_os_str().is_empty() {
            "(racine)".to_owned()
        } else {
            chemin.display().to_string()
        };
        match apres.get(chemin) {
            None => lignes.push(format!("SUPPRIMÉ  {nom}")),
            Some(nouveau) if nouveau != etat => lignes.push(format!(
                "MODIFIÉ   {nom} (taille {} → {}, date {}, contenu {})",
                etat.taille,
                nouveau.taille,
                if etat.modifie == nouveau.modifie {
                    "inchangée"
                } else {
                    "changée"
                },
                if etat.condense == nouveau.condense {
                    "inchangé"
                } else {
                    "changé"
                }
            )),
            Some(_) => {}
        }
    }
    for chemin in apres.keys().filter(|c| !avant.contains_key(*c)) {
        lignes.push(format!("CRÉÉ      {}", chemin.display()));
    }
    lignes
}

/// La base (ou l'arborescence de bases) est identique à l'octet : liste des
/// fichiers, tailles, dates (répertoires compris), contenus — `substrate.alloc`
/// compris, sans aucune exception.
fn exiger_identique(avant: &Empreinte, racine: &Path, quoi: &str) {
    let ecart = ecarts(avant, &empreinte(racine).unwrap());
    assert!(ecart.is_empty(), "{quoi} :\n{}", ecart.join("\n"));
}

fn dossier_vide(dossier: &Path) -> bool {
    std::fs::read_dir(dossier).unwrap().next().is_none()
}

#[test]
fn sans_designation_les_bases_sous_home_ne_sont_ni_ouvertes_ni_modifiees() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let tmp_enfant = tempfile::tempdir().unwrap();
    let avant = empreinte(&bases).unwrap();
    assert!(avant.len() > 10, "les fausses bases sont vides : {avant:?}");

    for role in ["lecteur", "ecrivain"] {
        let sortie = lancer_enfant(role, tmp.path(), tmp_enfant.path(), &[]);
        let (stdout, stderr) = (texte(&sortie.stdout), texte(&sortie.stderr));
        assert!(
            !stdout.contains("OUVERT"),
            "le banc « {role} » a ouvert une base sous HOME sans qu'elle soit désignée :\n{stdout}"
        );
        for base in Base::TOUTES {
            assert!(
                stdout.contains(&format!("SAUTE {}", base.nom())),
                "le banc « {role} » n'a pas sauté « {} » :\n{stdout}",
                base.nom()
            );
            assert!(
                stderr.contains(&format!("{} n'est pas définie", base.variable())),
                "le saut de « {} » n'est pas annoncé :\n{stderr}",
                base.nom()
            );
        }
        exiger_identique(
            &avant,
            &bases,
            &format!("le banc « {role} » a modifié les bases sous HOME"),
        );
        assert!(dossier_vide(tmp_enfant.path()), "une copie a été prise");
    }
}

#[test]
fn designee_une_cible_lectrice_ne_touche_pas_un_octet_de_la_base() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let home_vide = tempfile::tempdir().unwrap();
    let tmp_enfant = tempfile::tempdir().unwrap();
    let po = bases.join("po");
    let avant = empreinte(&bases).unwrap();

    let sortie = lancer_enfant(
        "lecteur",
        home_vide.path(),
        tmp_enfant.path(),
        &[(Base::Po, &po)],
    );
    let (stdout, stderr) = (texte(&sortie.stdout), texte(&sortie.stderr));
    assert_eq!(
        stdout.matches("OUVERT po noeuds=").count(),
        2,
        "la base désignée n'a pas été lue deux fois :\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains("SAUTE wikipedia") && stdout.contains("SAUTE megalaw"));
    // Deux demandes dans le même binaire, UNE copie.
    assert_eq!(
        stderr
            .matches("BANC [enfant_joue_un_banc] : copie de")
            .count(),
        1,
        "la base a été copiée plus d'une fois pour un seul binaire :\n{stderr}"
    );

    exiger_identique(
        &avant,
        &bases,
        "une cible lectrice a modifié la base désignée (sale : WAL non vide, graphe nommé)",
    );
    assert!(
        dossier_vide(tmp_enfant.path()),
        "la copie de lecture n'a pas été supprimée à la sortie du processus"
    );
}

#[test]
fn designee_une_cible_ecrivaine_n_ecrit_que_dans_sa_copie() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let home_vide = tempfile::tempdir().unwrap();
    let tmp_enfant = tempfile::tempdir().unwrap();
    let po = bases.join("po");
    let avant = empreinte(&bases).unwrap();

    let sortie = lancer_enfant(
        "ecrivain",
        home_vide.path(),
        tmp_enfant.path(),
        &[(Base::Po, &po)],
    );
    let stdout = texte(&sortie.stdout);
    // Le banc a bien écrit (un nœud de plus)…
    assert!(
        stdout.contains("OUVERT po ecrit=1"),
        "le banc n'a pas écrit dans sa copie :\n{stdout}\n{}",
        texte(&sortie.stderr)
    );
    // … mais pas dans la base désignée.
    exiger_identique(
        &avant,
        &bases,
        "le banc a écrit dans la base désignée au lieu de sa copie",
    );
    assert!(
        dossier_vide(tmp_enfant.path()),
        "la copie privée n'a pas été supprimée"
    );
}

// ─────────────── Reproduction du défaut moteur (rouge aujourd'hui) ──────────

/// DÉFAUT MOTEUR — `ObrainDB::open_read_only` ÉCRIT dans la base qu'il ouvre.
///
/// Reproduction minimale : une base laissée par un écrivain interrompu
/// (fenêtre WAL non vide, graphe nommé), ouverte en lecture seule puis
/// refermée, sans une seule requête. Attendu : la base est identique à
/// l'octet. Constaté : des fichiers sont réécrits (voir la sortie du test).
///
/// Cause : `SubstrateFile::open_read_only` appelle `open_inner(path, None,
/// false)` comme `open_shared_write`, et `SubstrateStore::open_read_only` ne
/// pose son drapeau `read_only` qu'APRÈS `from_substrate_partage`, qui a déjà
/// rejoué le WAL, relevé les filigranes dans `substrate.alloc`, rechargé les
/// graphes nommés, etc.
#[cfg(defauts_connus)]
#[test]
fn defaut_moteur_lecture_seule_ecrit_dans_la_base() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let po = bases.join("po");
    let avant = empreinte(&po).unwrap();

    drop(ObrainDB::open_read_only(&po).expect("ouverture en lecture seule"));

    exiger_identique(
        &avant,
        &po,
        "open_read_only a écrit dans la base (aucune requête n'a été faite)",
    );
}

/// DÉFAUT MOTEUR — `ObrainDB::open_read_only` sur un répertoire qui n'est pas
/// une base y CRÉE une base vide, au lieu d'échouer.
#[cfg(defauts_connus)]
#[test]
fn defaut_moteur_lecture_seule_cree_une_base() {
    let tmp = tempfile::tempdir().unwrap();
    let vide = tmp.path().join("pas-une-base");
    std::fs::create_dir(&vide).unwrap();

    let ouverture = ObrainDB::open_read_only(&vide);

    assert!(
        dossier_vide(&vide),
        "open_read_only a créé des fichiers dans un répertoire qui n'était pas une base : {:?}",
        std::fs::read_dir(&vide)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    assert!(
        ouverture.is_err(),
        "open_read_only d'un répertoire sans substrate.meta doit échouer"
    );
}
