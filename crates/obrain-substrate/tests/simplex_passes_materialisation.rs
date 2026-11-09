//! Passes de matérialisation simpliciales (T5) — comportement de la passe
//! complète sur le store réel : triangles closed-wedge + CYCLIC,
//! configuration persistante (reopen + WAL), idempotence et réparation
//! (graphe rétréci), clustering.
//!
//! Les volumes de référence (1 346 triangles / 51 K4 / hyperarêtes) sont
//! validés hors suite, sur la base obrain-carto-db (validation croisée
//! cbm) — ici, les invariants sont testés sur des graphes maîtrisés.

use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::{
    DispatchSpec, OrientationFace, SimplexPassesSpec, SubstrateStore, simplex_flags, simplex_kind,
};

use obrain_common::types::{NodeId, Value};

fn spec_calls_sans_dispatch() -> SimplexPassesSpec {
    SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec![],
        dispatch: None,
        maintenance: false,
    }
}

/// Crée `n` nœuds et rend leurs ids (slots 1..=n dans un store neuf).
fn noeuds(s: &SubstrateStore, n: usize) -> Vec<NodeId> {
    (0..n).map(|_| s.create_node(&["F"])).collect()
}

fn vue_triangles(s: &SubstrateStore) -> Vec<(Vec<u32>, u8)> {
    let mut out = Vec::new();
    for id in s.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap() {
        let v = s.get_simplex(id).unwrap().unwrap();
        out.push((v.members, v.flags));
    }
    out.sort();
    out
}

#[test]
fn passe_triangles_closed_wedge_et_cyclic() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 7);
    // Wedge fermé acyclique : n0→n1, n0→n2, n1→n2.
    s.create_edge(n[0], n[1], "CALLS");
    s.create_edge(n[0], n[2], "CALLS");
    s.create_edge(n[1], n[2], "CALLS");
    // Wedge fermé + arête de fermeture n5→n3 ⇒ CYCLIC.
    s.create_edge(n[3], n[4], "CALLS");
    s.create_edge(n[3], n[5], "CALLS");
    s.create_edge(n[4], n[5], "CALLS");
    s.create_edge(n[5], n[3], "CALLS");
    // 3-cycle dirigé PUR n4→n6→n0→n4 : PAS un closed-wedge, pas matérialisé.
    s.create_edge(n[4], n[6], "CALLS");
    s.create_edge(n[6], n[0], "CALLS");
    s.create_edge(n[0], n[4], "CALLS");
    // Bruit d'un autre type : jamais compté.
    s.create_edge(n[0], n[1], "IMPORTS");
    s.create_edge(n[1], n[2], "IMPORTS");

    s.configurer_passes_simpliciales(&spec_calls_sans_dispatch())
        .unwrap();
    let stats = s.materialiser_simplexes().unwrap();
    assert_eq!(stats.triangles, 2);
    assert_eq!(stats.triangles_cycliques, 1);
    assert_eq!(stats.crees, 2);
    assert_eq!((stats.k4, stats.k5, stats.hyperedges), (0, 0, 0));

    let tris = vue_triangles(&s);
    let attendu = vec![
        (vec![n[0].0 as u32, n[1].0 as u32, n[2].0 as u32], 0u8),
        (
            vec![n[3].0 as u32, n[4].0 as u32, n[5].0 as u32],
            simplex_flags::CYCLIC,
        ),
    ];
    assert_eq!(tris, attendu);
}

#[test]
fn passe_idempotente_puis_reparatrice_sur_graphe_retreci() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 4);
    s.create_edge(n[0], n[1], "CALLS");
    s.create_edge(n[0], n[2], "CALLS");
    let e_bc = s.create_edge(n[1], n[2], "CALLS");
    s.create_edge(n[0], n[3], "CALLS");
    s.create_edge(n[1], n[3], "CALLS");

    s.configurer_passes_simpliciales(&spec_calls_sans_dispatch())
        .unwrap();
    // Deux triangles : (n0,n1,n2) et (n0,n1,n3).
    let s1 = s.materialiser_simplexes().unwrap();
    assert_eq!(
        (s1.triangles, s1.crees, s1.inchanges, s1.tombstones),
        (2, 2, 0, 0)
    );

    // Re-matérialisation sans changement : « replaces, never duplicates »
    // — tout est no-op, aucun tombstone, aucun doublon.
    let s2 = s.materialiser_simplexes().unwrap();
    assert_eq!(
        (s2.triangles, s2.crees, s2.inchanges, s2.tombstones),
        (2, 0, 2, 0)
    );

    // Graphe rétréci : n1→n2 disparaît, (n0,n1,n2) est périmé,
    // (n0,n1,n3) survit intact.
    assert!(s.delete_edge(e_bc));
    let s3 = s.materialiser_simplexes().unwrap();
    assert_eq!((s3.triangles, s3.inchanges, s3.tombstones), (1, 1, 1));
    let restants = vue_triangles(&s);
    assert_eq!(restants.len(), 1);
    assert_eq!(
        restants[0].0,
        vec![n[0].0 as u32, n[1].0 as u32, n[3].0 as u32]
    );
}

