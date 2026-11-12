//! Fabrique une base JETABLE dont la fenêtre CDC n'est pas vide, pour exercer
//! la sonde `tests/cout_consolidation.rs` sans toucher à une vraie base.
//!
//! ```bash
//! cargo run --release -p obrain-engine --features cdc --example fabrique_base_cdc -- /tmp/x/cdc 2000
//! ```
//!
//! Refuse un chemin qui existe déjà.

#[cfg(feature = "cdc")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [chemin, n] = args.as_slice() else {
        eprintln!("usage : fabrique_base_cdc <dir> <N>");
        std::process::exit(2);
    };
    let n: i64 = n.parse().expect("N");
    assert!(
        !std::path::Path::new(chemin).exists(),
        "{chemin} existe déjà : on ne fabrique pas par-dessus"
    );
    let db = obrain_engine::ObrainDB::open(chemin).expect("création");
    let session = db.session();
    // Écritures GQL : le chemin que journalise le CDC.
    for i in 0..n {
        session
            .execute(&format!("INSERT (:Fait {{i: {i}}})"))
            .expect("insert");
    }
    session.execute("MATCH (f:Fait) SET f.vu = 1").expect("set");
    db.checkpoint().expect("checkpoint");
    println!(
        "base CDC fabriquée dans {chemin} : {} événements dans la fenêtre",
        db.cdc_log().changes_since_timestamp(0, |_| true).len()
    );
}

#[cfg(not(feature = "cdc"))]
fn main() {
    eprintln!("fabrique_base_cdc exige --features cdc");
    std::process::exit(2);
}
