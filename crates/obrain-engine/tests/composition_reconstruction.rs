//! Contre-épreuve de reconstruction relationnelle APRÈS réouverture : read_spec
//! reconstruit chaque définition constitutive (direction/type/rôle), sans
//! réutiliser les specs initiales, et ignore les relations extérieures.

use std::collections::BTreeSet;

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use obrain_engine::composition::{Composition, KnowledgeSpec, Relation, ResolveError};

fn spec(members: &[u64], children: &[u64], relations: &[Relation]) -> KnowledgeSpec {
    KnowledgeSpec {
        members: members.iter().map(|&n| NodeId(n)).collect(),
        children: children.iter().map(|&n| NodeId(n)).collect(),
        relations: relations.iter().cloned().collect(),
    }
}

fn rel(t: &str, src: u64, dst: u64, role: Option<&str>) -> Relation {
    Relation {
        rel_type: t.to_string(),
        src: NodeId(src),
        dst: NodeId(dst),
        role: role.map(|s| s.to_string()),
    }
}

#[test]
fn reconstruction_relationnelle_apres_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let db = ObrainDB::open_substrate(&path).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    // K1..K4 : mêmes participants, définitions relationnelles distinctes.
    let k1 = comp.construct(&spec(&[a.0, b.0], &[], &[rel("appelle", a.0, b.0, None)]));
    let k2 = comp.construct(&spec(&[a.0, b.0], &[], &[rel("contredit", a.0, b.0, None)]));
    let k3 = comp.construct(&spec(&[a.0, b.0], &[], &[rel("appelle", b.0, a.0, None)]));
    let k4 = comp.construct(&spec(
        &[a.0, b.0],
        &[],
        &[rel("appelle", a.0, b.0, Some("x"))],
    ));

    // Relation extérieure (arête flottante, non rattachée à une connaissance).
    db.create_edge(a, b, "externe");

    db.close().unwrap();
    let db2 = ObrainDB::open_substrate(&path).unwrap();
    let comp2 = Composition::new(&db2);

    // Reconstruction indépendante : les specs initiales ne servent pas de données.
    let r1 = comp2.read_spec(k1).expect("read_spec(k1)");
    let r2 = comp2.read_spec(k2).expect("read_spec(k2)");
    let r3 = comp2.read_spec(k3).expect("read_spec(k3)");
    let r4 = comp2.read_spec(k4).expect("read_spec(k4)");

    let only = |s: &KnowledgeSpec| s.relations.iter().next().unwrap().clone();
    let x1 = only(&r1);
    let x2 = only(&r2);
    let x3 = only(&r3);
    let x4 = only(&r4);

    // Type reconstruit.
    assert_eq!(x1.rel_type, "appelle", "K1 type");
    assert_eq!(x2.rel_type, "contredit", "K2 type distinct");
    // Direction reconstruite.
    assert_eq!((x1.src, x1.dst), (a, b), "K1 direction A->B");
    assert_eq!((x3.src, x3.dst), (b, a), "K3 direction B->A");
    // Rôle reconstruit.
    assert_eq!(x4.role.as_deref(), Some("x"), "K4 rôle");
    assert_eq!(x1.role, None, "K1 sans rôle");

    // Membres reconstruits.
    let bts = |n: &[NodeId]| n.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(r1.members, bts(&[a, b]), "K1 membres");

    // La relation extérieure n'appartient à AUCUNE connaissance reconstruite.
    for r in [&r1, &r2, &r3, &r4] {
        assert!(
            !r.relations.iter().any(|x| x.rel_type == "externe"),
            "relation extérieure exclue"
        );
    }
    db2.close().unwrap();
}

/// Corruption discriminante : une structure VALIDE (vraie clé canonique) est
/// lue OK, PUIS une 2e source est ajoutée à l'occurrence — l'erreur est
/// `MultipleSources` (multiplicité), PAS `KeyDivergence`. L'ancien test posait
/// une clé factice « x » et acceptait n'importe quelle InvalidStructure : il
/// pouvait passer pour la mauvaise raison (divergence de clé).
#[test]
fn corruption_relationnelle_discriminante() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let db = ObrainDB::open_substrate(&path).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    // Structure VALIDE : vraie clé canonique, arêtes constitutives complètes.
    let valid = spec(&[a.0, b.0], &[], &[rel("appelle", a.0, b.0, None)]);
    let k = db.create_node(&["structure"]);
    db.set_node_property(k, "canonical_key", Value::from(valid.structural_key()));
    db.create_edge(k, a, "member");
    db.create_edge(k, b, "member");
    let occ = db.create_node(&["relation_occurrence"]);
    db.set_node_property(occ, "rel_type", Value::from("appelle"));
    db.create_edge(occ, a, "src");
    db.create_edge(occ, b, "dst");
    db.create_edge(k, occ, "relation");

    // Valide : read_spec OK avant corruption.
    comp.read_spec(k).expect("structure valide lue avant corruption");

    // Corruption ciblée : une 2e source.
    db.create_edge(occ, b, "src");

    // L'erreur est la MULTIPLICITÉ, pas une divergence de clé.
    assert_eq!(comp.read_spec(k), Err(ResolveError::MultipleSources));
    db.close().unwrap();
}
