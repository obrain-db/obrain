//! LE README, EXÉCUTÉ. (v0.0.1, 2026-11-01)
//!
//! Chaque test de ce fichier correspond à une affirmation du `README.md` de la
//! façade `obrain`, telle qu'un utilisateur la lirait, avec les FEATURES PAR
//! DÉFAUT du crate publié (`embedded` : gql, ai, algos, parallel, regex).
//! Règle de la v0.0.1 : une affirmation sans test vert ici sort du README.
//!
//! Un test rouge se lit ainsi :
//!   - « Unknown procedure » → la procédure documentée n'existe pas sous ce
//!     nom (à implémenter, à renommer dans le README, ou à retirer) ;
//!   - autre erreur → elle existe mais l'appel documenté ne marche pas tel
//!     quel (paramètres, colonnes) : le README ment sur la forme.
//!
//! Les exemples Python / Node / Go / … du README ne sont PAS testés ici : il
//! n'y a aucun binding dans ce dépôt.

use obrain::{ObrainDB, Value};

/// Le graphe du Quick Start : Alix → Gus, plus Harm pour donner de la
/// matière aux algorithmes (un triangle et une feuille).
fn graphe_du_readme() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:Person {name: 'Alix', age: 30})")
        .unwrap();
    s.execute("INSERT (:Person {name: 'Gus', age: 25})")
        .unwrap();
    s.execute("INSERT (:Person {name: 'Harm', age: 41})")
        .unwrap();
    s.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'}) \
         INSERT (a)-[:KNOWS {since: 2020, w: 1.0}]->(b)",
    )
    .unwrap();
    s.execute(
        "MATCH (a:Person {name: 'Gus'}), (b:Person {name: 'Harm'}) \
         INSERT (a)-[:KNOWS {since: 2021, w: 2.0}]->(b)",
    )
    .unwrap();
    s.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Harm'}) \
         INSERT (a)-[:KNOWS {since: 2022, w: 3.0}]->(b)",
    )
    .unwrap();
    db
}

fn appel(db: &ObrainDB, call: &str) -> Result<usize, String> {
    db.session()
        .execute(call)
        .map(|r| r.row_count())
        .map_err(|e| e.to_string())
}

/// Le contrat minimal d'une procédure documentée : l'appel tel qu'écrit dans
/// le README rend `Ok`. Le nombre de lignes n'est pas imposé (certaines
/// procédures rendent légitimement 0 ligne sur 3 nœuds).
fn procedure_documentee(call: &str) {
    let db = graphe_du_readme();
    match appel(&db, call) {
        Ok(_) => {}
        Err(e) if e.contains("Unknown procedure") => {
            panic!("README : `{call}` — la procédure N'EXISTE PAS ({e})")
        }
        Err(e) => panic!("README : `{call}` — existe mais l'appel documenté échoue : {e}"),
    }
}

// ── Quick Start (README « Rust »), mot pour mot ─────────────────────────────

#[test]
fn quick_start_rust_mot_pour_mot() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute("INSERT (:Person {name: 'Alix', age: 30})")
        .unwrap();
    session
        .execute("INSERT (:Person {name: 'Gus', age: 25})")
        .unwrap();
    session
        .execute(
            "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'})
         INSERT (a)-[:KNOWS {since: 2020}]->(b)",
        )
        .unwrap();
    let result = session
        .execute(
            "MATCH (p:Person)-[:KNOWS]->(friend)
         RETURN p.name, friend.name",
        )
        .unwrap();
    let lignes: Vec<String> = result
        .iter()
        .map(|row| format!("{} knows {}", row[0], row[1]))
        .collect();
    assert_eq!(lignes, vec!["Alix knows Gus".to_string()]);
    let pr = session.execute("CALL obrain.pagerank()").unwrap();
    assert_eq!(pr.row_count(), 2, "un score PageRank par nœud");
    for row in pr.iter() {
        assert!(
            matches!(row[1], Value::Float64(_)),
            "colonne score : {:?}",
            row[1]
        );
    }
}

