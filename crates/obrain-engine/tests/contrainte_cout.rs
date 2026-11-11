//! Garde-fou de COÛT : le contrôle d'unicité d'un tuple ne dépend pas de
//! l'ordre dans lequel ses colonnes sont déclarées.
//!
//! `find_nodes_by_properties` résolvait par la PREMIÈRE condition indexée :
//! `UNIQUE (a, b)` avec `a` à deux valeurs relisait la moitié du label à chaque
//! insertion (744 µs contre 25 µs pour `UNIQUE (b, a)` à 10 000 nœuds, et
//! linéaire en N). Il résout maintenant par la condition dont le jeu de
//! candidats indexés est le plus petit.
//!
//! Le test mesure le temps de 2 000 insertions sous chacune des deux
//! déclarations, sur 100 000 nœuds, et exige un rapport au plus 2 (il était
//! d'environ 30). Trois répétitions, le minimum de chacune : la machine
//! partagée fait du bruit, pas un facteur 15.

#![cfg(feature = "cypher")]

use std::time::{Duration, Instant};

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

const N: i64 = 100_000;
const OPS: i64 = 2_000;

fn mesurer(db: &ObrainDB, contrainte: &str, nom: &str, debut_valeurs: i64) -> Duration {
    let session = db.session();
    session
        .execute_cypher(contrainte)
        .expect("CREATE CONSTRAINT");
    let mut minimum = Duration::MAX;
    for repetition in 0..3i64 {
        let base = debut_valeurs + repetition * (OPS + 1);
        // Échauffement : la première écriture d'une clé bâtit son index.
        session
            .execute(&format!("INSERT (:T {{a: {}, b: 'w{base}'}})", base % 2))
            .expect("échauffement");
        let top = Instant::now();
        for j in 1..=OPS {
            let v = base + j;
            session
                .execute(&format!("INSERT (:T {{a: {}, b: 'i{v}'}})", v % 2))
                .unwrap_or_else(|e| panic!("insertion {v} : {e}"));
        }
        minimum = minimum.min(top.elapsed());
    }
    session
        .execute_cypher(&format!("DROP CONSTRAINT {nom}"))
        .expect("DROP CONSTRAINT");
    minimum
}

#[test]
fn le_cout_du_tuple_ne_depend_pas_de_l_ordre_des_colonnes() {
    let db = ObrainDB::new_in_memory();
    {
        let store = db.graph_store();
        let (cle_a, cle_b) = (PropertyKey::new("a"), PropertyKey::new("b"));
        for i in 0..N {
            store.create_node_with_props(
                &["T"],
                &[
                    (cle_a.clone(), Value::Int64(i % 2)),
                    (cle_b.clone(), Value::from(format!("v{i}"))),
                ],
            );
        }
    }
    // `a` à deux valeurs en TÊTE : le cas qui coûtait.
    let a_puis_b = mesurer(
        &db,
        "CREATE CONSTRAINT ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE",
        "ab",
        N,
    );
    let b_puis_a = mesurer(
        &db,
        "CREATE CONSTRAINT ba FOR (n:T) REQUIRE (n.b, n.a) IS UNIQUE",
        "ba",
        N + 10 * (OPS + 1),
    );
    let rapport = a_puis_b.as_secs_f64() / b_puis_a.as_secs_f64();
    eprintln!(
        "UNIQUE (a, b) : {:.1} µs/insert ; UNIQUE (b, a) : {:.1} µs/insert ; rapport {rapport:.2}",
        a_puis_b.as_secs_f64() * 1e6 / OPS as f64,
        b_puis_a.as_secs_f64() * 1e6 / OPS as f64
    );
    assert!(
        rapport <= 2.0,
        "UNIQUE (a, b) coûte {rapport:.1} fois UNIQUE (b, a) : la colonne de tête \
         résout encore au lieu de la plus sélective"
    );
}
