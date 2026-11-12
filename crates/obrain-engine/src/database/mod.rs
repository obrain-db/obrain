//! The main database struct and operations.
//!
//! Start here with [`ObrainDB`] - it's your handle to everything.
//!
//! Operations are split across focused submodules:
//! - `query` - Query execution (execute, execute_cypher, etc.)
//! - `crud` - Node/edge CRUD operations
//! - `index` - Property, vector, and text index management
//! - `search` - Vector, text, and hybrid search
//! - `embed` - Embedding model management
//! - `persistence` - Save, load, snapshots, iteration
//! - `admin` - Stats, introspection, diagnostics, CDC
//! - `window` - 2D window queries per community (RFC zorder-superposition C1)

mod admin;
#[cfg(feature = "cognitive")]
pub(crate) mod annotator;
mod crud;
#[cfg(feature = "embed")]
mod embed;
mod index;
/// Persistance navigateur (wasm32 seulement) : `open_browser` et `persist`.
#[cfg(target_arch = "wasm32")]
mod navigateur;
/// (2026-09-07) Catalogue DURABLE des procedures stockees. Le catalogue GQL
/// (`CREATE PROCEDURE`) vit en RAM par processus — la persistance substrate
/// (`substrate.proccat`, meme pattern que les index) le rend rejouable a
/// l'ouverture : declaration au CREATE/DROP, hydratation au boot.
mod procedure_catalog;
/// (2026-09-17) CATALOGUE DE TYPES — meme constat que les procedures, memes
/// remedes : `CREATE NODE|EDGE TYPE` vivait en RAM par processus, donc
/// disparaissait au redemarrage (quatre tests de `wal_recovery` le disaient).
/// Declaration au CREATE/DROP, hydratation au boot depuis `substrate.typecat`.
mod schema_catalog;
/// (2026-09-17) L'INDEX TEXTE SURVIT AU PROCESSUS : les publications sont
/// persistees (`substrate.textidx.<label>.<propriete>`) avec une EMPREINTE du
/// store, et rechargees a l'ouverture quand elle correspond encore.
mod text_index_persist;
mod vector_index_persist;
// T17 final cutover (2026-04-23): the `persistence`, `mmap_store`,
// `nocache_reader`, and `native_writer` modules were deleted. They
// implemented the legacy single-file `.obrain` v1 (bincode snapshot)
// and v2 (mmap native) formats, which are retired in favor of the
// directory-based substrate backend. Users with a pre-substrate
// single-file database must use obrain-migrate ≤ v0.0.1 to convert
// to the directory-based LpgStore layout first, then a current
// release to convert that into substrate. Search archaeology:
// persistence.rs (1840 LOC) + mmap_store.rs (1315 LOC) +
// nocache_reader.rs (104 LOC) + native_writer.rs (133 LOC) +
// 26 feature-gated blocks in this file removed on 2026-04-23.
#[cfg(feature = "cdc")]
pub mod cdc_observer;
pub mod contient;
mod query;
#[cfg(feature = "rdf")]
mod rdf_ops;
pub mod reachable_count;
#[cfg(feature = "hybrid-search")]
pub mod hybrid_procedure;
mod search;
pub mod search_by_role;
pub mod search_causality;
#[cfg(feature = "cdc")]
pub mod search_conflicts;
#[cfg(feature = "cdc")]
pub mod search_delta;
#[cfg(feature = "cdc")]
pub mod search_relief;
#[cfg(feature = "cdc")]
pub mod search_surprise;
pub mod topological;
#[cfg(feature = "wal")]
pub mod wal_store;
mod window;

#[cfg(feature = "wal")]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use parking_lot::RwLock;

#[cfg(feature = "wal")]
use obrain_adapters::storage::wal::{
    DurabilityMode as WalDurabilityMode, LpgWal, WalConfig, WalRecord,
};
use obrain_common::memory::buffer::{BufferManager, BufferManagerConfig};
use obrain_common::utils::error::Result;
use obrain_core::graph::GraphStoreMut;

#[cfg(feature = "rdf")]
use obrain_core::graph::rdf::RdfStore;

use crate::catalog::Catalog;
use crate::config::Config;
use crate::query::cache::QueryCache;
use crate::session::Session;
use crate::transaction::TransactionManager;

/// Your handle to a Obrain database.
///
/// Start here. Create one with [`new_in_memory()`](Self::new_in_memory) for
/// quick experiments, or [`open()`](Self::open) for persistent storage.
/// Then grab a [`session()`](Self::session) to start querying.
///
/// # Examples
///
/// ```
/// use obrain_engine::ObrainDB;
///
/// // Quick in-memory database
/// let db = ObrainDB::new_in_memory();
///
/// // Add some data
/// db.create_node(&["Person"]);
///
/// // Query it
/// let session = db.session();
/// let result = session.execute("MATCH (p:Person) RETURN p")?;
/// # Ok::<(), obrain_common::utils::error::Error>(())
/// ```
pub struct ObrainDB {
    /// Database configuration.
    pub(super) config: Config,
    /// The underlying graph store.
    ///
    /// T17 Step 24 (2026-04-23): retyped from `Arc<LpgStore>` to the
    /// erased trait object. In substrate mode this holds a
    /// substrate-backed store; in the legacy path it holds an
    /// in-memory `LpgStore`. Both variants dispatch through
    /// `GraphStoreMut` — MVCC, named-graph, index-management, and
    /// introspection hooks all live on the trait with
    /// substrate-compatible defaults (LpgStore overrides delegate to
    /// its inherent impls).
    pub(super) store: Arc<dyn GraphStoreMut>,
    /// Schema and metadata catalog shared across sessions.
    pub(super) catalog: Arc<Catalog>,
    /// RDF triple store (if RDF feature is enabled).
    #[cfg(feature = "rdf")]
    pub(super) rdf_store: Arc<RdfStore>,
    /// Transaction manager.
    pub(super) transaction_manager: Arc<TransactionManager>,
    /// Unified buffer manager.
    pub(super) buffer_manager: Arc<BufferManager>,
    /// Write-ahead log manager (if durability is enabled).
    #[cfg(feature = "wal")]
    pub(super) wal: Option<Arc<LpgWal>>,
    /// Shared WAL graph context tracker. Tracks which named graph was last
    /// written to the WAL, so concurrent sessions can emit `SwitchGraph`
    /// records only when the context actually changes.
    #[cfg(feature = "wal")]
    pub(super) wal_graph_context: Arc<parking_lot::Mutex<Option<String>>>,
    /// Journal WAL of the RDF store (persistent substrate databases only).
    /// Triples are appended here and replayed on open; see `rdf_ops`.
    #[cfg(all(feature = "rdf", feature = "wal"))]
    pub(super) rdf_wal: Option<Arc<LpgWal>>,
    /// Query cache for parsed and optimized plans.
    pub(super) query_cache: Arc<QueryCache>,
    /// Shared commit counter for auto-GC across sessions.
    pub(super) commit_counter: Arc<AtomicUsize>,
    /// Whether the database is open.
    pub(super) is_open: RwLock<bool>,
    /// Change data capture log for tracking mutations.
    #[cfg(feature = "cdc")]
    pub(super) cdc_log: Arc<crate::cdc::CdcLog>,
    /// (2026-09-05) Observateur de mutations substrate actif (feature cdc) :
    /// quand vrai, les CREATE node/edge passent par l'observateur substrate —
    /// `crud.rs` neutralise alors ses propres enregistrements de création
    /// (anti-doublon) ; updates de propriétés inchangés (portés par crud).
    pub(super) cdc_observer_active: std::sync::atomic::AtomicBool,
    /// Registered embedding models for text-to-vector conversion.
    #[cfg(feature = "embed")]
    pub(super) embedding_models:
        RwLock<hashbrown::HashMap<String, Arc<dyn crate::embedding::EmbeddingModel>>>,
    /// External graph store (when using with_store()).
    /// When set, sessions route queries through this store instead of the built-in LpgStore.
    pub(super) external_store: Option<Arc<dyn GraphStoreMut>>,
    /// Typed [`SubstrateStore`] handle, retained alongside the erased
    /// `external_store` when the database is opened via `open_substrate`.
    ///
    /// Cognitive stores (energy/scar/utility/affinity/synapse) need the
    /// concrete substrate handle to call its column-view APIs
    /// (`reinforce_edge_synapse_f32`, `decay_all_edge_synapse`, etc.) which
    /// are not part of the `GraphStoreMut` trait. `None` when the database
    /// is backed by `LpgStore` (legacy path).
    ///
    /// [`SubstrateStore`]: obrain_substrate::SubstrateStore
    pub(super) substrate_store: Option<Arc<obrain_substrate::SubstrateStore>>,
    /// Miroir du journal OPFS, rempli par `open_browser` (wasm32 seulement).
    /// `persist` y publie l'instantané du store ; absent pour toute autre base.
    #[cfg(target_arch = "wasm32")]
    pub(super) miroir: Option<Arc<dyn obrain_substrate::miroir::Miroir>>,
    /// (2026-08-17, aretes inter-bases V2) Registre multi-store de la
    /// base : slug primaire + bases attachees (ouverture paresseuse).
    /// `None` tant que `attach_bases` n'a pas ete appele — une base
    /// mono-store ne paie rien et ne change pas de comportement.
    pub(super) base_registry:
        parking_lot::RwLock<Option<Arc<obrain_core::execution::BaseRegistry>>>,

    /// Index vectoriels du mode SUBSTRAT, cles par `"label:propriete"`.
    ///
    /// (2026-08-13, T17b) Pourquoi ici et pas sur `SubstrateStore`, ni dans le
    /// trait `GraphStoreMut` :
    ///
    /// - PAS dans le trait. Ses methodes d'index sont typees sur `HnswIndex`,
    ///   le type concret de `LpgStore`. Le moteur n'y appelle pas « un index
    ///   vectoriel » mais un HNSW — `config()`, `search_with_ef_and_filter()`,
    ///   un accessor de construction de graphe. Les retyper sur un contrat
    ///   general obligerait a y faire entrer des concepts HNSW : ce serait
    ///   salir l'abstraction pour faire passer des tests. La doc du trait dit
    ///   d'ailleurs depuis avril que les appelants « should dispatch on
    ///   `Arc<SubstrateStore>` directly in substrate-mode paths ».
    /// - PAS sur `SubstrateStore`. Le registre appartient a la BASE, qui est
    ///   ce sur quoi `create_vector_index` est une methode ; et le store fait
    ///   7 000 lignes qu'il est inutile de rouvrir pour ca.
    ///
    /// `RwLock` et non `Mutex` : les recherches sont concurrentes et ne
    /// prennent qu'une lecture partagee. Seuls `create_vector_index` et
    /// `drop_vector_index` prennent l'ecriture, et jamais pendant une
    /// recherche — la valeur est un `Arc` clone hors verrou.
    ///
    /// La valeur porte la dimension NATIVE en plus de l'index. L'index porte
    /// lui-meme la projection vers `L2_DIM` (identite + rembourrage en deca,
    /// PCA apprise au-dela — O-VEC C4). Garder la dimension native permet
    /// de REFUSER une requete de largeur incoherente au lieu de la rembourrer
    /// en silence — sans quoi une requete en 3 dimensions interrogerait
    /// joyeusement un index construit sur des embeddings en 128.
    /// Partage (`Arc`) avec l'observateur du store (T5) : il entretient ces index
    /// pour TOUT chemin d'ecriture, GQL compris.
    pub(super) substrate_vector_indexes: Arc<
        parking_lot::RwLock<
            std::collections::HashMap<String, (usize, Arc<obrain_substrate::SubstrateTieredIndex>)>,
        >,
    >,
    /// (O-VEC C3) Index vectoriels CHARGÉS depuis leurs zones nommées sans
    /// reconstruction, depuis l'ouverture.
    pub(super) index_vectoriel_charges: std::sync::atomic::AtomicUsize,
    /// (O-VEC C3) Index vectoriels RECONSTRUITS depuis la colonne (déclaration
    /// présente, zones absentes ou périmées), depuis l'ouverture.
    pub(super) index_vectoriel_reconstruits: std::sync::atomic::AtomicUsize,
    /// (O-VEC C3) Persistances d'index vectoriel réussies depuis l'ouverture.
    pub(super) index_vectoriel_persistes: std::sync::atomic::AtomicUsize,
    /// (O-VEC C3) Écritures de vecteurs de nœuds effectuées PAR LE MOTEUR (donc
    /// vues par l'entretien des index) ; comparé au compteur du store à la
    /// fermeture avant de persister un index.
    #[cfg(feature = "vector-index")]
    pub(super) ecritures_vectorielles_vues: Arc<std::sync::atomic::AtomicU64>,
    /// (O-VEC C3) Offset du WAL à la dernière synchronisation (chargement ou
    /// persistance) de chaque index vectoriel, par clé `label:prop`.
    #[cfg(feature = "vector-index")]
    pub(super) index_vectoriel_empreintes:
        parking_lot::RwLock<std::collections::HashMap<String, u64>>,
    /// Base ouverte en écriture PARTAGÉE (`open_shared_write`) : un pair peut
    /// écrire sans que ce processus le voie.
    pub(super) ecriture_partagee: bool,

