//! PRÉDICTION COMPOSITIONNELLE — protocole CORRIGÉ (3e mission).
//!
//! Corrections vs version précédente :
//!  A. K1, K2 apprises INDÉPENDAMMENT ; K3 n'est JAMAIS construite avant la
//!     prédiction. L'apprentissage utilise la politique ÉPISODIQUE (un engramme
//!     par ensemble observé) pour que les parties restent DISTINCTES.
//!  B. La prédiction est calculée AVANT toute présentation du composite.
//!  C. Information ÉGALE : union exacte des participants rappelés, moyenne des
//!     projections, composition explicite, rappel natif.
//!  D. Compositions inconnues : disjoints, partagés, relations différentes,
//!     directions opposées, rôles différents.
//!  E. FIDÉLITÉ STRUCTURELLE (participants / relations / imbrication).
//!
//! Un second test démontre la FUSION sous formation Hebbienne (limite).

#![cfg(feature = "cognitive-engram")]

use std::collections::{BTreeMap, BTreeSet};

use obrain_common::types::NodeId;
use obrain_cognitive::engram::PredictivePolicy;
use obrain_engine::{ChildRef, Composition, KnowledgeSpec, ObrainDB, Relation};

type Rel = (u64, u64, String, Option<String>);

#[derive(Clone, Default)]
struct Struct {
    members: BTreeSet<u64>,
    relations: BTreeSet<Rel>,
}

