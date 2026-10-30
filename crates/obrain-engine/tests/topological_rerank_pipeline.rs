#![cfg(all(feature = "vector-index", feature = "text-index", feature = "hybrid-search"))]
//! Pipeline « RAG topologique » industrialisé (RFC zorder-superposition C2).
//!
//! Même scénario que la v1 (`vector_latency_scale.rs::
//! topological_rerank_synthetic_c2` — hybrid seul precision@5 ~0,53 → hybride
//! + topologie ~0,94), mais consommé via l'API publique
//! [`ObrainDB::hybrid_search_topological`] avec un résolveur de cluster RÉEL
//! (`cluster_components_by_edge_type` sur les arêtes CALLS) — aucun oracle
//! `uid/per` : la structure du graphe départage.
//!
//! Le test v1 existant n'est pas modifié (C6) : il reste la preuve du
//! mécanisme avec oracle parfait ; celui-ci prouve le pipeline consommable.

use obrain_engine::database::ObrainDB;
use obrain_engine::database::topological::cluster_components_by_edge_type;
use obrain_common::types::NodeId;

#[test]
fn hybrid_search_topological_ameliore_sans_oracle() {
    let db = ObrainDB::new_in_memory();
    let (nc, per) = (20usize, 200usize);
    let mut docs: Vec<NodeId> = Vec::new();
    for c in 0..nc {
        for i in 0..per {
            let d = db.create_node(&["Doc"]);
            let mut v = vec![0.0f32; 64];
            v[(c / 2) % 64] = 1.0;
            v[(c + i) % 64] = 0.05 * (i as f32 + 1.0);
            db.set_node_property(d, "emb", obrain_common::types::Value::Vector(v.into()));
            db.set_node_property(
                d,
                "text",
                obrain_common::types::Value::String(
                    format!("k{} alpha beta gamma", c % 10).as_str().into(),
                ),
            );
            db.set_node_property(d, "uid", obrain_common::types::Value::Int64((c * per + i) as i64));
            docs.push(d);
        }
    }
    // Arêtes intra-cluster (anneau) — le voisinage « communauté » réel.
    for c in 0..nc {
        for i in 0..per {
            let a = docs[c * per + i];
            let b = docs[c * per + (i + 1) % per];
            db.create_edge(a, b, "CALLS");
        }
    }
    db.create_text_index("Doc", "text").unwrap();
    db.create_vector_index("Doc", "emb", Some(64), Some("cosine"), None, None)
        .expect("index");

    let uid_of = |nid: NodeId| -> i64 {
        let n = db.get_node(nid).unwrap();
        match n.properties.get(&obrain_common::types::PropertyKey::new("uid")) {
            Some(obrain_common::types::Value::Int64(x)) => *x,
            _ => -1,
        }
    };

    // Résolveur de cluster RÉEL : composantes connexes sur les arêtes CALLS,
    // restreintes aux candidats ∪ voisins 1-hop. Closure fermant sur le store
    // (pré-calcul par requête dans la méthode du pipeline).
    let cluster_real = |cands: &[NodeId]| -> std::collections::HashMap<NodeId, u64> {
        cluster_components_by_edge_type(db.store().as_ref(), cands, "CALLS")
    };

    let mut queries: Vec<(String, Vec<f32>)> = Vec::new();
    for q in 0..100 {
        let c = q % nc;
        let mut v = vec![0.0f32; 64];
        v[(c / 2) % 64] = 1.0;
        queries.push((format!("k{} alpha", c % 10), v));
    }

    let avg_p = |topo: bool| -> f64 {
        let mut acc = 0.0;
        for (qi, (qt, qv)) in queries.iter().enumerate() {
            let c = qi % nc;
            let cand = if topo {
                // Pipeline : hybrid sur-échantillonné + rerank par dominance.
                db.hybrid_search_topological(
                    "Doc", "text", "emb", qt, Some(qv), 5, 1.0,
                    // cluster_of injecté : composantes réelles du pré-calcul.
                    |nid| cluster_real(&[nid]).get(&nid).copied(),
                )
                .expect("pipeline topologique")
            } else {
                db.hybrid_search("Doc", "text", "emb", qt, Some(qv), 5, None)
                    .expect("hybrid seul")
            };
            let ok = cand
                .iter()
                .take(5)
                .filter(|(id, _)| (uid_of(*id) / per as i64) as usize == c)
                .count();
            acc += ok as f64 / 5.0;
        }
        acc / queries.len() as f64
    };

    let p_base = avg_p(false);
    let p_topo = avg_p(true);
    eprintln!("C2 pipeline : precision@5 hybride seul = {p_base:.3} | hybride+composantes réelles = {p_topo:.3}");
    assert!(
        p_topo > p_base + 0.05,
        "le pipeline topologique doit améliorer la précision (basé {p_base} → topo {p_topo})"
    );
    eprintln!("C2 pipeline OK — rerank par composantes réelles (sans oracle) améliore le retrieval");
}

