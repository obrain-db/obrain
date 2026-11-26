//! Version d'encodage et récupération. Une base créée par l'ANCIEN encodage
//! (clé v2) est migrée explicitement vers v3 ; une même définition n'est pas
//! dupliquée, deux définitions distinctes ne fusionnent pas. Les occurrences
//! relationnelles flottantes ne sont pas qualifiées.

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec, Relation};

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

#[test]
fn migration_legacy_v2_vers_v3_sans_duplication() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    // Connaissance valide (complète) construite en v3…
    let k = comp.construct(&spec(&[a.0, b.0], &[], &[])).unwrap();
    let v3 = comp.read_spec(k).unwrap().structural_key();
    let legacy = spec(&[a.0, b.0], &[], &[]).legacy_v2_key();
    // … puis forcée en encodage HISTORIQUE v2 (simule une base de l'ancien moteur).
    db.set_node_property(k, "canonical_key", Value::from(legacy.clone()));
    assert_eq!(comp.structural_key_of(k).as_deref(), Some(legacy.as_str()));

    // La première lecture MIGRE la clé en v3.
    let r = comp.read_spec(k).expect("structure legacy valide");
    assert_eq!(r.structural_key(), v3, "clé reconstruite en v3");
    assert_eq!(
        comp.structural_key_of(k).as_deref(),
        Some(v3.as_str()),
        "clé persistée migrée en v3"
    );

    // Une même définition n'est PAS dupliquée : construct retrouve le nœud migré.
    let again = comp.construct(&spec(&[a.0, b.0], &[], &[])).unwrap();
    assert_eq!(again, k, "pas de duplication après migration");
    db.close().unwrap();
}

#[test]
fn deux_definitions_distinctes_ne_fusionnent_pas() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    let c = db.create_node(&["fn"]);
    // Deux définitions DISTINCTES (membres différents), toutes deux en v2 legacy.
    let k1 = comp.construct(&spec(&[a.0, b.0], &[], &[])).unwrap();
    let k2 = comp.construct(&spec(&[a.0, c.0], &[], &[])).unwrap();
    db.set_node_property(
        k1,
        "canonical_key",
        Value::from(spec(&[a.0, b.0], &[], &[]).legacy_v2_key()),
    );
    db.set_node_property(
        k2,
        "canonical_key",
        Value::from(spec(&[a.0, c.0], &[], &[]).legacy_v2_key()),
    );
    let r1 = comp.read_spec(k1).unwrap();
    let r2 = comp.read_spec(k2).unwrap();
    assert_ne!(
        r1.structural_key(),
        r2.structural_key(),
        "clés v3 distinctes"
    );
    assert_ne!(k1, k2, "nœuds distincts — pas de fusion");
    db.close().unwrap();
}

#[test]
fn occurrence_flottante_non_qualifiee() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("kb")).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);
    // Occurrence relationnelle FLOTTANTE (jamais rattachée).
    let float = db.create_node(&["relation_occurrence"]);
    db.set_node_property(float, "rel_type", Value::from("appelle"));
    db.create_edge(float, a, "src");
    db.create_edge(float, b, "dst");
    // Arête typée NUE (non réifiée).
    db.create_edge(a, b, "calls");
    let k = comp.construct(&spec(&[a.0, b.0], &[], &[])).unwrap();
    let r = comp.read_spec(k).unwrap();
    assert!(
        r.relations.is_empty(),
        "aucune occurrence flottante / arête nue qualifiée"
    );
    db.close().unwrap();
}
