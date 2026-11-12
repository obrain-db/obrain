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

// ── (2026-11-01) Les promesses du README rendues VRAIES — alias, procédures
// exposées, algorithmes enregistrés. Rouges avant V1-FEATURES-2, verts après.
readme_procedure!(gds_k_core_alias, "CALL obrain.k_core()");
readme_procedure!(gds_jaccard_procedure, "CALL obrain.jaccard(0, 1)");
readme_procedure!(
    gds_cosine_similarity_procedure,
    "CALL obrain.cosine_similarity(0, 1)"
);
readme_procedure!(gds_adamic_adar_procedure, "CALL obrain.adamic_adar(0, 1)");
readme_procedure!(gds_triangle_count_procedure, "CALL obrain.triangle_count()");
readme_procedure!(gds_stable_communities, "CALL obrain.stable_communities()");
readme_procedure!(gds_irreducible_kernel, "CALL obrain.irreducible_kernel()");
readme_procedure!(gds_spectral_embedding, "CALL obrain.spectral_embedding()");
readme_procedure!(gds_hilbert_features, "CALL obrain.hilbert_features()");

#[test]
fn gds_astar_entre_deux_noeuds() {
    let (db, alix, harm) = graphe_avec_ids();
    let r = db
        .session()
        .execute(&format!("CALL obrain.astar({alix}, {harm})"))
        .unwrap_or_else(|e| panic!("README : astar(source, target) : {e}"));
    assert_eq!(r.row_count(), 1, "une ligne source/target/distance/path");
}

#[test]
fn gds_personalized_pagerank_et_heat_kernel_avec_graines() {
    let (db, alix, _) = graphe_avec_ids();
    for proc_ in ["personalized_pagerank", "heat_kernel_diffusion"] {
        appel(
            &db,
            &format!("CALL obrain.{proc_}({{seed_nodes: [{alix}]}})"),
        )
        .unwrap_or_else(|e| panic!("README : {proc_}({{seed_nodes}}) : {e}"));
    }
}

#[test]
fn gds_subgraph_contraction_avec_noeuds() {
    let (db, alix, harm) = graphe_avec_ids();
    appel(
        &db,
        &format!("CALL obrain.subgraph_contraction({{node_ids: [{alix}, {harm}]}})"),
    )
    .unwrap_or_else(|e| panic!("README : subgraph_contraction({{node_ids}}) : {e}"));
}

// ── Cognitif DEPUIS LA FAÇADE, avec `--features cognitive` ─────────────────
#[cfg(feature = "cognitive")]
mod cognitif {
    use super::*;

    readme_procedure!(
        cognitif_spread,
        "CALL obrain.cognitive.spread([0], {hops: 2})"
    );
    readme_procedure!(
        cognitif_distill,
        "CALL obrain.cognitive.distill({min_weight: 0.1})"
    );

    #[test]
    fn udf_cognitives_energy_risk_synapses() {
        let db = graphe_du_readme();
        for q in [
            "MATCH (n:Person) RETURN n.name, obrain.energy(id(n)) AS energy",
            "MATCH (n) RETURN obrain.risk(id(n)) AS risk ORDER BY risk DESC LIMIT 10",
            "MATCH (n:Person {name: 'Alix'}) RETURN obrain.synapses(id(n)) AS connections",
        ] {
            if let Err(e) = appel(&db, q) {
                panic!("README « Cognitive UDFs » : `{q}` échoue : {e}");
            }
        }
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

// ── Recherche hybride (README « Recherche hybride »), mot pour mot ───────────

#[cfg(feature = "hybrid-search")]
mod recherche_hybride {
    use obrain::{ObrainDB, Value};

    /// Trois `:Doc` portant `title` et `emb` (3 dimensions) ; les index sont
    /// créés par `indexes()`.
    fn docs() -> ObrainDB {
        let db = ObrainDB::new_in_memory();
        for (title, emb) in [
            ("Rust graph database", [1.0_f32, 0.0, 0.0]),
            ("Python machine learning", [0.0, 1.0, 0.0]),
            ("Rust systems programming", [0.9, 0.1, 0.0]),
        ] {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(n, "title", Value::String(title.into()));
            db.set_node_property(n, "emb", Value::Vector(emb.to_vec().into()));
        }
        db
    }

    fn indexes() -> ObrainDB {
        let db = docs();
        db.create_text_index("Doc", "title").unwrap();
        db.create_vector_index("Doc", "emb", Some(3), Some("cosine"), None, None)
            .unwrap();
        db
    }

    #[test]
    fn hybrid_search_texte_et_vecteur() {
        let r = indexes()
            .session()
            .execute(
                "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3}) YIELD node_id, score",
            )
            .unwrap();
        assert_eq!(r.columns, vec!["node_id".to_string(), "score".to_string()]);
        assert_eq!(r.row_count(), 3, "k = 3 sur trois documents");
    }

    #[test]
    fn hybrid_search_texte_seul() {
        let r = indexes()
            .session()
            .execute("CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph') YIELD node_id, score")
            .unwrap();
        assert_eq!(r.row_count(), 2, "BM25 seul : les deux documents qui parlent de Rust");
    }

    #[test]
    fn hybrid_search_enchaine_avec_match() {
        let r = indexes()
            .session()
            .execute(
                "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 2}) YIELD node_id MATCH (n) WHERE id(n) = node_id RETURN n.title",
            )
            .unwrap();
        let titres: Vec<&Value> = r.rows.iter().map(|row| &row[0]).collect();
        assert_eq!(
            titres,
            vec![
                &Value::String("Rust graph database".into()),
                &Value::String("Rust systems programming".into())
            ]
        );
    }

    #[test]
    fn hybrid_search_fusion_ponderee() {
        let r = indexes()
            .session()
            .execute(
                "CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3, fusion: 'weighted', weights: [0.5, 0.5]}) YIELD node_id, score",
            )
            .unwrap();
        assert_eq!(r.rows[0][1], Value::Float64(1.0), "premier des deux sources : 0.5 + 0.5");
    }

    #[test]
    fn hybrid_search_sans_index_nomme_les_index_attendus() {
        let e = docs()
            .session()
            .execute("CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph') YIELD node_id, score")
            .unwrap_err()
            .to_string();
        assert!(e.contains(":Doc(title)") && e.contains(":Doc(emb)"), "{e}");
    }
}
