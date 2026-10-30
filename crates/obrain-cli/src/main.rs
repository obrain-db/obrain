//! Obrain CLI - Command-line interface for Obrain graph databases.
//!
//! Provides admin commands, single-shot query execution, and an interactive REPL.

#![deny(unsafe_code)]

// ── Allocateur global de rechange (banc T7 perf/RAM) ────────────────────
// Par défaut : allocateur système (aucune feature). Les deux features sont
// exclusives — un binaire au double allocateur n'aurait pas de sens et le
// second `#[global_allocator]` serait de toute façon refusé par rustc ; le
// message ci-dessous rend le refus lisible.
#[cfg(all(feature = "alloc-jemalloc", feature = "alloc-mimalloc"))]
compile_error!("alloc-jemalloc et alloc-mimalloc sont exclusifs : n'en activer qu'une");

#[cfg(feature = "alloc-jemalloc")]
#[global_allocator]
static ALLOCATEUR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "alloc-mimalloc")]
#[global_allocator]
static ALLOCATEUR: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod viz_ws;
mod activation;
mod ancrage;
mod commands;
mod engine;
mod entretien;
mod harness_serve;
mod http_min;
mod hubs;
mod machines;
mod memoire;
mod mcp_server;
mod outils;
mod orbites;
mod output;
mod remote;
mod repl;
mod resonance;
mod viz_extract;
mod viz_serve;
mod viz_timeline;
mod viz_weave;
mod worlds;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::path::PathBuf;

/// CLI exit codes for scripting and CI integration.
///
/// | Code | Meaning |
/// |------|---------|
/// | 0 | Success |
/// | 1 | General error (runtime, I/O, query) |
/// | 2 | Validation failed (`obrain validate`) |
#[repr(u8)]
enum ExitCode {
    /// Command completed normally.
    Success = 0,
    /// Runtime error, I/O failure, or query error.
    GeneralError = 1,
    /// `obrain validate` found integrity errors.
    ValidationFailed = 2,
}

/// Obrain graph database CLI.
///
/// Inspect, query, and maintain Obrain graph databases from the command line.
#[derive(Parser)]
#[command(name = "obrain")]
#[command(author, version, about, long_about = None)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format (auto-detects TTY: table for interactive, JSON for piped)
    #[arg(long, global = true, default_value = "auto")]
    format: OutputFormat,

    /// Suppress progress and info messages
    #[arg(long, short, global = true)]
    quiet: bool,

    /// Enable verbose debug logging
    #[arg(long, short, global = true)]
    verbose: bool,

    /// Disable colored output (also respects NO_COLOR env var)
    #[arg(long, global = true)]
    no_color: bool,

    /// Force colored output even when piped
    #[arg(long, global = true, conflicts_with = "no_color")]
    color: bool,
}

/// Output format options.
#[derive(Clone, Copy, ValueEnum, Default)]
enum OutputFormat {
    /// Auto-detect: table for TTY, JSON when piped
    #[default]
    Auto,
    /// Human-readable table format
    Table,
    /// Machine-readable JSON format
    Json,
    /// CSV format
    Csv,
}

/// Available commands.
#[derive(Subcommand)]
enum Commands {
    /// Display database information (counts, size, mode)
    Info {
        /// Path to the database
        path: PathBuf,
    },

    /// Show detailed statistics
    Stats {
        /// Path to the database
        path: PathBuf,
    },

    /// Display schema information (labels, edge types, property keys)
    Schema {
        /// Path to the database
        path: PathBuf,
    },

    /// Diagnose (and optionally repair) an interrupted slot repermutation
    ///
    /// Without `--apply`, nothing is modified: the command only reports what
    /// the durable state allows to establish, and what it would do.
    Repair {
        /// Path to the database
        path: PathBuf,

        /// Explicitly request a dry run (the default; kept for symmetry
        /// with `compact` and for self-documenting scripts)
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,

        /// Actually apply the repair. Requires `--verdict`.
        #[arg(long)]
        apply: bool,

        /// Operator's attestation. The two values command OPPOSITE
        /// actions: getting it wrong corrupts the database.
        #[arg(long, value_enum, requires = "apply")]
        verdict: Option<commands::repair::Verdict>,
    },

