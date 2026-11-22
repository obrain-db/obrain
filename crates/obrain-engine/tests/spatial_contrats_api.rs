//! Contrats de l'API spatiale que les autres fichiers ne fixent pas.
//!
//! Le reste de la suite spatiale vérifie la parité index ⇔ balayage. Ce
//! fichier fixe les BORDS : les refus (base sans substrat, nom déjà pris,
//! arguments de procédure invalides), l'entretien de l'index quand un nœud
//! REÇOIT le label après coup, le nommage par l'API Rust, le kNN Rust, la
//! survie à la réouverture, et les formes de prédicat que le planificateur
//! laisse au balayage (le résultat doit rester celui du balayage).
//!
//! Les procédures `CALL obrain.spatial.*` exigent la feature `algos` : leurs
//! tests sont regroupés dans le module `procedures`, gaté.

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use obrain_engine::config::Config;

fn point(lon: f64, lat: f64) -> Value {
    Value::List(vec![Value::Float64(lon), Value::Float64(lat)].into())
}

/// Une base qui n'a PAS de substrat : `with_store` ne renseigne pas
/// `substrate_store` (même construction que `window_call_procedure`).
fn base_sans_substrat() -> (tempfile::TempDir, ObrainDB) {
    let td = tempfile::tempdir().expect("tempdir");
    let raw = obrain_substrate::SubstrateStore::create(td.path().join("kb")).expect("store");
    let erased: Arc<dyn obrain_core::graph::GraphStoreMut> = Arc::new(raw);
    let db = ObrainDB::with_store(erased, Config::in_memory()).expect("with_store");
    (td, db)
}

#[test]
fn le_ddl_spatial_exige_exactement_une_propriete() {
    let db = ObrainDB::new_in_memory();
    let err = db
        .execute("CREATE INDEX lieux FOR (n:Lieu) ON (n.pos, n.autre) USING SPATIAL")
        .expect_err("deux propriétés pour un point");
    let message = err.to_string();
    assert!(
        message.contains("UNE propriété"),
        "le refus dit la règle : {message}"
    );
    assert!(
        message.contains("2 ont été données"),
        "le refus dit ce qui a été reçu : {message}"
    );
}

#[test]
fn une_valeur_non_point_replie_sur_le_balayage_qui_refuse() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL");
    // Un nœud du label porte sur la propriété indexée une valeur qui n'est pas
    // un point. L'index ne peut pas rendre le même refus que le balayage : il
    // se replie sur lui, et c'est le balayage qui refuse. Une erreur, jamais un
    // résultat partiel.
    db.create_node_with_props(&["Lieu"], [("pos", Value::from("pas un point"))]);
    let err = db
        .execute("MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= 100 RETURN n")
        .expect_err("le balayage refuse une valeur non-point");
    assert!(
        err.to_string().contains("point"),
        "le refus parle du point attendu : {err}"
    );
}

#[test]
fn un_rayon_negatif_rend_zero_ligne_et_reste_au_balayage() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL");
    // Un rayon négatif n'a pas de sens géométrique : le planificateur laisse la
    // requête au balayage, qui ne rend rien. Ce qui compte, c'est qu'il ne
    // l'envoie PAS à l'index avec un rayon que celui-ci interpréterait.
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= -1 RETURN n"
        ),
        0,
        "un rayon négatif ne rend aucune ligne"
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= 100 RETURN n"
        ),
        1,
        "le rayon valide, lui, rend bien le point"
    );
}

fn lignes(db: &ObrainDB, q: &str) -> usize {
    db.execute(q)
        .unwrap_or_else(|e| panic!("requête refusée : {q} — {e}"))
        .rows
        .len()
}

// ── Base sans substrat : refus explicites, jamais un index fantôme ─────────

#[test]
fn sans_substrat_l_api_refuse_de_creer_ou_nommer_un_index() {
    let (_td, db) = base_sans_substrat();
    let err = db
        .create_spatial_index("Lieu", "pos")
        .expect_err("pas d'index spatial sans substrat");
    assert!(
        err.to_string().contains("store externe"),
        "le refus dit pourquoi : {err}"
    );
    let err = db
        .name_spatial_index("lieux", "Lieu", "pos")
        .expect_err("pas de nom sans substrat");
    assert!(
        err.to_string().contains("store externe"),
        "le refus dit pourquoi : {err}"
    );
}

#[test]
fn sans_substrat_le_ddl_spatial_est_refuse() {
    let (_td, db) = base_sans_substrat();
    let err = db
        .execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect_err("DDL spatial sans substrat");
    assert!(
        err.to_string().contains("substrat"),
        "le refus nomme la condition : {err}"
    );
    // La session reste utilisable : un balayage ordinaire répond.
    assert_eq!(lignes(&db, "MATCH (n:Lieu) RETURN n"), 0);
}

// ── Entretien : un nœud qui REÇOIT le label après coup ─────────────────────

