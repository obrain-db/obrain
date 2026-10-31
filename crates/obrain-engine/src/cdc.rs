//! Change Data Capture (CDC) for tracking entity mutations.
//!
//! When the `cdc` feature is enabled, the database records every mutation
//! (create, update, delete) with before/after property snapshots. This
//! enables audit trails, temporal queries, and downstream sync.
//!
//! # Durability (2026-08-31, prod-readiness)
//!
//! Le `CdcLog` est un journal en mémoire (`RwLock<HashMap>`). Sans
//! persistance, l'historique disparaît au redémarrage — bloquant pour la
//! prod. Le journal est donc **attaché à un persistor substrate**
//! ([`crate::cdc_persist::CdcPersist`]) quand le store substrate est
//! disponible : chaque mutation est sérialisée (bincode) et appendée dans
//! la zone `substrate.cdc_journal`, puis rechargée au boot. Les types sont
//! `Serialize`/`Deserialize` pour ce chemin.
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use obrain_engine::ObrainDB;
//! use obrain_common::types::Value;
//!
//! let db = ObrainDB::new_in_memory();
//! let id = db.create_node(&["Person"]);
//! db.set_node_property(id, "name", Value::from("Alix"));
//! db.set_node_property(id, "name", Value::from("Gus"));
//!
//! let history = db.history(id)?;
//! assert_eq!(history.len(), 3); // create + 2 updates
//! # Ok(())
//! # }
//! ```

use hashbrown::HashMap as HbHashMap;
use obrain_common::types::{EdgeId, EpochId, NodeId, Value};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use web_time::{SystemTime, UNIX_EPOCH};

/// The kind of mutation that occurred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    /// A new entity was created.
    Create,
    /// An existing entity was updated (property set or removed).
    Update,
    /// An entity was deleted.
    Delete,
}

/// A unique identifier for a graph entity (node or edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityId {
    /// A node identifier.
    Node(NodeId),
    /// An edge identifier.
    Edge(EdgeId),
}

impl From<NodeId> for EntityId {
    fn from(id: NodeId) -> Self {
        Self::Node(id)
    }
}

impl From<EdgeId> for EntityId {
    fn from(id: EdgeId) -> Self {
        Self::Edge(id)
    }
}

impl EntityId {
    /// Returns the raw u64 value for binding layers.
    #[must_use]
    pub fn as_u64(&self) -> u64 {
        match self {
            Self::Node(id) => id.as_u64(),
            Self::Edge(id) => id.as_u64(),
        }
    }

    /// Returns `true` if this is a node identifier.
    #[must_use]
    pub fn is_node(&self) -> bool {
        matches!(self, Self::Node(_))
    }
}

/// A recorded change event with before/after property snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// The entity that was changed.
    pub entity_id: EntityId,
    /// The kind of change.
    pub kind: ChangeKind,
    /// MVCC epoch when the change occurred.
    pub epoch: EpochId,
    /// Wall-clock timestamp (milliseconds since Unix epoch).
    pub timestamp: u64,
    /// Properties before the change (None for Create).
    pub before: Option<HashMap<String, Value>>,
    /// Properties after the change (None for Delete).
    pub after: Option<HashMap<String, Value>>,
}

