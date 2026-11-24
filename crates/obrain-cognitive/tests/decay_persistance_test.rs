//! La décroissance d'énergie doit survivre à la fermeture de la base — et
//! être MESURABLE sans dormir ni tolérer d'erreur d'horloge murale.
//!
//! Défaut mesuré (2026-11-24, base b8ee6f87) : en mode substrat, l'énergie est
//! stockée dans la colonne `NodeRecord.energy` (Q1.15) mais aucun instant de
//! référence n'était persisté. Au redémarrage, la valeur de la colonne était
//! relue telle quelle : figée. C'est corrigé depuis `7370d7d` (l'ancre
//! composite `(E_ref, t_ref, t_hi, H)` est la source de vérité).
//!
//! Ces tests s'attaquent au défaut RESTANT : le NON-DÉTERMINISME. L'ancien
//! test dormait vraiment (`thread::sleep`) et tolérait 3 % d'écart, soit
//! ~6 % d'erreur sur 2^(-2) ; il était intermittent. Ici l'horloge est
//! INJECTÉE (`EnergyStore::with_clock`, `Arc<AtomicI64>`) : fermeture et
//! réouverture se font à un instant EXACT, sans `thread::sleep`, et la
//! tolérance tombe à 1e-9.

#![cfg(feature = "substrate")]

use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

/// Époque de base en micros, loin de l'heure murale : 1e15 µs. Bien en deçà
/// de 2^53 (≈ 9,0e15) : le `t_ref` persisté en secondes epoch `f64` fait
/// l'aller-retour micros → secondes → micros SANS perte.
const BASE: i64 = 1_000_000_000_000_000;
/// Demi-vie d'UNE seconde : une demi-vie = 1e6 micros, exact en `f64`.
const HALF_LIFE: Duration = Duration::from_secs(1);

fn config() -> EnergyConfig {
    EnergyConfig {
        default_half_life: HALF_LIFE,
        ..Default::default()
    }
}

/// Horloge partagée : un `Arc<AtomicI64>` (écrit par le test) fermé dans une
/// clôture `Arc<dyn Fn() -> i64 + Send + Sync>` (lue par `EnergyStore`).
fn horloge() -> (Arc<AtomicI64>, Arc<dyn Fn() -> i64 + Send + Sync>) {
    let t = Arc::new(AtomicI64::new(BASE));
    let f: Arc<dyn Fn() -> i64 + Send + Sync> = {
        let t = Arc::clone(&t);
        Arc::new(move || t.load(Ordering::Relaxed))
    };
    (t, f)
}

/// Ouvre une base, booste un nœud à 1.0, ferme proprement, avance l'horloge
/// injectée de `ndemi` demi-vies, rouvre et lit. Le VRAI
/// `SubstrateStore::create/close/open` est exercé à chaque étape (couverture
/// du stockage, pas la fonction pure).
fn energie_apres(ndemi: u32) -> f64 {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kb");
    let (clock, clock_fn) = horloge();

    let node_id;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        node_id = sub.create_node(&["n"]);
        sub.flush().unwrap();
        let store = EnergyStore::with_substrate(config(), sub.clone()).with_clock(clock_fn.clone());
        store.boost(node_id, 1.0);
        let e_boost = store.get_energy(node_id);
        assert!(
            (e_boost - 1.0).abs() < 1e-9,
            "juste après le boost, énergie = {e_boost} (attendu 1.0)"
        );
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // Aucun `thread::sleep` : on déplace l'horloge injectée.
    clock.store(
        BASE + ndemi as i64 * HALF_LIFE.as_micros() as i64,
        Ordering::Relaxed,
    );

    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let store = EnergyStore::with_substrate(config(), sub.clone()).with_clock(clock_fn.clone());
    store.get_energy(node_id)
}

/// (a) Fermer/rouvrir au MÊME instant → énergie inchangée.
#[test]
fn fermer_rouvrir_au_meme_instant_ne_change_rien() {
    let e = energie_apres(0);
    assert!(
        (e - 1.0).abs() < 1e-9,
        "même instant : énergie = {e}, attendu 1.0"
    );
}

/// (b) Rouvrir après +1 demi-vie → 0,5.
#[test]
fn rouvrir_apres_une_demi_vie_donne_un_demi() {
    let e = energie_apres(1);
    assert!(
        (e - 0.5).abs() < 1e-9,
        "après 1 demi-vie : énergie = {e}, attendu 0.5"
    );
}

/// (c) Après +2 demi-vies → 0,25.
#[test]
fn rouvrir_apres_deux_demi_vies_donne_un_quart() {
    let e = energie_apres(2);
    assert!(
        (e - 0.25).abs() < 1e-9,
        "après 2 demi-vies : énergie = {e}, attendu 0.25"
    );
}
