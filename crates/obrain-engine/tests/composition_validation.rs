//! Qualification de chaque validation relationnelle : chaque corruption ciblée
//! produit EXACTEMENT la variante typée attendue (pas une InvalidStructure
//! générique, pas une divergence de clé qui masquerait la vraie cause).

use obrain_common::types::{EdgeId, NodeId, Value};
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

fn rel(t: &str, src: u64, dst: u64) -> Relation {
    Relation {
        rel_type: t.to_string(),
        src: NodeId(src),
        dst: NodeId(dst),
        role: None,
    }
}

/// Construit une connaissance VALIDE (vraie clé) avec une occurrence
/// relationnelle A--appelle-->B. Retourne (k, occ, src_eid, dst_eid).
fn build_valid(db: &ObrainDB, a: NodeId, b: NodeId) -> (NodeId, NodeId, EdgeId, EdgeId) {
    let valid = spec(&[a.0, b.0], &[], &[rel("appelle", a.0, b.0)]);
    let k = db.create_node(&["structure"]);
    db.set_node_property(k, "canonical_key", Value::from(valid.structural_key()));
    db.create_edge(k, a, "member");
    db.create_edge(k, b, "member");
    let occ = db.create_node(&["relation_occurrence"]);
    db.set_node_property(occ, "rel_type", Value::from("appelle"));
    let src_eid = db.create_edge(occ, a, "src");
    let dst_eid = db.create_edge(occ, b, "dst");
    db.create_edge(k, occ, "relation");
    (k, occ, src_eid, dst_eid)
}

#[test]
fn multiples_destinations_rejetees() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let (k, occ, _, _) = build_valid(&db, a, b);
    comp.read_spec(k).expect("valide avant corruption");
    db.create_edge(occ, a, "dst"); // 2e destination
    assert_eq!(comp.read_spec(k), Err(ResolveError::MultipleDestinations));
    db.close().unwrap();
}

#[test]
fn source_manquante_rejetee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let (k, _, src_eid, _) = build_valid(&db, a, b);
    comp.read_spec(k).expect("valide avant corruption");
    db.delete_edge(src_eid);
    assert_eq!(comp.read_spec(k), Err(ResolveError::MissingSource));
    db.close().unwrap();
}

#[test]
fn destination_manquante_rejetee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let (k, _, _, dst_eid) = build_valid(&db, a, b);
    comp.read_spec(k).expect("valide avant corruption");
    db.delete_edge(dst_eid);
    assert_eq!(comp.read_spec(k), Err(ResolveError::MissingDestination));
    db.close().unwrap();
}

#[test]
fn mauvais_label_relation_rejete() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let (k, _, _, _) = build_valid(&db, a, b);
    comp.read_spec(k).expect("valide avant corruption");
    let plain = db.create_node(&["fn"]);
    db.create_edge(k, plain, "relation"); // relation -> nœud sans label relation_occurrence
    assert_eq!(comp.read_spec(k), Err(ResolveError::WrongRelationLabel));
    db.close().unwrap();
}

#[test]
fn type_relation_manquant_rejete() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let valid = spec(&[a.0, b.0], &[], &[rel("appelle", a.0, b.0)]);
    let k = db.create_node(&["structure"]);
    db.set_node_property(k, "canonical_key", Value::from(valid.structural_key()));
    db.create_edge(k, a, "member");
    db.create_edge(k, b, "member");
    let occ = db.create_node(&["relation_occurrence"]);
    // PAS de rel_type
    db.create_edge(occ, a, "src");
    db.create_edge(occ, b, "dst");
    db.create_edge(k, occ, "relation");
    assert_eq!(comp.read_spec(k), Err(ResolveError::MissingRelationType));
    db.close().unwrap();
}

#[test]
fn role_non_textuel_rejete() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let (k, occ, _, _) = build_valid(&db, a, b);
    comp.read_spec(k).expect("valide avant corruption");
    db.set_node_property(occ, "role", Value::Int64(5)); // rôle non textuel
    assert_eq!(comp.read_spec(k), Err(ResolveError::NonTextualRole));
    db.close().unwrap();
}

#[test]
fn divergence_de_cle_rejetee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let (k, _, _, _) = build_valid(&db, a, b);
    comp.read_spec(k).expect("valide avant corruption");
    db.set_node_property(k, "canonical_key", Value::from("v2|corrompu"));
    assert_eq!(comp.read_spec(k), Err(ResolveError::KeyDivergence));
    db.close().unwrap();
}
