//! Sonde : le chemin `IN` doit sonder l'index, pas balayer le label.
#![cfg(feature = "gql")]

use obrain_engine::ObrainDB;
use web_time::Instant;

fn build(n: usize, indexed: bool) -> (ObrainDB, u128) {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        for i in 0..n {
            s.execute(&format!(
                "INSERT (:Big {{id: \"id{i}\", blob: \"{}\"}})",
                "x".repeat(200)
            ))
            .unwrap();
        }
    }
    if indexed {
        db.create_property_index("id");
    }
    let s = db.session();
    let target = format!("id{}", n - 1);
    // chauffe
    let _ = s.execute(&format!(
        "MATCH (n:Big) WHERE n.id IN [\"{target}\"] RETURN n.id AS i"
    ));
    let t = Instant::now();
    for _ in 0..5 {
        let r = s
            .execute(&format!(
                "MATCH (n:Big) WHERE n.id IN [\"{target}\"] RETURN n.id AS i"
            ))
            .unwrap();
        assert_eq!(r.rows.len(), 1);
    }
    (db, t.elapsed().as_micros() / 5)
}

#[test]
fn in_probes_index_instead_of_scanning_label() {
    // Comparer deux tailles à quelques microsecondes près mesurait le bruit
    // (7 µs → 28 µs sur un runner chargé : rouge sur `main` le 03/11/2026).
    // On compare à la place les DEUX régimes sur les MÊMES données : avec index
    // la sonde ne dépend pas de la cardinalité, sans index le plan générique
    // balaie le label. Meilleur de 5 séries pour écarter l'ordonnanceur.
    let meilleur = |indexed: bool| (0..5).map(|_| build(20_000, indexed).1).min().unwrap();
    let avec_index = meilleur(true);
    let sans_index = meilleur(false);
    println!("IN à 20k nœuds : indexé {avec_index} µs, balayage {sans_index} µs");
    assert!(
        avec_index.max(1) * 5 < sans_index,
        "IN semble balayer le label même indexé : indexé={avec_index} µs, \
         balayage={sans_index} µs (une sonde devrait être ≥5× plus rapide)"
    );
}

#[test]
fn in_without_index_still_returns_the_right_rows() {
    // Sans index le chemin rend la main au plan générique : on vérifie
    // seulement que le résultat reste juste.
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..500 {
        s.execute(&format!("INSERT (:NoIdx {{k: \"k{i}\"}})"))
            .unwrap();
    }
    let r = s
        .execute("MATCH (n:NoIdx) WHERE n.k IN [\"k7\", \"k499\", \"absent\"] RETURN n.k AS k")
        .unwrap();
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn indexed_in_matches_unindexed_result() {
    // Même question posée aux deux régimes : mêmes lignes.
    let mut sans = Vec::new();
    let mut avec = Vec::new();
    for (indexed, out) in [(false, &mut sans), (true, &mut avec)] {
        let db = ObrainDB::new_in_memory();
        {
            let s = db.session();
            for i in 0..300 {
                s.execute(&format!("INSERT (:Cmp {{id: \"c{i}\"}})"))
                    .unwrap();
            }
        }
        if indexed {
            db.create_property_index("id");
        }
        let s = db.session();
        let r = s
            .execute("MATCH (n:Cmp) WHERE n.id IN [\"c0\", \"c150\", \"c299\", \"c999\"] RETURN n.id AS i")
            .unwrap();
        let mut v: Vec<String> = r.rows.iter().map(|row| format!("{:?}", row[0])).collect();
        v.sort();
        *out = v;
    }
    assert_eq!(sans, avec, "l'index ne doit pas changer le résultat");
    assert_eq!(sans.len(), 3);
}
