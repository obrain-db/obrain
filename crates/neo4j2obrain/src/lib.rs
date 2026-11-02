//! # neo4j2obrain — importeur Neo4j → substrat obrain, **en bibliothèque**
//!
//! ## Pourquoi cette cible `lib` existe (2026-08-18)
//!
//! Ce crate n'exposait qu'un `[[bin]]` : toute sa logique — le pipeline en
//! huit étapes, le lecteur bolt, l'initialisation cognitive — était enfermée
//! dans `main.rs` et donc inatteignable depuis un autre programme. Un
//! migrateur externe ne pouvait que ré-exécuter le binaire et lire son code
//! de sortie ; il ne pouvait ni composer les étapes, ni s'insérer entre
//! elles, ni rapporter la progression.
//!
//! Deux besoins concrets l'ont demandé :
//!
//! 1. Un migrateur du **project-orchestrator** (Neo4j + Meilisearch → obrain)
//!    qui doit fusionner deux sources AVANT les étapes cognitives 2 à 10,
//!    pour que communautés, centralité et embeddings couvrent l'ensemble et
//!    non la moitié. Impossible en enchaînant deux binaires.
//! 2. L'intégration future dans le PO lui-même, pour que ses utilisateurs
//!    migrent depuis l'interface — ce qui suppose d'appeler le pipeline
//!    depuis un serveur, pas de lancer un processus.
//!
//! Le binaire subsiste et ne change pas de comportement : il devient une
//! CLI mince par-dessus cette bibliothèque.
//!
//! ## Drapeaux de compilation — À LIRE AVANT UN IMPORT RÉEL
//!
//! Deux features indispensables ne sont **pas** dans `default` :
//!
//! | Feature          | Sans elle                                              |
//! |------------------|--------------------------------------------------------|
//! | `neo4j-bolt`     | l'étape 1 n'existe pas — le binaire NE LIT PAS Neo4j    |
//! | `embed`          | l'étape 2 n'existe pas — aucun embedding calculé        |
//! | `cognitive-init` | (par défaut) sinon étape 4 en no-op averti, communautés vides |
//!
//! Un `cargo build -p neo4j2obrain` nu produit donc un outil qui ne sait
//! qu'enrichir un substrat déjà peuplé, en silence. Pour un import de
//! production : `--features neo4j-bolt,embed`.
//!
//! ## Version de format
//!
//! Les dépendances internes sont déclarées `workspace = true`, donc ce crate
//! écrit avec le `obrain-substrate` du workspace — `FORMAT_VERSION = 7`. Il
//! n'y a pas de version à choisir : le substrat produit est en v7 par
//! construction.

#[cfg(feature = "neo4j-bolt")]
pub mod bolt_reader;
pub mod cognitive_init;
#[cfg(feature = "embed")]
pub mod embedder;
pub mod pipeline;
#[cfg(feature = "embed")]
pub mod sentence_transformer;

// Réexports de confort : l'appelant type n'a besoin que de ces trois-là
// (c'est exactement ce que la CLI utilise).
pub use pipeline::{DEFAULT_TEXT_KEYS, PipelineOptions, run};

/// ── (2026-08-19) CE QUE `main()` FAISAIT AUTOUR DES MODULES ──
///
/// L extraction de la bibliotheque (756ae523) a deplace les `mod`, mais a
/// laisse dans le binaire tout ce que `main()` faisait AVANT d appeler
/// `run()` : l installation du souscripteur `tracing` et la configuration du
/// pool rayon. Consequence : tout appelant de la bibliotheque perdait les
/// deux, EN SILENCE.
///
/// Ce n est pas une hypothese, c est mesure. Le 18/08 au soir, un import de
/// `po2obrain` a tourne 2 h 30 sans emettre UNE SEULE ligne malgre
/// `RUST_LOG=info` pose dans l environnement — parce que `RUST_LOG` est un
/// filtre lu par un souscripteur, pas un interrupteur, et qu aucun
/// souscripteur n etait installe. On a suivi l avancement en regardant
/// grossir un repertoire.
///
/// C est la troisieme occurrence de la meme classe en deux jours (features
/// `neo4j-bolt` / `embed` absentes par defaut, pool rayon, souscripteur) :
/// une capacite cablee dans la CLI et absente du chemin bibliotheque. Le PO
/// tombera dedans le jour ou il appellera `run()` depuis son interface.
///
/// D ou cette fonction : UN point d entree que tout appelant invoque, la CLI
/// comprise. Elle est idempotente — `try_init` plutot que `init`, parce qu une
/// bibliotheque ne doit pas paniquer si l hote a deja pose son propre
/// souscripteur, et le pool global rayon ignore silencieusement une seconde
/// initialisation.
///
/// `workers = 0` laisse le defaut de rayon (nombre de coeurs physiques).
/// Le reglage compte : chaque worker de l etape 2 charge SA session ONNX
/// (`intra_threads = 1`, ~23 Mo), donc N workers = N threads reels.
pub fn init_runtime(workers: usize, verbeux: bool) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(if verbeux {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .with_target(false)
        .try_init();

    if workers > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build_global();
    }
}