// ── GDS : chaque ligne des tableaux « GDS Algorithms » ───────────────────────

macro_rules! readme_procedure {
    ($nom:ident, $call:expr) => {
        #[test]
        fn $nom() {
            procedure_documentee($call);
        }
    };
}

// Centrality
readme_procedure!(gds_pagerank, "CALL obrain.pagerank()");
readme_procedure!(
    gds_betweenness_centrality,
    "CALL obrain.betweenness_centrality()"
);
readme_procedure!(
    gds_closeness_centrality,
    "CALL obrain.closeness_centrality()"
);
readme_procedure!(gds_degree_centrality, "CALL obrain.degree_centrality()");
readme_procedure!(gds_hits, "CALL obrain.hits()");
// Community Detection
readme_procedure!(gds_louvain, "CALL obrain.louvain()");
readme_procedure!(gds_leiden, "CALL obrain.leiden()");
readme_procedure!(gds_label_propagation, "CALL obrain.label_propagation()");
// Shortest Paths
readme_procedure!(gds_dijkstra, "CALL obrain.dijkstra(0)");
readme_procedure!(gds_bellman_ford, "CALL obrain.bellman_ford(0)");
readme_procedure!(gds_floyd_warshall, "CALL obrain.floyd_warshall()");
// Traversal & Structure
readme_procedure!(gds_bfs, "CALL obrain.bfs(0)");
readme_procedure!(gds_dfs, "CALL obrain.dfs(0)");
readme_procedure!(
    gds_connected_components,
    "CALL obrain.connected_components()"
);
readme_procedure!(
    gds_strongly_connected_components,
    "CALL obrain.strongly_connected_components()"
);
readme_procedure!(gds_topological_sort, "CALL obrain.topological_sort()");
readme_procedure!(gds_bridges, "CALL obrain.bridges()");
readme_procedure!(gds_articulation_points, "CALL obrain.articulation_points()");
// Clustering & Similarity
readme_procedure!(
    gds_clustering_coefficient,
    "CALL obrain.clustering_coefficient()"
);
// Network Flow & MST
readme_procedure!(gds_kruskal, "CALL obrain.kruskal()");
readme_procedure!(gds_prim, "CALL obrain.prim()");

// ── Procédures à paramètres nommés / identifiants : ids pris sur le graphe ─
readme_procedure!(gds_kcore, "CALL obrain.kcore()");
readme_procedure!(gds_subgraph_khop, "CALL obrain.subgraph.khop(0, 2)");
readme_procedure!(gds_similarity, "CALL obrain.similarity(0, 1)");
readme_procedure!(
    gds_similarity_jaccard,
    "CALL obrain.similarity(0, 1, 'jaccard')"
);
readme_procedure!(
    gds_similarity_cosine,
    "CALL obrain.similarity(0, 1, 'cosine')"
);
readme_procedure!(
    gds_similarity_adamic_adar,
    "CALL obrain.similarity(0, 1, 'adamic_adar')"
);
readme_procedure!(
    gds_similarity_overlap,
    "CALL obrain.similarity(0, 1, 'overlap')"
);
readme_procedure!(gds_similarity_topk, "CALL obrain.similarity.topk(0, 2)");

/// Même graphe que `graphe_du_readme`, bâti par l'API directe pour connaître
/// les identifiants (les procédures à source/puits en ont besoin).
fn graphe_avec_ids() -> (ObrainDB, u64, u64) {
    let db = ObrainDB::new_in_memory();
    let alix = db.create_node(&["Person"]);
    let gus = db.create_node(&["Person"]);
    let harm = db.create_node(&["Person"]);
    db.create_edge(alix, gus, "KNOWS");
    db.create_edge(gus, harm, "KNOWS");
    db.create_edge(alix, harm, "KNOWS");
    (db, alix.0, harm.0)
}

