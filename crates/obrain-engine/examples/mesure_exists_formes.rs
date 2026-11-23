//! Quelles formes de `EXISTS { … }` à extrémité liée coûtent encore le graphe entier
//! après #345 (obrain#189) ? Même graphe synthétique (projet ⊃ fichiers ⊃ fonctions,
//! `CALLS` entre fonctions), mêmes trois tailles, une requête par forme.
//!
//! Le critère n'est pas la valeur d'une durée, c'est sa **croissance** : si la durée
//! suit la taille du graphe alors que le résultat ne change pas, le sous-motif est
//! encore énuméré pour tout le graphe. Les formes viennent des requêtes de PO que
//! #345 n'a PAS accélérées (`code_24`, `code_33`, `code_40`).
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example mesure_exists_formes -- 2000 8000 32000
//! ```
//!
//! Elle n'affirme rien : elle imprime.

#[cfg(feature = "cypher")]
use obrain_common::types::Value;
#[cfg(feature = "cypher")]
use obrain_engine::ObrainDB;
#[cfg(feature = "cypher")]
use std::fmt::Write;
#[cfg(feature = "cypher")]
use std::time::Instant;

#[cfg(feature = "cypher")]
const FORMES: &[(&str, &str)] = &[
    (
        "A  MATCH … WHERE EXISTS{ n<-CONTAINS*1..2-p }        (réparée par #345)",
        "MATCH (f:Function {name: 'fn0_0'})<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p:Project {id: 'p1'}) \
         MATCH (caller:Function)-[:CALLS]->(f) \
         WHERE EXISTS { MATCH (caller)<-[:CONTAINS*1..2]-(p) } RETURN count(caller) AS c",
    ),
    (
        "B  MATCH … WHERE EXISTS{ n<-CONTAINS-(:File)<-CONTAINS-p }   (intermédiaire étiqueté)",
        "MATCH (f:Function {name: 'fn0_0'})<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p:Project {id: 'p1'}) \
         MATCH (caller:Function)-[:CALLS]->(f) \
         WHERE EXISTS { MATCH (caller)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) } RETURN count(caller) AS c",
    ),
    (
        "C  OPTIONAL MATCH … WHERE EXISTS{ n<-CONTAINS*1..2-p }       (EXISTS sous OPTIONAL)",
        "MATCH (f:Function {name: 'fn0_0'})<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p:Project {id: 'p1'}) \
         OPTIONAL MATCH (caller:Function)-[:CALLS]->(f) \
         WHERE caller IS NOT NULL AND EXISTS { MATCH (caller)<-[:CONTAINS*1..2]-(p) } RETURN count(caller) AS c",
    ),
    (
        "D  OPTIONAL MATCH … WHERE EXISTS{ chaîne étiquetée }         (= code_33 de PO)",
        "MATCH (f:Function {name: 'fn0_0'})<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p:Project {id: 'p1'}) \
         OPTIONAL MATCH (caller:Function)-[:CALLS]->(f) \
         WHERE caller IS NOT NULL AND EXISTS { MATCH (caller)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) } RETURN count(caller) AS c",
    ),
    (
        "E  témoin : la même requête D sans EXISTS",
        "MATCH (f:Function {name: 'fn0_0'})<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p:Project {id: 'p1'}) \
         OPTIONAL MATCH (caller:Function)-[:CALLS]->(f) WHERE caller IS NOT NULL RETURN count(caller) AS c",
    ),
];

#[cfg(feature = "cypher")]
fn construire(fichiers: u64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let projet = s.create_node_with_props(&["Project"], [("id", Value::String("p1".into()))]);
    let mut fonctions = Vec::new();
    for i in 0..fichiers {
        let f =
            s.create_node_with_props(&["File"], [("path", Value::String(format!("f{i}").into()))]);
        s.create_edge(projet, f, "CONTAINS");
        for j in 0..3 {
            let g = s.create_node_with_props(
                &["Function"],
                [("name", Value::String(format!("fn{i}_{j}").into()))],
            );
            s.create_edge(f, g, "CONTAINS");
            fonctions.push(g);
        }
    }
    let n = fonctions.len();
    // `fn0_0` reçoit 5 appelants fixes (voisinage constant) ; le reste est un bruit de fond.
    for k in 1..=5 {
        s.create_edge(fonctions[(k * 7) % n], fonctions[0], "CALLS");
    }
    for i in 1..n {
        for cible in [(i * 5 + 1) % n, (i * 11 + 3) % n] {
            if cible != i && cible != 0 {
                s.create_edge(fonctions[i], fonctions[cible], "CALLS");
            }
        }
    }
    db
}

#[cfg(feature = "cypher")]
fn main() {
    let tailles: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let tailles = if tailles.is_empty() {
        vec![2_000, 8_000, 32_000]
    } else {
        tailles
    };
    let mut entete = String::new();
    for t in &tailles {
        let _ = write!(entete, "{t:>10}");
    }
    println!("{:<86} {entete}", "forme");
    let bases: Vec<ObrainDB> = tailles.iter().map(|&t| construire(t)).collect();
    for (nom, q) in FORMES {
        let mut cases = String::new();
        let mut valeurs = Vec::new();
        for db in &bases {
            let s = db.session();
            let r0 = s.execute_cypher(q);
            let mut ms = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                let _ = s.execute_cypher(q);
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            valeurs.push(match r0 {
                Ok(r) => r
                    .rows
                    .first()
                    .and_then(|l| l.first())
                    .map_or("?".into(), |v| format!("{v:?}")),
                Err(e) => format!("ERR {}", e.to_string().chars().take(20).collect::<String>()),
            });
            let _ = write!(cases, "{:>7.2} ms", ms[1]);
        }
        println!("{nom:<86} {cases}   résultats : {}", valeurs.join(" / "));
    }
}

#[cfg(not(feature = "cypher"))]
fn main() {
    eprintln!("mesure_exists_formes : requiert --features cypher");
}