    /// Index texte (BM25) du mode SUBSTRAT, cles par `"label:propriete"`.
    ///
    /// (2026-08-13, T17b) Le message de refus de `create_text_index` affirmait
    /// que « substrate mode exposes BM25 retrieval via SubstrateTieredIndex ».
    /// C'est FAUX : `SubstrateTieredIndex` est un cascade VECTORIEL (L0 bits /
    /// L1 bits / L2 f16), et un grep sur tout `obrain-substrate` ne trouve ni
    /// InvertedIndex ni BM25. Le substrat n'a aucun index texte.
    ///
    /// Il n'en a pas besoin : `obrain_core::index::text::InvertedIndex` se
    /// construit tres bien A PARTIR de donnees du substrat, en iterant
    /// `nodes_by_label` + `get_node_property` — c'est exactement ce que faisait
    /// deja le chemin LpgStore. Seul le REGISTRE manquait, et le defaut de
    /// trait `add_text_index` (corps vide) faisait passer l'enregistrement
    /// pour un succes.
    ///
    /// `RwLock` externe : plusieurs recherches concurrentes ne prennent qu'une
    /// lecture partagee du registre, et l'Arc est clone avant relachement.
    /// `RwLock` interne : impose par le type de `InvertedIndex`, qui est mute
    /// a l'insertion.
    ///
    /// GATE : ce champ nomme `obrain_core::index::text::InvertedIndex`, un
    /// type qui n'existe que sous la fonctionnalite `text-index`. Le champ
    /// doit donc porter le meme cfg que le type qu'il nomme — sans quoi le
    /// crate ne compile plus SEUL, alors que `cargo check --workspace` le
    /// masque en unifiant les fonctionnalites entre crates.
    /// Nombre d'index texte CHARGÉS depuis le disque (introspection : c'est ce
    /// qui permet de VÉRIFIER qu'une réouverture n'a pas reconstruit).
    pub(super) index_texte_charges: std::sync::atomic::AtomicUsize,
    /// Nombre d'index texte persistés (introspection).
    pub(super) index_texte_persistes: std::sync::atomic::AtomicUsize,
    /// L'empreinte (offset du WAL) à laquelle chaque index texte a été construit
    /// ou chargé : elle porte la validité INTER-SESSIONS (au réveil, l'offset du
    /// store dit si le fichier décrit encore ce store).
    #[cfg(feature = "text-index")]
    pub(super) index_texte_empreintes: parking_lot::RwLock<std::collections::HashMap<String, u64>>,
    /// L'EPOQUE à laquelle chaque index texte a été construit ou chargé : elle
    /// porte la validité INTRA-SESSION.
    ///
    /// ── POURQUOI DEUX SIGNAUX, ET PAS UN ────────────────────────────────────
    /// Mesuré : l'offset du WAL BOUGE à la fermeture elle-même (551 → 583, les
    /// écritures de flush), alors que l'EPOQUE, elle, ne bouge PAS
    /// (`epoque=5` avant et après la fermeture) et s'incrémente à chaque
    /// écriture (5 → 1 après réouverture, puis 2 après un `SET` de propriété).
    ///
    /// Aucun des deux ne suffit seul : l'offset empêcherait tout index d'être
    /// réputé à jour à la fermeture (il ne correspond jamais), et l'époch est
    /// remise à zéro à chaque réouverture (elle ne peut donc rien dire de la
    /// validité d'un fichier écrit par une session précédente). On demande donc à
    /// chacun ce qu'il sait : l'époch décide « cet index a-t-il vu une écriture
    /// depuis sa construction ? » (donc : réécrire ou effacer), l'offset décide
    /// « ce fichier décrit-il encore ce store ? » (donc : charger ou reconstruire).
    #[cfg(feature = "text-index")]
    pub(super) index_texte_epochs: parking_lot::RwLock<std::collections::HashMap<String, u64>>,
    /// `Arc` (et non `RwLock` nu) : la poignée de recherche hybride
    /// (`CALL obrain.hybrid_search`) en garde un clone, comme pour le
    /// registre vectoriel ci-dessus.
    #[cfg(feature = "text-index")]
    pub(super) substrate_text_indexes: Arc<
        parking_lot::RwLock<
            std::collections::HashMap<
                String,
                Arc<parking_lot::RwLock<obrain_core::index::text::InvertedIndex>>,
            >,
        >,
    >,
    /// Metrics registry shared across all sessions.
    #[cfg(feature = "metrics")]
    pub(crate) metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
    /// Persistent graph context for one-shot `execute()` calls.
    /// When set, each call to `session()` pre-configures the session to this graph.
    /// Updated after every one-shot `execute()` to reflect `USE GRAPH` / `SESSION RESET`.
    current_graph: RwLock<Option<String>>,
    /// Whether this database is open in read-only mode.
    /// When true, sessions automatically enforce read-only transactions.
    read_only: bool,
    /// Cognitive engine — unified access to energy, synapses, fabric, etc.
    /// Only present when the `cognitive` feature flag is enabled at compile time.
    #[cfg(feature = "cognitive")]
    cognitive_engine: Option<Arc<dyn obrain_cognitive::CognitiveEngine>>,
    /// Reactive scheduler (keeps the background task alive).
    #[cfg(feature = "cognitive")]
    _cognitive_scheduler: Option<obrain_reactive::Scheduler>,
}

// ---------------------------------------------------------------------------
// (V3, parade p8) Registre PROCESSUS des substrats vivants
// ---------------------------------------------------------------------------

/// Substrats VIVANTS de ce processus, par chemin canonique. `Weak` : le
/// registre ne retient rien — une base fermee disparait d'elle-meme.
///
/// Danger demontre par la verification adversariale (p8) : attacher PAR
/// CHEMIN une base deja ouverte dans le meme processus creait une seconde
/// instance des memes fichiers, aux sidecars divergents — les ecritures de
/// la premiere restaient invisibles a la seconde. L'ouvreur paresseux des
/// bases attachees consulte ce registre AVANT d'ouvrir.
static SUBSTRATS_VIVANTS: std::sync::LazyLock<
    parking_lot::Mutex<
        std::collections::HashMap<
            std::path::PathBuf,
            std::sync::Weak<obrain_substrate::SubstrateStore>,
        >,
    >,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// Chemin canonique (symlinks/relatifs resolus) — repli sur le chemin brut
