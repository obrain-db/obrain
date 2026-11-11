//! ── LE CONTRAT DE LA LECTURE SEULE, VU DU MOTEUR ─────────────────────────────
//!
//! `ObrainDB::open_read_only` ne modifie AUCUN octet d'AUCUN fichier de la base,
//! n'en crée aucun, n'en supprime aucun ; toute mutation de la session est
//! refusée par une erreur nommée ; un dossier qui n'est pas une base échoue sans
//! qu'une base vide y soit créée.
//!
//! Les états fabriqués et le mécanisme sont éprouvés au niveau du substrat
//! (`obrain-substrate/tests/lecture_seule_contrat.rs`). Ce fichier éprouve ce
//! que le MOTEUR ajoute autour : l'ouvre-ou-crée de `open`, les catalogues, les
//! index, le journal RDF, les requêtes, la fermeture.

use std::collections::BTreeMap;
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStore as _;
use obrain_engine::ObrainDB;

// ---------------------------------------------------------------------------
// Empreinte
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Trace {
    repertoire: bool,
    taille: u64,
    modifie: SystemTime,
    condense: u64,
}

type Empreinte = BTreeMap<PathBuf, Trace>;

fn condense(chemin: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in std::fs::read(chemin).expect("lire pour l'empreinte") {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Empreinte récursive (liste, taille, date, contenu). La date des RÉPERTOIRES
/// en fait partie : un fichier créé puis supprimé ne laisse que cette trace.
fn empreinte(racine: &Path) -> Empreinte {
    fn dans(racine: &Path, dossier: &Path, e: &mut Empreinte) {
        let m = std::fs::metadata(dossier).expect("metadata du dossier");
        e.insert(
            dossier.strip_prefix(racine).unwrap().to_path_buf(),
            Trace {
                repertoire: true,
                taille: 0,
                modifie: m.modified().unwrap(),
                condense: 0,
            },
        );
        for entree in std::fs::read_dir(dossier).expect("read_dir") {
            let chemin = entree.unwrap().path();
            let m = std::fs::metadata(&chemin).expect("metadata");
            if m.is_dir() {
                dans(racine, &chemin, e);
            } else {
                e.insert(
                    chemin.strip_prefix(racine).unwrap().to_path_buf(),
                    Trace {
                        repertoire: false,
                        taille: m.len(),
                        modifie: m.modified().unwrap(),
                        condense: condense(&chemin),
                    },
                );
            }
        }
    }
    let mut e = Empreinte::new();
    dans(racine, racine, &mut e);
    e
}

#[track_caller]
fn exiger_identique(avant: &Empreinte, racine: &Path, quoi: &str) {
    let apres = empreinte(racine);
    let mut ecarts = Vec::new();
    for (chemin, a) in avant {
        match apres.get(chemin) {
            None => ecarts.push(format!("SUPPRIMÉ   {}", chemin.display())),
            Some(b) if a != b => {
                let mut quoi = Vec::new();
                if a.taille != b.taille {
                    quoi.push(format!("taille {} → {}", a.taille, b.taille));
                }
                if a.condense != b.condense {
                    quoi.push("contenu".to_string());
                }
                if a.modifie != b.modifie {
                    quoi.push("date".to_string());
                }
                ecarts.push(format!(
                    "MODIFIÉ    {} ({})",
                    chemin.display(),
                    quoi.join(", ")
                ));
            }
            Some(_) => {}
        }
    }
    for chemin in apres.keys() {
        if !avant.contains_key(chemin) {
            ecarts.push(format!("CRÉÉ       {}", chemin.display()));
        }
    }
    assert!(
        ecarts.is_empty(),
        "{quoi} : la lecture seule a modifié la base ({} écart(s)) :\n  {}",
        ecarts.len(),
        ecarts.join("\n  ")
    );
}

fn laisser_passer_l_horloge() {
    std::thread::sleep(Duration::from_millis(30));
}

// ---------------------------------------------------------------------------
// Fabrique et lectures
// ---------------------------------------------------------------------------

fn peupler(d: &ObrainDB, debut: usize, n: usize) {
    let s = d.session();
    for i in debut..debut + n {
        s.execute(&format!(
            "INSERT (:Personne {{nom: 'p{i}', age: {i}, bio: 'réacteur numéro {i}'}})"
        ))
        .expect("INSERT");
    }
    s.execute(
        "MATCH (a:Personne), (b:Personne) WHERE b.age = a.age + 1 INSERT (a)-[:CONNAIT]->(b)",
    )
    .expect("arêtes");
}

fn entier(d: &ObrainDB, requete: &str) -> i64 {
    let r = d
        .session()
        .execute(requete)
        .unwrap_or_else(|e| panic!("requête refusée : {requete}\n{e}"));
    match r.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(c)) => *c,
        autre => panic!("{requete} doit rendre un entier, pas {autre:?}"),
    }
}

