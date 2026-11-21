//! Écriture CONCURRENTE : N fils appellent bulk_import_props sur des plages de
//! slots DISJOINTES. Teste si le multi-écrivain parallélise l'écriture d'UNE
//! migration (et non juste des sessions concurrentes).
//! Usage : mesure_ingestion_writers <jsonl> <limite> <n_writers>

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

fn parse_vecteur(s: &str) -> Option<Value> {
    let t = s.trim().trim_start_matches('[').trim_end_matches(']');
    let mut v: Vec<f32> = Vec::new();
    for tok in t.split(',') {
        v.push(tok.trim().parse::<f32>().ok()?);
    }
    if v.is_empty() {
        None
    } else {
        Some(Value::Vector(v.into()))
    }
}
fn json_val(j: &serde_json::Value) -> Option<Value> {
    Some(match j {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int64(i)
            } else {
                Value::Float64(n.as_f64()?)
            }
        }
        serde_json::Value::String(s) => Value::from(s.clone()),
        other => Value::from(other.to_string()),
    })
}
fn parse_ligne(line: &str) -> Vec<(String, Value)> {
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(serde_json::Value::Object(m)) => m
            .iter()
            .filter_map(|(k, v)| {
                if k == "embedding"
                    && let serde_json::Value::String(t) = v
                {
                    return parse_vecteur(t).map(|vv| (k.clone(), vv));
                }
                json_val(v).map(|vv| (k.clone(), vv))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn main() {
    let jsonl = std::env::args()
        .nth(1)
        .expect("usage: <jsonl> <limite> <n>");
    let limite: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap();
    let nw: usize = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    let total = BufReader::new(std::fs::File::open(&jsonl).unwrap())
        .lines()
        .take(limite)
        .count();
    eprintln!("total {total}, writers {nw}");
    let dir = std::env::temp_dir().join(format!("obrain-w-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = ObrainDB::open_substrate(&dir).expect("create");
    let labels: Vec<&str> = vec!["Row"; total];
    let (e1, e2, e3): (Vec<u32>, Vec<u32>, Vec<&str>) = (Vec::new(), Vec::new(), Vec::new());
    db.bulk_import_refs(&labels, &e1, &e2, &e3).expect("A");

    const BATCH: usize = 20_000;
    let (tx, rx) = mpsc::sync_channel::<(usize, Vec<String>)>(nw * 2);
    let rx = Arc::new(Mutex::new(rx));

    let t = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..nw {
            let rx = Arc::clone(&rx);
            let db = &db;
            s.spawn(move || {
                loop {
                    let item = { rx.lock().unwrap().recv() };
                    let Ok((seq, lines)) = item else { break };
                    let parsed: Vec<Vec<(String, Value)>> =
                        lines.iter().map(|l| parse_ligne(l)).collect();
                    let refs: Vec<Vec<(&str, Value)>> = parsed
                        .iter()
                        .map(|p| p.iter().map(|(k, v)| (k.as_str(), v.clone())).collect())
                        .collect();
                    db.bulk_import_props((seq * BATCH) as u32 + 1, &refs)
                        .expect("write");
                }
            });
        }
        // lecteur
        let rd = BufReader::new(std::fs::File::open(&jsonl).unwrap());
        let mut batch: Vec<String> = Vec::with_capacity(BATCH);
        let mut seq = 0usize;
        let mut n = 0usize;
        for line in rd.lines() {
            let Ok(l) = line else { continue };
            batch.push(l);
            n += 1;
            if batch.len() >= BATCH {
                if tx.send((seq, std::mem::take(&mut batch))).is_err() {
                    break;
                }
                seq += 1;
                batch = Vec::with_capacity(BATCH);
            }
            if n >= limite {
                break;
            }
        }
        if !batch.is_empty() {
            let _ = tx.send((seq, batch));
        }
        drop(tx);
    });
    let tp = t.elapsed();
    // Vérification d'intégrité : toutes les lignes présentes, ids tous distincts.
    {
        let s = db.session();
        if let Ok(c) = s.execute("MATCH (n:Row) RETURN count(n) AS c")
            && let Ok(d) = s.execute("MATCH (n:Row) RETURN count(DISTINCT n.id) AS d")
        {
            println!(
                "  vérif: count(n)={:?} count(DISTINCT id)={:?}",
                c.rows.first(),
                d.rows.first()
            );
        }
    }
    let t2 = Instant::now();
    drop(db);
    let tf = t2.elapsed();
    println!("writers={nw} n={limite}");
    println!(
        "  écriture (parse+bulk, {nw} fils) : {:>9.2} ms -> {:>12.0} nœuds/s",
        tp.as_secs_f64() * 1000.0,
        limite as f64 / tp.as_secs_f64()
    );
    println!("  checkpoint : {:>9.2} ms", tf.as_secs_f64() * 1000.0);
    println!(
        "  TOTAL      : {:>9.2} ms -> {:>12.0} nœuds/s",
        (tp + tf).as_secs_f64() * 1000.0,
        limite as f64 / (tp + tf).as_secs_f64()
    );
}
