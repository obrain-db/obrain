//! Versionnement physique : une construction interrompue (canonical_key écrit,
//! arêtes absentes) est détectée — pas de réutilisation silencieuse. Les arêtes
//! flottantes / occurrences réifiées non rattachées ne sont pas qualifiées.

use std::collections::BTreeSet;

use obrain_common::types::{NodeId, Value};
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

#[test]
fn construction_interrompue_detectee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    let s = spec(&[a.0, b.0], &[], &[]);
    // Demi-né : canonical_key écrit, AUCUNE arête (construction interrompue).
    let half = db.create_node(&["structure"]);
    db.set_node_property(half, "canonical_key", Value::from(s.structural_key()));

    // construct(spec) retrouve la clé mais détecte l'incomplétude (clé
    // recalculée du graphe vide ≠ clé persistée) — pas de retour silencieux.
    assert_eq!(comp.construct(&s), Err(ResolveError::KeyDivergence));
    db.close().unwrap();
}

#[test]
fn occurrence_flottante_non_qualifiee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    // Occurrence relationnelle FLOTTANTE (jamais rattachée à une connaissance).
    let float = db.create_node(&["relation_occurrence"]);
    db.set_node_property(float, "rel_type", Value::from("appelle"));
    db.create_edge(float, a, "src");
    db.create_edge(float, b, "dst");

    // Arête typée NUE (relation « calls » entre participants, non réifiée).
    db.create_edge(a, b, "calls");

    // Connaissance K sans relation constitutive.
    let k = comp.construct(&spec(&[a.0, b.0], &[], &[])).unwrap();
    let r = comp.read_spec(k).unwrap();
    assert!(
        r.relations.is_empty(),
        "aucune occurrence flottante / arête nue qualifiée"
    );
    db.close().unwrap();
}
