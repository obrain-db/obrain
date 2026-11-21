//! Poussée de prédicat à travers des filtres empilés (obrain-db/obrain#189, F4).
//!
//! `MATCH path = (f:File {project_id: $p})-[:IMPORTS*2..6]->(f) WHERE f.path =~ $r`
//! (topo_03 de PO) : la regex ne cite que `f`, mais elle restait AU-DESSUS du
//! filtre `_anon = f` posé par le chemin cyclique, donc au-dessus du VLE : les
//! chemins étaient énumérés depuis les 34 516 fichiers, puis la regex en
//! gardait ~50. Elle descend désormais sous l'expansion.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test poussee_filtres_empiles_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_core::execution::operators::chemins_enumeres;
use obrain_engine::ObrainDB;

const FICHIERS: usize = 60;

/// `FICHIERS` fichiers `src/x/fI.rs` en cycles d'imports de 3 (f0->f1->f2->f0,
/// …) ; seuls les 3 premiers sont sous `src/neo4j/`.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let fichiers: Vec<_> = (0..FICHIERS)
        .map(|i| {
            let dossier = if i < 3 { "neo4j" } else { "autre" };
            s.create_node_with_props(
                &["File"],
                [
                    (
                        "path",
                        Value::String(format!("src/{dossier}/f{i}.rs").into()),
                    ),
                    ("project_id", Value::String("p".into())),
                    (
                        "status",
                        Value::String(if i % 2 == 0 { "active" } else { "archived" }.into()),
                    ),
                ],
            )
        })
        .collect();
    for c in fichiers.chunks(3) {
        s.create_edge(c[0], c[1], "IMPORTS");
        s.create_edge(c[1], c[2], "IMPORTS");
        s.create_edge(c[2], c[0], "IMPORTS");
    }
    db
}

fn executer(db: &ObrainDB, q: &str) -> (Vec<String>, u64) {
    let avant = chemins_enumeres();
    let r = db
        .session()
        .execute_cypher(q)
        .unwrap_or_else(|e| panic!("{q}\n{e}"));
    let enumeres = chemins_enumeres() - avant;
    let mut lignes: Vec<String> = r.rows.iter().map(|l| format!("{l:?}")).collect();
    lignes.sort();
    (lignes, enumeres)
}

#[test]
fn la_regex_filtre_les_sources_avant_le_vle() {
    let db = base();
    let (lignes, enumeres) = executer(
        &db,
        "MATCH path = (f:File {project_id: 'p'})-[:IMPORTS*2..6]->(f) \
         WHERE f.path =~ '^.*src/neo4j/[^/]*$' \
         WITH f, length(path) AS cycle_len \
         RETURN DISTINCT f.path AS violator, min(cycle_len) AS shortest_cycle \
         ORDER BY shortest_cycle ASC LIMIT 10",
    );
    assert_eq!(lignes.len(), 3, "{lignes:?}");
    assert!(lignes.iter().all(|l| l.contains("Int64(3)")), "{lignes:?}");
    // Le VLE (guidé par la cible `f`) énumère ce qui part de chaque source :
    // mesuré 60 sans poussée (les 60 fichiers), à comparer aux 3 sources
    // que la regex retient. Borne : 2 chemins par source retenue (cycles de
    // 3 et de 6 sauts).
    assert!(
        enumeres <= 6,
        "{enumeres} chemins énumérés : la regex n'a pas été poussée sous le VLE"
    );
}

/// Les interdits, de bout en bout : mêmes lignes qu'une évaluation à la main.
#[test]
fn semantique_des_interdits() {
    let db = base();
    // WITH ... WHERE après une agrégation : le filtre porte sur le compte.
    let (l, _) = executer(
        &db,
        "MATCH (f:File)-[:IMPORTS]->(g:File) WITH f.status AS st, count(g) AS c \
         WHERE st = 'active' AND c > 0 RETURN st, c",
    );
    assert_eq!(l, vec![r#"[String("active"), Int64(30)]"#]);
    // LIMIT puis WHERE : filtrer APRÈS les 5 premières lignes.
    let (l, _) = executer(
        &db,
        "MATCH (f:File) WITH f ORDER BY f.path LIMIT 5 \
         MATCH (f)-[:IMPORTS]->(g) WHERE f.status = 'active' RETURN count(*) AS c",
    );
    let (attendu, _) = executer(
        &db,
        "MATCH (f:File) WITH f ORDER BY f.path LIMIT 5 \
         WITH collect(f) AS fs UNWIND fs AS f \
         MATCH (f)-[:IMPORTS]->(g) WITH f, g WHERE f.status = 'active' RETURN count(*) AS c",
    );
    assert_eq!(l, attendu);
    // OPTIONAL MATCH : un WHERE sur le côté optionnel rend NULL, pas rien.
    let (l, _) = executer(
        &db,
        "MATCH (f:File) WHERE f.path = 'src/neo4j/f0.rs' \
         OPTIONAL MATCH (f)-[:IMPORTS]->(g:File) WHERE g.status = 'active' \
         RETURN f.path AS f, g.path AS g",
    );
    assert_eq!(l, vec![r#"[String("src/neo4j/f0.rs"), Null]"#]);
    // UNWIND : la variable déroulée n'existe qu'au-dessus.
    let (l, _) = executer(
        &db,
        "MATCH (f:File)-[:IMPORTS]->(g:File) WHERE f.path STARTS WITH 'src/neo4j' \
         UNWIND [1, 2] AS x WITH f, g, x WHERE x = 2 AND g.status = 'active' \
         RETURN f.path, x",
    );
    assert_eq!(l.len(), 2, "{l:?}");
    // Division gardée par un filtre précédent : ne doit pas lever `/ 0`.
    let (l, _) = executer(
        &db,
        "UNWIND [0, 1, 2] AS v MATCH (f:File)-[:IMPORTS]->(g) \
         WHERE f.path = 'src/neo4j/f0.rs' WITH f, g, v WHERE v <> 0 \
         WITH f, g, v WHERE 10 / v > 1 RETURN v",
    );
    assert_eq!(l, vec!["[Int64(1)]", "[Int64(2)]"]);
}
