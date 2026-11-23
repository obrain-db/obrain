//! Mesure `EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) }` quand `n` ET `p` sont déjà
//! liés par le motif externe (obrain#189 : `code_45`, `code_40`, `code_24` de PO).
//!
//! Hypothèse mesurée : le chemin « semi-jointure » du planificateur plan le
//! sous-motif comme NON corrélé — il énumère tous les couples du graphe avant de
//! joindre sur (n, p). Le coût suivrait alors la taille du graphe, pas celle du
//! voisinage de `s` (ici : quelques voisins par `IMPORTS`).
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example mesure_exists_lie -- 2000 8000 32000
//! ```
//!
//! Elle n'affirme rien : elle imprime. Le même graphe, la même requête, trois
//! tailles : si la durée suit le nombre de fichiers, c'est le coût du graphe.

#[cfg(feature = "cypher")]
use obrain_common::types::Value;
#[cfg(feature = "cypher")]
use obrain_engine::ObrainDB;
#[cfg(feature = "cypher")]
use std::time::Instant;

/// Requête de la forme de `code_45` : voisins `IMPORTS` de `s`, gardés s'ils sont
/// contenus (profondeur 1..2) dans le projet `p`, déjà lié.
#[cfg(feature = "cypher")]
const REQUETE: &str = "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) \
    CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) \
           WHERE (n:File OR n:Function) AND EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } \
           RETURN collect(DISTINCT n) AS voisins } \
    RETURN size(voisins) AS c";

/// Même requête SANS l'EXISTS : isole le coût de la traversée `IMPORTS` seule.
#[cfg(feature = "cypher")]
const REQUETE_SANS_EXISTS: &str = "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) \
    CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) \
           WHERE (n:File OR n:Function) \
           RETURN collect(DISTINCT n) AS voisins } \
    RETURN size(voisins) AS c";

#[cfg(feature = "cypher")]
fn construire(fichiers: u64) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let projet = session.create_node_with_props(&["Project"], [("id", Value::String("p1".into()))]);
    let ids: Vec<_> = (0..fichiers)
        .map(|i| {
            let f = session.create_node_with_props(
                &["File"],
                [("path", Value::String(format!("f{i}").into()))],
            );
            session.create_edge(projet, f, "CONTAINS");
            // Trois fonctions par fichier : profondeur 2 depuis le projet.
            for j in 0..3 {
                let fonction = session.create_node_with_props(
                    &["Function"],
                    [("name", Value::String(format!("fn{i}_{j}").into()))],
                );
                session.create_edge(f, fonction, "CONTAINS");
            }
            f
        })
        .collect();
    let n = ids.len();
    for i in 0..n {
        for cible in [(i + 1) % n, (i * 7 + 1) % n, (i * 13 + 5) % n] {
            if cible != i {
                session.create_edge(ids[i], ids[cible], "IMPORTS");
            }
        }
    }
    db
}

