//! EngramStore — in-memory cache with write-through to CognitiveStorage.
//!
//! Provides concurrent access to engrams via `DashMap` and atomic ID generation.
//!
//! ## Substrate backend (feature = "substrate")
//!
//! When the `substrate` feature is enabled and a [`SubstrateWriter`] is
//! attached via [`EngramStore::attach_substrate`], mutations are mirrored
//! to the substrate side-column layout introduced in T7 Step 1:
//!
//! * `substrate.engram_members` — directory-indexed snapshot of each engram's
//!   node ensemble (engram_id u16 → Vec<node_id u32>).
//! * `substrate.engram_bitset` — per-node 64-bit folded Bloom signature
//!   (bit `i` set ⇔ some engram with `id & 0x3F == i` claims the node).
//!
//! The bitset is monotone-union: [`EngramStore::remove`] clears the members
//! directory entry but leaves the bitset bits alone — a stale bit is a cheap
//! false positive resolved by a members-table lookup (source of truth).
//!
//! Three new semantic primitives are exposed on top of the substrate path:
//!
//! * [`EngramStore::form`] — allocate a new id, persist members, set each
//!   node's bit.
//! * [`EngramStore::recall`] — two-tier candidate retrieval (bitset pre-filter
//!   over the in-memory engram set, then members-table verification).
//! * [`EngramStore::members`] — read the members snapshot for an engram id.
//!
//! The existing public API (`insert`, `update`, `remove`, `load_from_graph`,
//! `find_by_node`, `list`, `get`, `next_id`, `count`) is preserved byte-for-byte
//! and continues to work without substrate: the substrate writer is strictly
//! optional and additive.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use obrain_common::types::{NodeId, Value};

use super::traits::CognitiveStorage;
use super::types::{Engram, EngramId};

#[cfg(feature = "substrate")]
use obrain_substrate::{
    SubstrateError, SubstrateResult, Writer as SubstrateWriter, engram_bit_mask,
};
#[cfg(feature = "substrate")]
use parking_lot::RwLock as PlRwLock;

/// Label used for engram nodes in the backing graph store.
const ENGRAM_LABEL: &str = ":Engram";

/// Property key carrying the full serialized engram (JSON).
const PROP_ENGRAM_DATA: &str = "_engram_data";

/// Property key carrying the engram ID (convenience for filtering).
const PROP_ENGRAM_ID: &str = "_engram_id";

/// Substrate engram ids are `u16` — we refuse to mirror engrams with ids
/// beyond this range. In practice the cognitive layer allocates ids
/// monotonically from 1 via [`EngramStore::next_id`]; the 65 535-slot
/// directory is sized to exhaust the namespace well before the ABI limit.
#[cfg(feature = "substrate")]
const MAX_SUBSTRATE_ENGRAM_ID: u64 = u16::MAX as u64;

/// In-memory engram store with optional write-through to [`CognitiveStorage`]
/// and mirrored substrate side-columns.
pub struct EngramStore {
    /// Concurrent cache of all engrams keyed by their ID.
    cache: DashMap<EngramId, Engram>,
    /// Optional backing storage for persistence (write-through).
    storage: Option<Arc<dyn CognitiveStorage>>,
    /// Mapping from EngramId to the backing graph node, when persisted.
    /// Used for `update` / `remove` write-through.
    node_ids: DashMap<EngramId, NodeId>,
    /// Monotonically increasing counter for ID generation.
    next_id: AtomicU64,
    /// Monotonically increasing mutation counter.
    ///
    /// Anything derived from the whole store (the Hopfield pattern matrix, for
    /// instance) can cache itself against this value instead of being rebuilt on
    /// every call. Bumped by `insert`, `update`, `remove` and `form`: a
    /// conservative rule, because both the signature and the precision of an
    /// engram take part in the derived data.
    revision: AtomicU64,
    /// Optional substrate writer for side-table + bitset mirror (T7 Step 2).
    #[cfg(feature = "substrate")]
    substrate: PlRwLock<Option<Arc<SubstrateWriter>>>,
    /// Injection de faute (test) : fait échouer `add_engram_bit` APRÈS un
    /// `set_engram_state` réussi — reproduit l'écriture partielle du substrat.
    #[cfg(feature = "fault-injection")]
    fail_bit_after_state: std::sync::atomic::AtomicBool,
    /// Injection de faute (test) : fait échouer la RÉPARATION au chargement
    /// (`ensure_engram_bit`) — reproduit un échec d'écriture du bitset pendant
    /// le balayage, qui laisse l'engramme DÉGRADÉ (bit manquant). Feature
    /// DÉDIÉE, absente de la production.
    #[cfg(feature = "fault-injection")]
    fail_repair_bit: std::sync::atomic::AtomicBool,
    /// Injection de faute (test) : fait échouer la MATÉRIALISATION dans le
    /// cache (`revise_with_id`, après l'application au registre) — expose
    /// l'incohérence registre-Appliqué / store-non-matérialisé.
    #[cfg(feature = "fault-injection")]
    fail_materialise: std::sync::atomic::AtomicBool,
    /// Registre des engrammes dont une projection (bitset) est dégradée.
    /// Sert au repli du rappel natif sur l'état canonique. Effacé à chaque
    /// chargement : reflète l'état courant, pas une seconde autorité.
    #[cfg(feature = "substrate")]
    degraded: PlRwLock<HashSet<EngramId>>,
}

