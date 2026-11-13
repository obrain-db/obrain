//! Garantie MOTEUR de la lecture seule : une base ouverte par
//! `ObrainDB::open_read_only` refuse toute mutation par une erreur TYPÉE
//! (`Error::Transaction(TransactionError::ReadOnly)`), jamais par une panique,
//! et ce quel que soit le langage ou le chemin d'exécution (GQL, Cypher,
//! DDL d'index, procédure qui écrit).
//!
//! Ces tests ne dépendent d'aucun filtre lexical : le filtre de `obrain-bolt`
//! et de `obrain mcp` est une défense en profondeur, pas la garantie testée ici.

#![cfg(feature = "cypher")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use obrain_common::utils::error::{Error, TransactionError};
use obrain_engine::ObrainDB;

/// Base peuplée en écriture, puis fermée : la lecture seule l'ouvrira ensuite.
fn base_peuplee(dir: &Path) -> PathBuf {
    let chemin = dir.join("base");
    let d = ObrainDB::open(&chemin).expect("ouverture en écriture");
    {
        let s = d.session();
        for i in 0..4 {
            s.execute(&format!("INSERT (:Personne {{nom: 'p{i}', age: {i}}})"))
                .expect("INSERT");
        }
    }
    d.close().expect("fermeture");
    chemin
}

/// Issue d'une tentative de mutation sur un handle en lecture seule.
enum Issue {
    /// Refus typé attendu.
    RefusTypé,
    /// La mutation a été acceptée : faille.
    Acceptée,
    /// Refus, mais pas le bon type d'erreur (texte de l'erreur).
    MalTypée(String),
    /// Panique du moteur : interdit (message de la panique).
    Panique(String),
}

impl Issue {
    fn decrire(&self) -> String {
        match self {
            Issue::RefusTypé => "refus typé".to_string(),
            Issue::Acceptée => "ACCEPTÉE".to_string(),
            Issue::MalTypée(e) => format!("refus NON typé : {e}"),
            Issue::Panique(m) => format!("PANIQUE : {m}"),
        }
    }
}

fn juger<T>(
    requete: &str,
    tentative: impl FnOnce() -> obrain_common::utils::error::Result<T>,
) -> (String, Issue) {
    let resultat = catch_unwind(AssertUnwindSafe(tentative));
    let issue = match resultat {
        Err(p) => Issue::Panique(
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panique sans message".to_string()),
        ),
        Ok(Ok(_)) => Issue::Acceptée,
        Ok(Err(Error::Transaction(TransactionError::ReadOnly))) => Issue::RefusTypé,
        Ok(Err(e)) => Issue::MalTypée(format!("{e:?}")),
    };
    (requete.to_string(), issue)
}