/// si la canonisation echoue (chemin pas encore cree).
fn chemin_canonique(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Inscrit un substrat vivant (et purge les entrees mortes au passage).
fn enregistrer_substrat_vivant(
    path: &std::path::Path,
    store: &Arc<obrain_substrate::SubstrateStore>,
) {
    let mut carte = SUBSTRATS_VIVANTS.lock();
    carte.retain(|_, w| w.strong_count() > 0);
    carte.insert(chemin_canonique(path), Arc::downgrade(store));
}

/// Rend l'instance VIVANTE d'un chemin, si une base de ce processus la
/// tient encore ouverte.
/// Retire une base du registre des substrats vivants : appelé par `close()`,
/// parce qu'une base FERMÉE n'écrit plus — la rouvrir dans le même processus
/// est légitime, même si l'ancienne instance n'est pas encore `Drop`-ée.
fn desinscrire_substrat_vivant(path: &std::path::Path) {
    SUBSTRATS_VIVANTS.lock().remove(&chemin_canonique(path));
}

fn substrat_vivant(path: &std::path::Path) -> Option<Arc<obrain_substrate::SubstrateStore>> {
    SUBSTRATS_VIVANTS
        .lock()
        .get(&chemin_canonique(path))
        .and_then(std::sync::Weak::upgrade)
}

/// Branche le vocabulaire de l'hôte (UDF du registre de plugins) sur
/// l'évaluateur du moteur.
///
/// ── POURQUOI ICI, ET POURQUOI IDEMPOTENT ────────────────────────────────────
/// `obrain-adapters` porte `PluginRegistry::register_udf` et il était
/// **orphelin** : aucune base ne le lisait, donc une fonction enregistrée
/// restait invisible à `RETURN mon_algo(n)` — qui répondait « fonction
/// inconnue », en invitant même à vérifier l'orthographe.
///
/// Le pont se pose à la CONSTRUCTION de chaque base (les deux constructeurs
/// l'appellent) plutôt qu'à un endroit que chaque embedder devrait connaître :
/// une extension qui dépend de la mémoire de l'appelant n'est pas une
/// extension. L'appel est idempotent et sans coût mesurable après la première
/// fois (un `OnceLock`).
fn brancher_le_vocabulaire_de_l_hote() {
    obrain_adapters::plugins::installer_udfs_dans_le_moteur();
}

impl ObrainDB {
    /// Creates an in-memory database, fast to create, gone when dropped.
    ///
    /// Use this for tests, experiments, or when you don't need persistence.
    /// For data that survives restarts, use [`open()`](Self::open) instead.
    ///
    /// # Panics
    ///
    /// Panics if the internal arena allocator cannot be initialized (out of memory).
    /// Use [`with_config()`](Self::with_config) for a fallible alternative.
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::new_in_memory();
    /// let session = db.session();
    /// session.execute("INSERT (:Person {name: 'Alix'})")?;
    /// # Ok::<(), obrain_common::utils::error::Error>(())
    /// ```
    #[must_use]
    pub fn new_in_memory() -> Self {
        Self::with_config(Config::in_memory()).expect("In-memory database creation should not fail")
    }

    /// Opens a database at the given path, creating it if it doesn't exist.
    ///
    /// If you've used this path before, Obrain recovers your data from the
    /// write-ahead log automatically. First open on a new path creates an
    /// empty database.
    ///
    /// # Errors
    ///
    /// Returns an error if the path isn't writable or recovery fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::open("./my_social_network")?;
    /// # Ok::<(), obrain_common::utils::error::Error>(())
    /// ```
    #[cfg(feature = "wal")]
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        // T17 cutover: substrate is the single storage backend. The former
        // LpgStore-backed path (`Config::persistent` → `with_config`) and the
        // `OBRAIN_BACKEND=substrate` runtime knob are gone. Any lingering
        // `OBRAIN_BACKEND` env-var setting is ignored on purpose.
        Self::open_substrate(path)
    }

    /// Opens a database backed by the `SubstrateStore` (mmap + WAL native).
    ///
    /// This is the T5 migration path. Internally, it constructs a
    /// [`obrain_substrate::SubstrateStore`] at the given path and routes all
    /// queries and mutations through it via the [`with_store`] constructor.
    ///
    /// Since T17 cutover, [`ObrainDB::open`] delegates unconditionally to
    /// this method — substrate is the single storage backend. Kept as a
    /// public API for callers that want to express the substrate intent
    /// explicitly in tests or tooling.
    ///
    /// # Errors
    ///
    /// Returns an error if the substrate store cannot be created/opened at
    /// `path`.
    ///
    /// [`with_store`]: Self::with_store
    pub fn open_substrate(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::open_substrate_inner(path, false)
    }

    /// Ouvre le store en READ-ONLY (verrou partagé) quand `read_only=true`.
    /// `with_config(Config::read_only(...))` passe par ici : le verrou
    /// partagé du substrate permet à plusieurs processus de lire le même
    /// store simultanément (contrat promis par `Config::read_only`, tenu
    /// depuis 2026-08-31).
    pub(crate) fn open_substrate_read_only(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::open_substrate_inner(path, true)
    }

    fn open_substrate_inner(path: impl AsRef<std::path::Path>, read_only: bool) -> Result<Self> {
        let path_ref = path.as_ref();
        // ── UNE BASE, UNE INSTANCE PAR PROCESSUS (V1-EXHAUSTIF, 2026-11-01) ──
        // Le verrou de répertoire est PARTAGÉ entre les ouvertures d'un même
        // processus (`acquerir_verrou_repertoire` : « même processus : on
        // partage »), parce que plusieurs composants d'une même base le
        // prennent. Il ne refuse donc pas une seconde `ObrainDB` ouverte en
        // écriture sur le même chemin — deux instances des mêmes fichiers, aux
        // sidecars et compteurs divergents : les écritures de l'une sont
        // invisibles à l'autre (danger démontré par p8, et par
        // `test_multi_process_file_lock_rejected`, rouge en `--all-features`).
        // Le registre des substrats vivants, qui servait déjà l'ouvreur
        // paresseux des bases attachées, tranche ici aussi : une ouverture en
        // ÉCRITURE est refusée tant qu'une instance vit dans ce processus.
        // Les ouvertures en lecture seule restent permises (même contrat que
        // depuis un autre processus : verrou partagé, mmap propre).
        if !read_only && substrat_vivant(path_ref).is_some() {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "substrate {} is already open in this process — one writable \
                 ObrainDB per base and per process; drop the first instance (or \
                 open the second one read-only)",
                path_ref.display()
            )));
        }
        // Detect existing substrate: if `substrate.meta` is present, open;
        // otherwise create. This mirrors the semantics of ObrainDB::open(path)
        // on the legacy path (open-or-create idempotent).
        let meta_path = path_ref.join("substrate.meta");
        let store = if meta_path.is_file() {
            if read_only {
                obrain_substrate::SubstrateStore::open_read_only(path_ref).map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "substrate: failed to open existing store (read-only) at {}: {e}",
                        path_ref.display()
                    ))
                })?
            } else {
                obrain_substrate::SubstrateStore::open(path_ref).map_err(|e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "substrate: failed to open existing store at {}: {e}",
                        path_ref.display()
                    ))
                })?
            }
        } else if read_only {
            // ── LECTURE SEULE : ON N'INVENTE PAS UNE BASE ────────────────────
            // « Ouvrir ou créer » est le contrat de `open` ; en lecture seule il
            // CRÉAIT une base vide dans tout dossier qu'on lui montrait — le
            // dossier parent des bases, une faute de frappe, un répertoire
            // quelconque. Un lecteur ne crée rien : sans `substrate.meta`, ce
            // n'est pas une base, et l'ouverture échoue sans rien poser.
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "{} is not an obrain database (no substrate.meta) — a read-only \
                 open never creates one; check the path, or open it writable to \
                 create a new database",
                path_ref.display()
            )));
        } else {
            obrain_substrate::SubstrateStore::create(path_ref).map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "substrate: failed to create store at {}: {e}",
                    path_ref.display()
                ))
            })?
        };
        Self::with_substrate_store(store, Some(path_ref), read_only)
    }

    /// Opens (or creates) a substrate-backed database with an explicit WAL
    /// [`SyncMode`](obrain_substrate::wal_io::SyncMode) — T5 batch-WAL (plan
    /// b30650e2). `open_substrate`/`open` always use `EveryCommit` (fsync per
    /// commit); this entry point is for batch producers (imports, wardens)
    /// that accept a bounded durability window in exchange for throughput —
    /// pass `SyncMode::Group` and call [`ObrainDB::flush_wal`] every N writes
    /// / at end-of-batch. User-facing default behaviour is unchanged: this is
    /// purely additive, opt-in API.
    ///
    /// # Errors
    ///
    /// Returns an error if the substrate store cannot be created/opened at
    /// `path`.
    /// Identité de ce processus pour la numérotation des transactions (MP-8) :
    /// hachage aléatoire (graine par processus) de l'identifiant système et de
    /// l'horloge. Deux processus concurrents ont donc, en pratique, des identités
    /// distinctes ; la collision résiduelle est de l'ordre de 2^-31 par paire.
    fn identite_de_processus() -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        h.write_u128(nanos);
        h.finish() >> 33
    }

    /// Ouvre le store en SHARED-WRITE (verrou partagé + écriture WAL) :
    /// coexiste avec les lecteurs RO du service. Réservé au GraphWriter
    /// unique (langextract) — les mutations sont journalisées dans le WAL,
    /// les lecteurs les voient par le replay à leur ouverture.
    pub fn open_shared_write(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let meta_path = path_ref.join("substrate.meta");
        let store = if meta_path.is_file() {
            obrain_substrate::SubstrateStore::open_shared_write(path_ref).map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "substrate: failed to open existing store in shared-write mode at {}: {e}",
                    path_ref.display()
                ))
            })?
        } else {
            return Err(obrain_common::utils::error::Error::Internal(format!(
                "substrate: store introuvable en shared-write (doit exister): {}",
                path_ref.display()
            )));
        };
        let mut db = Self::with_substrate_store(store, Some(path_ref), false)?;
        // (O-VEC C3) Un pair peut écrire : la persistance des index vectoriels
        // à la fermeture applique la règle prudente (WAL immobile).
        db.ecriture_partagee = true;
        // MP-8 : identifiants de transaction uniques ENTRE processus (voir
        // `TransactionManager::numeroter_pour_processus`).
        db.transaction_manager
            .numeroter_pour_processus(Self::identite_de_processus());
        Ok(db)
    }

    /// Comme [`Self::open_substrate`], avec le mode de synchronisation du WAL
    /// choisi par l'appelant : ouvre le store si `substrate.meta` existe sous
    /// `path`, le crée sinon.
    ///
    /// # Errors
    ///
    /// Returns an error if the substrate store cannot be created/opened at
    /// `path`.
    pub fn open_substrate_with_mode(
        path: impl AsRef<std::path::Path>,
        sync_mode: obrain_substrate::wal_io::SyncMode,
    ) -> Result<Self> {
        let path_ref = path.as_ref();
        let meta_path = path_ref.join("substrate.meta");
        let store = if meta_path.is_file() {
            obrain_substrate::SubstrateStore::open_with_mode(path_ref, sync_mode).map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "substrate: failed to open existing store at {}: {e}",
                    path_ref.display()
                ))
            })?
        } else {
            obrain_substrate::SubstrateStore::create_with_mode(path_ref, sync_mode).map_err(
                |e| {
                    obrain_common::utils::error::Error::Internal(format!(
                        "substrate: failed to create store at {}: {e}",
                        path_ref.display()
                    ))
                },
            )?
        };
        Self::with_substrate_store(store, Some(path_ref), false)
    }

    /// Arme l'ÉCHÉANCE GLOBALE des requêtes sur une base déjà ouverte.
    ///
    /// ── POURQUOI CETTE ENTRÉE EXISTE (2026-08-24) ─────────────────────
    ///
    /// `Config::with_query_timeout` existe depuis longtemps, mais elle ne
    /// sert qu'à `ObrainDB::with_config`. Les appelants qui passent par
    /// `open_substrate` — c'est-à-dire TOUS les appelants de production
    /// depuis la bascule T17 — n'avaient aucun moyen de l'armer : le chemin
    /// construit sa `Config` lui-même. Résultat mesuré : `query_timeout`
    /// vaut `None` partout en production, donc `Executor::check_deadline`
    /// y est un no-op permanent, et aucune requête n'a jamais eu de limite
    /// au niveau du moteur.
    ///
    /// L'échéance vaut pour la vie de la base ouverte : `ObrainDB` est
    /// partagé par toutes les sessions, on ne peut donc pas y loger une
    /// valeur par requête. C'est un RAIL DE SÉCURITÉ, à régler large. La
    /// limite PAR REQUÊTE se pose avec un jeton
    /// ([`Session::execute_annulable`]), qui est plus précis et arrive
    /// toujours le premier.
    ///
    /// `None` rétablit le comportement d'avant : aucune échéance.
    #[must_use]
    pub fn with_query_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        self.config.query_timeout = timeout;
        self
    }

    /// (jalon 3 — RFC principal-natif-v2) Pose l'identite de lecture du
    /// substrat de CETTE base : passthrough vers
    /// [`obrain_substrate::SubstrateStore::avec_principal`].
    ///
    /// C'EST LE POINT D'ENTREE DU JALON 4 : `Database::session_as(principal)`
    /// derivera un `PrincipalSubstrat` du `Principal` de session et
    /// l'amenera ici. En attendant, un appelant (bridge, service) peut deja
    /// confiner un handle entier : une fois pose, TOUTES les lectures de ce
    /// handle — scans, seeks, index (egalite / trigrammes / ordonne),
    /// traversees (cascade sur l'extremite lointaine), compteurs — ne
    /// rendent que le tenant du principal et le public. Immuable une fois
    /// pose (pas de setter, pas d'effacement) ; un second appel echoue.
    ///
    /// PORTEE DU CONFINEMENT : le principal est PAR HANDLE de store — il
    /// s'applique a toutes les sessions de CE `ObrainDB` (et a tout clone
    /// de l'`Arc` sous-jacent). La multiplexion par session est le jalon 4.
    ///
    /// # Errors
    ///
    /// - base non substrat (memoire) : pas de substrat a confiner ;
    /// - `PrincipalIrrecevable` : `tenant_id == u32::MAX` (sentinelle) ;
    /// - `PrincipalDejaPose` : ce handle a deja une identite.
    pub fn avec_principal(&self, tenant_id: u32) -> Result<()> {
        let Some(ref store) = self.substrate_store else {
            return Err(obrain_common::utils::error::Error::Internal(
                "avec_principal : base non substrat — aucun substrat a confiner".into(),
            ));
        };
        let principal = obrain_substrate::PrincipalSubstrat::nouveau(tenant_id)
            .map_err(|e| obrain_common::utils::error::Error::Internal(e.to_string()))?;
        store
            .avec_principal(principal)
            .map_err(|e| obrain_common::utils::error::Error::Internal(e.to_string()))
    }

    /// Explicit WAL fsync passthrough — see [`obrain_substrate::SubstrateStore::fsync_wal`].
    /// No-op (returns `Ok(())`) when the database is not substrate-backed
    /// (e.g. an in-memory `ObrainDB`).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying WAL fsync fails.
    pub fn flush_wal(&self) -> Result<()> {
        if let Some(ref store) = self.substrate_store {
            store.fsync_wal().map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!("wal fsync failed: {e}"))
            })?;
        }
        Ok(())
    }

    /// ── ORACLE DE DURABILITÉ (test/diagnostic) ─────────────────────────────
    /// Offset de fin du WAL couvert par un `fsync` terminé, `None` pour une
    /// base en mémoire. Après un `commit()` acquitté en `EveryCommit`, la fin
    /// de la frame du commit doit être au plus égale à cette valeur : c'est ce
    /// que `tests/durabilite_commit_kill9.rs` vérifie après chaque ack, parce
    /// qu'un `kill -9` seul ne distingue pas « écrit » de « fsyncé ».
    /// Voir [`obrain_substrate::SubstrateStore::wal_octets_durables`].
    #[doc(hidden)]
    #[must_use]
    pub fn wal_octets_durables(&self) -> Option<u64> {
        self.substrate_store
            .as_ref()
            .map(|s| s.wal_octets_durables())
    }

    /// Nombre de `fsync` exécutés sur le WAL, `None` pour une base en mémoire.
    /// Voir [`obrain_substrate::SubstrateStore::wal_fsyncs`].
    #[doc(hidden)]
    #[must_use]
    pub fn wal_fsyncs(&self) -> Option<u64> {
        self.substrate_store.as_ref().map(|s| s.wal_fsyncs())
    }

    /// Force a full durability checkpoint: [`obrain_substrate::SubstrateStore::flush`]
    /// (WAL commit marker + msync of every mmap zone, including the v2
    /// props zone -- see that method's doc for the exact ordering).
    ///
    /// Unlike [`Self::flush_wal`] (WAL-only) and `commit_wal`
    /// (`#[cfg(feature = "wal")]`, engine-level mutation replay log),
    /// this method is **not** gated behind the `wal` feature: it exists
    /// purely to let a long-lived caller (e.g. a REPL session kept open
    /// for hours, or a connection-pool wrapper recycling sessions on a
    /// timer) bound how much unflushed mutation state a mid-session
    /// process kill can lose. Without a periodic call to this, the v2
    /// props zone's page-content writes (which are NOT WAL-protected --
    /// see the `obrain-substrate` gotcha on `append_scalar_to_props_zone_v2`)
    /// only ever get durably flushed on a graceful `close()`; a session
    /// killed (SIGKILL) before ever calling `close()` can lose or orphan
    /// property pages written since the last flush. Calling this on a
    /// cadence (e.g. every 30-60s of a session's lifetime) shrinks that
    /// exposure from "whole session lifetime" down to "one interval".
    ///
    /// No-op (returns `Ok(())`) when the database is not substrate-backed.
    ///
    /// # Errors
    ///
    /// Returns an error if any underlying zone fsync/msync fails.
    pub fn checkpoint(&self) -> Result<()> {
        if let Some(ref store) = self.substrate_store {
            store.flush().map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!("checkpoint failed: {e}"))
            })?;
        }
        Ok(())
    }

    /// Shared setup for a constructed [`obrain_substrate::SubstrateStore`]:
    /// wraps it as the erased `GraphStoreMut` backend and wires the
    /// cognitive engine (T17 Step 3 W1). Factored out of `open_substrate` so
    /// `open_substrate_with_mode` (T5 batch-WAL) shares the exact same
    /// wiring instead of risking drift between two copies.
    ///
    /// `path` porte le chemin REEL du store : `Some` pour une base sur
    /// disque, `None` pour un tempfile.
    ///
    /// (2026-08-13) Cette fonction codait `Config::in_memory()` en dur, quel
    /// que soit le store qu'on lui passait. Consequence : toute base ouverte
    /// par `ObrainDB::open(chemin)` se declarait NON PERSISTANTE — `info()`
    /// rendait `is_persistent: false` et `path()` rendait `None` pour une
    /// base bien presente sur le disque. Ce n'est pas qu'un test qui rate :
    /// tout code qui s'oriente sur `is_persistent()` ou `path()` prenait la
    /// mauvaise branche, en silence.
    fn with_substrate_store(
        store: obrain_substrate::SubstrateStore,
        path: Option<&std::path::Path>,
        read_only: bool,
    ) -> Result<Self> {
        // Keep a typed handle in addition to the erased `GraphStoreMut` one:
        // cognitive stores reach for the substrate column APIs (reinforce /
        // decay_all / iter_live_synapse_weights) which are not part of the
        // `GraphStoreMut` trait.
        let typed: Arc<obrain_substrate::SubstrateStore> = Arc::new(store);
        // ── ARMEMENT DU GROUP COMMIT PAR L'ENVIRONNEMENT (banc 2026-10-06) ───
        //
        // `OBRAIN_GROUP_COMMIT=1` arme le group commit à needle de durabilité
        // (`Writer::activer_group_commit_wal`) sur TOUTE base ouverte par le
        // moteur. POURQUOI UNE VARIABLE D'ENVIRONNEMENT, ET PAS UN DÉFAUT :
        // armer le group commit CHANGE la sémantique de durabilité (`commit()`
        // ne garantit plus « durable au retour »). Le défaut reste donc
        // INCHANGÉ — sans la variable, pas un octet du chemin d'écriture ne
        // change. C'est un levier de MESURE, pas une décision de production :
        // il n'existe que pour pouvoir comparer l'indexation réelle AVANT et
        // APRÈS sur le même binaire, sans recompiler deux fois.
        if std::env::var("OBRAIN_GROUP_COMMIT").is_ok_and(|v| v == "1") {
            typed.writer().activer_group_commit_wal();
        }
        // (V3, parade p8) Chaque base sur disque s'inscrit au registre
        // processus des substrats vivants : l'ouvreur paresseux des bases
        // attachees REUTILISE l'instance au lieu d'ouvrir une seconde fois
        // les memes fichiers (sidecars divergents).
        if let Some(p) = path {
            enregistrer_substrat_vivant(p, &typed);
        }
        let erased: Arc<dyn GraphStoreMut> = Arc::clone(&typed) as Arc<dyn GraphStoreMut>;
        let config = match path {
            Some(p) => Config::persistent(p),
            None => Config::in_memory(),
        };
        let mut db = Self::with_store(erased, config)?;
        db.substrate_store = Some(Arc::clone(&typed));

        // Journal RDF : replay des triplets persistés, puis ouverture en
        // ajout (écriture seule). Sans chemin (substrat temporaire), pas de
        // journal : l'état RDF n'a pas vocation à survivre.
        #[cfg(all(feature = "rdf", feature = "wal"))]
        if let Some(p) = path {
            db.ouvrir_journal_rdf(p, read_only)?;
        }
        #[cfg(not(all(feature = "rdf", feature = "wal")))]
        let _ = read_only;

        // (T5) Observateur d'index vectoriels : tout chemin d'ecriture de vecteur,
        // de suppression de noeud ou de propriete entretient les index, GQL compris.
        #[cfg(feature = "vector-index")]
        typed.add_mutation_observer(Arc::new(
            crate::database::vector_index_persist::ObservateurIndexVectoriel {
                registre: Arc::clone(&db.substrate_vector_indexes),
                compteur: Arc::clone(&db.ecritures_vectorielles_vues),
                store: Arc::downgrade(&typed),
            },
        ));

        // (2026-08-31) PERSISTANCE CDC — le journal survit au redémarrage.
        // `with_store` construit un `CdcLog` vide ; on y attache le persistor
        // substrate (zone `cdc_journal`) : recharge au boot + append à chaque
        // mutation. Prod-readiness §9.5 point 1.
        #[cfg(feature = "cdc")]
        {
            // Opt-out imports bulk : le CDC ouvre son journal/tail à chaque
            // mutation (~14 opens/nœud) — réservé au hot path applicatif.
            let cdc_off = std::env::var("OBRAIN_DISABLE_CDC").is_ok();
            if !cdc_off {
                db.cdc_log
                    .attach_persist(crate::cdc_persist::CdcPersist::open(Arc::clone(&typed)));
            }
        }
        #[cfg(feature = "cdc")]
        if std::env::var("OBRAIN_DISABLE_CDC").is_err() {
            // (2026-09-05) CDC×GQL : observe les créations substrate (le chemin
            // GQL/executor écrit directement dans le store, sans `crud`) →
            // search_delta/search_surprise voient le runtime réel.
            db.cdc_observer_active
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let observateur = Arc::new(crate::database::cdc_observer::CdcObserver::new(Arc::clone(
                &db.cdc_log,
            ))) as Arc<dyn obrain_substrate::StoreMutationObserver>;
            typed.add_mutation_observer(observateur);
        }

        // T17 Step 3 W1 — wire the cognitive engine to substrate column
        // routing. `with_store` leaves `cognitive_engine: None`; substrate-
        // backed databases deserve the same reactive bus + scheduler + built
        // engine as the legacy `with_config` path, but with the substrate
        // handle threaded through `CognitiveEngineBuilder::with_substrate` so
        // Energy/Synapse stores pick up the column-native constructors.
        #[cfg(feature = "cognitive")]
        {
            let bus = obrain_reactive::MutationBus::new();
            let scheduler =
                obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
            let config = obrain_cognitive::CognitiveConfig::default();
            #[allow(unused_mut)]
            let mut builder = obrain_cognitive::CognitiveEngineBuilder::from_config(&config)
                .with_graph_store(db.graph_store())
                .with_substrate(Arc::clone(&typed));
            // (DEC-P1) Câble le DecisionStore quand la feature est compilée.
            // Le registre de décision est passif (aucun listener, aucun flag de
            // config) : sans cet appel, `engine.decision_store()` reste `None`
            // dans la base réelle et `obrain.decide` rend toujours le prior
            // uniforme 0.5 / `obrain.settle` n'écrit rien — la couche DEC-P1
            // serait compilée mais morte au runtime.
            #[cfg(feature = "decision")]
            {
                builder = builder.with_decision();
            }
            // T17 Step 19 W2c — wire the kernel subsystem to the substrate-routed
            // store (NOT the dummy `db.store()` handle; see gotcha W4.p4.D1.s9).
            // `db.graph_store()` resolves to the real substrate backend in
            // substrate mode, giving KernelManager the populated graph instead
            // of the empty dummy LpgStore. Fixes the latent wiring gap where
            // the kernel subsystem was never attached in substrate mode.
            #[cfg(feature = "kernel")]
            {
                builder = builder.with_kernel_store(db.graph_store());
            }
            // (2026-08-31) ENGRAMS — active le Layer 0 (Hebbian + FSRS) quand
            // la feature est compilée : les procédures obrain.engrams.*
            // deviennent réelles (engram_store câblé via set_engram_subsystem).
            #[cfg(feature = "cognitive-engram")]
            {
                builder =
                    builder.with_engrams(obrain_cognitive::engram::EngramManagerConfig::default());
            }
            let engine = builder.build(&scheduler);
            // (2026-08-31) PERSISTANCE ENGRAMS — attache le writer substrate au
            // manager : les engrams formés par `record_activation` sont écrits
            // dans les zones `engram_members`/`engram_bitset` (survivent au
            // redémarrage — prod-readiness §9.5 point 2).
            #[cfg(feature = "cognitive-engram")]
            {
                use obrain_cognitive::CognitiveEngine as _;
                if let Some(mgr) = engine.engram_manager() {
                    mgr.attach_substrate(typed.writer_arc());
                }
            }
            let engine_dyn: Arc<dyn obrain_cognitive::CognitiveEngine> = Arc::new(engine);
            // (2026-11-01, v0.0.1) Les UDF `obrain.energy`, `obrain.risk` et
            // `obrain.synapses` étaient définies (cognitive_udfs.rs) mais
            // enregistrées SEULEMENT par un test : le README les promettait,
            // aucune base ne les offrait. Enregistrées ici, à chaque
            // construction d'un moteur cognitif, et le pont UDF → évaluateur
            // installé (idempotent).
            obrain_adapters::plugins::installer_udfs_dans_le_moteur();
            crate::cognitive_udfs::register_cognitive_udfs(
                obrain_adapters::plugins::registre_global(),
                Arc::clone(&engine_dyn),
            );
            db.cognitive_engine = Some(engine_dyn);
            db._cognitive_scheduler = Some(scheduler);
        }

        // (2026-09-07) CATALOGUE DE PROCEDURES — hydratation au boot.
        // `substrate_store` est pose ci-dessus ; le catalogue RAM du moteur
        // (`db.catalog`, partage avec les sessions via `SessionConfig`) est
        // repeuple depuis `substrate.proccat`. Sans cela, un process neuf ne
        // connaît aucune procedure creee par un process precedent (le WAL
        // moteur CreateProcedure n'est jamais rejoue — meme constat que les
        // index le 2026-08-14).
        db.hydrater_procedures();
        // (2026-09-17) CATALOGUE DE TYPES — hydratation au boot, dans le meme
        // mouvement. `CREATE NODE TYPE` ecrivait un WalRecord que PERSONNE ne
        // rejoue : le type vecu disparaissait a la fermeture. Le fichier
        // `substrate.typecat` porte l'intention, le catalogue RAM s'y plie.
        db.hydrater_types();
        // (2026-09-17) INDEX TEXTE : recharge les index déclarés depuis
        // `substrate.textidx.*` quand leur empreinte décrit encore ce store.
        // Sans cela, chaque ouverture repayait un balayage complet du label.
        #[cfg(feature = "text-index")]
        db.hydrater_index_textuels();

        Ok(db)
    }

    /// Opens an existing database in read-only mode.
    ///
    /// Uses a shared file lock, so multiple processes can read the same
    /// `.obrain` file concurrently. The database loads the last checkpoint
    /// snapshot but does **not** replay the WAL or allow mutations.
    ///
    /// Currently only supports the single-file (`.obrain`) format.
    ///
    /// # Errors
    ///
    /// Returns an error if the file doesn't exist or can't be read.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use obrain_engine::ObrainDB;
    ///
    /// let db = ObrainDB::open_read_only("./my_graph.obrain")?;
    /// let session = db.session();
    /// let result = session.execute("MATCH (n) RETURN n LIMIT 10")?;
    /// // Mutations will return an error:
    /// // session.execute("INSERT (:Person)") => Err(ReadOnly)
    /// # Ok::<(), obrain_common::utils::error::Error>(())
    /// ```
    ///
    /// (2026-08-13) CE BLOC DE DOCUMENTATION ETAIT ORPHELIN : il decrivait
    /// `open_read_only`, une fonction qui n'existait plus, et se retrouvait
    /// colle au doc-comment de `with_config`. Le doctest echouait donc a la
    /// compilation, en annoncant publiquement une API absente.
    ///
    /// La capacite, elle, EXISTE : `Config::read_only(path)` pose
    /// `AccessMode::ReadOnly`, que `with_config` honore deja (verrou partage,
    /// WAL desactive, mutations refusees au niveau session). Il ne manquait que
    /// le raccourci. On le remet plutot que de supprimer la promesse — la
    /// documentation avait raison, c'est le code qui manquait.
    pub fn open_read_only(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::with_config(Config::read_only(path.as_ref()))
    }

    /// Creates a database with custom configuration.
    ///
    /// Use this when you need fine-grained control over memory limits,
    /// thread counts, or persistence settings. For most cases,
    /// [`new_in_memory()`](Self::new_in_memory) or [`open()`](Self::open)
    /// are simpler.
    ///
    /// # Errors
    ///
    /// Returns an error if the database can't be created or recovery fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use obrain_engine::{ObrainDB, Config};
    ///
    /// // In-memory with a 512MB limit
    /// let config = Config::in_memory()
    ///     .with_memory_limit(512 * 1024 * 1024);
    ///
    /// let db = ObrainDB::with_config(config)?;
    /// # Ok::<(), obrain_common::utils::error::Error>(())
    /// ```
    pub fn with_config(config: Config) -> Result<Self> {
        // Les DEUX constructeurs branchent le vocabulaire : `new_in_memory()`
        // passe par ici et PAS par `with_store` — l'oublier laissait les bases en
        // mémoire sans UDF (constaté par le test `fonction_hote`, qui échouait
        // exactement là).
        brancher_le_vocabulaire_de_l_hote();
        // Validate configuration before proceeding
        config
            .validate()
            .map_err(|e| obrain_common::utils::error::Error::Internal(e.to_string()))?;

        // T17 final cutover (2026-04-23): `with_config` no longer builds
        // an in-memory LpgStore. Substrate is the single backend — an
        // in-memory config opens a substrate tempfile (auto-cleaned on
        // drop), a persistent config delegates to
        // `ObrainDB::open_substrate(path)`. Tests that rely on LpgStore-
        // inherent MVCC / named-graph semantics (~17 tests in session/
        // mod.rs + transaction/prepared.rs) are marked `#[ignore]` — the
        // substrate backend has its own transaction + persistence model
        // and the legacy LpgStore MVCC suite is no longer covered by
        // this entry point.
        if let Some(ref db_path) = config.path {
            // Persistent: delegate straight to the substrate open path.
            // (2026-08-31) ReadOnly → ouverture en VERROU PARTAGÉ : plusieurs
            // processus peuvent lire le même store simultanément (le contrat
            // de `Config::read_only`, qui promettait « shared file lock » mais
            // ouvrait en exclusif). `open_substrate_read_only` pose le
            // `LOCK_SH` côté substrate.
            let is_read_only = config.access_mode == crate::config::AccessMode::ReadOnly;
            let mut db = if is_read_only {
                Self::open_substrate_read_only(db_path)?
            } else {
                Self::open_substrate(db_path)?
            };
            // (2026-08-31) FIX READ-ONLY PERDU SUR LE CHEMIN SUBSTRATE.
            //
            // `open_substrate` → `with_substrate_store` construit la Config
            // elle-même (`Config::persistent`) et pose `read_only: false` —
            // le mode ReadOnly de `Config::read_only` était silencieusement
            // droppé. Conséquence mesurée au bench : le close d'une session
            // read-only déclenchait quand même le `flush()` substrate complet
            // (~11-22 ms), le seul point où obrain perdait vs cbm sur `open`.
            // En read_only, le close doit juste relâcher le verrou (c'est
            // déjà le comportement du champ `read_only` dans `close()`).
            if is_read_only {
                db.read_only = true;
            }
            return Ok(db);
        }

        // (2026-09-17) wasm32 : il n'y a PAS de système de fichiers, donc
        // `open_tempfile()` y panique (`std::env::temp_dir()` →
        // « no filesystem on this platform », trap `unreachable`). Le mode
        // « en mémoire » de la config doit donc être un vrai substrat EN
        // MÉMOIRE là où le temporaire n'existe pas. Sur natif, rien ne change :
        // le répertoire temporaire reste le comportement de référence.
        #[cfg(target_arch = "wasm32")]
        let ouverture = obrain_substrate::SubstrateStore::open_memory();
        #[cfg(not(target_arch = "wasm32"))]
        let ouverture = obrain_substrate::SubstrateStore::open_tempfile();
        let store = ouverture.map_err(|e| {
            obrain_common::utils::error::Error::Internal(format!(
                "substrate tempfile creation failed: {e}"
            ))
        })?;
        let typed: Arc<obrain_substrate::SubstrateStore> = Arc::new(store);
        let erased: Arc<dyn GraphStoreMut> = Arc::clone(&typed) as Arc<dyn GraphStoreMut>;
        let store = erased;
        #[cfg(feature = "rdf")]
        let rdf_store = Arc::new(RdfStore::new());
        let transaction_manager = Arc::new(TransactionManager::new());

        // Create buffer manager with configured limits
        let buffer_config = BufferManagerConfig {
            budget: config.memory_limit.unwrap_or_else(|| {
                (BufferManagerConfig::detect_system_memory() as f64 * 0.75) as usize
            }),
            spill_path: config
                .spill_path
                .clone()
                .or_else(|| config.path.as_ref().map(|p| p.join("spill"))),
            ..BufferManagerConfig::default()
        };
        let buffer_manager = BufferManager::new(buffer_config);

        // Create catalog early so WAL replay can restore schema definitions
        let catalog = Arc::new(Catalog::new());

        let is_read_only = config.access_mode == crate::config::AccessMode::ReadOnly;

        // T17 final cutover (2026-04-23): the single-file `.obrain`
        // format handling that lived here (v1 bincode snapshot + v2
        // mmap native, ~140 LOC) was feature-gated on `obrain-file`
        // which was retired in commit `b0f5bd11`. The block was
        // `let file_manager: Option<Arc<ObrainFileManager>>` plus
        // `let mut native_mmap_store: Option<Arc<mmap_store::MmapStore>>`,
        // both unreachable once the feature was removed. Substrate is
        // now the single backend; legacy `.obrain` files are
        // converted via `obrain-migrate` before opening.

        // Determine whether to use the WAL directory path (legacy) or sidecar
        // Read-only mode skips WAL entirely (no recovery, no creation).
        #[cfg(feature = "wal")]
        let wal = if is_read_only {
            None
        } else if config.wal_enabled {
            if let Some(ref db_path) = config.path {
                // T17 final cutover: WAL is always a directory inside
                // the database path. The pre-cutover sidecar layout
                // (when the store was a single `.obrain` file) no
                // longer applies — substrate is directory-based from
                // the start.
                std::fs::create_dir_all(db_path)?;
                let wal_path = db_path.join("wal");

                // T17 final cutover (2026-04-23): legacy tiered-storage
                // epoch-file recovery + LpgWal replay are gone.
                // Substrate's `open_substrate()` path handles its own WAL
                // recovery natively. `with_config` always builds a fresh
                // substrate tempfile (see above), so there is no prior
                // state to replay here.
                let _ = &wal_path;
                let _ = &catalog;

                // Open/create WAL manager with configured durability
                let wal_durability = match config.wal_durability {
                    crate::config::DurabilityMode::Sync => WalDurabilityMode::Sync,
                    crate::config::DurabilityMode::Batch {
                        max_delay_ms,
                        max_records,
                    } => WalDurabilityMode::Batch {
                        max_delay_ms,
                        max_records,
                    },
                    crate::config::DurabilityMode::Adaptive { target_interval_ms } => {
                        WalDurabilityMode::Adaptive { target_interval_ms }
                    }
                    crate::config::DurabilityMode::NoSync => WalDurabilityMode::NoSync,
                };
                let wal_config = WalConfig {
                    durability: wal_durability,
                    ..WalConfig::default()
                };
                let wal_manager = LpgWal::with_config(&wal_path, wal_config)?;
                Some(Arc::new(wal_manager))
            } else {
                None
            }
        } else {
            None
        };

        // Create query cache with default capacity (1000 queries)
        let query_cache = Arc::new(QueryCache::default());

        // After all snapshot/WAL recovery, sync TransactionManager epoch
        // with the store so queries use the correct viewing epoch.
        #[cfg(feature = "temporal")]
        transaction_manager.sync_epoch(store.current_epoch());

        // --- Cognitive engine initialization ---
        // Creates the reactive bus + scheduler + cognitive engine with default config.
        // Zero-cost: this entire block is compiled out when `cognitive` feature is off.
        #[cfg(feature = "cognitive")]
        let (cognitive_engine, _cognitive_scheduler) = {
            let bus = obrain_reactive::MutationBus::new();
            let scheduler =
                obrain_reactive::Scheduler::new(&bus, obrain_reactive::BatchConfig::default());
            let config = obrain_cognitive::CognitiveConfig::default();
            #[allow(unused_mut)]
            let mut builder = obrain_cognitive::CognitiveEngineBuilder::from_config(&config);
            #[cfg(feature = "kernel")]
            {
                // T17 Step 19 W2c + Step 24 `with_config` retype: the
                // local `store` is now `Arc<dyn GraphStoreMut>` (substrate
                // tempfile). Pass it straight through to the kernel
                // subsystem — no concrete LpgStore cast needed.
                builder = builder.with_kernel_store(Arc::clone(&store));
            }
            // (DEC-C6) La copie inline du câblage NE passait PAS par
            // `with_graph_store` / `with_substrate` : `substrate_handle()`
            // rendait None et les stores cognitifs (dont le DecisionStore
            // write-through) retombaient sur les propriétés LPG aux défauts
            // de trait vides — appels réussis sans rien faire. Ces deux
            // lignes routent vers le substrat, comme `with_substrate_store`.
            builder = builder
                .with_graph_store(Arc::clone(&store))
                .with_substrate(Arc::clone(&typed));
            // (DEC-P1) Même câblage que `with_substrate_store` : la copie inline
            // de `with_config` oubliait le DecisionStore, laissant
            // `decision_store()` à None sur ce chemin. `obrain.decide`/`settle`
            // seraient morts pour toute base ouverte par `with_config`.
            #[cfg(feature = "decision")]
            {
                builder = builder.with_decision();
            }
            let engine = builder.build(&scheduler);
            let engine_dyn: Arc<dyn obrain_cognitive::CognitiveEngine> = Arc::new(engine);
            // (2026-11-01, v0.0.1) Même oubli que les deux pavés ci-dessus : la
            // copie inline de `with_config` (chemin de `new_in_memory`) ne
            // passait pas par `with_substrate_store`, donc les UDF cognitives
            // n'y étaient jamais enregistrées — mesuré « fonction inconnue :
            // obrain.energy » avec le moteur cognitif pourtant construit.
            obrain_adapters::plugins::installer_udfs_dans_le_moteur();
            crate::cognitive_udfs::register_cognitive_udfs(
                obrain_adapters::plugins::registre_global(),
                Arc::clone(&engine_dyn),
            );
            (Some(engine_dyn), Some(scheduler))
        };

        let db = Self {
            config,
            store,
            catalog,
            #[cfg(feature = "rdf")]
            rdf_store,
            transaction_manager,
            buffer_manager,
            #[cfg(feature = "wal")]
            wal,
            #[cfg(feature = "wal")]
            wal_graph_context: Arc::new(parking_lot::Mutex::new(None)),
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            query_cache,
            commit_counter: Arc::new(AtomicUsize::new(0)),
            is_open: RwLock::new(true),
            #[cfg(feature = "cdc")]
            cdc_log: Arc::new(crate::cdc::CdcLog::new()),
            cdc_observer_active: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "embed")]
            embedding_models: RwLock::new(hashbrown::HashMap::new()),
            external_store: None,
            // (2026-08-13) `Some(typed)`, et non `None`. Cette base EST adossee
            // a un substrat — `open_tempfile()` quelques lignes plus haut — mais
            // `with_config` a sa propre copie inline du cablage et ne passait
            // jamais par `with_substrate_store`, seul endroit du fichier qui
            // renseigne ce champ (ligne ~357).
            //
            // TROIS CONSEQUENCES, toutes silencieuses :
            //   1. `create_vector_index` / `create_text_index` / `vector_search`
            //      / `text_search` testent `substrate_store.is_some()` pour
            //      REFUSER explicitement en mode substrat (« LpgStore-only —
            //      substrate mode exposes retrieval via SubstrateTieredIndex ; a
            //      unified retrieval API is tracked as T17b »). Avec `None`, les
            //      appels passaient A COTE de cette garde honnete, tombaient dans
            //      le chemin LpgStore, y trouvaient des defauts de trait a CORPS
            //      VIDE, et rendaient Ok(()) sans rien faire. Puis la recherche
            //      repondait « No index found. Call create_*_index() first » — en
            //      conseillant exactement ce qui venait d'etre fait. 42 tests.
            //   2. `substrate_handle()` rendait None, donc les stores cognitifs
            //      (energy / scar / utility / affinity / synapse) retombaient sur
            //      le chemin de proprietes LPG au lieu des APIs colonne du
            //      substrat — sa propre doc le dit.
            //   3. `is_persistent()` mentait, corrige separement.
            //
            // Le champ dit desormais la verite. Les gardes T17b se declenchent, et
            // les tests concernes echouent avec le VRAI motif au lieu d'un message
            // qui envoie chercher au mauvais endroit.
            substrate_store: Some(typed),
            #[cfg(target_arch = "wasm32")]
            miroir: None,
            base_registry: parking_lot::RwLock::new(None),
            substrate_vector_indexes: Arc::new(parking_lot::RwLock::new(
                std::collections::HashMap::new(),
            )),
            index_texte_charges: std::sync::atomic::AtomicUsize::new(0),
            index_texte_persistes: std::sync::atomic::AtomicUsize::new(0),
            index_vectoriel_charges: std::sync::atomic::AtomicUsize::new(0),
            index_vectoriel_reconstruits: std::sync::atomic::AtomicUsize::new(0),
            index_vectoriel_persistes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(feature = "vector-index")]
            ecritures_vectorielles_vues: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(feature = "vector-index")]
            index_vectoriel_empreintes: parking_lot::RwLock::new(std::collections::HashMap::new()),
            ecriture_partagee: false,
            #[cfg(feature = "text-index")]
            index_texte_empreintes: parking_lot::RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "text-index")]
            index_texte_epochs: parking_lot::RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "text-index")]
            substrate_text_indexes: Arc::new(parking_lot::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "metrics")]
            metrics: Some(Arc::new(crate::metrics::MetricsRegistry::new())),
            current_graph: RwLock::new(None),
            read_only: is_read_only,
            #[cfg(feature = "cognitive")]
            cognitive_engine,
            #[cfg(feature = "cognitive")]
            _cognitive_scheduler,
        };
        // (T5) Meme observateur d'index vectoriels que le chemin fichier : une base en
        // memoire doit entretenir ses index pour TOUT chemin d'ecriture (GQL compris).
        #[cfg(feature = "vector-index")]
        if let Some(ref typed) = db.substrate_store {
            typed.add_mutation_observer(Arc::new(
                crate::database::vector_index_persist::ObservateurIndexVectoriel {
                    registre: Arc::clone(&db.substrate_vector_indexes),
                    compteur: Arc::clone(&db.ecritures_vectorielles_vues),
                    store: Arc::downgrade(typed),
                },
            ));
        }
        Ok(db)
    }

    /// Creates a database backed by a custom [`GraphStoreMut`] implementation.
    ///
    /// The external store handles all data persistence. WAL, CDC, and index
    /// management are the responsibility of the store implementation.
    ///
    /// Query execution (all 6 languages, optimizer, planner) works through the
    /// provided store. Admin operations (schema introspection, persistence,
    /// vector/text indexes) are not available on external stores.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use obrain_engine::{ObrainDB, Config};
    /// use obrain_core::graph::GraphStoreMut;
    ///
    /// fn example(store: Arc<dyn GraphStoreMut>) -> obrain_common::utils::error::Result<()> {
    ///     let db = ObrainDB::with_store(store, Config::in_memory())?;
    ///     let result = db.execute("MATCH (n) RETURN count(n)")?;
    ///     Ok(())
    /// }
    /// ```
    ///
    /// [`GraphStoreMut`]: obrain_core::graph::GraphStoreMut
    pub fn with_store(store: Arc<dyn GraphStoreMut>, config: Config) -> Result<Self> {
        config
            .validate()
            .map_err(|e| obrain_common::utils::error::Error::Internal(e.to_string()))?;
        brancher_le_vocabulaire_de_l_hote();

        // T17 Step 24 (2026-04-23): the dummy `LpgStore::new()` that used
        // to shadow the real store in this constructor is gone — the
        // `store` field is now `Arc<dyn GraphStoreMut>`, so it can hold
        // the real store directly (legacy or substrate). The legacy
        // Session path still needs a separate `external_store` handle
        // for MVCC dispatch, which is kept below.
        let transaction_manager = Arc::new(TransactionManager::new());

        let buffer_config = BufferManagerConfig {
            budget: config.memory_limit.unwrap_or_else(|| {
                (BufferManagerConfig::detect_system_memory() as f64 * 0.75) as usize
            }),
            spill_path: None,
            ..BufferManagerConfig::default()
        };
        let buffer_manager = BufferManager::new(buffer_config);

        let query_cache = Arc::new(QueryCache::default());

        Ok(Self {
            config,
            store: Arc::clone(&store),
            catalog: Arc::new(Catalog::new()),
            #[cfg(feature = "rdf")]
            rdf_store: Arc::new(RdfStore::new()),
            transaction_manager,
            buffer_manager,
            #[cfg(feature = "wal")]
            wal: None,
            #[cfg(feature = "wal")]
            wal_graph_context: Arc::new(parking_lot::Mutex::new(None)),
            #[cfg(all(feature = "rdf", feature = "wal"))]
            rdf_wal: None,
            query_cache,
            commit_counter: Arc::new(AtomicUsize::new(0)),
            is_open: RwLock::new(true),
            #[cfg(feature = "cdc")]
            cdc_log: Arc::new(crate::cdc::CdcLog::new()),
            cdc_observer_active: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "embed")]
            embedding_models: RwLock::new(hashbrown::HashMap::new()),
            external_store: Some(store),
            substrate_store: None,
            #[cfg(target_arch = "wasm32")]
            miroir: None,
            base_registry: parking_lot::RwLock::new(None),
            substrate_vector_indexes: Arc::new(parking_lot::RwLock::new(
                std::collections::HashMap::new(),
            )),
            index_texte_charges: std::sync::atomic::AtomicUsize::new(0),
            index_texte_persistes: std::sync::atomic::AtomicUsize::new(0),
            index_vectoriel_charges: std::sync::atomic::AtomicUsize::new(0),
            index_vectoriel_reconstruits: std::sync::atomic::AtomicUsize::new(0),
            index_vectoriel_persistes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(feature = "vector-index")]
            ecritures_vectorielles_vues: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(feature = "vector-index")]
            index_vectoriel_empreintes: parking_lot::RwLock::new(std::collections::HashMap::new()),
            ecriture_partagee: false,
            #[cfg(feature = "text-index")]
            index_texte_empreintes: parking_lot::RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "text-index")]
            index_texte_epochs: parking_lot::RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "text-index")]
            substrate_text_indexes: Arc::new(parking_lot::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "metrics")]
            metrics: Some(Arc::new(crate::metrics::MetricsRegistry::new())),
            current_graph: RwLock::new(None),
            read_only: false,
            #[cfg(feature = "cognitive")]
            cognitive_engine: None,
            #[cfg(feature = "cognitive")]
            _cognitive_scheduler: None,
        })
    }

    /// (2026-08-17, aretes inter-bases V2) Declare la session
    /// multi-store de cette base : son SLUG (l'adresse par laquelle les
    /// autres bases la referencent) et, optionnellement, la RACINE des
    /// bases (`racine/slug` devient un chemin candidat pour l'ouverture
    /// paresseuse des bases attachees).
    ///
    /// Sans cet appel, les aretes externes s'ecrivent et survivent, mais
    /// leur traversee rend absent (le registre n'existe pas). Les
    /// sessions ouvertes APRES l'appel voient le registre.
    pub fn attach_bases(&self, primary_slug: &str, root: Option<&std::path::Path>) {
        let opener: Box<obrain_core::execution::base_registry::StoreOpener> = Box::new(
            |path: &std::path::Path| {
                // (V3, parade p8) REUTILISER l'instance vivante si une base
                // de ce processus tient deja ce chemin — jamais deux
                // instances des memes fichiers (sidecars divergents).
                if let Some(vivant) = substrat_vivant(path) {
                    return Some(vivant as Arc<dyn obrain_core::graph::GraphStore>);
                }
                match obrain_substrate::SubstrateStore::open(path) {
                    Ok(store) => {
                        let arc = Arc::new(store);
                        // Deux attacheurs du meme chemin partagent aussi.
                        enregistrer_substrat_vivant(path, &arc);
                        Some(arc as Arc<dyn obrain_core::graph::GraphStore>)
                    }
                    Err(e) => {
                        tracing::warn!(chemin = %path.display(), erreur = %e,
                            "base attachee inaccessible — les references vers elle rendront absent");
                        None
                    }
                }
            },
        );
        let primary: Arc<dyn obrain_core::graph::GraphStore> = self.graph_store();
        let mut registry = obrain_core::execution::BaseRegistry::new(primary_slug, primary, opener);
        if let Some(root) = root {
            registry = registry.with_root(root);
        }
        *self.base_registry.write() = Some(Arc::new(registry));
    }

    /// (2026-08-17, aretes inter-bases V2) Attache une base DEJA OUVERTE
    /// du meme processus sous `slug` (le pool du service, ou un autre
    /// [`ObrainDB`]). Necessite un `attach_bases` prealable.
    ///
    /// # Errors
    ///
    /// Erreur si `attach_bases` n'a pas encore declare le slug primaire.
    pub fn attach_base(&self, slug: &str, other: &ObrainDB) -> Result<()> {
        let guard = self.base_registry.read();
        let Some(registry) = guard.as_ref() else {
            return Err(obrain_common::utils::error::Error::InvalidValue(
                "attach_base: declare d'abord la session multi-store avec \
                 attach_bases(slug_primaire, racine)"
                    .to_string(),
            ));
        };
        let store: Arc<dyn obrain_core::graph::GraphStore> = other.graph_store();
        registry.register_store(slug, store);
        Ok(())
    }

    /// (2026-08-17, aretes inter-bases V2) Declare le chemin d'une base
    /// attachable sous `slug` (ouverture paresseuse au premier besoin).
    ///
    /// # Errors
    ///
    /// Erreur si `attach_bases` n'a pas encore declare le slug primaire.
    pub fn attach_base_path(&self, slug: &str, path: &std::path::Path) -> Result<()> {
        let guard = self.base_registry.read();
        let Some(registry) = guard.as_ref() else {
            return Err(obrain_common::utils::error::Error::InvalidValue(
                "attach_base_path: declare d'abord la session multi-store avec \
                 attach_bases(slug_primaire, racine)"
                    .to_string(),
            ));
        };
        registry.register_path(slug, path);
        Ok(())
    }

    /// (Service, eviction du pool) Detache une base du registre de cette
    /// base : ses stores ne sont plus retenus, les references vers elle
    /// redeviennent absentes. No-op sans registre.
    pub fn detach_base(&self, slug: &str) {
        if let Some(registry) = self.base_registry.read().as_ref() {
            registry.detach_store(slug);
        }
    }

    /// Registre multi-store de la base, s'il a ete declare.
    #[must_use]
    pub fn base_registry(&self) -> Option<Arc<obrain_core::execution::BaseRegistry>> {
        self.base_registry.read().clone()
    }

    /// Opens a new query [`Session`] bound to this database.
    #[must_use]
    pub fn session(&self) -> Session {
        let session_cfg = || crate::session::SessionConfig {
            transaction_manager: Arc::clone(&self.transaction_manager),
            query_cache: Arc::clone(&self.query_cache),
            catalog: Arc::clone(&self.catalog),
            adaptive_config: self.config.adaptive.clone(),
            factorized_execution: self.config.factorized_execution,
            graph_model: self.config.graph_model,
            query_timeout: self.config.query_timeout,
            commit_counter: Arc::clone(&self.commit_counter),
            gc_interval: self.config.gc_interval,
            read_only: self.read_only,
        };

        if let Some(ref ext_store) = self.external_store {
            #[allow(unused_mut)]
            let mut ext_session =
                Session::with_external_store(Arc::clone(ext_store), session_cfg())
                    .expect("arena allocation for external store session");
            // T17 cutover made substrate (routed through `external_store`)
            // the only production backend, so this early return is the path
            // EVERY real session takes — the wal/cdc/cognitive wiring below
            // was dead code for it. Wire the cognitive engine (3 piliers)
            // here too, or `CALL obrain.cognitive.*` silently falls through
            // to "Unknown procedure" for every substrate-backed database.
            #[cfg(feature = "cognitive")]
            if let Some(ref engine) = self.cognitive_engine {
                ext_session.set_cognitive_engine(Arc::clone(engine));
            }
            // Typed substrate handle — same wiring pattern, so substrate-
            // native procedures (`CALL obrain.window_2d`, RFC
            // zorder-superposition C5) resolve at query-plan time.
            if let Some(ref sub) = self.substrate_store {
                ext_session.set_substrate_store(Arc::clone(sub));
            }
            // CDC log — même câblage, pour `CALL obrain.search_delta`
            // (diff de changements) sur les bases substrate (ce chemin
            // external_store est celui que prend TOUTE base réelle).
            #[cfg(feature = "cdc")]
            ext_session.set_cdc_log(Arc::clone(&self.cdc_log));
            // Store RDF partagé avec la base (chemin substrat, celui de toute
            // base réelle) : sans ce câblage, la session écrit et lit dans un
            // store privé, et les triplets rejoués du journal lui sont invisibles.
            #[cfg(feature = "rdf")]
            ext_session.set_rdf_store(Arc::clone(&self.rdf_store));
            // Journal RDF, même chemin.
            #[cfg(all(feature = "rdf", feature = "wal"))]
            ext_session.set_rdf_wal(self.rdf_wal.clone());
            // Aretes inter-bases (V2) : la session herite du registre
            // multi-store declare sur la base.
            if let Some(registry) = self.base_registry.read().clone() {
                ext_session.set_base_registry(registry);
            }
            // Recherche hybride (`CALL obrain.hybrid_search`) : les index
            // texte/vecteur vivent ICI, dans ObrainDB — la poignée en porte
            // des clones Arc jusqu'au planificateur, même câblage que
            // substrate_store.
            #[cfg(feature = "hybrid-search")]
            ext_session.set_recherche_hybride(self.poignee_recherche_hybride());
            return ext_session;
        }

        #[cfg(feature = "rdf")]
        let mut session = Session::with_rdf_store_and_adaptive(
            Arc::clone(&self.store) as Arc<dyn GraphStoreMut>,
            Arc::clone(&self.rdf_store),
            session_cfg(),
        );
        #[cfg(not(feature = "rdf"))]
        let mut session = Session::with_adaptive(
            Arc::clone(&self.store) as Arc<dyn GraphStoreMut>,
            session_cfg(),
        );

        #[cfg(feature = "wal")]
        if let Some(ref wal) = self.wal {
            session.set_wal(Arc::clone(wal), Arc::clone(&self.wal_graph_context));
        }
        #[cfg(all(feature = "rdf", feature = "wal"))]
        session.set_rdf_wal(self.rdf_wal.clone());

        #[cfg(feature = "cdc")]
        session.set_cdc_log(Arc::clone(&self.cdc_log));

        // Recherche hybride : même câblage que sur le chemin substrat.
        #[cfg(feature = "hybrid-search")]
        session.set_recherche_hybride(self.poignee_recherche_hybride());

        // Wire the cognitive engine (3 piliers), if this DB has one, so
        // `CALL obrain.cognitive.*` resolves in every session's planner
        // instead of silently falling through to "Unknown procedure".
        #[cfg(feature = "cognitive")]
        {
            if let Some(ref engine) = self.cognitive_engine {
                session.set_cognitive_engine(Arc::clone(engine));
            }
        }
        // Typed substrate handle — same wiring pattern, so substrate-native
        // procedures (`CALL obrain.window_2d`, RFC zorder-superposition C5)
        // resolve at query-plan time in this branch too.
        if let Some(ref sub) = self.substrate_store {
            session.set_substrate_store(Arc::clone(sub));
        }

        #[cfg(feature = "metrics")]
        {
            if let Some(ref m) = self.metrics {
                session.set_metrics(Arc::clone(m));
                m.session_created
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                m.session_active
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // Propagate persistent graph context to the new session
        if let Some(ref graph) = *self.current_graph.read() {
            session.use_graph(graph);
        }

        // Aretes inter-bases (V2) : meme heritage du registre multi-store
        // que le chemin external_store plus haut — les DEUX chemins de
        // construction de session doivent le poser.
        if let Some(registry) = self.base_registry.read().clone() {
            session.set_base_registry(registry);
        }

        // Suppress unused_mut when cdc/wal are disabled
        let _ = &mut session;

        session
    }

    /// Returns the current graph name, if any.
    ///
    /// This is the persistent graph context used by one-shot `execute()` calls.
    /// It is updated whenever `execute()` encounters `USE GRAPH`, `SESSION SET GRAPH`,
    /// or `SESSION RESET`.
    #[must_use]
    pub fn current_graph(&self) -> Option<String> {
        self.current_graph.read().clone()
    }

    /// Sets the current graph context for subsequent one-shot `execute()` calls.
    ///
    /// This is equivalent to running `USE GRAPH <name>` but without creating a session.
    /// Pass `None` to reset to the default graph.
    pub fn set_current_graph(&self, name: Option<&str>) {
        *self.current_graph.write() = name.map(ToString::to_string);
    }

    /// Returns the adaptive execution configuration.
    #[must_use]
    pub fn adaptive_config(&self) -> &crate::config::AdaptiveConfig {
        &self.config.adaptive
    }

    /// Returns `true` if this database was opened in read-only mode.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Returns the configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the cognitive engine, if the `cognitive` feature is enabled and
    /// the engine was initialized.
    ///
    /// This provides unified access to energy, synapse, fabric, and other
    /// cognitive subsystems.
    #[cfg(feature = "cognitive")]
    #[must_use]
    pub fn cognitive_engine(&self) -> Option<&Arc<dyn obrain_cognitive::CognitiveEngine>> {
        self.cognitive_engine.as_ref()
    }

    /// (2026-08-31) Accès au journal CDC (audit trail) — exposé pour la
    /// prod/monitoring : `nodeHistory`/`changesBetween` via [`CdcLog`](crate::cdc::CdcLog).
    #[cfg(feature = "cdc")]
    #[must_use]
    pub fn cdc_log(&self) -> &Arc<crate::cdc::CdcLog> {
        &self.cdc_log
    }

    /// Returns the graph data model of this database.
    #[must_use]
    pub fn graph_model(&self) -> crate::config::GraphModel {
        self.config.graph_model
    }

    /// Returns the configured memory limit in bytes, if any.
    #[must_use]
    pub fn memory_limit(&self) -> Option<usize> {
        self.config.memory_limit
    }

    /// Returns a point-in-time snapshot of all metrics.
    ///
    /// If the `metrics` feature is disabled or the registry is not
    /// initialized, returns a default (all-zero) snapshot.
    #[cfg(feature = "metrics")]
    #[must_use]
    pub fn metrics(&self) -> crate::metrics::MetricsSnapshot {
        let mut snapshot = self
            .metrics
            .as_ref()
            .map_or_else(crate::metrics::MetricsSnapshot::default, |m| m.snapshot());

        // Augment with cache stats from the query cache (not tracked in the registry)
        let cache_stats = self.query_cache.stats();
        snapshot.cache_hits = cache_stats.parsed_hits + cache_stats.optimized_hits;
        snapshot.cache_misses = cache_stats.parsed_misses + cache_stats.optimized_misses;
        snapshot.cache_size = cache_stats.parsed_size + cache_stats.optimized_size;
        snapshot.cache_invalidations = cache_stats.invalidations;

        snapshot
    }

    /// Returns all metrics in Prometheus text exposition format.
    ///
    /// The output is ready to serve from an HTTP `/metrics` endpoint.
    #[cfg(feature = "metrics")]
    #[must_use]
    pub fn metrics_prometheus(&self) -> String {
        self.metrics
            .as_ref()
            .map_or_else(String::new, |m| m.to_prometheus())
    }

    /// Resets all metrics counters and histograms to zero.
    #[cfg(feature = "metrics")]
    pub fn reset_metrics(&self) {
        if let Some(ref m) = self.metrics {
            m.reset();
        }
        self.query_cache.reset_stats();
    }

    /// Returns the underlying (default) store.
    ///
    /// Returns the underlying graph store as `&Arc<dyn GraphStoreMut>`.
    ///
    /// T17 Step 24 (2026-04-23): retyped from the concrete
    /// `&Arc<LpgStore>` to the trait object. Callers that previously
    /// relied on LpgStore-inherent methods (memory_breakdown,
    /// edges_with_type, all_edges, nodes_with_label, create/drop
    /// property index, named-graph API) now reach the same surface
    /// through `GraphStoreMut` trait methods — substrate-compatible
    /// defaults return empty/zero values; LpgStore overrides
    /// delegate to inherent impls.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn GraphStoreMut> {
        &self.store
    }

    /// Returns the read-only mmap store if available (v2 native format).
    ///
    /// Returns the store for the currently active graph.
    ///
    /// If [`current_graph`](Self::current_graph) is `None` or `"default"`, returns
    /// the default store. Otherwise looks up the named graph in the root store.
    /// Falls back to the default store if the named graph does not exist.
    #[allow(dead_code)] // Reserved for future graph-aware CRUD methods
    fn active_store(&self) -> Arc<dyn GraphStoreMut> {
        let graph_name = self.current_graph.read().clone();
        let racine = self.data_store();
        match graph_name {
            None => racine,
            Some(ref name) if name.eq_ignore_ascii_case("default") => racine,
            Some(ref name) => racine.named_graph(name).unwrap_or(racine),
        }
    }

    // === Named Graph Management ===

    /// Creates a named graph. Returns `true` if created, `false` if it already exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph could not be created (nom invalide,
    /// repertoire inaccessible).
    pub fn create_graph(&self, name: &str) -> Result<bool> {
        // (2026-08-14) TROIS REFUS EXPLICITES SONT TOMBES ICI.
        //
        // `create_graph`, `drop_graph` et `list_graphs` commencaient par
        // `if self.substrate_store.is_some()` et rendaient respectivement une
        // erreur, `false` et un `Vec` vide, au motif que « substrate is
        // single-graph by design ». C'etait VRAI quand ces lignes ont ete
        // ecrites. Ca ne l'est plus : le substrat porte desormais ses graphes
        // nommes, un sous-substrat complet par graphe.
        //
        // Le symptome etait joliment trompeur : `CREATE GRAPH` fonctionnait
        // (la session passe par le store, pas par ces methodes) et le graphe
        // etait UTILISABLE — mais `list_graphs()` jurait qu'il n'existait pas.
        // Deux verites contradictoires dans la meme base, chacune sur de soi.
        //
        // Les trois passent maintenant par `data_store()`, comme tout le
        // reste.
        self.data_store()
            .create_named_graph(name)
            .map_err(|e| obrain_common::utils::error::Error::Internal(e.to_string()))
    }

    /// Drops a named graph. Returns `true` if dropped, `false` if it did not exist.
    ///
    pub fn drop_graph(&self, name: &str) -> bool {
        self.data_store().drop_named_graph(name)
    }

    /// Returns all named graph names.
    ///
    #[must_use]
    pub fn list_graphs(&self) -> Vec<String> {
        self.data_store().named_graph_names()
    }

    /// Returns the active data store as a trait object — the single
    /// authoritative backend for data operations (CRUD, queries, epoch,
    /// stats).
    ///
    /// Resolution priority:
    /// 1. `substrate_store` — typed substrate handle (preferred when present).
    /// 2. `external_store` — user-supplied `Arc<dyn GraphStoreMut>` from
    ///    `with_store()`.
    /// 3. `self.store` — the legacy `Arc<LpgStore>` field. In substrate
    ///    mode this holds a **dummy** created in `with_store()` and should
    ///    never be reached through this path; it is kept only as the
    ///    LpgStore-backed fallback for `with_config()` and tests.
    ///
    /// Use this helper internally in place of `self.store.X()` whenever
    /// `X` is a trait method (`current_epoch`, `node_count`, `edge_count`,
    /// `create_node`, `get_node`, ...). That routes the call to the real
    /// backend instead of the dummy, fixing the latent T17 bug where
    /// substrate-backed DBs silently returned dummy zeros/no-ops.
    ///
    /// See gotcha note `0b9fcabe-a780-4149-8709-ed32ee9ed82e`.
    #[must_use]
    pub(super) fn data_store(&self) -> Arc<dyn GraphStoreMut> {
        if let Some(ref sub) = self.substrate_store {
            return Arc::clone(sub) as Arc<dyn GraphStoreMut>;
        }
        if let Some(ref ext_store) = self.external_store {
            return Arc::clone(ext_store);
        }
        Arc::clone(&self.store) as Arc<dyn GraphStoreMut>
    }

    /// Returns the graph store as a trait object.
    ///
    /// This provides the [`GraphStoreMut`] interface for code that should work
    /// with any storage backend. Use this when you only need graph read/write
    /// operations and don't need admin methods like index management.
    ///
    /// [`GraphStoreMut`]: obrain_core::graph::GraphStoreMut
    #[must_use]
    pub fn graph_store(&self) -> Arc<dyn GraphStoreMut> {
        self.data_store()
    }

    /// Garbage collects old MVCC versions that are no longer visible.
    ///
    /// Determines the minimum epoch required by active transactions and prunes
    /// version chains older than that threshold. Also cleans up completed
    /// transaction metadata in the transaction manager.
    pub fn gc(&self) {
        let min_epoch = self.transaction_manager.min_active_epoch();
        self.store.gc_versions(min_epoch);
        self.transaction_manager.gc();
    }

    /// Returns the buffer manager for memory-aware operations.
    #[must_use]
    pub fn buffer_manager(&self) -> &Arc<BufferManager> {
        &self.buffer_manager
    }

    /// Returns the query cache.
    #[must_use]
    pub fn query_cache(&self) -> &Arc<QueryCache> {
        &self.query_cache
    }

    /// Clears all cached query plans.
    ///
    /// This is called automatically after DDL operations, but can also be
    /// invoked manually after external schema changes (e.g., WAL replay,
    /// import) or when you want to force re-optimization of all queries.
    pub fn clear_plan_cache(&self) {
        self.query_cache.clear();
    }

    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Closes the database, flushing all pending writes.
    ///
    /// For persistent databases, this ensures everything is safely on disk.
    /// Called automatically when the database is dropped, but you can call
    /// it explicitly if you need to guarantee durability at a specific point.
    ///
    /// # Errors
    ///
    /// Returns an error if the WAL can't be flushed (check disk space/permissions).
    pub fn close(&self) -> Result<()> {
        let mut is_open = self.is_open.write();
        if !*is_open {
            return Ok(());
        }
        // Une base fermée cesse d'être « vivante » pour l'ouvreur (voir
        // `open_substrate_inner`) : la rouvrir après `close()` est permis.
        if let Some(ref p) = self.config.path {
            desinscrire_substrat_vivant(p);
        }

        // Read-only databases: just release the shared lock, no checkpointing
        if self.read_only {
            *is_open = false;
            return Ok(());
        }

        // Journal RDF : fsync des records en attente (mode batch/nosync).
        #[cfg(all(feature = "rdf", feature = "wal"))]
        if let Some(ref rdf_wal) = self.rdf_wal {
            rdf_wal.sync()?;
        }

        // T17 final cutover: there is no single-file mode any more — substrate
        // databases are directory-based, legacy single-file `.obrain` v1/v2
        // was retired (feature `obrain-file` removed in commit `b0f5bd11`).

        // Commit WAL records (legacy directory format only).
        //
        // IMPORTANT: We must NOT write checkpoint metadata here because the
        // directory format has no base snapshot — data lives only in the WAL
        // files. Writing a checkpoint would cause recovery to skip all WAL
        // files before the checkpoint sequence, effectively losing all data.
        // (The single-file `.obrain` format handles this correctly via
        // `checkpoint_to_file()` which exports a real snapshot first.)
        #[cfg(feature = "wal")]
        if let Some(ref wal) = self.wal {
            // Use the last assigned transaction ID, or create one for the commit
            let commit_tx = self
                .transaction_manager
                .last_assigned_transaction_id()
                .unwrap_or_else(|| self.transaction_manager.begin());

            // Log a TransactionCommit to mark all pending records as committed,
            // so recovery knows to replay them.
            wal.log(&WalRecord::TransactionCommit {
                transaction_id: commit_tx,
            })?;

            // Flush to disk but do NOT write checkpoint.meta
            wal.sync()?;
        }

        // Substrate is the single backend (T17 cutover): persist its dict
        // (registries + high-water marks) and msync the mmap zones before
        // marking the database closed. Without this, reopen sees an empty
        // or stale dict and treats all previously allocated nodes/edges as
        // non-live (is_live_on_disk → false).
        if let Some(ref sub) = self.substrate_store
            && let Err(e) = sub.flush()
        {
            tracing::error!("substrate flush during close failed: {e}");
        }

        // ── LES INDEX TEXTE SONT REPERSISTÉS ICI, APRÈS LE FLUSH ────────────
        // L'empreinte du store (offset du WAL) BOUGE pendant la fermeture
        // elle-même — mesuré : 551 → 583, les écritures de flush. Un index
        // persisté au moment de sa construction porte donc une empreinte qui ne
        // correspond plus dès la première réouverture, et il serait reconstruit
        // à chaque fois : la persistance ne servirait à rien.
        //
        // Mais on ne peut pas pour autant tout réécrire : l'index en RAM n'est
        // PAS maintenu par les écritures, donc à la fermeture il peut être
        // PÉRIMÉ — le graver ferait croire à une réouverture qu'il décrit l'état
        // actuel (c'est le premier jet de ce chantier, et le test l'a démasqué).
        // La fonction décide donc index par index : elle réécrit ceux qui n'ont
        // rien vu passer, et EFFACE les autres.
        #[cfg(feature = "text-index")]
        self.repersister_index_textuels();
        #[cfg(feature = "vector-index")]
        self.repersister_index_vectoriels();

        *is_open = false;
        Ok(())
    }

    /// Commits the WAL — flushes pending records and optionally checkpoints
    /// the snapshot to the `.obrain` file.
    ///
    /// This is the primary durability API: callers should invoke this after
    /// a batch of mutations that must survive process restarts.
    #[cfg(feature = "wal")]
    pub fn commit_wal(&self) -> Result<()> {
        if let Some(ref wal) = self.wal {
            wal.flush()?;
        }

        // T17 final cutover: `file_manager` (single-file `.obrain` v1/v2
        // format) was retired; no checkpoint_to_file step is needed in
        // substrate mode — the substrate's own flush below handles
        // durability for mmap zones and dict columns.

        // Substrate dict + mmap zones must be persisted alongside the WAL
        // commit for cross-restart durability (T17 cutover — unconditional).
        if let Some(ref sub) = self.substrate_store {
            sub.flush().map_err(|e| {
                obrain_common::utils::error::Error::Internal(format!(
                    "substrate flush during commit_wal failed: {e}"
                ))
            })?;
        }

        Ok(())
    }

    /// Returns the typed WAL if available.
    #[cfg(feature = "wal")]
    #[must_use]
    pub fn wal(&self) -> Option<&Arc<LpgWal>> {
        self.wal.as_ref()
    }

    /// Returns a WAL-aware [`GraphStoreMut`] handle for direct-mutation consumers.
    ///
    /// When the database has a WAL, this returns an
    /// [`Arc<WalGraphStore>`](wal_store::WalGraphStore) that logs every mutation
    /// (create/delete nodes & edges, set properties) before delegating to the
    /// in-memory `LpgStore`. Without a WAL, it returns the raw
    /// `Arc<LpgStore>` directly.
    ///
    /// This is the sanctioned entry point for subsystems that need to mutate
    /// the graph outside a query (e.g. hub-level stores: AssetRegistry,
    /// UserBrain, PersonaStore, etc.) while preserving durability. It replaces
    /// the now-removed `PersistentStore` wrapper that used to live in
    /// `obrain-hub`.
    #[must_use]
    pub fn graph_store_mut(&self) -> Arc<dyn GraphStoreMut> {
        // T17 cutover: substrate is the single authoritative store. Its
        // `GraphStoreMut` impl is already WAL-native (every mutation logs a
        // WalRecord synchronously before touching mmap), so we do NOT wrap it
        // with `WalGraphStore` — doing so would funnel writes through the
        // dummy `self.store` LpgStore that lives next to the substrate in
        // `with_store`, silently dropping them on disk. Return the
        // substrate's erased handle directly.
        if let Some(ref sub) = self.substrate_store {
            return Arc::clone(sub) as Arc<dyn GraphStoreMut>;
        }
        #[cfg(feature = "wal")]
        if let Some(ref wal) = self.wal {
            return Arc::new(wal_store::WalGraphStore::new(
                Arc::clone(&self.store) as Arc<dyn GraphStoreMut>,
                Arc::clone(wal),
                Arc::clone(&self.wal_graph_context),
            ));
        }
        Arc::clone(&self.store) as Arc<dyn GraphStoreMut>
    }

    /// Returns the typed [`SubstrateStore`] handle when the database is
    /// backed by the substrate backend, or `None` otherwise.
    ///
    /// This is the progressive-cutover hook for T6: cognitive stores
    /// (energy / scar / utility / affinity / synapse) use this to route
    /// cognitive mutations through the substrate column APIs
    /// (`reinforce_edge_synapse_f32`, `decay_all_edge_synapse`,
    /// `boost_edge_synapse_f32`, ...) which aren't part of the
    /// `GraphStoreMut` trait. When `None`, cognitive stores fall back to
    /// the LPG-property path (legacy behaviour).
    ///
    /// A typical call sequence from the hub:
    ///
    /// ```ignore
    /// let synapse = match db.substrate_handle() {
    ///     Some(sub) => SynapseStore::with_substrate(cfg, graph_store, sub),
    ///     None => SynapseStore::new(cfg, graph_store),
    /// };
    /// ```
    ///
    /// [`SubstrateStore`]: obrain_substrate::SubstrateStore
    #[must_use]
    pub fn substrate_handle(&self) -> Option<Arc<obrain_substrate::SubstrateStore>> {
        self.substrate_store.as_ref().map(Arc::clone)
    }

    /// (2026-09-01) Reconstruit les colonnes de degrés (générique + typées)
    /// depuis un scan des arêtes — le remède aux sidecars périmés (migrés
    /// par un binaire plus ancien : CRC valide, compteurs faux → les
    /// expansions d'arêtes typées sortantes renvoyaient 0, ex.
    /// `sessions_list`/`session_load` du harness vides). Nécessite un
    /// lock exclusif sur le store. Retourne le nombre de sidecars
    /// supprimés/rebâtis.
    pub fn rebuild_degree_columns(&self) -> std::result::Result<usize, String> {
        let Some(store) = self.substrate_handle() else {
            return Err("rebuild_degrees: store substrate absent".into());
        };
        store
            .rebuild_degree_columns()
            .map_err(|e| format!("rebuild_degrees: {e}"))
    }

    /// (2026-09-14, NATIF-R1) Reconstruit les chaines d'adjacence
    /// (`next_from`/`next_to`/`first_edge_off`) depuis les `EdgeRecord`
    /// vivants — remede a l'explosion de la traversee non typee causee par
    /// des chaines CYCLIQUES (incident dedup runtime-dev). `count(*)` reste
    /// juste tandis que `count(a)` explose ; ni `repair-degrees` (degres
    /// recalcules depuis les records, deja justes) ni `compact` (ne touche
    /// pas les pointeurs de chaine) n'y remedient. Idempotent ; `dry_run`
    /// diagnostique sans ecrire. Necessite un lock exclusif sur le store.
    pub fn rebuild_edge_chains(
        &self,
        dry_run: bool,
    ) -> std::result::Result<obrain_substrate::store::EdgeChainRebuildStats, String> {
        let Some(store) = self.substrate_handle() else {
            return Err("rebuild-edge-chains: store substrate absent".into());
        };
        store
            .rebuild_edge_chains(dry_run)
            .map_err(|e| format!("rebuild-edge-chains: {e}"))
    }

    /// Journal de session : écrit les lignes brutes du log dans la zone
    /// substrate dédiée (migration depuis le MCP).
    pub fn write_session_journal(
        &self,
        session_id: &str,
        lines: &[String],
    ) -> std::result::Result<(), String> {
        let Some(store) = self.substrate_handle() else {
            return Err("journal: store substrate absent".into());
        };
        crate::journal_zone::write_session_journal(&store, session_id, lines)
    }

    /// Journal de session : APPEND les lignes brutes à la zone existante
    /// (capture live GraphWriter — le journal accumule les lots successifs).
    pub fn append_session_journal(
        &self,
        session_id: &str,
        lines: &[String],
    ) -> std::result::Result<(), String> {
        let Some(store) = self.substrate_handle() else {
            return Err("journal: store substrate absent".into());
        };
        crate::journal_zone::append_session_journal(&store, session_id, lines)
    }

    /// Journal de session : lit les lignes de la zone (paginées par seq).
    pub fn read_session_journal(
        &self,
        session_id: &str,
        after_seq: i64,
        before_seq: i64,
        limit: usize,
    ) -> Vec<String> {
        let Some(store) = self.substrate_handle() else {
            return Vec::new();
        };
        crate::journal_zone::read_session_journal(&store, session_id, after_seq, before_seq, limit)
    }

    /// Journal de session : le seq du dernier événement de la session (pour
    /// `events_last_seq`).
    pub fn session_journal_last_seq(&self, session_id: &str) -> i64 {
        let lines = self.read_session_journal(session_id, -1, 0, 1);
        lines
            .last()
            .map_or(-1, |l| crate::journal_zone::session_seq(l))
    }

    /// Logs a WAL record if WAL is enabled.
    #[cfg(feature = "wal")]
    pub(super) fn log_wal(&self, record: &WalRecord) -> Result<()> {
        if let Some(ref wal) = self.wal {
            wal.log(record)?;
        }
        Ok(())
    }

    // T17 final cutover (2026-04-23): `checkpoint_to_file` and
    // `file_manager` accessors were retired with the `obrain-file`
    // feature (commit `b0f5bd11`). Substrate is mmap + WAL-native and
    // persists automatically through its own flush path — no separate
    // single-file snapshot export is needed. A future slice (T17b)
    // may reintroduce an explicit snapshot export via
    // `SubstrateStore::snapshot_to_path()`.
}