/// The CDC log that records entity mutations.
///
/// Thread-safe: uses `RwLock<HashMap>` for concurrent access.
#[derive(Debug)]
pub struct CdcLog {
    events: RwLock<HbHashMap<EntityId, Vec<ChangeEvent>>>,
    /// (2026-08-31) Persistor substrate optionnel — quand il est attaché,
    /// chaque `record` est aussi appendé dans la zone `cdc_journal`
    /// (durabilité : le journal survit au redémarrage, prod-readiness §9.5).
    persist: parking_lot::RwLock<Option<Arc<crate::cdc_persist::CdcPersist>>>,
    /// (2026-09-05, search_delta) Timestamp wall-clock max enregistré —
    /// cache O(1) pour l'early-exit de `changes_since_timestamp` quand
    /// `since_ms` dépasse le plus récent événement (« rien de nouveau depuis
    /// ma vue », cas fréquent en session IA) : évite le scan de la fenêtre.
    max_timestamp: std::sync::atomic::AtomicU64,
    /// (2026-09-05) INDEX TEMPOREL — événements résidents par ordre de
    /// timestamp (append) : `(ts, entité, index dans events[entité])`. Permet
    /// à `changes_since_timestamp` de répondre en O(log n + nouveauté) au
    /// lieu de O(fenêtre) : le pattern « depuis ma dernière vue » (répété en
    /// session) passe en µs. Les Vec par entité sont append-only → index
    /// stable. Mis à jour à `record()` et au rechargement (2 sites uniques).
    by_ts: parking_lot::Mutex<Vec<(u64, EntityId, usize)>>,
    /// (2026-09-05) Historique persisté rechargé depuis la zone ? Le
    /// rechargement est LAZY (première lecture d'historique), pas au boot :
    /// décoder jusqu'à `CDC_RELOAD_CAP` événements en RAM à chaque ouverture
    /// de store coûtait ~0,7 s / ~1 Go sur les stores runtime (régression
    /// d'ouverture CLI/bridge imputée à c935b9d1) pour des processus qui ne
    /// lisent JAMAIS le CDC (import / lectures). `false` = pas encore rechargé.
    history_loaded: std::sync::atomic::AtomicBool,
    /// Sérialise le rechargement paresseux (exactement une fois).
    history_lock: parking_lot::Mutex<()>,
}

