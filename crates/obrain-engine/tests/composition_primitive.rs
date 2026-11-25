//! La primitive compositionnelle native (`Composition::construct` / `resolve`),
//! exercée sur la structure AB / ABC / ABD. Spécification exécutable du
//! contrat d'identité (déduplication canonique) et de la résolution.

use std::collections::BTreeSet;

use obrain_common::types::NodeId;
use obrain_engine::composition::{Composition, KnowledgeSpec};
use obrain_engine::ObrainDB;

fn spec(members: &[u64], children: &[u64]) -> KnowledgeSpec {
    KnowledgeSpec {
        members: members.iter().map(|&n| NodeId(n)).collect(),
        children: children.iter().map(|&n| NodeId(n)).collect(),
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

    // AB = participants {A, B}.
    let ab = comp.construct(&spec(&[a.0, b.0], &[]));

    // Déduplication : reconstruire la MÊME spécification → même nœud.
    let ab_again = comp.construct(&spec(&[a.0, b.0], &[]));
    assert_eq!(ab, ab_again, "déduplication canonique : {ab:?} vs {ab_again:?}");

    // ABC = {C} + référence AB ; ABD = {D} + référence AB.
    let abc = comp.construct(&spec(&[c.0], &[ab.0]));
    let abd = comp.construct(&spec(&[d.0], &[ab.0]));
    assert_ne!(abc, abd, "spécifications distinctes → nœuds distincts");

    // Résolution : aplatit les références transitives.
    assert_eq!(comp.resolve(abc), set(&[a, b, c]), "resolve(ABC) = {{A,B,C}}");
    assert_eq!(comp.resolve(abd), set(&[a, b, d]), "resolve(ABD) = {{A,B,D}}");
    assert_eq!(comp.resolve(ab), set(&[a, b]), "resolve(AB) = {{A,B}}");

    // Clé canonique conservée.
    assert_eq!(comp.canonical_key_of(ab), Some(spec(&[a.0, b.0], &[]).canonical_key()));

    db.close().unwrap();
}
