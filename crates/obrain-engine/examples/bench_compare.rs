//! Banc comparatif obrain / Neo4j sur une base réelle (obrain#189, #181).
//!
//! Ouvre la base obrain **une seule fois** et répète chaque requête, pour que
//! toutes les mesures soient à chaud — un premier accès coûte 11× le suivant
//! (mesuré), et comparer un froid à un chaud ne veut rien dire.
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher,algos \
//!     --example bench_compare -- /chemin/vers/base
//! ```
//!
//! Les temps Neo4j se mesurent à côté (HTTP), avec la même liste de requêtes.

use std::time::Instant;

const REQUETES: &[(&str, &str)] = &[
    ("count(Task)", "MATCH (t:Task) RETURN count(t) AS c"),
    (
        "3 sauts Project→Plan→Task",
        "MATCH (p:Project)-[:HAS_PLAN]->(pl:Plan)-[:HAS_TASK]->(t:Task) RETURN count(t) AS c",
    ),
    (
        "3 sauts + WHERE propriete",
        "MATCH (p:Project)-[:HAS_PLAN]->(pl:Plan)-[:HAS_TASK]->(t:Task) \
         WHERE t.status = 'completed' RETURN count(t) AS c",
    ),
    (
        "traversee variable CO_CHANGED*1..2",
        "MATCH (c:Commit)-[:CO_CHANGED*1..2]->(d:Commit) RETURN count(d) AS c",
    ),
];

fn main() {
    let chemin = std::env::args()
        .nth(1)
        .expect("usage : bench_compare <repertoire>");
    let repetitions: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let db = obrain_engine::ObrainDB::open(&chemin).expect("ouverture de la base");
    let session = db.session();

    println!("base : {chemin}");
    println!("repetitions : {repetitions} (on garde la mediane, apres echauffement)");
    println!();
    println!(
        "{:<38} {:>10} {:>10} {:>12}",
        "requete", "mediane", "min", "lignes"
    );

    for (nom, requete) in REQUETES {
        // Échauffement : une exécution jetée, pour ne pas mesurer le mmap froid.
        let _ = session.execute_cypher(requete);

        let mut durees: Vec<f64> = Vec::with_capacity(repetitions);
        let mut lignes = 0usize;
        for _ in 0..repetitions {
            let depart = Instant::now();
            match session.execute_cypher(requete) {
                Ok(resultat) => {
                    lignes = resultat.rows.len();
                    durees.push(depart.elapsed().as_secs_f64() * 1000.0);
                }
                Err(erreur) => {
                    println!(
                        "{nom:<38} ERREUR: {}",
                        format!("{erreur}").lines().next().unwrap_or("")
                    );
                    durees.clear();
                    break;
                }
            }
        }
        if durees.is_empty() {
            continue;
        }
        durees.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mediane = durees[durees.len() / 2];
        println!(
            "{nom:<38} {:>8.1} ms {:>8.1} ms {:>12}",
            mediane, durees[0], lignes
        );
    }
}
