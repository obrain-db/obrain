//! La primitive compositionnelle native (Composition::construct / resolve) :
//! construction canonique, résolution, identité relationnelle, encodage
//! injectif, détection de cycles/diamants, validation des références.

// Noms à un caractère = notation A/B/C/D de l'expérience compositionnelle.
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeSet;

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec, Relation, ResolveError};

fn spec(members: &[u64], children: &[u64], relations: &[Relation]) -> KnowledgeSpec {
    KnowledgeSpec {
        members: members.iter().map(|&n| NodeId(n)).collect(),
        children: children
            .iter()
            .map(|&n| ChildRef {
                target: NodeId(n),
                role: None,
            })
            .collect(),
        relations: relations.iter().cloned().collect(),
    }
}

fn rel(t: &str, src: u64, dst: u64) -> Relation {
    Relation {
        rel_type: t.to_string(),
        src: NodeId(src),
        dst: NodeId(dst),
        role: None,
    }
}

fn set(ns: &[NodeId]) -> BTreeSet<NodeId> {
    ns.iter().copied().collect()
}

#[test]
fn construction_canonique_et_resolution() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);

    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let c = db.create_node(&["fn"]);
    let d = db.create_node(&["fn"]);

    let ab = comp.construct(&spec(&[a.0, b.0], &[], &[]));
    let ab_again = comp.construct(&spec(&[a.0, b.0], &[], &[]));
    assert_eq!(ab, ab_again, "déduplication canonique");

    let abc = comp.construct(&spec(&[c.0], &[ab.0], &[]));
    let abd = comp.construct(&spec(&[d.0], &[ab.0], &[]));
    assert_ne!(abc, abd, "spécifications distinctes → nœuds distincts");

    assert_eq!(
        comp.resolve(abc).unwrap(),
        set(&[a, b, c]),
        "resolve(ABC)={{A,B,C}}"
    );
    assert_eq!(
        comp.resolve(abd).unwrap(),
        set(&[a, b, d]),
        "resolve(ABD)={{A,B,D}}"
    );

    assert_eq!(
        comp.structural_key_of(ab),
        Some(spec(&[a.0, b.0], &[], &[]).structural_key()),
        "clé structurelle conservée"
    );
    db.close().unwrap();
}

#[test]
fn relations_orientes_distinguent_l_identite() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);

    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    let spec_ab = spec(&[a.0, b.0], &[], &[rel("calls", a.0, b.0)]);
    let spec_ba = spec(&[a.0, b.0], &[], &[rel("calls", b.0, a.0)]);
    let k_ab = comp.construct(&spec_ab);
    let k_ba = comp.construct(&spec_ba);
    assert_ne!(
        k_ab, k_ba,
        "A→B ≠ B→A : l'orientation fait partie de l'identité"
    );
    assert_eq!(comp.construct(&spec_ab), k_ab, "même relation → même nœud");
    assert_eq!(
        spec_ab.projection_members_key(),
        spec_ba.projection_members_key(),
        "équivalence selon la projection membres-seuls"
    );
    db.close().unwrap();
}

#[test]
fn encodage_structurel_injectif() {
    let a = NodeId(1);
    let b = NodeId(2);

    // None vs Some("") : spécifications Rust distinctes → clés distinctes.
    let r_none = Relation {
        rel_type: "calls".into(),
        src: a,
        dst: b,
        role: None,
    };
    let r_empty = Relation {
        rel_type: "calls".into(),
        src: a,
        dst: b,
        role: Some(String::new()),
    };
    let s_none = spec(&[1, 2], &[], &[r_none]);
    let s_empty = spec(&[1, 2], &[], &[r_empty]);
    assert_ne!(
        s_none.structural_key(),
        s_empty.structural_key(),
        "None ≠ Some(\"\")"
    );

    // Séparateurs dans les chaînes : pas de collision entre une chaîne contenant
    // « -> » / « # » et une combinaison de champs voisins.
    let r_sep = Relation {
        rel_type: "a->b#c".into(),
        src: a,
        dst: b,
        role: None,
    };
    let r_two = Relation {
        rel_type: "a".into(),
        src: a,
        dst: b,
        role: Some("b#c".into()),
    };
    let s_sep = spec(&[1, 2], &[], &[r_sep]);
    let s_two = spec(&[1, 2], &[], &[r_two]);
    assert_ne!(
        s_sep.structural_key(),
        s_two.structural_key(),
        "séparateurs sans collision"
    );
}

/// Diamant discriminant : une racine K référence ABC et ABD, qui référencent
/// tous deux AB. Un seul resolve(K) ; le partage de AB n'est PAS un cycle.
#[test]
fn diamant_partage_non_cycle() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);

    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let c = db.create_node(&["fn"]);
    let d = db.create_node(&["fn"]);

    let ab = comp.construct(&spec(&[a.0, b.0], &[], &[]));
    let abc = comp.construct(&spec(&[c.0], &[ab.0], &[]));
    let abd = comp.construct(&spec(&[d.0], &[ab.0], &[]));
    let k = comp.construct(&spec(&[], &[abc.0, abd.0], &[]));

    let resolved = comp.resolve(k).expect("diamant résolu sans cycle");
    assert_eq!(resolved, set(&[a, b, c, d]), "resolve(K)={{A,B,C,D}}");
    db.close().unwrap();
}

#[test]
fn resolution_valide_racine_et_references() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);

    let a = db.create_node(&["fn"]);

    // Racine inexistante.
    assert_eq!(
        comp.resolve(NodeId(9999)),
        Err(ResolveError::RootMissing(NodeId(9999)))
    );

    // Racine de type invalide (participant, pas connaissance).
    assert_eq!(comp.resolve(a), Err(ResolveError::RootNotKnowledge(a)));

    // Enfant existant mais non-connaissance.
    let k1 = db.create_node(&["structure"]);
    db.create_edge(k1, a, "child");
    assert_eq!(comp.resolve(k1), Err(ResolveError::NotKnowledge(a)));

    // Participant absent (member vers nœud inexistant).
    let k2 = db.create_node(&["structure"]);
    db.create_edge(k2, NodeId(777), "member");
    assert_eq!(
        comp.resolve(k2),
        Err(ResolveError::MissingParticipant(NodeId(777)))
    );

    // Référence absente (child vers nœud inexistant).
    let k3 = db.create_node(&["structure"]);
    db.create_edge(k3, NodeId(888), "child");
    assert_eq!(
        comp.resolve(k3),
        Err(ResolveError::MissingReference(NodeId(888)))
    );

    // Cycle indirect : k4 → k5 → k6 → k4.
    let k4 = db.create_node(&["structure"]);
    let k5 = db.create_node(&["structure"]);
    let k6 = db.create_node(&["structure"]);
    db.create_edge(k4, k5, "child");
    db.create_edge(k5, k6, "child");
    db.create_edge(k6, k4, "child");
    assert!(
        matches!(comp.resolve(k4), Err(ResolveError::Cycle(_))),
        "cycle indirect détecté"
    );

    // Cycle direct.
    let k7 = db.create_node(&["structure"]);
    let k8 = db.create_node(&["structure"]);
    db.create_edge(k7, k8, "child");
    db.create_edge(k8, k7, "child");
    assert!(
        matches!(comp.resolve(k7), Err(ResolveError::Cycle(_))),
        "cycle direct détecté"
    );
    db.close().unwrap();
}
