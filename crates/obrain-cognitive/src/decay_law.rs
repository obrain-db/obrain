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
//!
//! ## L'ancre : `(E_ref, t_ref, t_hi, H)`
//!
//! | Pièce | Où elle vit | Unité |
//! |---|---|---|
//! | `E_ref` | l'ancre composite (propriété unique) | `[0, 1]` |
//! | `t_ref` | l'ancre composite | secondes epoch (`f64`) |
//! | `t_hi`  | l'ancre composite (high-water mark) | secondes epoch (`f64`) |
//! | `H` (loi) | l'ancre composite (FIGÉE) | durée |
//!
//! L'ensemble vit dans UNE SEULE propriété (voir `crate::decay_clock`) : c'est
//! ce qui rend l'écriture de l'état temporel **atomique** — une propriété
//! s'écrit en un enregistrement, jamais « énergie puis ancre » en deux temps.
//!
//! ## Monotonie (high-water mark)
//!
//! `SystemTime` n'est pas monotone : une correction NTP ou un changement
//! d'heure peut le faire reculer. Borner `Δt` à `[0, …]` n'empêche PAS la
//! remontée : ancre fixe, lecture à `t=24 h` → 0,5 ; si l'horloge revient à
//! `t=12 h`, les deux `Δt` sont positifs et la lecture donne 0,7071. Le clamp
//! ne s'active jamais. `t_hi` est donc un **high-water mark persisté** : la
//! lecture utilise `now_eff = max(now, t_hi)`, si bien qu'un recul d'horloge
//! ne rend jamais plus que ce qui a déjà été observé.
//!
//! ## Limite du contrat à travers un redémarrage
//!
//! `t_hi` n'est écrit que par les chemins d'ÉCRITURE (`reinforce` /
//! `advance_all`). Une LECTURE seule d'un instant plus grand fait monter le
//! high-water mark DE SESSION (`session_hi`, en mémoire) mais ne persiste
//! rien. Après un crash suivi d'un recul d'horloge, on repart donc sur le
//! dernier `t_hi` durable : l'énergie peut remonter vers la valeur qu'implique
//! cette dernière borne. Limitation documentée du contrat, pas une corruption.
//!
//! ## H est FIGÉE dans l'ancre
//!
//! Si `H` ne venait que de la configuration courante, rouvrir avec une autre
//! valeur recalculerait rétroactivement toute la période écoulée. On fige donc
//! `H` dans l'ancre : un changement de configuration ne s'applique qu'aux
//! ancrages SUIVANTS, jamais au passé.

use std::time::Duration;

/// Nombre de tiques par demi-vie. 64 donne un facteur de 2^(-1/64) ≈ 0,98925.
pub const TICKS_PER_HALF_LIFE: u32 = 64;

/// Micros Unix de l'instant courant — la même origine que l'horodatage du WAL.
pub fn unix_micros() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as i64)
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

/// Écoulement MONOTONE entre une ancre et `now`, borné à `[0, ∞)`.
///
/// `t_hi` est le high-water mark persisté : `now` effectif vaut `max(now,
/// t_hi)`. Une horloge qui recule ne produit donc jamais un `Δt` plus petit
/// que le plus grand déjà observé — l'énergie ne remonte pas. Une ancre dans
/// le futur donne zéro (on ne rend jamais plus que la valeur stockée).
pub fn elapsed_monotonic(t_ref_micros: i64, now_micros: i64, t_hi_micros: i64) -> Duration {
    let now_eff = now_micros.max(t_hi_micros);
    let dt = now_eff.saturating_sub(t_ref_micros);
    if dt <= 0 {
        return Duration::ZERO;
    }
    Duration::from_micros(dt as u64)
}

/// Écoulement entre une ancre et `now`, borné à `[0, ∞)` (pas de high-water
/// mark : `t_hi = now`). Conservé pour les chemins sans ancre composite.
pub fn elapsed_since(t_ref_micros: i64, now_micros: i64) -> Duration {
    elapsed_monotonic(t_ref_micros, now_micros, now_micros)
}

/// Loi de décroissance exponentielle de demi-vie `H`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecayLaw {
    half_life: Duration,
}

