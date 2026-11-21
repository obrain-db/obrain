//! (LIMIT-croissant) Un `LIMIT k` sur une expansion (`MATCH (a)-[r]->(b)`) ne
//! doit plus balayer tous les nœuds : le scan source est plafonné, et le
//! plafond CROÎT tant que le consommateur n'a pas ses k lignes — jamais de
//! sous-livraison, même quand des feuilles (degré 0) sont en tête de scan.
//!
//! Ces tests tournent sur une base DISQUE (SubstrateStore) : lui seul implémente
//! `node_ids_capped` (arrêt anticipé) et `scan_visibility_is_trivial`. En
//! mémoire, le trait retombe sur `node_ids() + truncate` et ne plafonne pas.

use obrain_engine::ObrainDB;

fn base() -> (tempfile::TempDir, ObrainDB) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path()).expect("ouverture base disque");
    (dir, db)
}

/// 60 feuilles (aucune arête sortante) créées EN PREMIER, donc scannées en
/// tête. Le plafond du scan doit croître AU-DELÀ pour trouver les arêtes — un
/// plafond fixe rendrait 0 ligne.
#[test]
fn limit_sur_expansion_rend_k_malgre_feuilles_en_tete() {
    let (_d, db) = base();
    let s = db.session();
    for i in 0..60 {
        s.execute(&format!("INSERT (:Feuille {{v: {i}}})")).unwrap();
    }
    for i in 0..10 {
        s.execute(&format!(
            "INSERT (:Source {{v: {i}}})-[:REL]->(:Cible {{v: {i}}})"
        ))
        .unwrap();
    }
    let cinq = s.execute("MATCH (a)-[r]->(b) RETURN a, b LIMIT 5").unwrap();
    assert_eq!(
        cinq.rows.len(),
        5,
        "LIMIT 5 doit rendre 5 arêtes malgré 60 feuilles en tête"
    );
    let tout = s
        .execute("MATCH (a)-[r]->(b) RETURN a, b LIMIT 100")
        .unwrap();
    assert_eq!(
        tout.rows.len(),
        10,
        "LIMIT 100 doit rendre les 10 arêtes (aucune perdue)"
    );
    assert_eq!(
        cinq.rows.as_slice(),
        &tout.rows[..5],
        "LIMIT 5 = préfixe exact du sans-plafond"
    );
}

/// Un `LIMIT` supérieur au nombre d'arêtes rend TOUTES les arêtes (ni plus, ni
/// moins) : la croissance s'arrête à l'épuisement réel.
#[test]
fn limit_sur_expansion_au_dela_du_cardinal_rend_tout() {
    let (_d, db) = base();
    let s = db.session();
    for i in 0..3 {
        s.execute(&format!("INSERT (:S {{v: {i}}})-[:R]->(:T {{v: {i}}})"))
            .unwrap();
    }
    let r = s
        .execute("MATCH (a)-[r]->(b) RETURN a, b LIMIT 100")
        .unwrap();
    assert_eq!(
        r.rows.len(),
        3,
        "LIMIT 100 sur 3 arêtes doit rendre 3 lignes"
    );
}

/// Sans `LIMIT`, le balayage complet reste la référence (toutes les arêtes).
#[test]
fn sans_limit_l_expansion_rend_tout() {
    let (_d, db) = base();
    let s = db.session();
    for i in 0..7 {
        s.execute(&format!("INSERT (:A {{v: {i}}})-[:L]->(:B {{v: {i}}})"))
            .unwrap();
    }
    let r = s.execute("MATCH (a)-[r]->(b) RETURN a, b").unwrap();
    assert_eq!(
        r.rows.len(),
        7,
        "sans LIMIT, toutes les arêtes sont rendues"
    );
}
