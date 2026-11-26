//! Migration d'une ANCIENNE base vers l'ancre composite.
//!
//! Avant l'ancre composite `(E_ref, t_ref, t_hi, H)`, l'énergie vivait dans la
//! colonne Q1.15 et l'instant de référence dans une propriété f64 séparée
//! `_cog_energy_last_activated_epoch`. Ces tests simulent une telle base et
//! vérifient :
//!  1. la lecture applique la décroissance depuis l'ancre héritée (pas de
//!     valeur figée) ;
//!  2. `advance_all` migre l'ancre vers la composite SANS remettre l'énergie
//!     à zéro silencieusement ;
//!  3. une base SANS aucune ancre ne voit pas son énergie réinitialisée à la
//!     lecture : elle rend `E_ref` tel quel, et l'ancrage se fait à l'écriture.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_cognitive::store_trait::{PROP_ENERGY_LAST_ACTIVATED_EPOCH, persist_node_f64};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

/// Époque de base en micros, loin de l'heure murale : 1e15 µs (= 1e9 s,
/// exact en f64, bien sous 2^53). Le `t_ref` persisté en secondes epoch f64
/// fait l'aller-retour micros → secondes → micros SANS perte.
const BASE: i64 = 1_000_000_000_000_000;
/// Demi-vie de 24 h ; une demi-vie = 1 jour = 86 400 × 1e6 µs.
const H: Duration = Duration::from_secs(24 * 3600);
const JOUR: i64 = 24 * 3600 * 1_000_000;

fn config() -> EnergyConfig {
    EnergyConfig {
        default_half_life: H,
        ..Default::default()
    }
}

fn horloge() -> (Arc<AtomicI64>, Arc<dyn Fn() -> i64 + Send + Sync>) {
    let t = Arc::new(AtomicI64::new(BASE));
    let f: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&t);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    (t, f)
}

/// Ancienne base : colonne + ancre héritée (f64 epoch), PAS d'ancre composite.
/// Après une demi-vie hors ligne, la lecture doit donner 0,5 (décroissance
/// appliquée depuis l'ancre héritée), pas la colonne figée à 1,0.
#[test]
fn une_ancienne_base_avec_ancre_heritee_decroit_correctement() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (clock, clock_fn) = horloge();

    let node;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        node = sub.create_node(&["n"]);
        sub.set_node_energy_f32(node, 1.0).unwrap();
        // Ancre héritée : secondes epoch f64. BASE µs = BASE/1e6 secondes.
        persist_node_f64(
            sub.as_ref(),
            node,
            PROP_ENERGY_LAST_ACTIVATED_EPOCH,
            (BASE as f64) / 1_000_000.0,
        );
        // Aucune ancre composite écrite : c'est bien une base d'avant.
        assert!(decay_clock::read_anchor(&sub, node).is_none());
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // +1 demi-vie hors ligne.
    clock.store(BASE + JOUR, Ordering::Relaxed);
    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store = EnergyStore::with_substrate(config(), sub.clone()).with_clock(clock_fn);
    let e = store.get_energy(node);
    assert!(
        (e - 0.5).abs() < 1e-9,
        "ancre héritée : énergie = {e}, attendu 0.5 (pas la colonne figée 1.0)"
    );
}

/// Ancienne base SANS ancre du tout (ni composite ni héritée) : la lecture ne
/// réinitialise PAS silencieusement (elle rend E_ref tel quel, l'âge est
/// inconnu), et `advance_all` ancre le nœud à `now` — migration explicite dans
/// le chemin d'écriture.
#[test]
fn une_base_sans_ancre_ne_reinitialise_pas_silencieusement() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node = sub.create_node(&["n"]);
    sub.set_node_energy_f32(node, 1.0).unwrap();
    sub.flush().unwrap();

    let law = DecayLaw::new(H);
    // Lecture sans ancre : rend E_ref tel quel (pas de remise à zéro).
    let e = decay_clock::energy_at(&sub, node, &law, BASE).unwrap();
    assert!(
        (e - 1.0).abs() < 1e-9,
        "sans ancre, la lecture rend E_ref tel quel : {e} (attendu 1.0)"
    );

    // Migration : advance_all ancre le nœud à `now`.
    let stats = decay_clock::advance_all(&sub, &law, 0, BASE).unwrap();
    assert_eq!(stats.unanchored, 1, "un nœud sans ancre migré");
    assert!(
        decay_clock::read_anchor(&sub, node).is_some(),
        "l'ancre composite est posée par la migration"
    );
}

// ---------------------------------------------------------------------------
// Migration de CONFIGURATION `decay_factor` → `half_life` (point 6 de la revue)
// ---------------------------------------------------------------------------

/// `decay_factor = 0.99` toutes les 60 s ⇒ demi-vie `H = −Δt·ln2 / ln f` ≈ 69 min.
/// L'ancien facteur fixe aurait été 20,9× trop rapide : la migration doit
/// produire ~69 min, PAS le défaut 24 h.
#[cfg(feature = "thinker")]
#[test]
fn la_configuration_decay_factor_est_migree_vers_half_life() {
    use obrain_cognitive::decay_law::DecayLaw;
    use obrain_cognitive::thinker::ThinkerFleetConfig;
    use obrain_cognitive::thinker::config::ConsolidatorTomlCfg;

    let mut fleet = ThinkerFleetConfig::<ConsolidatorTomlCfg>::default();
    fleet.inner_toml.interval_secs = Some(60);
    fleet.inner_toml.decay_factor = Some(0.99);
    let cfg = fleet.inner();

    let attendu = DecayLaw::implied_half_life(0.99, Duration::from_secs(60));
    let ecart = (cfg.half_life.as_secs_f64() - attendu.as_secs_f64()).abs();
    assert!(
        ecart < 1.0,
        "migration : H = −Δt·ln2 / ln f (écart {ecart}s — précision f32/f64 de 0.99)"
    );
    assert!(
        cfg.half_life.as_secs() < 24 * 3600,
        "migré vers ~69 min, pas le défaut 24 h : {:?}",
        cfg.half_life
    );
    assert!(
        (cfg.half_life.as_secs() as f64 - 4140.0).abs() < 60.0,
        "≈ 69 min : {:?}",
        cfg.half_life
    );
}

/// Sans période (`interval_secs` absent), l'incompatibilité est annoncée et la
/// demi-vie reste le défaut — aucune conversion silencieuse à partir d'un
/// facteur seul.
#[cfg(feature = "thinker")]
#[test]
fn decay_factor_sans_periode_reste_sur_le_defaut() {
    use obrain_cognitive::thinker::ThinkerFleetConfig;
    use obrain_cognitive::thinker::config::ConsolidatorTomlCfg;

    let mut fleet = ThinkerFleetConfig::<ConsolidatorTomlCfg>::default();
    fleet.inner_toml.interval_secs = None;
    fleet.inner_toml.decay_factor = Some(0.99);
    let cfg = fleet.inner();
    assert_eq!(
        cfg.half_life,
        Duration::from_secs(24 * 3600),
        "sans période, le défaut est conservé (incompatibilité annoncée)"
    );
}
