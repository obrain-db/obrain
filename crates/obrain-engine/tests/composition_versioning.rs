//! Encodage (v2 original, v2 intermédiaire, v3), migration EXPLICITE, lecture
//! seule, et récupération PROUVÉE. `read_spec` ne mute jamais ; `abandon_incomplete`
//! est la SEULE voie de suppression et exige une preuve (état pending + non référencé).

use std::collections::{BTreeMap, BTreeSet};

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec, Relation, ResolveError};

fn spec(members: &[u64], children: &[u64], relations: &[Relation]) -> KnowledgeSpec {
    KnowledgeSpec {
        members: members.iter().map(|&n| NodeId(n)).collect(),
        children: children
            .iter()
            .map(|&n| {
                (
                    ChildRef {
                        target: NodeId(n),
                        role: None,
                    },
                    1,
                )
            })
            .collect(),
        relations: relations.iter().cloned().map(|r| (r, 1)).collect(),
    }
}

fn child(target: NodeId, role: Option<&str>) -> ChildRef {
    ChildRef {
        target,
        role: role.map(|s| s.to_string()),
    }
}

#[test]
fn migration_v2_original_reelle() {
    // Base à l'ANCIENNE représentation : child SANS propriété role, clé v2
    // originale (pas de rôle, pas de multiplicité).
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let ab = db.create_node(&["structure"]);
    db.create_edge(ab, a, "member");
    let s = spec(&[a.0], &[ab.0], &[]);
    let legacy = s.legacy_v2_original_key().expect("v2 original compatible");
    let k = db.create_node(&["structure"]);
    db.set_node_property(k, "canonical_key", Value::from(legacy.clone()));
    db.create_edge(k, a, "member");
    db.create_edge(k, ab, "child"); // PAS de propriété role
    // read_spec NE MUTE PAS.
    let r = comp.read_spec(k).expect("lecture legacy v2 original");
    assert_eq!(r.children.get(&child(ab, None)), Some(&1));
    assert_eq!(
        comp.structural_key_of(k).as_deref(),
        Some(legacy.as_str()),
        "read_spec n'a PAS migré la clé"
    );
    // Migration EXPLICITE.
    assert!(comp.migrate_encoding(k).expect("migrate"));
    assert!(comp.structural_key_of(k).unwrap().starts_with("v3|"));
    db.close().unwrap();
}

#[test]
fn migration_v2_intermediaire_reelle() {
    // Base intermédiaire (720e189) : child AVEC rôle, clé v2 sans multiplicité.
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let ab = db.create_node(&["structure"]);
    db.create_edge(ab, a, "member");
    let s = KnowledgeSpec {
        members: [a].into_iter().collect(),
        children: [(child(ab, Some("gauche")), 1)].into_iter().collect(),
        relations: BTreeMap::new(),
    };
    assert!(
        s.legacy_v2_original_key().is_none(),
        "rôle child -> pas v2 original"
    );
    let legacy = s
        .legacy_v2_intermediate_key()
        .expect("v2 intermédiaire compatible");
    let k = db.create_node(&["structure"]);
    db.set_node_property(k, "canonical_key", Value::from(legacy.clone()));
    db.create_edge(k, a, "member");
    let eid = db.create_edge(k, ab, "child");
    db.set_edge_property(eid, "role", Value::from("gauche"));
    let r = comp.read_spec(k).expect("lecture legacy v2 intermédiaire");
    assert_eq!(r.children.get(&child(ab, Some("gauche"))), Some(&1));
    assert!(comp.migrate_encoding(k).expect("migrate"));
    assert!(comp.structural_key_of(k).unwrap().starts_with("v3|"));
    db.close().unwrap();
}

