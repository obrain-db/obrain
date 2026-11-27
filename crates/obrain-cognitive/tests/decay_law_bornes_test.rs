//! Bornes de la loi de décroissance — compléments de couverture (#414, périmètre C).
//!
//! Couvre les chemins restés non couverts de `decay_law` : `elapsed_since`,
//! les gardes de `energy_at` / `implied_half_life` (bornes),
//! `half_lives_to` / `elapsed_to_reach`, `EnergyAnchor::energy_now`
//! et `EnergyAnchor::materialized`.

use obrain_cognitive::decay_law::{DecayLaw, EnergyAnchor, elapsed_since, unix_micros};
use std::time::Duration;

const H: Duration = Duration::from_secs(3600);

#[test]
fn elapsed_since_mesure_la_difference_sans_high_water_mark() {
    assert_eq!(
        elapsed_since(1_000_000, 2_000_000),
        Duration::from_micros(1_000_000)
    );
    // Ancre dans le futur → zéro (jamais de remontée).
    assert_eq!(elapsed_since(2_000_000, 1_000_000), Duration::ZERO);
    assert_eq!(elapsed_since(0, 0), Duration::ZERO);
}

#[test]
fn energy_at_refuse_une_energie_de_reference_aberrante() {
    let law = DecayLaw::new(H);
    // Non fini ou ≤ 0 → 0, sans fabriquer de NaN.
    assert_eq!(law.energy_at(f64::NAN, Duration::ZERO), 0.0);
    assert_eq!(law.energy_at(f64::INFINITY, Duration::ZERO), 0.0);
    assert_eq!(law.energy_at(f64::NEG_INFINITY, Duration::ZERO), 0.0);
    assert_eq!(law.energy_at(0.0, Duration::ZERO), 0.0);
    assert_eq!(law.energy_at(-1.0, Duration::ZERO), 0.0);
}

#[test]
fn implied_half_life_refuse_un_facteur_hors_de_l_intervalle_ouvert() {
    // Facteur hors de (0, 1) → zéro.
    assert_eq!(DecayLaw::implied_half_life(0.0, H), Duration::ZERO);
    assert_eq!(DecayLaw::implied_half_life(-0.5, H), Duration::ZERO);
    assert_eq!(DecayLaw::implied_half_life(1.0, H), Duration::ZERO);
    assert_eq!(DecayLaw::implied_half_life(1.5, H), Duration::ZERO);
    assert_eq!(DecayLaw::implied_half_life(f64::NAN, H), Duration::ZERO);
    // Période nulle → zéro.
    assert_eq!(
        DecayLaw::implied_half_life(0.5, Duration::ZERO),
        Duration::ZERO
    );
}

#[test]
fn implied_half_life_retrouve_la_demi_vie_d_un_facteur_arbitraire() {
    // Un facteur 0,25 par heure = 2 demi-vies par heure → H = 30 min.
    let h = DecayLaw::implied_half_life(0.25, Duration::from_secs(3600));
    assert!((h.as_secs_f64() - 1800.0).abs() < 1e-6, "H = {h:?}");
}

#[test]
fn half_lives_to_compte_les_demi_vies() {
    let law = DecayLaw::new(H);
    assert!((law.half_lives_to(1.0, 0.5) - 1.0).abs() < 1e-12);
    assert!((law.half_lives_to(1.0, 0.25) - 2.0).abs() < 1e-12);
    assert!((law.half_lives_to(8.0, 1.0) - 3.0).abs() < 1e-12);
}

#[test]
fn half_lives_to_refuse_un_rapport_incoherent() {
    let law = DecayLaw::new(H);
    assert_eq!(law.half_lives_to(0.0, 0.5), 0.0);
    assert_eq!(law.half_lives_to(1.0, 0.0), 0.0);
    assert_eq!(law.half_lives_to(-1.0, 0.5), 0.0);
    assert_eq!(law.half_lives_to(0.5, 1.0), 0.0);
    assert_eq!(law.half_lives_to(1.0, 1.0), 0.0);
}

#[test]
fn elapsed_to_reach_donne_la_duree_pour_atteindre_un_seuil() {
    let law = DecayLaw::new(H);
    assert!((law.elapsed_to_reach(1.0, 0.5).as_secs_f64() - 3600.0).abs() < 1e-9);
    assert!((law.elapsed_to_reach(1.0, 0.25).as_secs_f64() - 7200.0).abs() < 1e-9);
    // Seuil inatteignable (e_ref ≤ threshold) → zéro.
    assert_eq!(law.elapsed_to_reach(0.5, 1.0), Duration::ZERO);
}

#[test]
fn energy_now_lit_une_ancre_fraiche_a_environ_un() {
    let ancre = EnergyAnchor::new(1.0, unix_micros(), H);
    let e = ancre.energy_now();
    assert!((e - 1.0).abs() < 1e-9, "ancre fraîche = 1.0, lu {e}");
}

#[test]
fn materialized_avance_l_ancre_sans_faire_reculer_le_high_water_mark() {
    let ancre = EnergyAnchor::new(1.0, 0, H);
    let now = 2 * 3600 * 1_000_000i64;
    let avancee = ancre.materialized(0.25, now);
    assert!((avancee.e_ref - 0.25).abs() < 1e-12);
    assert_eq!(avancee.t_ref_micros, now);
    assert_eq!(avancee.t_hi_micros, now);
    assert_eq!(avancee.half_life, H);

    // Le high-water mark ne recule jamais, même si `now` est antérieur.
    let gardee = EnergyAnchor {
        t_hi_micros: now + 1,
        ..ancre
    };
    let avancee2 = gardee.materialized(0.5, now);
    assert_eq!(avancee2.t_hi_micros, now + 1);
    assert_eq!(avancee2.t_ref_micros, now);
    assert_eq!(avancee2.half_life, H);
}