impl CdcLog {
    /// Creates a new empty CDC log.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: RwLock::new(HbHashMap::new()),
            persist: parking_lot::RwLock::new(None),
            max_timestamp: std::sync::atomic::AtomicU64::new(0),
            history_loaded: std::sync::atomic::AtomicBool::new(false),
            history_lock: parking_lot::Mutex::new(()),
            by_ts: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// (2026-08-31) Attache un persistor substrate. Depuis 2026-09-05 le
    /// rechargement de la zone est PARESSEUX : `attach_persist` ne fait
    /// qu'armer la persistance des `record` ; l'historique persisté n'est
    /// décodé (fenêtre bornée [`CDC_RELOAD_CAP`]) qu'à la première lecture
    /// d'historique ([`Self::ensure_history_loaded`]). Une ouverture de
    /// store ne paie donc plus le décodage de la zone CDC (régression
    /// 2026-09-05 : +0,7 s / +1 Go d'ouverture CLI/bridge sur stores runtime).
    pub fn attach_persist(&self, persist: crate::cdc_persist::CdcPersist) {
        let mut slot = self.persist.write();
        let premier_attachement = slot.is_none();
        *slot = Some(Arc::new(persist));
        drop(slot);
        // Un persistor fraîchement attaché peut apporter de l'historique
        // persisté (jamais encore lu) → la prochaine lecture rechargera.
        // (Ré-attachement sur un persistor déjà armé : l'état de
        // rechargement courant est conservé — pas de double insertion.)
        if premier_attachement {
            self.history_loaded
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// (2026-09-05) Recharge l'historique persisté depuis la zone UNE fois
    /// (fenêtre bornée à `CDC_RELOAD_CAP`), au premier usage d'une lecture
    /// d'historique. No-op si aucun persistor, ou déjà rechargé.
    ///
    /// DÉDUPLICATION : un même process peut avoir `record`é des événements
    /// AVANT la première lecture (écrivain + search_delta en session) — ces
    /// événements sont déjà en mémoire ET dans la zone. On n'importe donc que
    /// les événements de la zone ABSENTS de la mémoire, et l'identité comparée
    /// est l'ÉVÉNEMENT ENTIER (voir [`DedupCdc`]) : l'ancienne clé
    /// `(entity_id, epoch, timestamp)` effaçait les écritures multiples d'une
    /// même milliseconde — un cinquième des événements d'une rafale, et les
    /// contradictions avec eux.
    fn ensure_history_loaded(&self) {
        if self
            .history_loaded
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let _guard = self.history_lock.lock();
        if self
            .history_loaded
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let persist = self.persist.read().clone();
        if let Some(persist) = persist {
            if let Ok(loaded) = persist.load(crate::cdc_persist::cdc_reload_cap()) {
                let mut events = self.events.write();
                let mut deja = DedupCdc::default();
                for vec in events.values() {
                    for ev in vec.iter() {
                        deja.deja_connu(ev);
                    }
                }
                let mut max_ts = self
                    .max_timestamp
                    .load(std::sync::atomic::Ordering::Relaxed);
                let mut importes = 0usize;
                for (_ts, mut ev) in loaded {
                    if !deja.neuf(&ev) {
                        continue; // EXACTEMENT le même événement, déjà en mémoire
                    }
                    // (2026-09-05) RÉTENTION LÉGÈRE au rechargement : la zone
                    // garde tout ; la vue mémoire ne retient les payloads
                    // (before/after, souvent lourds : textes/arguments) que
                    // pour les Update (search_conflicts en a besoin) — les
                    // Create/Delete ne servent aux formes que par (id, kind,
                    // ts). Mesuré : fenêtre 10-20 k × payloads ≈ +150-200 Mo
                    // RSS par process sur stores runtime ; stripé → ~métadonnées.
                    if !matches!(ev.kind, ChangeKind::Update) {
                        ev.before = None;
                        ev.after = None;
                    }
                    max_ts = max_ts.max(ev.timestamp);
                    events.entry(ev.entity_id).or_default().push(ev);
                    importes += 1;
                }
                if importes > 0 {
                    self.max_timestamp
                        .store(max_ts, std::sync::atomic::Ordering::Relaxed);
                }
                // Miroir dans l'index temporel : on le REBÂTIT depuis la map
                // (une fois par process, au premier rechargement — O(fenêtre),
                // coût noyé dans le rechargement lui-même). Pas de verrou
                // imbriqué : on relâche events AVANT de prendre by_ts.
                drop(events);
                let rebuilt: Vec<(u64, EntityId, usize)> = {
                    let g = self.events.read();
                    let mut b = Vec::new();
                    for (ent, vec) in g.iter() {
                        for (i, ev) in vec.iter().enumerate() {
                            b.push((ev.timestamp, *ent, i));
                        }
                    }
                    b
                };
                if !rebuilt.is_empty() {
                    let mut by = self.by_ts.lock();
                    by.clear();
                    by.extend(rebuilt);
                    by.sort_unstable_by_key(|(t, _, _)| *t);
                }
            }
        }
        self.history_loaded
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// (2026-08-31) Persiste un événement dans la zone (fail-soft : un échec
    /// d'append ne casse pas l'écriture en mémoire — le journal reste chaud).
    fn persist_event(&self, event: &ChangeEvent) {
        if let Some(p) = self.persist.read().as_ref() {
            let _ = p.append(event);
        }
    }

    /// Records a change event.
    pub fn record(&self, event: ChangeEvent) {
        // Maintient le cache du timestamp max (early-exit search_delta).
        self.max_timestamp
            .fetch_max(event.timestamp, std::sync::atomic::Ordering::Relaxed);
        self.persist_event(&event);
        let ent = event.entity_id;
        let mut guard = self.events.write();
        let vec = guard.entry(ent).or_default();
        let idx = vec.len();
        let ts = event.timestamp;
        vec.push(event);
        drop(guard);
        self.by_ts.lock().push((ts, ent, idx));
    }

    /// Records a node creation.
    pub fn record_create_node(
        &self,
        id: NodeId,
        epoch: EpochId,
        props: Option<HashMap<String, Value>>,
    ) {
        self.record(ChangeEvent {
            entity_id: EntityId::Node(id),
            kind: ChangeKind::Create,
            epoch,
            timestamp: now_millis(),
            before: None,
            after: props,
        });
    }

    /// Records an edge creation.
    pub fn record_create_edge(
        &self,
        id: EdgeId,
        epoch: EpochId,
        props: Option<HashMap<String, Value>>,
    ) {
        self.record(ChangeEvent {
            entity_id: EntityId::Edge(id),
            kind: ChangeKind::Create,
            epoch,
            timestamp: now_millis(),
            before: None,
            after: props,
        });
    }

    /// Records a property update.
    pub fn record_update(
        &self,
        entity_id: EntityId,
        epoch: EpochId,
        key: &str,
        old_value: Option<Value>,
        new_value: Value,
    ) {
        let before = old_value.map(|v| {
            let mut m = HashMap::new();
            m.insert(key.to_string(), v);
            m
        });
        let mut after_map = HashMap::new();
        after_map.insert(key.to_string(), new_value);

        self.record(ChangeEvent {
            entity_id,
            kind: ChangeKind::Update,
            epoch,
            timestamp: now_millis(),
            before,
            after: Some(after_map),
        });
    }

    /// Records an entity deletion.
    pub fn record_delete(
        &self,
        entity_id: EntityId,
        epoch: EpochId,
        props: Option<HashMap<String, Value>>,
    ) {
        self.record(ChangeEvent {
            entity_id,
            kind: ChangeKind::Delete,
            epoch,
            timestamp: now_millis(),
            before: props,
            after: None,
        });
    }

    /// Returns all change events for an entity, ordered by epoch.
    #[must_use]
    pub fn history(&self, entity_id: EntityId) -> Vec<ChangeEvent> {
        self.ensure_history_loaded();
        self.events
            .read()
            .get(&entity_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns change events for an entity since the given epoch.
    #[must_use]
    pub fn history_since(&self, entity_id: EntityId, since_epoch: EpochId) -> Vec<ChangeEvent> {
        self.ensure_history_loaded();
        self.events
            .read()
            .get(&entity_id)
            .map(|events| {
                events
                    .iter()
                    .filter(|e| e.epoch >= since_epoch)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all change events across all entities in an epoch range.
    #[must_use]
    pub fn changes_between(&self, start_epoch: EpochId, end_epoch: EpochId) -> Vec<ChangeEvent> {
        self.ensure_history_loaded();
        let guard = self.events.read();
        let mut results = Vec::new();
        for events in guard.values() {
            for event in events {
                if event.epoch >= start_epoch && event.epoch <= end_epoch {
                    results.push(event.clone());
                }
            }
        }
        results.sort_by_key(|e| e.epoch);
        results
    }

    /// (2026-09-05, search_delta) Tous les changements dont le timestamp
    /// wall-clock est ≥ `since_ms`, triés par timestamp croissant.
    ///
    /// C'est le signal « ce qui a changé depuis ma dernière vue » pour une
    /// IA en session : le timestamp est en millisecondes Unix (pas l'epoch
    /// MVCC, invisible pour l'appelant). `filter` permet de restreindre
    /// (ex. un type d'entité) sans allouer la liste complète.
    ///
    /// # Coût
    ///
    /// O(n) sur la fenêtre chaude (le CDC est borné à `CDC_RELOAD_CAP` au
    /// rechargement) — pas de scan store. Tri O(m log m) sur les matches.
    ///
    /// (2026-09-06, chantier B « cdc_tail ») Chemin FROID : tant que
    /// l'historique persisté n'est pas résident (premier appel d'un process
    /// neuf), un persistor armé permet de répondre depuis l'index de queue
    /// [`crate::cdc_persist`] — décodage des SEULES frames ≥ `since_ms`
    /// (µs..ms) au lieu du rechargement lazy de la fenêtre (~20-30 ms), qui
    /// reste le fallback quand l'index ne couvre pas le `since` demandé ou
    /// qu'il est incohérent avec la zone.
    pub fn changes_since_timestamp(
        &self,
        since_ms: u64,
        filter: impl Fn(&ChangeEvent) -> bool,
    ) -> Vec<ChangeEvent> {
        // (2026-09-06, cdc_tail) Voie froide : tentative AVANT le rechargement
        // lazy. `changes_since_cold` rend `None` quand elle ne peut pas
        // répondre (pas de persistor, queue incohérente/périmée, `since` plus
        // vieux que la couverture de l'index) → chemin historique inchangé.
        if !self
            .history_loaded
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let persist = self.persist.read().clone();
            if let Some(persist) = persist.as_ref()
                && let Some(mut results) = self.changes_since_cold(persist, since_ms, &filter)
            {
                results.sort_by_key(|e| e.timestamp);
                return results;
            }
        }
        // (2026-09-05) L'historique persisté est rechargé AVANT l'early-exit :
        // `max_timestamp` ne vaut rien tant que la zone CDC n'a pas été lue.
        self.ensure_history_loaded();
        // Early-exit O(1) : rien de nouveau depuis `since_ms` (cas fréquent
        // en session IA — « ma dernière vue » est récente).
        if since_ms
            > self
                .max_timestamp
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Vec::new();
        }
        // INDEX TEMPOREL : borne inférieure par timestamp puis parcours de la
        // seule nouveauté — O(log n + m) au lieu de O(fenêtre). Aucun verrou
        // imbriqué (on copie la tranche d'index, puis on lit la map).
        let slice: Vec<(u64, EntityId, usize)> = {
            let by = self.by_ts.lock();
            let pos = by.partition_point(|(t, _, _)| *t < since_ms);
            by[pos..].to_vec()
        };
        let guard = self.events.read();
        let mut results = Vec::new();
        for (_ts, ent, idx) in slice {
            if let Some(vec) = guard.get(&ent)
                && let Some(event) = vec.get(idx)
                && filter(event)
            {
                results.push(event.clone());
            }
        }
        results
    }

    /// (2026-09-06, cdc_tail) Voie froide de [`Self::changes_since_timestamp`]
    /// — voir la doc de la méthode publique.
    ///
    /// Répond depuis DEUX sources, fusionnées par clé `(entity_id, epoch,
    /// timestamp)` (même clé de déduplication que le rechargement) :
    ///   1. la ZONE, via l'index de queue — frames ≥ `since_ms` décodées par
    ///      seek (jamais la marche de la zone) ;
    ///   2. la MÉMOIRE session (événements enregistrés par ce process, voire
    ///      non persistés : append fail-soft, records antérieurs à
    ///      l'attache) — même sélection par l'index temporel résident.
    ///
    /// La rétention légère du rechargement est appliquée aux événements de la
    /// zone (payloads Create/Delete stripés) : les formes servies par la voie
    /// froide sont BYTE-À-BYTE symétriques de celles du chemin chaud — un
    /// 1er appel ne peut pas différer des suivants.
    ///
    /// Retourne `None` si la voie froide ne peut PAS répondre (l'appelant
    /// retombe sur `ensure_history_loaded` + chemin chaud) ; `Some(events)`
    /// sinon (vide = « rien de neuf », réponse sans rechargement).
    fn changes_since_cold(
        &self,
        persist: &crate::cdc_persist::CdcPersist,
        since_ms: u64,
        filter: &impl Fn(&ChangeEvent) -> bool,
    ) -> Option<Vec<ChangeEvent>> {
        // (a) Zone : `None` = index incohérent ou couverture insuffisante.
        let zone_events = persist.cold_zone_delta(since_ms)?;
        let mut results: Vec<ChangeEvent> = Vec::new();
        let mut deja = DedupCdc::default();
        for mut ev in zone_events {
            // Rétention légère — miroir exact de `ensure_history_loaded`.
            retenir_leger(&mut ev);
            if !deja.neuf(&ev) {
                continue;
            }
            if filter(&ev) {
                results.push(ev);
            }
        }
        // (b) Mémoire session — même sélection que le chemin chaud, sans
        //     verrous imbriqués (tranche copiée, puis lecture de la map).
        let slice: Vec<(u64, EntityId, usize)> = {
            let by = self.by_ts.lock();
            let pos = by.partition_point(|(t, _, _)| *t < since_ms);
            by[pos..].to_vec()
        };
        let guard = self.events.read();
        for (_ts, ent, idx) in slice {
            if let Some(vec) = guard.get(&ent)
                && let Some(event) = vec.get(idx)
            {
                // MÊME rétention que la zone, sinon la même écriture lue des deux
                // côtés ne serait plus reconnue comme la même et compterait deux
                // fois (`DedupCdc` compare l'événement entier).
                let mut vu = event.clone();
                retenir_leger(&mut vu);
                if !deja.neuf(&vu) {
                    continue;
                }
                if filter(&vu) {
                    results.push(vu);
                }
            }
        }
        Some(results)
    }

    /// Returns the total number of recorded events.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.events.read().values().map(Vec::len).sum()
    }
}

impl Default for CdcLog {
    fn default() -> Self {
        Self::new()
    }
}

/// ── DÉDUPLICATION CDC : L'ÉVÉNEMENT ENTIER FAIT LA CLÉ ──────────────────────
///
/// Défaut mesuré le 2026-09-17 (`relief_domine_le_nouveau_hub_et_penalise_la_
/// contradiction`) : la déduplication reposait sur `(entity_id, epoch,
/// timestamp)`, avec l'hypothèse écrite noir sur blanc — « deux mutations d'une
/// même entité ne partagent pas l'epoch ». **C'est faux** : cinq écritures de
/// propriété sur le MÊME nœud dans la MÊME milliseconde partagent l'epoch ET le
/// timestamp. Quatre événements sur cinq étaient donc jetés comme « doublons »,
/// silencieusement, dans la voie froide comme dans le rechargement d'historique.
/// Conséquence observable : `search_conflicts` ne voyait plus qu'un seul flip,
/// donc plus aucune contradiction — une réponse FAUSSE qui ressemble à une
/// réponse (« aucune dissonance dans la fenêtre »), exactement le genre de
/// défaut que ce dépôt refuse ailleurs.
///
/// La déduplication garde son objet — le même événement lu DEUX fois (zone +
/// mémoire de session) — mais le fait sur l'événement ENTIER : deux écritures
/// différentes ne sont jamais confondues, deux lectures de la même écriture le
/// sont toujours. Le regroupement par clé grossière évite de comparer toute la
/// fenêtre entre elle (la comparaison reste locale aux événements d'une même
/// milliseconde d'une même entité).
#[derive(Default)]
struct DedupCdc {
    /// Clé grossière → indice de hachage FINE → indices des événements acceptés.
    ///
    /// ── POURQUOI DEUX NIVEAUX, MESURÉ ───────────────────────────────────────
    /// La première version gardait, pour chaque clé grossière, un `Vec<ChangeEvent>`
    /// et comparait l'événement à TOUS ses prédécesseurs. Exact, mais **quadratique
    /// dans la taille du groupe** — et un groupe, c'est exactement ce que produit un
    /// import : des milliers d'écritures sur la MÊME entité (le nœud de session)
    /// dans la MÊME milliseconde. Mesuré sur le chemin froid (`changes_since_cold`,
    /// celui d'un pont qui lit une zone) : 16,4 ms pour 1 000 événements, 59,0 ms
    /// pour 2 000, **228,1 ms pour 4 000** — ×3,9 par doublement, soit quadratique.
    /// Extrapolé à la zone du store réel (des dizaines de milliers d'événements sur
    /// la même entité), cela fait des MINUTES à 100 % d'un cœur : c'est très
    /// exactement l'emballement de pont observé trois fois le 2026-09-17 (85 % de
    /// CPU, 2 Go de RSS, presque aucune E/S). Un correctif de justesse avait
    /// introduit un défaut de coût.
    ///
    /// L'exactitude est CONSERVÉE : la comparaison fine reste l'égalité complète
    /// (`==`), elle est simplement filtrée par un hachage stable — deux événements
    /// ne sont confondus que s'ils sont égaux, une collision de hachage n'étant
    /// départagée que par la comparaison.
    par_cle: std::collections::HashMap<(EntityId, EpochId, u64), DedupGroupe>,
}

/// Les événements acceptés d'une même clé grossière, indexés par hachage fin.
#[derive(Default)]
struct DedupGroupe {
    /// Hachage stable de l'événement → positions dans `vus`.
    par_hachage: std::collections::HashMap<u64, Vec<usize>>,
    /// Les événements acceptés (source de vérité de l'égalité).
    vus: Vec<ChangeEvent>,
}

/// Un hachage STABLE et déterministe des champs qui décident de l'égalité.
///
/// `HashMap` a un ordre d'itération aléatoire : hacher les propriétés dans cet
/// ordre donnerait deux hachages différents pour deux événements égaux (le
/// dédoublonnage ne marcherait plus). On trie donc les clés — le coût est sur le
/// nombre de propriétés d'un événement, pas sur la taille du groupe.
fn hachage_fin(ev: &ChangeEvent) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // `kind` n'implémente pas `Hash` : on hache son discriminant textuel.
    format!("{:?}", ev.kind).hash(&mut h);
    for carte in [&ev.before, &ev.after] {
        match carte {
            None => 0u8.hash(&mut h),
            Some(props) => {
                1u8.hash(&mut h);
                let mut cles: Vec<&String> = props.keys().collect();
                cles.sort();
                for cle in cles {
                    cle.hash(&mut h);
                    format!("{:?}", props.get(cle)).hash(&mut h);
                }
            }
        }
    }
    h.finish()
}

impl DedupCdc {
    /// Enregistre `ev` s'il n'a jamais été vu ; rend `true` dans ce cas.
    ///
    /// Rend `false` quand l'événement est **exactement** celui déjà vu — et non
    /// quand il lui ressemble : c'est toute la différence entre dédupliquer et
    /// effacer.
    fn neuf(&mut self, ev: &ChangeEvent) -> bool {
        let cle = (ev.entity_id, ev.epoch, ev.timestamp);
        let h = hachage_fin(ev);
        let groupe = self.par_cle.entry(cle).or_default();
        if let Some(candidats) = groupe.par_hachage.get(&h) {
            // Seuls les événements qui HACHENT pareil sont comparés : le coût ne
            // dépend plus de la taille du groupe, seulement des collisions.
            if candidats.iter().any(|i| groupe.vus[*i] == *ev) {
                return false;
            }
        }
        let position = groupe.vus.len();
        groupe.vus.push(ev.clone());
        groupe.par_hachage.entry(h).or_default().push(position);
        true
    }

    /// Marque un événement DÉJÀ connu (pré-remplissage), sans le rendre.
    fn deja_connu(&mut self, ev: &ChangeEvent) {
        let _ = self.neuf(ev);
    }
}

/// Rétention LÉGÈRE de la vue mémoire : seuls les `Update` gardent leurs
/// payloads (`before`/`after`), les autres les perdent — la forme servie par
/// les recherches n'en a pas besoin, et la zone garde tout.
///
/// Une seule fonction pour les DEUX côtés de la voie froide : appliquée d'un
/// seul côté, elle rendrait les deux copies du même événement dissemblables, et
/// la déduplication (qui compare l'événement entier) le compterait deux fois.
fn retenir_leger(ev: &mut ChangeEvent) {
    if !matches!(ev.kind, ChangeKind::Update) {
        ev.before = None;
        ev.after = None;
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_history() {
        let log = CdcLog::new();
        let node_id = NodeId::new(1);

        log.record_create_node(node_id, EpochId(1), None);
        log.record_update(
            EntityId::Node(node_id),
            EpochId(2),
            "name",
            None,
            Value::from("Alix"),
        );
        log.record_update(
            EntityId::Node(node_id),
            EpochId(3),
            "name",
            Some(Value::from("Alix")),
            Value::from("Gus"),
        );

        let history = log.history(EntityId::Node(node_id));
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].kind, ChangeKind::Create);
        assert_eq!(history[1].kind, ChangeKind::Update);
        assert_eq!(history[2].kind, ChangeKind::Update);
    }

    #[test]
    fn test_history_since() {
        let log = CdcLog::new();
        let node_id = NodeId::new(1);

        log.record_create_node(node_id, EpochId(1), None);
        log.record_update(
            EntityId::Node(node_id),
            EpochId(5),
            "name",
            None,
            Value::from("Alix"),
        );
        log.record_update(
            EntityId::Node(node_id),
            EpochId(10),
            "name",
            Some(Value::from("Alix")),
            Value::from("Gus"),
        );

        let since_5 = log.history_since(EntityId::Node(node_id), EpochId(5));
        assert_eq!(since_5.len(), 2);
        assert_eq!(since_5[0].epoch, EpochId(5));
    }

    #[test]
    fn test_changes_between() {
        let log = CdcLog::new();

        log.record_create_node(NodeId::new(1), EpochId(1), None);
        log.record_create_node(NodeId::new(2), EpochId(3), None);
        log.record_update(
            EntityId::Node(NodeId::new(1)),
            EpochId(5),
            "x",
            None,
            Value::from(42),
        );

        let changes = log.changes_between(EpochId(2), EpochId(5));
        assert_eq!(changes.len(), 2); // epoch 3 and 5
    }

    #[test]
    fn test_delete_event() {
        let log = CdcLog::new();
        let node_id = NodeId::new(1);

        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::from("Alix"));

        log.record_create_node(node_id, EpochId(1), Some(props.clone()));
        log.record_delete(EntityId::Node(node_id), EpochId(2), Some(props));

        let history = log.history(EntityId::Node(node_id));
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].kind, ChangeKind::Delete);
        assert!(history[1].after.is_none());
        assert!(history[1].before.is_some());
    }

