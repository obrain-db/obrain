//! One-shot : corrige `name` des nœuds File du graphe (défaut d'indexation
//! historique — voir crates/obrain-indexer/src/index.rs, 2026-09-09).
//!
//! Contexte : le qn d'un fichier finit par la sentinelle d'ancrage
//! `.__file__` (convention héritée du contrat C). L'ancien indexeur prenait
//! le dernier segment du qn comme `name` → CHAQUE nœud File s'appelait
//! `__file__` (14 449 nœuds sur les stores runtime). L'indexeur est corrigé
//! (name = basename réel) ; cet outil répare les nœuds DÉJÀ persistés, sans
//! réindexer (aucun doublon) : `name := basename(file_path)` pour tout nœud
//! File dont `name == "__file__"` avec un `file_path` valide. Idempotent.
//! Le qn (ancrage `.__file__`) n'est PAS modifié.
//!
//! Usage :
//!   cargo run --release -p obrain-engine --example fix_file_names -- <store>          # dry-run
//!   cargo run --release -p obrain-engine --example fix_file_names -- <store> --apply  # écrit + checkpoint
//!
//! Fenêtre de maintenance requise pour un store vivant (même procédure que
//! scripts/index-code-projects.sh : un SEUL writer à la fois — stopper les
//! webs avant `--apply`, redémarrer après).
use obrain_common::types::Value;
use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};

fn main() {
    let mut a = std::env::args().skip(1);
    let Some(store) = a.next() else {
        eprintln!("usage: fix_file_names <store.obrain> [--apply]");
        std::process::exit(2);
    };
    let apply = a.next().as_deref() == Some("--apply");

    let db = obrain_engine::ObrainDB::open_substrate(&store)
        .unwrap_or_else(|e| panic!("ouverture {store} : {e}"));
    let sub = db
        .substrate_handle()
        .expect("store non substrate — nœuds File absents de l'API moteur");

    let ids = sub.nodes_by_label("File");
    let total = ids.len();
    let mut fixes = 0u64;
    let mut no_path = 0u64;
    let mut samples: Vec<(String, String)> = Vec::new();
    let prop_path = obrain_common::types::PropertyKey::new("file_path");
    let prop_name = obrain_common::types::PropertyKey::new("name");
    for id in ids {
        let fp = match sub.get_node_property(id, &prop_path) {
            Some(Value::String(s)) => s.to_string(),
            _ => {
                no_path += 1;
                continue;
            }
        };
        // Ne corrige que la signature fautive ; les noms déjà corrects sont
        // laissés intacts (idempotent, non destructif).
        let name_is_file = matches!(
            sub.get_node_property(id, &prop_name),
            Some(Value::String(s)) if s.as_str() == "__file__"
        );
        if !name_is_file {
            continue;
        }
        let base = std::path::Path::new(&fp)
            .file_name()
            .map(|b| b.to_string_lossy().to_string())
            .unwrap_or_default();
        if base.is_empty() || base == "__file__" {
            continue;
        }
        fixes += 1;
        if samples.len() < 5 {
            samples.push((fp.clone(), base.clone()));
        }
        if apply {
            sub.set_node_property(id, "name", Value::String(base.into()));
        }
    }
    if apply {
        let _ = sub.checkpoint();
    }
    println!(
        "=== fix_file_names {} (mode: {}) ===",
        store,
        if apply { "APPLY" } else { "dry-run" }
    );
    println!("nœuds File scannés : {total}");
    println!("corrections nécessaires (name=__file__ → basename) : {fixes}");
    println!("File sans file_path (non corrigibles, à signaler) : {no_path}");
    for (fp, base) in samples {
        println!("  ex. {fp} → {base}");
    }
    if apply {
        println!("APPLY : écritures + checkpoint effectués.");
    } else {
        println!("dry-run : aucune écriture. Relancer avec --apply pour corriger.");
    }
}
