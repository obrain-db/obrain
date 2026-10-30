//! V4 — USING BASE (sucre d'alias), health inter-bases, statistiques
//! d'optimiseur, federation de recherche.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,wal,text-index" --test v4_inter_bases
//! ```

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Deux bases en memoire attachees : alpha (primaire) -> beta.
fn deux_bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', titre: 'Guide'})")
        .expect("insert beta");
    beta.graph_store().create_property_index("id");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'note agent'})")
        .expect("insert alpha");
    sa.execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
        .expect("create arete externe");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

// ─── USING BASE : alias de bases (sucre demande par l'entite) ────────────

#[test]
fn using_base_alias_dans_ref_et_in() {
    let (alpha, _beta) = deux_bases();
    let s = alpha.session();
    // ref(alias, clef) en ecriture.
    s.execute(
        "USING BASE b AS \"beta\" MATCH (a:Note) CREATE (a)-[:CITE]->(ref(b, \"doc_1\"))",
    )
    .expect("using base + ref(alias)");
    let store = alpha.graph_store();
    {
        use obrain_core::graph::GraphStore;
        let n = store.nodes_by_label("Note")[0];
        assert!(
            store
                .external_edges_from(n)
                .iter()
                .any(|e| e.edge_type == "CITE" && e.to_store == "beta"),
            "l'alias doit se resoudre vers le slug"
        );
    }
    // IN alias en lecture.
    let r = s
        .execute("USING BASE b AS \"beta\" MATCH (a:Note)-[:DOCUMENTS]->(x IN b) RETURN x.titre")
        .expect("using base + IN alias");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("Guide".into()));
}

#[test]
fn using_base_alias_inconnu_erreur_enseignante() {
    let (alpha, _beta) = deux_bases();
    let err = alpha
        .session()
        .execute("MATCH (a:Note) CREATE (a)-[:X]->(ref(inconnu, \"k\"))")
        .expect_err("alias non declare");
    let msg = format!("{err}");
    assert!(
        msg.contains("USING BASE"),
        "l'erreur doit enseigner le preambule, recu : {msg}"
    );
}

#[test]
fn sans_preambule_rien_ne_change() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b IN \"beta\") RETURN b.titre")
        .expect("forme litterale intacte");
    assert_eq!(r.rows.len(), 1);
}

// ─── Health : compteurs inter-bases, instantane sans remise a zero ───────

#[test]
fn le_health_expose_les_compteurs_inter_bases() {
    let (alpha, beta) = deux_bases();
    // Une pendante : cible supprimee puis traversee.
    beta.session()
        .execute("MATCH (d:Doc) DETACH DELETE d")
        .expect("suppression");
    let s = alpha.session();
    s.execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b")
        .expect("traversee pendante");
    // Un refus de droits.
    {
        use obrain_core::execution::AccessContext;
        use std::sync::Arc;
        let s2 = alpha.session();
        s2.set_access_context(Arc::new(AccessContext::new(
            "public",
            Box::new(|_, slug| slug == "alpha"),
        )));
        s2.execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b")
            .expect("traversee refusee");
    }
    let sante = alpha.inter_base_health();
    assert_eq!(sante.external_edge_count, 1);
    assert!(sante.dangling_refs >= 1, "la pendante doit se voir au health");
    assert!(sante.access_denied_refs >= 1, "le refus doit se voir au health");
    // Instantane SANS remise a zero : une seconde lecture rend au moins autant.
    let sante2 = alpha.inter_base_health();
    assert!(sante2.dangling_refs >= sante.dangling_refs);
}

// ─── Statistiques d'optimiseur : cardinalite des aretes externes ─────────

#[test]
fn la_cardinalite_externe_par_type_est_connue() {
    let (alpha, _beta) = deux_bases();
    let s = alpha.session();
    s.execute("MATCH (a:Note) CREATE (a)-[:CITE]->(ref(\"beta\", \"doc_1\"))")
        .expect("deuxieme type");
    let store = alpha.graph_store();
    assert_eq!(store.external_edge_count_by_type(None), 2);
    assert_eq!(store.external_edge_count_by_type(Some("DOCUMENTS")), 1);
    assert_eq!(store.external_edge_count_by_type(Some("CITE")), 1);
    assert_eq!(store.external_edge_count_by_type(Some("ABSENT")), 0);
    // Le raccord planificateur (estimate_expand_degree) consulte cette
    // cardinalite quand un registre est attache — verifie par lecture du
    // code (planner/lpg/mod.rs) ; l'estimation n'est pas observable par
    // l'API publique, le comptage l'est.
}

// ─── Federation de recherche : fan-out + fusion + droits ─────────────────

#[cfg(feature = "text-index")]
#[test]
fn la_recherche_federee_fusionne_et_etiquette() {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    alpha
        .session()
        .execute("INSERT (:Doc {texte: 'le graphe des connaissances partagees'})")
        .expect("insert alpha");
    beta.session()
        .execute("INSERT (:Doc {texte: 'les connaissances du graphe prive'})")
        .expect("insert beta");
    alpha.create_text_index("Doc", "texte").expect("index alpha");
    beta.create_text_index("Doc", "texte").expect("index beta");

    let resultats = ObrainDB::text_search_federated(
        &[("alpha", &alpha), ("beta", &beta)],
        None,
        "Doc",
        "texte",
        "connaissances",
        10,
    )
    .expect("federation");
    assert_eq!(resultats.len(), 2, "les deux bases repondent");
    let slugs: Vec<&str> = resultats.iter().map(|(s, _, _)| s.as_str()).collect();
    assert!(slugs.contains(&"alpha") && slugs.contains(&"beta"));
    // Scores tries decroissant.
    assert!(resultats[0].2 >= resultats[1].2);
}

#[cfg(feature = "text-index")]
#[test]
fn la_recherche_federee_respecte_les_droits_et_les_bases_mal_equipees() {
    use obrain_core::execution::AccessContext;
    use std::sync::Arc;
    let alpha = ObrainDB::new_in_memory();
    let privee = ObrainDB::new_in_memory();
    let sans_index = ObrainDB::new_in_memory();
    alpha
        .session()
        .execute("INSERT (:Doc {texte: 'connaissances publiques'})")
        .expect("insert");
    privee
        .session()
        .execute("INSERT (:Doc {texte: 'connaissances privees'})")
        .expect("insert");
    alpha.create_text_index("Doc", "texte").expect("index");
    privee.create_text_index("Doc", "texte").expect("index");

    let ctx = Arc::new(AccessContext::new(
        "public",
        Box::new(|_, slug: &str| slug != "privee"),
    ));
    let resultats = ObrainDB::text_search_federated(
        &[("alpha", &alpha), ("privee", &privee), ("vide", &sans_index)],
        Some(&ctx),
        "Doc",
        "texte",
        "connaissances",
        10,
    )
    .expect("la base refusee et la base sans index ne tuent pas la federation");
    assert_eq!(resultats.len(), 1, "seule la base publique repond");
    assert_eq!(resultats[0].0, "alpha");
}
