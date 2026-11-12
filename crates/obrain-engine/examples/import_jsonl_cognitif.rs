//! Importe un graphe JSONL par l'API publique et mesure ce que la couche
//! cognitive en fait (énergie, synapses) — sans aucun câblage manuel.
//!
//! Format d'entrée (un répertoire) :
//! - `nodes.jsonl` : `{"id": "<externe>", "labels": ["A"], "props": {"k": v}}`
//! - `edges.jsonl` : `{"src": "<externe>", "dst": "<externe>", "type": "T", "props": {}}`
//!
//! ```bash
//! cargo run --release --example import_jsonl_cognitif --features cognitive -- \
//!     <répertoire> [--db <chemin>] [--lot 1000] [--dump <répertoire de sortie>]
//! ```
//!
//! `--dump` écrit `energy.tsv` (id externe, énergie, degré sortant, degré
//! entrant) et `synapses.tsv` (src externe, dst externe, poids) pour analyse.
//!
//! Les valeurs JSON sont converties ainsi : nombre entier → `Int64`, nombre à
//! virgule → `Float64`, booléen → `Bool`, chaîne → `String` ; le reste est
//! ignoré. L'identifiant externe est conservé dans la propriété `ext_id`.
#![cfg(feature = "cognitive")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

fn valeur(v: &serde_json::Value) -> Option<Value> {
    match v {
        serde_json::Value::Bool(b) => Some(Value::Bool(*b)),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Int64)
            .or_else(|| n.as_f64().map(Value::Float64)),
        serde_json::Value::String(s) => Some(Value::String(s.as_str().into())),
        _ => None,
    }
}

/// RSS courant du processus en Mo, par `ps` (portable macOS/Linux, sans
/// `unsafe` — le dépôt interdit `unsafe_code`). NaN si `ps` manque.
fn rss_mo() -> f64 {
    let pid = std::process::id().to_string();
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(f64::NAN, |kio| kio / 1024.0)
}

fn lignes(path: &Path) -> impl Iterator<Item = serde_json::Value> {
    let f = std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| {
            if l.trim().is_empty() {
                None
            } else {
                Some(serde_json::from_str(&l).expect("ligne JSON"))
            }
        })
}

struct Args {
    dir: PathBuf,
    db: Option<PathBuf>,
    lot: usize,
    dump: Option<PathBuf>,
}

fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let dir = PathBuf::from(
        it.next()
            .expect("usage: <répertoire> [--db <chemin>] [--lot n] [--dump <dir>]"),
    );
    let mut a = Args {
        dir,
        db: None,
        lot: 1000,
        dump: None,
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--db" => a.db = it.next().map(PathBuf::from),
            "--lot" => a.lot = it.next().and_then(|s| s.parse().ok()).unwrap_or(1000),
            "--dump" => a.dump = it.next().map(PathBuf::from),
            autre => panic!("option inconnue : {autre}"),
        }
    }
    a
}