#[test]
fn configuration_persistante_au_reopen() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    {
        let s = SubstrateStore::create(&chemin).unwrap();
        let _ = noeuds(&s, 3);
        let cfg = s
            .configurer_passes_simpliciales(&SimplexPassesSpec {
                clique_edge_type: "CALLS".into(),
                clique_labels: vec!["Function".into(), "Method".into()],
                dispatch: Some(DispatchSpec {
                    edge_type: "CALLS".into(),
                    group_prop_key: "line".into(),
                    he_type: "DISPATCH".into(),
                }),
                maintenance: true,
            })
            .unwrap();
        assert!(cfg.maintenance_active());
        assert!(cfg.dispatch.is_some());
        assert_ne!(cfg.clique_label_mask, 0, "deux labels internés → deux bits");
        s.flush().unwrap();
        let relue = s.passes_simpliciales_config().unwrap().unwrap();
        assert_eq!(relue, cfg);
        drop(s);
        let s = SubstrateStore::open(&chemin).unwrap();
        let apres_reopen = s.passes_simpliciales_config().unwrap().unwrap();
        assert_eq!(apres_reopen, cfg);
    }
}

#[test]
fn erreur_enseignante_sans_configuration() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let err = s.materialiser_simplexes().unwrap_err();
    assert!(
        err.to_string().contains("configurer_passes_simpliciales"),
        "le diagnostic doit enseigner le remède : {err}"
    );
    assert!(s.passes_simpliciales_config().unwrap().is_none());
}

#[test]
fn clustering_noeud_et_global() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 4);
    // K4 dirigé canonique complet sur n0..n3 (6 arêtes low→high) : chaque
    // nœud a k=3 voisins non orientés, tous les C(3,2)=3 triplets fermés.
    for i in 0..4 {
        for j in (i + 1)..4 {
            s.create_edge(n[i], n[j], "CALLS");
        }
    }
    s.configurer_passes_simpliciales(&spec_calls_sans_dispatch())
        .unwrap();
    let stats = s.materialiser_simplexes().unwrap();
    assert_eq!(stats.triangles, 4); // C(4,3)

    for id in &n {
        assert_eq!(s.triangles_du_noeud(*id).unwrap(), 3);
        assert_eq!(s.clustering_noeud(*id).unwrap(), Some(1.0));
    }
    // Transitivité : 3×4 / Σ C(3,2) = 12 / 12 = 1.
    assert_eq!(s.clustering_global().unwrap(), Some(1.0));

    // Nœud isolé : pas de triplet ⇒ None.
    let seul = s.create_node(&["F"]);
    assert_eq!(s.clustering_noeud(seul).unwrap(), None);
}

