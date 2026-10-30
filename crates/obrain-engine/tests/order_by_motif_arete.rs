//! (2026-09-14) CONTRAT — semantique de ORDER BY sur motif avec arete.
//!
//! Enquete de production (runtime-dev, sessions_list variante owner) : des
//! "props NULL apres ORDER BY DESC" qui etaient en fait la semantique
//! standard NULL = plus grand (nulls PREMIERS en DESC, DERNIERS en ASC) :
//! des sessions techniques sans createdAt passaient en tete du top-k.
//! NON-BUG requalifie le 2026-09-15. Ces tests fixent le contrat :
//! 1) les props sont conservees apres ORDER BY (avec et sans LIMIT) ;
//! 2) la position des NULL est documentee et stable.
//! Consequence produit : un tri applicatif recent-d-abord doit rester en
//! Rust avec cle par defaut (unwrap_or(0) -> nulls derniers), comme dans
//! sessions_list.

use obrain_engine::ObrainDB;

fn base() -> (tempfile::TempDir, ObrainDB) {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(&td.path().join("kb-orderby-arete")).expect("open_substrate");
    {
        let s = db.session();
        s.execute("CREATE (:S {qualified_name: 's1', createdAt: 100})").unwrap();
        s.execute("CREATE (:S {qualified_name: 's2', createdAt: 300})").unwrap();
        s.execute("CREATE (:S {qualified_name: 's3', createdAt: 200})").unwrap();
        s.execute("CREATE (:A {qualified_name: 'acc'})").unwrap();
        for n in ["s1", "s2", "s3"] {
            s.execute(&format!(
                "MATCH (a:S {{qualified_name: '{n}'}}), (b:A {{qualified_name: 'acc'}}) MERGE (a)-[:OWNED_BY]->(b)"
            ))
            .unwrap();
        }
    }
    (td, db)
}

/// TEMOIN : sans ORDER BY, les props du motif avec arete sont servies.
#[test]
fn temoin_sans_order_by_props_presentes() {
    let (_td, db) = base();
    let s = db.session();
    let res = s
        .execute("MATCH (s:S)-[:OWNED_BY]->(a:A {qualified_name: 'acc'}) RETURN s.qualified_name, s.createdAt")
        .unwrap();
    assert_eq!(res.rows.len(), 3, "3 lignes attendues");
    for r in &res.rows {
        assert!(
            r.get(1).and_then(|v| v.as_int64()).is_some(),
            "temoin: createdAt doit etre non NULL, ligne {r:?}"
        );
    }
}

/// REPRO : avec ORDER BY, les props doivent rester servies ET triees.
#[test]
fn order_by_sur_motif_avec_arete_conserve_les_props() {
    let (_td, db) = base();
    let s = db.session();
    let res = s
        .execute("MATCH (s:S)-[:OWNED_BY]->(a:A {qualified_name: 'acc'}) RETURN s.qualified_name, s.createdAt ORDER BY s.createdAt DESC")
        .unwrap();
    assert_eq!(res.rows.len(), 3, "3 lignes attendues");
    let vals: Vec<Option<i64>> = res
        .rows
        .iter()
        .map(|r| r.get(1).and_then(|v| v.as_int64()))
        .collect();
    assert_eq!(
        vals,
        vec![Some(300), Some(200), Some(100)],
        "props apres ORDER BY (NULL = bug moteur) : {vals:?}"
    );
}

/// CONTRAT : NULL est traite comme la plus grande valeur — PREMIERS en DESC,
/// DERNIERS en ASC. C est la semantique constatee et adoptee (Cypher/Neo4j).
#[test]
fn order_by_position_des_null_documentee() {
    let (_td, db) = base();
    let s = db.session();
    // une session technique SANS createdAt, reliee au meme compte
    s.execute("CREATE (:S {qualified_name: 's0'})").unwrap();
    s.execute("MATCH (a:S {qualified_name: 's0'}), (b:A {qualified_name: 'acc'}) MERGE (a)-[:OWNED_BY]->(b)").unwrap();

    let desc = s.execute("MATCH (s:S)-[:OWNED_BY]->(a:A {qualified_name: 'acc'}) RETURN s.qualified_name, s.createdAt ORDER BY s.createdAt DESC LIMIT 2").unwrap();
    assert_eq!(desc.rows.len(), 2);
    assert!(desc.rows[0].get(1).and_then(|v| v.as_int64()).is_none(),
        "DESC : la ligne sans createdAt (NULL) passe en PREMIER, obtenu {:?}", desc.rows[0]);
    assert_eq!(desc.rows[1].get(1).and_then(|v| v.as_int64()), Some(300),
        "DESC : puis la plus grande valeur reelle");

    let asc = s.execute("MATCH (s:S)-[:OWNED_BY]->(a:A {qualified_name: 'acc'}) RETURN s.qualified_name, s.createdAt ORDER BY s.createdAt ASC LIMIT 4").unwrap();
    assert_eq!(asc.rows.len(), 4);
    let derniers: Vec<Option<i64>> = asc.rows.iter().map(|r| r.get(1).and_then(|v| v.as_int64())).collect();
    assert_eq!(derniers, vec![Some(100), Some(200), Some(300), None],
        "ASC : les NULL passent en DERNIER");
}
