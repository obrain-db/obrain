//! (#205) IMPORT EN LOT DE GROSSES VALEURS, INTERROMPU : un `kill -9` pendant
//! `bulk_import_props` ne doit laisser ni panique, ni valeur FAUSSE.
//!
//! Le lot écrit les blobs (valeurs > 256 o) droit dans leurs colonnes, sans
//! trame WAL par valeur : avant le checkpoint de l'appelant, une coupure peut
//! en laisser absents. C'est le contrat (un import interrompu se REFAIT). Ce
//! que le test fige, c'est ce qui ne doit JAMAIS arriver : une valeur relue
//! différente de celle écrite (le défaut d'obrain#120 : « un nœud relit les
//! octets d'un autre »), une base qui ne s'ouvre plus, ou une écriture
//! ordinaire qui échoue ensuite.
//!
//! Protocole de `import_en_masse_kill9` : le parent relance son binaire en
//! enfant ; l'enfant importe puis dort ; le parent attend le marqueur, tue -9.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::path::Path;
use std::time::Duration;

const N: usize = 120_000;
const PAQUET: usize = 10_000;
const ENV_DIR: &str = "OBRAIN_TEST_IMPORT_BLOBS_KILL9_DIR";
const CAS_ENFANT: &str = "enfant_importe_des_blobs_puis_dort";

fn donnee(i: usize) -> String {
    format!("d{i}-{}", "q".repeat(1_500))
}

#[test]
fn enfant_importe_des_blobs_puis_dort() {
    let Ok(dir) = std::env::var(ENV_DIR) else {
        return;
    };
    let db = ObrainDB::open_substrate(&dir).expect("création");
    let labels: Vec<&str> = vec!["Function"; N];
    let src: Vec<u32> = (1..N as u32).collect();
    let dst: Vec<u32> = (2..=N as u32).collect();
    let types: Vec<&str> = vec!["CALLS"; N - 1];
    db.bulk_import_refs(&labels, &src, &dst, &types)
        .expect("import en masse");
    for p in 0..N / PAQUET {
        let props: Vec<Vec<(&str, Value)>> = (p * PAQUET..(p + 1) * PAQUET)
            .map(|i| {
                vec![
                    ("name", Value::from(format!("f{i}"))),
                    ("data", Value::from(donnee(i))),
                ]
            })
            .collect();
        db.bulk_import_props((p * PAQUET) as u32 + 1, &props)
            .expect("props");
        if p == 0 {
            // Le premier paquet est écrit : le parent tue pendant les suivants.
            std::fs::write(Path::new(&dir).join("import.marqueur"), b"ok").unwrap();
        }
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[test]
fn un_kill9_pendant_l_import_de_blobs_ne_laisse_aucune_valeur_fausse() {
    if std::env::var(ENV_DIR).is_ok() {
        return;
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
        assert!(
            tours < 60_000,
            "l'enfant n'a pas fini son 1er paquet en 60 s"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    enfant.kill().expect("kill -9");
    let _ = enfant.wait();

    // Le lot a bien été pris : le chemin unitaire journalise chaque valeur
    // entière (>= 15 Mo dès le premier paquet de 10 000 × 1,5 Ko), le lot une
    // trame par clé et par appel.
    let wal = std::fs::metadata(dir.join("substrate.wal")).map_or(0, |m| m.len());
    assert!(
        wal < 4 * 1024 * 1024,
        "le WAL porte {wal} o : les blobs ont pris le chemin unitaire"
    );
    let db = ObrainDB::open_substrate(dir.to_str().unwrap()).expect("réouverture");
    let s = db.session();
    let r = s
        .execute("MATCH (n) RETURN n.name, n.data")
        .expect("lecture après kill -9");
    let (mut lues, mut absentes) = (0usize, 0usize);
    for row in &r.rows {
        let (Some(Value::String(nom)), data) = (row.first(), row.get(1)) else {
            continue; // nom absent : nœud dont les propriétés ne sont pas arrivées
        };
        let i: usize = nom.trim_start_matches('f').parse().expect("nom fN");
        match data {
            Some(Value::String(t)) => {
                assert_eq!(t.as_str(), donnee(i), "valeur FAUSSE pour le nœud {i}");
                lues += 1;
            }
            Some(Value::Null) | None => absentes += 1,
            autre => panic!("type inattendu pour data du nœud {i} : {autre:?}"),
        }
    }
    eprintln!("RELU après kill -9 : {lues} blobs exacts, {absentes} absents");
    // Une écriture ordinaire d'un blob après la reprise : relue exactement, et
    // sans altérer celle d'un autre nœud.
    let neuf = "N".repeat(900);
    s.execute(&format!("MATCH (n {{name: 'f5'}}) SET n.data = '{neuf}'"))
        .expect("écriture après reprise");
    let r = s
        .execute("MATCH (n {name: 'f5'}) RETURN n.data")
        .expect("relecture");
    assert_eq!(r.rows[0][0], Value::from(neuf));
}
