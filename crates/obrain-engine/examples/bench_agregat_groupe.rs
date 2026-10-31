//! # `bench_agregat_groupe` — le coût des deux agrégats groupés, à CHAUD
//!
//! Mesure, DANS UN SEUL PROCESSUS, les deux requêtes que les écrans d'état et
//! les vues agrégées paient à chaque exécution :
//!
//!   A. `MATCH ()-[r]->() RETURN type(r), count(r) ORDER BY c DESC LIMIT 20`
//!   B. `MATCH (n) RETURN labels(n)[0], count(n) ORDER BY c DESC LIMIT k`
//!
//! ── POURQUOI UN NONCE UNIQUE PAR EXÉCUTION ──────────────────────────────
//!
//! Un texte de requête IDENTIQUE peut être servi par un cache (le pont MCP, le
//! `/api/query` du `viz serve`, ou le cache de plan du moteur) : on croirait
//! alors mesurer le raccourci en mesurant un `HashMap`. Chaque exécution porte
//! donc une colonne constante tirée au hasard (`'<nonce>' AS k`), ce qui rend
//! le texte unique — et ne change ni le plan ni le coût, la colonne n'étant
//! qu'une constante projetée.
//!
//! ── POURQUOI UN PROCESSUS LONG ──────────────────────────────────────────
//!
//! C'est le contexte RÉEL des routes chaudes (`viz serve`, pont MCP) : un
//! processus qui vit. Le CLI, lui, repaie l'ouverture à chaque appel ; il est
//! mesuré à part par `scripts/requetes-lentes.sh`.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release -p obrain-engine --example bench_agregat_groupe -- <STORE> [TOURS]
//! ```
//!
//! Le premier tour est affiché À PART : c'est celui qui paie ce qui ne se paie
//! qu'une fois (index à la demande, pages mmap à froid). « min » et « médiane »
//! sont le régime établi.

use std::time::Duration;
use web_time::Instant;

use obrain_engine::ObrainDB;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(store) = args.next() else {
        eprintln!("usage: bench_agregat_groupe <STORE> [TOURS]");
        std::process::exit(2);
    };
    let tours: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);

    let t = Instant::now();
    let db = ObrainDB::open_read_only(&store).expect("ouverture en lecture seule");
    let ouverture = t.elapsed();

    println!("store    : {store}");
    println!("ouverture: {:.1} ms", ouverture.as_secs_f64() * 1e3);
    println!("tours    : {tours}");
    println!();

    // Clé = libellé ; valeur = générateur de requête à nonce.
    let corpuses: [(&str, fn(&str) -> String); 2] = [
        ("A type(r) groupé", |n| {
            format!(
                "MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS c, '{n}' AS k \
                 ORDER BY c DESC LIMIT 20"
            )
        }),
        ("B labels(n)[0] groupé", |n| {
            format!(
                "MATCH (n) RETURN labels(n)[0] AS l, count(n) AS c, '{n}' AS k \
                 ORDER BY c DESC LIMIT 20"
            )
        }),
    ];

    println!(
        "  {:<24} {:>11} {:>11} {:>11} {:>8}",
        "requête", "1er tour", "min", "médiane", "lignes"
    );
    for (nom, genere) in corpuses {
        // Nonce unique par exécution — y compris le premier tour.
        let nonce = |i: usize| {
            format!(
                "{}{}{}",
                std::process::id(),
                Instant::now().elapsed().as_nanos(),
                web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
                    + i as u128
            )
        };
        let q = genere(&nonce(0));
        let t = Instant::now();
        let premier = match db.session().execute(&q) {
            Ok(r) => (t.elapsed(), r.rows.len()),
            Err(e) => {
                println!("  {nom:<24} ÉCHEC : {e}");
                continue;
            }
        };
        let mut temps: Vec<Duration> = Vec::with_capacity(tours);
        let mut lignes = premier.1;
        for i in 0..tours {
            let q = genere(&nonce(i + 1));
            let t = Instant::now();
            match db.session().execute(&q) {
                Ok(r) => {
                    temps.push(t.elapsed());
                    lignes = r.rows.len();
                }
                Err(_) => break,
            }
        }
        temps.sort();
        let min = temps.first().copied().unwrap_or(Duration::ZERO);
        let med = temps
            .get(temps.len() / 2)
            .copied()
            .unwrap_or(Duration::ZERO);
        println!(
            "  {nom:<24} {:>8.2} ms {:>8.2} ms {:>8.2} ms {:>8}",
            premier.0.as_secs_f64() * 1e3,
            min.as_secs_f64() * 1e3,
            med.as_secs_f64() * 1e3,
            lignes
        );
    }

    println!();
    println!("  Le « 1er tour » paie ce qui ne se paie qu'une fois ; « min » et");
    println!("  « médiane » sont le régime établi d'un processus qui VIT.");
}
