//! Composition récursive : diamant à sous-connaissance partagée, rôles
//! contextuels et relation interne. Vérifie les références DIRECTES
//! (identité + multiplicité) PUIS la projection transitive (resolve).

// Noms à un caractère = notation A/B/C/D de l'expérience compositionnelle.
#![allow(clippy::many_single_char_names)]

use std::collections::BTreeSet;

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
                children: BTreeSet::new(),
                relations: [rel("appelle", a.0, b.0)].into_iter().collect(),
            })
            .unwrap();

        // ABC : référence AB (rôle gauche) + membre C.
        abc = comp
            .construct(&KnowledgeSpec {
                members: [c].iter().copied().collect(),
                children: [child(ab.0, Some("gauche"))].into_iter().collect(),
                relations: BTreeSet::new(),
            })
            .unwrap();

        // ABD : référence AB (rôle droite) + membre D.
        abd = comp
            .construct(&KnowledgeSpec {
                members: [d].iter().copied().collect(),
                children: [child(ab.0, Some("droite"))].into_iter().collect(),
                relations: BTreeSet::new(),
            })
            .unwrap();

        // K : référence ABC et ABD (rôles contextuels).
        k = comp
            .construct(&KnowledgeSpec {
                members: BTreeSet::new(),
                children: [child(abc.0, Some("base")), child(abd.0, Some("variante"))]
                    .into_iter()
                    .collect(),
                relations: BTreeSet::new(),
            })
            .unwrap();

        // --- Références DIRECTES (identité + multiplicité) ---
        // ABC et ABD référencent AB avec des rôles DISTINCTS : deux occurrences
        // différentes de la même définition AB.
        let sabc = comp.read_spec(abc).unwrap();
        assert_eq!(
            sabc.children,
            [child(ab.0, Some("gauche"))]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "ABC -> AB (gauche)"
        );
        let sabd = comp.read_spec(abd).unwrap();
        assert_eq!(
            sabd.children,
            [child(ab.0, Some("droite"))]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "ABD -> AB (droite)"
        );
        assert_eq!(sabc.projection_children(), set(&[ab]));
        assert_eq!(sabd.projection_children(), set(&[ab]));

        // K référence ABC et ABD avec des rôles distincts.
        let sk = comp.read_spec(k).unwrap();
        assert_eq!(
            sk.children,
            [child(abc.0, Some("base")), child(abd.0, Some("variante"))]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "K -> {{ABC (base), ABD (variante)}}"
        );

        // Relation interne d'AB préservée.
        let sab = comp.read_spec(ab).unwrap();
        assert_eq!(
            sab.relations,
            [rel("appelle", a.0, b.0)]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "AB : relation interne A->B"
        );

        db.close().unwrap();
    }
    // --- Projection transitive (resolve) après réouverture ---
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        assert_eq!(
            comp.resolve(k).unwrap(),
            set(&[a, b, c, d]),
            "resolve(K) = {{A,B,C,D}}"
        );
        let sabc = comp.read_spec(abc).unwrap();
        assert_eq!(
            sabc.children,
            [child(ab.0, Some("gauche"))]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "ABC -> AB (gauche) après réouverture"
        );
        let sabd = comp.read_spec(abd).unwrap();
        assert_eq!(
            sabd.children,
            [child(ab.0, Some("droite"))]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "ABD -> AB (droite) après réouverture"
        );
        db.close().unwrap();
    }
}