fn bilan(echecs: &[(String, Issue)], attendu: &str) {
    assert!(
        echecs.is_empty(),
        "{attendu} — échecs sur une base en lecture seule :\n  {}",
        echecs
            .iter()
            .map(|(q, i)| format!("{q} => {}", i.decrire()))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

const MUTATIONS_CYPHER: &[&str] = &[
    "CREATE (:Intrus {nom: 'x'})",
    "MATCH (n:Personne) WHERE n.nom = 'p1' SET n.age = 99",
    "MATCH (n:Personne) WHERE n.nom = 'p1' REMOVE n.age",
    "MATCH (n:Personne) WHERE n.nom = 'p1' DELETE n",
    "MERGE (:Intrus {nom: 'x'})",
    "MERGE (n:Personne {nom: 'p1'}) SET n.age = 1",
];

#[test]
fn cypher_create_set_delete_merge_refuses_typed_read_only() {
    let td = tempfile::tempdir().unwrap();
    let base = base_peuplee(td.path());
    let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");

    let mut echecs = Vec::new();
    for q in MUTATIONS_CYPHER {
        let s = d.session();
        let (nom, issue) = juger(q, || s.execute_cypher(q));
        if !matches!(issue, Issue::RefusTypé) {
            echecs.push((nom, issue));
        }
    }
    bilan(
        &echecs,
        "CREATE/SET/DELETE/MERGE Cypher doivent être refusés",
    );
}

#[test]
fn chemins_avec_parametres_refusent_aussi_typed_read_only() {
    use std::collections::HashMap;
    let td = tempfile::tempdir().unwrap();
    let base = base_peuplee(td.path());
    let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");

    let mut echecs = Vec::new();
    // GQL avec paramètres (execute_with_params).
    {
        let s = d.session();
        let q = "INSERT (:Intrus {nom: 'x'})";
        let (nom, issue) = juger(q, || s.execute_with_params(q, HashMap::new()));
        if !matches!(issue, Issue::RefusTypé) {
            echecs.push((format!("GQL params : {nom}"), issue));
        }
    }
    // Cypher avec paramètres (execute_language, branche "cypher" + Some(params)).
    {
        let s = d.session();
        let q = "CREATE (:Intrus {nom: 'x'})";
        let (nom, issue) = juger(q, || s.execute_language(q, "cypher", Some(HashMap::new())));
        if !matches!(issue, Issue::RefusTypé) {
            echecs.push((format!("Cypher params : {nom}"), issue));
        }
    }
    bilan(
        &echecs,
        "les chemins avec paramètres doivent refuser comme les autres",
    );
}

#[test]
fn index_create_and_drop_refused_typed_read_only() {
    let td = tempfile::tempdir().unwrap();
    let base = base_peuplee(td.path());
    let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");

    let requetes = [
        "CREATE INDEX idx_intrus FOR (n:Personne) ON (n.nom)",
        "DROP INDEX idx_intrus",
    ];
    let mut echecs = Vec::new();
    for q in requetes {
        let s = d.session();
        let (nom, issue) = juger(q, || s.execute_cypher(q));
        if !matches!(issue, Issue::RefusTypé) {
            echecs.push((nom, issue));
        }
    }
    bilan(&echecs, "la création/suppression d'index doit être refusée");
}

// Les procédures `CALL` exigent `algos` (plugin d'algorithmes) et les procédures
// cognitives (`obrain.settle`, `obrain.engrams.forget`) exigent `cognitive`.
// Sans elles, l'erreur serait « Unknown procedure », pas le refus recherché.
#[cfg(all(feature = "algos", feature = "cognitive"))]
#[test]
fn procedure_qui_ecrit_refusee_typed_read_only() {
    let td = tempfile::tempdir().unwrap();
    let base = base_peuplee(td.path());
    let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");

    // Procédures qui écrivent : création/suppression de projection (famille 3 de
    // PROCEDURES_DE_LECTURE, plan.rs) et oubli d'engram (RGPD, écrit).
    let requetes = [
        "CALL obrain.projection.drop('p_intrus')",
        "CALL obrain.engrams.forget(1)",
    ];
    let mut echecs = Vec::new();
    for q in requetes {
        let s = d.session();
        let (nom, issue) = juger(q, || s.execute_cypher(q));
        if !matches!(issue, Issue::RefusTypé) {
            echecs.push((nom, issue));
        }
    }
    bilan(&echecs, "une procédure qui écrit doit être refusée");
}

#[test]
fn mutation_en_lecture_seule_ne_panique_jamais() {
    let td = tempfile::tempdir().unwrap();
    let base = base_peuplee(td.path());
    let d = ObrainDB::open_read_only(&base).expect("ouverture en lecture seule");

    let toutes: Vec<&str> = MUTATIONS_CYPHER
        .iter()
        .copied()
        .chain([
            "CREATE INDEX idx_intrus FOR (n:Personne) ON (n.nom)",
            "INSERT (:Intrus {nom: 'x'})",
        ])
        .collect();
    let mut paniques = Vec::new();
    for q in toutes {
        let s = d.session();
        let (nom, issue) = juger(q, || s.execute_cypher(q));
        if let Issue::Panique(m) = issue {
            paniques.push(format!("{nom} => panique : {m}"));
        }
    }
    assert!(
        paniques.is_empty(),
        "le moteur a paniqué sur une base en lecture seule :\n  {}",
        paniques.join("\n  ")
    );
}