/// Ce qu'un lecteur voit, par les routes du moteur : compte, filtre indexé,
/// texte, arêtes, agrégat.
fn lire(d: &ObrainDB) -> Vec<i64> {
    vec![
        entier(d, "MATCH (n:Personne) RETURN count(n)"),
        entier(d, "MATCH (n:Personne) WHERE n.nom = 'p3' RETURN count(n)"),
        entier(
            d,
            "MATCH (n:Personne) WHERE n.bio CONTAINS 'réacteur' RETURN count(n)",
        ),
        entier(
            d,
            "MATCH (:Personne)-[r:CONNAIT]->(:Personne) RETURN count(r)",
        ),
        entier(d, "MATCH (n:Personne) WHERE n.age >= 2 RETURN count(n)"),
        entier(d, "MATCH (n:Personne) RETURN sum(n.age)"),
    ]
}

fn base_fermee(chemin: &Path, n: usize) -> Vec<i64> {
    let d = ObrainDB::open(chemin).expect("création");
    peupler(&d, 0, n);
    let vue = lire(&d);
    d.close().expect("close");
    drop(d);
    vue
}

// ---------------------------------------------------------------------------
// Harnais : un écrivain dans un AUTRE processus, tué avant de fermer
// ---------------------------------------------------------------------------

#[test]
fn enfant() {
    let Ok(role) = std::env::var("LSM_ROLE") else {
        return;
    };
    let base = PathBuf::from(std::env::var("LSM_BASE").expect("LSM_BASE"));
    assert_eq!(role, "tue_avant_fermeture");
    let d = ObrainDB::open(&base).expect("création");
    peupler(&d, 0, 12);
    d.checkpoint().expect("checkpoint");
    // Écritures postérieures au dernier checkpoint : elles ne sont que dans le
    // WAL et dans les pages sales des zones.
    let s = d.session();
    for i in 100..110 {
        s.execute(&format!(
            "INSERT (:Personne {{nom: 'p{i}', age: {i}, bio: 'réacteur numéro {i}'}})"
        ))
        .expect("INSERT");
    }
    let vue = lire(&d);
    println!(
        "PRET {}",
        vue.iter().map(i64::to_string).collect::<Vec<_>>().join(" ")
    );
    std::io::stdout().flush().ok();
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Lance l'écrivain, attend qu'il ait écrit, le tue (`kill -9`), rend sa vue.
fn base_d_un_ecrivain_tue(base: &Path) -> Vec<i64> {
    let mut enfant = Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", "enfant", "--nocapture"])
        .env("LSM_ROLE", "tue_avant_fermeture")
        .env("LSM_BASE", base)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("lancer l'enfant");
    let sortie = enfant.stdout.take().expect("stdout");
    let mut vue = None;
    for ligne in std::io::BufReader::new(sortie).lines() {
        let ligne = ligne.expect("lire l'enfant");
        if let Some(reste) = ligne.strip_prefix("PRET ") {
            vue = Some(
                reste
                    .split_whitespace()
                    .map(|v| v.parse::<i64>().expect("entier"))
                    .collect::<Vec<_>>(),
            );
            break;
        }
    }
    enfant.kill().expect("kill -9");
    enfant.wait().expect("attendre l'enfant");
    vue.expect("l'enfant s'est terminé sans annoncer sa vue")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn ouvrir_interroger_fermer_une_base_propre_ne_modifie_rien() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("propre");
    let attendue = base_fermee(&base, 20);
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    for _ in 0..2 {
        let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");
        assert!(d.is_read_only());
        assert_eq!(lire(&d), attendue);
        d.close().expect("fermeture");
    }
    // Et sans `close()` : le `Drop` n'écrit pas davantage.
    {
        let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");
        assert_eq!(lire(&d), attendue);
    }

    exiger_identique(&avant, &base, "base propre");
}

#[test]
fn ecrivain_tue_la_base_est_lue_entiere_et_reste_identique() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("tue");
    let vue_ecrivain = base_d_un_ecrivain_tue(&base);
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    {
        let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");
        assert_eq!(
            lire(&d),
            vue_ecrivain,
            "le lecteur voit ce que l'écrivain avait écrit, checkpointé ou non"
        );
        d.close().expect("fermeture");
    }

    exiger_identique(&avant, &base, "écrivain tué");
}

