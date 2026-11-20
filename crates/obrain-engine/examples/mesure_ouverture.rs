//! Chronomètre l'ouverture d'une base obrain, isolée du reste, puis la
//! première utilisation des structures reconstruites paresseusement.
//!
//! ```bash
//! cargo run -p obrain-engine --release --features cypher --example mesure_ouverture -- <base>
//! ```
//!
//! Variables d'environnement :
//! * `MODE=ro`   : `open_read_only` au lieu de `open` ;
//! * `REQUETES=1`: après chaque ouverture, chronomètre les premières requêtes
//!   (comptage, lecture de propriétés, expansion) — le coût DÉPLACÉ par les
//!   reconstructions paresseuses, qu'il ne faut pas cacher ;
//! * `ECRITURE=1`: écrit ensuite une valeur longue (route blob) : coût de la
//!   première écriture, puis du `drop` (checkpoint). MODIFIE la base : à ne
//!   lancer que sur une copie ;
//! * `TRACE_OPEN=1` : journal `tracing` des phases d'ouverture (stderr).
//!
//! Elle n'affirme rien : elle imprime.
use std::time::Instant;

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    if std::env::var("TRACE_OPEN").is_ok() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .init();
    }
    let chemin = std::env::args().nth(1).expect("usage : <base>");
    let lecture_seule = std::env::var("MODE").is_ok_and(|m| m == "ro");
    let requetes = std::env::var("REQUETES").is_ok();
    let ecriture = std::env::var("ECRITURE").is_ok();
    for essai in 1..=3 {
        let depart = Instant::now();
        let db = if lecture_seule {
            obrain_engine::ObrainDB::open_read_only(&chemin)
        } else {
            obrain_engine::ObrainDB::open(&chemin)
        }
        .expect("ouverture");
        let duree = depart.elapsed();
        println!(
            "essai {essai} : ouverture {:?}  ({:.3} ms)",
            duree,
            ms(duree)
        );
        if requetes {
            let session = db.session();
            for (nom, q) in [
                ("comptage    ", "MATCH (n) RETURN count(n)"),
                ("proprietes  ", "MATCH (n) RETURN n LIMIT 20"),
                ("expansion   ", "MATCH (a)-[r]->(b) RETURN a, b LIMIT 20"),
            ] {
                let t = Instant::now();
                let r = session.execute(q);
                println!(
                    "    1re requête {nom}: {:>9.3} ms  ({})",
                    ms(t.elapsed()),
                    match &r {
                        Ok(r) => format!("{} lignes", r.rows.len()),
                        Err(e) => format!("ERREUR {e}"),
                    }
                );
            }
        }
        if ecriture && !lecture_seule {
            let session = db.session();
            let t = Instant::now();
            let long = "x".repeat(400);
            let r = session.execute(&format!(
                "MATCH (n) WITH n LIMIT 1 SET n.sonde_264 = '{long}' RETURN count(n)"
            ));
            println!(
                "    1re écriture longue : {:>9.3} ms  ({})",
                ms(t.elapsed()),
                if r.is_ok() { "ok" } else { "ERREUR" }
            );
        }
        let t = Instant::now();
        drop(db);
        println!("    drop : {:.3} ms", ms(t.elapsed()));
    }
}
