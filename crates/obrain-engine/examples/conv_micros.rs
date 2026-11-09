//! Bench µs de l'API substrat reverse-PART_OF sur une COPIE de base dev,
//! avec exactitude croisée contre le scan GQL (mêmes comptes).
//! Usage : conv_micros <base_dir>
use obrain_engine::ObrainDB;
use obrain_substrate::SubstrateStore;
use web_time::Instant;

fn main() {
    let base = std::env::args()
        .nth(1)
        .expect("usage: conv_micros <base_dir>");
    let s = SubstrateStore::open(std::path::Path::new(&base)).expect("open substrate copy");
    let top = s
        .sessions_part_of()
        .into_iter()
        .map(|k| (k, s.count_messages_de_session(k).unwrap_or(0)))
        .max_by_key(|(_, n)| *n)
        .unwrap();
    println!(
        "session top (slot {}) : {} messages via API O(1)",
        top.0, top.1
    );
    // 1) exactitude croisée : count API == count GQL (scan) pour la même session
    let gql = {
        let db = ObrainDB::open_substrate(&base).expect("open gql");
        let q = format!(
            "MATCH ()-[r:PART_OF]->(x:Session) WHERE id(x) = {} RETURN count(r)",
            top.0
        );
        let r = db.session().execute(&q).expect("scan gql");
        r.rows.first().and_then(|row| row.first()).map_or(-1, |v| {
            if let obrain_common::types::Value::Int64(n) = v {
                *n
            } else {
                -1
            }
        })
    };
    println!(
        "count GQL (scan, toutes sources) = {gql} -> {}",
        if gql as usize == top.1 {
            "ÉGAL ✓"
        } else {
            "DIFFÉRENT ✗"
        }
    );
    if gql as usize != top.1 {
        std::process::exit(2);
    }
    // 2) µs : N appels O(1)
    let n = 200_000u64;
    let t0 = Instant::now();
    let mut acc = 0usize;
    for _ in 0..n {
        acc += s.count_messages_de_session(top.0).unwrap_or(0);
    }
    let dt = t0.elapsed().as_secs_f64() / n as f64 * 1e6;
    println!("count_messages_de_session : {dt:.3} µs/appel (n={n}, acc={acc})");
    // 3) liste : lecture de tous les ids
    let t1 = Instant::now();
    let ids = s.messages_de_session(top.0);
    let dl = t1.elapsed().as_secs_f64() * 1e6;
    println!("messages_de_session ({} ids) : {dl:.1} µs", ids.len());
    assert_eq!(ids.len(), top.1, "liste == count");
    println!("=== OK : API µs + exactitude croisée GQL ===");
}