#[test]
fn un_dossier_qui_n_est_pas_une_base_echoue_sans_rien_creer() {
    let td = tempfile::tempdir().unwrap();
    let parent = td.path().join("bases");
    std::fs::create_dir(&parent).unwrap();
    base_fermee(&parent.join("une"), 2);
    let vide = td.path().join("vide");
    std::fs::create_dir(&vide).unwrap();
    let absent = td.path().join("absent");
    let avant = empreinte(td.path());
    laisser_passer_l_horloge();

    for (quoi, chemin) in [
        ("dossier vide", &vide),
        ("dossier PARENT de bases", &parent),
        ("chemin absent", &absent),
    ] {
        let erreur = match ObrainDB::open_read_only(chemin) {
            Ok(_) => panic!("{quoi} : ouvert en lecture seule comme une base"),
            Err(e) => e.to_string(),
        };
        assert!(
            erreur.contains("read-only") || erreur.contains("lecture seule"),
            "{quoi} : le refus doit dire que la lecture seule ne crée rien : {erreur}"
        );
    }

    exiger_identique(&avant, td.path(), "dossiers qui ne sont pas des bases");
    assert!(!absent.exists(), "le chemin absent a été créé");
    assert_eq!(
        std::fs::read_dir(&vide).unwrap().count(),
        0,
        "une base a été créée dans le dossier vide"
    );
}

/// Chaque famille de mutation, d'abord PROUVÉE valide sur une base inscriptible
/// (sinon un refus de syntaxe passerait pour un refus de lecture seule), puis
/// tentée sur la session en lecture seule.
#[test]
fn les_mutations_d_une_session_en_lecture_seule_sont_refusees_et_n_ecrivent_rien() {
    const MUTATIONS: &[&str] = &[
        "INSERT (:Intrus {nom: 'x'})",
        "MATCH (n:Personne) WHERE n.nom = 'p1' SET n.age = 99",
        "MATCH (n:Personne) WHERE n.nom = 'p1' SET n:Intrus",
        "MATCH (n:Personne) WHERE n.nom = 'p1' REMOVE n.age",
        "MATCH (a:Personne), (b:Personne) WHERE a.nom = 'p1' AND b.nom = 'p2' \
         INSERT (a)-[:INTRUS]->(b)",
        "MATCH (:Personne)-[r:CONNAIT]->(:Personne) DELETE r",
        "MATCH (n:Personne) WHERE n.nom = 'p1' DETACH DELETE n",
        "CREATE GRAPH intrus",
    ];

    let td = tempfile::tempdir().unwrap();
    // Témoin inscriptible : chaque instruction y est acceptée.
    let temoin = td.path().join("temoin");
    {
        let d = ObrainDB::open(&temoin).expect("témoin");
        peupler(&d, 0, 6);
        for m in MUTATIONS {
            d.session()
                .execute(m)
                .unwrap_or_else(|e| panic!("le témoin inscriptible refuse `{m}` : {e}"));
        }
        d.close().unwrap();
    }

    let base = td.path().join("lue");
    let attendue = base_fermee(&base, 6);
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");
    let mut acceptees = Vec::new();
    let mut mal_nommees = Vec::new();
    let mut juger = |nom: &str, resultat: Result<(), String>| match resultat {
        Ok(()) => acceptees.push(nom.to_string()),
        Err(e) if e.contains("read-only") || e.contains("lecture seule") => {}
        Err(e) => mal_nommees.push(format!("{nom} : {e}")),
    };
    for m in MUTATIONS {
        juger(
            m,
            d.session()
                .execute(m)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        );
    }
    // Transaction explicite : elle ne doit pas rouvrir le droit d'écrire.
    {
        let s = d.session();
        let _ = s.execute("START TRANSACTION");
        juger(
            "INSERT dans une transaction explicite",
            s.execute("INSERT (:Intrus)")
                .map(|_| ())
                .map_err(|e| e.to_string()),
        );
        let _ = s.execute("ROLLBACK");
    }
    // Opérations d'administration du moteur.
    juger(
        "ObrainDB::checkpoint",
        d.checkpoint().map_err(|e| e.to_string()),
    );
    // API directes INFAILLIBLES (`create_node` rend un identifiant, pas un
    // `Result`) : elles ne peuvent pas rendre l'erreur, elles paniquent avec
    // son message — avant toute modification.
    let panique = |f: &dyn Fn()| -> Result<(), String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(()) => Ok(()),
            Err(p) => Err(p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panique sans message".to_string())),
        }
    };
    juger(
        "ObrainDB::create_node",
        panique(&|| {
            d.create_node(&["Intrus"]);
        }),
    );
    juger(
        "Session::create_node",
        panique(&|| {
            d.session().create_node(&["Intrus"]);
        }),
    );

    assert!(
        acceptees.is_empty(),
        "mutations ACCEPTÉES par une base en lecture seule : {acceptees:#?}"
    );
    assert!(
        mal_nommees.is_empty(),
        "refus qui ne nomment pas la lecture seule :\n  {}",
        mal_nommees.join("\n  ")
    );

    assert_eq!(lire(&d), attendue, "la vue de la session après les refus");
    d.close().expect("fermeture");
    drop(d);
    exiger_identique(&avant, &base, "mutations refusées");
}

