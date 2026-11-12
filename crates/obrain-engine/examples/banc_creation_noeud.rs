//! Banc de `Session::create_node_with_props` / `ObrainDB::create_node_with_props`
//! (obrain#120 pt 4) : coût d'écriture par création, en µs.
//!
//! Usage : `banc_creation_noeud <répertoire> [creations=3000] [chemin=session|db]`
//! Affiche une ligne `p50_us=… p90_us=… moy_us=… n=…`. Pour comparer avant/après,
//! compiler deux binaires (avant, après) et les alterner A B A B.
//! 3 propriétés courtes + 1 longue (585 o) par nœud ; chaque appel est chronométré.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("répertoire");
    let n: usize = a.next().map_or(3000, |s| s.parse().unwrap());
    let chemin = a.next().unwrap_or_else(|| "session".into());
    let db = ObrainDB::open(&dir).expect("open");
    let session = db.session();
    let longue = "x".repeat(585);
    let mut us = Vec::with_capacity(n);
    for i in 0..n {
        let props = [
            ("a", Value::String(format!("court-{i}").into())),
            ("b", Value::Int64(i as i64)),
            ("c", Value::String("constante".into())),
            ("d", Value::String(longue.clone().into())),
        ];
        let t = Instant::now();
        if chemin == "db" {
            db.create_node_with_props(&["Banc"], props);
        } else {
            session.create_node_with_props(&["Banc"], props);
        }
        us.push(t.elapsed().as_nanos() as f64 / 1000.0);
    }
    us.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let p = |q: f64| us[((us.len() - 1) as f64 * q) as usize];
    println!(
        "p50_us={:.1} p90_us={:.1} moy_us={:.1} n={}",
        p(0.5),
        p(0.9),
        us.iter().sum::<f64>() / us.len() as f64,
        us.len()
    );
}
