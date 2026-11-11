//! Preuve de l'incident du 2026-11-11 : un `cargo test` ordinaire ne doit ni
//! ouvrir ni modifier une base qui se trouve sous `$HOME/.obrain/db/`.
//!
//! Le test fabrique de VRAIES bases Obrain dans un faux `HOME` temporaire,
//! en prend l'empreinte (liste des fichiers, tailles, dates, condensés),
//! puis relance ce binaire en sous-processus — `HOME` est global au
//! processus, on ne le modifie donc jamais ici — pour y jouer ce que font
//! les bancs : résoudre la base, l'ouvrir, y écrire.
//!
//! * sans variable `OBRAIN_BENCH_DB_*` : rien n'est ouvert, l'empreinte est
//!   identique (rouge avant le correctif : le banc écrivait dans la base) ;
//! * base désignée, cible lectrice : ouverture en lecture seule, empreinte
//!   identique — à l'exception, nommée dans le test, de `substrate.alloc` (la
//!   borne d'allocation partagée, que toute ouverture republie) ;
//! * base désignée, cible écrivaine : l'écriture a lieu dans une copie,
//!   l'empreinte de la base désignée est identique.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use obrain_bases_banc::{Base, Empreinte, base_copie, base_lecture, empreinte};
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Rôle de l'enfant : absent dans un lancement normal.
const ROLE: &str = "OBRAIN_TEST_ENFANT_BASES_BANC";
const NOEUDS: usize = 3;

/// Le corps d'un banc, joué dans le sous-processus. Sans [`ROLE`], ne fait
/// rien : c'est le cas de tout lancement normal de la suite.
#[test]
fn enfant_joue_un_banc() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    for base in Base::TOUTES {
        match role.as_str() {
            // Ce que font les bancs qui ne lisent que.
            "lecteur" => {
                let Some(chemin) = base_lecture(base) else {
                    println!("SAUTE {}", base.nom());
                    continue;
                };
                let db = ObrainDB::open_read_only(&chemin).expect("ouverture en lecture seule");
                println!("OUVERT {} noeuds={}", base.nom(), db.store().node_count());
            }
            // Ce que faisait `po_vs_neo4j_bench` : ouvrir en écriture et créer.
            "ecrivain" => {
                let Some(copie) = base_copie(base) else {
                    println!("SAUTE {}", base.nom());
                    continue;
                };
                let db = ObrainDB::open(&copie).expect("ouverture en écriture");
                db.session()
                    .create_node_with_props(&["_BenchNode"], [("id", Value::Int64(0))]);
                db.checkpoint().expect("checkpoint");
                println!("OUVERT {} noeuds={}", base.nom(), db.store().node_count());
            }
            autre => panic!("rôle inconnu : {autre}"),
        }
    }
}

/// Faux `HOME` : `<racine>/.obrain/db/{po,wikipedia,megalaw}`, trois vraies
/// bases de [`NOEUDS`] nœuds, fermées.
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
    db
}

/// Relance ce binaire sur le seul test enfant, avec `HOME` = `home` et les
/// désignations `designations` — et aucune autre.
fn lancer_enfant(role: &str, home: &Path, designations: &[(Base, &Path)]) -> Output {
    let mut commande = Command::new(std::env::current_exe().unwrap());
    commande
        .args(["--exact", "enfant_joue_un_banc", "--nocapture"])
        .env(ROLE, role)
        .env("HOME", home)
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
        String::from_utf8_lossy(&sortie.stdout),
        String::from_utf8_lossy(&sortie.stderr)
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
        match apres.get(chemin) {
            None => lignes.push(format!("SUPPRIMÉ  {}", chemin.display())),
            Some(nouveau) if nouveau != etat => lignes.push(format!(
                "MODIFIÉ   {} (taille {} → {}, date {}, contenu {})",
                chemin.display(),
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

#[test]
fn sans_designation_les_bases_sous_home_ne_sont_ni_ouvertes_ni_modifiees() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let avant = empreinte(&bases).unwrap();
    assert!(avant.len() > 3, "les fausses bases sont vides : {avant:?}");

    for role in ["lecteur", "ecrivain"] {
        let sortie = lancer_enfant(role, tmp.path(), &[]);
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
        let ecart = ecarts(&avant, &empreinte(&bases).unwrap());
        assert!(
            ecart.is_empty(),
            "le banc « {role} » a modifié les bases sous HOME :\n{}",
            ecart.join("\n")
        );
    }
}

#[test]
fn designee_une_cible_lectrice_laisse_la_base_intacte() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let home_vide = tempfile::tempdir().unwrap();
    let po = bases.join("po");
    let avant = empreinte(&bases).unwrap();

    let sortie = lancer_enfant("lecteur", home_vide.path(), &[(Base::Po, &po)]);
    let stdout = texte(&sortie.stdout);
    assert!(
        stdout.contains(&format!("OUVERT po noeuds={NOEUDS}")),
        "la base désignée n'a pas été lue :\n{stdout}\n{}",
        texte(&sortie.stderr)
    );
    assert!(stdout.contains("SAUTE wikipedia") && stdout.contains("SAUTE megalaw"));

    // UNE exception, nommée : `substrate.alloc`, la borne d'allocation partagée
    // entre processus (4 Kio, `borne_allocation.rs`). Toute ouverture — lecture
    // seule comprise — y republie le filigrane relevé des zones (`fetch_max`,
    // monotone). Ce n'est pas une donnée : les zones font foi et la borne est
    // recalculée à chaque ouverture. Tout le reste doit être identique à l'octet.
    let ecart: Vec<String> = ecarts(&avant, &empreinte(&bases).unwrap())
        .into_iter()
        .filter(|ligne| !ligne.starts_with("MODIFIÉ   po/substrate.alloc "))
        .collect();
    assert!(
        ecart.is_empty(),
        "l'ouverture en lecture seule a modifié la base désignée :\n{}",
        ecart.join("\n")
    );
}

#[test]
fn designee_une_cible_ecrivaine_n_ecrit_que_dans_sa_copie() {
    let tmp = tempfile::tempdir().unwrap();
    let bases = faux_home(tmp.path());
    let home_vide = tempfile::tempdir().unwrap();
    let po = bases.join("po");
    let avant = empreinte(&bases).unwrap();

    let sortie = lancer_enfant("ecrivain", home_vide.path(), &[(Base::Po, &po)]);
    let stdout = texte(&sortie.stdout);
    // Le banc a bien écrit (un nœud de plus)…
    assert!(
        stdout.contains(&format!("OUVERT po noeuds={}", NOEUDS + 1)),
        "le banc n'a pas écrit dans sa copie :\n{stdout}\n{}",
        texte(&sortie.stderr)
    );
    // … mais pas dans la base désignée.
    let ecart = ecarts(&avant, &empreinte(&bases).unwrap());
    assert!(
        ecart.is_empty(),
        "le banc a écrit dans la base désignée au lieu de sa copie :\n{}",
        ecart.join("\n")
    );
    let relue = ObrainDB::open_read_only(&po).unwrap();
    assert_eq!(relue.store().node_count(), NOEUDS);
}