    /// Rebuild the degree columns (generic + per-edge-type) from an
    /// edge scan — the remedy for stale degree sidecars.
    ///
    /// Stale sidecars (migrated by an older binary with a different slot
    /// mapping) pass the open validation but carry wrong counters, so
    /// outgoing typed-edge patterns (`MATCH (a)-[:T]->(b)`) return 0 and
    /// harness queries like `sessions_list`/`session_load` come back
    /// empty. Idempotent ; requires exclusive access to the store
    /// (stop the service first, restart afterwards).
    RepairDegrees {
        /// Path to the database
        path: PathBuf,
    },

    /// Scan the v2 property zone for DANGLING heap references and, with
    /// `--apply`, remove the affected property entries.
    ///
    /// A `StringRef` / `BytesRef` / `ArrStringRef` entry carries a
    /// `HeapRef` into the companion heap zone, not the bytes themselves.
    /// An earlier flush order made the POINTER durable before the
    /// POINTEE, so a hard stop in that window left entries referencing
    /// heap bytes that never reached disk: the read path returns "no
    /// value" for a property the props zone still holds.
    ///
    /// Without `--apply` the store is opened READ-ONLY and nothing is
    /// written — that is the mode to run on a production base. Entries
    /// buried under a healthy rewrite of the same key are reported but
    /// never removed (a tombstone would destroy the healthy value).
    /// Heap pages whose CRC32 fails are reported too, and never touched.
    RepairProps {
        /// Path to the database
        path: PathBuf,

        /// Explicitly request a dry run (the default; kept for symmetry
        /// with `repair` and `compact`)
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,

        /// Actually remove the dangling property entries, then flush.
        #[arg(long)]
        apply: bool,
    },

    /// (NATIF-R1) Reconstruit les chaines d'adjacence des aretes
    /// (`next_from` sortant / `next_to` entrant / `first_edge_off`) depuis
    /// les `EdgeRecord` vivants, la source de verite.
    ///
    /// Remede a l'EXPLOSION de la traversee non typee : des ecritures
    /// concurrentes d'avant `splice_lock` (incident dedup runtime-dev
    /// 2026-09-08) ont rendu des chaines CYCLIQUES. La marche des chaines
    /// (`MATCH (a)-->(b)`) boucle alors jusqu'a la garde anti-cycle et rend
    /// N fois chaque arete, tandis que le compteur d'enregistrements
    /// (`MATCH ()-[r]->()`) reste juste. Ni `repair-degrees` (les degres
    /// sont recalcules depuis les records, deja justes) ni `compact` (ne
    /// touche pas les pointeurs de chaine) n'y remedient.
    ///
    /// Sans `--apply` : DIAGNOSTIC seul (aucune ecriture) — rapporte combien
    /// de pointeurs divergent de l'etat reconstruit (0 = sain). Idempotent ;
    /// requiert un acces exclusif (arreter les ecrivains d'abord).
    RepairEdgeChains {
        /// Path to the database
        path: PathBuf,

        /// Applique reellement la reconstruction (ecrit). Defaut : dry-run.
        #[arg(long)]
        apply: bool,
    },

    /// Ajoute du « relief » structurel à un store existant (post-import) :
    /// passes d'enrichissement additives et idempotentes. `--ricci` :
    /// courbure de Ricci-Ollivier (Jost-Liu) calculée et persistée sur toutes
    /// les arêtes vivantes (débloque node_curvature / signaux bottleneck).
    Enrich {
        /// Path to the database
        path: PathBuf,

        /// Persist the per-edge Ricci curvature (batch, full-graph).
        #[arg(long)]
        ricci: bool,
    },

    /// RÉSONANCE — le geste qui rend les FACETTES PUBLIQUES d'un self lisibles
    /// par un autre self, et qui se referme. Sans ce geste, rien n'est partagé :
    /// un partage activé par défaut serait une fuite qu'on n'a pas annoncée.
    ///
    /// Ce qui passe : les NOMS d'objectifs et de procédures, les mondes apportés,
    /// le volume de mémoire et la dernière activité. Ce qui ne passe JAMAIS : le
    /// texte libre des notes et l'état intime (`Note`, `EtatSelf`).
    Resonance {
        /// Le self dont on publie les facettes (celui qui ACCORDE le lien).
        #[arg(long)]
        publieur: String,

        /// Le self autorisé à les lire (celui qui REÇOIT le lien).
        #[arg(long)]
        lecteur: String,

        /// Referme le lien au lieu de l'établir.
        #[arg(long)]
        delier: bool,
    },