#[test]
fn recevoir_le_label_indexe_insere_un_point_et_ignore_le_reste() {
    let db = ObrainDB::new_in_memory();
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL");
    let avec_point = db.create_node_with_props(&["Brouillon"], [("pos", point(2.0, 48.0))]);
    let pas_un_point = db.create_node_with_props(&["Brouillon"], [("pos", Value::from("Paris"))]);
    let sans_pos = db.create_node_with_props(&["Brouillon"], [("nom", Value::from("x"))]);
    let vides = db
        .spatial_search("lieux_pos", 2.0, 48.0, 10.0)
        .expect("index");
    assert_eq!(vides.len(), 0, "aucun nœud n'a encore de point indexé");

    assert!(db.add_node_label(avec_point, "Lieu"));
    assert!(db.add_node_label(pas_un_point, "Lieu"));
    assert!(db.add_node_label(sans_pos, "Lieu"));

    let servis: Vec<_> = db
        .spatial_search("lieux_pos", 2.0, 48.0, 10.0)
        .expect("index")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        servis,
        vec![avec_point],
        "seul le nœud porteur d'un point entre dans l'index"
    );
}

// ── Nommage et kNN par l'API Rust ──────────────────────────────────────────

#[test]
fn l_api_nomme_un_index_et_refuse_un_nom_deja_pris_ailleurs() {
    let db = ObrainDB::new_in_memory();
    for i in 0..5 {
        db.create_node_with_props(
            &["Lieu"],
            [("pos", point(2.0 + f64::from(i) * 0.001, 48.0))],
        );
    }
    db.create_node_with_props(&["Gare"], [("pos", point(2.0, 48.0))]);

    assert_eq!(
        db.create_named_spatial_index("lieux", "Lieu", "pos")
            .expect("création"),
        5
    );
    // Même nom, même couple : idempotent.
    assert_eq!(
        db.create_named_spatial_index("lieux", "Lieu", "pos")
            .expect("idempotent"),
        5
    );
    let err = db
        .create_named_spatial_index("lieux", "Gare", "pos")
        .expect_err("le nom désigne déjà :Lieu(pos)");
    assert!(
        err.to_string().contains("Lieu"),
        "le refus nomme l'index existant : {err}"
    );

    // Nommer un index qui existe : idempotent, et résoluble par le nom.
    db.name_spatial_index("lieux", "Lieu", "pos")
        .expect("nom idempotent");
    assert_eq!(
        db.spatial_search("lieux", 2.0, 48.0, 1_000.0)
            .expect("par nom")
            .len(),
        5
    );
    // Nommer un couple sans index : refusé.
    assert!(db.name_spatial_index("gares", "Gare", "pos").is_err());
}

#[test]
fn le_knn_rust_rend_k_voisins_par_distance_croissante() {
    let db = ObrainDB::new_in_memory();
    let mut ids = Vec::new();
    for i in 0..6 {
        ids.push(
            db.create_node_with_props(&["Lieu"], [("pos", point(2.0 + f64::from(i) * 0.01, 48.0))]),
        );
    }
    db.create_spatial_index("Lieu", "pos").expect("index");
    let knn = db.spatial_knn("Lieu:pos", 2.0, 48.0, 3).expect("kNN");
    let rendus: Vec<_> = knn.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        rendus,
        ids[..3].to_vec(),
        "les trois plus proches, dans l'ordre"
    );
    assert!(
        knn.windows(2).all(|w| w[0].1 <= w[1].1),
        "distances croissantes : {knn:?}"
    );
    assert!(db.spatial_knn("Inconnu:pos", 2.0, 48.0, 3).is_err());
}

// ── DDL : nom pris, IF NOT EXISTS, réouverture ─────────────────────────────

#[test]
fn le_ddl_refuse_un_nom_deja_pris_et_if_not_exists_ne_reconstruit_pas() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL");
    let err = db
        .execute("CREATE INDEX lieux_pos FOR (n:Gare) ON (n.pos) USING SPATIAL")
        .expect_err("le nom désigne déjà :Lieu(pos)");
    assert!(
        err.to_string().contains("lieux_pos"),
        "le refus nomme le nom : {err}"
    );
    let r = db
        .execute("CREATE INDEX IF NOT EXISTS lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("IF NOT EXISTS");
    assert_eq!(r.status_message.as_deref(), Some("No change"));
}

#[test]
fn un_index_declare_se_rebatit_a_la_reouverture() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("base");
    let q = "MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= 100 RETURN n";
    {
        let db = ObrainDB::open(&chemin).expect("ouverture");
        db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
        db.create_node_with_props(&["Lieu"], [("pos", point(3.0, 49.0))]);
        db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
            .expect("DDL");
        assert_eq!(lignes(&db, q), 1);
        db.close().expect("fermeture");
    }
    let db = ObrainDB::open(&chemin).expect("réouverture");
    assert_eq!(
        db.spatial_search("lieux_pos", 2.0, 48.0, 100.0)
            .expect("index rebâti")
            .len(),
        1,
        "la déclaration durable rebâtit l'index"
    );
    assert_eq!(lignes(&db, q), 1);
}

