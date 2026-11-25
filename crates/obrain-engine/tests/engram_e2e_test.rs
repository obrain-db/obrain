//! Chaîne E2E « observation → activation → engramme → rappel par indice
//! partiel → consolidation → persistance → redémarrage → rappel ».
//!
//! Rouge sans le pont (`EngramObservationBridge`, enregistré par le builder
//! cognitif sous `#[cfg(feature = "engram")]`) : avant, `record_activation`
//! n'avait AUCUN appelant hors tests, donc aucune mutation de l'API publique
//! ne formait jamais d'engramme.
//!
//! Aucun câblage manuel ici : on passe par `ObrainDB::open_substrate_observed`,
//! la même voie qu'un utilisateur réel. Les mutations GQL (`CREATE` / `SET`)
//! traversent `CognitiveObserver` → `MutationBus` → `Scheduler` → bridge, et
//! les engrams se forment SANS appel direct à `record_activation`.
#![cfg(feature = "cognitive-engram")]

use std::time::Duration;

use obrain_cognitive::engram::EngramHorizon;
use obrain_common::types::NodeId;
use obrain_engine::{ObrainDB, Session};

const ATTENTE: Duration = Duration::from_secs(5);

/// Id d'un nœud `Item` par son nom, via l'API publique.
fn id_de(session: &Session, nom: &str) -> NodeId {
    let res = session
        .execute(&format!("MATCH (n:Item {{name:'{nom}'}}) RETURN id(n)"))
        .expect("match id");
    let brut = res.rows[0][0].to_string();
    NodeId(brut.parse::<u64>().expect("id entier"))
}

/// Une « activation » du trio : trois `SET` (un par nœud) dans la même
/// fenêtre — donc le même lot dispatché — soit UN épisode de co-activation.
/// `valeur` change la valeur pour garantir un vrai changement de propriété.
fn activer(session: &Session, noms: &[&str], valeur: i64) {
    // Triangle d'arêtes {a,b,c} : chaque arête est une co-activation GENUINE
    // {src,dst} (pas un lot de singletons). Les paires (a,b),(b,c),(c,a)
    // co-occurrent 3 fois → composante connexe {a,b,c}.
    let (a, b, c) = (noms[0], noms[1], noms[2]);
    for (x, y) in [(a, b), (b, c), (c, a)] {
        session
            .execute(&format!(
                "MATCH (n:Item {{name:'{x}'}}), (m:Item {{name:'{y}'}}) CREATE (n)-[:COACT {{r:{valeur}}}]->(m)"
            ))
            .expect("edge");
    }
}

/// Ids triés de l'ensemble d'un engramme.
fn ensemble_trie(ids: &[NodeId]) -> Vec<u64> {
    let mut v: Vec<u64> = ids.iter().map(|n| n.0).collect();
    v.sort_unstable();
    v
}

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

