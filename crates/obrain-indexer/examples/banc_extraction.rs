//! T27 — où part le temps de l'extraction ? Analyse tree-sitter SEULE contre
//! extraction complète, sur les mêmes fichiers, en parallèle.
use rayon::prelude::*;
use std::time::Instant;

fn main() {
    let racine = std::env::args()
        .nth(1)
        .expect("usage: banc_extraction <depot>");
    let racine = std::path::Path::new(&racine);
    let disco = obrain_indexer::discovery::discover(racine, &Default::default());
    let fichiers: Vec<_> = disco
        .files
        .iter()
        .filter(|f| matches!(f.extension().and_then(|e| e.to_str()), Some("c" | "h")))
        .cloned()
        .collect();
    let t = Instant::now();
    let paires: Vec<(std::path::PathBuf, String)> = fichiers
        .par_iter()
        .filter_map(|f| std::fs::read_to_string(f).ok().map(|s| (f.clone(), s)))
        .collect();
    let sources: Vec<&str> = paires.iter().map(|(_, s)| s.as_str()).collect();
    let octets: usize = sources.iter().map(|s| s.len()).sum();
    println!(
        "lecture : {} fichiers C, {:.2} Mo, {:.2}s",
        sources.len(),
        octets as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    let noeuds: usize = sources.par_iter().map(|s| {
        thread_local! { static P: std::cell::RefCell<tree_sitter::Parser> = std::cell::RefCell::new({
            let mut p = tree_sitter::Parser::new();
            p.set_language(&tree_sitter_c::LANGUAGE.into()).unwrap();
            p
        }); }
        P.with(|p| p.borrow_mut().parse(s, None).map(|t| t.root_node().descendant_count()).unwrap_or(0))
    }).sum();
    println!(
        "analyse tree-sitter seule : {:.2}s ({noeuds} nœuds syntaxiques)",
        t.elapsed().as_secs_f64()
    );

    let registre = obrain_indexer::lang::build_registry();
    let t = Instant::now();
    let defs: usize = paires
        .par_iter()
        .map(|(f, s)| {
            registre
                .language_for_path(f)
                .map(|l| l.extract(s, f).definitions.len())
                .unwrap_or(0)
        })
        .sum();
    println!(
        "extraction complète : {:.2}s ({defs} définitions)",
        t.elapsed().as_secs_f64()
    );
}
