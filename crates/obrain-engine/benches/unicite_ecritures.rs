//! Coût PAR ÉCRITURE de chaque chemin soumis à une contrainte d'unicité, sur
//! une base de N nœuds — pour vérifier qu'il ne croît pas avec N.
//!
//! Ce n'est pas un banc Criterion : chaque mesure porte sur une base de
//! plusieurs centaines de milliers de nœuds qu'on ne reconstruit pas par
//! échantillon. Le banc bâtit UNE base sur disque, y rejoue chaque chemin
//! d'écriture sous chaque configuration de contrainte, et imprime le coût
//! moyen par instruction (analyse et planification comprises).
//!
//! Le WAL est ouvert sans fsync par commit (`SyncMode::Never`) : ce coût est
//! le même avec ou sans contrainte, et noierait celui du contrôle.
//!
//! Données : `(:T {a: i, g: i % 2, b: 'v<i>'})` — `a` et `b` uniques, `g` à
//! deux valeurs. La base est semée par l'API directe du store (hors
//! contrainte), les contraintes sont déclarées ensuite.
//!
//! Configurations :
//! - `sans_contrainte`        : plancher ;
//! - `unique_a`               : `UNIQUE (a)` ;
//! - `unique_b_g`             : `UNIQUE (b, g)`, colonne sélective en tête ;
//! - `unique_g_b`             : `UNIQUE (g, b)`, colonne à deux valeurs en tête
//!   (peu d'opérations : tant que le store résout par la PREMIÈRE colonne
//!   indexée, ce cas relit la moitié du label).
//!
//! Chemins : `INSERT`, `SET` d'une colonne contrainte, `SET` d'une colonne
//! libre (témoin), `MERGE` qui trouve (`ON MATCH SET`), `MERGE` qui crée.
//!
//! Variables d'environnement :
//! - `UNICITE_N`   : taille de la base (défaut 100 000) ;
//! - `UNICITE_OPS` : opérations par mesure (défaut 2 000) ;
//! - `UNICITE_DIR` : dossier de la base (défaut `<crate>/target/banc-unicite`).
//!   Il est SUPPRIMÉ à la fin.
//!
//! Rejouer :
//! `UNICITE_N=1000000 cargo bench -p obrain-engine --features cypher --bench unicite_ecritures`

use std::path::PathBuf;
use std::time::Instant;

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

fn env_usize(nom: &str, defaut: usize) -> usize {
    std::env::var(nom)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(defaut)
}

/// Exécute `ops` instructions et rend le coût moyen en microsecondes.
fn mesurer(db: &ObrainDB, ops: usize, requete: impl Fn(usize) -> String) -> f64 {
    let session = db.session();
    // Une instruction d'échauffement : le premier usage d'une clé bâtit son
    // index (coût payé une fois par processus, hors mesure).
    session
        .execute_cypher(&requete(ops))
        .unwrap_or_else(|e| panic!("échauffement : {e}"));
    let debut = Instant::now();
    for j in 0..ops {
        session
            .execute_cypher(&requete(j))
            .unwrap_or_else(|e| panic!("instruction {j} : {e}"));
    }
    debut.elapsed().as_secs_f64() * 1e6 / ops as f64
}

