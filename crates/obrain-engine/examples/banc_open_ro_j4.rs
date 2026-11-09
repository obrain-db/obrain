//! Banc J4 (scratch, non commite) : cout REEL d'une ouverture RO d'un store
//! chaud, et empreinte (fds, mappings) par poignee — les chiffres qui
//! tranchent la fourche du plan J4 (poignees par tenant en cache vs
//! mecanisme par requete).
//!
//!   cargo run --release -p obrain-engine --example banc_open_ro_j4 -- <store> [n]
//!   cargo run --release -p obrain-engine --example banc_open_ro_j4 -- <store> creer <noeuds>

use obrain_engine::ObrainDB;
use web_time::Instant;

fn fds() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count())
}
fn maps() -> usize {
    std::fs::read_to_string("/proc/self/maps").map_or(0, |s| s.lines().count())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: banc_open_ro_j4 <store> [n | creer N]");
    let deux = args.next();

    if deux.as_deref() == Some("creer") {
        let n: usize = args
            .next()
            .and_then(|s| s.parse().ok())
            .expect("creer <nombre de noeuds>");
        let d = ObrainDB::open_substrate(&path).expect("open_substrate (creer)");
        let s = d.session();
        for i in 0..n as i64 {
            s.create_node_with_props(&["Doc"], [("i", obrain_common::types::Value::Int64(i))]);
        }
        d.checkpoint().expect("checkpoint");
        println!("store cree : {n} noeuds Doc a {path}");
        return;
    }

    let n: usize = deux.and_then(|s| s.parse().ok()).unwrap_or(6);

    // Ouverture FROIDE (la premiere paie le cache de pages si le store ne
    // vient pas d'etre lu) puis ouvertures CHAUDES repetees.
    let t0 = Instant::now();
    let premier = ObrainDB::open_read_only(&path).expect("open froid");
    let dt0 = t0.elapsed();
    let tc = Instant::now();
    let c0 = premier
        .session()
        .execute("MATCH (n) RETURN count(n)")
        .expect("count froid");
    println!(
        "open froid : {dt0:?} ; count(n) sans principal = {:?} en {:?}",
        c0.rows[0][0],
        tc.elapsed()
    );

    let (f0, m0) = (fds(), maps());
    let mut poignees = Vec::new();
    for i in 0..n {
        let t = Instant::now();
        let h = ObrainDB::open_read_only(&path).expect("open chaud");
        let dt_open = t.elapsed();
        h.avec_principal(7).expect("avec_principal(7)");
        let t2 = Instant::now();
        let c = h
            .session()
            .execute("MATCH (n) RETURN count(n)")
            .expect("count confine");
        println!(
            "open chaud #{i} : {dt_open:?} ; count(vue tenant 7) = {:?} en {:?} ; fds +{}, mappings +{}",
            c.rows[0][0],
            t2.elapsed(),
            fds().saturating_sub(f0),
            maps().saturating_sub(m0)
        );
        poignees.push(h);
    }
    println!(
        "empreinte pour {n} poignees RO simultanees : fds +{}, mappings +{}",
        fds().saturating_sub(f0),
        maps().saturating_sub(m0)
    );
    drop(premier);
    drop(poignees);
}