// ── Formes laissées au balayage : le résultat reste celui du balayage ──────

#[test]
fn les_formes_non_routees_rendent_le_resultat_du_balayage() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(
        &["Lieu"],
        [
            ("pos", point(2.0, 48.0)),
            ("lon", Value::Float64(2.0)),
            ("r", Value::Float64(10.0)),
        ],
    );
    db.create_node_with_props(
        &["Lieu"],
        [
            ("pos", point(3.0, 49.0)),
            ("lon", Value::Float64(3.0)),
            ("r", Value::Float64(10.0)),
        ],
    );
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL");
    // Rayon lu sur le nœud : non constant, donc balayage.
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= n.r RETURN n"
        ),
        1
    );
    // Coordonnée du point lue sur le nœud : point non fixe, donc balayage.
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Lieu) WHERE distance(n.pos, point(n.lon, 48.0)) <= 10 RETURN n"
        ),
        1
    );
    // Deuxième argument qui n'est pas un point fixe : balayage.
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Lieu) WHERE distance(n.pos, n.pos) <= 1 RETURN n"
        ),
        2
    );
    // Liste écrite à la main : même résultat que point().
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Lieu) WHERE distance(n.pos, [2.0, 48.0]) <= 10 RETURN n"
        ),
        1
    );
}

#[test]
fn dans_une_transaction_l_index_respecte_la_visibilite() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL");
    let mut s = db.session();
    s.begin_transaction().expect("début");
    let r = s
        .execute("MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= 10 RETURN n")
        .expect("requête en transaction");
    assert_eq!(
        r.rows.len(),
        1,
        "le nœud validé avant la transaction est visible"
    );
    s.commit().expect("validation");
}

// ── Procédures CALL : arguments refusés avec un message qui dit quoi faire ─

#[cfg(feature = "algos")]
mod procedures {
    use super::*;

    fn base() -> ObrainDB {
        let db = ObrainDB::new_in_memory();
        for i in 0..4 {
            db.create_node_with_props(
                &["Lieu"],
                [("pos", point(-2.0 + f64::from(i) * 0.001, 48.0))],
            );
        }
        db.execute("CREATE INDEX lieux_pos FOR (n:Lieu) ON (n.pos) USING SPATIAL")
            .expect("DDL");
        db
    }

    fn refus(db: &ObrainDB, appel: &str, attendu: &str) {
        let q = format!("CALL {appel} YIELD node, distance RETURN node");
        let err = db
            .execute(&q)
            .expect_err(&format!("doit être refusé : {appel}"));
        assert!(
            err.to_string().contains(attendu),
            "{appel} : le message doit contenir « {attendu} », reçu : {err}"
        );
    }

    #[test]
    fn les_arguments_invalides_sont_refuses_et_nommes() {
        let db = base();
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', 2.0, 48.0)",
            "attend 4 arguments",
        );
        refus(
            &db,
            "obrain.spatial.within('lieux_pos', 2.0, 48.0)",
            "rayon_m",
        );
        refus(&db, "obrain.spatial.nearest(1, 2.0, 48.0, 3)", "index_name");
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', 'est', 48.0, 3)",
            "« lon »",
        );
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', 2.0, 95.0, 3)",
            "latitude",
        );
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', 2.0, 48.0, 2.5)",
            "entier positif",
        );
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', 2.0, 48.0, -1)",
            "entier positif",
        );
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', -'a', 48.0, 3)",
            "moins unaire",
        );
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', $lon, 48.0, 3)",
            "non fourni",
        );
        refus(
            &db,
            "obrain.spatial.nearest('lieux_pos', 1 + 1, 48.0, 3)",
            "constantes littérales",
        );
    }

    #[test]
    fn une_longitude_negative_litterale_est_acceptee() {
        let db = base();
        let r = db
            .execute(
                "CALL obrain.spatial.nearest('lieux_pos', -2.0, 48.0, 2) YIELD node, distance \
                 RETURN distance",
            )
            .expect("moins unaire sur un flottant");
        assert_eq!(r.rows.len(), 2);
        let premiere = r.rows[0]
            .first()
            .and_then(Value::as_float64)
            .expect("distance");
        assert!(
            premiere < 1.0,
            "le point (-2, 48) est à moins d'un mètre : {premiere}"
        );
    }

    #[test]
    fn sans_substrat_une_procedure_spatiale_est_refusee() {
        let (_td, db) = base_sans_substrat();
        let err = db
            .execute(
                "CALL obrain.spatial.nearest('lieux_pos', 2.0, 48.0, 3) YIELD node, distance \
                 RETURN node",
            )
            .expect_err("aucun index n'existe sans substrat");
        assert!(
            err.to_string().contains("index spatial inconnu"),
            "le refus dit que l'index n'existe pas : {err}"
        );
    }
}