#[test]
fn passe_cliques_et_hyperedges_dispatch() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 9);
    // K4 dirigé canonique complet sur n0..n3.
    for i in 0..4 {
        for j in (i + 1)..4 {
            s.create_edge(n[i], n[j], "CALLS");
        }
    }
    // Site DISPATCH : n4 appelle n5 et n6 depuis la ligne 7, n7 depuis la
    // ligne 9 (une seule cible : pas un dispatch), n8 SANS ligne (non
    // groupable).
    let e1 = s.create_edge(n[4], n[5], "CALLS");
    let e2 = s.create_edge(n[4], n[6], "CALLS");
    let e3 = s.create_edge(n[4], n[7], "CALLS");
    s.create_edge(n[4], n[8], "CALLS");
    s.set_edge_property(e1, "line", Value::Int64(7));
    s.set_edge_property(e2, "line", Value::Int64(7));
    s.set_edge_property(e3, "line", Value::Int64(9));

    let cfg = s
        .configurer_passes_simpliciales(&SimplexPassesSpec {
            clique_edge_type: "CALLS".into(),
            clique_labels: vec![],
            dispatch: Some(DispatchSpec {
                edge_type: "CALLS".into(),
                group_prop_key: "line".into(),
                he_type: "DISPATCH".into(),
            }),
            maintenance: false,
        })
        .unwrap();
    let stats = s.materialiser_simplexes().unwrap();
    assert_eq!(stats.triangles, 4); // C(4,3)
    assert_eq!((stats.k4, stats.k5), (1, 0));
    assert_eq!(stats.hyperedges, 1);
    assert_eq!(stats.membres_hyperedges, 3);
    assert_eq!(stats.groupes_hors_bornes, 0);

    // Le K4 est bien (n0,n1,n2,n3).
    let k4_ids = s.iter_simplices_kind(simplex_kind::K4).unwrap();
    assert_eq!(k4_ids.len(), 1);
    let k4 = s.get_simplex(k4_ids[0]).unwrap().unwrap();
    let attendu: Vec<u32> = (0..4).map(|i| n[i].0 as u32).collect();
    assert_eq!(k4.members, attendu);

    // L'hyperarête : membres {n4,n5,n6} triés, type DISPATCH, props
    // (caller u32 LE, ligne i64 LE).
    let he_ids = s.iter_simplices_kind(simplex_kind::HYPEREDGE).unwrap();
    assert_eq!(he_ids.len(), 1);
    let he = s.get_simplex(he_ids[0]).unwrap().unwrap();
    let mut membres_attendus: Vec<u32> = [4, 5, 6].iter().map(|&i| n[i].0 as u32).collect();
    membres_attendus.sort_unstable();
    assert_eq!(he.members, membres_attendus);
    assert_eq!(
        he.type_id,
        cfg.dispatch.unwrap().he_type,
        "type_id = id dict du type DISPATCH"
    );
    let caller = u32::from_le_bytes(he.props[0..4].try_into().unwrap());
    let ligne = i64::from_le_bytes(he.props[4..12].try_into().unwrap());
    assert_eq!((caller, ligne), (n[4].0 as u32, 7));

    // L'incidence relie chaque membre à l'hyperarête (navigation
    // sommet → simplexes).
    for &i in &[4usize, 5, 6] {
        assert!(
            s.simplices_of_node(n[i]).unwrap().contains(&he_ids[0]),
            "incidence du membre n{i}"
        );
    }

    // Le site rétrécit : plus qu'UNE cible ligne 7 ⇒ l'hyperarête est
    // périmée à la re-matérialisation.
    assert!(s.delete_edge(e2));
    let stats2 = s.materialiser_simplexes().unwrap();
    assert_eq!(stats2.hyperedges, 0);
    assert!(
        s.iter_simplices_kind(simplex_kind::HYPEREDGE)
            .unwrap()
            .is_empty(),
        "résultat attendu"
    );
}

#[test]
fn dispatch_bornes_i3_groupe_jete_entier() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    // 70 cibles depuis le même site : 71 membres > 64 ⇒ groupe jeté ENTIER.
    let n = noeuds(&s, 71);
    for i in 1..71 {
        let e = s.create_edge(n[0], n[i], "CALLS");
        s.set_edge_property(e, "line", Value::Int64(3));
    }
    s.configurer_passes_simpliciales(&SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec![],
        dispatch: Some(DispatchSpec {
            edge_type: "CALLS".into(),
            group_prop_key: "line".into(),
            he_type: "DISPATCH".into(),
        }),
        maintenance: false,
    })
    .unwrap();
    let stats = s.materialiser_simplexes().unwrap();
    assert_eq!(stats.hyperedges, 0);
    assert_eq!(stats.groupes_hors_bornes, 1);
    assert!(
        s.iter_simplices_kind(simplex_kind::HYPEREDGE)
            .unwrap()
            .is_empty(),
        "résultat attendu"
    );
}

#[test]
fn passe_avec_filtre_de_labels() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    // Triangle entre trois Function + un triangle « pollué » par un File.
    let a = s.create_node(&["Function"]);
    let b = s.create_node(&["Method"]);
    let c = s.create_node(&["Function"]);
    let f = s.create_node(&["File"]);
    for (x, y) in [(a, b), (a, c), (b, c), (a, f), (b, f)] {
        s.create_edge(x, y, "CALLS");
    }
    s.configurer_passes_simpliciales(&SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec!["Function".into(), "Method".into()],
        dispatch: None,
        maintenance: false,
    })
    .unwrap();
    let stats = s.materialiser_simplexes().unwrap();
    assert_eq!(
        stats.triangles, 1,
        "le wedge via File est hors graphe d'appel"
    );
    let tris = vue_triangles(&s);
    assert_eq!(tris[0].0, vec![a.0 as u32, b.0 as u32, c.0 as u32]);
}