impl std::fmt::Debug for EngramStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("EngramStore");
        dbg.field("cache_len", &self.cache.len())
            .field("has_storage", &self.storage.is_some())
            .field("next_id", &self.next_id.load(Ordering::Relaxed));
        #[cfg(feature = "substrate")]
        dbg.field("has_substrate", &self.substrate.read().is_some());
        dbg.finish()
    }
}

/// Bilan structuré d'un chargement depuis le substrate : distingue les
/// engrammes cohérents, les états canoniques valides dont une projection est
/// dégradée, et les états canoniques invalides écartés (id réservés).
/// Cause d'un rejet d'engramme au chargement : distingue corruption locale
/// (lecture), incohérence de longueurs, et échec de validation canonique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectCause {
    ReadError(String),
    InconsistentWeights { members: usize, weights: usize },
    Validation(String),
}

#[derive(Debug, Clone, Default)]
pub struct LoadReport {
    pub coherent: usize,
    pub degraded: Vec<(EngramId, String)>,
    pub rejected: Vec<(EngramId, RejectCause)>,
    /// Défaillance GLOBALE de la zone d'engrammes (magic/version/taille
    /// invalides) : le chargement s'est arrêté SANS balayer les 65 535
    /// identifiants. En sa présence, `rejected` est VIDE (une zone morte n'est
    /// pas un rejet individuel par identifiant — cela épuiserait l'espace
    /// d'identifiants). Une défaillance globale n'est JAMAIS présentée comme
    /// un chargement sain.
    pub global_failure: Option<String>,
}

/// Erreur de mutation du store : distingue absence et échec de persistance
/// (au lieu d'un échec silencieux journalisé).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    EngramAbsent(EngramId),
    ValidationFailed(String),
    PersistenceFailed(String),
}

