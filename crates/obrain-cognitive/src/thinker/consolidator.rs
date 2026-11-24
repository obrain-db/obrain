//! # Consolidator — décroissance d'énergie (loi physique) + (à venir) élagage
//!
//! Le Consolidator est le penseur d'hygiène mémoire. À chaque tique il :
//!
//! 1. **Fait vieillir l'énergie** de chaque nœud le long de SA propre ancre
//!    physique persistée (`2^(-Δt/H)`), en bornant le travail par tique.
//! 2. *(à venir)* élague les synapses dont le poids est passé sous le seuil.
//! 3. *(à venir)* renforce les engrames chauds.
//!
//! ## Ce qui a changé le 2026-11-24, et pourquoi
//!
//! La version précédente était un **stub** : elle n'appelait rien et se
//! contentait d'incrémenter un compteur. La câbler « telle quelle » avec son
//! `decay_factor = 0.99` toutes les 60 s aurait donné une demi-vie effective de
//! 69 min au lieu de 24 h — **20,9× trop rapide** — c'est-à-dire un graphe dont
//! toute l'énergie disparaît en une heure.
//!
//! Il n'y a donc plus de `decay_factor` dans la configuration : la cadence est
//! **dérivée** de la demi-vie (`H / TICKS_PER_HALF_LIFE`, soit `H/64`), et le
//! facteur appliqué par tique est `2^(-1/64)`. Mieux : la tique n'applique pas
//! un facteur unique à la colonne, elle avance chaque nœud depuis SON ancre.
//! Conséquence utile — une tique manquée, un redémarrage ou une machine
//! éteinte une semaine ne perdent rien : la tique suivante voit un `Δt` plus
//! grand et la loi de semi-groupe fait le reste. Voir
//! [`crate::decay_clock::advance_all`] et [`crate::decay_law`].

use std::sync::Arc;
use std::time::Duration;

use obrain_substrate::SubstrateStore;

use super::{Thinker, ThinkerBudget, ThinkerKind, ThinkerTickError, ThinkerTickReport};
use crate::decay_clock::{self, DecayAdvance};
use crate::decay_law::{DecayLaw, unix_micros};

/// Configuration for the [`Consolidator`] — resolved from
/// `HubConfig.substrate.thinkers.consolidator` at Thinker construction.
#[derive(Debug, Clone)]
pub struct ConsolidatorConfig {
    /// Demi-vie de l'énergie. **La cadence en est dérivée** (voir
    /// [`Self::interval`]) : il n'existe plus de facteur de décroissance fixe,
    /// parce qu'un facteur fixe n'exprime pas une demi-vie.
    pub half_life: Duration,
    /// Nombre maximal de nœuds avancés par tique (`0` = pas de borne).
    ///
    /// Les nœuds non traités gardent leur ancre et sont rattrapés à la tique
    /// suivante, sans erreur : c'est ce qui permet de borner le coût d'une
    /// tique sans fausser la loi.
    pub max_nodes_per_tick: usize,
    /// Synapse weight threshold (Q1.15 u16) below which an edge is
    /// pruned. `0` disables pruning.
    pub synapse_prune_threshold_u16: u16,
    /// Energy boost applied to members of an engram whose recall
    /// count exceeded the hot threshold. Expressed in Q1.15 u16.
    pub engram_reinforce_delta_u16: u16,
    /// Budget & cadence.
    pub interval: Duration,
    /// CPU-fraction and per-tick duration budget enforced by the runtime.
    pub budget: ThinkerBudget,
}

impl Default for ConsolidatorConfig {
    fn default() -> Self {
        let half_life = Duration::from_secs(24 * 3600);
        Self {
            half_life,
            max_nodes_per_tick: 20_000,
            synapse_prune_threshold_u16: 64, // ≈ 0.002 in Q1.15
            engram_reinforce_delta_u16: 512, // ≈ 0.016 in Q1.15
            // DÉRIVÉE, pas choisie : une tique par 1/64 de demi-vie.
            interval: DecayLaw::new(half_life).tick_interval(),
            budget: ThinkerBudget::new(0.15, Duration::from_millis(500)),
        }
    }
}

impl ConsolidatorConfig {
    /// La loi que ce penseur applique.
    pub fn law(&self) -> DecayLaw {
        DecayLaw::new(self.half_life)
    }

    /// Facteur appliqué par tique — `2^(-(H/64)/H)` = `2^(-1/64)` ≈ 0,98925.
    ///
    /// Ce n'est pas un réglage : c'est la conséquence de `half_life` et de
    /// [`TICKS_PER_HALF_LIFE`](crate::decay_law::TICKS_PER_HALF_LIFE). Le test `decay_law::tests::
    /// un_facteur_fixe_est_refuse` vérifie que le facteur fixe historique
    /// (0,99 / 60 s) est bien refusé comme représentation d'une demi-vie de
    /// 24 h.
    pub fn tick_factor(&self) -> f64 {
        self.law().tick_factor()
    }
}