impl Drop for ObrainDB {
    fn drop(&mut self) {
        if let Err(e) = self.close() {
            tracing::error!("Error closing database: {}", e);
        }
    }
}

impl crate::admin::AdminService for ObrainDB {
    fn info(&self) -> crate::admin::DatabaseInfo {
        self.info()
    }

    fn detailed_stats(&self) -> crate::admin::DatabaseStats {
        self.detailed_stats()
    }

    fn schema(&self) -> crate::admin::SchemaInfo {
        self.schema()
    }

    fn validate(&self) -> crate::admin::ValidationResult {
        self.validate()
    }

    fn wal_status(&self) -> crate::admin::WalStatus {
        self.wal_status()
    }

    fn wal_checkpoint(&self) -> Result<()> {
        self.wal_checkpoint()
    }
}

// =========================================================================
// Query Result Types
// =========================================================================

/// The result of running a query.
///
/// Contains rows and columns, like a table. Use [`iter()`](Self::iter) to
/// loop through rows, or [`scalar()`](Self::scalar) if you expect a single value.
///
/// # Examples
///
/// ```
/// use obrain_engine::ObrainDB;
///
/// let db = ObrainDB::new_in_memory();
/// db.create_node(&["Person"]);
///
/// let result = db.execute("MATCH (p:Person) RETURN count(p) AS total")?;
///
/// // Check what we got
/// println!("Columns: {:?}", result.columns);
/// println!("Rows: {}", result.row_count());
///
/// // Iterate through results
/// for row in result.iter() {
///     println!("{:?}", row);
/// }
/// # Ok::<(), obrain_common::utils::error::Error>(())
/// ```
#[derive(Debug)]
pub struct QueryResult {
    /// Column names from the RETURN clause.
    pub columns: Vec<String>,
    /// Column types - useful for distinguishing NodeId/EdgeId from plain integers.
    pub column_types: Vec<obrain_common::types::LogicalType>,
    /// The actual result rows.
    pub rows: Vec<Vec<obrain_common::types::Value>>,
    /// Query execution time in milliseconds (if timing was enabled).
    pub execution_time_ms: Option<f64>,
    /// Number of rows scanned during query execution (estimate).
    pub rows_scanned: Option<u64>,
    /// Status message for DDL and session commands (e.g., "Created node type 'Person'").
    pub status_message: Option<String>,
    /// GQLSTATUS code per ISO/IEC 39075:2024, sec 23.
    pub gql_status: obrain_common::utils::GqlStatus,
}