#[test]
fn rerank_pure_boost_zero_bit_identique() {
    // Bit-identité de la fonction pure : boost = 0.0 ⇒ les candidats
    // traversent inchangés (tri descendant + top-k), quel que soit le
    // résolveur. C'est la garantie « le chemin non-topologique n'est jamais
    // altéré » — testée au niveau où elle vit (les candidats en entrée de la
    // fonction), pas au niveau du pipeline dont le sur-échantillonnage
    // interne est un paramètre d'appel distinct.
    use obrain_engine::database::topological::rerank_by_cluster_dominance;

    // Cas construit à la main : A (1.0), B (0.9), C (0.8), D (0.7) — déjà
    // triés par score descendant.
    let cands: Vec<(NodeId, f64)> = vec![
        (NodeId::new(1), 1.0),
        (NodeId::new(2), 0.9),
        (NodeId::new(3), 0.8),
        (NodeId::new(4), 0.7),
    ];
    let want = cands.clone();

    // boost = 0 : bit-identique (même avec résolveur actif) — scores inclus.
    let out0 = rerank_by_cluster_dominance(cands.clone(), 4, 0.0, |_| Some(0));
    assert_eq!(out0, want, "boost=0 doit laisser les candidats inchangés");

    // Résolveur None partout + boost>0 : aucun bonus ⇒ inchangé (scores inclus).
    let out_none = rerank_by_cluster_dominance(cands.clone(), 4, 1.0, |_| None);
    assert_eq!(out_none, want, "cluster None partout ⇒ aucun bonus");

    let ordre = |v: &[(NodeId, f64)]| -> Vec<u64> { v.iter().map(|(id, _)| id.0).collect() };

    // Clusters équilibrés {A,C} vs {B,D} (dominance 0,5 partout) : bonus
    // identique pour tous ⇒ l'ORDRE est inchangé (les scores montent, c'est
    // le contrat du boost).
    let balanced = rerank_by_cluster_dominance(cands.clone(), 4, 1.0, |id| match id.0 {
        1 | 3 => Some(0),
        _ => Some(1),
    });
    assert_eq!(
        ordre(&balanced),
        ordre(&want),
        "dominance égale ⇒ ordre inchangé"
    );

    // Clusters déséquilibrés : A et C ensemble (dominance 0,5), B seul
    // (0,25), D seul (0,25) ⇒ A,C gagnent 0,5 ; B,D gagnent 0,25.
    // Scores : A 1,5 · B 1,15 · C 1,3 · D 0,95 → ordre A, C, B, D.
    let mordant = rerank_by_cluster_dominance(cands, 4, 1.0, |id| match id.0 {
        1 | 3 => Some(0),
        2 => Some(1),
        _ => Some(2),
    });
    let ids: Vec<u64> = mordant.iter().map(|(id, _)| id.0).collect();
    assert_eq!(ids, vec![1, 3, 2, 4], "le boost doit réordonner A,C,B,D");
    eprintln!("C2 bit-identité OK — boost=0/None ⇒ inchangé ; équilibré ⇒ ordre stable ; déséquilibré ⇒ réordonne");
}
