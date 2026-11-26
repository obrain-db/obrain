//! Identité d'occurrence et MULTIPLICITÉ, indépendamment de l'identité de
//! définition. Deux occurrences d'une même cible — rôles différents OU même
//! rôle — sont deux occurrences : la représentation constitutive les préserve,
//! la projection ensembliste les réduit à une cible (perte documentée).

use std::collections::{BTreeMap, BTreeSet};

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec, Relation};

fn child(target: u64, role: Option<&str>) -> ChildRef {
    ChildRef {
        target: NodeId(target),
        role: role.map(|s| s.to_string()),
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
fn deux_occurrences_meme_cible_roles_distincts() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, ab, k);
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        a = db.create_node(&["fn"]);
        b = db.create_node(&["fn"]);
        ab = comp
            .construct(&KnowledgeSpec {
                members: [a, b].iter().copied().collect(),
                children: BTreeMap::new(),
                relations: BTreeMap::new(),
            })
            .unwrap();
        // K référence AB DEUX fois, rôles différents.
        k = comp
            .construct(&KnowledgeSpec {
                members: BTreeSet::new(),
                children: [
                    (child(ab.0, Some("gauche")), 1),
                    (child(ab.0, Some("droite")), 1),
                ]
                .into_iter()
                .collect(),
                relations: BTreeMap::new(),
            })
            .unwrap();
        let r = comp.read_spec(k).expect("spec de K");
        assert_eq!(r.children.len(), 2, "deux occurrences distinctes");
        assert_eq!(r.children.get(&child(ab.0, Some("gauche"))), Some(&1));
        assert_eq!(r.children.get(&child(ab.0, Some("droite"))), Some(&1));
        assert_eq!(r.projection_children(), set(&[ab]), "projection = AB");
        db.close().unwrap();
    }
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        let r = comp.read_spec(k).expect("spec de K après réouverture");
        assert_eq!(r.children.len(), 2, "2 occurrences après réouverture");
        assert_eq!(r.projection_children(), set(&[ab]), "projection = AB");
        db.close().unwrap();
    }
}

#[test]
fn deux_occurrences_meme_cible_meme_role() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, ab, k);
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        a = db.create_node(&["fn"]);
        b = db.create_node(&["fn"]);
        ab = comp
            .construct(&KnowledgeSpec {
                members: [a, b].iter().copied().collect(),
                children: BTreeMap::new(),
                relations: BTreeMap::new(),
            })
            .unwrap();
        // K référence AB DEUX fois avec le MÊME rôle : multiplicité 2.
        k = comp
            .construct(&KnowledgeSpec {
                members: BTreeSet::new(),
                children: [(child(ab.0, Some("gauche")), 2)].into_iter().collect(),
                relations: BTreeMap::new(),
            })
            .unwrap();
        let r = comp.read_spec(k).expect("spec de K");
        assert_eq!(
            r.children.get(&child(ab.0, Some("gauche"))),
            Some(&2),
            "multiplicité 2 (même rôle, ne fusionne pas)"
        );
        assert_eq!(r.children_total(), 2, "2 occurrences constitutives");
        assert_eq!(r.projection_children(), set(&[ab]), "projection = AB");
        db.close().unwrap();
    }
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        let r = comp.read_spec(k).expect("spec de K après réouverture");
        assert_eq!(
            r.children.get(&child(ab.0, Some("gauche"))),
            Some(&2),
            "multiplicité 2 préservée après réouverture"
        );
        assert_eq!(r.children_total(), 2);
        assert_eq!(r.projection_children(), set(&[ab]), "projection = AB");
        db.close().unwrap();
    }
}

#[test]
fn relations_multiplicite() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let r0 = rel("appelle", a.0, b.0);
    // Deux relations IDENTIQUES : multiplicité 2, pas une fusion.
    let k = comp
        .construct(&KnowledgeSpec {
            members: [a, b].iter().copied().collect(),
            children: BTreeMap::new(),
            relations: [(r0.clone(), 2)].into_iter().collect(),
        })
        .unwrap();
    let r = comp.read_spec(k).expect("spec de K");
    assert_eq!(
        r.relations.get(&r0),
        Some(&2),
        "2 occurrences relationnelles"
    );
    assert_eq!(r.relations_total(), 2, "relations_total == 2");
    db.close().unwrap();
}
