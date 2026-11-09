//! # `bench_texte_gql` — ce que l'index de sous-chaînes change à la REQUÊTE
//!
//! Le banc de `obrain-substrate` (`bench_trigrammes`) mesure le PRÉDICAT et la
//! revérification. Celui-ci mesure la **requête GQL**, celle que l'utilisateur
//! paie : `MATCH (n) WHERE n.name CONTAINS 'iam' …`.
//!
//! ── POURQUOI UN PROCESSUS LONG, ET C'EST LE POINT ────────────────────────
//!
//! L'index est **construit à la demande**, une fois par processus et par
//! propriété : 0,26 s pour `name` (279 084 valeurs), 2,94 s pour
//! `qualified_name` (347 748). Mesuré le 2026-09-13 : en lançant `obrain query`
//! (un processus par requête), la requête passait de 1,10 s à **2,32 s** — la
//! construction était repayée à chaque appel. Le gain n'existe donc que dans un
//! processus qui VIT : c'est le cas du pont MCP et du `viz serve`, pas du CLI.
//! Ce banc mesure les deux : le premier tour (qui construit) et le régime
//! établi.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release -p obrain-engine --example bench_texte_gql -- <STORE> [TOURS]
//! OBRAIN_TRIGRAMMES=0 …   # le MÊME binaire, sans l'index : le témoin
//! ```

use std::time::Duration;
use web_time::Instant;

use obrain_engine::ObrainDB;

/// Les requêtes de la plainte du journal MCP du 2026-09-12, plus deux témoins :
/// un terme COMMUN (tous ses grammes au-dessus du seuil → l'index se tait) et
/// un terme ABSENT (absence prouvée → zéro ligne, sans balayer).
fn requetes() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "iam (3 termes, 2 props)",
            "MATCH (n) WHERE n.name CONTAINS 'iam' OR n.name CONTAINS 'IAM' \
             OR n.qualified_name CONTAINS 'iam' RETURN n.qualified_name, labels(n) LIMIT 30",
        ),
        (
            "auth/token (4 termes)",
            "MATCH (n) WHERE n.name CONTAINS 'auth' OR n.name CONTAINS 'Auth' \
             OR n.name CONTAINS 'token' OR n.name CONTAINS 'Token' \
             RETURN n.qualified_name, labels(n) LIMIT 30",
        ),
        (
            "count qn CONTAINS 'iam'",
            "MATCH (n) WHERE n.qualified_name CONTAINS 'iam' RETURN count(n)",
        ),
        (
            "count name CONTAINS 'token'",
            "MATCH (n) WHERE n.name CONTAINS 'token' RETURN count(n)",
        ),
        (
            "témoin commun 'ion' (index muet)",
            "MATCH (n) WHERE n.name CONTAINS 'ion' RETURN count(n)",
        ),
        (
            "témoin absent 'zzqq' (conclusif)",
            "MATCH (n) WHERE n.name CONTAINS 'zzqq-absent' RETURN count(n)",
        ),
    ]
}

/// Le CORPUS RÉEL : un fichier `libellé|requête` par ligne (produit par
/// `scripts/requetes-lentes.sh --corpus`). Le banc mesure alors, DANS UN SEUL
/// PROCESSUS, le corpus entier — la mesure qui a un sens pour un pont MCP ou un
/// `viz serve`, alors qu'un CLI repayait la construction à chaque requête.
///
/// (Rétabli le 2026-09-13 : cette fonction avait disparu de l'arbre — le fichier
/// a été commité dans une version ANTÉRIEURE à son ajout, par une autre session.
/// Sans elle, le banc ne mesure que ses six requêtes codées en dur.)
fn lit_corpus(chemin: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(chemin)
        .expect("corpus lisible")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (nom, q) = l.split_once('|')?;
            Some((nom.trim().to_string(), q.trim().to_string()))
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(store_path) = args.next() else {
        eprintln!("usage: bench_texte_gql <STORE> [TOURS] [CORPUS]");
        std::process::exit(2);
    };
    let tours: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let corpus_fichier = args.next();
    let index_actif = std::env::var("OBRAIN_TRIGRAMMES").map_or(true, |v| v != "0");

    // LECTURE SEULE (verrou partagé) : c'est ce qui permet de mesurer un store
    // VIVANT — celui du harness — sans l'arrêter. `OBRAIN_LECTURE_SEULE=1`.
    let lecture_seule = std::env::var("OBRAIN_LECTURE_SEULE").is_ok_and(|v| v == "1");
    let db = if lecture_seule {
        ObrainDB::open_read_only(&store_path).expect("ouverture en lecture seule")
    } else {
        ObrainDB::open_substrate(&store_path).expect("ouverture du store")
    };
    println!("store  : {store_path}");
    println!(
        "index  : {}",
        if index_actif {
            "ACTIF (défaut)"
        } else {
            "ÉTEINT (témoin OBRAIN_TRIGRAMMES=0)"
        }
    );
    println!("tours  : {tours}");
    println!(
        "mode   : {}",
        if lecture_seule {
            "LECTURE SEULE (store vivant)"
        } else {
            "écriture"
        }
    );
    println!();
    println!(
        "  {:<32} {:>10} {:>11} {:>11} {:>8}   verdict",
        "requête", "1er tour", "min", "médiane", "lignes"
    );

    let liste: Vec<(String, String)> = match &corpus_fichier {
        Some(f) => lit_corpus(f),
        None => requetes()
            .into_iter()
            .map(|(n, q)| (n.to_string(), q.to_string()))
            .collect(),
    };
    for (nom, q) in &liste {
        let (nom, q) = (nom.as_str(), q.as_str());
        // 1er tour : il paie la construction de l'index si elle n'a pas eu lieu.
        let t = Instant::now();
        let premier = match db.session().execute(q) {
            Ok(r) => (t.elapsed(), r.rows.len()),
            Err(e) => {
                println!("  {nom:<32} ÉCHEC : {e}");
                continue;
            }
        };
        // VALEUR : première cellule de la première ligne, quand
        // `OBRAIN_BANC_VALEURS=1`. Sans elle, un `count(...)` ne se compare que
        // par « 1 ligne » — c'est-à-dire pas du tout. C'est le contrôle qui a
        // manqué tant que le CLI était cassé par une édition concurrente.
        let montrer = std::env::var("OBRAIN_BANC_VALEURS").is_ok_and(|v| v == "1");
        let mut valeur = String::new();
        if montrer
            && let Ok(r) = db.session().execute(q)
            && let Some(l) = r.rows.first()
            && let Some(c) = l.first()
        {
            valeur = format!("  valeur={c:?}");
        }

        let mut temps: Vec<Duration> = Vec::with_capacity(tours);
        let mut lignes = premier.1;
        for _ in 0..tours {
            let t = Instant::now();
            match db.session().execute(q) {
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
            "  {nom:<32} {:>9.1} ms {:>8.2} ms {:>8.2} ms {:>8}{valeur}",
            premier.0.as_secs_f64() * 1e3,
            min.as_secs_f64() * 1e3,
            med.as_secs_f64() * 1e3,
            lignes
        );
    }

    println!();
    println!("  « 1er tour » inclut la CONSTRUCTION de l'index (une fois par processus");
    println!("  et par propriété) ; « min » et « médiane » sont le régime établi.");
    println!("  Comparez les deux exécutions : le nombre de lignes doit être IDENTIQUE.");
}