    /// SENTINELLE de continuité d'écriture (LECTURE) — « que ça n'arrive
    /// plus » (2026-09-05) : rapporte les sessions dont le dernier événement
    /// journal est plus vieux que `--stale-after-min`, et sort en code 1 si
    /// le store est FIGÉ (aucun événement écrit depuis N minutes — écrivain
    /// absent). Sortie JSON stable pour un timer ops.
    Continuity {
        /// Path to the database
        path: PathBuf,

        /// Alarme si aucun événement n'est écrit depuis N minutes.
        #[arg(long, default_value_t = 15)]
        stale_after_min: u64,
    },

    /// Diff structurel entre deux variantes d'un même dépôt indexé
    /// (branches / worktrees / écarts). Identité par (label, qualified_name).
    Compare {
        /// First store (baseline)
        path_a: PathBuf,
        /// Second store (variante)
        path_b: PathBuf,
        /// Max samples printed per direction
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Project-name prefix of store A to strip from qualified_names
        #[arg(long)]
        prefix_a: Option<String>,
        /// Project-name prefix of store B to strip from qualified_names
        #[arg(long)]
        prefix_b: Option<String>,
        /// Deep compare : attributs modifiés + échantillons (plus lent)
        #[arg(long)]
        attrs: bool,
    },

    /// Validate database integrity
    Validate {
        /// Path to the database
        path: PathBuf,
    },

    /// Manage indexes
    #[command(subcommand)]
    Index(IndexCommands),

    /// Manage backups
    #[command(subcommand)]
    Backup(BackupCommands),

    /// Export/import data
    #[command(subcommand)]
    Data(DataCommands),

    /// Manage Write-Ahead Log
    #[command(subcommand)]
    Wal(WalCommands),

    /// Simplicial topology passes (triangles, cliques, hyperedges)
    #[command(subcommand)]
    Topo(commands::topo::TopoCommands),

    /// Jump columns — the unified form of multi-hop connections
    #[command(subcommand)]
    Jump(commands::jump::JumpCommands),

    /// Session journal zones (migration MCP → substrate)
    #[command(subcommand)]
    Journal(commands::journal::JournalCommands),

    /// Couche de restitution visuelle : sous-graphes rendables + exports
    /// (json-graph v1 / DOT / Mermaid, overlays cognitifs)
    #[command(subcommand)]
    Viz(commands::viz::VizCommands),

    /// Administration des identités (comptes, rôles, politiques, jetons, RGPD)
    /// sur une base `.obrain`, HORS BANDE : le CLI agit sur un fichier.
    /// Sans la feature `iam`, la commande existe toujours et s'annonce
    /// incompilée au lieu de disparaître.
    #[command(subcommand)]
    Iam(commands::iam::IamCommands),

    /// Mémoire du chat : ce que la conversation a structuré (notes, objectifs,
    /// recettes, états du Self), en attente d'application dans la base du Self.
    Memoire(commands::memoire::MemoireArgs),

    /// La flotte : ce que le registre des machines connaît, et son inscription
    /// dans une base comme nœuds `machine:` — la machine comme fait du graphe.
    Machines(commands::machines::MachinesArgs),

    /// Entretien EN ATTENTE : les réparations attachées en vol pendant que les
    /// bases tournaient, à appliquer à la prochaine fenêtre d'exclusivité.
    Entretien(commands::entretien_cmd::EntretienArgs),

    /// Harness natif (T1) : daemon HTTP persistant servant les contrats du
    /// harness (health, sessions, injection) directement sur le store, sans
    /// respawn de bridge. Lecture seule — coexiste avec l ecrivain dsh.
    HarnessServe {
        /// Chemin du store
        path: std::path::PathBuf,
        /// Adresse d ecoute
        #[arg(long, default_value = "127.0.0.1:8090")]
        addr: String,
    },

    /// Journal d'import : manifest + diff → substrate.import-journal
    /// (RFC diff-incremental-variantes, étape 2 — captation).
    JournalImport {
        /// Store (répertoire substrate) fraîchement importé
        path: PathBuf,
        /// Préfixe projet CBM à retirer des qualified_names
        #[arg(long)]
        prefix: Option<String>,
    },

    /// MCP bridge server (JSON-RPC stdio) — the harness client speaks to obrain
    Mcp(mcp_server::McpArgs),

    /// Bridge mode (alias de `mcp`) — le GraphWriter/GraphInjector du harness
    /// lance `obrain bridge` (le protocole `codebase-memory-mcp bridge`).
    Bridge(mcp_server::McpArgs),

