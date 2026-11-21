//! G4 — procédures `CALL obrain.spatial.*`.
//!
//! Ce fichier est gaté sur la feature `algos` : c'est elle qui arme `CALL`
//! dans obrain (`CALL procedures require the 'algos' feature`). Les procédures
//! vectorielles et cognitives vivent sous la même porte — ce n'est pas une
//! limitation du spatial, c'est le contrat du planificateur.
//!
//! Lancer : `cargo test -p obrain-engine --features cypher,algos --test spatial_gql_procedures`
#![cfg(feature = "algos")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn point(lon: f64, lat: f64) -> Value {
    Value::List(vec![Value::Float64(lon), Value::Float64(lat)].into())
}

fn peupler(db: &ObrainDB, label: &str) {
    for i in 0..40u32 {
        for j in 0..25u32 {
            let lon = (2_000.0 + f64::from(i)) / 1_000.0;
            let lat = (48_000.0 + f64::from(j)) / 1_000.0;
            db.create_node_with_props(&[label], [("pos", point(lon, lat))]);
        }
    }
}

fn creer_index(db: &ObrainDB) {
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL SPATIAL");
}

// ── G4 : procédures CALL ──────────────────────────────────────────────────

#[test]
fn call_nearest_rend_les_plus_proches_dans_le_bon_ordre() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);
    let r = db
        .execute(
            "CALL obrain.spatial.nearest('lieux_pos', 2.020, 48.012, 5) YIELD node, distance \
             RETURN distance",
        )
        .expect("la procédure doit être connue");
    assert_eq!(r.rows.len(), 5, "5 voisins demandés");
    let distances: Vec<f64> = r
        .rows
        .iter()
        .filter_map(|l| l.first().and_then(Value::as_float64))
        .collect();
    assert_eq!(distances.len(), 5);
    assert!(
        distances.windows(2).all(|w| w[0] <= w[1]),
        "distances croissantes : {distances:?}"
    );
    // Le premier voisin est le point de la grille le plus proche de (2.050, 48.050).
    assert!(
        distances[0] < 1.0,
        "le point le plus proche est à moins d'un mètre : {distances:?}"
    );
}

#[test]
fn call_within_rend_la_fenetre_et_accepte_yield_node() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);
    let r = db
        .execute(
            "CALL obrain.spatial.within('lieux_pos', 2.020, 48.012, 300) YIELD node, distance \
             RETURN distance",
        )
        .expect("la procédure doit être connue");
    // Comparaison avec le prédicat GQL équivalent : c'est le MÊME ensemble.
    let attendu = db
        .execute("MATCH (n:Lieu) WHERE distance(n.pos, point(2.020, 48.012)) <= 300 RETURN n")
        .unwrap()
        .rows
        .len();
    assert_eq!(
        r.rows.len(),
        attendu,
        "la fenêtre doit égaler le prédicat GQL"
    );

    // `node` est un NŒUD : on doit pouvoir lire ses propriétés après YIELD.
    let r2 = db
        .execute(
            "CALL obrain.spatial.nearest('lieux_pos', 2.020, 48.012, 3) YIELD node AS n \
             WHERE n.pos IS NOT NULL RETURN n.pos AS p",
        )
        .expect("YIELD node AS n ... WHERE n.pos doit fonctionner");
    assert_eq!(r2.rows.len(), 3);
}

#[test]
fn les_procedures_sont_documentees_dans_obrain_procedures() {
    let db = ObrainDB::new_in_memory();
    let r = db
        .execute("CALL obrain.procedures()")
        .expect("le catalogue des procédures");
    let noms: Vec<String> = r
        .rows
        .iter()
        .filter_map(|l| l.first().and_then(Value::as_str).map(str::to_string))
        .collect();
    for attendu in ["obrain.spatial.nearest", "obrain.spatial.within"] {
        assert!(
            noms.iter().any(|n| n == attendu),
            "{attendu} doit être listé : {noms:?}"
        );
    }
}

#[test]
fn call_sur_un_index_inconnu_est_une_erreur_qui_le_nomme() {
    let db = ObrainDB::new_in_memory();
    let err = db
        .execute("CALL obrain.spatial.nearest('inexistant', 2.0, 48.0, 3) YIELD node RETURN node")
        .expect_err("un index inconnu doit être une erreur")
        .to_string();
    assert!(
        err.contains("inexistant"),
        "l'erreur doit nommer l'index : {err}"
    );
}

// ── Pas d'index implicite (2026-11-21) ───────────────────────────────────
//
// Régression : un identifiant `label:propriété` était accepté sans
// déclaration, et l'index se bâtissait au premier usage — y compris depuis
// le PLANIFICATEUR, sur un simple `WHERE distance(...)`. Une base sans
// `CREATE INDEX` finissait avec un index en mémoire, non durable et
// invisible. La preuve est déterministe : après la lecture, le `CALL` par
// identifiant doit toujours refuser.

#[test]
fn une_lecture_spatiale_ne_batit_aucun_index_implicite() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    let r = db
        .execute("MATCH (n:Lieu) WHERE distance(n.pos, point(2.020, 48.012)) <= 300 RETURN n")
        .expect("balayage sans index");
    assert!(!r.rows.is_empty(), "le balayage doit trouver des points");
    let e = db
        .execute(
            "CALL obrain.spatial.within('Lieu:pos', 2.020, 48.012, 300) YIELD node RETURN node",
        )
        .expect_err("aucun index n'a été déclaré : l'identifiant doit être refusé");
    assert!(
        format!("{e:?}").contains("index spatial inconnu"),
        "le refus doit nommer la cause : {e:?}"
    );
}

#[test]
fn lidentifiant_dun_index_declare_reste_accepte() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);
    let r = db
        .execute(
            "CALL obrain.spatial.within('Lieu:pos', 2.020, 48.012, 300) YIELD node RETURN node",
        )
        .expect("identifiant d'un index déclaré");
    assert!(!r.rows.is_empty());
}

#[test]
fn regression_geofence_pres_frontiere_arrondi_f32() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(179.999_992_5, 0.0))]);
    let requete = "MATCH (n:Lieu) WHERE distance(n.pos, point(179.9999922, 0.0)) <= 0.1 RETURN n";
    let avant = db.execute(requete).expect("balayage").rows.len();
    creer_index(&db);
    let apres = db.execute(requete).expect("index").rows.len();
    assert_eq!(avant, 1);
    assert_eq!(apres, avant, "un index ne doit pas modifier le résultat");
}