#[cfg(feature = "cypher")]
fn main() {
    let tailles: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let tailles = if tailles.is_empty() {
        vec![2_000, 8_000, 32_000]
    } else {
        tailles
    };
    println!(
        "{:>10} {:>12} {:>14} {:>10}",
        "fichiers", "nœuds", "durée (ms)", "résultat"
    );
    for fichiers in tailles {
        let db = construire(fichiers);
        let session = db.session();
        // Cinq tours consécutifs : si le premier seul est cher, c'est un coût
        // de première utilisation (structure paresseuse), pas celui de la requête.
        let tours: Vec<String> = (0..5)
            .map(|_| {
                let t = Instant::now();
                let _ = session.execute_cypher(REQUETE);
                format!("{:.2}", t.elapsed().as_secs_f64() * 1000.0)
            })
            .collect();
        println!("  tours consécutifs (ms) : {}", tours.join("  "));
        let t = Instant::now();
        let r = session.execute_cypher(REQUETE);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let valeur = match &r {
            Ok(res) => res
                .rows
                .first()
                .and_then(|l| l.first())
                .map_or("(vide)".to_string(), |v| format!("{v:?}")),
            Err(e) => format!("ERREUR {e}").chars().take(40).collect(),
        };
        let _ = session.execute_cypher(REQUETE_SANS_EXISTS);
        let t2 = Instant::now();
        let _ = session.execute_cypher(REQUETE_SANS_EXISTS);
        let ms_sans = t2.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{:>10} {:>12} {:>14.2} {:>10}   (sans EXISTS : {:.2} ms)",
            fichiers,
            fichiers * 4 + 1,
            ms,
            valeur,
            ms_sans
        );
        if fichiers >= 32_000 {
            let variantes: &[(&str, &str)] = &[
                (
                    "label seul AND exists",
                    "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) WHERE n:File AND EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } RETURN collect(DISTINCT n) AS v } RETURN size(v) AS c",
                ),
                (
                    "(File OR Function) AND ex",
                    "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) WHERE (n:File OR n:Function) AND EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } RETURN collect(DISTINCT n) AS v } RETURN size(v) AS c",
                ),
                (
                    "EXISTS AND (File OR Fn)",
                    "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) WHERE EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } AND (n:File OR n:Function) RETURN collect(DISTINCT n) AS v } RETURN size(v) AS c",
                ),
                (
                    "File OR Function seul",
                    "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) WHERE (n:File OR n:Function) RETURN collect(DISTINCT n) AS v } RETURN size(v) AS c",
                ),
                (
                    "exists saut simple",
                    "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) WHERE EXISTS { MATCH (n)<-[:CONTAINS]-(p) } RETURN collect(DISTINCT n) AS v } RETURN size(v) AS c",
                ),
                (
                    "exists *1..2 sans OR",
                    "MATCH (s:File {path: 'f0'})<-[:CONTAINS]-(p:Project {id: 'p1'}) CALL { WITH s, p MATCH (s)-[:IMPORTS*1..1]-(n) WHERE EXISTS { MATCH (n)<-[:CONTAINS*1..2]-(p) } RETURN collect(DISTINCT n) AS v } RETURN size(v) AS c",
                ),
                (
                    "*1..2 entrant sans EXISTS",
                    "MATCH (n:File {path: 'f1'}) MATCH (n)<-[:CONTAINS*1..2]-(x) RETURN count(x) AS c",
                ),
                (
                    "*1 entrant sans EXISTS",
                    "MATCH (n:File {path: 'f1'}) MATCH (n)<-[:CONTAINS]-(x) RETURN count(x) AS c",
                ),
            ];
            for (nom, q) in variantes {
                let _ = session.execute_cypher(q);
                let t = Instant::now();
                let r = session.execute_cypher(q);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let v = r
                    .as_ref()
                    .ok()
                    .and_then(|x| x.rows.first().and_then(|l| l.first()))
                    .map_or("?".into(), |v| format!("{v:?}"));
                println!("  variante {nom:<28} {ms:>9.2} ms  -> {v}");
            }
            if let Ok(plan) = session.execute_cypher(&format!("PROFILE {REQUETE}")) {
                println!("--- PROFILE (taille {fichiers}) ---");
                for ligne in plan.rows.iter().take(60) {
                    for v in ligne {
                        if let Value::String(t) = v {
                            println!("{t}");
                        }
                    }
                }
                println!("---");
            }
        }
        if fichiers == 2_000
            && let Ok(plan) = session.execute_cypher(&format!("EXPLAIN {REQUETE}"))
        {
            println!("--- EXPLAIN (taille {fichiers}) ---");
            for ligne in plan.rows.iter().take(40) {
                println!(
                    "{}",
                    ligne
                        .iter()
                        .map(|v| format!("{v:?}"))
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            }
            println!("---");
        }
    }
}

/// Sans la feature `cypher`, l'exemple n'a rien à mesurer : il compile quand même
/// (la CI construit tous les exemples avec les features par défaut).
#[cfg(not(feature = "cypher"))]
fn main() {
    eprintln!("mesure_exists_lie : requiert --features cypher");
}
