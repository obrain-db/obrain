//! Parité INDEX ≡ BALAYAGE : erreurs, valeurs non point, labels, DROP.
//!
//! Scénarios issus de la revue indépendante de #282 (21/11/2026 — le fichier
//! `review_282_extra.rs` du relecteur, repris ici avec ses cas et complété).
//! Règle : un index ne change JAMAIS le résultat d'une requête — ni ses
//! lignes, ni son erreur, ni le texte de l'erreur. La forme
//! `distance(point(..), n.pos)` (point en premier) n'est pas routée : elle sert
//! de balayage témoin quand on compare les deux chemins sur la même base.
//!
//! Lancer : `cargo test -p obrain-engine --features cypher --test spatial_parite_balayage`
use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn point(lon: f64, lat: f64) -> Value {
    Value::List(vec![Value::Float64(lon), Value::Float64(lat)].into())
}

fn creer_index(db: &ObrainDB) {
    db.execute("CREATE INDEX geo FOR (n:Lieu) ON (n.pos) USING SPATIAL")
        .expect("DDL SPATIAL");
}

/// Le résultat d'une requête réduit à ce qui doit être identique entre les
/// deux chemins : le nombre de lignes, ou le texte exact de l'erreur.
fn compte(db: &ObrainDB, q: &str) -> Result<usize, String> {
    db.execute(q)
        .map(|r| r.rows.len())
        .map_err(|e| format!("{e:?}"))
}

const INDEX_100: &str = "MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 48.0)) <= 100 RETURN n";
const SCAN_100: &str = "MATCH (n:Lieu) WHERE distance(point(2.0, 48.0), n.pos) <= 100 RETURN n";

// ── Valeurs qui ne sont pas des points ─────────────────────────────────────

#[test]
fn valeur_non_point_le_balayage_refuse_et_lindex_aussi_avec_le_meme_texte() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", Value::from("oops"))]);
    let avant = compte(&db, INDEX_100);
    creer_index(&db);
    let apres = compte(&db, INDEX_100);
    assert!(
        avant.is_err(),
        "le balayage refuse une valeur non point : {avant:?}"
    );
    assert_eq!(
        apres, avant,
        "l'index ne doit ni cacher l'erreur ni la reformuler"
    );
    assert!(
        avant
            .as_ref()
            .unwrap_err()
            .contains("distance() attend deux points"),
        "{avant:?}"
    );
}

#[test]
fn liste_de_trois_nombres_est_refusee_des_deux_cotes() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(
        &["Lieu"],
        [(
            "pos",
            Value::List(
                vec![
                    Value::Float64(1.0),
                    Value::Float64(2.0),
                    Value::Float64(3.0),
                ]
                .into(),
            ),
        )],
    );
    creer_index(&db);
    let (index, scan) = (compte(&db, INDEX_100), compte(&db, SCAN_100));
    assert!(scan.is_err(), "{scan:?}");
    assert_eq!(index, scan);
}

#[test]
fn valeur_non_point_corrigee_lindex_sert_a_nouveau_les_memes_lignes() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(&["Lieu"], [("pos", Value::from("oops"))]);
    creer_index(&db);
    assert!(compte(&db, INDEX_100).is_err());
    db.execute("MATCH (n:Lieu) WHERE n.pos = 'oops' SET n.pos = point(2.0, 48.0)")
        .expect("correction");
    assert_eq!(compte(&db, INDEX_100), Ok(2));
    assert_eq!(compte(&db, SCAN_100), Ok(2));
}

#[test]
fn valeur_non_point_retiree_ou_mise_a_null_le_balayage_et_lindex_concordent() {
    for correction in [
        "MATCH (n:Lieu) WHERE n.pos = 'oops' REMOVE n.pos",
        "MATCH (n:Lieu) WHERE n.pos = 'oops' SET n.pos = NULL",
    ] {
        let db = ObrainDB::new_in_memory();
        db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
        db.create_node_with_props(&["Lieu"], [("pos", Value::from("oops"))]);
        creer_index(&db);
        assert!(compte(&db, INDEX_100).is_err(), "{correction}");
        db.execute(correction).expect(correction);
        assert_eq!(compte(&db, INDEX_100), Ok(1), "{correction}");
        assert_eq!(compte(&db, SCAN_100), Ok(1), "{correction}");
    }
}

#[test]
fn noeud_non_point_supprime_le_balayage_et_lindex_concordent() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(&["Lieu"], [("pos", Value::from("oops"))]);
    creer_index(&db);
    assert!(compte(&db, INDEX_100).is_err());
    db.execute("MATCH (n:Lieu) WHERE n.pos = 'oops' DELETE n")
        .expect("suppression");
    assert_eq!(compte(&db, INDEX_100), Ok(1));
    assert_eq!(compte(&db, SCAN_100), Ok(1));
}

