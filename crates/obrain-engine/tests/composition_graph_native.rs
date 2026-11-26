//! Composition au niveau GRAPHE natif (sans modification du format disque) :
//! AB est un nœud identifiable, référencé par ABC et ABD via une arête typée
//! « child », avec une relation orientée A--calls-->B et des contextes
//! indépendants (propriétés). Vérifie la conservation après fermeture/réouverture.

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

#[test]
fn composition_graphe_natif_fermeture_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (ab, abc, abd, a, b, calls_ab, child_abc, child_abd);
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        a = db.create_node(&["fn"]);
        b = db.create_node(&["fn"]);
        let c = db.create_node(&["fn"]);
        let d = db.create_node(&["fn"]);

        // AB : nœud structure, membres A/B, relation orientée A--calls-->B.
        ab = db.create_node(&["structure"]);
        db.create_edge(ab, a, "member");
        db.create_edge(ab, b, "member");
        calls_ab = db.create_edge(a, b, "calls");

        // ABC : référence AB (child) + membre C + relation B->C + contexte.
        abc = db.create_node(&["structure"]);
        child_abc = db.create_edge(abc, ab, "child");
        db.create_edge(abc, c, "member");
        db.create_edge(b, c, "calls");
        db.set_node_property(abc, "confidence", Value::Float64(0.9));

        // ABD : référence AB + membre D + contexte (indépendant).
        abd = db.create_node(&["structure"]);
        child_abd = db.create_edge(abd, ab, "child");
        db.create_edge(abd, d, "member");
        db.set_node_property(abd, "confidence", Value::Float64(0.7));

        db.close().unwrap();
    }
    // Réouverture réelle : identités, références, relation orientée, contextes.
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        // AB identifiable + présent.
        assert!(db.get_node(ab).is_some(), "AB présent après réouverture");
        // Références enfants conservées.
        assert!(
            db.get_edge(child_abc).is_some(),
            "référence ABC->AB conservée"
        );
        assert!(
            db.get_edge(child_abd).is_some(),
            "référence ABD->AB conservée"
        );
        // Relation orientée conservée (src/dst).
        let e = db.get_edge(calls_ab).expect("arête calls présente");
        assert_eq!(e.src, a);
        assert_eq!(e.dst, b);
        assert_eq!(e.edge_type.as_str(), "calls");
        // Contextes indépendants conservés.
        let conf = |id| {
            db.get_node(id)
                .unwrap()
                .properties
                .get(&PropertyKey::from("confidence"))
                .and_then(|v| v.as_float64())
                .unwrap()
        };
        assert!((conf(abc) - 0.9).abs() < 1e-9, "contexte ABC conservé");
        assert!(
            (conf(abd) - 0.7).abs() < 1e-9,
            "contexte ABD conservé (indépendant)"
        );
        db.close().unwrap();
    }
}
