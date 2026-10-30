//! # Per-edge-type degree registry (T17h T8)
//!
//! Maintains one [`DegreeColumn`](crate::degree_column::DegreeColumn)
//! per `edge_type_id: u16`, persisted as
//! `substrate.degrees.node.<edge_type_name>.u32`.
//!
//! ## Why
//!
//! T5 gave us a **total** out/in degree per node. That's enough for
//! `ORDER BY total_degree` queries, but **not** for Cypher patterns
//! that filter by edge type :
//!
//! ```cypher
//! MATCH (f:File)
//! OPTIONAL MATCH (f)-[:IMPORTS]->(imported:File)
//! WITH f, count(*) AS imports ORDER BY imports DESC LIMIT 50
//! ```
//!
//! Using the total degree here would inflate the count with non-IMPORTS
//! edges (CONTAINS, SYNAPSE, etc.). This module keeps per-type counters
//! so Cypher can replace the `walk + count per edge type` with an
//! O(1) lookup.
//!
//! ## Design (T17h T8)
//!
//! - **One file per edge type** : `substrate.degrees.node.<type_name>.u32`
//!   where `<type_name>` is the ASCII edge-type name from the registry
//!   (with a small filename-safe encoding for the few special chars).
//! - **Columns stored in a `DashMap<u16, Arc<TypedColumn>>`** (le wrapper
//!   `TypedColumn` ajoute un drapeau dirty par colonne, cf. sa doc) :
//!   lazy creation on first `incr_*` for a type; existing types picked
//!   up via directory scan at `open_all`.
//! - **Reuses [`DegreeColumn`]** verbatim — same header, same CRC, same
//!   atomic accessors. Each column is independent (no cross-type lock).
//! - **Total degree T5 is preserved** : writer hooks call BOTH total
//!   (T5) and typed (T8) increments. `sum_over_types(out_by_type[n]) ==
//!   total_out[n]` is an invariant verified by tests.
//!
//! ## Crash-safety
//!
//! Each column has its own CRC header. On open, corrupt columns return
//! `None` → the caller rebuilds via `rebuild_from_scan` (walks edges,
//! partitions by type). One corrupt type doesn't invalidate the
//! others.
//!
//! ## Memory / disk cost
//!
//! Per-type column = 64 B header + 8 B × n_slots. For a store with K
//! edge types and N nodes :
//!
//! - PO (1.36M nodes × 63 types) : ~630 MB total
//! - Wikipedia (4.56M × 5 types)  : ~185 MB
//! - Megalaw (8.13M × 10 types)   : ~650 MB
//!
//! Lazy creation mitigates this : rare edge types (< 1% of nodes) can
//! be left uncolumned (fallback walk). For now the simplest policy is
//! "create column on first edge of that type".

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::RwLock;

use crate::degree_column::DegreeColumn;
use crate::error::SubstrateResult;
use crate::file::SubstrateFile;

/// Filename prefix — full pattern is
/// `substrate.degrees.node.<type_name>.u32`. Strips the u32 suffix from
/// the T5 base filename to insert the type name mid-path.
pub const TYPED_DEGREE_FILENAME_PREFIX: &str = "substrate.degrees.node.";
pub const TYPED_DEGREE_FILENAME_SUFFIX: &str = ".u32";

/// Filename-safe encoding for a type name. Rejects characters that
/// would confuse the filesystem (path separators, `..`). In practice
/// edge type names in Obrain are ASCII identifiers
/// (`IMPORTS`, `HAS_FILE`, etc.) — this function mainly guards against
/// future additions.
pub fn filename_for_type(type_name: &str) -> String {
    let safe: String = type_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{TYPED_DEGREE_FILENAME_PREFIX}{safe}{TYPED_DEGREE_FILENAME_SUFFIX}")
}

/// First-init state for [`TypedDegreeRegistry`] — controls the
/// T17i T1 deferred initialisation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitState {
    /// Registry has been constructed but `ensure_initialized` hasn't
    /// been called yet — no sidecars opened, no edge scan performed.
    Uninitialized,
    /// Registry is fully hydrated ; subsequent calls to
    /// `ensure_initialized` are cheap no-ops.
    Initialized,
}

