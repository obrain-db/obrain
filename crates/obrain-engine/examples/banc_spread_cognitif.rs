//! Banc A4 (plan 808fa9a7) : depuis des graines, compare trois façons de
//! proposer des voisins — l'activation propagée sur les synapses obrain
//! (`spread`), le voisinage structurel à 2 sauts, et un PageRank personnalisé
//! sur le graphe structurel — pour qu'une analyse externe les confronte à une
//! vérité terrain (ici : les synapses d'usage réel de PO).
//!
//! ```bash
//! cargo run --release --example banc_spread_cognitif --features cognitive -- \
//!     <base substrat> <graines.txt (un ext_id par ligne)> <répertoire de sortie> [--hops 2]
//! ```
//!
//! Sorties : `rangs.tsv` (méthode, graine, rang, nœud, score — 10 par
//! méthode et par graine, nœuds `Note` seulement) et `resume.tsv` (graine,
//! nœuds activés par spread, durée du spread en µs).
#![cfg(feature = "cognitive")]

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use obrain_cognitive::activation::{SpreadConfig, SynapseActivationSource, spread};
use obrain_common::types::{NodeId, PropertyKey};
use obrain_engine::ObrainDB;

const TOP: usize = 10;

fn top_notes(
    scores: &HashMap<NodeId, f64>,
    seed: NodeId,
    est_note: &HashSet<NodeId>,
) -> Vec<(NodeId, f64)> {
    let mut v: Vec<(NodeId, f64)> = scores
        .iter()
        .filter(|(n, s)| **n != seed && est_note.contains(n) && **s > 0.0)
        .map(|(n, s)| (*n, *s))
        .collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(TOP);
    v
}

