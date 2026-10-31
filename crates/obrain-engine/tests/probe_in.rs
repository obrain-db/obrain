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
    let (_a, petit) = build(2_000, true);
    let (_b, grand) = build(20_000, true);
    println!("IN indexé : 2k → {petit} µs, 20k → {grand} µs");

    // Un balayage de label coûterait ~10× de plus à 10× la taille. Une
    // sonde d'index ne dépend pas de la cardinalité. Marge large pour
    // absorber le bruit de l'ordonnanceur, mais suffisante pour
    // distinguer les deux régimes.
    assert!(
        grand < petit.max(1) * 4,
        "IN semble balayer le label : 2k={petit} µs, 20k={grand} µs \
         (une sonde d'index devrait rester du même ordre)"
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
