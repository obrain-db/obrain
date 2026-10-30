//! BANC MICRO du correctif E (comparateur canonique) — chemins chauds de
//! comparaison, dos à dos AVANT/APRÈS.
//!
//! Méthode (revue v1) : ~100 000 nœuds, n = 40 répétitions par requête,
//! profil release, médiane et minimum publiés. Le même fichier est déposé
//! tel quel sur l'arbre AVANT (base de la branche) et APRÈS : seule la
//! bibliothèque mesurée change.
//!
//! Lancement : cargo test --release --test banc_comparateurs -- --ignored --nocapture

use obrain_engine::ObrainDB;

const N_NOEUDS: usize = 100_000;
const N_MESURES: usize = 40;

fn mediane_et_min(mut d: Vec<f64>) -> (f64, f64) {
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (d[d.len() / 2], d[0])
}

#[test]
#[ignore = "banc de performance — lancer explicitement en release"]
fn banc_chemins_de_comparaison() {
    let db = ObrainDB::new_in_memory();
    {
        let mut s = db.session();
        s.begin_transaction().unwrap();
        for i in 0..N_NOEUDS {
            // v : entier ; s : chaîne (cardinalité 1000 pour DISTINCT).
            s.execute(&format!(
                "INSERT (:B {{id: {i}, v: {}, s: 's{}'}})",
                (i * 7919) % N_NOEUDS,
                i % 1000
            ))
            .unwrap();
        }
        s.commit().unwrap();
    }
    let s = db.session();

    let requetes: &[(&str, &str)] = &[
        (
            "WHERE-intervalle (balayage)",
            "MATCH (n:B) WHERE n.v > 50000 RETURN count(*)",
        ),
        (
            "WHERE-evaluateur (conjonction)",
            "MATCH (n:B) WHERE n.v > 50000 AND n.id >= 0 RETURN count(*)",
        ),
        (
            "ORDER BY (tri 100k)",
            "MATCH (n:B) RETURN n.v ORDER BY n.v DESC LIMIT 100",
        ),
        (
            "DISTINCT (1000 classes)",
            "MATCH (n:B) RETURN DISTINCT n.s",
        ),
        (
            "egalite planifiee (=)",
            "MATCH (n:B) WHERE n.v = 4242 RETURN count(*)",
        ),
    ];

    println!("banc comparateurs — {N_NOEUDS} noeuds, n = {N_MESURES}");
    for (nom, q) in requetes {
        // une exécution d'échauffement hors mesure
        s.execute(q).unwrap();
        let mut durees = Vec::with_capacity(N_MESURES);
        for _ in 0..N_MESURES {
            let t = web_time::Instant::now();
            let r = s.execute(q).unwrap();
            let dt = t.elapsed().as_secs_f64() * 1e3;
            std::hint::black_box(r);
            durees.push(dt);
        }
        let (med, min) = mediane_et_min(durees);
        println!("BANC | {nom:32} | mediane {med:9.3} ms | min {min:9.3} ms");
    }
}