impl EngramStore {
    /// Creates a new `EngramStore`.
    ///
    /// When `storage` is provided, every `insert` / `update` / `remove` is
    /// written through to the backing [`CognitiveStorage`] so engrams survive
    /// process restarts. Use [`EngramStore::load_from_graph`] on startup to
    /// rehydrate the cache from persisted `:Engram` nodes.
    pub fn new(storage: Option<Arc<dyn CognitiveStorage>>) -> Self {
        Self {
            cache: DashMap::new(),
            storage,
            node_ids: DashMap::new(),
            next_id: AtomicU64::new(1),
            revision: AtomicU64::new(0),
            #[cfg(feature = "substrate")]
            substrate: PlRwLock::new(None),
            #[cfg(feature = "substrate")]
            degraded: PlRwLock::new(HashSet::new()),
            #[cfg(feature = "fault-injection")]
            fail_bit_after_state: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "fault-injection")]
            fail_repair_bit: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "fault-injection")]
            fail_materialise: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Injection de faute (test) : TANT QUE ce drapeau est actif, le miroir
    /// retourne une erreur APRÈS `set_engram_state` et AVANT le premier appel
    /// réel à `add_engram_bit` — reproduit l'écriture partielle du substrat
    /// (membres+poids persistés, bitset non posé). Feature DÉDIÉE, absente de
    /// la production. Le drapeau reste actif jusqu'à désactivation explicite.
    #[cfg(feature = "fault-injection")]
    #[doc(hidden)]
    pub fn set_fail_bit_after_state(&self, fail: bool) {
        self.fail_bit_after_state
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    /// Injection de faute (test) : TANT QUE ce drapeau est actif, la
    /// RÉPARATION du bitset au chargement (`ensure_engram_bit`) échoue —
    /// l'engramme est chargé avec une projection DÉGRADÉE (bit manquant), ce
    /// qui exerce le repli du rappel natif sur l'état canonique. Feature
    /// DÉDIÉE, absente de la production.
    #[cfg(feature = "fault-injection")]
    #[doc(hidden)]
    pub fn set_fail_repair_bit(&self, fail: bool) {
        self.fail_repair_bit
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    /// Injection de faute (test) : TANT QUE ce drapeau est actif, `mutate`
    /// échoue AVANT de committer le candidat au cache — reproduit l'échec de
    /// matérialisation APRÈS que le registre d'observation a dit `Applied`.
    #[cfg(feature = "fault-injection")]
    #[doc(hidden)]
    pub fn set_fail_materialise(&self, fail: bool) {
        self.fail_materialise
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    /// Vrai si la faute de matérialisation est armée (consommée par le manager).
    #[cfg(feature = "fault-injection")]
    #[doc(hidden)]
    pub fn take_fail_materialise(&self) -> bool {
        self.fail_materialise
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Attach a substrate writer. After this call every subsequent
    /// `insert` / `remove` / `load_from_graph` mutation is mirrored to the
    /// substrate engram side-column zones (members + bitset), and the
    /// substrate-native primitives ([`Self::form`], [`Self::recall`],
    /// [`Self::members`]) become usable.
    ///
    /// Pass `None`-wrapped to detach; re-attaching overwrites the previous
    /// writer.
    #[cfg(feature = "substrate")]
    pub fn attach_substrate(&self, writer: Arc<SubstrateWriter>) {
        *self.substrate.write() = Some(writer);
    }

    /// Returns `true` when a substrate writer is attached.
    #[cfg(feature = "substrate")]
    pub fn has_substrate(&self) -> bool {
        self.substrate.read().is_some()
    }

    /// Returns the next unique [`EngramId`] and advances the counter.
    pub fn next_id(&self) -> EngramId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        EngramId(id)
    }

    /// Current mutation revision. See [`EngramStore::revision`]'s field docs.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Visits every engram **without cloning it**.
    ///
    /// `list()` clones the whole store - every ensemble, every signature, every
    /// recall history - which is why callers that only need one field paid for
    /// everything. Anything derived from the store should iterate through here.
    pub fn for_each<F: FnMut(&Engram)>(&self, mut f: F) {
        for entry in &self.cache {
            f(entry.value());
        }
    }

    /// Retrieves a clone of the engram with the given `id`, if present.
    pub fn get(&self, id: EngramId) -> Option<Engram> {
        self.cache.get(&id).map(|entry| entry.value().clone())
    }

    /// Serializes an engram into graph node properties.
    fn engram_properties(engram: &Engram) -> HashMap<String, Value> {
        let mut props = HashMap::new();
        props.insert(PROP_ENGRAM_ID.to_string(), Value::Int64(engram.id.0 as i64));
        // Full serialization as JSON — simplest path that preserves every
        // field (ensemble, spectral_signature, FSRS state, history, ...).
        match serde_json::to_string(engram) {
            Ok(json) => {
                props.insert(PROP_ENGRAM_DATA.to_string(), Value::String(json.into()));
            }
            Err(e) => {
                tracing::warn!(
                    engram_id = engram.id.0,
                    error = %e,
                    "Failed to serialize engram, skipping persistence"
                );
            }
        }
        props
    }

    /// Inserts an engram into the cache and writes through to storage.
    pub fn insert(&self, engram: Engram) {
        self.revision.fetch_add(1, Ordering::Relaxed);
        let id = engram.id;
        if let Some(storage) = &self.storage {
            // If we already have a node for this id, update it in place.
            let props = Self::engram_properties(&engram);
            if !props.contains_key(PROP_ENGRAM_DATA) {
                // Serialization failed, still cache but skip persistence.
                self.cache.insert(id, engram);
                return;
            }
            if let Some(nid) = self.node_ids.get(&id).map(|e| *e.value()) {
                storage.update_node(nid, &props);
            } else {
                let nid = storage.create_node(ENGRAM_LABEL, &props);
                self.node_ids.insert(id, nid);
            }
        }
        #[cfg(feature = "substrate")]
        let _ = self.mirror_substrate_insert(&engram);
        self.cache.insert(id, engram);
    }

    /// Updates an engram in-place by applying `f` to its mutable reference.
    ///
    /// This is a no-op if no engram with the given `id` exists.
    /// Reconstruit une projection DÉRIVÉE (signature spectrale) SANS muter
    /// l'état canonique : ni révision, ni write-through, ni miroir substrate.
    /// Le chargement re-dérive ainsi les signatures sans amplification
    /// d'écriture à chaque redémarrage.
    pub fn rehydrate_signature(&self, id: EngramId, sig: Vec<f64>) {
        if let Some(mut entry) = self.cache.get_mut(&id) {
            entry.value_mut().spectral_signature = sig;
            // Invalide le cache de la matrice Hopfield (indexé par révision)
            // SANS déclencher de write-through durable : une signature changée
            // sans révision laisserait une matrice obsolète réutilisée.
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Applique `f` à l'engramme `id` (modification en place + write-through),
    /// puis incrémente la révision.
    pub fn update(&self, id: EngramId, f: impl FnOnce(&mut Engram)) {
        // Chemin infaillible historique : délègue à la discipline commune.
        // Les erreurs de persistance sont journalisées (compatibilité) ; les
        // chemins critiques utilisent `mutate` qui les propage.
        if let Err(e) = self.mutate(id, f) {
            tracing::warn!(engram_id = id.0, error = ?e, "engram update failed");
        }
    }

    /// Removes and returns the engram with the given `id`, if present.
    pub fn remove(&self, id: EngramId) -> Option<Engram> {
        self.revision.fetch_add(1, Ordering::Relaxed);
        let removed = self.cache.remove(&id).map(|(_, engram)| engram);
        if removed.is_some()
            && let Some(storage) = &self.storage
            && let Some((_, nid)) = self.node_ids.remove(&id)
        {
            storage.delete_node(nid);
        }
        // Substrate mirror: clear the members directory entry. Bitset bits
        // are intentionally left in place (monotone-union semantics) —
        // stale bits are a cheap false positive resolved by the members
        // lookup in `recall`.
        #[cfg(feature = "substrate")]
        if removed.is_some() {
            self.mirror_substrate_remove(id);
        }
        removed
    }

    /// Rehydrates the cache from persisted `:Engram` nodes in the backing
    /// graph store. Returns the number of engrams loaded.
    ///
    /// Also advances the internal ID counter past the highest persisted ID
    /// so newly generated IDs do not collide with existing ones.
    pub fn load_from_graph(&self) -> usize {
        let Some(storage) = &self.storage else {
            return 0;
        };
        let nodes = storage.query_nodes(ENGRAM_LABEL, None);
        let mut loaded = 0usize;
        let mut max_id: u64 = 0;
        for node in nodes {
            let Some(Value::String(json)) = node.properties.get(PROP_ENGRAM_DATA) else {
                continue;
            };
            match serde_json::from_str::<Engram>(json.as_ref()) {
                Ok(engram) => {
                    let eid = engram.id;
                    max_id = max_id.max(eid.0);
                    self.node_ids.insert(eid, node.id);
                    #[cfg(feature = "substrate")]
                    let _ = self.mirror_substrate_insert(&engram);
                    self.cache.insert(eid, engram);
                    loaded += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        node_id = node.id.0,
                        error = %e,
                        "Failed to deserialize persisted engram"
                    );
                }
            }
        }
        // Advance the ID counter so next_id() starts above the max loaded.
        if max_id > 0 {
            let next = max_id.saturating_add(1);
            let current = self.next_id.load(Ordering::Relaxed);
            if next > current {
                self.next_id.store(next, Ordering::Relaxed);
            }
        }
        // ── UN CHARGEMENT EST UNE MUTATION : LA RÉVISION LE DIT ──────────────
        // Défaut mesuré (2026-09-11) : `revision` n'était incrémenté que par
        // `form`/`update`/`remove`. Or `recall.rs` affirme que le cache de
        // motifs et l'index de position sont invalidés « whenever
        // EngramStore::revision changes, so a mutation can never be served from
        // stale derived state ». Après un chargement, un rappel servi par le
        // cache rendait donc l'état D'AVANT le chargement : les engrams chargés
        // étaient invisibles jusqu'à la mutation suivante. On incrémente ici,
        // comme partout ailleurs où le contenu change.
        self.revision.fetch_add(1, Ordering::Relaxed);
        loaded
    }

    /// (2026-08-31) Recharge les engrams depuis le SUBSTRATE (zones
    /// `engram_members`) — prod-readiness §9.5 point 2 : les engrams
    /// formés via `form()` survivent au redémarrage. Itère les ids
    /// 1..=65535 (les slots vides sont des `None` rapides) et réhydrate
    /// le cache. La signature spectrale est re-encodée au prochain rappel.
    #[cfg(feature = "substrate")]
    pub fn load_from_substrate(&self) -> LoadReport {
        let mut report = LoadReport::default();
        let Some(writer) = self.substrate_writer() else {
            return report;
        };
        // DÉFAILLANCE GLOBALE : la zone d'engrammes est structurellement
        // invalide (magic/version/taille). On REFUSE le chargement SANS balayer
        // les 65 535 identifiants : un balayage transformerait une zone morte
        // en 65 535 rejets artificiels et épuiserait l'espace d'identifiants.
        // Une défaillance globale n'est JAMAIS présentée comme un chargement
        // sain (ni comme une série de rejets locaux).
        if let Err(e) = writer.validate_engram_zones() {
            tracing::error!(
                error = %e,
                "zone d'engrammes structurellement invalide : chargement refusé (défaillance globale)"
            );
            report.global_failure = Some(e.to_string());
            return report;
        }
        // CONTRAT DE CONCURRENCE (publication registre+cache au chargement) :
        // le registre dégradé est construit dans un ensemble LOCAL pendant le
        // balayage, puis publié d'un bloc à la fin (swap atomique). Jamais un
        // état intermédiaire « effacé puis repeuplé progressivement »
        // observable : un recall() concurrent pendant le chargement voit
        // l'ANCIEN registre jusqu'au swap final, puis le NOUVEAU. Le cache,
        // lui, croît de façon monotone (même contrat que load_from_graph).
        let mut degraded_local: HashSet<EngramId> = HashSet::new();
        let mut max_id: u64 = 0;
        for eid_u16 in 1u16..=u16::MAX {
            // Lecture COHÉRENTE membres+poids sous un même verrou (pas de
            // fenêtre d'entrelacement entre deux lectures séparées).
            let (members, weights) = match writer.read_engram_state(eid_u16) {
                Ok(Some(v)) => v,
                Ok(None) => continue, // slot vide
                Err(e) => {
                    // Zone corrompue : on RÉSERVE l'identifiant (pour ne jamais
                    // le réallouer) et on met l'engramme en quarantaine.
                    tracing::error!(
                        engram_id = eid_u16,
                        error = %e,
                        "état d'engramme illisible : identifiant réservé, engramme en quarantaine"
                    );
                    max_id = max_id.max(eid_u16 as u64);
                    report.rejected.push((EngramId(eid_u16 as u64), RejectCause::ReadError(e.to_string())));
                    continue;
                }
            };
            if members.is_empty() {
                continue;
            }
            // Slot occupé : réserve l'identifiant quoi qu'il arrive (même si
            // les poids sont incohérents ou absents).
            max_id = max_id.max(eid_u16 as u64);
            let eid = EngramId(eid_u16 as u64);
            // Restaure les poids (état dynamique) depuis la zone parallèle.
            let ensemble: Vec<(NodeId, f64)> = match weights {
                Some(ws) if ws.len() == members.len() => members
                    .into_iter()
                    .zip(ws)
                    .map(|(n, w)| (NodeId(n as u64), w as f64))
                    .collect(),
                // Absence legacy (base sans zone de poids) : 1.0 voulu.
                None => members
                    .into_iter()
                    .map(|n| (NodeId(n as u64), 1.0))
                    .collect(),
                // Incohérence de longueur : corruption — PAS de fabrication de
                // poids. Quarantaine (l'id est déjà réservé ci-dessus).
                Some(ws) => {
                    tracing::error!(
                        engram_id = eid_u16,
                        membres = members.len(),
                        poids = ws.len(),
                        "incohérence membres/poids : engramme en quarantaine (aucun poids inventé)"
                    );
                    report.rejected.push((EngramId(eid_u16 as u64), RejectCause::InconsistentWeights { members: members.len(), weights: ws.len() }));
                    continue;
                }
            };
            let mut engram = Engram::new(eid, ensemble);
            // VALIDATION de l'état canonique AVANT toute réparation/publication :
            // unicité des membres + poids finis (validate_candidate). Un état
            // invalide est écarté (id déjà réservé), pas réparé ni publié.
            if let Err(e) = self.validate_candidate(&engram) {
                tracing::error!(engram_id = eid_u16, error = ?e, "engramme invalide au chargement : écarté");
                report.rejected.push((eid, RejectCause::Validation(format!("{:?}", e))));
                continue;
            }
            // RÉPARATION des projections : reconstruire le bitset depuis les
            // membres validés, sans écriture inutile (ensure_engram_bit no-op si
            // le bit est déjà posé). Un échec marque la projection DÉGRADÉE mais
            // l'état canonique reste valide et publié.
            let mut degraded: Option<String> = None;
            for (nid, _) in &engram.ensemble {
                #[cfg(feature = "fault-injection")]
                let repair = if self.fail_repair_bit.load(std::sync::atomic::Ordering::Relaxed) {
                    Err(SubstrateError::WalBadFrame(
                        "injected ensure_engram_bit failure (repair)".into(),
                    ))
                } else {
                    writer.ensure_engram_bit(nid.0 as u32, eid_u16)
                };
                #[cfg(not(feature = "fault-injection"))]
                let repair = writer.ensure_engram_bit(nid.0 as u32, eid_u16);
                if let Err(e) = repair {
                    tracing::error!(engram_id = eid_u16, node_id = nid.0, error = %e, "projection bitset dégradée");
                    degraded = Some(e.to_string());
                }
            }
            // Signature spectrale : re-encodée paresseusement par le manager
            // (le rappel spectral l'utilise ; ici on la laisse vide, le
            // vecteur index sera ré-upserté au prochain `recall`).
            engram.spectral_signature = Vec::new();
            self.cache.insert(eid, engram);
            max_id = max_id.max(eid.0);
            if let Some(cause) = degraded {
                report.degraded.push((eid, cause));
                degraded_local.insert(eid);
            } else {
                report.coherent += 1;
            }
        }
        // Avance le compteur d'ids au-dessus du max chargé.
        if max_id > 0 {
            let next = max_id.saturating_add(1);
            let current = self.next_id.load(Ordering::Relaxed);
            if next > current {
                self.next_id.store(next, Ordering::Relaxed);
            }
        }
        // ── UN CHARGEMENT EST UNE MUTATION : LA RÉVISION LE DIT ──────────────
        // Défaut mesuré (2026-09-11) : `revision` n'était incrémenté que par
        // `form`/`update`/`remove`. Or `recall.rs` affirme que le cache de
        // motifs et l'index de position sont invalidés « whenever
        // EngramStore::revision changes, so a mutation can never be served from
        // stale derived state ». Après un chargement, un rappel servi par le
        // cache rendait donc l'état D'AVANT le chargement : les engrams chargés
        // étaient invisibles jusqu'à la mutation suivante. On incrémente ici,
        // comme partout ailleurs où le contenu change.
        // Publication ATOMIQUE du registre dégradé (remplacement d'un bloc) :
        // aucun lecteur ne voit un registre à moitié repeuplé.
        *self.degraded.write() = degraded_local;
        self.revision.fetch_add(1, Ordering::Relaxed);
        report
    }

    /// Returns a snapshot of all engrams currently in the cache.
    pub fn list(&self) -> Vec<Engram> {
        self.cache
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Finds all engram IDs whose ensemble contains the given `node_id`.
    pub fn find_by_node(&self, node_id: NodeId) -> Vec<EngramId> {
        self.cache
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .ensemble
                    .iter()
                    .any(|(nid, _)| *nid == node_id)
            })
            .map(|entry| *entry.key())
            .collect()
    }

    /// Returns the number of engrams in the cache.
    pub fn count(&self) -> usize {
        self.cache.len()
    }

    // =================================================================
    // Substrate-native semantic primitives (T7 Step 2)
    // =================================================================

    /// Allocate a fresh engram id, materialise a minimal [`Engram`] in the
    /// cache, and mirror the ensemble to the substrate side-columns.
    ///
    /// For each node in `nids` the folded-Bloom bit for the new id is OR-ed
    /// into the node's signature column, and the members snapshot is written
    /// once as a contiguous blob. The cache gets a default-strength engram
    /// so [`Self::recall`] / [`Self::find_by_node`] / [`Self::list`] all see
    /// the new engram without any external `insert` call.
    ///
    /// Returns an error if no substrate writer is attached, or if the
    /// allocated id would overflow the substrate `u16` engram namespace.
    /// Overflow is pre-checked before the id counter is bumped so a failed
    /// `form` does not waste a slot.
    #[cfg(feature = "substrate")]
    pub fn form(&self, nids: &[NodeId]) -> SubstrateResult<EngramId> {
        if self.substrate_writer().is_none() {
            return Err(SubstrateError::WalBadFrame(
                "no substrate writer attached".into(),
            ));
        }
        // Peek the next id without bumping the counter so we can fail before
        // mutating state.
        let peek = self.next_id.load(Ordering::Relaxed);
        if peek == 0 || peek > MAX_SUBSTRATE_ENGRAM_ID {
            return Err(SubstrateError::WalBadFrame(format!(
                "engram id {peek} overflows substrate u16 namespace (max {})",
                MAX_SUBSTRATE_ENGRAM_ID
            )));
        }
        let eid = self.next_id();
        let ensemble: Vec<(NodeId, f64)> = nids.iter().map(|&n| (n, 1.0)).collect();
        let engram = Engram::new(eid, ensemble);
        // `insert` handles: cache population, optional JSON write-through,
        // and substrate mirror (members + bitset) via mirror_substrate_insert.
        // Mirror failures are logged rather than propagated to preserve
        // insert's contract; we pre-validated the namespace above so the
        // happy path never hits a substrate error.
        self.insert(engram);
        Ok(eid)
    }

    /// Candidate-recall for `query_nid` via the two-tier substrate path.
    ///
    /// 1. Read the 64-bit bitset column for the node.
    /// 2. Enumerate cached engrams whose id mod 64 matches a set bit
    ///    (cheap in-memory filter).
    /// 3. Verify actual membership against the `substrate.engram_members`
    ///    side-table — stale bits from removed engrams or from Bloom
    ///    collisions are filtered out here.
    ///
    /// Returns the verified engram ids in ascending order. An empty
    /// bitset (== 0) short-circuits to an empty result : c'est une ABSENCE
    /// légitime (aucun engramme ne revendique le nœud), pas une erreur.
    /// Une erreur de LECTURE du bitset (zone corrompue) est PROPAGÉE en
    /// `Err` — jamais assimilée à une absence normale de connaissance.
    #[cfg(feature = "substrate")]
    pub fn recall(&self, query_nid: NodeId) -> SubstrateResult<Vec<EngramId>> {
        let writer = self
            .substrate_writer()
            .ok_or_else(|| SubstrateError::WalBadFrame("no substrate writer attached".into()))?;
        let nid_u32 = Self::node_id_to_u32(query_nid).ok_or_else(|| {
            SubstrateError::WalBadFrame("node id overflows u32 substrate addressing".into())
        })?;
        let bitset = writer.engram_bitset(nid_u32)?;
        let mut candidates: Vec<EngramId> = Vec::new();
        if bitset != 0 {
        // Two-tier filter: collect cached engrams whose bit is set, then
        // verify via the substrate members snapshot.
        for entry in &self.cache {
            let eid = *entry.key();
            let Some(eid_u16) = Self::engram_id_to_u16(eid) else {
                continue;
            };
            if bitset & engram_bit_mask(eid_u16) == 0 {
                continue;
            }
            // Bitset says "maybe" — verify against the members table.
            match writer.engram_members(eid_u16)? {
                Some(members) if members.contains(&nid_u32) => candidates.push(eid),
                _ => {}
            }
        }
        }
        // Repli sur l'état canonique des engrammes DÉGRADÉS (bitset incomplet) :
        // l'appartenance est vérifiée sur l'ensemble pondéré, même si le bit
        // manque. Ne présente pas une projection dégradée comme une absence.
        for eid in self.degraded.read().iter() {
            if let Some(engram) = self.cache.get(eid) {
                if engram.ensemble.iter().any(|(n, _)| *n == query_nid) && !candidates.contains(eid) {
                    candidates.push(*eid);
                }
            }
        }
        candidates.sort_by_key(|eid| eid.0);
        Ok(candidates)
    }

    /// Engrammes dont une projection (bitset) est dégradée.
    #[cfg(feature = "substrate")]
    pub fn degraded_engrams(&self) -> Vec<EngramId> {
        self.degraded.read().iter().copied().collect()
    }

    /// Read the members snapshot for `eid` directly from the substrate
    /// side-table. Returns an empty vector if the engram is unknown to
    /// substrate (never formed, or was cleared via [`Self::remove`]).
    #[cfg(feature = "substrate")]
    pub fn members(&self, eid: EngramId) -> SubstrateResult<Vec<NodeId>> {
        let writer = self
            .substrate_writer()
            .ok_or_else(|| SubstrateError::WalBadFrame("no substrate writer attached".into()))?;
        let Some(eid_u16) = Self::engram_id_to_u16(eid) else {
            return Ok(Vec::new());
        };
        let raw = writer.engram_members(eid_u16)?.unwrap_or_default();
        Ok(raw.into_iter().map(|n| NodeId(n as u64)).collect())
    }

    // --- substrate internals -----------------------------------------

    #[cfg(feature = "substrate")]
    fn substrate_writer(&self) -> Option<Arc<SubstrateWriter>> {
        self.substrate.read().as_ref().map(Arc::clone)
    }

    /// Persist the (id, members) pair + per-node bit for an engram whose id
    /// was allocated externally (used by `form` and by the insert mirror).
    #[cfg(feature = "substrate")]
    #[allow(dead_code)]
    fn form_with_id(&self, eid: EngramId, nids: &[NodeId]) -> SubstrateResult<()> {
        let writer = self
            .substrate_writer()
            .ok_or_else(|| SubstrateError::WalBadFrame("no substrate writer attached".into()))?;
        let eid_u16 = Self::engram_id_to_u16(eid).ok_or_else(|| {
            SubstrateError::WalBadFrame(format!(
                "engram id {} overflows substrate u16 namespace (max {})",
                eid.0, MAX_SUBSTRATE_ENGRAM_ID
            ))
        })?;
        let mut member_u32s: Vec<u32> = Vec::with_capacity(nids.len());
        for nid in nids {
            if let Some(n) = Self::node_id_to_u32(*nid) {
                member_u32s.push(n);
            } else {
                tracing::warn!(
                    node_id = nid.0,
                    "node id overflows substrate u32 namespace; skipping engram mirror"
                );
            }
        }
        writer.set_engram_members(eid_u16, member_u32s.clone())?;
        for nid in &member_u32s {
            writer.add_engram_bit(*nid, eid_u16)?;
        }
        Ok(())
    }

    /// Best-effort insert/update mirror: refreshes members snapshot and
    /// OR-s membership bits. Failures are logged but not propagated — the
    /// cognitive layer treats substrate as a derived view, not the source
    /// of truth.
    /// Discipline COMMUNE de mutation d'un engramme : applique `f` sous le
    /// verrou d'entrée du cache, persiste membres+poids (substrate) et
    /// write-through DANS la même section critique, puis n'incrémente la
    /// révision qu'APRÈS. Propage les erreurs de persistance.
    pub fn mutate(
        &self,
        id: EngramId,
        f: impl FnOnce(&mut Engram),
    ) -> Result<(), StoreError> {
        let Some(mut entry) = self.cache.get_mut(&id) else {
            return Err(StoreError::EngramAbsent(id));
        };
        // ÉTAT CANDIDAT : clone, applique, valide — SANS muter l'état publié.
        let mut candidate = entry.value().clone();
        f(&mut candidate);
        // RÉVISION CANONIQUE : toute mutation qui change l'état avance la
        // révision de CETTE connaissance (autorité unique du contrat
        // d'observation, quel que soit le chemin : revise, revise_with_id,
        // mutate, formation, update...).
        candidate.revision = candidate.revision.saturating_add(1);
        // Invariance de l'identité logique : une révision ne change pas l'id.
        if candidate.id != id {
            return Err(StoreError::ValidationFailed(
                "la révision ne doit pas changer l'identité logique".into(),
            ));
        }
        self.validate_candidate(&candidate)?;
        // Persister le candidat AVANT de committer : une erreur laisse le cache
        // inchangé. Le substrat peut être en écriture partielle/indéterminée.
        #[cfg(feature = "substrate")]
        self.mirror_substrate_insert(&candidate)?;
        if let Some(storage) = &self.storage {
            let props = Self::engram_properties(&candidate);
            if let Some(nid) = self.node_ids.get(&id).map(|e| *e.value()) {
                storage.update_node(nid, &props);
            } else {
                let nid = storage.create_node(ENGRAM_LABEL, &props);
                self.node_ids.insert(id, nid);
            }
        }
        // COMMIT du candidat dans l'entrée (sous le verrou).
        *entry.value_mut() = candidate;
        drop(entry);
        // Révision incrémentée APRÈS modification + persistance effectives.
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// MATÉRIALISE l'état canonique d'une observation dans le cache, en
    /// positionnant la révision à `max(révision_courante, révision_demandée)`
    /// (jamais de régression). Utilisé par le chemin d'application ET par la
    /// RÉPARATION d'une matérialisation manquante.
    ///
    /// La garde « ne jamais restaurer un ancien candidat sur une connaissance
    /// plus récente » appartient à l'APPELANT (comparer `revision` avant), pas à
    /// ce point qui matérialise ce qu'on lui donne.
    pub fn materialize_state(
        &self,
        id: EngramId,
        ensemble: Vec<(NodeId, f64)>,
        strength: f64,
        recall_count: u32,
        revision: u64,
    ) -> Result<bool, StoreError> {
        let Some(mut entry) = self.cache.get_mut(&id) else {
            return Err(StoreError::EngramAbsent(id));
        };
        let mut candidate = entry.value().clone();
        candidate.ensemble = ensemble;
        candidate.strength = strength;
        candidate.recall_count = recall_count;
        candidate.revision = candidate.revision.max(revision);
        self.validate_candidate(&candidate)?;
        #[cfg(feature = "substrate")]
        self.mirror_substrate_insert(&candidate)?;
        *entry.value_mut() = candidate;
        drop(entry);
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }

    /// Validation d'un candidat AVANT mutation : représentabilité u16/u32 et
    /// poids finis. Aucun chemin silencieux.
    fn validate_candidate(&self, e: &Engram) -> Result<(), StoreError> {
        // Validation GÉNÉRIQUE (indépendante du backend) : unicité des membres
        // et poids f64 finis. La représentabilité u16/u32/f32 est vérifiée par
        // mirror_substrate_insert (sous feature substrate), pas ici.
        let mut seen = HashSet::new();
        for (n, w) in &e.ensemble {
            if !w.is_finite() {
                return Err(StoreError::ValidationFailed(format!("poids non fini: {w}")));
            }
            if !seen.insert(*n) {
                return Err(StoreError::ValidationFailed(format!("membre dupliqué: {n:?}")));
            }
        }
        Ok(())
    }

    #[cfg(feature = "substrate")]
    fn mirror_substrate_insert(&self, engram: &Engram) -> Result<(), StoreError> {
        if self.substrate.read().is_none() {
            return Ok(());
        }
        let Some(writer) = self.substrate_writer() else {
            return Ok(());
        };
        let Some(eid_u16) = Self::engram_id_to_u16(engram.id) else {
            return Err(StoreError::ValidationFailed("EngramId non-u16".into()));
        };
        let mut members = Vec::with_capacity(engram.ensemble.len());
        let mut weights = Vec::with_capacity(engram.ensemble.len());
        for (n, w) in &engram.ensemble {
            let Some(nu32) = Self::node_id_to_u32(*n) else {
                return Err(StoreError::ValidationFailed(format!("NodeId non-u32: {n:?}")));
            };
            let wf32 = *w as f32;
            if !wf32.is_finite() {
                return Err(StoreError::ValidationFailed(format!(
                    "poids non représentable en f32: {w}"
                )));
            }
            members.push(nu32);
            weights.push(wf32);
        }
        // Écriture ATOMIQUE membres+poids (même verrou), puis bitset par nœud
        // (aucune correspondance d'ordre) — pas de réécriture des membres.
        writer
            .set_engram_state(eid_u16, members.clone(), weights)
            .map_err(|e| StoreError::PersistenceFailed(e.to_string()))?;
        // Point d'injection : échec AVANT add_engram_bit, après set_engram_state.
        #[cfg(feature = "fault-injection")]
        if self
            .fail_bit_after_state
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(StoreError::PersistenceFailed(
                "injected add_engram_bit failure".into(),
            ));
        }
        for nid in &members {
            writer
                .add_engram_bit(*nid, eid_u16)
                .map_err(|e| StoreError::PersistenceFailed(e.to_string()))?;
        }
        // Miroir complet RÉUSSI (membres + tous les bits posés) : la projection
        // n'est plus dégradée. On efface le marqueur — le registre dégradé ne
        // devient jamais une autorité périmée qui survivrait à une réparation
        // effective.
        self.degraded.write().remove(&engram.id);
        Ok(())
    }

    /// Best-effort remove mirror: clears the members directory entry.
    /// Bitset bits are deliberately left in place; stale bits are cheap
    /// false positives resolved by `recall` via the members lookup.
    #[cfg(feature = "substrate")]
    fn mirror_substrate_remove(&self, id: EngramId) {
        // L'engramme disparaît : son éventuel marqueur dégradé n'a plus lieu
        // d'être (pas de registre périmé pointant vers un id disparu).
        self.degraded.write().remove(&id);
        let Some(writer) = self.substrate_writer() else {
            return;
        };
        let Some(eid_u16) = Self::engram_id_to_u16(id) else {
            return;
        };
        // Suppression atomique : membres ET poids vides (set_engram_state
        // valide 0 == 0 et écrit les deux zones sous un même verrou).
        if let Err(e) = writer.set_engram_state(eid_u16, Vec::new(), Vec::new()) {
            tracing::warn!(
                engram_id = id.0,
                error = %e,
                "substrate engram mirror failed (remove)"
            );
        }
    }

    #[cfg(feature = "substrate")]
    fn engram_id_to_u16(id: EngramId) -> Option<u16> {
        if id.0 == 0 || id.0 > MAX_SUBSTRATE_ENGRAM_ID {
            None
        } else {
            Some(id.0 as u16)
        }
    }

    #[cfg(feature = "substrate")]
    fn node_id_to_u32(nid: NodeId) -> Option<u32> {
        if nid.0 > u32::MAX as u64 {
            None
        } else {
            Some(nid.0 as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_roundtrip() {
        let store = EngramStore::new(None);
        let id = store.next_id();
        let engram = Engram::new(id, vec![(NodeId(1), 1.0), (NodeId(2), 0.8)]);

        store.insert(engram.clone());
        assert_eq!(store.count(), 1);

        let fetched = store.get(id).expect("engram should exist");
        assert_eq!(fetched.id, id);
        assert_eq!(fetched.ensemble.len(), 2);

        let removed = store.remove(id).expect("engram should be removable");
        assert_eq!(removed.id, id);
        assert_eq!(store.count(), 0);
        assert!(store.get(id).is_none());
    }

    #[test]
    fn update_modifies_in_place() {
        let store = EngramStore::new(None);
        let id = store.next_id();
        let engram = Engram::new(id, vec![(NodeId(1), 1.0)]);
        store.insert(engram);

        store.update(id, |e| {
            e.strength = 0.99;
        });

        let updated = store.get(id).unwrap();
        assert!((updated.strength - 0.99).abs() < f64::EPSILON);
    }

    #[test]
    fn find_by_node_returns_matching_engrams() {
        let store = EngramStore::new(None);

        let id1 = store.next_id();
        store.insert(Engram::new(id1, vec![(NodeId(10), 1.0), (NodeId(20), 0.5)]));

        let id2 = store.next_id();
        store.insert(Engram::new(id2, vec![(NodeId(20), 1.0), (NodeId(30), 0.5)]));

        let id3 = store.next_id();
        store.insert(Engram::new(id3, vec![(NodeId(30), 1.0)]));

        let matches = store.find_by_node(NodeId(20));
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&id1));
        assert!(matches.contains(&id2));
    }

    #[test]
    fn list_returns_all_engrams() {
        let store = EngramStore::new(None);
        for _ in 0..5 {
            let id = store.next_id();
            store.insert(Engram::new(id, vec![(NodeId(1), 1.0)]));
        }
        assert_eq!(store.list().len(), 5);
    }

    #[test]
    fn next_id_is_monotonic() {
        let store = EngramStore::new(None);
        let a = store.next_id();
        let b = store.next_id();
        let c = store.next_id();
        assert!(a.0 < b.0);
        assert!(b.0 < c.0);
    }
}

#[cfg(test)]
mod tests_rehydratation {
    use super::*;

    /// `rehydrate_signature` reconstruit la projection DÉRIVÉE (signature
    /// spectrale) sans muter l'état canonique : elle invalide le cache
    /// Hopfield (révision +1) mais ne déclenche aucun write-through. Un id
    /// absent est un no-op silencieux.
    #[test]
    fn rehydrate_signature_reconstruit_la_projection_sans_reviser_l_engramme() {
        let store = EngramStore::new(None);
        let id = store.next_id();
        store.insert(Engram::new(id, vec![(NodeId(1), 1.0), (NodeId(2), 0.5)]));
        let rev_avant = store.revision();

        let sig = vec![0.1, 0.2, 0.3];
        store.rehydrate_signature(id, sig.clone());
        assert_eq!(store.get(id).unwrap().spectral_signature, sig);
        assert_eq!(
            store.revision(),
            rev_avant + 1,
            "le cache Hopfield est invalidé"
        );

        // Id absent : no-op, sans panique ni révision supplémentaire.
        let rev_apres = store.revision();
        store.rehydrate_signature(EngramId(999_999), vec![9.9]);
        assert_eq!(store.revision(), rev_apres);
    }
}