#[test]
fn valeur_non_point_sur_un_autre_label_ne_concerne_pas_lindex() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(&["Autre"], [("pos", Value::from("oops"))]);
    creer_index(&db);
    assert_eq!(compte(&db, INDEX_100), Ok(1));
    assert_eq!(compte(&db, SCAN_100), Ok(1));
}

#[test]
fn valeur_null_nest_ni_un_point_ni_une_faute_des_deux_cotes() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(&["Lieu"], [("pos", Value::Null)]);
    db.create_node(&["Lieu"]);
    creer_index(&db);
    assert_eq!(compte(&db, INDEX_100), Ok(1));
    assert_eq!(compte(&db, SCAN_100), Ok(1));
}

// ── Le point fixe du prédicat ──────────────────────────────────────────────

#[test]
fn point_fixe_invalide_le_balayage_refuse_et_lindex_aussi_avec_le_meme_texte() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    let q = "MATCH (n:Lieu) WHERE distance(n.pos, point(2.0, 200.0)) <= 100 RETURN n";
    let avant = compte(&db, q);
    creer_index(&db);
    let apres = compte(&db, q);
    assert!(avant.is_err(), "{avant:?}");
    assert!(
        avant.as_ref().unwrap_err().contains("latitude 200"),
        "{avant:?}"
    );
    assert_eq!(apres, avant);
}

#[test]
fn liste_litterale_hors_domaine_nest_refusee_par_personne_et_rend_pareil() {
    // `[lon, lat]` écrit directement n'est pas validé par le balayage non
    // plus (`distance()` accepte deux nombres) : les deux chemins calculent.
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 200.0))]);
    let index = "MATCH (n:Lieu) WHERE distance(n.pos, [2.0, 200.0]) <= 100 RETURN n";
    let scan = "MATCH (n:Lieu) WHERE distance([2.0, 200.0], n.pos) <= 100 RETURN n";
    let avant = compte(&db, index);
    creer_index(&db);
    assert_eq!(compte(&db, index), avant);
    assert_eq!(compte(&db, scan), avant);
    assert!(avant.is_ok(), "{avant:?}");
}

// ── Transactions ───────────────────────────────────────────────────────────

#[test]
fn rollback_dun_deplacement_remet_lindex_au_niveau_du_balayage() {
    let db = ObrainDB::new_in_memory();
    let id = db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    let mut session = db.session();
    session.begin_transaction().expect("begin");
    session.set_node_property(id, "pos", point(3.0, 49.0));
    session.rollback().expect("rollback");
    let index = session.execute(INDEX_100).expect("index").rows.len();
    let scan = session.execute(SCAN_100).expect("balayage").rows.len();
    assert_eq!(index, scan);
    assert_eq!(scan, 1, "le rollback a remis la position d'origine");
}

#[test]
fn un_instantane_voit_les_memes_valeurs_avec_et_sans_index() {
    let db = ObrainDB::new_in_memory();
    let id = db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    let mut lecture = db.session();
    lecture.begin_transaction().expect("begin");
    db.set_node_property(id, "pos", point(3.0, 49.0));
    let index = lecture.execute(INDEX_100).expect("index").rows.len();
    let scan = lecture.execute(SCAN_100).expect("balayage").rows.len();
    assert_eq!(
        index, scan,
        "instantané : l'index et le balayage voient la même base"
    );
    lecture.rollback().expect("rollback");
}

// ── Labels ─────────────────────────────────────────────────────────────────

#[test]
fn label_ajoute_en_gql_entre_dans_lindex() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Autre"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    db.execute("MATCH (n:Autre) SET n:Lieu").expect("SET label");
    assert_eq!(compte(&db, SCAN_100), Ok(1));
    assert_eq!(
        compte(&db, INDEX_100),
        Ok(1),
        "le nœud devenu :Lieu est servi par l'index"
    );
}

#[test]
fn label_retire_sort_de_lindex() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    db.execute("MATCH (n:Lieu) REMOVE n:Lieu")
        .expect("REMOVE label");
    assert_eq!(compte(&db, SCAN_100), Ok(0));
    assert_eq!(compte(&db, INDEX_100), Ok(0));
}

#[test]
fn label_retire_position_changee_puis_label_remis() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    db.execute("MATCH (n:Lieu) REMOVE n:Lieu SET n:Autre")
        .expect("hors du label");
    db.execute("MATCH (n:Autre) SET n.pos = point(3.0, 49.0)")
        .expect("déplacement");
    db.execute("MATCH (n:Autre) SET n:Lieu")
        .expect("retour dans le label");
    assert_eq!(
        compte(&db, SCAN_100),
        Ok(0),
        "la nouvelle position est loin"
    );
    assert_eq!(
        compte(&db, INDEX_100),
        Ok(0),
        "l'ancienne position ne doit pas être servie"
    );
    let pres = "MATCH (n:Lieu) WHERE distance(n.pos, point(3.0, 49.0)) <= 1 RETURN n";
    assert_eq!(compte(&db, pres), Ok(1));
}