#[test]
fn emission_lexicographique_ids_croissants() {
    // I4 : l'ordre d'émission (donc l'allocation des ids sur base neuve)
    // est l'ordre lexicographique des membres.
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 5);
    // Triangles (n0,n1,n2), (n0,n1,n3), (n2,n3,n4) — créés dans le désordre.
    for (a, b) in [
        (2usize, 4usize),
        (3, 4),
        (2, 3),
        (0, 1),
        (1, 3),
        (0, 3),
        (1, 2),
        (0, 2),
    ] {
        s.create_edge(n[a], n[b], "CALLS");
    }
    s.configurer_passes_simpliciales(&spec_calls_sans_dispatch())
        .unwrap();
    s.materialiser_simplexes().unwrap();

    let ids = s.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap();
    let membres: Vec<Vec<u32>> = ids
        .iter()
        .map(|id| s.get_simplex(*id).unwrap().unwrap().members)
        .collect();
    let mut tries = membres.clone();
    tries.sort();
    assert_eq!(membres, tries, "ids croissants ⇔ membres lexicographiques");
}

#[test]
fn faces_orientees_et_navigation_0_simplexes() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 6);
    // Face non orientée : wedge fermé n0,n1,n2 sans fermeture.
    s.create_edge(n[0], n[1], "CALLS");
    s.create_edge(n[0], n[2], "CALLS");
    s.create_edge(n[1], n[2], "CALLS");
    // Face ORIENTÉE : wedge fermé n3,n4,n5 + fermeture n5→n3 (3-cycle).
    s.create_edge(n[3], n[4], "CALLS");
    s.create_edge(n[3], n[5], "CALLS");
    s.create_edge(n[4], n[5], "CALLS");
    s.create_edge(n[5], n[3], "CALLS");
    s.configurer_passes_simpliciales(&spec_calls_sans_dispatch())
        .unwrap();
    s.materialiser_simplexes().unwrap();

    let ids = s.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap();
    assert_eq!(ids.len(), 2);
    // Émission lexicographique : ids[0] = (n0,n1,n2), ids[1] = (n3,n4,n5).
    assert_eq!(
        s.orientation_face(ids[0]).unwrap(),
        Some(OrientationFace::NonOrientee)
    );
    assert_eq!(
        s.orientation_face(ids[1]).unwrap(),
        Some(OrientationFace::Cyclique)
    );
    // L'orientation lue = le bit CYCLIC stocké (cohérence flags/API).
    let vue = s.get_simplex(ids[1]).unwrap().unwrap();
    assert_ne!(vue.flags & simplex_flags::CYCLIC, 0);

    // Navigation face → sommets : les membres sont des 0-simplexes
    // vivants ; l'inverse (sommet → faces) retombe sur la face.
    let sommets = s.sommets_de_simplexe(ids[1]).unwrap().unwrap();
    assert_eq!(sommets, vec![n[3], n[4], n[5]]);
    for v in &sommets {
        assert!(s.est_0_simplexe(*v));
        assert!(s.simplices_of_node(*v).unwrap().contains(&ids[1]));
    }

    // Un id inexistant ou non-triangle n'a pas d'orientation.
    assert_eq!(s.orientation_face(9999).unwrap(), None);

    // Un nœud supprimé n'est plus un 0-simplexe — et la cascade I5 (T4)
    // tombstone ses faces : l'orientation disparaît avec la face.
    assert!(s.delete_node(n[5]));
    assert!(!s.est_0_simplexe(n[5]));
    assert_eq!(s.orientation_face(ids[1]).unwrap(), None);
    assert_eq!(s.sommets_de_simplexe(ids[1]).unwrap(), None);
}

/// Identité sémantique d'un simplexe vivant : (kind, type_id, membres,
/// flags, props) — indépendante des ids.
type IdentiteSimplexe = (u8, u16, Vec<u32>, u8, Vec<u8>);

