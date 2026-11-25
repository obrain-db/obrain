//! Horloge contrôlée : monotonie DE SESSION (point 2) et reprise du balayage
//! borné (point 2b).
//!
//! Deux défauts restants après l'ancre composite :
//! 1. Le `t_hi` persisté ne bouge qu'aux ÉCRITURES. Une lecture pure qui voit
//!    l'horloge reculer (NTP, changement d'heure) remonterait l'énergie au
//!    sein d'un même processus. Le high-water mark DE SESSION (`session_hi`)
//!    borne ce recul sans réécrire l'ancre.
//! 2. `advance_all` balayait les nœuds dans l'ordre des slots : avec une
//!    borne `max_nodes`, les nœuds de tête étaient avancés en permanence et
//!    les derniers affamés. Le tri par ancienneté (`t_hi`, `t_ref`, id) rend
//!    la reprise implicite.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

const BASE: i64 = 1_000_000_000_000_000;
const H24: Duration = Duration::from_secs(24 * 3600);
const JOUR: i64 = 24 * 3600 * 1_000_000;
/// Demi-vie de 64 s : la tique vaut exactement 1 s.
const H: Duration = Duration::from_secs(64);

fn config_h24() -> EnergyConfig {
    EnergyConfig {
        default_half_life: H24,
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

// ---------------------------------------------------------------------------
// Point 2 — temps effectif / recul d'horloge en lecture pure
// ---------------------------------------------------------------------------

/// Le défaut de la lecture pure : ancre (E=1, t_ref, t_hi), lire à +24 h (0,5),
/// reculer l'horloge à +12 h, relire. SANS garde, la lecture donne 0,7071
/// (remontée). AVEC le high-water mark de session, elle reste 0,5.
#[test]
fn un_recul_d_horloge_dans_la_session_ne_fait_pas_remonter() {
    let td = tempfile::tempdir().unwrap();
    let (clock, clock_fn) = horloge();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let node = sub.create_node(&["n"]);
    sub.flush().unwrap();

    let store = EnergyStore::with_substrate(config_h24(), sub.clone()).with_clock(clock_fn.clone());
    // Ancre (E=1, t_ref=BASE, t_hi=BASE, H=24 h) : boost à l'instant BASE.
    store.boost(node, 1.0);

    // Lecture à +24 h : 0,5.
    clock.store(BASE + JOUR, Ordering::Relaxed);
    let a_24h = store.get_energy(node);
    assert!((a_24h - 0.5).abs() < 1e-9, "à +24 h : {a_24h}");

    // L'horloge recule à +12 h. SANS garde de session : 2^(-12/24) = 0,7071.
    clock.store(BASE + JOUR / 2, Ordering::Relaxed);
    let a_12h = store.get_energy(node);
    assert!(
        (a_12h - 0.5).abs() < 1e-9,
        "recul à +12 h : {a_12h}, attendu 0,5 (pas 0,7071)"
    );
}

/// Borne APRÈS redémarrage : le `session_hi` meurt avec le processus. La
/// remontée résiduelle est alors bornée par le DERNIER `t_hi` PERSISTÉ. Ici on
/// matérialise à +24 h (t_hi persisté = 24 h), on ferme, on rouvre à +12 h :
/// la lecture reste 0,5. Sans cette matérialisation (lecture pure puis
/// fermeture), t_hi resterait 0 et la lecture rendrait 0,7071.
#[test]
fn apres_redemarrage_la_remontee_est_bornee_par_le_t_hi_persiste() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (clock, clock_fn) = horloge();
    let node;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        node = sub.create_node(&["n"]);
        sub.flush().unwrap();
        let store =
            EnergyStore::with_substrate(config_h24(), sub.clone()).with_clock(clock_fn.clone());
        store.boost(node, 1.0); // ancre (E=1, BASE, BASE, H=24 h)
        // Matérialisation à +24 h : l'ancre avance et PERSISTE t_hi = 24 h.
        clock.store(BASE + JOUR, Ordering::Relaxed);
        let avances = store.decay_tick(0);
        assert_eq!(avances, 1, "un nœud matérialisé");
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // Redémarrage : session_hi repart de 0, horloge revenue à +12 h.
    clock.store(BASE + JOUR / 2, Ordering::Relaxed);
    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store = EnergyStore::with_substrate(config_h24(), sub.clone()).with_clock(clock_fn.clone());
    let e = store.get_energy(node);
    // Assertion explicite de la borne : le t_hi persisté (24 h) borne le recul.
    assert!(
        (e - 0.5).abs() < 1e-9,
        "recul après redémarrage : {e}, attendu 0,5 (borné par t_hi persisté)"
    );
    assert!(
        (e - std::f64::consts::FRAC_1_SQRT_2).abs() > 0.05,
        "la borne persistée doit empêcher la remontée à 0,7071 : {e}"
    );
}

// ---------------------------------------------------------------------------
// Point 2b — reprise du balayage borné
// ---------------------------------------------------------------------------

/// 5 nœuds boostés au même instant, `max_nodes = 2`, une tique par appel.
/// Après 3 appels, les 5 nœuds ont leur ancre avancée : aucun n'est affamé en
/// permanence. La comptabilité `DecayAdvance` reste exacte.
#[test]
fn le_balayage_borne_n_affame_aucun_noeud() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let law = DecayLaw::new(H);

    let mut ids = Vec::new();
    for _ in 0..5 {
        let id = sub.create_node(&["n"]);
        sub.set_node_energy_f32(id, 1.0).unwrap();
        ids.push(id);
    }
    sub.flush().unwrap();

    // 5 nœuds ancrés au même instant t0 (E=1, t_ref=t0, t_hi=t0, H=64 s).
    let t0 = BASE;
    for &id in &ids {
        decay_clock::persist_anchor_explicit(&sub, id, 1.0, t0, H);
    }

    let pas = H.as_micros() as i64 / 64; // une tique = 1 s
    let mut avance_total = 0u64;
    for k in 1..=3 {
        let stats = decay_clock::advance_all(&sub, &law, 2, t0 + k * pas).unwrap();
        assert_eq!(stats.scanned, 5, "5 slots vivants");
        assert_eq!(stats.dormant, 0, "aucun dormant (énergie 1.0)");
        assert_eq!(stats.unanchored, 0, "tous ancrés au départ");
        assert_eq!(stats.deferred, 3, "5 candidats - 2 avancés = 3 différés");
        avance_total += stats.advanced;
    }

    // Après 3 appels (2 nœuds / appel), les 5 nœuds ont été avancés au moins
    // une fois : leur t_ref a quitté t0.
    for &id in &ids {
        let a = decay_clock::read_anchor(&sub, id).expect("nœud ancré");
        assert!(
            a.t_ref_micros > t0,
            "nœud {id:?} affamé : t_ref = {} (jamais avancé)",
            a.t_ref_micros
        );
    }
    // Comptabilité : 2 avancées par appel, 3 différées.
    assert_eq!(
        avance_total, 6,
        "2 avancées × 3 appels = 6, mesuré {avance_total}"
    );
}