/// Accumulated stats since the Thinker started. Read by the runtime to
/// produce `/metrics` output.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConsolidatorStats {
    /// Total ticks executed since the Thinker started.
    pub ticks: u64,
    /// Total nodes whose energy was decayed across all ticks.
    pub nodes_decayed: u64,
    /// Slots vivants examinés à la dernière tique.
    pub last_scanned: u64,
    /// Nœuds reportés à la tique suivante (budget), à la dernière tique.
    pub last_deferred: u64,
    /// Total weak `SYNAPSE` edges pruned across all ticks.
    pub synapses_pruned: u64,
    /// Total hot engrams whose member nodes received an energy boost.
    pub engrams_reinforced: u64,
    /// Wall-clock duration of the most recent tick, in milliseconds.
    pub last_tick_elapsed_ms: u64,
}

/// Energy decay + synapse pruning + engram reinforcement Thinker.
pub struct Consolidator {
    config: ConsolidatorConfig,
    /// Horloge injectable (tests déterministes / simulation) : micros Unix.
    /// Par défaut [`crate::decay_law::unix_micros`]. Même abstraction que
    /// [`crate::energy::EnergyStore`] : unifie la source de temps du penseur.
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    // Stats are written only by the Thinker thread but read by callers
    // using AtomicU64 would require heavier plumbing. Keep a parking_lot
    // mutex for now — tick is infrequent, contention is irrelevant.
    stats: parking_lot::Mutex<ConsolidatorStats>,
}

impl Consolidator {
    /// Builds a Consolidator with the given configuration and zeroed stats.
    pub fn new(config: ConsolidatorConfig) -> Self {
        Self {
            config,
            clock: Arc::new(unix_micros),
            stats: parking_lot::Mutex::new(ConsolidatorStats::default()),
        }
    }

    /// Returns a snapshot of the accumulated stats.
    pub fn stats(&self) -> ConsolidatorStats {
        *self.stats.lock()
    }

    /// Injecte une horloge (tests déterministes / simulation). Par défaut
    /// [`crate::decay_law::unix_micros`]. Même builder que
    /// [`crate::energy::EnergyStore::with_clock`] : une horloge figée rend la
    /// tique reproductible sans toucher à la vraie montre.
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    /// Instant brut de l'horloge injectée, en micros Unix.
    pub fn now_micros(&self) -> i64 {
        (self.clock)()
    }

    /// Une tique : avance la colonne d'énergie le long des ancres physiques,
    /// jusqu'à `now`, sans dépasser `max_nodes_per_tick`.
    fn tick_impl(
        &self,
        store: &Arc<SubstrateStore>,
        now_micros: i64,
    ) -> Result<(ThinkerTickReport, DecayAdvance), ThinkerTickError> {
        let r = ThinkerTickReport::start();
        let law = self.config.law();
        let avance =
            decay_clock::advance_all(store, &law, self.config.max_nodes_per_tick, now_micros)?;
        let mut report = r;
        report.nodes_touched = avance.advanced;
        report.side_counter = avance.deferred;
        Ok((report.finish(), avance))
    }
}