#[test]
fn label_ajoute_a_un_noeud_a_valeur_non_point_fait_replier_lindex() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    db.create_node_with_props(&["Autre"], [("pos", Value::from("oops"))]);
    creer_index(&db);
    assert_eq!(compte(&db, INDEX_100), Ok(1));
    db.execute("MATCH (n:Autre) SET n:Lieu").expect("SET label");
    let (index, scan) = (compte(&db, INDEX_100), compte(&db, SCAN_100));
    assert!(scan.is_err(), "{scan:?}");
    assert_eq!(index, scan);
}

// ── Chaîne GQL : DROP, et pas d'index implicite par l'API Rust ─────────────

#[test]
fn drop_index_retire_un_index_spatial() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    assert_eq!(db.spatial_index_names().len(), 1);
    db.execute("DROP INDEX geo")
        .expect("DROP INDEX doit retirer un index créé par GQL");
    assert_eq!(db.spatial_index_names(), Vec::new());
    assert_eq!(
        compte(&db, INDEX_100),
        Ok(1),
        "sans index, le balayage sert"
    );
}

#[test]
fn drop_index_if_exists_retire_un_index_spatial_present() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    db.execute("DROP INDEX IF EXISTS geo")
        .expect("DROP IF EXISTS");
    assert_eq!(
        db.spatial_index_names(),
        Vec::new(),
        "« No change » serait un mensonge"
    );
}

#[test]
fn drop_index_if_exists_sans_index_spatial_garde_son_comportement() {
    let db = ObrainDB::new_in_memory();
    let r = db
        .execute("DROP INDEX IF EXISTS inexistant")
        .expect("idempotent");
    assert_eq!(r.status_message.as_deref(), Some("No change"));
}

#[test]
fn api_rust_ne_batit_pas_dindex_non_declare() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    let r = db.spatial_search("Lieu:pos", 2.0, 48.0, 100.0);
    assert!(
        r.is_err(),
        "un index non déclaré ne doit pas être construit implicitement : {r:?}"
    );
    assert_eq!(db.spatial_index_names(), Vec::new());
}

// ── Rollback : l'index suit l'état restauré ────────────────────────────────

#[test]
fn rollback_dune_suppression_de_noeud_remet_le_point() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    let mut session = db.session();
    session.begin_transaction().expect("begin");
    session
        .execute("MATCH (n:Lieu) DETACH DELETE n")
        .expect("delete");
    session.rollback().expect("rollback");
    assert_eq!(compte(&db, SCAN_100), Ok(1), "le rollback restaure le nœud");
    assert_eq!(compte(&db, INDEX_100), Ok(1), "l'index doit le réapprendre");
}

#[test]
fn rollback_dun_ajout_de_label_retire_le_point() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Autre"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    let mut session = db.session();
    session.begin_transaction().expect("begin");
    session
        .execute("MATCH (n:Autre) SET n:Lieu")
        .expect("SET label");
    session.rollback().expect("rollback");
    assert_eq!(compte(&db, SCAN_100), Ok(0));
    assert_eq!(compte(&db, INDEX_100), Ok(0));
}

#[test]
fn rollback_dun_retrait_de_label_remet_le_point() {
    let db = ObrainDB::new_in_memory();
    db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
    creer_index(&db);
    let mut session = db.session();
    session.begin_transaction().expect("begin");
    session
        .execute("MATCH (n:Lieu) REMOVE n:Lieu")
        .expect("REMOVE label");
    session.rollback().expect("rollback");
    assert_eq!(compte(&db, SCAN_100), Ok(1));
    assert_eq!(compte(&db, INDEX_100), Ok(1));
}

// ── DROP durable ───────────────────────────────────────────────────────────

#[test]
fn drop_dun_index_non_charge_vaut_apres_reouverture() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let chemin = tmp.path().join("base");
    {
        let db = ObrainDB::open(&chemin).expect("ouverture");
        db.create_node_with_props(&["Lieu"], [("pos", point(2.0, 48.0))]);
        creer_index(&db);
    }
    {
        // Rouverte : l'index est DÉCLARÉ mais pas chargé en mémoire.
        let db = ObrainDB::open(&chemin).expect("réouverture");
        assert_eq!(db.spatial_index_names().len(), 1);
        db.execute("DROP INDEX geo")
            .expect("DROP d'un index non chargé");
        assert_eq!(db.spatial_index_names(), Vec::new());
    }
    let db = ObrainDB::open(&chemin).expect("seconde réouverture");
    assert_eq!(
        db.spatial_index_names(),
        Vec::new(),
        "la déclaration ne doit pas revenir"
    );
    assert!(db.spatial_search("geo", 2.0, 48.0, 1.0).is_err());
    assert_eq!(compte(&db, INDEX_100), Ok(1), "le balayage sert");
}
