//! Portée relationnelle + isolation de mutation. Deux compléments au test
//! composition_graph_native :
//!  1. la mutation d'un contexte n'affecte pas l'autre connaissance, la
//!     suppression d'ABD survit à la réouverture, et la référence child
//!     restante est vérifiée par src/dst/type (pas seulement son existence).
//!  2. contre-exemple : K1 (A CALLS B) et K2 (B CALLS A) ont les mêmes
//!     membres {A,B} mais des relations orientées DISTINCTES, chacune
//!     explicitement possédée (propriété owned_by) : elles restent
//!     distinguables, et une relation ajoutée au graphe global ne change
//!     pas leur identité.

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

fn conf(db: &ObrainDB, id: obrain_common::types::NodeId) -> f64 {
    db.get_node(id)
        .unwrap()
        .properties
        .get(&PropertyKey::from("confidence"))
        .and_then(|v| v.as_float64())
        .unwrap()
}

#[test]
fn isolation_de_mutation_et_suppression() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (ab, abc, abd, child_abc);
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let a = db.create_node(&["fn"]);
        let b = db.create_node(&["fn"]);
        let c = db.create_node(&["fn"]);
        let d = db.create_node(&["fn"]);
        ab = db.create_node(&["structure"]);
        abc = db.create_node(&["structure"]);
        abd = db.create_node(&["structure"]);
        db.create_edge(ab, a, "member");
        db.create_edge(ab, b, "member");
        child_abc = db.create_edge(abc, ab, "child");
        let child_abd = db.create_edge(abd, ab, "child");
        db.create_edge(abc, c, "member");
        db.create_edge(abd, d, "member");
        db.set_node_property(abc, "confidence", Value::Float64(0.9));
        db.set_node_property(abd, "confidence", Value::Float64(0.7));

        // MUTATION du contexte d'ABC : ABD doit garder sa valeur.
        db.set_node_property(abc, "confidence", Value::Float64(0.95));
        assert!((conf(&db, abc) - 0.95).abs() < 1e-9);
        assert!(
            (conf(&db, abd) - 0.7).abs() < 1e-9,
            "ABD isolé de la mutation du contexte d'ABC"
        );

        // SUPPRESSION d'ABD (arête child + nœud).
        assert!(db.delete_edge(child_abd), "child_abd supprimée");
        assert!(db.delete_node(abd), "ABD supprimé");
        db.close().unwrap();
    }
    // Réouverture : AB et ABC présents, la référence child ABC->AB est
    // valide (src/dst/type), ABD durablement absent.
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        assert!(db.get_node(ab).is_some(), "AB présent");
        assert!(db.get_node(abc).is_some(), "ABC présent");
        assert!(db.get_node(abd).is_none(), "ABD durablement supprimé");

        let e = db.get_edge(child_abc).expect("référence child ABC->AB");
        assert_eq!(e.src, abc, "child.src == ABC");
        assert_eq!(e.dst, ab, "child.dst == AB");
        assert_eq!(e.edge_type.as_str(), "child", "child.type == child");

        assert!(
            (conf(&db, abc) - 0.95).abs() < 1e-9,
            "contexte ABC conservé après réouverture"
        );
        db.close().unwrap();
    }
}

#[test]
fn contre_exemple_relations_inverses() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (a, b, k1, k2, e1, e2);
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        a = db.create_node(&["fn"]);
        b = db.create_node(&["fn"]);
        k1 = db.create_node(&["knowledge"]);
        k2 = db.create_node(&["knowledge"]);

        // K1 possède A CALLS B ; K2 possède B CALLS A. Mêmes membres, relations
        // inversées, chacune explicitement possédée.
        e1 = db.create_edge(a, b, "calls");
        e2 = db.create_edge(b, a, "calls");
        db.set_edge_property(e1, "owned_by", Value::Int64(k1.0 as i64));
        db.set_edge_property(e2, "owned_by", Value::Int64(k2.0 as i64));

        // Une relation AJOUTÉE au graphe global (non possédée par K1 ni K2)
        // ne doit pas modifier leur identité.
        db.create_edge(a, b, "cooperates");
        db.close().unwrap();
    }
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let r1 = db.get_edge(e1).expect("relation K1 présente");
        let r2 = db.get_edge(e2).expect("relation K2 présente");

        // Orientations distinctes → K1 et K2 distinguables.
        assert_eq!(r1.src, a);
        assert_eq!(r1.dst, b);
        assert_eq!(r2.src, b);
        assert_eq!(r2.dst, a);
        assert_eq!(r1.edge_type.as_str(), "calls");
        assert_eq!(r2.edge_type.as_str(), "calls");

        // Chacune possède SA relation : la possession est explicite et distincte.
        let owner = |e| {
            db.get_edge(e)
                .unwrap()
                .properties
                .get(&PropertyKey::from("owned_by"))
                .and_then(|v| v.as_int64())
                .unwrap()
        };
        assert_eq!(owner(e1), k1.0 as i64, "K1 possède A->B");
        assert_eq!(owner(e2), k2.0 as i64, "K2 possède B->A");
        assert_ne!(owner(e1), owner(e2), "possessions distinctes");
        db.close().unwrap();
    }
}