#[test]
fn chaine_e2e_observation_vers_rappel_persistant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin = dir.path().join("base");

    // ── Phase 1 : formation AUTOMATIQUE depuis les mutations ──────────────
    let db = ObrainDB::open_substrate_observed(&chemin).expect("open");
    let session = db.session();

    // Deux trios disjoints : {a,b,c} et {d,e,f}.
    session
        .execute(
            "CREATE (:Item {name:'a'}), (:Item {name:'b'}), (:Item {name:'c'}),
                    (:Item {name:'d'}), (:Item {name:'e'}), (:Item {name:'f'})",
        )
        .expect("create");
    quiesce(&db);

    let a = id_de(&session, "a");
    let b = id_de(&session, "b");
    let c = id_de(&session, "c");
    let d = id_de(&session, "d");
    let e = id_de(&session, "e");
    let f = id_de(&session, "f");
    let trio1 = [a, b, c];
    let trio2 = [d, e, f];

    // Trois co-activations de chaque trio (min_episodes = 3 par défaut).
    for valeur in 1..=3 {
        activer(&session, &["a", "b", "c"], valeur);
        quiesce(&db);
        activer(&session, &["d", "e", "f"], valeur);
        quiesce(&db);
    }

    let engine = db.cognitive_engine().expect("moteur cognitif");
    let mgr = engine.engram_manager().expect("engram manager");

    // Les deux engrams se sont formés automatiquement.
    let engrams = mgr.store().list();
    let abc = engrams
        .iter()
        .find(|e| ensemble_trie(&e.node_ids()) == ensemble_trie(&trio1));
    let def = engrams
        .iter()
        .find(|e| ensemble_trie(&e.node_ids()) == ensemble_trie(&trio2));
    assert!(
        abc.is_some(),
        "engramme {{a,b,c}} formé automatiquement (engrams={:?})",
        engrams
            .iter()
            .map(|e| ensemble_trie(&e.node_ids()))
            .collect::<Vec<_>>()
    );
    assert!(def.is_some(), "engramme {{d,e,f}} formé automatiquement");

    // ── Phase 2 : rappel par indice PARTIEL (pas une égalité exacte) ──────
    // Cue = un seul membre du trio → on retrouve l'engramme COMPLET.
    let rappel_a = mgr.recall(&[a], 5);
    assert!(
        !rappel_a.is_empty(),
        "rappel par indice partiel [a] non vide"
    );
    // ASSERTION STRICTE (P0) : le top-1 doit être l'engramme COMPLET. ROUGE
    // tant que la formation incrémentale laisse subsister un engramme
    // intermédiaire {a,b} (défaut mesuré — note diagnostic causal).
    assert_eq!(
        ensemble_trie(&rappel_a[0].engram.node_ids()),
        ensemble_trie(&trio1),
        "top-1 de recall([a]) = {{a,b,c}} complet (obtenu {:?})",
        rappel_a
            .iter()
            .map(|r| ensemble_trie(&r.engram.node_ids()))
            .collect::<Vec<_>>()
    );

    // ── Phase 3 : distracteurs ────────────────────────────────────────────
    // Un indice appartenant à l'AUTRE trio ne doit pas ramener {a,b,c}.
    let rappel_d = mgr.recall(&[d], 5);
    assert!(!rappel_d.is_empty(), "rappel par [d] non vide");
    assert_eq!(
        ensemble_trie(&rappel_d[0].engram.node_ids()),
        ensemble_trie(&trio2),
        "top-1 de recall([d]) = {{d,e,f}} (obtenu {:?})",
        rappel_d
            .iter()
            .map(|r| ensemble_trie(&r.engram.node_ids()))
            .collect::<Vec<_>>()
    );

    // ── Phase 4 : bruit ───────────────────────────────────────────────────
    // Un indice DOMINANT bruité (a, b + un distracteur d) doit toujours
    // compléter {a,b,c} : le bruit n'inverse pas le rappel.
    let rappel_bruite = mgr.recall(&[a, b, d], 5);
    assert!(!rappel_bruite.is_empty(), "rappel bruité non vide");
    assert_eq!(
        ensemble_trie(&rappel_bruite[0].engram.node_ids()),
        ensemble_trie(&trio1),
        "top-1 d'un indice bruité [a, b, d] = {{a,b,c}} (obtenu {:?})",
        rappel_bruite
            .iter()
            .map(|r| ensemble_trie(&r.engram.node_ids()))
            .collect::<Vec<_>>()
    );

    // ── Phase 5 : consolidation ───────────────────────────────────────────
    let id_abc = abc.unwrap().id;
    for _ in 0..10 {
        mgr.mark_used(id_abc);
    }
    mgr.tick();
    let consolide = mgr.store().get(id_abc).expect("engramme encore présent");
    assert_eq!(
        consolide.horizon,
        EngramHorizon::Consolidated,
        "après rappels + tique, l'engramme est consolidé (horizon={:?})",
        consolide.horizon
    );
    // Le rappel fonctionne toujours après consolidation.
    let apres_consolidation = mgr.recall(&[a], 5);
    assert!(
        apres_consolidation
            .iter()
            .any(|r| ensemble_trie(&r.engram.node_ids()) == ensemble_trie(&trio1)),
        "rappel par indice partiel toujours OK après consolidation"
    );

    // ── Phase 6 : persistance et redémarrage ──────────────────────────────
    db.close().expect("close");
    drop(db);

    let db2 = ObrainDB::open_substrate_observed(&chemin).expect("reopen");
    let engine2 = db2.cognitive_engine().expect("moteur après réouverture");
    let mgr2 = engine2
        .engram_manager()
        .expect("engram manager après réouverture");

    let rehydrates = mgr2.store().list();
    assert!(
        rehydrates
            .iter()
            .any(|e| ensemble_trie(&e.node_ids()) == ensemble_trie(&trio1)),
        "l'engramme {{a,b,c}} survit au redémarrage (rechargés={:?})",
        rehydrates
            .iter()
            .map(|e| ensemble_trie(&e.node_ids()))
            .collect::<Vec<_>>()
    );

    // Le rappel par indice partiel fonctionne APRÈS redémarrage : le sous-
    // ensemble [a] (ou [b]) restitue le trio complet.
    let rappel_apres_restart = mgr2.recall(&[b], 5);
    assert!(
        rappel_apres_restart
            .iter()
            .any(|r| ensemble_trie(&r.engram.node_ids()) == ensemble_trie(&trio1)),
        "rappel par indice partiel [b] après redémarrage restitue {{a,b,c}}"
    );
}

