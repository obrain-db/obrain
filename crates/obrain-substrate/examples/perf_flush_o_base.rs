//! obrain#120 point 3 — le flush complet est O(base) ?
//!
//! Mesure le coût d'un `flush()` sur une base de N nœuds (propriété courte
//! chacun) + N-1 arêtes, en format hérité (`OBRAIN_PROPS_V2=0`) et neuf.
//!
//! ```text
//! cargo run --release -p obrain-substrate --example perf_flush_o_base -- <N> <herite|neuf>
//! ```
//!
//! Mesure, pour la base :
//!   - `flush_propre` : un flush SANS aucune écriture entre deux flushes
//!     (isole le coût fixe du flush : sidecar réécrit en entier, msync des
//!     zones, dict).
//!   - `flush_blob` : un flush après UNE écriture longue (> 256 o, routée en
//!     colonne blob) — le scénario « toute écriture > 256 o paie ce flush ».
//!
//! Sortie : une ligne `mesure<TAB>valeur<TAB>unité` par mesure (médiane sur
//! K échantillons).

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use web_time::Instant;

fn quantile_ms(mut v: Vec<f64>, q: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * q).round() as usize]
}

fn dire(nom: &str, valeur: f64, unite: &str) {
    println!("{nom}\t{valeur:.2}\t{unite}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (n, format) = match args.as_slice() {
        [n, f] => (n.parse::<u64>().expect("N"), f.clone()),
        _ => {
            eprintln!("usage : perf_flush_o_base <N> <herite|neuf>");
            std::process::exit(2);
        }
    };
    if format == "herite" {
        unsafe { std::env::set_var("OBRAIN_PROPS_V2", "0") };
    } else if format != "neuf" {
        eprintln!("format inconnu : {format} (attendu herite|neuf)");
        std::process::exit(2);
    }

    let dir = tempfile::Builder::new()
        .prefix("obrain-flush-o-base-")
        .tempdir()
        .expect("tempdir");
    let chemin = dir.path().join("kb");

    // 1. Fabriquer la base : N nœuds (propriété courte) + N-1 arêtes.
    let t = Instant::now();
    let s = SubstrateStore::create(&chemin).expect("création");
    let mut precedent: Option<NodeId> = None;
    for i in 0..n {
        let id = s.create_node(&["File"]);
        s.set_node_property(id, "i", Value::Int64(i64::try_from(i).unwrap_or(i64::MAX)));
        if let Some(p) = precedent {
            s.create_edge(p, id, "NEXT");
        }
        precedent = Some(id);
    }
    s.flush().expect("flush de construction");
    drop(s);
    let remplissage = t.elapsed().as_secs_f64();
    dire("remplissage", remplissage, "s");

    // 2. Rouvrir et mesurer le flush PROPRE (aucune écriture entre flushes).
    let s = SubstrateStore::open(&chemin).expect("ouverture");
    // Échauffement : le premier flush après l'ouverture réécrit toujours le
    // sidecar (génération sentinelle). On le paie une fois ici, hors mesure,
    // pour ne mesurer que le RÉGIME ÉTABLI.
    s.flush().expect("flush d'échauffement");
    let k = 5;
    let mut propres = Vec::with_capacity(k);
    for _ in 0..k {
        let t = Instant::now();
        s.flush().expect("flush propre");
        propres.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    dire(
        "flush_propre_p50_ms",
        quantile_ms(propres.clone(), 0.5),
        "ms",
    );
    dire("flush_propre_max_ms", quantile_ms(propres, 1.0), "ms");

    // 3. Flush après UNE écriture longue (> 256 o, colonne blob).
    let premier = NodeId(1); // le premier nœud alloué
    let mut blobs = Vec::with_capacity(k);
    for i in 0..k {
        let longue = format!("{i:08}").repeat(73) + "x"; // 585 o, au-dessus du seuil
        s.set_node_property(premier, "doc", Value::String(longue.into()));
        let t = Instant::now();
        s.flush().expect("flush après blob");
        blobs.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    dire("flush_blob_p50_ms", quantile_ms(blobs.clone(), 0.5), "ms");
    dire("flush_blob_max_ms", quantile_ms(blobs, 1.0), "ms");

    // 4. Sanity : la valeur longue est relue correctement.
    let relue = s.get_node_property(premier, &obrain_common::types::PropertyKey::new("doc"));
    let ok = matches!(relue, Some(Value::String(ref v)) if v.len() == 585);
    dire("blob_relue_585o", if ok { 1.0 } else { 0.0 }, "0/1");

    drop(s);
}
