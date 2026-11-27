//! VRAIE requête cognitive GQL, exécutée par `Session::execute` (parcours
//! parse -> plan -> exécution), pas par un appel direct au dispatcher.
//!
//! On réutilise les procédures engramme DÉJÀ présentes (aucun second moteur
//! de requête) : `obrain.engrams.list`, `obrain.engrams.inspect`,
//! `obrain.cognitive.metrics`. Les engrams sont formés par les mutations GQL
//! ordinaires (CREATE + co-activations), via la même chaîne observée qu'un
//! utilisateur réel (CognitiveObserver -> MutationBus -> Scheduler -> bridge).

#![cfg(all(feature = "cognitive-engram", feature = "algos"))]

use std::time::Duration;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const ATTENTE: Duration = Duration::from_secs(5);

/// Attend que la cognition ait traité toutes les mutations publiées.
fn quiesce(db: &ObrainDB) {
    assert!(
        db.cognitive_quiesce(ATTENTE),
        "quiesce : publiés={}, dispatchés={}, perdus={}",
        db.cognitive_events_published(),
        db.cognitive_events_dispatched(),
        db.cognitive_events_lost()
    );
    assert_eq!(db.cognitive_events_lost(), 0, "aucun événement perdu");
}

/// Triangle d'arêtes {a,b,c} : chaque arête est une co-activation GENUINE
/// {src,dst}. Trois passages forment la composante connexe {a,b,c}.
fn activer(session: &obrain_engine::Session, noms: &[&str], valeur: i64) {
    let (a, b, c) = (noms[0], noms[1], noms[2]);
    for (x, y) in [(a, b), (b, c), (c, a)] {
        session
            .execute(&format!(
                "MATCH (n:Item {{name:'{x}'}}), (m:Item {{name:'{y}'}}) CREATE (n)-[:COACT {{r:{valeur}}}]->(m)"
            ))
            .expect("edge");
    }
}

/// `CALL obrain.engrams.list()` via Session::execute : le résultat reflète les
/// engrams réellement formés par les mutations GQL.
#[test]
fn call_engrams_list_gql_reel() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate_observed(dir.path().join("base")).unwrap();
    let session = db.session();

    session
        .execute("CREATE (:Item {name:'a'}), (:Item {name:'b'}), (:Item {name:'c'})")
        .unwrap();
    quiesce(&db);

    for v in 1..=3 {
        activer(&session, &["a", "b", "c"], v);
        quiesce(&db);
    }

    // Référence native : combien d'engrams le manager voit-il ?
    let mgr = db.cognitive_engine().unwrap().engram_manager().unwrap();
    let n_ref = mgr.store().count();
    assert!(n_ref >= 1, "au moins un engramme formé par co-activation");

    // VRAIE requête GQL.
    let r = session
        .execute(
            "CALL obrain.engrams.list() \
             YIELD id, strength, recall_count, horizon, ensemble_size \
             RETURN id, strength, recall_count, horizon, ensemble_size",
        )
        .expect("CALL obrain.engrams.list");

    assert_eq!(r.columns.len(), 5, "5 colonnes RETURNées");
    assert_eq!(
        r.rows.len(),
        n_ref,
        "la requête GQL rend le même nombre d'engrams que le store natif"
    );
    for row in &r.rows {
        assert!(
            matches!(row[0], Value::Int64(_)),
            "id est Int64 : {:?}",
            row[0]
        );
        assert!(
            matches!(row[2], Value::Int64(_)),
            "recall_count est Int64 : {:?}",
            row[2]
        );
        assert!(
            matches!(row[4], Value::Int64(n) if n >= 2),
            "ensemble_size >= 2 (co-activation) : {:?}",
            row[4]
        );
    }
    // La composante complète {a,b,c} (taille 3) finit par se former.
    assert!(
        r.rows.iter().any(|row| matches!(row[4], Value::Int64(3))),
        "la composante {{a,b,c}} (taille 3) est formée"
    );

    // L'id rendu par GQL correspond à un id réel du store natif.
    let ids_natifs: Vec<i64> = mgr.store().list().iter().map(|e| e.id.0 as i64).collect();
    for row in &r.rows {
        if let Value::Int64(id) = row[0] {
            assert!(
                ids_natifs.contains(&id),
                "id GQL {id} présent dans le store"
            );
        }
    }
}

/// `CALL obrain.engrams.inspect(id)` rend le détail d'un engramme identifié
/// par sa propre liste GQL (requête chaînée, pas un id codé en dur).
#[test]
fn call_engrams_inspect_gql_reel() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate_observed(dir.path().join("base")).unwrap();
    let session = db.session();

    session
        .execute("CREATE (:Item {name:'a'}), (:Item {name:'b'}), (:Item {name:'c'})")
        .unwrap();
    quiesce(&db);
    for v in 1..=3 {
        activer(&session, &["a", "b", "c"], v);
        quiesce(&db);
    }

    let list = session
        .execute("CALL obrain.engrams.list() YIELD id RETURN id")
        .expect("CALL list");
    let id = match list.rows.first().and_then(|r| r.first()) {
        Some(Value::Int64(i)) => *i,
        other => panic!("id Int64 attendu, obtenu {:?}", other),
    };

    let insp = session
        .execute(&format!(
            "CALL obrain.engrams.inspect({id}) \
             YIELD id, strength, recall_count, ensemble_size, spectral_dim \
             RETURN id, strength, recall_count, ensemble_size, spectral_dim"
        ))
        .expect("CALL obrain.engrams.inspect");
    assert!(!insp.rows.is_empty(), "inspect rend une ligne");
    let row = &insp.rows[0];
    assert_eq!(row[0], Value::Int64(id), "même id inspecté");
    assert!(
        matches!(row[3], Value::Int64(n) if n >= 2),
        "ensemble_size >= 2 : {:?}",
        row[3]
    );
    assert!(
        matches!(row[4], Value::Int64(64)),
        "spectral_dim=64 (encodeur spectral par défaut) : {:?}",
        row[4]
    );
}

/// `CALL obrain.cognitive.metrics()` : compteur de formations cohérent avec le
/// nombre d'engrams formés.
#[test]
fn call_cognitive_metrics_gql_reel() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate_observed(dir.path().join("base")).unwrap();
    let session = db.session();

    session
        .execute("CREATE (:Item {name:'a'}), (:Item {name:'b'}), (:Item {name:'c'})")
        .unwrap();
    quiesce(&db);
    for v in 1..=3 {
        activer(&session, &["a", "b", "c"], v);
        quiesce(&db);
    }

    let m = session
        .execute(
            "CALL obrain.cognitive.metrics() \
             YIELD engrams_active, engrams_formed, recalls_attempted \
             RETURN engrams_active, engrams_formed, recalls_attempted",
        )
        .expect("CALL obrain.cognitive.metrics");
    assert!(!m.rows.is_empty(), "metrics rend une ligne");
    let row = &m.rows[0];
    // engrams_active / engrams_formed : au moins un engramme formé.
    assert!(
        matches!(&row[0], Value::Int64(n) if *n >= 1),
        "engrams_active >= 1, obtenu {:?}",
        row[0]
    );
    assert!(
        matches!(&row[1], Value::Int64(n) if *n >= 1),
        "engrams_formed >= 1, obtenu {:?}",
        row[1]
    );
}