fn main() {
    let n = env_usize("UNICITE_N", 100_000);
    let ops = env_usize("UNICITE_OPS", 2_000);
    assert!(
        n > 2 * ops + 2,
        "UNICITE_N doit dépasser 2 × UNICITE_OPS + 2"
    );
    let dossier = std::env::var("UNICITE_DIR").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/banc-unicite"),
        PathBuf::from,
    );
    let _ = std::fs::remove_dir_all(&dossier);
    std::fs::create_dir_all(&dossier).expect("dossier du banc");
    let chemin = dossier.join("base");

    // Sans fsync par commit : il coûte pareil avec ou sans contrainte et
    // masquerait (4 ms) les nanosecondes du contrôle qu on veut isoler.
    let db = ObrainDB::open_substrate_with_mode(&chemin, obrain_substrate::wal_io::SyncMode::Never)
        .expect("ouverture de la base de banc");
    let debut = Instant::now();
    {
        let store = db.graph_store();
        let (cle_a, cle_g, cle_b, cle_f) = (
            PropertyKey::new("a"),
            PropertyKey::new("g"),
            PropertyKey::new("b"),
            PropertyKey::new("f"),
        );
        for i in 0..n as i64 {
            store.create_node_with_props(
                &["T"],
                &[
                    (cle_a.clone(), Value::Int64(i)),
                    (cle_g.clone(), Value::Int64(i % 2)),
                    (cle_b.clone(), Value::from(format!("v{i}"))),
                    (cle_f.clone(), Value::Float64(i as f64 + 0.5)),
                ],
            );
        }
    }
    // Le MATCH qui précède un SET doit trouver son nœud par l'index, sinon
    // c'est le balayage du MATCH qu'on mesurerait, pas le SET.
    db.create_property_index("a");
    eprintln!(
        "base semée : {n} nœuds en {:.1} s ({})",
        debut.elapsed().as_secs_f64(),
        chemin.display()
    );

    let configurations: [(&str, Option<&str>, usize); 5] = [
        ("sans_contrainte", None, ops),
        (
            "unique_a",
            Some("CREATE CONSTRAINT banc FOR (n:T) REQUIRE n.a IS UNIQUE"),
            ops,
        ),
        (
            "unique_b_g",
            Some("CREATE CONSTRAINT banc FOR (n:T) REQUIRE (n.b, n.g) IS UNIQUE"),
            ops,
        ),
        (
            "unique_g_b",
            Some("CREATE CONSTRAINT banc FOR (n:T) REQUIRE (n.g, n.b) IS UNIQUE"),
            ops.min(5),
        ),
        // Colonne FLOTTANTE non entière : l'index d'égalité ne la projette pas.
        (
            "unique_f",
            Some("CREATE CONSTRAINT banc FOR (n:T) REQUIRE n.f IS UNIQUE"),
            ops.min(5),
        ),
    ];

    println!("N = {n}, µs par instruction");
    println!(
        "{:<18} {:>6} {:>10} {:>12} {:>12} {:>12} {:>12}",
        "configuration", "ops", "INSERT", "SET contr.", "SET libre", "MERGE trouve", "MERGE crée"
    );
    // Chaque configuration écrit dans sa propre plage de valeurs neuves.
    let mut base = n as i64;
    for (nom, contrainte, ops) in configurations {
        let session = db.session();
        if let Some(c) = contrainte {
            let debut = Instant::now();
            session.execute_cypher(c).expect("CREATE CONSTRAINT");
            eprintln!(
                "{nom} : CREATE CONSTRAINT sur {n} nœuds en {:.0} ms",
                debut.elapsed().as_secs_f64() * 1e3
            );
        }
        let pas = (ops + 1) as i64;
        let plage = |k: i64| base + k * pas;

        let p = plage(0);
        let insert = mesurer(&db, ops, |j| {
            let v = p + j as i64;
            format!("CREATE (:T {{a: {v}, g: {}, b: 'i{v}', f: {v}.25}})", v % 2)
        });
        // SET d'une colonne contrainte vers une valeur neuve, sur un nœud
        // existant trouvé par l'index de `a`.
        let p = plage(1);
        let sur_a = nom == "unique_a";
        let sur_f = nom == "unique_f";
        let set_contrainte = mesurer(&db, ops, |j| {
            let v = p + j as i64;
            if sur_f {
                format!("MATCH (n:T {{a: {j}}}) SET n.f = {v}.75")
            } else if sur_a {
                // `a` est la colonne contrainte : on la réécrit sur des nœuds
                // que les mesures suivantes ne cherchent pas.
                format!("MATCH (n:T {{a: {}}}) SET n.a = {v}", ops + 1 + j)
            } else {
                format!("MATCH (n:T {{a: {j}}}) SET n.b = 's{v}'")
            }
        });
        let set_libre = mesurer(&db, ops, |j| {
            format!("MATCH (n:T {{a: {j}}}) SET n.libre = {j}")
        });
        let p = plage(2);
        let merge_trouve = mesurer(&db, ops, |j| {
            let v = p + j as i64;
            if sur_f {
                format!("MERGE (n:T {{a: {j}}}) ON MATCH SET n.f = {v}.8125")
            } else {
                format!("MERGE (n:T {{a: {j}}}) ON MATCH SET n.b = 'm{v}'")
            }
        });
        let p = plage(3);
        let merge_cree = mesurer(&db, ops, |j| {
            let v = p + j as i64;
            format!(
                "MERGE (n:T {{a: {v}}}) ON CREATE SET n.g = {}, n.b = 'c{v}', n.f = {v}.125",
                v % 2
            )
        });
        println!(
            "{nom:<18} {ops:>6} {insert:>10.1} {set_contrainte:>12.1} {set_libre:>12.1} \
             {merge_trouve:>12.1} {merge_cree:>12.1}"
        );
        base = plage(4);
        if contrainte.is_some() {
            session
                .execute_cypher("DROP CONSTRAINT banc")
                .expect("DROP CONSTRAINT");
        }
    }

    drop(db);
    std::fs::remove_dir_all(&dossier).expect("suppression de la base de banc");

    parallele(&dossier);
    let _ = std::fs::remove_dir_all(&dossier);
}