/// Handle d'une colonne de degrés typée + son drapeau **dirty**.
///
/// ## Pourquoi un drapeau SÉPARÉ de [`DegreeColumn`]
///
/// `DegreeColumn` est PARTAGÉ avec la colonne T5 totale (le `SubstrateStore`
/// la matérialise dans son propre `degrees_cell`). Y ajouter un champ `dirty`
/// l'armerait aussi sur le chemin T5, où l'on n'en veut pas. Le drapeau vit
/// donc CÔTÉ REGISTRE, armé uniquement par les mutations des colonnes TYPÉES.
///
/// ## Pourquoi ce drapeau existe (optimisation DIRTY-FLAG du flush)
///
/// `flush()` est appelé périodiquement (~60 s en production) sur des bases
/// où la plupart des types n'ont pas bougé. L'ancien code persistait CHAQUE
/// colonne `substrate.degrees.node.<type>.u32` à chaque flush :
/// `persist_header_crc()` (CRC IEEE sur TOUT le payload) + `msync()`, SANS
/// drapeau → O(E_types) msync par flush, ~0,83 ms/type, soit ~6 s à 7000
/// types, même quand RIEN n'a changé. Le drapeau fait tomber le coût
/// périodique à O(types_sales) : une colonne propre ne paie ni CRC ni msync.
struct TypedColumn {
    col: RwLock<DegreeColumn>,
    dirty: AtomicBool,
}

impl TypedColumn {
    /// Colonne fraîchement CRÉÉE (`create_by_filename`) : son en-tête (CRC=0)
    /// n'est pas encore msyncé, elle doit être persistée au prochain flush.
    fn created(col: DegreeColumn) -> Self {
        Self {
            col: RwLock::new(col),
            dirty: AtomicBool::new(true),
        }
    }

    /// Colonne OUVERTE depuis le disque (`open_by_filename`, CRC valide) :
    /// l'en-tête sur disque est déjà exact, rien à repersister.
    fn opened(col: DegreeColumn) -> Self {
        Self {
            col: RwLock::new(col),
            dirty: AtomicBool::new(false),
        }
    }

    /// Arme le drapeau dirty. Doit être appelé APRÈS la mutation du payload,
    /// SOUS le verrou de colonne (voir [`TypedDegreeRegistry::incr_out`]) —
    /// jamais avant, sinon un `flush()` concurrent qui efface le drapeau sous
    /// son write-lock laisserait la mutation invisible à la prochaine passe.
    #[inline]
    fn marquer_sale(&self) {
        self.dirty.store(true, Ordering::Release);
    }
}

/// Registry mapping `edge_type_id` → DegreeColumn. Created once per
/// store; internal DashMap supports concurrent lazy insertion.
///
/// ### T17i T1 deferred initialisation
///
/// Prior to T17i, `SubstrateStore::from_substrate` called
/// `init_typed_degrees` eagerly, which ran `open_existing_columns` —
/// one CRC header check + mmap per sidecar, observed at
/// ~1.3-4.7 ms/edge_type on PO/Megalaw respectively. For a hub with
/// frequent reopens this became the dominant startup cost (PO :
/// 8 ms → 93 ms).
///
/// The registry is now constructed empty at open time, and the
/// first hot-path call (`typed_degrees()` from `SubstrateStore`
/// accessor, triggered by create_edge / delete_edge / query
/// planner) invokes [`Self::ensure_initialized`] which takes an
/// exclusive lock and does the work exactly once. Concurrent
/// `create_edge` calls wait on the lock and do not double-count
/// (the in-flight edge is either fully incremented before the init
/// completes and included in the rebuild, or happens after the
/// init finishes and increments its own column).
pub struct TypedDegreeRegistry {
    /// `edge_type_id → Arc<TypedColumn>`. `Arc` so callers can
    /// `.clone()` the column handle for short-lived read-lock scopes.
    columns: DashMap<u16, Arc<TypedColumn>>,
    /// Needed for lazy file creation.
    substrate: Arc<parking_lot::Mutex<SubstrateFile>>,
    /// Edge-type name registry for filename resolution at persist /
    /// rebuild time. Same registry as `SubstrateStore::edge_types`.
    edge_type_names: Arc<RwLock<crate::store::EdgeTypeRegistry>>,
    /// First-init latch (T17i T1). Cheap `parking_lot::Mutex<InitState>` :
    /// `ensure_initialized` locks, checks, performs the hydration
    /// body once, and flips to `Initialized`. All subsequent calls
    /// observe `Initialized` under the lock and return immediately.
    init_state: parking_lot::Mutex<InitState>,
    /// Test-only : nombre de colonnes réellement persistées (CRC + msync)
    /// par [`Self::flush`]. Prouve, dans les tests, qu'un flush sur un
    /// store PROPRE n'en persiste AUCUNE (le coût périodique tombe à zéro
    /// quand rien n'est sale).
    #[cfg(test)]
    flush_persist_count: std::sync::atomic::AtomicUsize,
}

