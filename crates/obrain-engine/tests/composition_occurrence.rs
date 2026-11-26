//! Identité d'occurrence : deux références vers la même sous-connaissance avec
//! des rôles différents sont DEUX occurrences distinctes (représentation
//! constitutive), tandis que la projection ensembliste les réduit à une cible.

use std::collections::BTreeSet;

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec};

fn child(target: u64, role: Option<&str>) -> ChildRef {
    ChildRef {
        target: NodeId(target),
        role: role.map(|s| s.to_string()),
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

        // Sous-connaissance AB (définition structurelle partageable).
        ab = comp.construct(&KnowledgeSpec {
            members: [a, b].iter().copied().collect(),
            children: BTreeSet::new(),
            relations: BTreeSet::new(),
        });

        // K référence AB DEUX fois, rôles différents : deux occurrences.
        let spec = KnowledgeSpec {
            members: BTreeSet::new(),
            children: [child(ab.0, Some("gauche")), child(ab.0, Some("droite"))]
                .into_iter()
                .collect(),
            relations: BTreeSet::new(),
        };
        k = comp.construct(&spec);

        // Représentation constitutive : DEUX identités d'occurrence.
        let r = comp.read_spec(k).expect("spec de K");
        assert_eq!(r.children.len(), 2, "deux occurrences distinctes");
        assert!(r.children.contains(&child(ab.0, Some("gauche"))));
        assert!(r.children.contains(&child(ab.0, Some("droite"))));
        // Projection ensembliste : réduite à UNE cible (perte documentée).
        assert_eq!(r.projection_children(), set(&[ab]), "projection = {{AB}}");
        db.close().unwrap();
    }
    // Réouverture : les DEUX identités d'occurrence survivent.
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        let r = comp.read_spec(k).expect("spec de K après réouverture");
        assert_eq!(r.children.len(), 2, "2 identités après réouverture");
        assert!(r.children.contains(&child(ab.0, Some("gauche"))));
        assert!(r.children.contains(&child(ab.0, Some("droite"))));
        assert_eq!(r.projection_children(), set(&[ab]), "projection = {{AB}}");
        // La résolution (projection transitive) aplatit AB en {A,B}.
        assert_eq!(
            comp.resolve(k).unwrap(),
            set(&[a, b]),
            "resolve(K) = {{A,B}}"
        );
        db.close().unwrap();
    }
}