/// Ensemble canonique des simplexes vivants.
fn ensemble_vivant(s: &SubstrateStore) -> std::collections::BTreeSet<IdentiteSimplexe> {
    let mut out = std::collections::BTreeSet::new();
    for kind in [
        simplex_kind::TRIANGLE,
        simplex_kind::K4,
        simplex_kind::K5,
        simplex_kind::HYPEREDGE,
    ] {
        for id in s.iter_simplices_kind(kind).unwrap() {
            let v = s.get_simplex(id).unwrap().unwrap();
            out.insert((v.kind, v.type_id, v.members, v.flags, v.props));
        }
    }
    out
}

fn spec_maintenance() -> SimplexPassesSpec {
    SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec![],
        dispatch: Some(DispatchSpec {
            edge_type: "CALLS".into(),
            group_prop_key: "line".into(),
            he_type: "DISPATCH".into(),
        }),
        maintenance: true,
    }
}

#[test]
fn incremental_converge_vers_le_rebuild() {
    // Store A : couche simpliciale maintenue INCRÉMENTALEMENT au fil de
    // mutations adverses (ajouts dans le désordre, fermetures tardives,
    // suppressions, propriétés changées). Store B : même graphe FINAL,
    // matérialisé par la passe complète. Contrat spec §4 : mêmes
    // simplexes vivants (kind, type, membres, flags, props).
    let td = tempfile::tempdir().unwrap();
    let a = SubstrateStore::create(td.path().join("a")).unwrap();
    let na = noeuds(&a, 10);
    a.configurer_passes_simpliciales(&spec_maintenance())
        .unwrap();
    a.materialiser_simplexes().unwrap(); // couche vide, active la vie au commit

    // K4 dirigé n0..n3 construit arête par arête, dans le désordre.
    for (i, j) in [(2usize, 3usize), (0, 1), (1, 3), (0, 3), (1, 2), (0, 2)] {
        a.create_edge(na[i], na[j], "CALLS");
    }
    // 3-cycle dirigé n4,n5,n6 : wedge fermé + fermeture TARDIVE.
    a.create_edge(na[4], na[5], "CALLS");
    a.create_edge(na[4], na[6], "CALLS");
    a.create_edge(na[5], na[6], "CALLS");
    a.create_edge(na[6], na[4], "CALLS"); // pose le bit CYCLIC après coup
    // Triangle éphémère n7,n8,n9 : créé PUIS cassé.
    a.create_edge(na[7], na[8], "CALLS");
    a.create_edge(na[7], na[9], "CALLS");
    let e_ephemere = a.create_edge(na[8], na[9], "CALLS");
    assert!(a.delete_edge(e_ephemere));
    // DISPATCH : site (n7, ligne 5) — cibles ajoutées, une ligne déplacée.
    let d1 = a.create_edge(na[7], na[4], "CALLS");
    let d2 = a.create_edge(na[7], na[5], "CALLS");
    a.set_edge_property(d1, "line", Value::Int64(5));
    a.set_edge_property(d2, "line", Value::Int64(9)); // pas encore un groupe
    a.set_edge_property(d2, "line", Value::Int64(5)); // déplacée → groupe (n7,5)
    // Une cible de plus puis retirée par RETRAIT de la propriété.
    let d3 = a.create_edge(na[7], na[6], "CALLS");
    a.set_edge_property(d3, "line", Value::Int64(5));
    assert!(a.remove_edge_property(d3, "line").is_some());

    // Store B : le même graphe final, passe complète.
    let b = SubstrateStore::create(td.path().join("b")).unwrap();
    let nb = noeuds(&b, 10);
    b.configurer_passes_simpliciales(&spec_maintenance())
        .unwrap();
    for (i, j) in [(2usize, 3usize), (0, 1), (1, 3), (0, 3), (1, 2), (0, 2)] {
        b.create_edge(nb[i], nb[j], "CALLS");
    }
    b.create_edge(nb[4], nb[5], "CALLS");
    b.create_edge(nb[4], nb[6], "CALLS");
    b.create_edge(nb[5], nb[6], "CALLS");
    b.create_edge(nb[6], nb[4], "CALLS");
    b.create_edge(nb[7], nb[8], "CALLS");
    b.create_edge(nb[7], nb[9], "CALLS");
    let f1 = b.create_edge(nb[7], nb[4], "CALLS");
    let f2 = b.create_edge(nb[7], nb[5], "CALLS");
    b.create_edge(nb[7], nb[6], "CALLS");
    b.set_edge_property(f1, "line", Value::Int64(5));
    b.set_edge_property(f2, "line", Value::Int64(5));
    b.materialiser_simplexes().unwrap();

    let va = ensemble_vivant(&a);
    let vb = ensemble_vivant(&b);
    assert_eq!(va, vb, "incrémental ≡ rebuild sur le même graphe final");
    // Sanité : le contenu attendu est bien là.
    assert_eq!(
        va.iter().filter(|e| e.0 == simplex_kind::TRIANGLE).count(),
        5
    );
    assert_eq!(va.iter().filter(|e| e.0 == simplex_kind::K4).count(), 1);
    assert_eq!(
        va.iter()
            .filter(|e| e.0 == simplex_kind::TRIANGLE && e.3 & simplex_flags::CYCLIC != 0)
            .count(),
        1,
        "le 3-cycle n4,n5,n6 est la seule face orientée"
    );
    assert_eq!(
        va.iter().filter(|e| e.0 == simplex_kind::HYPEREDGE).count(),
        1
    );

    // Et la re-matérialisation de A est un NO-OP intégral : l'incrémental
    // avait déjà tout (0 créé, 0 réécrit, 0 tombstoné).
    let s = a.materialiser_simplexes().unwrap();
    assert_eq!((s.crees, s.reecrits, s.tombstones), (0, 0, 0));
}