/// Un nœud isolé (jamais co-activé avec un autre) ne forme jamais d'engramme :
/// la formation exige une CO-occurrence (≥ 2 nœuds) répétée.
#[test]
fn un_noeud_isole_ne_forme_pas_d_engramme() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate_observed(dir.path().join("base")).expect("open");
    let session = db.session();
    session
        .execute("CREATE (:Item {name:'solo'})")
        .expect("create");
    quiesce(&db);

    // De nombreuses activations d'un SEUL nœud (mises à jour successives).
    for valeur in 1..=6 {
        session
            .execute(&format!(
                "MATCH (n:Item {{name:'solo'}}) SET n.touch = {valeur}"
            ))
            .expect("set");
        quiesce(&db);
    }

    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("engram manager");
    assert!(
        mgr.store().list().is_empty(),
        "un nœud isolé ne forme pas d'engramme (min 2 nœuds co-activés)"
    );
}

/// PARITÉ (Lot 1) : état cognitif en mémoire / après formation substrate /
/// après réouverture. Mesure la divergence des poids et signatures.
#[test]
fn parite_etat_cognitif_memoire_substrate_reouverture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin = dir.path().join("base");
    let db = ObrainDB::open_substrate_observed(&chemin).expect("open");
    let session = db.session();
    session
        .execute("CREATE (:Item {name:'a'}), (:Item {name:'b'}), (:Item {name:'c'})")
        .expect("create");
    quiesce(&db);
    for valeur in 1..=3 {
        activer(&session, &["a", "b", "c"], valeur);
        quiesce(&db);
    }
    let engine = db.cognitive_engine().expect("moteur");
    let mgr = engine.engram_manager().expect("mgr");
    let avant: Vec<(u64, Vec<f64>, usize)> = mgr
        .store()
        .list()
        .iter()
        .map(|e| {
            let w: Vec<f64> = e.ensemble.iter().map(|(_, x)| *x).collect();
            (e.id.0, w, e.spectral_signature.len())
        })
        .collect();
    db.close().expect("close");
    drop(db);

    let db2 = ObrainDB::open_substrate_observed(&chemin).expect("reopen");
    let engine2 = db2.cognitive_engine().expect("moteur2");
    let mgr2 = engine2.engram_manager().expect("mgr2");
    let apres: Vec<(u64, Vec<f64>, usize)> = mgr2
        .store()
        .list()
        .iter()
        .map(|e| {
            let w: Vec<f64> = e.ensemble.iter().map(|(_, x)| *x).collect();
            (e.id.0, w, e.spectral_signature.len())
        })
        .collect();
    eprintln!("PARITE avant={avant:?}");
    eprintln!("PARITE apres={apres:?}");
}