impl QueryResult {
    /// Creates a fully empty query result (no columns, no rows).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            column_types: Vec::new(),
            rows: Vec::new(),
            execution_time_ms: None,
            rows_scanned: None,
            status_message: None,
            gql_status: obrain_common::utils::GqlStatus::SUCCESS,
        }
    }

    /// Creates a query result with only a status message (for DDL commands).
    #[must_use]
    pub fn status(msg: impl Into<String>) -> Self {
        Self {
            columns: Vec::new(),
            column_types: Vec::new(),
            rows: Vec::new(),
            execution_time_ms: None,
            rows_scanned: None,
            status_message: Some(msg.into()),
            gql_status: obrain_common::utils::GqlStatus::SUCCESS,
        }
    }

    /// Creates a new empty query result.
    #[must_use]
    pub fn new(columns: Vec<String>) -> Self {
        let len = columns.len();
        Self {
            columns,
            column_types: vec![obrain_common::types::LogicalType::Any; len],
            rows: Vec::new(),
            execution_time_ms: None,
            rows_scanned: None,
            status_message: None,
            gql_status: obrain_common::utils::GqlStatus::SUCCESS,
        }
    }

    /// Creates a new empty query result with column types.
    #[must_use]
    pub fn with_types(
        columns: Vec<String>,
        column_types: Vec<obrain_common::types::LogicalType>,
    ) -> Self {
        Self {
            columns,
            column_types,
            rows: Vec::new(),
            execution_time_ms: None,
            rows_scanned: None,
            status_message: None,
            gql_status: obrain_common::utils::GqlStatus::SUCCESS,
        }
    }

    /// Sets the execution metrics on this result.
    pub fn with_metrics(mut self, execution_time_ms: f64, rows_scanned: u64) -> Self {
        self.execution_time_ms = Some(execution_time_ms);
        self.rows_scanned = Some(rows_scanned);
        self
    }

    /// Returns the execution time in milliseconds, if available.
    #[must_use]
    pub fn execution_time_ms(&self) -> Option<f64> {
        self.execution_time_ms
    }

    /// Returns the number of rows scanned, if available.
    #[must_use]
    pub fn rows_scanned(&self) -> Option<u64> {
        self.rows_scanned
    }

    /// Returns the number of rows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Returns the number of columns.
    #[must_use]
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Returns true if the result is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Extracts a single value from the result.
    ///
    /// Use this when your query returns exactly one row with one column,
    /// like `RETURN count(n)` or `RETURN sum(p.amount)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the result has multiple rows or columns.
    pub fn scalar<T: FromValue>(&self) -> Result<T> {
        if self.rows.len() != 1 || self.columns.len() != 1 {
            return Err(obrain_common::utils::error::Error::InvalidValue(
                "Expected single value".to_string(),
            ));
        }
        T::from_value(&self.rows[0][0])
    }

    /// Returns an iterator over the rows.
    pub fn iter(&self) -> impl Iterator<Item = &Vec<obrain_common::types::Value>> {
        self.rows.iter()
    }
}