/// Débit de `CREATE` de N fils sur une base DURABLE (fsync à chaque commit),
/// sans contrainte puis sous `UNIQUE (a)` : le verrou d'unicité sérialise les
/// écrivains contraints du processus, ce qui se paie ici si le fsync passe
/// sous le verrou.
fn parallele(dossier: &std::path::Path) {
    const PAR_FIL: usize = 300;
    println!("\nécritures parallèles, WAL durable : CREATE par seconde (total)");
    println!(
        "{:<18} {:>10} {:>10} {:>10}",
        "configuration", "1 fil", "4 fils", "8 fils"
    );
    for (nom, contrainte) in [
        ("sans_contrainte", None),
        (
            "unique_a",
            Some("CREATE CONSTRAINT banc FOR (n:T) REQUIRE n.a IS UNIQUE"),
        ),
    ] {
        let mut debits = Vec::new();
        for fils in [1usize, 4, 8] {
            let chemin = dossier.join(format!("par-{nom}-{fils}"));
            let _ = std::fs::remove_dir_all(&chemin);
            std::fs::create_dir_all(dossier).expect("dossier");
            let db = std::sync::Arc::new(ObrainDB::open_substrate(&chemin).expect("ouverture"));
            if let Some(c) = contrainte {
                db.session().execute_cypher(c).expect("CREATE CONSTRAINT");
            }
            let debut = Instant::now();
            let mut hs = Vec::new();
            for t in 0..fils {
                let db = std::sync::Arc::clone(&db);
                hs.push(std::thread::spawn(move || {
                    let s = db.session();
                    for j in 0..PAR_FIL {
                        let v = t * PAR_FIL + j;
                        s.execute_cypher(&format!("CREATE (:T {{a: {v}}})"))
                            .expect("CREATE");
                    }
                }));
            }
            for h in hs {
                h.join().unwrap();
            }
            debits.push((fils * PAR_FIL) as f64 / debut.elapsed().as_secs_f64());
            drop(db);
            let _ = std::fs::remove_dir_all(&chemin);
        }
        println!(
            "{nom:<18} {:>10.0} {:>10.0} {:>10.0}",
            debits[0], debits[1], debits[2]
        );
    }
}
