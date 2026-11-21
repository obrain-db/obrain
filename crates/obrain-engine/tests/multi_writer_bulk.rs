//! Multi-écrivain MONO-STORE : N fils écrivant des propriétés sur des slots
//! DISJOINTS doivent produire exactement les mêmes données qu'un seul fil.
//!
//! Avant le correctif, l'écriture bulk partait du curseur global lu SANS
//! réservation : deux écrivains se chevauchaient et le substrat refusait
//! (« blocs non contigus — zone trouée »). La réservation atomique
//! (`reserver_et_avancer` sous verrou) leur donne des plages disjointes.
#![cfg(feature = "wal")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Importe n nœuds (ids t00000000…) avec nw fils concurrents sur slots
/// disjoints, et rend (count(n), count(DISTINCT id)).
fn importe(nw: usize, n: usize) -> (i64, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate(dir.path()).expect("base");
    let labels: Vec<&str> = vec!["Row"; n];
    let (e1, e2, e3): (Vec<u32>, Vec<u32>, Vec<&str>) = (Vec::new(), Vec::new(), Vec::new());
    db.bulk_import_refs(&labels, &e1, &e2, &e3)
        .expect("phase A");

    let chunk = n.div_ceil(nw);
    std::thread::scope(|sc| {
        for w in 0..nw {
            let db = &db;
            sc.spawn(move || {
                let start = w * chunk;
                let end = ((w + 1) * chunk).min(n);
                if start >= end {
                    return;
                }
                let props: Vec<Vec<(&str, Value)>> = (start..end)
                    .map(|i| {
                        vec![
                            ("id", Value::from(format!("t{i:08}"))),
                            ("n", Value::from(i as i64)),
                        ]
                    })
                    .collect();
                db.bulk_import_props(start as u32 + 1, &props)
                    .expect("props");
            });
        }
    });

    let s = db.session();
    let rc = s
        .execute("MATCH (n:Row) RETURN count(n) AS c")
        .expect("count");
    let c = match rc.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(i)) => *i,
        _ => -1,
    };
    let rd = s
        .execute("MATCH (n:Row) RETURN count(DISTINCT n.id) AS d")
        .expect("distinct");
    let d = match rd.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(i)) => *i,
        _ => -1,
    };
    (c, d)
}

#[test]
fn quatre_ecrivains_egalent_un_seul() {
    let n = 20_000;
    let (c1, d1) = importe(1, n);
    let (c4, d4) = importe(4, n);
    assert_eq!(c1, n as i64, "un fil doit écrire les {n} nœuds");
    assert_eq!(
        (c1, d1),
        (c4, d4),
        "4 écrivains concurrents doivent produire EXACTEMENT les mêmes données qu'un seul"
    );
}