impl std::fmt::Display for QueryResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let table = obrain_common::fmt::format_result_table(
            &self.columns,
            &self.rows,
            self.execution_time_ms,
            self.status_message.as_deref(),
        );
        f.write_str(&table)
    }
}

/// Converts a [`obrain_common::types::Value`] to a concrete Rust type.
///
/// Implemented for common types like `i64`, `f64`, `String`, and `bool`.
/// Used by [`QueryResult::scalar()`] to extract typed values.
pub trait FromValue: Sized {
    /// Attempts the conversion, returning an error on type mismatch.
    fn from_value(value: &obrain_common::types::Value) -> Result<Self>;
}

impl FromValue for i64 {
    fn from_value(value: &obrain_common::types::Value) -> Result<Self> {
        value
            .as_int64()
            .ok_or_else(|| obrain_common::utils::error::Error::TypeMismatch {
                expected: "INT64".to_string(),
                found: value.type_name().to_string(),
            })
    }
}

impl FromValue for f64 {
    fn from_value(value: &obrain_common::types::Value) -> Result<Self> {
        value
            .as_float64()
            .ok_or_else(|| obrain_common::utils::error::Error::TypeMismatch {
                expected: "FLOAT64".to_string(),
                found: value.type_name().to_string(),
            })
    }
}

