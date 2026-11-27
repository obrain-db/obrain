//! Node energy system — exponential decay and activation boosting.
//!
//! Each node in the graph has an associated energy level that decays
//! exponentially over time: `E(t) = E0 × 2^(-Δt / half_life)`.
//!
//! Nodes are boosted (energy increased) whenever they are mutated,
//! queried, or otherwise "activated". Nodes whose energy falls below
//! a configurable threshold are candidates for archival or eviction.

use async_trait::async_trait;
use dashmap::DashMap;
use obrain_common::types::NodeId;
use obrain_reactive::{MutationEvent, MutationListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;
use web_time::Instant;

use crate::decay_law::{DecayLaw, epoch_secs_to_micros, unix_micros};
use crate::store_trait::{
    OptionalGraphStore, PROP_ENERGY, PROP_ENERGY_ANCHOR, PROP_ENERGY_LAST_ACTIVATED_EPOCH,
    epoch_to_instant, load_node_f64, now_epoch_secs, persist_node_f64,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the energy subsystem.
#[derive(Debug, Clone)]
pub struct EnergyConfig {
    /// Energy boost applied to nodes touched by a mutation (update, delete).
    pub boost_on_mutation: f64,
    /// (2026-11-12, plan 808fa9a7 T2) Boost appliqué à une CRÉATION (nœud ou
    /// arête). Distinct de `boost_on_mutation` pour qu'un import massif ne
    /// fasse pas de l'énergie un simple compteur de volume : MESURÉ sur le
    /// graphe PO, l'énergie « par mutation » corrèle au degré (Spearman
    /// 0,365) et pas à l'usage réel (−0,004). Défaut : égal à
    /// `boost_on_mutation` (comportement historique).
    pub boost_on_create: Option<f64>,
    /// Default energy for newly tracked nodes.
    pub default_energy: f64,
    /// Default half-life for energy decay.
    pub default_half_life: Duration,
    /// Minimum energy threshold — nodes below this are "low energy".
    pub min_energy: f64,
    /// Maximum energy cap — energy is clamped to `[0.0, max_energy]` after every operation.
    pub max_energy: f64,
    /// Reference energy for normalization: `energy_score = 1 - exp(-energy / ref_energy)`.
    /// Higher values spread the curve, lower values compress it.
    pub ref_energy: f64,
    /// Structural reinforcement coefficient (α).
    /// Modulates half-life by node degree: `effective_half_life = base * (1 + α * ln(1 + degree))`.
    /// Set to 0.0 to disable structural reinforcement.
    pub structural_reinforcement_alpha: f64,
}

impl Default for EnergyConfig {
    fn default() -> Self {
        Self {
            boost_on_mutation: 1.0,
            boost_on_create: None,
            default_energy: 1.0,
            default_half_life: Duration::from_secs(24 * 3600), // 24 hours
            min_energy: 0.01,
            max_energy: 10.0,
            ref_energy: 1.0,
            structural_reinforcement_alpha: 0.0,
        }
    }
}

impl EnergyConfig {
    /// Creates a config with custom half-life.
    pub fn with_half_life(mut self, half_life: Duration) -> Self {
        self.default_half_life = half_life;
        self
    }

    /// Creates a config with custom mutation boost.
    pub fn with_boost(mut self, boost: f64) -> Self {
        self.boost_on_mutation = boost;
        self
    }
}

// ---------------------------------------------------------------------------
// NodeEnergy
// ---------------------------------------------------------------------------

/// Energy state for a single node.
///
/// Energy decays exponentially: `E(t) = E0 × 2^(-Δt / half_life)`.
/// Calling [`boost`](NodeEnergy::boost) adds energy and resets the
/// activation timestamp.
#[derive(Debug, Clone)]
pub struct NodeEnergy {
    /// Stored energy level (as of `last_activated`).
    energy: f64,
    /// When the energy was last set/boosted.
    last_activated: Instant,
    /// Half-life for exponential decay.
    half_life: Duration,
    /// Monotonic access counter for LRU eviction (updated on every read/write).
    pub(crate) last_access: u64,
}

impl NodeEnergy {
    /// Creates a new `NodeEnergy` with the given initial energy and half-life.
    pub fn new(energy: f64, half_life: Duration) -> Self {
        Self {
            energy,
            last_activated: Instant::now(),
            half_life,
            last_access: 0,
        }
    }

    /// Creates a `NodeEnergy` with a specific activation time.
    ///
    /// Useful for testing and simulation where you need to control the
    /// starting timestamp.
    pub fn new_at(energy: f64, half_life: Duration, last_activated: Instant) -> Self {
        Self {
            energy,
            last_activated,
            half_life,
            last_access: 0,
        }
    }

    /// Returns the current energy after applying decay.
    ///
    /// `E(t) = E0 × 2^(-Δt / half_life)`
    pub fn current_energy(&self) -> f64 {
        self.energy_at(Instant::now())
    }

    /// Returns the energy at a specific instant (for testing/simulation).
    pub fn energy_at(&self, now: Instant) -> f64 {
        let elapsed = now.duration_since(self.last_activated);
        let half_lives = elapsed.as_secs_f64() / self.half_life.as_secs_f64();
        self.energy * 2.0_f64.powf(-half_lives)
    }

    /// Boosts the node's energy by `amount`.
    ///
    /// First applies the current decay, then adds `amount`, and resets
    /// the activation timestamp.
    pub fn boost(&mut self, amount: f64) {
        self.boost_at(amount, Instant::now());
    }

    /// Boosts at a specific instant.
    ///
    /// Useful for testing and simulation.
    pub fn boost_at(&mut self, amount: f64, now: Instant) {
        let current = self.energy_at(now);
        self.energy = current + amount;
        self.last_activated = now;
    }

    /// Matérialise la décroissance jusqu'à `now` et déplace l'ancre.
    ///
    /// Sert au chemin sans substrat, où l'ancre est un `Instant` : c'est
    /// légitime DANS une session (la loi ne dépend que de `Δt`), mais cette
    /// ancre ne survit pas au processus — en mode substrat l'ancre est un
    /// epoch Unix persisté (voir [`crate::decay_law`]).
    pub fn advance_to(&mut self, now: Instant) {
        self.energy = self.energy_at(now);
        self.last_activated = now;
    }

    /// Returns the stored (non-decayed) energy value.
    pub fn raw_energy(&self) -> f64 {
        self.energy
    }

    /// Returns the last activation instant.
    pub fn last_activated(&self) -> Instant {
        self.last_activated
    }

    /// Returns the configured half-life.
    pub fn half_life(&self) -> Duration {
        self.half_life
    }

    /// Clamps the stored energy to `[min, max]`.
    pub fn clamp(&mut self, min: f64, max: f64) {
        self.energy = self.energy.clamp(min, max);
    }
}

// ---------------------------------------------------------------------------
// EnergyStore
// ---------------------------------------------------------------------------

/// Thread-safe store for node energy states.
///
/// Uses `DashMap` for concurrent read/write without global locks.
/// When a backing [`GraphStoreMut`](obrain_core::graph::GraphStoreMut) is
/// provided, every write is also persisted as a node property (write-through).
/// On read, if the node is not in the hot cache, the store attempts to load
/// the value lazily from the graph property.
pub struct EnergyStore {
    /// Per-node energy entries (includes inline LRU access counter).
    ///
    /// When the store is backed by a `SubstrateStore` (via [`Self::with_substrate`]),
    /// this cache is a best-effort write-back layer around the authoritative
    /// on-disk column; reads first probe the cache, then fall through to the
    /// substrate column. In the legacy graph-store mode it is the primary
    /// working set, write-through'd to the backing `GraphStoreMut` properties.
    nodes: DashMap<NodeId, NodeEnergy>,
    /// Cross-base energy map — tracks energy for nodes identified by (db_id, node_id) pairs.
    #[cfg(feature = "synapse")]
    cross_base: DashMap<crate::synapse::CrossBaseNodeId, f64>,
    /// Monotonic counter for LRU tracking.
    access_counter: AtomicU64,
    /// Maximum cache entries (0 = unlimited).
    max_cache_entries: usize,
    /// Configuration.
    config: EnergyConfig,
    /// Optional backing graph store for write-through persistence (legacy mode).
    graph_store: OptionalGraphStore,
    /// Optional backing substrate store — when set, the cognitive column of
    /// `NodeRecord.energy` (u16 Q1.15) is the source of truth and cache is a
    /// thin accelerator.
    #[cfg(feature = "substrate")]
    substrate: Option<Arc<obrain_substrate::SubstrateStore>>,
    /// Horloge injectable (tests déterministes / simulation) : micros Unix.
    /// Par défaut [`crate::decay_law::unix_micros`].
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// High-water mark DE SESSION : borne les reculs d'horloge au sein d'un
    /// même processus. Le `t_hi` persisté dans l'ancre borne le redémarrage.
    session_hi: AtomicI64,
}

impl EnergyStore {
    /// Creates a new, empty energy store (in-memory only, no persistence).
    pub fn new(config: EnergyConfig) -> Self {
        Self {
            nodes: DashMap::new(),
            #[cfg(feature = "synapse")]
            cross_base: DashMap::new(),
            access_counter: AtomicU64::new(0),
            max_cache_entries: 0,
            config,
            graph_store: None,
            #[cfg(feature = "substrate")]
            substrate: None,
            clock: Arc::new(unix_micros),
            session_hi: AtomicI64::new(0),
        }
    }

    /// Creates a new energy store with write-through persistence.
    pub fn with_graph_store(
        config: EnergyConfig,
        graph_store: Arc<dyn obrain_core::graph::GraphStoreMut>,
    ) -> Self {
        Self {
            nodes: DashMap::new(),
            #[cfg(feature = "synapse")]
            cross_base: DashMap::new(),
            access_counter: AtomicU64::new(0),
            max_cache_entries: 0,
            config,
            graph_store: Some(graph_store),
            #[cfg(feature = "substrate")]
            substrate: None,
            clock: Arc::new(unix_micros),
            session_hi: AtomicI64::new(0),
        }
    }

    /// Creates a new energy store backed by a substrate column view (T6).
    ///
    /// The `NodeRecord.energy` u16 Q1.15 column is the source of truth —
    /// every `boost` / `set` translates to a dedicated `EnergyReinforce`
    /// WAL record + mmap column mutation via
    /// [`SubstrateStore::boost_node_energy_f32`](obrain_substrate::SubstrateStore::boost_node_energy_f32) /
    /// [`SubstrateStore::set_node_energy_f32`](obrain_substrate::SubstrateStore::set_node_energy_f32). The in-memory `DashMap`
    /// cache is retained as a warm-read accelerator.
    ///
    /// **Ancre physique (2026-11-24).** `SubstrateStore` implémente déjà
    /// `GraphStoreMut`, donc le canal de propriétés est disponible : on le
    /// rattache ici pour que l'instant de référence
    /// (`_cog_energy_last_activated_epoch`, secondes epoch `f64`) soit écrit à
    /// chaque boost et relu à chaque lecture. Sans ce rattachement il n'existe
    /// AUCUN endroit où écrire l'ancre : la colonne se relit telle quelle au
    /// redémarrage et l'énergie est figée (défaut mesuré : +12 700 % d'erreur
    /// à 168 h pour une demi-vie de 24 h). La décroissance n'a donc plus
    /// besoin d'une cadence pour être JUSTE ; [`Self::decay_tick`] ne sert plus
    /// qu'à faire vieillir les nœuds que personne ne lit.
    #[cfg(feature = "substrate")]
    pub fn with_substrate(
        config: EnergyConfig,
        substrate: Arc<obrain_substrate::SubstrateStore>,
    ) -> Self {
        let graph_store: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
        Self {
            nodes: DashMap::new(),
            #[cfg(feature = "synapse")]
            cross_base: DashMap::new(),
            access_counter: AtomicU64::new(0),
            max_cache_entries: 0,
            config,
            graph_store: Some(graph_store),
            substrate: Some(substrate),
            clock: Arc::new(unix_micros),
            session_hi: AtomicI64::new(0),
        }
    }

    /// Returns `true` if this store routes through a substrate column view.
    #[cfg(feature = "substrate")]
    pub fn is_substrate_backed(&self) -> bool {
        self.substrate.is_some()
    }

    /// Instant brut de l'horloge injectée, en micros Unix.
    pub fn now_micros(&self) -> i64 {
        (self.clock)()
    }

    /// Instant effectif : `max(now, session_hi)`.
    ///
    /// Le high-water mark DE SESSION borne les reculs d'horloge (NTP,
    /// changement d'heure) au sein d'un même processus : une lecture à 24 h
    /// suivie d'un recul à 12 h rend toujours 0,5, jamais 0,7071. Le `t_hi`
    /// persisté dans l'ancre n'est écrit que par `reinforce` / `advance_all` :
    /// à travers un redémarrage, une lecture seule d'un instant plus grand
    /// n'étant pas persistée, un crash suivi d'un recul d'horloge peut
    /// remonter l'énergie vers la valeur qu'implique le dernier `t_hi` durable
    /// (limitation documentée, pas une corruption).
    fn now_eff(&self) -> i64 {
        let now = self.now_micros();
        self.session_hi.fetch_max(now, Ordering::Relaxed).max(now)
    }

    /// Injecte une horloge (tests déterministes / simulation). Reconstruit le
    /// store : `EnergyStore::with_substrate(cfg, sub).with_clock(horloge)`.
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    /// Sets the maximum number of cache entries. When exceeded, LRU eviction kicks in.
    /// Evicted entries remain in the graph store and are reloaded on-demand.
    pub fn with_max_cache_entries(mut self, max: usize) -> Self {
        self.max_cache_entries = max;
        self
    }

    /// Records an access for LRU tracking — updates the inline counter
    /// in the already-locked DashMap entry (no extra DashMap lookup).
    fn touch(&self, node_id: NodeId) {
        let order = self.access_counter.fetch_add(1, Ordering::Relaxed);
        if let Some(mut entry) = self.nodes.get_mut(&node_id) {
            entry.last_access = order;
        }
    }

    /// Evicts least-recently-used entries if cache exceeds max_cache_entries.
    fn maybe_evict(&self) {
        if self.max_cache_entries == 0 {
            return;
        }
        let current_len = self.nodes.len();
        if current_len <= self.max_cache_entries {
            return;
        }
        let to_evict = current_len - self.max_cache_entries;
        // Collect entries sorted by access order (ascending = oldest first)
        let mut entries: Vec<(NodeId, u64)> = self
            .nodes
            .iter()
            .map(|e| (*e.key(), e.value().last_access))
            .collect();
        entries.sort_by_key(|(_, order)| *order);
        for (node_id, _) in entries.into_iter().take(to_evict) {
            self.nodes.remove(&node_id);
        }
    }

    /// Returns the current energy for a node.
    ///
    /// In **legacy mode**, decay is applied on read via `NodeEnergy::current_energy()`.
    /// In **substrate-backed mode**, the value returned is
    /// `E_ref · 2^(-Δt / H)` where `E_ref` is the column and `t_ref` the
    /// persisted epoch anchor — so the decay is correct even if no tick ran,
    /// even across a restart. Returns `0.0` if the node has never been tracked
    /// (or has tombstoned / missing column).
    /// Lecture énergétique QUALIFIÉE : l'autorité décisionnelle (le f64 de
    /// `get_energy` reste une projection appauvrie, jamais une autorité).
    #[cfg(feature = "substrate")]
    pub fn read_qualified(&self, node_id: NodeId) -> crate::decay_clock::EnergyRead {
        let law = DecayLaw::new(self.config.default_half_life);
        crate::decay_clock::read_energy_qualified(
            self.substrate.as_ref().unwrap(),
            node_id,
            &law,
            self.now_eff(),
        )
    }

    /// Énergie courante du nœud (projection f64), `0.0` si jamais suivi ou colonne absente.
    pub fn get_energy(&self, node_id: NodeId) -> f64 {
        // Substrate-backed: column = E_ref, property = t_ref.
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.substrate.as_ref() {
            let law = DecayLaw::new(self.config.default_half_life);
            return crate::decay_clock::energy_at(sub, node_id, &law, self.now_eff())
                .unwrap_or(0.0);
        }

        if let Some(entry) = self.nodes.get(&node_id) {
            let energy = entry.current_energy();
            drop(entry);
            // Only update LRU access order when eviction is configured —
            // avoids a DashMap write on every read in the default (no-eviction) path.
            if self.max_cache_entries > 0 {
                self.touch(node_id);
            }
            return energy;
        }
        // Lazy load from graph store — reconstruct Instant from epoch
        if let Some(gs) = &self.graph_store
            && let Some(raw_energy) = load_node_f64(gs.as_ref(), node_id, PROP_ENERGY)
        {
            let last_activated = epoch_to_instant(load_node_f64(
                gs.as_ref(),
                node_id,
                PROP_ENERGY_LAST_ACTIVATED_EPOCH,
            ));
            let mut ne =
                NodeEnergy::new_at(raw_energy, self.config.default_half_life, last_activated);
            if self.max_cache_entries > 0 {
                ne.last_access = self.access_counter.fetch_add(1, Ordering::Relaxed);
            }
            let current = ne.current_energy();
            self.nodes.insert(node_id, ne);
            self.maybe_evict();
            return current;
        }
        0.0
    }

    /// Boosts a node's energy by `amount`.
    ///
    /// **Substrate mode**: lit la colonne (`E_ref`), applique la décroissance
    /// écoulue depuis l'ancre persistée, ajoute `amount`, clampe, puis écrit
    /// colonne ET ancre (`EnergyReinforce` + `PropSetInline`). La valeur rendue
    /// par [`Self::get_energy`] est donc toujours cohérente avec l'ancre.
    ///
    /// **Saturation (choix explicite).** La colonne Q1.15 porte `[0, 1]` : un
    /// boost sur un nœud déjà à 1,0 ne peut PAS s'accumuler, et c'est
    /// irréversible — l'information « ce nœud a été boosté 5 fois plutôt que 1 »
    /// est perdue. On ne compense pas en gonflant la valeur (elle est le
    /// multiplicateur de la loi) ni en trichant sur l'ancre : les trois
    /// régimes sont distincts et testés (`tests/decay_regimes_test.rs`) —
    /// sous le seuil, au seuil, au-dessus. Seule la borne haute est
    /// destructive, et elle l'est de façon uniforme : un nœud saturé et un
    /// nœud boosté une fois ne sont PAS distinguables dans la colonne, mais
    /// ils le sont par la date de leur dernière activation.
    ///
    /// **Legacy mode**: if the node is not yet tracked, it is created with
    /// the boost as initial energy. The new energy value is written through
    /// to the backing graph store (if configured).
    pub fn boost(&self, node_id: NodeId, amount: f64) {
        let max_energy = self.config.max_energy;

        // Substrate-backed: the column is the source of truth. We still
        // maintain the cache for legacy callers that inspect `snapshot()` /
        // `len()` fast-path, but the durable write is a WAL-first column
        // mutation.
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.substrate.as_ref() {
            let law = DecayLaw::new(self.config.default_half_life);
            let now = self.now_eff();
            // `reinforce` rend `None` sur un slot mort ou non alloué — même
            // contrat que l'ancien `boost_node_energy_f32`.
            if let Some(nouvelle) =
                crate::decay_clock::reinforce(sub, node_id, amount, &law, max_energy, now)
            {
                // Cache chaud, pour la parité de `len()`/`snapshot()` legacy.
                self.nodes
                    .entry(node_id)
                    .and_modify(|e| {
                        e.energy = nouvelle;
                        e.last_activated = Instant::now();
                    })
                    .or_insert_with(|| NodeEnergy::new(nouvelle, law.half_life()));
                self.touch(node_id);
                self.maybe_evict();
            }
            return;
        }

        self.nodes
            .entry(node_id)
            .and_modify(|e| {
                e.boost(amount);
                e.clamp(0.0, max_energy);
            })
            .or_insert_with(|| {
                let clamped = amount.clamp(0.0, max_energy);
                NodeEnergy::new(clamped, self.config.default_half_life)
            });
        self.touch(node_id);
        self.maybe_evict();
        // Write-through: persist raw energy + epoch timestamp
        if let Some(gs) = &self.graph_store
            && let Some(entry) = self.nodes.get(&node_id)
        {
            persist_node_f64(gs.as_ref(), node_id, PROP_ENERGY, entry.raw_energy());
            persist_node_f64(
                gs.as_ref(),
                node_id,
                PROP_ENERGY_LAST_ACTIVATED_EPOCH,
                now_epoch_secs(),
            );
        }
    }

    /// Apply an eager multiplicative decay to every live node's energy
    /// column. `factor` is in `[0.0, 1.0]` (`0.5` halves all energies).
    ///
    /// **Substrate mode**: dispatches to [`SubstrateStore::decay_all_energy`](obrain_substrate::SubstrateStore::decay_all_energy)
    /// which logs a single `EnergyDecay` WAL record and rewrites every live
    /// slot in one pass, puis avance toutes les ancres au même instant.
    ///
    /// **À ne pas câbler tel quel.** Ce facteur est CONSTANT : l'appliquer
    /// toutes les 60 s avec `0.99` donne une demi-vie effective de 69 min au
    /// lieu de 24 h (20,9× trop rapide), et l'appliquer une fois par
    /// redémarrage donne l'erreur inverse. La production doit appeler
    /// [`Self::decay_tick`], qui dérive le facteur de `Δt` et de la demi-vie.
    ///
    /// **Legacy mode**: iterates the hot cache and multiplies each
    /// `raw_energy`, resetting `last_activated` to `now`. The backing
    /// graph-store (if any) is NOT re-persisted in this path — callers that
    /// rely on legacy `PROP_ENERGY` durability should stick with lazy decay.
    pub fn decay_all(&self, factor: f64) {
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.substrate.as_ref() {
            let _ = sub.decay_all_energy(factor as f32);
            // La colonne vient d'être multipliée : les ancres doivent avancer
            // au MÊME instant, sinon la lecture paresseuse rejouerait le même
            // intervalle (double décroissance). C'est le seul cas où les
            // ancres bougent en masse sans que chaque valeur soit recalculée.
            self.reancrer(sub);
            // Invalidate cache — cheapest path is to clear, next read re-reads
            // the column. The cache is a best-effort accelerator in
            // substrate mode, not a source of truth.
            self.nodes.clear();
            return;
        }

        let f = factor.clamp(0.0, 1.0);
        let now = Instant::now();
        for mut entry in self.nodes.iter_mut() {
            entry.energy *= f;
            entry.last_activated = now;
        }
    }

    /// Avance chaque nœud le long de SA PROPRE loi, depuis SA PROPRE ancre
    /// persistée, jusqu'à l'instant courant. Rend le nombre de nœuds avancés.
    ///
    /// C'est le remplaçant de [`Self::decay_all`] côté production : le facteur
    /// n'est plus un paramètre, il est calculé par nœud à partir de `Δt` et de
    /// la demi-vie (`2^(-Δt/H)`). Une tique manquée — ou un redémarrage, ou
    /// une machine éteinte une semaine — ne perd rien : la tique suivante voit
    /// un `Δt` plus grand et applique la même loi. C'est la propriété de
    /// semi-groupe de l'exponentielle, et c'est ce que le facteur fixe du stub
    /// (0,99 toutes les 60 s) ne pouvait pas donner.
    ///
    /// `max_nodes` borne le travail d'une tique (0 = pas de borne) : les nœuds
    /// non traités gardent leur ancre et sont rattrapés à la suivante, sans
    /// erreur.
    pub fn decay_tick(&self, max_nodes: usize) -> usize {
        let law = DecayLaw::new(self.config.default_half_life);
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.substrate.as_ref() {
            return match crate::decay_clock::advance_all(sub, &law, max_nodes, self.now_eff()) {
                Ok(stats) => stats.advanced as usize,
                Err(_) => 0,
            };
        }
        // Chemin sans substrat : l'ancre est un `Instant` de la cache — même
        // loi, seule horloge disponible.
        let now = Instant::now();
        let mut n = 0usize;
        for mut entry in self.nodes.iter_mut() {
            if max_nodes != 0 && n >= max_nodes {
                break;
            }
            entry.advance_to(now);
            n += 1;
        }
        n
    }

    /// Avance l'ancre de chaque nœud porteur d'énergie jusqu'à maintenant.
    ///
    /// Réservé à [`Self::decay_all`], où un seul facteur a été appliqué à
    /// toute la colonne : dans ce cas précis les ancres doivent bouger en
    /// masse. Les nœuds d'énergie nulle sont sautés (la loi y est un point
    /// fixe, réécrire leur ancre serait deux écritures pour rien).
    #[cfg(feature = "substrate")]
    fn reancrer(&self, sub: &obrain_substrate::SubstrateStore) -> usize {
        let now = self.now_eff();
        let Ok(slots) = sub.iter_live_node_energies() else {
            return 0;
        };
        let mut n = 0;
        for (id, e) in slots {
            if e <= 0.0 {
                continue;
            }
            crate::decay_clock::persist_anchor(sub, id, now);
            n += 1;
        }
        n
    }

    /// Returns all node IDs whose current energy is below `threshold`.
    pub fn list_low_energy(&self, threshold: f64) -> Vec<NodeId> {
        #[cfg(feature = "substrate")]
        if self.substrate.is_some() {
            // Passe par `snapshot` : la loi doit être appliquée AVANT le
            // seuil, sinon un nœud qui a décru sous le seuil depuis la
            // dernière tique échappe à l'archivage.
            return self
                .snapshot()
                .into_iter()
                .filter(|(_, e)| *e < threshold)
                .map(|(id, _)| id)
                .collect();
        }

        self.nodes
            .iter()
            .filter(|entry| entry.value().current_energy() < threshold)
            .map(|entry| *entry.key())
            .collect()
    }

    /// Rehydrates the energy cache from the backing graph store.
    ///
    /// **Substrate mode**: this is a no-op returning `0`. The on-disk
    /// u16 Q1.15 column is the source of truth; there is nothing to
    /// "rehydrate" — the substrate's own mmap + WAL-replay already leaves
    /// the column in its crash-consistent state on open. The in-memory
    /// DashMap cache is populated lazily on demand (next `get_energy` /
    /// `boost`).
    ///
    /// **Legacy mode**: iterates every node and loads `PROP_ENERGY` (+
    /// epoch) into the hot cache. Required on brain open — otherwise
    /// `len()`, `snapshot()`, and aggregate health metrics (total energy,
    /// activation count) report zero until a `boost()` call repopulates the
    /// DashMap.
    ///
    /// Returns the number of nodes loaded.
    pub fn load_from_graph(&self) -> usize {
        #[cfg(feature = "substrate")]
        if self.substrate.is_some() {
            // The substrate column is the source of truth. The cache is a
            // warm-read accelerator populated lazily — no eager fill here.
            tracing::trace!("energy::load_from_graph: no-op (state is column-resident)");
            return 0;
        }

        let Some(gs) = self.graph_store.as_ref() else {
            return 0;
        };
        let mut loaded = 0usize;
        for nid in gs.node_ids() {
            if self.nodes.contains_key(&nid) {
                continue;
            }
            let Some(raw_energy) = load_node_f64(gs.as_ref(), nid, PROP_ENERGY) else {
                continue;
            };
            let last_activated = epoch_to_instant(load_node_f64(
                gs.as_ref(),
                nid,
                PROP_ENERGY_LAST_ACTIVATED_EPOCH,
            ));
            let ne = NodeEnergy::new_at(raw_energy, self.config.default_half_life, last_activated);
            self.nodes.insert(nid, ne);
            loaded += 1;
        }
        loaded
    }

    /// Returns the number of tracked nodes.
    ///
    /// **Substrate mode**: this reports the cache size (eagerly-boosted or
    /// previously-read nodes). Use [`Self::snapshot`] for an authoritative
    /// count over the on-disk column.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if no nodes are tracked.
    ///
    /// **Substrate mode**: reflects the cache only — see [`Self::len`].
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns a reference to the configuration.
    pub fn config(&self) -> &EnergyConfig {
        &self.config
    }

    /// Returns all tracked node IDs with their current energy.
    ///
    /// **Substrate mode**: itère la colonne autoritaire (O(N) en mmap sur les
    /// slots vivants non enterrés) et applique la loi jusqu'à maintenant. Les
    /// ancres sont lues en UN lot (`get_node_property_batch`) : N valeurs pour
    /// une seule acquisition de verrou, pas N.
    ///
    /// **Legacy mode**: snapshots the DashMap cache, applying decay on read.
    pub fn snapshot(&self) -> Vec<(NodeId, f64)> {
        #[cfg(feature = "substrate")]
        if let Some(sub) = self.substrate.as_ref() {
            let Ok(pairs) = sub.iter_live_node_energies() else {
                return Vec::new();
            };
            let law = DecayLaw::new(self.config.default_half_life);
            let now = self.now_eff();
            let ids: Vec<NodeId> = pairs.iter().map(|(id, _)| *id).collect();
            let pk_ancre = obrain_common::types::PropertyKey::from(PROP_ENERGY_ANCHOR);
            let pk_legacy =
                obrain_common::types::PropertyKey::from(PROP_ENERGY_LAST_ACTIVATED_EPOCH);
            let ancres = self
                .graph_store
                .as_ref()
                .map(|gs| gs.get_node_property_batch(&ids, &pk_ancre))
                .unwrap_or_default();
            let legacy = self
                .graph_store
                .as_ref()
                .map(|gs| gs.get_node_property_batch(&ids, &pk_legacy))
                .unwrap_or_default();
            return pairs
                .into_iter()
                .enumerate()
                .map(|(i, (id, e))| {
                    let e_ref = e as f64;
                    let t_legacy = legacy
                        .get(i)
                        .and_then(|v| v.as_ref())
                        .and_then(|v| v.as_float64())
                        .filter(|s| s.is_finite() && *s > 0.0)
                        .map(epoch_secs_to_micros);
                    let composite = ancres.get(i).and_then(|v| v.as_ref());
                    (
                        id,
                        crate::decay_clock::energy_from_parts(
                            e_ref, composite, t_legacy, &law, now,
                        ),
                    )
                })
                .collect();
        }

        self.nodes
            .iter()
            .map(|entry| (*entry.key(), entry.value().current_energy()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Cross-base energy operations
    // -----------------------------------------------------------------------

    /// Returns a snapshot of all cross-base energy entries.
    #[cfg(feature = "synapse")]
    pub fn snapshot_cross_base(&self) -> Vec<(crate::synapse::CrossBaseNodeId, f64)> {
        use crate::synapse::CrossBaseNodeId;
        self.cross_base
            .iter()
            .map(
                |entry: dashmap::mapref::multiple::RefMulti<'_, CrossBaseNodeId, f64>| {
                    (entry.key().clone(), *entry.value())
                },
            )
            .collect()
    }

    /// Returns the top `limit` nodes for a specific database, sorted by
    /// energy descending.
    #[cfg(feature = "synapse")]
    pub fn get_top_nodes_for_base(&self, db_id: &str, limit: usize) -> Vec<(NodeId, f64)> {
        use crate::synapse::CrossBaseNodeId;
        let mut entries: Vec<(NodeId, f64)> = self
            .cross_base
            .iter()
            .filter(
                |entry: &dashmap::mapref::multiple::RefMulti<'_, CrossBaseNodeId, f64>| {
                    entry.key().db_id == db_id
                },
            )
            .map(
                |entry: dashmap::mapref::multiple::RefMulti<'_, CrossBaseNodeId, f64>| {
                    (entry.key().node_id, *entry.value())
                },
            )
            .collect();
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        entries.truncate(limit);
        entries
    }

    /// Boosts (inserts or adds to) the cross-base energy for a given node.
    #[cfg(feature = "synapse")]
    pub fn boost_cross_base(&self, xbid: crate::synapse::CrossBaseNodeId, amount: f64) {
        self.cross_base
            .entry(xbid)
            .and_modify(|e| *e += amount)
            .or_insert(amount);
    }
}

// ---------------------------------------------------------------------------
// Normalized scoring functions
// ---------------------------------------------------------------------------

/// Converts a raw energy value to a normalized score in `[0.0, 1.0]`.
///
/// Uses the formula: `score = 1 - exp(-energy / ref_energy)`.
///
/// - `energy = 0` → score = `0.0`
/// - `energy → ∞` → score → `1.0`
/// - Negative energy is clamped to `0.0`.
/// - `ref_energy` controls the curve spread (higher = slower saturation).
///   If `ref_energy <= 0` or NaN, falls back to `ref_energy = 1.0`.
///
/// This provides a smooth, bounded mapping from the unbounded energy
/// domain to a [0, 1] range suitable for cross-metric comparison.
#[inline]
pub fn energy_score(energy: f64, ref_energy: f64) -> f64 {
    if energy <= 0.0 || energy.is_nan() {
        return 0.0;
    }
    let r = if ref_energy <= 0.0 || ref_energy.is_nan() || ref_energy.is_infinite() {
        1.0
    } else {
        ref_energy
    };
    (1.0 - (-energy / r).exp()).clamp(0.0, 1.0)
}

/// Computes the effective half-life modulated by structural degree.
///
/// `effective_half_life = base_half_life * (1 + α * ln(1 + degree))`
///
/// Nodes with higher degree (more connections) retain energy longer,
/// reflecting their structural importance in the graph.
///
/// - `degree = 0` → effective = base (no reinforcement)
/// - Higher α → stronger reinforcement effect
/// - α = 0 → no reinforcement (returns base)
#[inline]
pub fn effective_half_life(base: Duration, degree: usize, alpha: f64) -> Duration {
    if alpha <= 0.0 || alpha.is_nan() {
        return base;
    }
    let factor = 1.0 + alpha * (1.0 + degree as f64).ln();
    let secs = base.as_secs_f64() * factor;
    if secs.is_finite() && secs > 0.0 {
        Duration::from_secs_f64(secs)
    } else {
        base
    }
}

impl std::fmt::Debug for EnergyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnergyStore")
            .field("tracked_nodes", &self.nodes.len())
            .field("config", &self.config)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// EnergyListener
// ---------------------------------------------------------------------------

/// A [`MutationListener`] that automatically boosts energy for mutated nodes.
///
/// When a node or edge is created/updated/deleted, the involved node(s)
/// receive an energy boost of `config.boost_on_mutation`.
pub struct EnergyListener {
    /// Shared energy store.
    store: Arc<EnergyStore>,
}

impl EnergyListener {
    /// Creates a new energy listener backed by the given store.
    pub fn new(store: Arc<EnergyStore>) -> Self {
        Self { store }
    }

    /// Returns a reference to the underlying store.
    pub fn store(&self) -> &Arc<EnergyStore> {
        &self.store
    }

    /// Extracts all node IDs affected by a mutation event.
    fn affected_nodes(event: &MutationEvent) -> smallvec::SmallVec<[NodeId; 2]> {
        use smallvec::smallvec;
        match event {
            MutationEvent::NodeCreated { node } => smallvec![node.id],
            MutationEvent::NodeUpdated { after, .. } => smallvec![after.id],
            MutationEvent::NodeDeleted { node } => smallvec![node.id],
            MutationEvent::EdgeCreated { edge } => smallvec![edge.src, edge.dst],
            MutationEvent::EdgeUpdated { after, .. } => smallvec![after.src, after.dst],
            MutationEvent::EdgeDeleted { edge } => smallvec![edge.src, edge.dst],
        }
    }
}

#[async_trait]
impl MutationListener for EnergyListener {
    fn name(&self) -> &str {
        "cognitive:energy"
    }

    async fn on_event(&self, event: &MutationEvent) {
        let boost = self.boost_pour(event);
        for node_id in Self::affected_nodes(event) {
            self.store.boost(node_id, boost);
        }
    }

    async fn on_batch(&self, events: &[MutationEvent]) {
        for event in events {
            let boost = self.boost_pour(event);
            for node_id in Self::affected_nodes(event) {
                self.store.boost(node_id, boost);
            }
        }
    }
}

impl EnergyListener {
    /// Création (nœud ou arête) → `boost_on_create` ; le reste →
    /// `boost_on_mutation`.
    fn boost_pour(&self, event: &MutationEvent) -> f64 {
        match event {
            MutationEvent::NodeCreated { .. } | MutationEvent::EdgeCreated { .. } => self
                .store
                .config
                .boost_on_create
                .unwrap_or(self.store.config.boost_on_mutation),
            _ => self.store.config.boost_on_mutation,
        }
    }
}

#[cfg(test)]
mod boost_par_type_tests {
    use super::*;
    use obrain_reactive::NodeSnapshot;

    fn instantane(id: u64) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId(id),
            labels: Default::default(),
            properties: Vec::new(),
        }
    }

    #[tokio::test]
    async fn une_creation_booste_moins_qu_une_mise_a_jour() {
        let store = Arc::new(EnergyStore::new(EnergyConfig {
            boost_on_create: Some(0.25),
            boost_on_mutation: 1.0,
            ..Default::default()
        }));
        let l = EnergyListener::new(Arc::clone(&store));
        l.on_batch(&[
            MutationEvent::NodeCreated {
                node: instantane(1),
            },
            MutationEvent::NodeUpdated {
                before: instantane(2),
                after: instantane(2),
            },
        ])
        .await;
        let cree = store.get_energy(NodeId(1));
        let modifie = store.get_energy(NodeId(2));
        assert!(
            modifie > cree,
            "mise à jour ({modifie}) doit booster plus qu'une création ({cree})"
        );
        assert!((modifie - cree - 0.75).abs() < 1e-6, "écart = 1.0 − 0.25");
    }
}