#[test]
fn v3_courant_et_multiplicite_nulle() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let k = comp.construct(&spec(&[a.0], &[], &[])).unwrap();
    assert!(comp.structural_key_of(k).unwrap().starts_with("v3|"));
    assert!(
        !comp.migrate_encoding(k).unwrap(),
        "déjà v3 -> pas de réécriture"
    );
    let bad = KnowledgeSpec {
        members: BTreeSet::new(),
        children: [(child(a, None), 0)].into_iter().collect(),
        relations: BTreeMap::new(),
    };
    assert_eq!(comp.construct(&bad), Err(ResolveError::InvalidMultiplicity));
    db.close().unwrap();
}

#[test]
fn encodage_incompatible_refuse() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let k = comp.construct(&spec(&[a.0], &[], &[])).unwrap();
    db.set_node_property(k, "canonical_key", Value::from("v2|garbage"));
    // Ni v3, ni legacy compatible -> refus (pas de migration silencieuse).
    assert_eq!(comp.read_spec(k), Err(ResolveError::KeyDivergence));
    assert_eq!(comp.migrate_encoding(k), Err(ResolveError::KeyDivergence));
    db.close().unwrap();
}

#[test]
fn lecture_seule_migration_refusee() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let k;
    {
        let db = ObrainDB::open_substrate(&path).unwrap();
        let comp = Composition::new(&db);
        let a = db.create_node(&["fn"]);
        k = comp.construct(&spec(&[a.0], &[], &[])).unwrap();
        let legacy = spec(&[a.0], &[], &[]).legacy_v2_original_key().unwrap();
        db.set_node_property(k, "canonical_key", Value::from(legacy));
        db.close().unwrap();
    }
    {
        let ro = ObrainDB::open_read_only(&path).unwrap();
        let comp = Composition::new(&ro);
        comp.read_spec(k).expect("lecture legacy en lecture seule");
        assert_eq!(comp.migrate_encoding(k), Err(ResolveError::ReadOnly));
        let _ = ro.close();
    }
}

#[test]
fn abandon_prouve_et_non_suppression() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let s = spec(&[a.0], &[], &[]);
    // 1) Demi-né pending non référencé : construct SIGNALE, n'abandonne pas.
    let half = db.create_node(&["structure"]);
    db.set_node_property(half, "canonical_key", Value::from(s.structural_key()));
    db.set_node_property(half, "construction_state", Value::from("pending"));
    assert!(matches!(
        comp.construct(&s),
        Err(ResolveError::IncompleteConstruction(x)) if x == half
    ));
    assert!(db.get_node(half).is_some(), "PAS supprimé automatiquement");
    comp.abandon_incomplete(half).expect("abandon prouvé");
    assert!(db.get_node(half).is_none());
    // 2) Pending RÉFÉRENCÉE : abandon REFUSÉ, jamais supprimée.
    let pending2 = db.create_node(&["structure"]);
    db.set_node_property(pending2, "construction_state", Value::from("pending"));
    let parent = db.create_node(&["structure"]);
    db.create_edge(parent, pending2, "child");
    assert_eq!(
        comp.abandon_incomplete(pending2),
        Err(ResolveError::AbandonRefused(pending2))
    );
    assert!(
        db.get_node(pending2).is_some(),
        "référencée -> non supprimée"
    );
    // 3) Statut inconnu (aucun marqueur) : abandon REFUSÉ.
    let unknown = db.create_node(&["structure"]);
    assert_eq!(
        comp.abandon_incomplete(unknown),
        Err(ResolveError::AbandonRefused(unknown))
    );
    assert!(db.get_node(unknown).is_some());
    db.close().unwrap();
}

#[test]
fn occurrence_flottante_non_qualifiee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let float = db.create_node(&["relation_occurrence"]);
    db.set_node_property(float, "rel_type", Value::from("appelle"));
    db.create_edge(float, a, "src");
    db.create_edge(float, b, "dst");
    db.create_edge(a, b, "calls");
    let k = comp.construct(&spec(&[a.0, b.0], &[], &[])).unwrap();
    assert!(comp.read_spec(k).unwrap().relations.is_empty());
    db.close().unwrap();
}