#[test]
fn incremental_suppression_noeud_cascade() {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    let n = noeuds(&s, 4);
    s.configurer_passes_simpliciales(&spec_maintenance())
        .unwrap();
    s.materialiser_simplexes().unwrap();
    for i in 0..4 {
        for j in (i + 1)..4 {
            s.create_edge(n[i], n[j], "CALLS");
        }
    }
    assert_eq!(
        s.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap().len(),
        4
    );
    assert_eq!(s.iter_simplices_kind(simplex_kind::K4).unwrap().len(), 1);
    // DETACH DELETE de n3 : les hooks d'arêtes + la cascade I5 convergent
    // vers le triangle restant (n0,n1,n2).
    s.delete_node_edges(n[3]);
    assert!(s.delete_node(n[3]));
    assert_eq!(
        s.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap().len(),
        1
    );
    assert!(
        s.iter_simplices_kind(simplex_kind::K4).unwrap().is_empty(),
        "résultat attendu vide"
    );
    // Rebuild de contrôle : rien à réparer.
    let st = s.materialiser_simplexes().unwrap();
    assert_eq!((st.crees, st.reecrits, st.tombstones), (0, 0, 0));
}

#[test]
fn byte_identical_deux_publications_du_meme_graphe() {
    // Contrat I4 : deux publications du même graphe (même histoire de
    // construction) ⇒ zones simpliciales identiques à l'octet près après
    // compact_simplexes.
    let td = tempfile::tempdir().unwrap();
    let construire = |chemin: &std::path::Path| -> SubstrateStore {
        let s = SubstrateStore::create(chemin).unwrap();
        let n = noeuds(&s, 12);
        // Mélange : K4, 3-cycle orienté, dispatch.
        for i in 0..4 {
            for j in (i + 1)..4 {
                s.create_edge(n[i], n[j], "CALLS");
            }
        }
        s.create_edge(n[4], n[5], "CALLS");
        s.create_edge(n[4], n[6], "CALLS");
        s.create_edge(n[5], n[6], "CALLS");
        s.create_edge(n[6], n[4], "CALLS");
        for k in 7..12 {
            let e = s.create_edge(n[4], n[k], "CALLS");
            s.set_edge_property(e, "line", Value::Int64(33));
        }
        s.configurer_passes_simpliciales(&spec_maintenance())
            .unwrap();
        s.materialiser_simplexes().unwrap();
        s.compact_simplexes().unwrap();
        s.flush().unwrap();
        s
    };
    let s1 = construire(&td.path().join("pub1"));
    let s2 = construire(&td.path().join("pub2"));
    drop(s1);
    drop(s2);
    for fichier in ZONES_SIMPLICIALES {
        let a = std::fs::read(td.path().join("pub1").join(fichier)).unwrap();
        let b = std::fs::read(td.path().join("pub2").join(fichier)).unwrap();
        assert_eq!(a.len(), b.len(), "{fichier} : longueurs");
        assert_eq!(a, b, "{fichier} : octets");
    }
}

/// Les quatre zones simpliciales sur disque — le support du contrat I4.
const ZONES_SIMPLICIALES: [&str; 4] = [
    "substrate.simplex_dir",
    "substrate.simplex_heap",
    "substrate.simplex_incidence",
    "substrate.simplex_postings",
];