/// `sssp` désigne sa source par la propriété `name` (chaîne), pas par un id.
#[test]
fn gds_sssp_avec_source_par_nom() {
    let db = graphe_du_readme();
    appel(&db, "CALL obrain.sssp({source: 'Alix'})")
        .unwrap_or_else(|e| panic!("README : sssp({{source: 'Alix'}}) : {e}"));
}

#[test]
fn gds_max_flow_et_min_cost_max_flow_avec_source_et_puits() {
    let (db, alix, harm) = graphe_avec_ids();
    for proc_ in ["max_flow", "min_cost_max_flow"] {
        appel(
            &db,
            &format!("CALL obrain.{proc_}({{source: {alix}, sink: {harm}}})"),
        )
        .unwrap_or_else(|e| panic!("README : {proc_}({{source, sink}}) : {e}"));
    }
}

// ── « CALL Procedures Reference » : la syntaxe documentée ───────────────────

readme_procedure!(
    syntaxe_call_avec_parametres,
    "CALL obrain.pagerank({damping: 0.85, max_iterations: 20})"
);
readme_procedure!(
    syntaxe_call_yield,
    "CALL obrain.louvain() YIELD node_id, community_id"
);
readme_procedure!(
    syntaxe_call_yield_alias,
    "CALL obrain.pagerank() YIELD node_id AS id, score AS rank"
);

// ── Cognitif, DEPUIS LA FAÇADE PUBLIÉE (features par défaut) ────────────────
// La façade `obrain` n'expose aucune feature `cognitive` : si ces tests sont
// rouges, le README promet depuis le crate publié ce que seul `obrain-engine`
// avec `--features cognitive` peut tenir.

// ── « RAG with Cognitive Scoring » ───────────────────────────────────────────

// ── « Features › Core Capabilities » : MVCC, snapshot isolation ─────────────

#[test]
fn mvcc_snapshot_isolation_une_lecture_ne_voit_pas_le_commit_posterieur() {
    let db = graphe_du_readme();
    let mut lecteur = db.session();
    lecteur.begin_transaction().unwrap();
    let avant = lecteur
        .execute("MATCH (n:Person) RETURN n")
        .unwrap()
        .row_count();
    assert_eq!(avant, 3);

    let mut ecrivain = db.session();
    ecrivain.begin_transaction().unwrap();
    ecrivain
        .execute("INSERT (:Person {name: 'Tard', age: 1})")
        .unwrap();
    ecrivain.commit().unwrap();

    let pendant = lecteur
        .execute("MATCH (n:Person) RETURN n")
        .unwrap()
        .row_count();
    lecteur.commit().unwrap();
    assert_eq!(
        pendant, 3,
        "snapshot isolation : la transaction ouverte avant le commit voisin ne doit pas le voir"
    );
    assert_eq!(
        db.session()
            .execute("MATCH (n:Person) RETURN n")
            .unwrap()
            .row_count(),
        4
    );
}

// ── « In-memory and persistent storage modes » ──────────────────────────────
// `ObrainDB::open` exige la feature `wal` (groupe `storage`), ABSENTE du profil
// `embedded` par défaut : le README doit le dire (mesuré : E0599 sans la feature).

#[cfg(feature = "wal")]
#[test]
fn stockage_persistant_rouvrir_et_relire() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("readme.obrain");
    {
        let db = ObrainDB::open(&chemin).unwrap();
        db.session()
            .execute("INSERT (:Person {name: 'Alix', age: 30})")
            .unwrap();
        db.close().unwrap();
    }
    let db = ObrainDB::open(&chemin).unwrap();
    assert_eq!(
        db.session()
            .execute("MATCH (n:Person {name: 'Alix'}) RETURN n.age")
            .unwrap()
            .rows[0][0],
        Value::Int64(30)
    );
}
