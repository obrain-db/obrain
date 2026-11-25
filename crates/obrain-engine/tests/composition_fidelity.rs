//! Contre-épreuve de fidélité de la composition : perte de direction, type,
//! rôle et imbrication dans les projections (membres résolus / membres directs).

use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;
use obrain_engine::composition::{ChildRef, Composition, KnowledgeSpec, Relation};

fn rel(t: &str, src: NodeId, dst: NodeId, role: Option<&str>) -> Relation {
    Relation {
        rel_type: t.to_string(),
        src,
        dst,
        role: role.map(|s| s.to_string()),
    }
}

#[test]
fn perte_de_projection_composition() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let db = ObrainDB::open_substrate(&path).unwrap();
    let comp = Composition::new(&db);
    let a = db.create_node(&["fn"]);
    let b = db.create_node(&["fn"]);

    // Chaque sous-connaissance et chaque relation est UNE occurrence
    // (multiplicité 1) : la spec porte la cardinalité, pas l'identité.
    let mk = |members: &[NodeId], children: &[NodeId], rels: &[Relation]| KnowledgeSpec {
        members: members.iter().copied().collect(),
        children: children
            .iter()
            .map(|c| {
                (
                    ChildRef {
                        target: *c,
                        role: None,
                    },
                    1,
                )
            })
            .collect(),
        relations: rels.iter().map(|r| (r.clone(), 1)).collect(),
    };

    // 1. Direction différente, mêmes membres.
    let s_ab = mk(&[a, b], &[], &[rel("calls", a, b, None)]);
    let s_ba = mk(&[a, b], &[], &[rel("calls", b, a, None)]);
    println!(
        "direction: structural {} vs {} | projection {}",
        s_ab.structural_key(),
        s_ba.structural_key(),
        s_ab.projection_members_key()
    );
    assert_ne!(
        s_ab.structural_key(),
        s_ba.structural_key(),
        "direction → identité distincte"
    );
    assert_eq!(
        s_ab.projection_members_key(),
        s_ba.projection_members_key(),
        "direction PERDUE en projection"
    );

    // 2. Type de relation différent, mêmes membres + direction.
    let s_calls = mk(&[a, b], &[], &[rel("calls", a, b, None)]);
    let s_knows = mk(&[a, b], &[], &[rel("knows", a, b, None)]);
    assert_ne!(
        s_calls.structural_key(),
        s_knows.structural_key(),
        "type → identité distincte"
    );
    assert_eq!(
        s_calls.projection_members_key(),
        s_knows.projection_members_key(),
        "type PERDU en projection"
    );

    // 3. Rôle différent, mêmes membres/type/direction.
    let s_r1 = mk(&[a, b], &[], &[rel("calls", a, b, Some("subject"))]);
    let s_r2 = mk(&[a, b], &[], &[rel("calls", a, b, Some("object"))]);
    assert_ne!(
        s_r1.structural_key(),
        s_r2.structural_key(),
        "rôle → identité distincte"
    );
    assert_eq!(
        s_r1.projection_members_key(),
        s_r2.projection_members_key(),
        "rôle PERDU en projection"
    );

    // 4. Imbrication différente, mêmes membres résolus.
    let bb = comp
        .construct(&mk(&[b], &[], &[]))
        .expect("construction de b");
    let flat = comp
        .construct(&mk(&[a, b], &[], &[]))
        .expect("construction plate");
    let nested = comp
        .construct(&mk(&[a], &[bb], &[]))
        .expect("construction imbriquée");
    assert_ne!(flat, nested, "imbrication → identité distincte");
    assert_eq!(
        comp.resolve(flat).unwrap(),
        comp.resolve(nested).unwrap(),
        "imbrication PERDUE en resolve (mêmes membres résolus)"
    );

    // Réouverture : reconstruction des structures, relations et références.
    db.close().unwrap();
    let db2 = ObrainDB::open_substrate(&path).unwrap();
    let comp2 = Composition::new(&db2);
    assert_eq!(
        comp2.resolve(flat).unwrap(),
        comp2.resolve(nested).unwrap(),
        "résolution reconstruite après réouverture"
    );
    assert_eq!(
        comp2.structural_key_of(flat),
        Some(mk(&[a, b], &[], &[]).structural_key()),
        "clé structurelle conservée"
    );
    db2.close().unwrap();
}