/// BYTE-IDENTICAL À L'ÉCHELLE DU CORPUS (rapport T5) — hors suite.
///
/// Le test unitaire ci-dessus prouve le contrat sur douze nœuds, où
/// l'ordre d'émission est trivialement stable. Celui-ci le prouve sur
/// 37 678 nœuds / 246 934 arêtes : 1 346 triangles, 51 cliques et 7 282
/// hyperarêtes émis depuis DEUX chargements INDÉPENDANTS du même dump
/// doivent donner les mêmes octets. C'est là que se verraient un ordre
/// d'itération instable, un tampon non initialisé ou un reste de
/// fragmentation dans le tas.
///
/// ```bash
/// obrain data load carto-dump-props.jsonl /tmp/a
/// obrain data load carto-dump-props.jsonl /tmp/b
/// CORPUS_A=/tmp/a CORPUS_B=/tmp/b cargo test --release -p obrain-substrate \
///   --test simplex_passes_materialisation -- --ignored byte_identical_corpus --nocapture
/// ```
/// ⚠️ les deux bases sont MODIFIÉES (matérialisation + compaction).
#[test]
#[ignore = "byte-identical corpus — CORPUS_A/CORPUS_B requis, bases modifiées"]
fn byte_identical_corpus() {
    let a = std::env::var("CORPUS_A").expect("CORPUS_A non défini");
    let b = std::env::var("CORPUS_B").expect("CORPUS_B non défini");
    assert_ne!(a, b, "deux chargements DISTINCTS sont attendus");

    let spec = SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec!["Function".into(), "Method".into()],
        dispatch: Some(DispatchSpec {
            edge_type: "CALLS".into(),
            group_prop_key: "line".into(),
            he_type: "DISPATCH".into(),
        }),
        maintenance: false,
    };
    let publier = |chemin: &str| {
        let s = SubstrateStore::open(chemin).unwrap();
        s.configurer_passes_simpliciales(&spec).unwrap();
        let stats = s.materialiser_simplexes().unwrap();
        s.compact_simplexes().unwrap();
        s.flush().unwrap();
        stats
    };
    let sa = publier(&a);
    let sb = publier(&b);

    // Prémisse : les deux publications portent bien le corpus attendu —
    // sans quoi comparer des zones vides passerait pour un succès.
    assert_eq!(
        (sa.triangles, sa.k4, sa.k5, sa.hyperedges),
        (sb.triangles, sb.k4, sb.k5, sb.hyperedges)
    );
    assert!(
        sa.triangles > 1_000 && sa.hyperedges > 1_000,
        "corpus de référence attendu, reçu {sa:?}"
    );
    println!(
        "PUBLICATIONS : {} triangles, {} K4, {} K5, {} hyperarêtes ({} membres)",
        sa.triangles, sa.k4, sa.k5, sa.hyperedges, sa.membres_hyperedges
    );

    for fichier in ZONES_SIMPLICIALES {
        let oa = std::fs::read(std::path::Path::new(&a).join(fichier)).unwrap();
        let ob = std::fs::read(std::path::Path::new(&b).join(fichier)).unwrap();
        assert_eq!(oa.len(), ob.len(), "{fichier} : longueurs");
        assert_eq!(oa, ob, "{fichier} : octets");
        println!("{fichier} : {} octets identiques", oa.len());
    }
}

