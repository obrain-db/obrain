//! IMPORT EN MASSE INTERROMPU : le registre des types d'arêtes doit survivre à
//! un `kill -9` qui suit l'écriture des arêtes par les zones.
//!
//! Défaut mesuré sur le noyau Linux (03/11/2026) : `bulk_import_labels_edges`
//! écrit les arêtes et interne leurs types, mais n'écrit PAS `substrate.dict`.
//! Le dictionnaire n'est persisté que par un `checkpoint` ultérieur. Un
//! `kill -9` entre les deux laisse des arêtes dont le type n'a pas de nom à la
//! réouverture ; l'écriture incrémentale suivante ré-alloue alors des ids déjà
//! pris et panique (`EdgeTypeRegistry : l'id 9 porte déjà ...`).
//!
//! Protocole identique à `durabilite_commit_kill9` : le parent relance son
//! propre binaire en enfant ; l'enfant importe puis dort ; le parent attend le
//! marqueur, tue -9, rouvre et vérifie.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::time::Duration;

/// Taille de l'import : assez grande pour que le kill tombe pendant les propriétés.
const N: usize = 600_000;
const ENV_DIR: &str = "OBRAIN_TEST_IMPORT_KILL9_DIR";
// Nom de test SANS préfixe de module : c'est ce que `--exact` attend.
const CAS_ENFANT: &str = "enfant_importe_puis_dort";

fn compte(db: &ObrainDB, requete: &str) -> i64 {
    let s = db.session();
    let r = s.execute(requete).expect("requête");
    match r.rows.first().and_then(|row| row.first()) {
        Some(Value::Int64(n)) => *n,
        autre => panic!("compte inattendu pour {requete} : {autre:?}"),
    }
}

#[test]
fn enfant_importe_puis_dort() {
    let Ok(dir) = std::env::var(ENV_DIR) else {
        return; // exécuté seul : rien à faire
    };
    let db = ObrainDB::open_substrate(&dir).expect("création");
    // Même forme que le chemin natif du noyau : nœuds+arêtes par les zones,
    // PUIS propriétés par paquets. Le marqueur part après les arêtes ; le parent
    // tue pendant l'écriture des propriétés, comme sur le noyau.
    let labels: Vec<&str> = vec!["Function"; N];
    let src: Vec<u32> = (1..N as u32).collect();
    let dst: Vec<u32> = (2..=N as u32).collect();
    let types: Vec<&str> = (0..N - 1)
        .map(|i| match i % 3 {
            0 => "CALLS",
            1 => "PARENT",
            _ => "FILE_CHANGES_WITH",
        })
        .collect();
    db.bulk_import_refs(&labels, &src, &dst, &types)
        .expect("import en masse");
    std::fs::write(Path::new(&dir).join("import.marqueur"), b"ok").unwrap();
    let props: Vec<Vec<(&str, obrain_common::types::Value)>> = (0..N)
        .map(|i| vec![("name", Value::from(format!("f{i}")))])
        .collect();
    for (p, bloc) in props.chunks(100_000).enumerate() {
        db.bulk_import_props((p * 100_000) as u32 + 1, bloc)
            .expect("props");
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[test]
fn le_registre_des_types_survit_a_un_kill9_pendant_l_import() {
    if std::env::var(ENV_DIR).is_ok() {
        return; // on est l'enfant
    }
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("import.obrain");
    let marqueur = dir.join("import.marqueur");

    let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CAS_ENFANT, "--nocapture", "--test-threads=1"])
        .env(ENV_DIR, &dir)
        .spawn()
        .expect("lancement de l'enfant");
    let mut tours = 0u32;
    while !marqueur.exists() {
        tours += 1;
        assert!(tours < 2400, "l'enfant n'a pas fini son import en 60 s");
        std::thread::sleep(Duration::from_millis(25));
    }
    // Mesure : où en est l'enfant quand on le tue (fichiers de propriétés déjà là ?).
    // Tuer PENDANT l'écriture des propriétés : on attend que le fichier de
    // propriétés soit engagé (>= 32 Mo), comme sur le noyau au moment du défaut.
    let props = dir.join("substrate.props.v2");
    let mut tours = 0u32;
    while std::fs::metadata(&props).map_or(0, |m| m.len()) < 32 * 1024 * 1024 {
        tours += 1;
        assert!(
            tours < 2400,
            "les propriétés ne se sont pas engagées en 60 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let props_avant = std::fs::metadata(&props).map_or(0, |m| m.len());
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    // Réouverture : comptes EXACTS attendus, relus depuis le disque.
    let db = ObrainDB::open_substrate(dir.to_str().unwrap()).expect("réouverture");
    let total = compte(&db, "MATCH ()-[r]->() RETURN count(r)");
    let calls = compte(&db, "MATCH ()-[r:CALLS]->() RETURN count(r)");
    let parent = compte(&db, "MATCH ()-[r:PARENT]->() RETURN count(r)");
    let fcw = compte(&db, "MATCH ()-[r:FILE_CHANGES_WITH]->() RETURN count(r)");
    let noeuds = compte(&db, "MATCH (n) RETURN count(n)");
    eprintln!(
        "RELU après kill -9 : noeuds={noeuds} aretes={total} CALLS={calls} PARENT={parent} FCW={fcw} (props_avant={props_avant})"
    );
    assert_eq!(total, (N - 1) as i64, "toutes les arêtes importées sont là");
    let attendu = |k: usize| (0..N - 1).filter(|i| i % 3 == k).count() as i64;
    assert_eq!(calls, attendu(0), "les arêtes CALLS gardent leur type");
    assert_eq!(
        parent,
        attendu(1),
        "le type PARENT est nommé après un kill -9 pendant l'import"
    );
    assert_eq!(fcw, attendu(2), "le type FILE_CHANGES_WITH est nommé");
    // L'écriture incrémentale qui suit ne doit ni paniquer ni réutiliser un id :
    // on crée un type d'arête JAMAIS vu, puis on relit tout le registre.
    let a = obrain_common::types::NodeId(1);
    let b = obrain_common::types::NodeId(2);
    db.create_edge(a, b, "NOUVEAU");
    assert_eq!(compte(&db, "MATCH ()-[r:NOUVEAU]->() RETURN count(r)"), 1);
    assert_eq!(
        compte(&db, "MATCH ()-[r:PARENT]->() RETURN count(r)"),
        parent
    );
}
