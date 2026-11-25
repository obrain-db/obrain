//! La primitive compositionnelle native (Composition::construct / resolve),
//! exercée sur AB / ABC / ABD + relations orientées + détection de cycles.

use std::collections::BTreeSet;

use obrain_common::types::NodeId;
use obrain_engine::composition::{Composition, KnowledgeSpec, Relation, ResolveError};
use obrain_engine::ObrainDB;

fn spec(members: &[u64], children: &[u64], relations: &[Relation]) -> KnowledgeSpec {
    KnowledgeSpec {
        members: members.iter().map(|&n| NodeId(n)).collect(),
        children: children.iter().map(|&n| NodeId(n)).collect(),
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
    let db = ObrainDB::open_substrate(&td.path().join("kb")).unwrap();
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

    // Résolution transitive : AB référencé par ABC ET ABD = diamant légitime.
    assert_eq!(comp.resolve(abc).unwrap(), set(&[a, b, c]), "resolve(ABC)={{A,B,C}}");
    assert_eq!(comp.resolve(abd).unwrap(), set(&[a, b, d]), "resolve(ABD)={{A,B,D}}");
    assert_eq!(comp.resolve(ab).unwrap(), set(&[a, b]), "resolve(AB)={{A,B}}");

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
    let db = ObrainDB::open_substrate(&td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);

    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    // Mêmes membres {A,B}, relations inversées : identités structurelles distinctes.
    let spec_ab = spec(&[a.0, b.0], &[], &[rel("calls", a.0, b.0)]);
    let spec_ba = spec(&[a.0, b.0], &[], &[rel("calls", b.0, a.0)]);
    let k_ab = comp.construct(&spec_ab);
    let k_ba = comp.construct(&spec_ba);
    assert_ne!(k_ab, k_ba, "A→B ≠ B→A : l'orientation fait partie de l'identité");

    // Même relation → déduplication.
    assert_eq!(comp.construct(&spec_ab), k_ab, "même relation → même nœud");

    // Projection « membres seuls » : les deux partagent la même clé de projection.
    assert_eq!(
        spec_ab.projection_members_key(),
        spec_ba.projection_members_key(),
        "équivalence selon la projection membres-seuls"
    );
    db.close().unwrap();
}

#[test]
fn resolution_signal_cycle_et_reference_absente() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(&td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);

    // Cycle de références interdit.
    let k1 = db.create_node(&["structure"]);
    let k2 = db.create_node(&["structure"]);
    db.create_edge(k1, k2, "child");
    db.create_edge(k2, k1, "child");
    assert!(
        matches!(comp.resolve(k1), Err(ResolveError::Cycle(_))),
        "cycle de références détecté"
    );

    // Référence absente : child vers un nœud inexistant.
    let k3 = db.create_node(&["structure"]);
    db.create_edge(k3, NodeId(999), "child");
    assert_eq!(
        comp.resolve(k3),
        Err(ResolveError::MissingReference(NodeId(999))),
        "référence absente signalée"
    );
    db.close().unwrap();
}