/// RSS courant du processus (KiB) — mesure macOS via ps, assez précise
/// pour le contrat « RAM relâchée après passe ».
fn rss_kib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// MESURES CORPUS (rapport T5) — hors suite : exige la base de référence.
/// `CORPUS_DB=/tmp/t5-mesures-db cargo test --release -p obrain-substrate \
///  --test simplex_passes_materialisation -- --ignored mesures_corpus --nocapture`
/// ⚠️ la base est MODIFIÉE (matérialisation + arêtes d'essai) : travailler
/// sur un clone.
#[test]
#[ignore = "mesures corpus — CORPUS_DB requis, base modifiée"]
fn mesures_corpus() {
    let chemin = std::env::var("CORPUS_DB").expect("CORPUS_DB non défini");
    let s = SubstrateStore::open(&chemin).unwrap();

    // ── CONTRÔLE : coût de create_edge/delete_edge SANS maintenance
    //    (couche non configurée) — la différence isole le hook. ──
    let n_essais = 200u64;
    {
        let _ = s.create_edge(NodeId(50), NodeId(25_000), "CALLS"); // chauffe incoming_heads
        let mut aretes = Vec::new();
        let t0 = web_time::Instant::now();
        for i in 0..n_essais {
            aretes.push(s.create_edge(NodeId(100 + i), NodeId(20_000 + i), "CALLS"));
        }
        let d_ajout = t0.elapsed();
        let t0 = web_time::Instant::now();
        for e in &aretes {
            assert!(s.delete_edge(*e));
        }
        let d_supp = t0.elapsed();
        println!(
            "CONTROLE SANS MAINTENANCE : ajout {:.1} µs/arête, suppression {:.1} µs/arête",
            d_ajout.as_secs_f64() * 1e6 / n_essais as f64,
            d_supp.as_secs_f64() * 1e6 / n_essais as f64
        );
    }

    s.configurer_passes_simpliciales(&SimplexPassesSpec {
        clique_edge_type: "CALLS".into(),
        clique_labels: vec!["Function".into(), "Method".into()],
        dispatch: Some(DispatchSpec {
            edge_type: "CALLS".into(),
            group_prop_key: "line".into(),
            he_type: "DISPATCH".into(),
        }),
        maintenance: true,
    })
    .unwrap();

    // ── Passe complète : RSS avant/pendant(pic)/après + durée. ──
    let rss_avant = rss_kib();
    let pic = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sonde = {
        let (pic, stop) = (pic.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                pic.fetch_max(rss_kib(), std::sync::atomic::Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        })
    };
    let t0 = web_time::Instant::now();
    let stats = s.materialiser_simplexes().unwrap();
    let duree = t0.elapsed();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    sonde.join().unwrap();
    let rss_apres = rss_kib();
    println!(
        "PASSE COMPLETE : {:?} — {} triangles ({} cycliques), {} K4, {} K5, {} hyperarêtes ({} membres)",
        duree,
        stats.triangles,
        stats.triangles_cycliques,
        stats.k4,
        stats.k5,
        stats.hyperedges,
        stats.membres_hyperedges
    );
    println!(
        "RSS : avant {} KiB, pic pendant {} KiB, après {} KiB",
        rss_avant,
        pic.load(std::sync::atomic::Ordering::Relaxed),
        rss_apres
    );
    // Le delta avant→après est dominé par les PAGES MMAP fichier touchées
    // par la passe (zones nœuds/arêtes/props, ~250 Mo lus) — réclamables
    // par l'OS, pas du tas. Preuve que les tampons de passe (CSR,
    // attendus) sont bien rendus : une DEUXIÈME passe in-process ne fait
    // pas croître le RSS (et donne le coût re-matérialisation à chaud).
    let t0 = web_time::Instant::now();
    let _ = s.materialiser_simplexes().unwrap();
    println!(
        "SECONDE PASSE (no-op, à chaud) : {:?} — RSS {} KiB (croissance {} KiB)",
        t0.elapsed(),
        rss_kib(),
        rss_kib() as i64 - rss_apres as i64
    );

    // ── Borne incrémentale : 1 arête ajoutée/supprimée, maintenance
    //    active, sur le corpus entier. ──
    let base = ensemble_vivant(&s).len();
    // Warmup du chemin avec maintenance (incoming_heads déjà chaud).
    {
        let e = s.create_edge(NodeId(50), NodeId(25_001), "CALLS");
        assert!(s.delete_edge(e));
    }
    let mut aretes = Vec::new();
    let t0 = web_time::Instant::now();
    for i in 0..n_essais {
        let src = NodeId(100 + i);
        let dst = NodeId(20_000 + i);
        aretes.push(s.create_edge(src, dst, "CALLS"));
    }
    let d_ajout = t0.elapsed();
    let t0 = web_time::Instant::now();
    for e in &aretes {
        assert!(s.delete_edge(*e));
    }
    let d_supp = t0.elapsed();
    println!(
        "INCREMENTAL : ajout {:.1} µs/arête, suppression {:.1} µs/arête ({} essais)",
        d_ajout.as_secs_f64() * 1e6 / n_essais as f64,
        d_supp.as_secs_f64() * 1e6 / n_essais as f64,
        n_essais
    );
    // Le corpus n'a PAS été recalculé : l'ensemble vivant est revenu au
    // point de départ, et un rebuild de contrôle est un no-op intégral.
    assert_eq!(ensemble_vivant(&s).len(), base);
    let ctrl = s.materialiser_simplexes().unwrap();
    assert_eq!((ctrl.crees, ctrl.reecrits, ctrl.tombstones), (0, 0, 0));
    println!("CONTROLE : rebuild no-op intégral après ajout+suppression — OK");
}