    /// Compact the database
    Compact {
        /// Path to the database
        path: PathBuf,

        /// Perform a dry-run (show what would be done)
        #[arg(long)]
        dry_run: bool,

        /// Prune old WAL files after compaction
        #[arg(long)]
        prune_wal: bool,
    },

    /// Ingestion native zone-direct (RFC §16.1) : importe un corpus binaire
    /// (format décrit dans `native_import`) dans un store neuf.
    NativeImport {
        /// Chemin du nouveau store substrate
        path: PathBuf,
        /// Fichier corpus binaire (généré par l'importeur)
        file: PathBuf,
    },

    /// Execute a query against a database
    Query {
        /// Path to the database
        path: PathBuf,

        /// GQL query string (or use --file / --stdin)
        query: Option<String>,

        /// Read query from a file
        #[arg(short, long)]
        file: Option<PathBuf>,

        /// Read query from stdin
        #[arg(long)]
        stdin: bool,

        /// Query parameter (key=value), can be repeated
        #[arg(short, long)]
        param: Vec<String>,

        /// Query language
        #[arg(short, long, default_value = "gql")]
        lang: QueryLanguage,

        /// Show query execution time
        #[arg(long)]
        timing: bool,

        /// Truncate cell values wider than N characters
        #[arg(long)]
        max_width: Option<usize>,

        /// Ouvrir la base en LECTURE SEULE (verrou partagé).
        ///
        /// POURQUOI CE DRAPEAU EXISTE. Un store tenu par un autre processus — le
        /// pont MCP du harness, un `viz serve` — refuse toute ouverture en
        /// écriture (« already open by another process »). Mesurer les requêtes
        /// RÉELLES sur le store VIVANT, et non sur une copie, exige donc une
        /// ouverture qui COEXISTE : c'est exactement ce que fait déjà le pont
        /// MCP (`ObrainDB::open_read_only`). Sans ce drapeau, le contexte le plus
        /// important de la campagne — le store en production — restait
        /// immesurable.
        #[arg(long)]
        read_only: bool,

        /// Imprime sur STDERR le nombre de slots-noeuds lus par la requete
        /// (compteur deterministe du substrat : la preuve de route indexee,
        /// insensible a la charge machine)
        #[arg(long)]
        slots: bool,
    },

    /// Create a new database
    Init {
        /// Path for the new database
        path: PathBuf,

        /// Graph model
        #[arg(long, default_value = "lpg")]
        mode: GraphMode,
    },

    /// Interactive query shell (REPL)
    Shell {
        /// Path to the database
        path: PathBuf,

        /// Query language
        #[arg(short, long, default_value = "gql")]
        lang: QueryLanguage,

        /// Show query execution time (toggle in shell with :timing)
        #[arg(long)]
        timing: bool,
    },

    /// Show version and build info
    Version,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

/// Index management commands.
#[derive(Subcommand)]
enum IndexCommands {
    /// List all indexes
    List {
        /// Path to the database
        path: PathBuf,
    },

    /// Show index statistics
    Stats {
        /// Path to the database
        path: PathBuf,
    },

    /// DECLARE (et construire) un index de propriété
    ///
    /// POURQUOI CE VERBE EXISTE. `create_property_index` n'était appelable que
    /// depuis l'API Rust : aucune commande, aucune DDL. Conséquence mesurée sur
    /// les stores réels — la série ORDONNÉE reste VIDE (`substrate.props.idx.v2`
    /// = 4 096 o sur runtime-dev) alors que la série d'égalité est bâtie
    /// (6,5 Mo) : l'égalité répond donc en **5-10 µs** et le moindre intervalle
    /// retombe sur un balayage de 348 000 nœuds (~70 ms). Un index qu'on ne peut
    /// pas déclarer ne sert à rien ; c'est ce verrou-là que ce verbe enlève.
    ///
    /// La déclaration est durable (`substrate.indexcat`) et la CONSTRUCTION de
    /// la série ordonnée se fait à la première requête par intervalle, puis est
    /// persistée au `flush` — donc cet appel n'impose pas le balayage de
    /// construction.
    Create {
        /// Path to the database
        path: PathBuf,

        /// Propriété à indexer (égalité + série ordonnée)
        property: String,
    },