impl std::fmt::Debug for EnergyListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnergyListener")
            .field("store", &self.store)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Substrate-backed tests (T6)
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "substrate"))]
mod substrate_tests {
    use super::*;
    use obrain_substrate::SubstrateStore;
    use std::sync::Arc;

    /// Helper: create a substrate store in a tempdir with `n` node slots
    /// allocated via the `GraphStoreMut` trait (each node gets a single
    /// label "n") so they have legitimate `label_bitset` entries. Returns
    /// the store and the allocated `NodeId`s.
    fn make_substrate(n: usize) -> (Arc<SubstrateStore>, Vec<NodeId>, tempfile::TempDir) {
        use obrain_core::graph::traits::GraphStoreMut;
        let td = tempfile::tempdir().unwrap();
        let sub = SubstrateStore::create(td.path().join("kb")).unwrap();
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            let id = sub.create_node(&["n"]);
            ids.push(id);
        }
        sub.flush().unwrap();
        (Arc::new(sub), ids, td)
    }

    #[test]
    fn substrate_energy_boost_reads_back_through_column() {
        let (sub, ids, _td) = make_substrate(3);
        let cfg = EnergyConfig::default();
        let store = EnergyStore::with_substrate(cfg, sub.clone());
        assert!(store.is_substrate_backed());

        // Boost slot 1 by 0.5 — column should hold ≈ 0.5.
        store.boost(ids[0], 0.5);
        let e0 = store.get_energy(ids[0]);
        assert!((e0 - 0.5).abs() < 1e-3, "energy after boost: {e0}");

        // Another boost — saturates at max_energy (clamped to 1.0 in Q1.15).
        store.boost(ids[0], 0.8);
        let e1 = store.get_energy(ids[0]);
        assert!(e1 <= 1.0_f64 + 1e-3, "energy exceeded 1.0: {e1}");
        assert!(e1 >= 0.9_f64, "energy should approach max: {e1}");
    }

    #[test]
    fn substrate_decay_all_halves_every_node() {
        let (sub, ids, _td) = make_substrate(5);
        let cfg = EnergyConfig::default();
        let store = EnergyStore::with_substrate(cfg, sub.clone());

        // Seed every node at 0.8.
        for id in &ids {
            store.boost(*id, 0.8);
        }
        // Decay ×0.5.
        store.decay_all(0.5);
        // Every node's column should now be ≈ 0.4 (±1 ULP of Q1.15).
        for id in &ids {
            let e = store.get_energy(*id);
            assert!(
                (e - 0.4).abs() < 5e-3,
                "node {id:?} energy: {e} (expected ≈ 0.4)"
            );
        }
    }

    #[test]
    fn substrate_load_from_graph_is_noop_but_returns_zero() {
        let (sub, ids, _td) = make_substrate(2);
        let cfg = EnergyConfig::default();
        let store = EnergyStore::with_substrate(cfg, sub.clone());
        // Seed one node so the column is non-zero.
        store.boost(ids[0], 0.3);
        // load_from_graph is a no-op in substrate mode (column is
        // already authoritative on disk).
        assert_eq!(store.load_from_graph(), 0);
        // But the value is still readable.
        assert!(store.get_energy(ids[0]) > 0.0);
    }

    #[test]
    fn substrate_snapshot_iterates_live_column() {
        let (sub, ids, _td) = make_substrate(4);
        let cfg = EnergyConfig::default();
        let store = EnergyStore::with_substrate(cfg, sub.clone());
        store.boost(ids[0], 0.2);
        store.boost(ids[2], 0.7);
        // Nodes 1 & 3 were never boosted — they have zero energy in the
        // column but are still "live" (allocated slots). snapshot() iterates
        // every live slot so it returns 4 entries.
        let snap = store.snapshot();
        assert_eq!(snap.len(), 4);
        let sum: f64 = snap.iter().map(|(_, e)| *e).sum();
        assert!((sum - 0.9).abs() < 5e-3, "sum of energies: {sum}");
    }

    #[test]
    fn substrate_list_low_energy_filters_via_column() {
        let (sub, ids, _td) = make_substrate(3);
        let cfg = EnergyConfig::default();
        let store = EnergyStore::with_substrate(cfg, sub.clone());
        store.boost(ids[0], 0.1);
        store.boost(ids[1], 0.5);
        store.boost(ids[2], 0.9);
        let low = store.list_low_energy(0.3);
        assert_eq!(low.len(), 1);
        assert_eq!(low[0], ids[0]);
    }
}

