//! Mesure des compteurs de cardinalité (issue #254) sur une base réelle.
//!
//!   cargo run --release -p obrain-bases-banc --example mesure_compteurs -- <chemin absolu>
//!
//! La base désignée n'est JAMAIS ouverte : elle est copiée (clonée en
//! copy-on-write quand le système de fichiers le permet) par `copie_de`, la
//! copie est ouverte EN LECTURE SEULE, et supprimée à la sortie du processus.
//!
//! Mesure, après un tour d'échauffement (qui déclenche la vérification
//! d'ouverture, UNE fois par session) :
//!   · `edge_counts_by_type()`       — la source de `count(DISTINCT type(r))`
//!   · `node_counts_by_first_label()` — la source de `count(DISTINCT labels(n)[0])`
//!   · `node_count()` / `edge_count()` — les compteurs déjà O(1), témoins.

use std::path::PathBuf;
use std::time::Instant;

use obrain_bases_banc::copie_de;
use obrain_core::graph::traits::GraphStore;
use obrain_substrate::SubstrateStore;

fn main() {
    let chemin = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage : mesure_compteurs <chemin absolu d'une base>");

    // Copie privée (clone COW), supprimée à la sortie — voir `copie_de`.
    let copie = copie_de(&chemin, "mesure_compteurs");
    let t_open = Instant::now();
    let s = SubstrateStore::open_read_only(copie.chemin()).expect("ouverture en lecture seule");
    let open_ms = t_open.elapsed().as_secs_f64() * 1e3;

    // Échauffement : le premier lecteur déclenche la vérification des
    // compteurs (balayage d'ouverture, UNE fois par session) — hors mesure.
    let _ = s.edge_counts_by_type();
    let _ = s.node_counts_by_first_label();

    let mut par_type = None;
    let mut par_label = None;

    let n = 20usize;
    let t = Instant::now();
    for _ in 0..n {
        par_type = s.edge_counts_by_type();
    }
    let d_type_ns = t.elapsed().as_nanos() as f64 / n as f64;

    let t = Instant::now();
    for _ in 0..n {
        par_label = s.node_counts_by_first_label();
    }
    let d_label_ns = t.elapsed().as_nanos() as f64 / n as f64;

    let t = Instant::now();
    let nodes = s.node_count();
    let edges = s.edge_count();
    let d_count_ns = t.elapsed().as_nanos() as f64 / 1.0;

    let types = par_type.as_ref().map_or(0, |m| m.len());
    let labels = par_label.as_ref().map_or(0, |v| v.len());
    let somme_types = par_type.as_ref().map_or(0, |m| m.values().sum::<usize>());
    let somme_labels = par_label
        .as_ref()
        .map_or(0, |v| v.iter().map(|(_, c)| c).sum::<usize>());

    println!("ouverture (lecture seule) : {open_ms:.1} ms");
    println!("nodes={nodes}  edges={edges}");
    println!(
        "edge_counts_by_type()      : {d_type_ns:.0} ns/appel  ({} types, somme={somme_types})",
        types
    );
    println!(
        "node_counts_by_first_label(): {d_label_ns:.0} ns/appel  ({} groupes, somme={somme_labels})",
        labels
    );
    println!("node_count()+edge_count()  : {d_count_ns:.0} ns (témoin O(1))");
}
