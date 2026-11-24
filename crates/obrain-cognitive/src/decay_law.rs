//! # Loi de décroissance et horloge physique
//!
//! L'énergie d'un nœud suit `E(t) = E_ref · 2^(-Δt / H)`. Pour que cette loi
//! reste VRAIE au redémarrage, `Δt` doit être mesuré sur une horloge qui
//! survit au processus.
//!
//! ## Pourquoi ce n'est pas un `web_time::Instant`
//!
//! `Instant` est **monotone** et **locale au processus** :
//!
//! - elle n'a pas d'époque : son origine est arbitraire (démarrage de la
//!   machine, ou du processus) et n'est comparable à rien d'autre ;
//! - elle n'est **pas sérialisable** : rien ne peut l'écrire dans une base ;
//! - elle **recule** à travers une mise en veille ou un changement de source
//!   (`CLOCK_MONOTONIC` ne compte pas la veille, `CLOCK_BOOTTIME` si).
//!
//! Une ancre écrite avec `Instant` est donc perdue à la mort du processus.
//! C'est exactement le défaut mesuré le 2026-11-24 : après fermeture et
//! réouverture de la base, l'énergie valait encore 1,0 au lieu de 0,25
//! (`tests/decay_persistance_test.rs`, rouge avant ce module).
//!
//! ## L'ancre : `(E_ref, t_ref, loi)`
//!
//! | Pièce | Où elle vit | Unité |
//! |---|---|---|
//! | `E_ref` | colonne `NodeRecord.energy` (Q1.15) du substrat | `[0, 1]` |
//! | `t_ref` | propriété `_cog_energy_last_activated_epoch` | secondes epoch (`f64`) |
//! | `H` (loi) | `EnergyConfig::default_half_life` | durée |
//!
//! `t_ref` est un **temps physique** : epoch Unix, même valeur pour tous les
//! processus, toutes les sessions et toutes les machines. C'est la même
//! origine que l'horodatage `timestamp: i64` (micros Unix) que le WAL porte
//! déjà sur chaque enregistrement (`obrain-substrate/src/writer.rs`,
//! `unix_micros()`), donc une ancre et une entrée de journal se comparent
//! directement. On garde ici des **micros** (`i64`, exacts) et on ne convertit
//! en secondes `f64` qu'au contact de la propriété — la résolution de `f64`
//! autour de 1,8e9 s est de ~0,5 µs, donc la conversion ne perd rien.
//!
//! L'epoch peut sauter en arrière (correction NTP, changement d'heure mal
//! réglé) : `Δt` est donc borné à `[0, MAX_ELAPSED]`, ce qui rend une ancre
//! future inoffensive (facteur 1) au lieu de faire exploser l'énergie.
//!
//! ## La cadence est DÉRIVÉE, jamais fixe
//!
//! Le câblage naïf — `factor = 0.99` toutes les 60 s — donne une demi-vie
//! effective de `60·ln2 / −ln(0,99)` = 69 min au lieu de 24 h, soit **20,9×
//! trop rapide**. Ici la tique vaut `H / TICKS_PER_HALF_LIFE` et son facteur
//! `2^(-1/64)`, ce qui redonne exactement `H` (voir
//! [`DecayLaw::implied_half_life`] et le test `un_facteur_fixe_est_refuse`).

use std::time::Duration;

/// Nombre de tiques par demi-vie. 64 donne un facteur de 2^(-1/64) ≈ 0,98925
/// et une erreur de matérialisation bornée à 1,1 % (une tique).
pub const TICKS_PER_HALF_LIFE: u32 = 64;

/// Borne haute de `Δt` : 365 jours. Au-delà, l'énergie est de toute façon
/// indistinguable de zéro, et une ancre aberrante ne peut pas faire déborder
/// un `Duration`.
pub const MAX_ELAPSED: Duration = Duration::from_secs(365 * 24 * 3600);

/// Micros Unix de l'instant courant — la même origine que l'horodatage du WAL.
pub fn unix_micros() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// Convertit une ancre (secondes epoch `f64`, telle que persistée) en micros.
pub fn epoch_secs_to_micros(secs: f64) -> i64 {
    if !secs.is_finite() {
        return 0;
    }
    (secs * 1_000_000.0).round() as i64
}

