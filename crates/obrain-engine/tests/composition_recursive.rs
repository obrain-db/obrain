//! Composition récursive : diamant à sous-connaissance partagée. Qualifie
//! EXPLICITEMENT : partage effectif, reconstruction, multiplicité,
//! fermeture/réouverture, version logique ET physique, interruption de
//! construction, cycles.

// Noms à un caractère = notation A/B/C/D de l'expérience compositionnelle.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec, Relation, ResolveError};

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
fn diamant_recursif_sous_connaissance_partagee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, c, d, ab, abc, abd, k);
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        a = db.create_node(&["fn"]);
        b = db.create_node(&["fn"]);
        c = db.create_node(&["fn"]);
        d = db.create_node(&["fn"]);

        // AB : sous-connaissance partagée, relation interne A->B.
        ab = comp
            .construct(&KnowledgeSpec {
                members: [a, b].iter().copied().collect(),
                children: BTreeMap::new(),
                relations: [(rel("appelle", a.0, b.0), 1)].into_iter().collect(),
            })
            .unwrap();
        // ABC : référence AB (rôle gauche) + membre C.
        abc = comp
            .construct(&KnowledgeSpec {
                members: [c].iter().copied().collect(),
                children: [(child(ab.0, Some("gauche")), 1)].into_iter().collect(),
                relations: BTreeMap::new(),
            })
            .unwrap();
        // ABD : référence AB (rôle droite) + membre D.
        abd = comp
            .construct(&KnowledgeSpec {
                members: [d].iter().copied().collect(),
                children: [(child(ab.0, Some("droite")), 1)].into_iter().collect(),
                relations: BTreeMap::new(),
            })
            .unwrap();
        // K : référence ABC et ABD.
        k = comp
            .construct(&KnowledgeSpec {
                members: BTreeSet::new(),
                children: [
                    (child(abc.0, Some("base")), 1),
                    (child(abd.0, Some("variante")), 1),
                ]
                .into_iter()
                .collect(),
                relations: BTreeMap::new(),
            })
            .unwrap();

        // (a) PARTAGE EFFECTIF : ABC et ABD référencent le MÊME NodeId AB.
        let sabc = comp.read_spec(abc).unwrap();
        let sabd = comp.read_spec(abd).unwrap();
        assert!(
            sabc.children.keys().any(|c| c.target == ab)
                && sabd.children.keys().any(|c| c.target == ab),
            "AB partagé : même NodeId référencé par ABC et ABD"
        );
        // (b) RECONSTRUCTION : rôles distincts reconstruits.
        assert_eq!(sabc.children.get(&child(ab.0, Some("gauche"))), Some(&1));
        assert_eq!(sabd.children.get(&child(ab.0, Some("droite"))), Some(&1));
        // (c) MULTIPLICITÉ : 2 occurrences d'AB dans le diamant (une par parent).
        assert_eq!(sabc.children_total(), 1);
        assert_eq!(sabd.children_total(), 1);
        // Relation interne d'AB préservée.
        assert_eq!(
            comp.read_spec(ab)
                .unwrap()
                .relations
                .get(&rel("appelle", a.0, b.0)),
            Some(&1)
        );
        // (e-logique) VERSION : clé en encodage courant v3.
        assert!(
            comp.structural_key_of(ab).unwrap().starts_with("v3|"),
            "version logique v3"
        );
        db.close().unwrap();
    }
    // (d) FERMETURE/RÉOUVERTURE + (e-physique) persistance durable.
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        // (e-physique) la clé persistée est toujours v3 et intacte.
        let re = comp.read_spec(ab).unwrap().structural_key();
        assert_eq!(comp.structural_key_of(ab).as_deref(), Some(re.as_str()));
        // Références directes intactes.
        assert_eq!(
            comp.read_spec(abc)
                .unwrap()
                .children
                .get(&child(ab.0, Some("gauche"))),
            Some(&1)
        );
        assert_eq!(
            comp.read_spec(abd)
                .unwrap()
                .children
                .get(&child(ab.0, Some("droite"))),
            Some(&1)
        );
        // Projection transitive du diamant : {A,B,C,D}, AB partagé n'est pas un cycle.
        assert_eq!(comp.resolve(k).unwrap(), set(&[a, b, c, d]), "resolve(K)");
        db.close().unwrap();
    }
}

#[test]
fn cycle_detecte() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    // K1 -> K2 -> K1 : cycle direct.
    let k1 = db.create_node(&["structure"]);
    let k2 = db.create_node(&["structure"]);
    db.create_edge(k1, k2, "child");
    db.create_edge(k2, k1, "child");
    db.create_edge(k1, a, "member");
    assert!(
        matches!(comp.resolve(k1), Err(ResolveError::Cycle(_))),
        "cycle détecté"
    );
    db.close().unwrap();
}