    #[test]
    fn test_empty_history() {
        let log = CdcLog::new();
        let history = log.history(EntityId::Node(NodeId::new(999)));
        assert!(history.is_empty());
    }

    #[test]
    fn test_event_count() {
        let log = CdcLog::new();
        assert_eq!(log.event_count(), 0);

        log.record_create_node(NodeId::new(1), EpochId(1), None);
        log.record_create_node(NodeId::new(2), EpochId(2), None);
        assert_eq!(log.event_count(), 2);
    }

    #[test]
    fn test_entity_id_conversions() {
        let node_id = NodeId::new(42);
        let entity: EntityId = node_id.into();
        assert!(entity.is_node());
        assert_eq!(entity.as_u64(), 42);

        let edge_id = EdgeId::new(7);
        let entity: EntityId = edge_id.into();
        assert!(!entity.is_node());
        assert_eq!(entity.as_u64(), 7);
    }

    /// (2026-09-06, cdc_tail) La voie froide sert la nouveauté récente depuis
    /// l'index de queue SANS recharger la fenêtre ; elle applique le filtre,
    /// retombe sur le chemin historique quand `since` est trop vieux pour la
    /// couverture de l'index, et répond « rien de neuf » sans recharger.
    #[test]
    fn test_changes_since_timestamp_cold_tail() {
        use crate::cdc_persist::CdcPersist;
        let dir = tempfile::tempdir().unwrap();
        let store =
            obrain_substrate::SubstrateStore::create(dir.path().join("cdc_cold_tail")).unwrap();
        let persist = CdcPersist::open(std::sync::Arc::new(store));
        let mut after = HashMap::new();
        after.insert("k".to_string(), Value::from(42));
        for ts in 1..=20u64 {
            persist
                .append(&ChangeEvent {
                    entity_id: EntityId::Node(NodeId::new((ts % 3) + 1)),
                    kind: ChangeKind::Update,
                    epoch: EpochId(ts),
                    timestamp: ts,
                    before: None,
                    after: Some(after.clone()),
                })
                .unwrap();
        }
        let log = CdcLog::new();
        log.attach_persist(persist);
        // Historique jamais résident → la zone est servie par l'index de queue.
        let got = log.changes_since_timestamp(10, |_| true);
        let ts: Vec<u64> = got.iter().map(|e| e.timestamp).collect();
        assert_eq!(ts, (10..=20).collect::<Vec<u64>>());
        // Rien de neuf depuis le dernier événement → vide, sans rechargement.
        assert!(log.changes_since_timestamp(21, |_| true).is_empty());
        // Le filtre est appliqué sur la voie froide.
        let got1 =
            log.changes_since_timestamp(10, |e| e.entity_id == EntityId::Node(NodeId::new(1)));
        let ts1: Vec<u64> = got1.iter().map(|e| e.timestamp).collect();
        // entity = ts % 3 + 1 == 1 ⟺ ts ≡ 0 mod 3 → 12, 15, 18.
        assert_eq!(ts1, vec![12, 15, 18]);
        // `since` trop vieux (< première entrée de queue) → rechargement borné
        // (chemin historique) : toute la zone, une fois pour ce process.
        let got0 = log.changes_since_timestamp(0, |_| true);
        assert_eq!(got0.len(), 20);
        // Le chemin chaud (résident) répond alors à l'identique.
        let hot = log.changes_since_timestamp(10, |_| true);
        let ts_hot: Vec<u64> = hot.iter().map(|e| e.timestamp).collect();
        assert_eq!(ts_hot, (10..=20).collect::<Vec<u64>>());
    }

