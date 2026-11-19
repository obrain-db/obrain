//! Mesure la traversée multi-sauts sur un graphe synthétique (obrain#189).
//!
//! Le ticket mesure 1 000 à 24 000× plus lent que Neo4j sur des requêtes PO
//! nommées (`topo_03` : 58,8 s), mais la copie de 3,9 M nœuds n'est pas
//! distribuée — donc personne ne peut itérer. Cette sonde génère un graphe
//! reproductible et chronomètre les mêmes formes.
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example mesure_traversee -- 5000 3
//! ```
//!
//! Elle n'affirme rien : elle imprime. Un seuil en dur sur une machine chargée
//! produirait le flaky qu'on vient de passer deux jours à traquer.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::time::Instant;

fn main() {
    let noeuds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000);

    let branchement: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // Chaîne de `n` nœuds : chaque nœud a une arête vers le suivant, plus une
    // arête « longue » tous les 10 (pour que les sauts aient du branching).
    let depart = Instant::now();
    let ids: Vec<_> = (0..noeuds)
        .map(|indice| {
            session.create_node_with_props(
                &["Topo"],
                [
                    ("i", Value::Int64(indice as i64)),
                    ("k", Value::String(format!("n{indice}").into())),
                ],
            )
        })
        .collect();
    // Branchement `b` : le nœud i a pour enfants b*i+1 .. b*i+b, tant qu'ils
    // existent. b = 1 redonne une chaîne.
    for index in 0..ids.len() {
        for decalage in 1..=branchement {
            let cible = index * branchement + decalage;
            if cible < ids.len() {
                session.create_edge(ids[index], ids[cible], "SUIT");
            }
        }
    }
    println!(
        "construction : {:?} pour {noeuds} nœuds, branchement {branchement}",
        depart.elapsed()
    );

    println!();
    println!("{:<46} {:>12}", "requête", "durée");
    let cas: &[(&str, &str)] = &[
        (
            "saut simple *1",
            "MATCH (a:Topo)-[:SUIT]->(b:Topo) RETURN count(b) AS c",
        ),
        (
            "traversée *2",
            "MATCH (a:Topo)-[:SUIT*2]->(b:Topo) RETURN count(b) AS c",
        ),
        (
            "traversée *3",
            "MATCH (a:Topo)-[:SUIT*3]->(b:Topo) RETURN count(b) AS c",
        ),
        (
            "traversée *2..3",
            "MATCH (a:Topo)-[:SUIT*2..3]->(b:Topo) RETURN count(b) AS c",
        ),
        (
            "traversée *2..6",
            "MATCH (a:Topo)-[:SUIT*2..6]->(b:Topo) RETURN count(b) AS c",
        ),
        (
            "traversée *1..6",
            "MATCH (a:Topo)-[:SUIT*1..6]->(b:Topo) RETURN count(b) AS c",
        ),
        (
            "chemin depuis un seul nœud",
            "MATCH (a:Topo {i: 0})-[:SUIT*2..6]->(b:Topo) RETURN count(b) AS c",
        ),
    ];
    println!();
    println!("lecture : la colonne « par chemin » normalise par le nombre de");
    println!("chemins rendus — c'est elle qui distingue un coût inherent d'une");
    println!("re-expansion (obrain#189).");
    println!();
    for (nom, q) in cas {
        let chrono = Instant::now();
        match session.execute_cypher(q) {
            Ok(resultat) => {
                let chemins = match resultat.rows.first().and_then(|l| l.first()) {
                    Some(Value::Int64(i)) => *i,
                    _ => -1,
                };
                let millis = chrono.elapsed().as_secs_f64() * 1000.0;
                let par_chemin = if chemins > 0 {
                    millis * 1_000.0 / chemins as f64
                } else {
                    0.0
                };
                println!(
                    "{:<46} {:>9.1} ms  ({chemins} chemins, {par_chemin:.2} µs/chemin)",
                    nom, millis
                );
            }
            Err(erreur) => println!(
                "{:<46} ERREUR: {}",
                nom,
                format!("{erreur}")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(60)
                    .collect::<String>()
            ),
        }
    }
}
