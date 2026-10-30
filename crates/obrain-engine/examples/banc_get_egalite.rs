//! BANC DE LECTURE — le GET par `qualified_name` frappe-t-il l'index d'égalité ?
//!
//! ── LA QUESTION, ET POURQUOI ELLE SE POSE ────────────────────────────────
//!
//! Un store porte un index d'égalité PERSISTÉ (`substrate.props.eq.idx.v2`) et
//! un catalogue (`substrate.indexcat`) qui DÉCLARE la propriété. Le GET par
//! cette propriété devrait donc être un seek. Mesuré sur `full2-4` (1,55 M
//! nœuds) : le plan rendu par `EXPLAIN` est
//!
//! ```text
//! Return (n.qualified_name AS q)
//!   Filter (n.qualified_name Eq mnt.obrain…:branch:DETACHED) [index: qualified_name]
//!     NodeScan (n:*)
//! ```
//!
//! — l'annotation `[index: qualified_name]` dit que le planificateur a CRU
//! tenir un index, et le `NodeScan` juste dessous dit qu'il n'en a rien fait.
//!
//! Ce banc mesure ce que ce plan coûte, par requête, dans UN SEUL processus
//! (l'ouverture et le mmap sont hors du chronomètre — c'est la requête qu'on
//! juge, pas le démarrage).
//!
//! Usage :
//!   cargo run --release -p obrain-engine --example banc_get_egalite -- \
//!       <chemin> [valeur] [sondes]
//!
//! `OBRAIN_EGALITE_DES_LA=1` force la construction de l'index EN RAM au premier
//! appel : c'est le témoin du chemin indexé, à comparer au même binaire sans la
//! variable.

use obrain_engine::ObrainDB;
use web_time::Instant;

fn pct(tries: &[u128], p: f64) -> u128 {
    if tries.is_empty() {
        return 0;
    }
    let i = ((tries.len() as f64 - 1.0) * p).round() as usize;
    tries[i]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let chemin = args
        .first()
        .cloned()
        .expect("usage: banc_get_egalite <chemin> [valeur] [sondes]");
    let valeur = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "mnt.obrain.sources.linux_6.6:branch:DETACHED".to_string());
    let sondes: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50);
    let propriete = args.get(3).cloned().unwrap_or_else(|| "qualified_name".to_string());

    let t_open = Instant::now();
    let db = ObrainDB::open_read_only(std::path::Path::new(&chemin)).expect("ouverture");
    println!(
        "ouverture (hors chrono) : {:.2} s   nœuds={}",
        t_open.elapsed().as_secs_f64(),
        db.graph_store().node_count()
    );

    let session = db.session();
    let q =
        format!("MATCH (n) WHERE n.{propriete} = '{valeur}' RETURN n.{propriete} AS q");

    // Le PLAN, tel que le planificateur le rend : c'est le verdict structurel,
    // indépendant de la charge machine.
    match session.execute(&format!("EXPLAIN {q}")) {
        Ok(r) => println!("\nPLAN :\n{}", r.rows[0][0]),
        Err(e) => println!("\nEXPLAIN refusé : {e}"),
    }

    // Rodage (première exécution : caches froids, index paresseux).
    let t = Instant::now();
    let r = session.execute(&q).expect("requete");
    let attendu = r.rows.len();
    println!(
        "\n1re exécution : {:.2} ms   lignes={attendu}",
        t.elapsed().as_secs_f64() * 1e3,
    );

    let mut tries: Vec<u128> = Vec::with_capacity(sondes);
    for _ in 0..sondes {
        let t = Instant::now();
        let r = session.execute(&q).expect("requete");
        tries.push(t.elapsed().as_micros());
        assert_eq!(
            r.rows.len(),
            attendu,
            "le banc doit rendre le MEME nombre de lignes a chaque tour"
        );
    }
    tries.sort_unstable();
    let somme: u128 = tries.iter().sum();
    println!(
        "GET par {propriete} : n={}  moy={:.1} µs  p50={} µs  p95={} µs  max={} µs",
        tries.len(),
        somme as f64 / tries.len() as f64,
        pct(&tries, 0.50),
        pct(&tries, 0.95),
        tries.last().copied().unwrap_or(0),
    );
    println!(
        "OBRAIN_EGALITE_DES_LA={:?}",
        std::env::var("OBRAIN_EGALITE_DES_LA").ok()
    );
}
