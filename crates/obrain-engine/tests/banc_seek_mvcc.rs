//! Micro-banc permanent — chemin de lecture à SEEK D'INDEX de propriété et
//! COMPTE raccourci par étiquette (né vague 4a, garde de visibilité MVCC).
//!
//! Usage comparatif : compiler le MÊME harnais sur la base et le candidat
//! (provenance des binaires VÉRIFIÉE par `cmp` — un target dir partagé entre
//! worktrees rend « Fresh » sans jamais rebâtir), exécuter en paires
//! alternées, comparer les MINIMA (méthodo campagne, charge DGX ambiante).
//!
//! Sortie machine : une ligne `BANC_SEEK_MS: <f64>` et `BANC_COUNT_MS: <f64>`.

use obrain_engine::ObrainDB;
use web_time::Instant;

#[test]
fn banc_seek_mvcc() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // 20 000 nœuds :B {id}, insérés par lots de 100 (hors chrono).
    for lot in 0..200u64 {
        let mut stmt = String::from("INSERT ");
        for i in 0..100u64 {
            let id = lot * 100 + i;
            if i > 0 {
                stmt.push_str(", ");
            }
            stmt.push_str(&format!("(:B {{id: {id}}})"));
        }
        session.execute(&stmt).expect("insertion du lot");
    }

    // Index de propriété RÉEL, par l'API directe (celle que consulte
    // `has_property_index` au planificateur) : sans lui,
    // `find_nodes_by_property` replie en balayage (~22 ms/requête mesurés) et
    // le banc ne mesure plus le seek. Le DDL Cypher `CREATE INDEX ... FOR ...
    // ON ...` ne crée PAS cet index-là (vérifié : ~45 s les 2 000 requêtes,
    // inchangé).
    db.create_property_index("id");

    // Échauffement (plans, index paresseux éventuels).
    for _ in 0..50 {
        let r = session
            .execute("MATCH (n:B {id: 14771}) RETURN n.id AS i")
            .expect("seek");
        assert_eq!(r.rows.len(), 1);
    }

    // ── Mesure 1 : 2 000 requêtes à seek d'index (prédicat en ligne). ──
    let debut = Instant::now();
    for k in 0..2000u64 {
        let id = (k * 7919) % 20000; // parcours pseudo-aléatoire déterministe
        let q = format!("MATCH (n:B {{id: {id}}}) RETURN n.id AS i");
        let r = session.execute(&q).expect("seek");
        assert_eq!(r.rows.len(), 1, "id {id} doit exister");
    }
    let seek_ms = debut.elapsed().as_secs_f64() * 1000.0;

    // ── Mesure 2 : 2 000 comptes raccourcis (le raccourci doit rester armé
    // hors écritures en attente). ──
    let debut = Instant::now();
    for _ in 0..2000u64 {
        let r = session
            .execute("MATCH (n:B) RETURN count(n) AS c")
            .expect("count");
        assert_eq!(format!("{:?}", r.rows[0][0]), "Int64(20000)");
    }
    let count_ms = debut.elapsed().as_secs_f64() * 1000.0;

    println!("BANC_SEEK_MS: {seek_ms:.1}");
    println!("BANC_COUNT_MS: {count_ms:.1}");
}
