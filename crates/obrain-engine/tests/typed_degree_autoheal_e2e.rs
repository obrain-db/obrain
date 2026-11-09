//! (2026-09-05) E2E — fermeture définitive du bug « arêtes typées → 0 » :
//! des sidecars de degrés typés PÉRIMÉS (CRC valide, compteurs faux) ne
//! doivent plus jamais produire de 0 silencieux sur `MATCH (a)-[:T]->(b)`.
//!
//! Scénarios (writer = le chemin réel `import_events` : GQL MERGE via
//! `session.execute`) :
//!   1. réouverture en ÉCRITURE → auto-guérison à la première utilisation
//!      typed → le count typé rend le résultat vrai ;
//!   2. réouverture en LECTURE SEULE (pas de guérison possible) → le
//!      pré-filtre typed-degree est désactivé pour le type douteux → le
//!      chemin scan générique rend le résultat vrai.
//!      Aucune variable d'environnement (kill switch), aucun `repair-degrees`.

use obrain_engine::ObrainDB;

fn corrompre_sidecar_typed(path: &std::path::Path, type_name: &str) {
    // Sidecar `substrate.degrees.node.<TYPE>.u32` : en-tête 64 B conservé
    // (magic/version/n_slots), payload remis à ZÉRO, CRC IEEE recalculé sur
    // `n_slots × 8` octets — exactement la péremption laissée par un
    // binaire plus ancien (mapping de slots différent).
    let col = path.join(format!("substrate.degrees.node.{type_name}.u32"));
    let bytes = std::fs::read(&col).expect("sidecar présent");
    let n_slots = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let payload_bytes = n_slots * 8;
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..64]);
    out.resize(64 + payload_bytes, 0u8);
    let crc = {
        let mut h = crc32fast::Hasher::new();
        h.update(&out[64..]);
        h.finalize()
    };
    out[16..20].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&col, &out).unwrap();
}

fn planter_graphe(db: &ObrainDB) {
    // Même forme d'écriture que `import_events`/`link_edge` (GQL MERGE).
    let s = db.session();
    let _ = s
        .execute("CREATE (:A {qualified_name: 'x', name: 'x'})")
        .unwrap();
    let _ = s
        .execute("CREATE (:B {qualified_name: 'y', name: 'y'})")
        .unwrap();
    let _ = s
        .execute(
            "MATCH (a:A {qualified_name: 'x'}), (b:B {qualified_name: 'y'}) MERGE (a)-[:TOUCHES]->(b)",
        )
        .unwrap();
    db.checkpoint().unwrap();
}

fn count_typé(db: &ObrainDB) -> i64 {
    let res = db
        .session()
        .execute("MATCH (a)-[:TOUCHES]->(b) RETURN count(*) AS n")
        .unwrap();
    assert!(
        !res.rows.is_empty(),
        "la requête typée doit rendre une ligne"
    );
    match &res.rows[0][0] {
        obrain_common::types::Value::Int64(v) => *v,
        other => panic!("count inattendu: {other:?}"),
    }
}

#[test]
fn typed_degree_auto_heal_apres_reouverture_ecriture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-e2e-rw");
    {
        let db = ObrainDB::open_substrate_with_mode(
            &path,
            obrain_substrate::wal_io::SyncMode::EveryCommit,
        )
        .unwrap();
        planter_graphe(&db);
    } // flush/close
    // Péremption simulée du sidecar TOUCHES.
    corrompre_sidecar_typed(&path, "TOUCHES");
    {
        let db = ObrainDB::open_substrate_with_mode(
            &path,
            obrain_substrate::wal_io::SyncMode::EveryCommit,
        )
        .unwrap();
        // Auto-guérison : le count typé doit rendre le résultat VRAI (1),
        // pas 0 — sans kill switch, sans repair manuel.
        assert_eq!(
            count_typé(&db),
            1,
            "auto-guérison E2E : count typé ≠ 0 après péremption"
        );
    }
}

#[test]
fn typed_degree_fallback_lecture_seule_rend_le_vrai_resultat() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-e2e-ro");
    {
        let db = ObrainDB::open_substrate_with_mode(
            &path,
            obrain_substrate::wal_io::SyncMode::EveryCommit,
        )
        .unwrap();
        planter_graphe(&db);
    }
    corrompre_sidecar_typed(&path, "TOUCHES");
    {
        // Lecteur RO (ne peut pas guérir) : le pré-filtre typed-degree est
        // désactivé pour le type douteux → scan générique → résultat vrai.
        let db = ObrainDB::open_read_only(&path).unwrap();
        assert_eq!(
            count_typé(&db),
            1,
            "fallback RO : count typé ≠ 0 malgré colonne périmée"
        );
    }
}