/// Colonne de degrés TYPÉE périmée (payload remis à zéro, CRC recalculé valide :
/// ce que laisse un binaire plus ancien), vue par le moteur. Avant, une session en
/// lecture seule ne pouvait pas guérir (la guérison écrit) : elle éteignait son
/// pré-filtre. Ses colonnes sont des projections privées : la guérison se fait en
/// MÉMOIRE, les requêtes typées sont exactes, et le FICHIER n'est pas touché.
#[test]
fn une_colonne_de_degres_typee_perimee_est_guerie_en_memoire_sans_toucher_au_fichier() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("degres-types");
    {
        let d = ObrainDB::open(&base).expect("création");
        let s = d.session();
        for i in 0..6 {
            s.execute(&format!("INSERT (:N {{k: {i}}})")).unwrap();
        }
        s.execute("MATCH (a:N), (b:N) WHERE b.k = a.k + 1 INSERT (a)-[:TOUCHES]->(b)")
            .unwrap();
        d.close().unwrap();
    }
    let colonne = base.join(obrain_substrate::typed_degree::filename_for_type("TOUCHES"));
    let octets = std::fs::read(&colonne).expect("colonne typée présente");
    let n_slots = u32::from_le_bytes(octets[8..12].try_into().unwrap()) as usize;
    let charge = n_slots * obrain_substrate::degree_column::DEGREE_RECORD_SIZE;
    let mut perime = Vec::with_capacity(octets.len());
    perime.extend_from_slice(&octets[..64]);
    perime.resize(64 + charge, 0u8);
    let crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&perime[64..]);
        h.finalize()
    };
    perime[16..20].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&colonne, &perime).unwrap();
    let avant = empreinte(&base);
    laisser_passer_l_horloge();

    {
        let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");
        assert_eq!(
            entier(&d, "MATCH (:N)-[r:TOUCHES]->(:N) RETURN count(r)"),
            5,
            "arêtes typées"
        );
        assert_eq!(
            entier(
                &d,
                "MATCH (a:N)-[:TOUCHES]->(b:N) WHERE a.k = 0 RETURN count(b)"
            ),
            1
        );
        // La colonne vue par le store est guérie (fiable) — en mémoire.
        let store = d.store();
        assert!(
            store.typed_degree_reliable("TOUCHES"),
            "guérison en mémoire attendue"
        );
        d.close().unwrap();
    }

    exiger_identique(&avant, &base, "colonne de degrés typée périmée");
    assert_eq!(std::fs::read(&colonne).unwrap(), perime, "colonne réécrite");
}
