//! Câblage OPT-IN des thinkers (T13) par l'API PUBLIQUE (2026-11-25).
//!
//! Le défaut historique : `spawn_standard_fleet` n'était appelé que par un
//! test ; la doc prétendait que `SubstrateStore::open` la lançait (faux). Une
//! base ordinaire ne doit RIEN démarrer en tâche de fond — seule l'ouverture
//! explicite `open_substrate_with_thinkers` arme la flotte. Ces deux tests
//! figent les deux moitiés du contrat.
#![cfg(feature = "thinker")]

use std::time::{Duration, Instant};

use obrain_cognitive::thinker::{ThinkerKind, ThinkersConfig};
use obrain_engine::ObrainDB;

/// Cadence de tique minimale (1 s) pour que le test voie les tiques avancer
/// sans attendre les cadences de production (Predictor 30 s, Warden 300 s,
/// Dreamer 600 s, Consolidator dérivé de la demi-vie 24 h).
fn config_rapide() -> ThinkersConfig {
    let mut cfg = ThinkersConfig::default();
    cfg.consolidator.inner_toml.interval_secs = Some(1);
    cfg.warden.inner_toml.interval_secs = Some(1);
    cfg.predictor.inner_toml.interval_secs = Some(1);
    cfg.dreamer.inner_toml.interval_secs = Some(1);
    cfg
}

/// Ouverture opt-in : la flotte par défaut (Consolidator + Warden + Predictor
/// et Dreamer) est vivante dès l'ouverture, ses tiques avancent, et l'arrêt ne
/// laisse aucune tâche survivre (le test se termine sans hang).
#[test]
fn ouverture_opt_in_demarre_la_flotte_et_s_arrete_proprement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate_with_thinkers(dir.path().join("base"), config_rapide())
        .expect("open with thinkers");

    // Flotte vivante immédiatement : les 4 thinkers standard sont lancés.
    let stats = db.thinker_stats().expect("runtime thinkers présent");
    assert!(
        stats.threads_live >= 4,
        "flotte vivante (threads={})",
        stats.threads_live
    );
    let kinds = db.thinker_kinds();
    assert!(
        kinds.contains(&ThinkerKind::Consolidator),
        "Consolidator lancé (kinds={kinds:?})"
    );

    // Les tiques avancent (intervalle minimal 1 s, temps d'attente borné).
    let debut = Instant::now();
    while db.thinker_stats().is_some_and(|s| s.total_ticks == 0)
        && debut.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    let stats = db.thinker_stats().expect("runtime");
    assert!(
        stats.total_ticks > 0,
        "les tiques avancent (ticks={})",
        stats.total_ticks
    );

    // Arrêt propre : `close()` consomme le runtime (joint les threads dans le
    // timeout), donc plus aucune tâche ne survit. Le test se termine sans hang.
    db.close().expect("close");
    assert!(db.thinker_stats().is_none(), "runtime démonté après close");
}

/// Ouverture ORDINAIRE : aucune tâche de fond — le champ thinker_runtime reste
/// `None`, aucun handle, aucune flotte.
#[test]
fn ouverture_ordinaire_ne_demarre_rien() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate(dir.path().join("base")).expect("open");
    assert!(
        db.thinker_stats().is_none(),
        "aucun runtime thinker en ouverture ordinaire"
    );
    assert!(
        db.thinker_kinds().is_empty(),
        "aucun thinker lancé en ouverture ordinaire"
    );
    db.close().expect("close");
}