// ---------------------------------------------------------------------------
// Limite du contrat — lecture pure puis redémarrage : la remontée est RÉELLE
// ---------------------------------------------------------------------------

/// Le scénario DISCRIMINANT, figé et documenté (pas caché) :
///
/// 1. ancre durable à `t0` via `EnergyStore::boost` (ancre composite
///    `t_hi = t0`) ;
/// 2. LECTURE SEULE à `t0 + H` → 0,5 : le `session_hi` monte EN MÉMOIRE,
///    mais RIEN n'est persisté — une lecture n'écrit pas l'ancre ;
/// 3. arrêt sans persister ce maximum (`close()` du store, qui ne persiste
///    pas `session_hi`, puis réouverture) ;
/// 4. réouverture avec l'horloge revenue à `t0 + H/2`.
///
/// L'énergie vaut alors ≈ 0,7071 : elle REMONTE, parce que le maximum de
/// lecture n'a PAS été persisté. C'est la limite du contrat : la monotonie
/// n'est garantie qu'au sein d'une session (via `session_hi`) et, à travers un
/// redémarrage, que par le dernier `t_hi` DURABLE (écrit seulement par
/// `reinforce` / `advance_all`).
#[test]
fn apres_redemarrage_sans_materialisation_la_remontee_est_reelle() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (clock, clock_fn) = horloge();
    let node;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        node = sub.create_node(&["n"]);
        sub.flush().unwrap();
        let store =
            EnergyStore::with_substrate(config_h24(), sub.clone()).with_clock(clock_fn.clone());
        // 1. Ancre durable à t0 (E=1, t_ref=BASE, t_hi=BASE, H=24 h).
        store.boost(node, 1.0);

        // 2. LECTURE SEULE à +24 h : 0,5. `session_hi` monte en mémoire,
        //    mais AUCUNE écriture d'ancre n'a lieu (lecture paresseuse).
        clock.store(BASE + JOUR, Ordering::Relaxed);
        let a_24h = store.get_energy(node);
        assert!((a_24h - 0.5).abs() < 1e-9, "à +24 h : {a_24h}");

        // 3. Arrêt sans persister ce maximum : `close()` ne persiste pas
        //    `session_hi`. Le `t_hi` durable vaut toujours t0 (BASE).
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // 4. Réouverture, horloge revenue à +12 h : `session_hi` repart de 0, le
    //    `t_hi` durable est resté BASE. L'énergie REMONTE à ≈ 0,7071.
    clock.store(BASE + JOUR / 2, Ordering::Relaxed);
    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store = EnergyStore::with_substrate(config_h24(), sub.clone()).with_clock(clock_fn.clone());
    let e = store.get_energy(node);
    // Assertion qui FIGE la limite (pas qui la cache) : le maximum de lecture
    // n'a pas été persisté, donc la remontée est réelle et vaut 2^(-12/24).
    assert!(
        (e - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-3,
        "remontée après redémarrage sans matérialisation : {e}, attendu ≈ 0,7071"
    );
    assert!(
        (e - 0.5).abs() > 0.05,
        "sans t_hi durable au-delà de t0, la lecture ne peut pas rester 0,5 : {e}"
    );
}