fn jaccard<T: Ord>(a: &BTreeSet<T>, b: &BTreeSet<T>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

fn spec(s: &Struct) -> KnowledgeSpec {
    let relations: BTreeMap<Relation, usize> = s
        .relations
        .iter()
        .map(|(src, dst, ty, role)| {
            (
                Relation {
                    rel_type: ty.clone(),
                    src: NodeId(*src),
                    dst: NodeId(*dst),
                    role: role.clone(),
                },
                1usize,
            )
        })
        .collect();
    KnowledgeSpec {
        members: s.members.iter().map(|n| NodeId(*n)).collect(),
        children: Default::default(),
        relations,
    }
}

fn structural_fidelity(pred: &Struct, truth: &Struct) -> (f64, f64) {
    (
        jaccard(&pred.members, &truth.members),
        jaccard(&pred.relations, &truth.relations),
    )
}

struct ScenarioDef {
    name: &'static str,
    k1_members: [usize; 2],
    k2_members: [usize; 2],
    k1_rels: &'static [(usize, usize, &'static str, Option<&'static str>)],
    k2_rels: &'static [(usize, usize, &'static str, Option<&'static str>)],
}

fn build_parts(def: &ScenarioDef, ids: &[u64; 4]) -> (Struct, Struct) {
    let mk = |members: &[usize; 2],
              rels: &[(usize, usize, &str, Option<&str>)]|
     -> Struct {
        Struct {
            members: members.iter().map(|i| ids[*i]).collect(),
            relations: rels
                .iter()
                .map(|(s2, d2, t, r)| {
                    (ids[*s2], ids[*d2], t.to_string(), r.map(|x| x.to_string()))
                })
                .collect(),
        }
    };
    (mk(&def.k1_members, def.k1_rels), mk(&def.k2_members, def.k2_rels))
}

#[test]
fn prediction_compositionnelle_protocole_corrige() {
    let defs = vec![
        ScenarioDef { name: "disjoints", k1_members: [0, 1], k2_members: [2, 3], k1_rels: &[], k2_rels: &[] },
        ScenarioDef { name: "participants_partages", k1_members: [0, 1], k2_members: [1, 2], k1_rels: &[], k2_rels: &[] },
        ScenarioDef { name: "relations_differentes", k1_members: [0, 1], k2_members: [2, 3], k1_rels: &[(0, 1, "cause", None)], k2_rels: &[(2, 3, "support", None)] },
        ScenarioDef { name: "directions_opposees", k1_members: [0, 1], k2_members: [2, 3], k1_rels: &[(0, 1, "r", None)], k2_rels: &[(3, 2, "r", None)] },
        ScenarioDef { name: "roles_differents", k1_members: [0, 1], k2_members: [2, 3], k1_rels: &[(0, 1, "r", Some("source"))], k2_rels: &[(2, 3, "r", Some("effect"))] },
    ];

    for def in &defs {
        // Base ISOLÉE par scénario : la voie de rappel Hopfield est dense et
        // renverrait les engrams des scénarios précédents (pollution du rappel).
        let dir = tempfile::tempdir().unwrap();
        let db = ObrainDB::open_substrate_observed(dir.path().join("base")).unwrap();
        let comp = Composition::new(&db);
        let mgr = db
            .cognitive_engine()
            .expect("moteur cognitif")
            .engram_manager()
            .expect("engram manager");

        // 0. Quatre participants réels (ids dynamiques).
        let mut ids = [0u64; 4];
        for slot in ids.iter_mut() {
            *slot = db.create_node(&["Participant"]).0;
        }
        let (k1_struct, k2_struct) = build_parts(def, &ids);

        // A. Apprendre K1, K2 indépendamment par politique ÉPISODIQUE (pas de fusion).
        let k1_node = comp.construct(&spec(&k1_struct)).expect("construire K1");
        let k2_node = comp.construct(&spec(&k2_struct)).expect("construire K2");
        let m1: Vec<NodeId> = k1_struct.members.iter().map(|n| NodeId(*n)).collect();
        let m2: Vec<NodeId> = k2_struct.members.iter().map(|n| NodeId(*n)).collect();
        for _ in 0..3 {
            let _ = mgr
                .predictive_step_with_policy(&[m1[0]], &m1, 8, PredictivePolicy::Episodic)
                .expect("pas K1");
            let _ = mgr
                .predictive_step_with_policy(&[m2[0]], &m2, 8, PredictivePolicy::Episodic)
                .expect("pas K2");
        }

        // Vérité terrain du composite, calculée ABSTRAITEMENT (pas dans le graphe).
        let truth = Struct {
            members: k1_struct.members.union(&k2_struct.members).copied().collect(),
            relations: k1_struct.relations.union(&k2_struct.relations).cloned().collect(),
        };

        // Garde-fou : aucun engram n'a observé le composite entier avant la prédiction.
        let composite_seen = mgr.store().list().iter().any(|e| {
            let s: BTreeSet<u64> = e.node_ids().iter().map(|n| n.0).collect();
            s == truth.members
        });
        assert!(
            !composite_seen,
            "[{}] le composite n'a pas été observé avant la prédiction",
            def.name
        );

        // B. PRÉDICTION (avant de construire K3).
        let cues = vec![m1[0], m2[0]];
        let truth_ids: Vec<NodeId> = truth.members.iter().map(|n| NodeId(*n)).collect();
        let truth_emb = mgr.encode_observation(&truth_ids);

        // (i) Centroïde natif (projection).
        let centroid = mgr.predict(&cues, 8).expect("centroïde");
        let centroid_err = mgr
            .prediction_error(&centroid.predicted, &truth_emb)
            .expect("dim");

        // (ii) Union exacte des participants rappelés (structure).
        let mut union_recalled = Struct::default();
        for r in mgr.recall(&cues, 8) {
            for n in r.engram.node_ids() {
                union_recalled.members.insert(n.0);
            }
        }
        let (u_members, u_relations) = structural_fidelity(&union_recalled, &truth);

        // (iii) Moyenne des projections des parties.
        let p1 = mgr.encode_observation(&m1);
        let p2 = mgr.encode_observation(&m2);
        let avg: Vec<f64> = p1.iter().zip(&p2).map(|(x, y)| (x + y) / 2.0).collect();
        let avg_err = mgr.prediction_error(&avg, &truth_emb).expect("dim");

        // (iv) Composition EXPLICITE des parties (structure, information égale).
        let mut explicit = Struct::default();
        for part in [&k1_struct, &k2_struct] {
            explicit.members.extend(part.members.iter().copied());
            explicit.relations.extend(part.relations.iter().cloned());
        }
        let (e_members, e_relations) = structural_fidelity(&explicit, &truth);

        // (v) Rappel cognitif natif.
        let recalled = mgr.recall(&cues, 8);
        let recalled_ids: Vec<u64> = recalled.iter().map(|r| r.engram_id.0).collect();

        // C. Mesure.
        eprintln!(
            "[{}] vérité: {} membres, {} relations",
            def.name,
            truth.members.len(),
            truth.relations.len()
        );
        eprintln!(
            "[{}] centroïde: erreur_vect={:.4} (projection non décodable -> fidélité structurelle N/A)",
            def.name, centroid_err
        );
        eprintln!(
            "[{}] union_rappelée: fidélité participants={:.3} relations={:.3}",
            def.name, u_members, u_relations
        );
        eprintln!("[{}] moyenne_projections: erreur_vect={:.4}", def.name, avg_err);
        eprintln!(
            "[{}] composition_explicite: fidélité participants={:.3} relations={:.3}",
            def.name, e_members, e_relations
        );
        eprintln!("[{}] rappel_natif: engrams={:?}", def.name, recalled_ids);

        // Assertions HONNÊTES.
        assert_eq!(u_members, 1.0, "[{}] union rappelée = participants exacts", def.name);
        if !truth.relations.is_empty() {
            assert_eq!(
                u_relations, 0.0,
                "[{}] union rappelée perd toutes les relations (les engrams ne les portent pas)",
                def.name
            );
        }
        assert_eq!(e_members, 1.0, "[{}] composition explicite exacte (membres)", def.name);
        assert_eq!(e_relations, 1.0, "[{}] composition explicite exacte (relations)", def.name);
        assert!(centroid_err.is_finite(), "[{}] centroïde mesurable", def.name);

        // D. APRÈS la prédiction : construire K3 et vérifier la résolution.
        let k3 = comp
            .construct(&KnowledgeSpec {
                members: Default::default(),
                children: [k1_node, k2_node]
                    .into_iter()
                    .map(|child| (ChildRef { target: child, role: None }, 1usize))
                    .collect(),
                relations: Default::default(),
            })
            .expect("construire K3");
        let resolved = comp.resolve(k3).expect("resolve K3");
        let resolved_set: BTreeSet<u64> = resolved.iter().map(|n| n.0).collect();
        assert_eq!(
            resolved_set, truth.members,
            "[{}] resolve(K3) aplatit les parties = vérité terrain",
            def.name
        );
        eprintln!(
            "[{}] imbrication: K3={} -> enfants K1={} K2={} ; resolve={} membres",
            def.name,
            k3.0,
            k1_node.0,
            k2_node.0,
            resolved.len()
        );
    }
}

/// LIMITE MESURÉE : sous formation HEBBITENNE (composantes connexes), deux
/// connaissances qui PARTAGENT un participant FUSIONNENT en un seul engramme,
/// qui est alors le composite lui-même — donc « observer », pas « prédire ».
#[test]
fn fusion_hebbienne_des_parties_partagees() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate_observed(dir.path().join("base")).unwrap();
    let mgr = db
        .cognitive_engine()
        .expect("moteur")
        .engram_manager()
        .expect("manager");

    let a = db.create_node(&["P"]);
    let b = db.create_node(&["P"]);
    let c = db.create_node(&["P"]);

    // K1 = {a,b}, K2 = {b,c} : participant b PARTAGÉ.
    let k1 = vec![a, b];
    let k2 = vec![b, c];
    for _ in 0..3 {
        let _ = mgr.predictive_step(&[a], &k1, 8).expect("pas K1");
        let _ = mgr.predictive_step(&[b], &k2, 8).expect("pas K2");
    }

    // Le composite {a,b,c} existe-t-il comme engramme ? (fusion)
    let fused = mgr.store().list().iter().any(|e| {
        let s: BTreeSet<u64> = e.node_ids().iter().map(|n| n.0).collect();
        s == [a.0, b.0, c.0].into_iter().collect()
    });
    eprintln!(
        "[fusion] engrams = {:?} ; composite {{a,b,c}} présent = {}",
        mgr.store()
            .list()
            .iter()
            .map(|e| e.node_ids().iter().map(|n| n.0).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        fused
    );
    assert!(
        fused,
        "LIMITE : la formation hebbienne fusionne les parties partagées en le composite"
    );
}