fn main() {
    let mut it = std::env::args().skip(1);
    let base = PathBuf::from(
        it.next()
            .expect("usage: <base> <graines> <sortie> [--hops n]"),
    );
    let graines = PathBuf::from(it.next().expect("fichier de graines"));
    let sortie = PathBuf::from(it.next().expect("répertoire de sortie"));
    let mut hops: u32 = 2;
    while let Some(flag) = it.next() {
        if flag == "--hops" {
            hops = it.next().and_then(|s| s.parse().ok()).unwrap_or(2);
        }
    }
    std::fs::create_dir_all(&sortie).expect("sortie");

    let db = ObrainDB::open_substrate(&base).expect("open_substrate");
    let store = db.graph_store();

    // --- identités externes et étiquettes ---
    let cle = PropertyKey::new("ext_id");
    let mut ext_of: HashMap<NodeId, String> = HashMap::new();
    let mut id_of: HashMap<String, NodeId> = HashMap::new();
    let mut est_note: HashSet<NodeId> = HashSet::new();
    for n in store.all_nodes() {
        if let Some(ext) = n.properties.get(&cle).and_then(|v| v.as_str()) {
            ext_of.insert(n.id, ext.to_string());
            id_of.insert(ext.to_string(), n.id);
        }
        if n.labels.iter().any(|l| l.as_str() == "Note") {
            est_note.insert(n.id);
        }
    }

    // --- adjacence structurelle non orientée, sans les arêtes SYNAPSE ---
    // (`all_edges()` rend vide sur substrat : on passe par `edges_from`.)
    let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut n_struct = 0usize;
    for n in ext_of.keys() {
        for (voisin, eid) in store.edges_from(*n, obrain_core::graph::Direction::Outgoing) {
            let Some(e) = store.get_edge(eid) else {
                continue;
            };
            if e.edge_type.as_str() == obrain_cognitive::synapse::SYNAPSE_EDGE_TYPE {
                continue;
            }
            adj.entry(*n).or_default().push(voisin);
            adj.entry(voisin).or_default().push(*n);
            n_struct += 1;
        }
    }
    let ids: Vec<NodeId> = ext_of.keys().copied().collect();
    let idx: HashMap<NodeId, usize> = ids.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    println!(
        "base : {} nœuds ({} notes), {} arêtes structurelles, {} synapses obrain",
        ids.len(),
        est_note.len(),
        n_struct,
        db.cognitive_engine()
            .and_then(|e| e.synapse_store().map(|s| s.len()))
            .unwrap_or(0)
    );

    let engine = db.cognitive_engine().expect("moteur cognitif");
    let synapses = Arc::clone(engine.synapse_store().expect("synapse store"));
    let source = SynapseActivationSource::new(synapses);
    let cfg = SpreadConfig {
        max_hops: hops,
        max_activated_nodes: 0,
        ..Default::default()
    };

    let mut rangs = std::fs::File::create(sortie.join("rangs.tsv")).expect("rangs.tsv");
    let mut resume = std::fs::File::create(sortie.join("resume.tsv")).expect("resume.tsv");
    writeln!(resume, "graine\tspread_actives\tspread_us\tppr_us").ok();

    let liste = std::fs::read_to_string(&graines).expect("graines");
    let mut n_graines = 0usize;
    for ext in liste.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some(&seed) = id_of.get(ext) else {
            eprintln!("graine inconnue : {ext}");
            continue;
        };
        n_graines += 1;

        // 1. spread sur les synapses obrain
        let t = Instant::now();
        let act = spread(&[(seed, 1.0)], &source, &cfg);
        let spread_us = t.elapsed().as_micros();
        let n_act = act.len();
        for (r, (n, s)) in top_notes(&act, seed, &est_note).iter().enumerate() {
            writeln!(rangs, "spread\t{ext}\t{}\t{}\t{s}", r + 1, ext_of[n]).ok();
        }

        // 2. voisinage structurel à 2 sauts : 1 par voisin direct, 1/deg(x)
        //    par chemin seed–x–y (les hubs — fichiers, projets — pèsent peu)
        let mut vois: HashMap<NodeId, f64> = HashMap::new();
        if let Some(d1) = adj.get(&seed) {
            for x in d1 {
                *vois.entry(*x).or_default() += 1.0;
                if let Some(d2) = adj.get(x) {
                    let p = 1.0 / d2.len() as f64;
                    for y in d2 {
                        if *y != seed {
                            *vois.entry(*y).or_default() += p;
                        }
                    }
                }
            }
        }
        for (r, (n, s)) in top_notes(&vois, seed, &est_note).iter().enumerate() {
            writeln!(rangs, "voisinage\t{ext}\t{}\t{}\t{s}", r + 1, ext_of[n]).ok();
        }

        // 3. PageRank personnalisé (α = 0,15, 20 itérations) sur le graphe
        //    structurel non orienté
        let t = Instant::now();
        let n = ids.len();
        let s_idx = idx[&seed];
        let mut p = vec![0.0f64; n];
        p[s_idx] = 1.0;
        for _ in 0..20 {
            let mut q = vec![0.0f64; n];
            for (u, pu) in p.iter().enumerate() {
                if *pu == 0.0 {
                    continue;
                }
                if let Some(vs) = adj.get(&ids[u]) {
                    let part = 0.85 * pu / vs.len() as f64;
                    for v in vs {
                        q[idx[v]] += part;
                    }
                } else {
                    q[s_idx] += 0.85 * pu;
                }
            }
            q[s_idx] += 0.15;
            p = q;
        }
        let ppr_us = t.elapsed().as_micros();
        let ppr: HashMap<NodeId, f64> = p
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0.0)
            .map(|(i, v)| (ids[i], *v))
            .collect();
        for (r, (n, s)) in top_notes(&ppr, seed, &est_note).iter().enumerate() {
            writeln!(rangs, "ppr\t{ext}\t{}\t{}\t{s}", r + 1, ext_of[n]).ok();
        }

        writeln!(resume, "{ext}\t{n_act}\t{spread_us}\t{ppr_us}").ok();
    }
    println!(
        "{n_graines} graines traitées, sorties dans {}",
        sortie.display()
    );
}