    /// (2026-09-06, cdc_tail) La voie froide fusionne les événements de la
    /// zone avec les événements de la MÉMOIRE session non persistés (records
    /// antérieurs à l'attache) — la déduplication par (entité, epoch, ts)
    /// élimine les doublons quand un même événement est dans les deux.
    #[test]
    fn test_cold_tail_merge_memoire_session() {
        use crate::cdc_persist::CdcPersist;
        let dir = tempfile::tempdir().unwrap();
        let store =
            obrain_substrate::SubstrateStore::create(dir.path().join("cdc_cold_mem")).unwrap();
        let log = CdcLog::new();
        // Records AVANT l'attache : mémoire seule (zone encore absente).
        log.record_create_node(NodeId::new(7), EpochId(1), None);
        log.record_create_node(NodeId::new(8), EpochId(2), None);
        log.attach_persist(CdcPersist::open(std::sync::Arc::new(store)));
        // Zone vide + mémoire session → la voie froide sert les records.
        let got = log.changes_since_timestamp(0, |_| true);
        let mut ids: Vec<u64> = got.iter().map(|e| e.entity_id.as_u64()).collect();
        ids.sort();
        assert_eq!(ids, vec![7, 8]);
        // Rien de neuf très récent → vide (toujours sans rechargement).
        assert!(
            log.changes_since_timestamp(u64::MAX - 1, |_| true)
                .is_empty()
        );
    }
}