impl TypedDegreeRegistry {
    /// Construct an empty registry. Columns are created lazily on
    /// first `incr_*` for a type, or all-at-once via `rebuild_from_scan`.
    pub(crate) fn new(
        substrate: Arc<parking_lot::Mutex<SubstrateFile>>,
        edge_type_names: Arc<RwLock<crate::store::EdgeTypeRegistry>>,
    ) -> Self {
        Self {
            columns: DashMap::new(),
            substrate,
            edge_type_names,
            init_state: parking_lot::Mutex::new(InitState::Uninitialized),
            #[cfg(test)]
            flush_persist_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// T17i T1 — deferred first-init entry point. Exactly one
    /// caller runs the hydration body, the rest see `Initialized`
    /// under the lock and return immediately.
    ///
    /// ### Body semantics
    ///
    /// The `rebuild_scan` closure is invoked **only** when
    /// [`Self::open_existing_columns`] returns 0 (no sidecars
    /// persisted on disk yet — a fresh store or a pre-T8 upgrade).
    /// When sidecars exist, `open_existing_columns` mmaps them in
    /// place and the rebuild is skipped.
    ///
    /// ### Concurrency
    ///
    /// The `parking_lot::Mutex` serialises the first init. The two
    /// reasonable race paths :
    /// - Thread A locks and starts init ; thread B contends on the
    ///   mutex until A finishes. After A unlocks, B observes
    ///   `Initialized` and returns.
    /// - During A's rebuild scan, a concurrent `create_edge` may
    ///   call `incr_out`/`incr_in` on a column not-yet-in-scan-range.
    ///   The DashMap `get_or_create` atomically materialises the
    ///   column, the increment lands ; A's rebuild iterates the
    ///   persisted edges zone which already contains that edge
    ///   (mmap is authoritative), so it increments the same column
    ///   again — **and we get a double-count**. The caller must
    ///   therefore ensure `ensure_initialized` is driven to
    ///   completion **before** any concurrent mutator can fire. In
    ///   practice this means `SubstrateStore::typed_degrees()`
    ///   invokes ensure_initialized on every accessor call —
    ///   before returning, so `incr_typed_out_degree` /
    ///   `incr_typed_in_degree` always see a fully-initialised
    ///   registry.
    ///
    /// ### Idempotency
    ///
    /// Subsequent calls are O(1) lock acquisition + a cheap
    /// PartialEq compare. No rebuild scan is ever re-run.
    pub(crate) fn ensure_initialized<F>(&self, rebuild_scan: F) -> SubstrateResult<()>
    where
        F: FnOnce(&Self) -> SubstrateResult<()>,
    {
        let mut state = self.init_state.lock();
        if *state == InitState::Initialized {
            return Ok(());
        }
        // First init — do the work under the lock so concurrent
        // callers wait for us.
        let opened = self.open_existing_columns()?;
        if opened == 0 {
            rebuild_scan(self)?;
        }
        *state = InitState::Initialized;
        Ok(())
    }

    /// Test-only inspection : `true` once `ensure_initialized` has
    /// completed at least once on this registry.
    #[cfg(test)]
    // Test-support introspection hook; not called by production code.
    #[allow(dead_code)]
    pub(crate) fn is_initialized_for_test(&self) -> bool {
        *self.init_state.lock() == InitState::Initialized
    }

    /// Retrieve (or lazily create) the column for `edge_type_id`.
    /// Returns a clone of the Arc so callers can drop the registry
    /// guard before holding the column lock.
    ///
    /// NE MARQUE PAS la colonne sale : c'est le rôle des mutations
    /// ([`Self::incr_out`] / [`Self::incr_in`] / [`Self::zero_type`]), qui
    /// arment le drapeau APRÈS avoir écrit le payload. La création
    /// elle-même arme le drapeau (en-tête CRC=0 non encore msyncé).
    pub(crate) fn get_or_create(
        &self,
        edge_type_id: u16,
        n_slots_hint: u32,
    ) -> SubstrateResult<Arc<TypedColumn>> {
        // Fast path: column exists.
        if let Some(col) = self.columns.get(&edge_type_id) {
            return Ok(col.clone());
        }
        // Cold path: create. Use entry() or_insert_with to avoid race
        // (two threads both trying to create).
        let type_name = self
            .edge_type_names
            .read()
            .name_for(edge_type_id)
            .ok_or_else(|| {
                crate::error::SubstrateError::Internal(format!(
                    "typed_degree: unknown edge_type_id {edge_type_id}"
                ))
            })?
            .to_string();
        let entry = self.columns.entry(edge_type_id).or_try_insert_with(|| {
            let sub = self.substrate.lock();
            // Try open existing first (for bases where the column was
            // persisted previously). Fall back to create.
            let col = match DegreeColumn::open_by_filename(&sub, &filename_for_type(&type_name))? {
                // Ouverte depuis le disque : CRC valide, en-tête déjà exact →
                // colonne PROPRE.
                Some(c) => TypedColumn::opened(c),
                // Créée : en-tête CRC=0 posé en mémoire mais pas encore
                // msyncé → colonne SALE.
                None => TypedColumn::created(DegreeColumn::create_by_filename(
                    &sub,
                    &filename_for_type(&type_name),
                    n_slots_hint,
                )?),
            };
            Ok::<_, crate::error::SubstrateError>(Arc::new(col))
        })?;
        Ok(entry.value().clone())
    }

    /// Increment out-degree for `(edge_type_id, src_slot)` by `delta`.
    /// Creates the column lazily if absent. Safe under concurrent
    /// writers (atomic fetch_add inside read lock; grow via upgrade).
    ///
    /// ## Dirty-flag — armer SOUS le verrou, APRÈS la mutation
    ///
    /// Le drapeau est posé APRÈS `incr_out`, jamais avant : si `flush()`
    /// lit `dirty == false`, la mutation n'a pas eu lieu ; s'il lit
    /// `dirty == true`, il prend le write-lock et exclut toute mutation
    /// concurrente le temps du CRC+msync. Armer AVANT créerait la fenêtre
    /// « flush efface le drapeau sous son write-lock, puis l'incrémenteur
    /// bloque sur le read-lock et écrit APRÈS » → mutation perdue.
    pub fn incr_out(
        &self,
        edge_type_id: u16,
        slot: u32,
        delta: i32,
        n_slots_hint: u32,
    ) -> SubstrateResult<()> {
        let tc = self.get_or_create(edge_type_id, n_slots_hint)?;
        {
            let rl = tc.col.read();
            if slot < rl.n_slots() {
                rl.incr_out(slot, delta);
                tc.marquer_sale();
                return Ok(());
            }
        }
        let mut wl = tc.col.write();
        wl.ensure_slot(slot)?;
        wl.incr_out(slot, delta);
        tc.marquer_sale();
        Ok(())
    }

    /// (2026-09-05) Auto-guérison : remet à zéro la colonne d'un type
    /// (le rescan des arêtes suit dans `SubstrateStore`). L'appelant doit
    /// tenir le write-lock de la colonne — voir [`DegreeColumn::zero_out_in`].
    pub(crate) fn zero_type(
        &self,
        edge_type_id: u16,
        n_slots_hint: u32,
    ) -> SubstrateResult<()> {
        let tc = self.get_or_create(edge_type_id, n_slots_hint)?;
        // zero_out_in réécrit TOUT le payload : la colonne devient sale, même
        // si aucun `incr_*` ne suit (cas : type sans arête vivante après heal).
        {
            let mut wl = tc.col.write();
            wl.zero_out_in();
            tc.marquer_sale();
        }
        Ok(())
    }

    /// (2026-09-05) Sommes out/in de la colonne du type (slots 1..n_slots,
    /// sentinelle 0 exclue). `None` si aucune colonne n'est matérialisée
    /// pour ce type. Oracle côté colonne de la vérification de péremption.
    pub(crate) fn out_in_sums(&self, edge_type_id: u16) -> Option<(u64, u64)> {
        let tc = self.columns.get(&edge_type_id)?;
        let rl = tc.col.read();
        Some(rl.sum_out_in())
    }

    /// Increment in-degree for `(edge_type_id, dst_slot)`. Same as
    /// `incr_out` but for the second u32 of each record.
    pub fn incr_in(
        &self,
        edge_type_id: u16,
        slot: u32,
        delta: i32,
        n_slots_hint: u32,
    ) -> SubstrateResult<()> {
        let tc = self.get_or_create(edge_type_id, n_slots_hint)?;
        {
            let rl = tc.col.read();
            if slot < rl.n_slots() {
                rl.incr_in(slot, delta);
                tc.marquer_sale();
                return Ok(());
            }
        }
        let mut wl = tc.col.write();
        wl.ensure_slot(slot)?;
        wl.incr_in(slot, delta);
        tc.marquer_sale();
        Ok(())
    }

    /// Read out-degree for `(edge_type_id, slot)`. Returns 0 if no
    /// column exists for that type (no edges of that type ever
    /// created).
    pub fn out_degree(&self, edge_type_id: u16, slot: u32) -> u32 {
        self.columns
            .get(&edge_type_id)
            .map(|tc| tc.col.read().out_degree(slot))
            .unwrap_or(0)
    }

    /// Read in-degree for `(edge_type_id, slot)`. Returns 0 if no
    /// column exists for that type.
    pub fn in_degree(&self, edge_type_id: u16, slot: u32) -> u32 {
        self.columns
            .get(&edge_type_id)
            .map(|tc| tc.col.read().in_degree(slot))
            .unwrap_or(0)
    }

    /// Chantier SlotRemappable — remap des colonnes de degrés typées
    /// (chemin vif). Matérialise d'abord toutes les colonnes persistées
    /// sur disque (`open_existing_columns` est idempotent : les colonnes
    /// déjà ouvertes — porteuses de compteurs non synchronisés — sont
    /// laissées telles quelles), puis permute chacune via
    /// [`DegreeColumn::remap_node_slots`] (out-of-place, en-tête
    /// CRC + epoch en dernier). Les colonnes au CRC invalide sur disque
    /// sont ignorées par `open_existing_columns` — elles sont déjà
    /// « absentes » et repasseront par le rebuild.
    pub(crate) fn remap_node_slots_all(
        &self,
        old_to_new: &[u32],
        epoch: u32,
    ) -> SubstrateResult<()> {
        let _ = self.open_existing_columns()?;
        for entry in self.columns.iter() {
            // `remap_node_slots` persiste DÉJÀ l'en-tête (CRC+epoch) et fait
            // son propre msync+fsync en interne : la colonne est durable à la
            // sortie, le drapeau dirty n'est donc PAS touché ici.
            entry
                .value()
                .col
                .write()
                .remap_node_slots(old_to_new, epoch)?;
        }
        Ok(())
    }

    /// Persist open columns : compute CRC, msync. Called from
    /// `SubstrateStore::flush`.
    ///
    /// ## Optimisation DIRTY-FLAG
    ///
    /// Ne persiste QUE les colonnes sales (`dirty == true`). Une colonne
    /// propre n'a pas bougé depuis son dernier flush réussi : son en-tête
    /// sur disque est déjà exact, on ne paie ni CRC ni msync. C'est ce qui
    /// fait tomber le coût du flush périodique de O(E_types) à O(types_sales).
    pub fn flush(&self) -> SubstrateResult<()> {
        for entry in self.columns.iter() {
            let tc = entry.value();
            // Pré-filtre SANS verrou : le drapeau est un AtomicBool, le lire
            // ne coûte rien et une colonne propre est la grande majorité des
            // cas en régime périodique.
            if !tc.dirty.load(Ordering::Acquire) {
                continue;
            }
            let mut col = tc.col.write();
            // Le write-lock exclut tout `incr_*` concurrent : aucun nouvel
            // incrément ne peut se glisser entre le CRC et le msync.
            col.persist_header_crc();
            col.msync()?;
            // Effacer SOUS le write-lock : un mutateur bloqué sur read/write
            // re-armera le drapeau APRÈS ce store (voir `marquer_sale`).
            tc.dirty.store(false, Ordering::Release);
            #[cfg(test)]
            self.flush_persist_count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Test-only : nombre de colonnes SALE. Prouve, dans les tests, qu'un
    /// flush sur un store propre n'arme aucun drapeau (donc ne msync rien).
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn dirty_count_for_test(&self) -> usize {
        self.columns
            .iter()
            .filter(|e| e.value().dirty.load(Ordering::Acquire))
            .count()
    }

    /// Test-only : nombre total de colonnes persistées (CRC+msync) par tous
    /// les appels à [`Self::flush`] sur ce registre.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn flush_persist_count_for_test(&self) -> usize {
        self.flush_persist_count.load(Ordering::Relaxed)
    }

    /// Returns the number of edge-type columns currently materialized.
    /// For observability / tests.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Returns true if no column has been created yet.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Returns all (edge_type_id, edge_type_name) pairs for which a
    /// column exists. For observability / debug.
    pub fn edge_type_ids(&self) -> Vec<u16> {
        let mut ids: Vec<u16> = self.columns.iter().map(|e| *e.key()).collect();
        ids.sort_unstable();
        ids
    }

    /// Scan the substrate directory for existing
    /// `substrate.degrees.node.<type>.u32` files and open each one into
    /// the registry. Idempotent — a type whose column is already
    /// materialised is left untouched.
    ///
    /// Returns the number of columns opened (freshly picked up from
    /// disk this call — already-materialised ones aren't counted).
    ///
    /// Used at open time after restore_counters + init_degrees (T5) to
    /// hydrate typed columns for bases that persisted them in a prior
    /// session. Bases with no prior typed-column sidecars return 0,
    /// and the caller is expected to invoke the rebuild path.
    pub(crate) fn open_existing_columns(&self) -> SubstrateResult<usize> {
        // NOTE(SlotRemappable) : appelé aussi par `remap_node_slots_all` —
        // toute évolution doit rester idempotente vis-à-vis des colonnes
        // déjà matérialisées (ne jamais écraser un handle vivant).
        let dir_path = {
            let sub = self.substrate.lock();
            sub.path().to_path_buf()
        };
        let rd = match std::fs::read_dir(&dir_path) {
            Ok(rd) => rd,
            Err(_) => return Ok(0),
        };
        let mut opened = 0usize;
        for entry in rd.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            // Match `substrate.degrees.node.<type>.u32`, but reject the
            // plain-total T5 sidecar `substrate.degrees.node.u32` —
            // that one has no embedded type name.
            let Some(rest) = name_str.strip_prefix(TYPED_DEGREE_FILENAME_PREFIX) else {
                continue;
            };
            let Some(type_name) = rest.strip_suffix(TYPED_DEGREE_FILENAME_SUFFIX) else {
                continue;
            };
            if type_name.is_empty() {
                continue;
            }
            let reg = self.edge_type_names.read();
            let type_id = match reg.id_for(type_name) {
                Some(id) => id,
                None => {
                    // Orphan column — type name not in registry. Skip
                    // silently (could be from a pre-pruning compaction).
                    drop(reg);
                    continue;
                }
            };
            drop(reg);
            // Fast path — column already materialised (e.g. after a
            // prior incr). Skip re-open.
            if self.columns.contains_key(&type_id) {
                continue;
            }
            // Direct open path : bypasses get_or_create so a CRC-bad
            // file returns None cleanly (get_or_create would fall
            // through to create_by_filename with n_slots_hint=0, which
            // silently replaces the bad column with an empty one —
            // silent data loss). Here we skip bad columns ; the caller
            // is expected to trigger a rebuild for orphaned types.
            let sub = self.substrate.lock();
            let col_opt = DegreeColumn::open_by_filename(&sub, &filename_for_type(type_name))?;
            drop(sub);
            let Some(col) = col_opt else {
                // Corrupt CRC or unexpected shape — leave to rebuild.
                continue;
            };
            self.columns.insert(type_id, Arc::new(TypedColumn::opened(col)));
            opened += 1;
        }
        Ok(opened)
    }
}

/// Chantier SlotRemappable — enrôlement du registre de degrés typés dans
/// les repermutations de slots nœuds (chemin vif ; le chemin rejeu passe
/// par `crate::slot_remap::remap_sidecar_files_for_replay`, qui balaie
/// les fichiers `substrate.degrees.node.*.u32` sur disque).
impl crate::slot_remap::SlotRemappable for TypedDegreeRegistry {
    fn remap_name(&self) -> &'static str {
        "typed_degrees"
    }

    fn remap_node_slots(
        &self,
        _substrate: &Arc<parking_lot::Mutex<crate::file::SubstrateFile>>,
        old_to_new: &[u32],
        epoch: u32,
        _ctx: crate::slot_remap::RemapContext,
    ) -> SubstrateResult<()> {
        self.remap_node_slots_all(old_to_new, epoch)
    }
}