impl FromValue for String {
    fn from_value(value: &obrain_common::types::Value) -> Result<Self> {
        value.as_str().map(String::from).ok_or_else(|| {
            obrain_common::utils::error::Error::TypeMismatch {
                expected: "STRING".to_string(),
                found: value.type_name().to_string(),
            }
        })
    }
}

impl FromValue for bool {
    fn from_value(value: &obrain_common::types::Value) -> Result<Self> {
        value
            .as_bool()
            .ok_or_else(|| obrain_common::utils::error::Error::TypeMismatch {
                expected: "BOOL".to_string(),
                found: value.type_name().to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_in_memory_database() {
        let db = ObrainDB::new_in_memory();
        assert_eq!(db.node_count(), 0);
        assert_eq!(db.edge_count(), 0);
    }

    #[test]
    fn test_database_config() {
        let config = Config::in_memory().with_threads(4).with_query_logging();

        let db = ObrainDB::with_config(config).unwrap();
        assert_eq!(db.config().threads, 4);
        assert!(db.config().query_logging);
    }

    #[test]
    fn test_database_session() {
        let db = ObrainDB::new_in_memory();
        let _session = db.session();
        // Session should be created successfully
    }

    #[cfg(feature = "wal")]
    #[test]
    fn test_persistent_database_recovery() {
        use obrain_common::types::Value;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_db");

        // Create database and add some data.
        // NodeId allocation is backend-dependent: substrate reserves NodeId(0)
        // as a sentinel and allocates from 1 upward, whereas the legacy
        // LpgStore started at 0. Capture the IDs returned by `create_node` so
        // the assertions below stay portable across backends.
        let (alix_id, gus_id) = {
            let db = ObrainDB::open(&db_path).unwrap();

            let alix = db.create_node(&["Person"]);
            db.set_node_property(alix, "name", Value::from("Alix"));

            let gus = db.create_node(&["Person"]);
            db.set_node_property(gus, "name", Value::from("Gus"));

            let _edge = db.create_edge(alix, gus, "KNOWS");

            // Explicitly close to flush WAL
            db.close().unwrap();
            (alix, gus)
        };

        // Reopen and verify data was recovered
        {
            let db = ObrainDB::open(&db_path).unwrap();

            assert_eq!(db.node_count(), 2);
            assert_eq!(db.edge_count(), 1);

            // Verify nodes exist (use captured IDs — backend-agnostic).
            let node0 = db.get_node(alix_id);
            assert!(node0.is_some());

            let node1 = db.get_node(gus_id);
            assert!(node1.is_some());
        }
    }

    #[cfg(feature = "wal")]
    #[test]
    fn test_wal_logging() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("wal_test_db");

        let db = ObrainDB::open(&db_path).unwrap();

        // Create some data
        let node = db.create_node(&["Test"]);
        db.delete_node(node);

        // WAL should have records
        if let Some(wal) = db.wal() {
            assert!(wal.record_count() > 0);
        }

        db.close().unwrap();
    }

    #[cfg(feature = "wal")]
    #[test]
    fn test_wal_recovery_multiple_sessions() {
        // Tests that WAL recovery works correctly across multiple open/close cycles
        use obrain_common::types::Value;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("multi_session_db");

        // Session 1: Create initial data.
        // Capture the substrate-allocated IDs so the cross-session assertions
        // below remain portable (substrate starts at NodeId(1); legacy LpgStore
        // started at 0).
        let alix_id = {
            let db = ObrainDB::open(&db_path).unwrap();
            let alix = db.create_node(&["Person"]);
            db.set_node_property(alix, "name", Value::from("Alix"));
            db.close().unwrap();
            alix
        };

        // Session 2: Add more data
        let gus_id = {
            let db = ObrainDB::open(&db_path).unwrap();
            assert_eq!(db.node_count(), 1); // Previous data recovered
            let gus = db.create_node(&["Person"]);
            db.set_node_property(gus, "name", Value::from("Gus"));
            db.close().unwrap();
            gus
        };

        // Session 3: Verify all data
        {
            let db = ObrainDB::open(&db_path).unwrap();
            assert_eq!(db.node_count(), 2);

            // Verify properties were recovered correctly (use captured IDs).
            let node0 = db.get_node(alix_id).unwrap();
            assert!(node0.labels.iter().any(|l| l.as_str() == "Person"));

            let node1 = db.get_node(gus_id).unwrap();
            assert!(node1.labels.iter().any(|l| l.as_str() == "Person"));
        }
    }

    #[cfg(feature = "wal")]
    #[test]
    fn test_database_consistency_after_mutations() {
        // Tests that database remains consistent after a series of create/delete operations
        use obrain_common::types::Value;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("consistency_db");

        // Capture substrate-allocated IDs so the cross-session assertions
        // below remain portable across backends (substrate reserves NodeId(0)
        // as a sentinel).
        let (a_id, b_id, c_id) = {
            let db = ObrainDB::open(&db_path).unwrap();

            // Create nodes
            let a = db.create_node(&["Node"]);
            let b = db.create_node(&["Node"]);
            let c = db.create_node(&["Node"]);

            // Create edges
            let e1 = db.create_edge(a, b, "LINKS");
            let _e2 = db.create_edge(b, c, "LINKS");

            // Delete middle node and its edge
            db.delete_edge(e1);
            db.delete_node(b);

            // Set properties on remaining nodes
            db.set_node_property(a, "value", Value::Int64(1));
            db.set_node_property(c, "value", Value::Int64(3));

            db.close().unwrap();
            (a, b, c)
        };

        // Reopen and verify consistency
        {
            let db = ObrainDB::open(&db_path).unwrap();

            // Should have 2 nodes (a and c), b was deleted
            // Note: node_count includes deleted nodes in some implementations
            // What matters is that the non-deleted nodes are accessible
            let node_a = db.get_node(a_id);
            assert!(node_a.is_some());

            let node_c = db.get_node(c_id);
            assert!(node_c.is_some());

            // Middle node should be deleted
            let node_b = db.get_node(b_id);
            assert!(node_b.is_none());
        }
    }

    #[cfg(feature = "wal")]
    #[test]
    fn test_close_is_idempotent() {
        // Calling close() multiple times should not cause errors
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("close_test_db");

        let db = ObrainDB::open(&db_path).unwrap();
        db.create_node(&["Test"]);

        // First close should succeed
        assert!(db.close().is_ok());

        // Second close should also succeed (idempotent)
        assert!(db.close().is_ok());
    }

    #[test]
    fn test_checkpoint_flushes_persistent_store() {
        // db.checkpoint() should succeed on a substrate-backed persistent
        // database, be callable repeatedly (idempotent), and not lose any
        // data in between calls -- it exists specifically to let a
        // long-lived caller bound how much unflushed props-zone-v2 state
        // a mid-session process kill can lose (see doc comment).
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("checkpoint_test_db");

        let db = ObrainDB::open(&db_path).unwrap();
        let node = db.create_node(&["Test"]);
        db.set_node_property(node, "name", obrain_common::types::Value::from("alpha"));

        assert!(db.checkpoint().is_ok());
        // Idempotent: calling it again with no new mutations must also succeed.
        assert!(db.checkpoint().is_ok());

        // Data must still be intact after checkpointing.
        let fetched = db.get_node(node).unwrap();
        assert_eq!(
            fetched
                .properties
                .get(&obrain_common::types::PropertyKey::new("name")),
            Some(&obrain_common::types::Value::from("alpha"))
        );

        db.close().unwrap();
    }

    #[test]
    fn test_checkpoint_is_noop_on_in_memory_db() {
        // Non-substrate-backed (in-memory) databases have no zones to
        // flush -- checkpoint() must be a harmless Ok(()) no-op, not an
        // error, so callers can call it unconditionally regardless of
        // backend.
        let db = ObrainDB::new_in_memory();
        assert!(db.checkpoint().is_ok());
    }

    #[test]
    fn test_with_store_external_backend() {
        let external = Arc::new(obrain_substrate::SubstrateStore::open_tempfile().unwrap())
            as Arc<dyn obrain_core::graph::GraphStoreMut>;

        // Seed data on the external store directly
        let n1 = external.create_node(&["Person"]);
        external.set_node_property(n1, "name", obrain_common::types::Value::from("Alix"));

        let db = ObrainDB::with_store(
            Arc::clone(&external) as Arc<dyn GraphStoreMut>,
            Config::in_memory(),
        )
        .unwrap();

        let session = db.session();

        // Session should see data from the external store via execute
        {
            let result = session.execute("MATCH (p:Person) RETURN p.name").unwrap();
            assert_eq!(result.rows.len(), 1);
        }
    }

    #[test]
    fn test_with_config_custom_memory_limit() {
        let config = Config::in_memory().with_memory_limit(64 * 1024 * 1024); // 64 MB

        let db = ObrainDB::with_config(config).unwrap();
        assert_eq!(db.config().memory_limit, Some(64 * 1024 * 1024));
        assert_eq!(db.node_count(), 0);
    }

    #[cfg(feature = "metrics")]
    #[test]
    fn test_database_metrics_registry() {
        let db = ObrainDB::new_in_memory();

        // Perform some operations
        db.create_node(&["Person"]);
        db.create_node(&["Person"]);

        // Check that metrics snapshot returns data
        let snap = db.metrics();
        // Session created counter should reflect at least 0 (metrics is initialized)
        assert_eq!(snap.query_count, 0); // No queries executed yet
    }

    #[test]
    fn test_query_result_has_metrics() {
        // Verifies that query results include execution metrics
        let db = ObrainDB::new_in_memory();
        db.create_node(&["Person"]);
        db.create_node(&["Person"]);

        {
            let result = db.execute("MATCH (n:Person) RETURN n").unwrap();

            // Metrics should be populated
            assert!(result.execution_time_ms.is_some());
            assert!(result.rows_scanned.is_some());
            assert!(result.execution_time_ms.unwrap() >= 0.0);
            assert_eq!(result.rows_scanned.unwrap(), 2);
        }
    }

    #[test]
    fn test_empty_query_result_metrics() {
        // Verifies metrics are correct for queries returning no results
        let db = ObrainDB::new_in_memory();
        db.create_node(&["Person"]);

        {
            // Query that matches nothing
            let result = db.execute("MATCH (n:NonExistent) RETURN n").unwrap();

            assert!(result.execution_time_ms.is_some());
            assert!(result.rows_scanned.is_some());
            assert_eq!(result.rows_scanned.unwrap(), 0);
        }
    }

    #[cfg(feature = "cdc")]
    mod cdc_integration {
        use super::*;

        #[test]
        fn test_node_lifecycle_history() {
            let db = ObrainDB::new_in_memory();

            // Create
            let id = db.create_node(&["Person"]);
            // Update
            db.set_node_property(id, "name", "Alix".into());
            db.set_node_property(id, "name", "Gus".into());
            // Delete
            db.delete_node(id);

            let history = db.history(id).unwrap();
            assert_eq!(history.len(), 4); // create + 2 updates + delete
            assert_eq!(history[0].kind, crate::cdc::ChangeKind::Create);
            assert_eq!(history[1].kind, crate::cdc::ChangeKind::Update);
            assert!(history[1].before.is_none()); // first set_node_property has no prior value
            assert_eq!(history[2].kind, crate::cdc::ChangeKind::Update);
            assert!(history[2].before.is_some()); // second update has prior "Alix"
            assert_eq!(history[3].kind, crate::cdc::ChangeKind::Delete);
        }

        #[test]
        fn test_edge_lifecycle_history() {
            let db = ObrainDB::new_in_memory();

            let alix = db.create_node(&["Person"]);
            let gus = db.create_node(&["Person"]);
            let edge = db.create_edge(alix, gus, "KNOWS");
            db.set_edge_property(edge, "since", 2024i64.into());
            db.delete_edge(edge);

            let history = db.history(edge).unwrap();
            assert_eq!(history.len(), 3); // create + update + delete
            assert_eq!(history[0].kind, crate::cdc::ChangeKind::Create);
            assert_eq!(history[1].kind, crate::cdc::ChangeKind::Update);
            assert_eq!(history[2].kind, crate::cdc::ChangeKind::Delete);
        }

        #[test]
        fn test_create_node_with_props_cdc() {
            let db = ObrainDB::new_in_memory();

            let id = db.create_node_with_props(
                &["Person"],
                vec![
                    ("name", obrain_common::types::Value::from("Alix")),
                    ("age", obrain_common::types::Value::from(30i64)),
                ],
            );

            let history = db.history(id).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].kind, crate::cdc::ChangeKind::Create);
            // Props should be captured
            let after = history[0].after.as_ref().unwrap();
            assert_eq!(after.len(), 2);
        }

        #[test]
        fn test_changes_between() {
            let db = ObrainDB::new_in_memory();

            let id1 = db.create_node(&["A"]);
            let _id2 = db.create_node(&["B"]);
            db.set_node_property(id1, "x", 1i64.into());

            // All events should be at the same epoch (in-memory, epoch doesn't advance without tx)
            let changes = db
                .changes_between(
                    obrain_common::types::EpochId(0),
                    obrain_common::types::EpochId(u64::MAX),
                )
                .unwrap();
            assert_eq!(changes.len(), 3); // 2 creates + 1 update
        }
    }