/// Convertit des micros Unix en secondes epoch `f64` (unité de la propriété).
pub fn micros_to_epoch_secs(micros: i64) -> f64 {
    micros as f64 / 1_000_000.0
}

/// Écoulement entre une ancre et `now`, borné à `[0, MAX_ELAPSED]`.
///
/// Une ancre dans le futur (horloge corrigée en arrière) donne zéro : on ne
/// rend jamais d'énergie supérieure à la valeur stockée.
pub fn elapsed_since(t_ref_micros: i64, now_micros: i64) -> Duration {
    let dt = now_micros.saturating_sub(t_ref_micros);
    if dt <= 0 {
        return Duration::ZERO;
    }
    Duration::from_micros(dt as u64).min(MAX_ELAPSED)
}

/// Loi de décroissance exponentielle de demi-vie `H`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecayLaw {
    half_life: Duration,
}

impl DecayLaw {
    /// Construit une loi. Une demi-vie nulle, négative ou non finie est
    /// remplacée par 24 h (le défaut de [`crate::energy::EnergyConfig`]) plutôt
    /// que de produire une division par zéro silencieuse.
    pub fn new(half_life: Duration) -> Self {
        let secs = half_life.as_secs_f64();
        let h = if !secs.is_finite() || secs <= 0.0 {
            Duration::from_secs(24 * 3600)
        } else {
            half_life
        };
        Self { half_life: h }
    }

    /// Demi-vie `H`.
    pub fn half_life(&self) -> Duration {
        self.half_life
    }

    /// Facteur de décroissance `2^(-Δt / H)`, dans `(0, 1]`.
    pub fn factor(&self, elapsed: Duration) -> f64 {
        let dt = elapsed.min(MAX_ELAPSED).as_secs_f64();
        let f = 2.0_f64.powf(-dt / self.half_life.as_secs_f64());
        if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Énergie à `E_ref` après `elapsed` : `E_ref · 2^(-Δt/H)`.
    pub fn energy_at(&self, e_ref: f64, elapsed: Duration) -> f64 {
        if !e_ref.is_finite() || e_ref <= 0.0 {
            return 0.0;
        }
        e_ref * self.factor(elapsed)
    }

    /// Demi-vie qu'implique un `factor` appliqué toutes les `elapsed` :
    /// `H = Δt · ln2 / −ln(factor)`. C'est l'instrument qui REFUSE un facteur
    /// fixe : il rend la demi-vie qu'un `0.99` toutes les 60 s prétend avoir.
    pub fn implied_half_life(factor: f64, elapsed: Duration) -> Duration {
        let dt = elapsed.as_secs_f64();
        if !(factor > 0.0 && factor < 1.0) || !(dt > 0.0) {
            return Duration::from_secs(0);
        }
        let h = dt * std::f64::consts::LN_2 / -factor.ln();
        Duration::from_secs_f64(h)
    }

    /// Cadence de matérialisation dérivée de la demi-vie : `H / 64`.
    pub fn tick_interval(&self) -> Duration {
        self.half_life / TICKS_PER_HALF_LIFE
    }

    /// Facteur appliqué par tique : `2^(-1/64)`.
    pub fn tick_factor(&self) -> f64 {
        self.factor(self.tick_interval())
    }

    /// Nombre de demi-vies pour passer de `e_ref` à `threshold` :
    /// `log2(e_ref / threshold)`.
    pub fn half_lives_to(&self, e_ref: f64, threshold: f64) -> f64 {
        if e_ref <= 0.0 || threshold <= 0.0 || e_ref <= threshold {
            return 0.0;
        }
        (e_ref / threshold).log2()
    }

    /// Durée pour passer de `e_ref` à `threshold` : `H · log2(e_ref/threshold)`.
    pub fn elapsed_to_reach(&self, e_ref: f64, threshold: f64) -> Duration {
        Duration::from_secs_f64(self.half_life.as_secs_f64() * self.half_lives_to(e_ref, threshold))
    }
}

/// Ancre persistée d'un nœud : la valeur `e_ref` au temps `t_ref_micros`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyAnchor {
    /// Énergie au moment de référence (colonne Q1.15).
    pub e_ref: f64,
    /// Instant de référence, micros Unix.
    pub t_ref_micros: i64,
}