#[cfg(test)]
mod tests_chemin_sans_substrat {
    use super::*;
    use web_time::Instant;

    /// `advance_to` matérialise la décroissance jusqu'à `now` et DÉPLACE
    /// l'ancre (`last_activated`) : après une demi-vie, 1,0 → 0,5.
    #[test]
    fn advance_to_materialise_et_deplace_l_ancre() {
        let t0 = Instant::now();
        let mut e = NodeEnergy::new_at(1.0, Duration::from_secs(3600), t0);
        let t1 = t0 + Duration::from_secs(3600);
        e.advance_to(t1);
        assert!((e.raw_energy() - 0.5).abs() < 1e-12, "une demi-vie : {e:?}");
        assert_eq!(e.last_activated(), t1);
    }

    /// `decay_tick` SANS substrat avance chaque nœud du cache le long de sa
    /// propre ancre, et `max_nodes` borne le travail d'une tique (les nœuds
    /// restants sont reportés à la suivante, sans erreur).
    #[test]
    fn decay_tick_sans_substrat_avance_et_bornes_par_max_nodes() {
        let store = EnergyStore::new(EnergyConfig::default());
        store.boost(NodeId(1), 1.0);
        store.boost(NodeId(2), 0.5);
        store.boost(NodeId(3), 0.25);
        // Sans borne : les trois nœuds sont avancés en une tique.
        assert_eq!(store.decay_tick(0), 3);
        // Borne à 1 : un seul nœud avancé, `break` sur le suivant.
        assert_eq!(store.decay_tick(1), 1);
    }
}