    #[test]
    fn test_with_store_basic() {
        let store = Arc::new(obrain_substrate::SubstrateStore::open_tempfile().unwrap())
            as Arc<dyn obrain_core::graph::GraphStoreMut>;
        let n1 = store.create_node(&["Person"]);
        store.set_node_property(n1, "name", "Alix".into());

        let graph_store = Arc::clone(&store) as Arc<dyn GraphStoreMut>;
        let db = ObrainDB::with_store(graph_store, Config::in_memory()).unwrap();

        let result = db.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_with_store_session() {
        let store = Arc::new(obrain_substrate::SubstrateStore::open_tempfile().unwrap())
            as Arc<dyn obrain_core::graph::GraphStoreMut>;
        let graph_store = Arc::clone(&store) as Arc<dyn GraphStoreMut>;
        let db = ObrainDB::with_store(graph_store, Config::in_memory()).unwrap();

        let session = db.session();
        let result = session.execute("MATCH (n) RETURN count(n)").unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_with_store_mutations() {
        let store = Arc::new(obrain_substrate::SubstrateStore::open_tempfile().unwrap())
            as Arc<dyn obrain_core::graph::GraphStoreMut>;
        let graph_store = Arc::clone(&store) as Arc<dyn GraphStoreMut>;
        let db = ObrainDB::with_store(graph_store, Config::in_memory()).unwrap();

        let mut session = db.session();

        // Use an explicit transaction so INSERT and MATCH share the same
        // transaction context. With PENDING epochs, uncommitted versions are
        // only visible to the owning transaction.
        session.begin_transaction().unwrap();
        session.execute("INSERT (:Person {name: 'Alix'})").unwrap();

        let result = session.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);

        session.commit().unwrap();
    }
}