impl EnergyAnchor {
    /// Construit une ancre.
    pub fn new(e_ref: f64, t_ref_micros: i64) -> Self {
        Self {
            e_ref,
            t_ref_micros,
        }
    }

    /// Énergie de cette ancre à `now` selon `law`.
    pub fn energy_at(&self, law: &DecayLaw, now_micros: i64) -> f64 {
        law.energy_at(self.e_ref, elapsed_since(self.t_ref_micros, now_micros))
    }

    /// Énergie actuelle.
    pub fn energy_now(&self, law: &DecayLaw) -> f64 {
        self.energy_at(law, unix_micros())
    }

    /// L'ancre avancée à `now` pour une énergie `e` (matérialisation).
    pub fn advanced(e: f64, now_micros: i64) -> Self {
        Self::new(e, now_micros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le facteur fixe du stub est REFUSÉ : il implique 69 min là où la
    /// configuration annonce 24 h, soit 20,9× trop rapide.
    #[test]
    fn un_facteur_fixe_est_refuse() {
        let naif = DecayLaw::implied_half_life(0.99, Duration::from_secs(60));
        let minutes = naif.as_secs_f64() / 60.0;
        assert!(
            (minutes - 69.0).abs() < 1.0,
            "0,99 toutes les 60 s implique {minutes:.2} min de demi-vie"
        );
        let rapport = (24.0 * 60.0) / minutes;
        assert!(
            (rapport - 20.9).abs() < 0.5,
            "le facteur fixe est {rapport:.2}× trop rapide (attendu ≈ 20,9)"
        );
        // Et il ne peut pas se faire passer pour la loi de 24 h.
        assert!((naif.as_secs_f64() - 24.0 * 3600.0).abs() > 3600.0);
    }

    /// La cadence dérivée redonne EXACTEMENT la demi-vie configurée.
    #[test]
    fn la_cadence_derivee_redonne_la_demi_vie() {
        for h in [1u64, 60, 3600, 24 * 3600, 7 * 24 * 3600] {
            let law = DecayLaw::new(Duration::from_secs(h));
            assert_eq!(
                law.tick_interval(),
                Duration::from_secs(h) / TICKS_PER_HALF_LIFE
            );
            let retrouvee = DecayLaw::implied_half_life(law.tick_factor(), law.tick_interval());
            let err = (retrouvee.as_secs_f64() - h as f64).abs() / h as f64;
            assert!(
                err < 1e-9,
                "H={h} : retrouvée {retrouvee:?}, erreur relative {err}"
            );
        }
    }

    /// La loi est exacte aux points remarquables.
    #[test]
    fn la_loi_passe_par_les_points_remarquables() {
        let law = DecayLaw::new(Duration::from_secs(3600));
        assert!((law.energy_at(1.0, Duration::ZERO) - 1.0).abs() < 1e-12);
        assert!((law.energy_at(1.0, Duration::from_secs(3600)) - 0.5).abs() < 1e-12);
        assert!((law.energy_at(1.0, Duration::from_secs(7200)) - 0.25).abs() < 1e-12);
        // Deux demis pas valent un pas entier : la loi est un semi-groupe.
        let a = law.factor(Duration::from_secs(900)) * law.factor(Duration::from_secs(900));
        let b = law.factor(Duration::from_secs(1800));
        assert!((a - b).abs() < 1e-12);
    }

    /// Une ancre future ou une horloge corrigée en arrière ne rend jamais plus
    /// que la valeur stockée.
    #[test]
    fn une_ancre_dans_le_futur_ne_gonfle_pas_l_energie() {
        let law = DecayLaw::new(Duration::from_secs(3600));
        let ancre = EnergyAnchor::new(0.5, 1_000_000);
        assert_eq!(ancre.energy_at(&law, 900_000), 0.5);
        assert!(ancre.energy_at(&law, 1_000_000) == 0.5);
    }

    /// Conversion d'unités : secondes epoch `f64` ↔ micros, sans perte
    /// au-delà du micro.
    #[test]
    fn les_conversions_d_unites_sont_stables() {
        let micros = unix_micros();
        let secs = micros_to_epoch_secs(micros);
        assert!((epoch_secs_to_micros(secs) - micros).abs() <= 1);
    }
}