impl Thinker for Consolidator {
    fn kind(&self) -> ThinkerKind {
        ThinkerKind::Consolidator
    }
    fn budget(&self) -> ThinkerBudget {
        self.config.budget
    }
    /// Cadence = `half_life / 64`, dérivée par
    /// [`DecayLaw::tick_interval`]. Changer la demi-vie change la cadence.
    fn interval(&self) -> Duration {
        self.config.interval
    }
    fn tick(&self, store: &Arc<SubstrateStore>) -> Result<ThinkerTickReport, ThinkerTickError> {
        // L'instant vient de l'horloge injectée (défaut `unix_micros`), pas
        // d'un appel direct à la montre : les tests peuvent figer le temps.
        // La monotonie INTER-PROCESSUS ne repose pas ici sur un high-water
        // mark de session : `advance_all` persiste `t_hi` dans l'ancre à
        // chaque matérialisation, et c'est ce `t_hi` durable qui borne les
        // reculs d'horloge à travers un redémarrage (voir `decay_law`).
        let (report, avance) = self.tick_impl(store, self.now_micros())?;
        let mut s = self.stats.lock();
        s.ticks += 1;
        s.nodes_decayed += avance.advanced;
        s.last_scanned = avance.scanned;
        s.last_deferred = avance.deferred;
        s.last_tick_elapsed_ms = report.elapsed.as_millis() as u64;
        drop(s);
        tracing::debug!(
            scanned = avance.scanned,
            advanced = avance.advanced,
            dormant = avance.dormant,
            deferred = avance.deferred,
            unanchored = avance.unanchored,
            half_life = ?self.config.half_life,
            "consolidator: énergie avancée"
        );
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decay_law::TICKS_PER_HALF_LIFE;
    use obrain_core::graph::GraphStoreMut;
    use obrain_substrate::store::SubstrateStore;
    use tempfile::TempDir;

    #[test]
    fn tick_increments_stats_and_reports_elapsed() {
        let td = TempDir::new().unwrap();
        let store = Arc::new(SubstrateStore::create(td.path().join("c-test")).unwrap());
        let c = Consolidator::new(ConsolidatorConfig::default());
        let r = c.tick(&store).expect("tick ok");
        let s = c.stats();
        assert_eq!(s.ticks, 1);
        // Base vide : aucun slot vivant, donc aucun nœud touché. Ce n'est plus
        // un stub — c'est le compte réel de ce qu'il y avait à faire.
        assert_eq!(r.nodes_touched, 0);
        assert_eq!(s.last_scanned, 0);
    }

    /// La tique applique la LOI : un nœud ancré une demi-vie dans le passé est
    /// divisé par deux, un nœud ancré deux demi-vies dans le passé par quatre.
    /// C'est le test qui refuse le facteur fixe : avec `0.99` par tique il
    /// faudrait 69 tiques pour diviser par deux, ici une seule suffit.
    #[test]
    fn une_tique_applique_la_loi_depuis_l_ancre() {
        let td = TempDir::new().unwrap();
        let store = Arc::new(SubstrateStore::create(td.path().join("c-loi")).unwrap());
        let n1 = store.create_node(&["n"]);
        let n2 = store.create_node(&["n"]);
        store.set_node_energy_f32(n1, 1.0).unwrap();
        store.set_node_energy_f32(n2, 1.0).unwrap();

        let half = Duration::from_secs(3600);
        let maintenant = unix_micros();
        // n1 : ancre à now − H ; n2 : ancre à now − 2H.
        decay_clock::persist_anchor(store.as_ref(), n1, maintenant - half.as_micros() as i64);
        decay_clock::persist_anchor(store.as_ref(), n2, maintenant - 2 * half.as_micros() as i64);

        let config = ConsolidatorConfig {
            half_life: half,
            ..Default::default()
        };
        let c = Consolidator::new(config);
        let r = c.tick(&store).expect("tick ok");
        assert_eq!(r.nodes_touched, 2, "les deux nœuds porteurs d'énergie");

        let e1 = crate::decay_clock::current_energy(&store, n1, &c.config.law()).unwrap();
        let e2 = crate::decay_clock::current_energy(&store, n2, &c.config.law()).unwrap();
        assert!((e1 - 0.5).abs() < 0.02, "une demi-vie → 0.5, mesuré {e1}");
        assert!(
            (e2 - 0.25).abs() < 0.02,
            "deux demi-vies → 0.25, mesuré {e2}"
        );
    }

    /// Une tique ne peut pas dépasser son budget : les nœuds non traités
    /// gardent leur ancre et sont rattrapés plus tard, sans erreur.
    #[test]
    fn le_budget_borne_la_tique_sans_perdre_le_temps() {
        let td = TempDir::new().unwrap();
        let store = Arc::new(SubstrateStore::create(td.path().join("c-budget")).unwrap());
        let mut ids = Vec::new();
        for _ in 0..5 {
            let id = store.create_node(&["n"]);
            store.set_node_energy_f32(id, 1.0).unwrap();
            ids.push(id);
        }
        let half = Duration::from_secs(3600);
        let maintenant = unix_micros();
        for id in &ids {
            decay_clock::persist_anchor(store.as_ref(), *id, maintenant - half.as_micros() as i64);
        }
        let c = Consolidator::new(ConsolidatorConfig {
            half_life: half,
            max_nodes_per_tick: 2,
            ..Default::default()
        });
        let r1 = c.tick(&store).unwrap();
        assert_eq!(r1.nodes_touched, 2);
        assert_eq!(r1.side_counter, 3, "trois nœuds reportés");

        // Les tiques suivantes rattrapent, et la valeur reste juste.
        c.tick(&store).unwrap();
        c.tick(&store).unwrap();
        let law = c.config.law();
        for id in &ids {
            let e = crate::decay_clock::current_energy(&store, *id, &law).unwrap();
            assert!((e - 0.5).abs() < 0.02, "nœud {id:?} : {e} (attendu 0.5)");
        }
    }

    #[test]
    fn default_config_has_sensible_values() {
        let c = ConsolidatorConfig::default();
        // La cadence est DÉRIVÉE de la demi-vie, pas un réglage indépendant.
        assert_eq!(c.interval, c.half_life / TICKS_PER_HALF_LIFE);
        assert!(c.interval >= Duration::from_secs(60));
        // Le facteur par tique est celui que la loi impose…
        let f = c.tick_factor();
        assert!(f > 0.98 && f < 1.0, "facteur par tique = {f}");
        // …et il redonne EXACTEMENT la demi-vie configurée.
        let h = DecayLaw::implied_half_life(f, c.interval);
        assert!(
            (h.as_secs_f64() - c.half_life.as_secs_f64()).abs() < 1.0,
            "la cadence implique {h:?} au lieu de {:?}",
            c.half_life
        );
    }
}