fn main() {
    let a = args();
    let tmp = tempfile::tempdir().expect("tempdir");
    let chemin = a.db.clone().unwrap_or_else(|| tmp.path().join("base"));
    let db = ObrainDB::open_substrate(&chemin).expect("open_substrate");
    let store = db.graph_store_mut();

    // --- Nœuds ---
    let t0 = Instant::now();
    let mut ids: HashMap<String, NodeId> = HashMap::new();
    let mut n_nodes = 0usize;
    for n in lignes(&a.dir.join("nodes.jsonl")) {
        let ext = n["id"].as_str().expect("id").to_string();
        let labels: Vec<&str> = n["labels"]
            .as_array()
            .map(|l| l.iter().filter_map(|x| x.as_str()).collect())
            .unwrap_or_default();
        let id = store.create_node(&labels);
        store.set_node_property(id, "ext_id", Value::String(ext.as_str().into()));
        if let Some(props) = n["props"].as_object() {
            for (k, v) in props {
                if let Some(val) = valeur(v) {
                    store.set_node_property(id, k, val);
                }
            }
        }
        ids.insert(ext, id);
        n_nodes += 1;
        if n_nodes.is_multiple_of(a.lot) {
            print!("\rnœuds : {n_nodes}");
            let _ = std::io::stdout().flush();
        }
    }
    let t_nodes = t0.elapsed();
    println!("\rnœuds : {n_nodes} en {t_nodes:.1?}");

    // --- Arêtes ---
    let t1 = Instant::now();
    let mut n_edges = 0usize;
    let mut n_orphelines = 0usize;
    for e in lignes(&a.dir.join("edges.jsonl")) {
        let (Some(s), Some(d)) = (
            e["src"].as_str().and_then(|k| ids.get(k)),
            e["dst"].as_str().and_then(|k| ids.get(k)),
        ) else {
            n_orphelines += 1;
            continue;
        };
        let t = e["type"].as_str().unwrap_or("REL");
        let eid = store.create_edge(*s, *d, t);
        if let Some(props) = e["props"].as_object() {
            for (k, v) in props {
                if let Some(val) = valeur(v) {
                    store.set_edge_property(eid, k, val);
                }
            }
        }
        n_edges += 1;
        if n_edges.is_multiple_of(a.lot) {
            print!("\rarêtes : {n_edges}");
            let _ = std::io::stdout().flush();
        }
    }
    let t_edges = t1.elapsed();
    println!("\rarêtes : {n_edges} en {t_edges:.1?} ({n_orphelines} orphelines ignorées)");

    // --- Quiescence cognitive ---
    let t2 = Instant::now();
    let a_jour = db.cognitive_quiesce(Duration::from_secs(600));
    println!(
        "cognition : publiés={} dispatchés={} perdus={} à jour={a_jour} en {:.1?}",
        db.cognitive_events_published(),
        db.cognitive_events_dispatched(),
        db.cognitive_events_lost(),
        t2.elapsed()
    );

    let engine = db.cognitive_engine().expect("moteur cognitif");
    let energy = engine.energy_store().expect("energy store");
    let synapses = engine.synapse_store().expect("synapse store");
    let snap = energy.snapshot();
    let mut valeurs: Vec<f64> = snap.iter().map(|(_, e)| *e).collect();
    valeurs.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let q = |p: f64| {
        valeurs
            .get(((valeurs.len() as f64 - 1.0) * p) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "énergie : {} nœuds énergisés / {} ; min={:.3} p50={:.3} p90={:.3} max={:.3}",
        snap.len(),
        store.node_count(),
        q(0.0),
        q(0.5),
        q(0.9),
        q(1.0)
    );
    println!(
        "synapses : {} (arêtes importées : {n_edges})",
        synapses.len()
    );
    println!("RSS : {:.0} Mo", rss_mo());
    println!(
        "total : {:.1?} (nœuds {:.1?}, arêtes {:.1?}, quiescence {:.1?})",
        t0.elapsed(),
        t_nodes,
        t_edges,
        t2.elapsed()
    );

    if let Some(dump) = a.dump {
        std::fs::create_dir_all(&dump).expect("dump dir");
        let inverse: HashMap<NodeId, &str> = ids.iter().map(|(k, v)| (*v, k.as_str())).collect();
        let mut f = std::fs::File::create(dump.join("energy.tsv")).expect("energy.tsv");
        for (id, e) in &snap {
            if let Some(ext) = inverse.get(id) {
                writeln!(
                    f,
                    "{ext}\t{e}\t{}\t{}",
                    store.out_degree(*id),
                    store.in_degree(*id)
                )
                .ok();
            }
        }
        let mut g = std::fs::File::create(dump.join("synapses.tsv")).expect("synapses.tsv");
        for s in synapses.snapshot() {
            if let (Some(a), Some(b)) = (inverse.get(&s.source), inverse.get(&s.target)) {
                writeln!(g, "{a}\t{b}\t{}", s.current_weight()).ok();
            }
        }
        println!("dump écrit dans {}", dump.display());
    }
}