    /// Rebatit depuis les chaines, puis persiste, les index de toutes les
    /// proprietes declarees au catalogue (egalite + trigrammes). L'acte de
    /// maintenance qui donne ses series a une zone d'index introduite
    /// apres l'indexation du store.
    Rebuild {
        /// Path to the database
        path: PathBuf,
        /// (NATIF-J6) Propriete a indexer par ORDRE de valeur, pour rendre
        /// `ORDER BY n.<prop> LIMIT k`, `min(n.<prop>)` et `max(n.<prop>)`
        /// instantanes. Repetable. Sans elle, seules les proprietes declarees
        /// au catalogue (egalite + trigrammes) sont rebaties.
        #[arg(long = "property", short = 'p')]
        property: Vec<String>,
    },
}

/// Backup commands.
#[derive(Subcommand)]
enum BackupCommands {
    /// Create a native backup
    Create {
        /// Path to the database
        path: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Restore from a native backup
    Restore {
        /// Path to the backup file
        backup: PathBuf,

        /// Target database path
        path: PathBuf,

        /// Overwrite if exists
        #[arg(long)]
        force: bool,
    },
}

/// Data export/import commands.
#[derive(Subcommand)]
enum DataCommands {
    /// Export data to a portable format
    Dump {
        /// Path to the database
        path: PathBuf,

        /// Output file or directory
        #[arg(short, long)]
        output: PathBuf,

        /// Export format (parquet, turtle, json)
        #[arg(long = "export-format")]
        export_format: Option<String>,
    },

    /// Import data from a dump
    Load {
        /// Path to the dump file/directory
        input: PathBuf,

        /// Target database path
        path: PathBuf,

        /// Ne PAS construire les index par defaut (qualified_name, name)
        /// en fin d'import — le store restaure sera nu, les seeks
        /// balaieront jusqu'a un `index rebuild`.
        #[arg(long = "no-index")]
        no_index: bool,
    },
}

/// WAL management commands.
#[derive(Subcommand)]
enum WalCommands {
    /// Show WAL status
    Status {
        /// Path to the database
        path: PathBuf,
    },

