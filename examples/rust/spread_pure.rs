//! Mesure le noyau spread PUR (sans CLI, sans JSON) sur une base réelle.
use std::path::PathBuf;
use std::time::Instant;
use obrain_engine::ObrainDB;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args().nth(1).expect("db"));
    let db = ObrainDB::open(&path)?;
    let session = db.session();
    // seed : une vraie fonction
    let fid = session.execute("MATCH (n:Function) RETURN id(n) LIMIT 1")?
        .rows.first().and_then(|r| r.first()).map(|v| v.to_string()).unwrap();
    let q = format!("CALL obrain.cognitive.spread([{fid}], {{hops: 3}}) YIELD activated, energy RETURN activated, energy");
    // warmup
    let _ = session.execute(&q)?;
    let mut ts = Vec::new();
    for _ in 0..10 {
        let t = Instant::now();
        let r = session.execute(&q)?;
        ts.push(t.elapsed().as_secs_f64() * 1000.0);
        if ts.len() == 1 { println!("résultats: {} lignes", r.row_count()); }
    }
    ts.sort_by(|a,b| a.partial_cmp(b).unwrap());
    println!("spread pur: médiane {:.2} ms (min {:.2}, max {:.2})", ts[5], ts[0], ts[9]);
    Ok(())
}