impl DecayLaw {
    /// Construit une loi. Une demi-vie nulle, négative ou non finie est
    /// remplacée par 24 h plutôt que de produire une division par zéro.
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
    ///
    /// Pas de borne haute de `Δt` : pour `Δt` grand, `2^(-Δt/H)` sous-passe à
    /// `0.0` en `f64` — exactement la limite de la loi, pas un plafond imposé.
    pub fn factor(&self, elapsed: Duration) -> f64 {
        let dt = elapsed.as_secs_f64();
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
    /// `H = Δt · ln2 / −ln(factor)`.
    pub fn implied_half_life(factor: f64, elapsed: Duration) -> Duration {
        let dt = elapsed.as_secs_f64();
        // partial_cmp : préserve la sémantique NaN (None ≠ Some → invalide).
        if factor.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
            || factor.partial_cmp(&1.0) != Some(std::cmp::Ordering::Less)
            || dt.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
        {
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

    /// Nombre de demi-vies pour passer de `e_ref` à `threshold`.
    pub fn half_lives_to(&self, e_ref: f64, threshold: f64) -> f64 {
        if e_ref <= 0.0 || threshold <= 0.0 || e_ref <= threshold {
            return 0.0;
        }
        (e_ref / threshold).log2()
    }

    /// Durée pour passer de `e_ref` à `threshold`.
    pub fn elapsed_to_reach(&self, e_ref: f64, threshold: f64) -> Duration {
        Duration::from_secs_f64(self.half_life.as_secs_f64() * self.half_lives_to(e_ref, threshold))
    }
}

/// Ancre persistée d'un nœud : `(E_ref, t_ref, t_hi, H)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyAnchor {
    /// Énergie au moment de référence.
    pub e_ref: f64,
    /// Instant de référence, micros Unix.
    pub t_ref_micros: i64,
    /// High-water mark monotone, micros Unix (jamais ne recule).
    pub t_hi_micros: i64,
    /// Demi-vie FIGÉE dans l'ancre.
    pub half_life: Duration,
}

impl EnergyAnchor {
    /// Construit une ancre (le high-water mark part de `t_ref`).
    pub fn new(e_ref: f64, t_ref_micros: i64, half_life: Duration) -> Self {
        Self {
            e_ref,
            t_ref_micros,
            t_hi_micros: t_ref_micros,
            half_life,
        }
    }

    /// Énergie de cette ancre à `now`, selon SA demi-vie figée et le
    /// high-water mark (monotonie).
    pub fn energy_at(&self, now_micros: i64) -> f64 {
        let law = DecayLaw::new(self.half_life);
        law.energy_at(
            self.e_ref,
            elapsed_monotonic(self.t_ref_micros, now_micros, self.t_hi_micros),
        )
    }

    /// Énergie actuelle.
    pub fn energy_now(&self) -> f64 {
        self.energy_at(unix_micros())
    }

    /// L'ancre avancée à `now` pour une énergie `e` (matérialisation). La
    /// demi-vie est conservée, le high-water mark ne recule jamais.
    pub fn materialized(&self, e: f64, now_micros: i64) -> Self {
        Self {
            e_ref: e,
            t_ref_micros: now_micros,
            t_hi_micros: self.t_hi_micros.max(now_micros),
            half_life: self.half_life,
        }
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
        let a = law.factor(Duration::from_secs(900)) * law.factor(Duration::from_secs(900));
        let b = law.factor(Duration::from_secs(1800));
        assert!((a - b).abs() < 1e-12);
    }

    /// Point 4 — pas de plafond à 365 jours : au-delà de quelques demi-vies,
    /// l'énergie TEND vers zéro (la loi), au lieu de se figer.
    #[test]
    fn au_dela_de_365_jours_l_energie_tend_vers_zero() {
        let law = DecayLaw::new(Duration::from_secs(3600));
        let un_an = Duration::from_secs(365 * 24 * 3600);
        let dix_ans = Duration::from_secs(10 * 365 * 24 * 3600);
        let e_un_an = law.energy_at(1.0, un_an);
        let e_dix_ans = law.energy_at(1.0, dix_ans);
        // 365 jours = 8760 demi-vies : 2^-8760 est dénormal, puis 0.
        assert!(e_un_an < 1e-100, "1 an d'une demi-vie d'1 h : {e_un_an}");
        assert_eq!(e_dix_ans, 0.0, "10 ans → exactement 0 : {e_dix_ans}");
        // Aucun débordement de Duration : un Δt énorme reste fini.
        let enorme = Duration::from_micros(i64::MAX as u64);
        assert_eq!(law.energy_at(1.0, enorme), 0.0);
    }

    /// Point 2 — le high-water mark empêche la remontée : une horloge qui
    /// recule ne rend jamais plus que ce qui a déjà été observé.
    #[test]
    fn le_high_water_mark_empeche_la_remontee() {
        let h = Duration::from_secs(24 * 3600);
        let ancre = EnergyAnchor::new(1.0, 0, h);
        // Lecture à t = 24 h : 0,5.
        let a_24h = ancre.energy_at(24 * 3600 * 1_000_000);
        assert!((a_24h - 0.5).abs() < 1e-12);
        // L'horloge revient à t = 12 h SANS high-water mark : remontée.
        let sans_garde = EnergyAnchor::new(1.0, 0, h).energy_at(12 * 3600 * 1_000_000);
        assert!(
            (sans_garde - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "remontée reproduite : {sans_garde}"
        );
        // AVEC high-water mark (t_hi = 24 h persisté) : pas de remontée.
        let gardee = EnergyAnchor {
            t_hi_micros: 24 * 3600 * 1_000_000,
            ..ancre
        };
        let a_12h_garde = gardee.energy_at(12 * 3600 * 1_000_000);
        assert!(
            (a_12h_garde - 0.5).abs() < 1e-12,
            "recul d'horloge avec garde : {a_12h_garde} (attendu 0.5)"
        );
    }

    /// Une ancre future ou une horloge corrigée en arrière ne rend jamais plus
    /// que la valeur stockée.
    #[test]
    fn une_ancre_dans_le_futur_ne_gonfle_pas_l_energie() {
        let h = Duration::from_secs(3600);
        let ancre = EnergyAnchor::new(0.5, 1_000_000, h);
        assert_eq!(ancre.energy_at(900_000), 0.5);
        assert_eq!(ancre.energy_at(1_000_000), 0.5);
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

#[cfg(test)]
mod tests_branches_limites {
    use super::*;

    /// Une ancre non finie (NaN, ±∞) n'a pas d'instant : convertie en 0,
    /// jamais propagée dans une loi qui produirait du NaN.
    #[test]
    fn une_ancre_non_finie_est_convertie_en_zero() {
        assert_eq!(epoch_secs_to_micros(f64::NAN), 0);
        assert_eq!(epoch_secs_to_micros(f64::INFINITY), 0);
        assert_eq!(epoch_secs_to_micros(f64::NEG_INFINITY), 0);
    }

    /// Une demi-vie nulle (division par zéro sinon) retombe sur le défaut
    /// 24 h au lieu de produire une loi dégénérée.
    #[test]
    fn une_demi_vie_nulle_retombe_sur_24h() {
        let law = DecayLaw::new(Duration::ZERO);
        assert_eq!(law.half_life(), Duration::from_secs(24 * 3600));
    }

    /// Une énergie de référence aberrante (NaN, nulle, négative) ne doit pas
    /// franchir la loi : zéro, sans fabriquer de NaN.
    #[test]
    fn une_energie_de_reference_aberrante_rend_zero() {
        let law = DecayLaw::new(Duration::from_secs(3600));
        assert_eq!(law.energy_at(f64::NAN, Duration::ZERO), 0.0);
        assert_eq!(law.energy_at(0.0, Duration::ZERO), 0.0);
        assert_eq!(law.energy_at(-1.0, Duration::ZERO), 0.0);
    }

    /// Une demi-vie impliquée invalide (facteur hors de (0,1), ou période
    /// nulle) ne fabrique pas de durée : zéro, pour ne pas propager NaN.
    #[test]
    fn une_demi_vie_impliquee_invalide_rend_zero() {
        assert_eq!(
            DecayLaw::implied_half_life(0.0, Duration::from_secs(60)),
            Duration::ZERO
        );
        assert_eq!(
            DecayLaw::implied_half_life(1.0, Duration::from_secs(60)),
            Duration::ZERO
        );
        assert_eq!(
            DecayLaw::implied_half_life(f64::NAN, Duration::from_secs(60)),
            Duration::ZERO
        );
        assert_eq!(
            DecayLaw::implied_half_life(0.5, Duration::ZERO),
            Duration::ZERO
        );
    }

    /// Un nombre de demi-vies incohérent (référence nulle, seuil nul, ou déjà
    /// sous le seuil) rend zéro au lieu de `log2` d'un rapport invalide.
    #[test]
    fn un_nombre_de_demi_vies_invalide_rend_zero() {
        let law = DecayLaw::new(Duration::from_secs(3600));
        assert_eq!(law.half_lives_to(0.0, 0.5), 0.0);
        assert_eq!(law.half_lives_to(1.0, 0.0), 0.0);
        assert_eq!(law.half_lives_to(0.5, 1.0), 0.0);
    }

    /// `energy_now` lit l'ancre à l'instant courant : une ancre posée
    /// « maintenant » avec une énergie 1,0 se relit ≈ 1,0.
    #[test]
    fn energy_now_lit_l_ancre_a_l_instant_courant() {
        let h = Duration::from_secs(3600);
        let ancre = EnergyAnchor::new(1.0, unix_micros(), h);
        assert!(
            (ancre.energy_now() - 1.0).abs() < 1e-9,
            "ancre fraîche = 1,0"
        );
    }

    /// `materialized` avance l'ancre à `now` (t_ref = now) et conserve la
    /// demi-vie, sans jamais faire RECULER le high-water mark.
    #[test]
    fn materialized_avance_l_ancre_sans_faire_reculer_le_high_water_mark() {
        let h = Duration::from_secs(3600);
        let ancre = EnergyAnchor::new(1.0, 0, h);
        let now = 2 * 3600 * 1_000_000;
        let avancee = ancre.materialized(0.25, now);
        assert_eq!(avancee.e_ref, 0.25);
        assert_eq!(avancee.t_ref_micros, now);
        assert_eq!(avancee.t_hi_micros, now);
        assert_eq!(avancee.half_life, h);

        // Le high-water mark ne recule jamais, même si `now` est antérieur.
        let gardee = EnergyAnchor {
            t_hi_micros: now + 1,
            ..ancre
        };
        let avancee2 = gardee.materialized(0.5, now);
        assert_eq!(avancee2.t_hi_micros, now + 1);
    }
}