    /// Force a WAL checkpoint
    Checkpoint {
        /// Path to the database
        path: PathBuf,
    },
}

/// Query language selection.
#[derive(Clone, Copy, ValueEnum)]
enum QueryLanguage {
    /// GQL (ISO/IEC 39075:2024)
    Gql,
    /// Cypher (openCypher 9.0)
    Cypher,
    /// SPARQL (W3C 1.1)
    Sparql,
    /// SQL/PGQ (SQL:2023)
    Sql,
}

/// Graph model selection.
#[derive(Clone, Copy, ValueEnum)]
enum GraphMode {
    /// Labeled Property Graph
    Lpg,
    /// RDF Triple Store
    Rdf,
}

fn main() {
    let cli = Cli::parse();

    // Handle color flags.
    // SAFETY: called once at startup before any threads are spawned.
    #[allow(unsafe_code)]
    if cli.no_color {
        unsafe { std::env::set_var("NO_COLOR", "1") };
    } else if cli.color {
        unsafe { std::env::remove_var("NO_COLOR") };
    }

    // Set up logging based on verbosity. Le mode MCP parle JSON-RPC sur
    // stdout — tout log (WARN du replay substrate…) DOIT aller sur stderr,
    // sinon le client MCP lit une ligne polluée.
    let log_to_stderr = matches!(cli.command, Commands::Mcp(_));
    let mut builder = tracing_subscriber::fmt().with_writer(std::io::stderr);
    if cli.verbose {
        builder = builder.with_max_level(tracing::Level::DEBUG);
    } else if !cli.quiet || log_to_stderr {
        builder = builder.with_max_level(tracing::Level::WARN);
    } else {
        builder = builder.with_max_level(tracing::Level::ERROR);
    }
    builder.init();

    let result = match cli.command {
        Commands::Info { path } => commands::info::run(&path, cli.format, cli.quiet),
        Commands::Stats { path } => commands::stats::run(&path, cli.format, cli.quiet),
        Commands::Schema { path } => commands::schema::run(&path, cli.format, cli.quiet),
        Commands::Repair {
            path,
            dry_run,
            apply,
            verdict,
        } => {
            let _ = dry_run; // le dry-run est le defaut : `apply` seul decide
            commands::repair::run(&path, apply, verdict, cli.format, cli.quiet)
        }
        Commands::RepairDegrees { path } => {
            commands::repair::run_degrees(&path, cli.format, cli.quiet)
        }
        Commands::RepairProps {
            path,
            dry_run,
            apply,
        } => {
            let _ = dry_run; // le dry-run est le defaut : `apply` seul decide
            commands::repair::run_props(&path, apply, cli.format, cli.quiet)
        }
        Commands::RepairEdgeChains { path, apply } => {
            commands::repair::run_edge_chains(&path, apply, cli.format, cli.quiet)
        }
        Commands::Enrich { path, ricci } => {
            commands::enrich::run_enrich(&path, ricci, cli.format, cli.quiet)
        }
        Commands::Resonance { publieur, lecteur, delier } => {
            // Le geste vit dans `commands/resonance`, mais la règle
            // (« ce qui se partage », « ce qui ne sort jamais ») vit dans le
            // module `resonance`, avec ses tests : le CLI ne fait que la
            // déclencher et la DIRE.
            commands::resonance::run(&publieur, &lecteur, delier, cli.format, cli.quiet)
        }
        Commands::Continuity {
            path,
            stale_after_min,
        } => commands::continuity::run(&path, stale_after_min, cli.format, cli.quiet),
        Commands::Compare {
            path_a,
            path_b,
            limit,
            prefix_a,
            prefix_b,
            attrs,
        } => commands::compare::run_compare(
            &path_a, &path_b, limit, attrs, prefix_a, prefix_b, cli.format, cli.quiet,
        ),
        Commands::Validate { path } => commands::validate::run(&path, cli.format, cli.quiet),
        Commands::Index(cmd) => commands::index::run(cmd, cli.format, cli.quiet),
        Commands::Backup(cmd) => commands::backup::run(cmd, cli.format, cli.quiet),
        Commands::Data(cmd) => commands::data::run(cmd, cli.format, cli.quiet),
        Commands::Wal(cmd) => commands::wal::run(cmd, cli.format, cli.quiet),
        Commands::Topo(cmd) => commands::topo::run(cmd, cli.format, cli.quiet),
        Commands::Jump(cmd) => commands::jump::run(cmd, cli.format, cli.quiet),
        Commands::Journal(cmd) => commands::journal::run(cmd, cli.format, cli.quiet),
        Commands::HarnessServe { path, addr } => harness_serve::run(&addr, &path, cli.quiet),
        Commands::Viz(cmd) => commands::viz::run(cmd, cli.quiet),
        Commands::Iam(cmd) => commands::iam::run(cmd, cli.format),
        Commands::Memoire(args) => commands::memoire::run(args, cli.quiet),
        Commands::Machines(args) => commands::machines::run(args, cli.quiet),
        Commands::Entretien(args) => commands::entretien_cmd::run(args, cli.quiet),
        Commands::JournalImport { path, prefix } => {
            commands::import_journal::run_journal_import(&path, prefix.as_deref()).map(|j| {
                if !cli.quiet {
                    println!(
                        "journal-import: {} — +{} nœuds / -{} nœuds / +{} arêtes / -{} arêtes",
                        path.display(),
                        j.added_nodes.len(),
                        j.removed_nodes.len(),
                        j.added_edges.len(),
                        j.removed_edges.len()
                    );
                }
            })
        }
        Commands::Mcp(args) => mcp_server::run(args).map(|_| ()),
        Commands::Bridge(args) => mcp_server::run(args).map(|_| ()),
        Commands::Compact {
            path,
            dry_run,
            prune_wal,
        } => commands::compact::run(&path, dry_run, prune_wal, cli.format, cli.quiet),
        Commands::NativeImport { path, file } => commands::native_import::run(&path, &file),
        Commands::Query {
            path,
            query,
            file,
            stdin,
            param,
            lang,
            timing,
            max_width,
            read_only,
            slots,
        } => commands::query::run(commands::query::QueryRunOptions {
            path: &path,
            query,
            file,
            stdin,
            params: &param,
            read_only,
            lang,
            format: cli.format,
            quiet: cli.quiet,
            timing,
            max_width,
            slots,
        }),
        Commands::Init { path, mode } => commands::init::run(&path, mode, cli.format, cli.quiet),
        Commands::Shell { path, lang, timing } => {
            repl::run(&path, lang, cli.format, cli.quiet, timing)
        }
        Commands::Version => {
            commands::version::run(cli.quiet);
            Ok(())
        }
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "obrain", &mut std::io::stdout());
            Ok(())
        }
    };

    let code = match result {
        Ok(()) => ExitCode::Success,
        Err(e) => {
            if e.downcast_ref::<commands::validate::ValidationFailed>()
                .is_some()
            {
                // Validation already printed results, just set the exit code.
                ExitCode::ValidationFailed
            } else {
                output::error(&format!("{e:?}"));
                ExitCode::GeneralError
            }
        }
    };

    std::process::exit(code as i32);
}
