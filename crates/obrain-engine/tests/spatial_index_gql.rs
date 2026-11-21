//! G2 — Index spatial exposé en GQL : `CREATE INDEX ... USING SPATIAL`.
//!
//! Ce que ces tests prouvent, dans l'ordre où un utilisateur le découvre :
//!
//! 1. le DDL existe, il ÉVALUE (les autres sortes gardent l'erreur T17), et
//!    `SHOW INDEXES` le dit ;
//! 2. la requête par rayon rend EXACTEMENT ce que le prédicat `distance()` du
//!    moteur rend sur le même jeu — différentiel, pas relecture ;
//! 3. l'index survit à une fermeture/réouverture (déclaration durable) ;
//! 4. un nom inconnu est une ERREUR qui le nomme, jamais un résultat vide ;
//! 5. une écriture APRÈS la création est vue, un déplacement aussi, une
//!    suppression aussi.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn point(lon: f64, lat: f64) -> Value {
    Value::List(vec![Value::Float64(lon), Value::Float64(lat)].into())
}

/// 40 × 25 lieux en grille à 0,001° — la vraie donnée quantifiée (ex æquo
/// partout), pas un tirage uniforme qui n'existe nulle part en pratique.
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
        .expect("le DDL SPATIAL doit être ÉVALUÉ, pas refusé comme les autres sortes");
}

#[test]
fn le_ddl_cree_lindex_et_show_indexes_le_dit() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);
    let r = db.execute("SHOW INDEXES").expect("SHOW INDEXES");
    let noms: Vec<String> = r
        .rows
        .iter()
        .filter_map(|l| l.first().and_then(Value::as_str).map(str::to_string))
        .collect();
    assert!(
        noms.iter().any(|n| n == "lieux_pos"),
        "SHOW INDEXES doit lister l'index créé : {noms:?}"
    );
}

#[test]
fn le_rayon_de_lindex_egale_le_predicat_du_moteur() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);

    for (i, rayon) in [0.0_f64, 150.0, 700.0, 5_000.0].iter().enumerate() {
        let lon = 2.012 + i as f64 * 0.001;
        let lat = 48.010;

        // 1. L'INDEX : distance exacte, en mètres.
        let mut par_index: Vec<f64> = db
            .spatial_search("lieux_pos", lon, lat, *rayon)
            .expect("recherche par index")
            .into_iter()
            .map(|(_, d)| d)
            .collect();

        // 2. LE MOTEUR : le même prédicat, évalué par balayage GQL.
        let requete = format!(
            "MATCH (n:Lieu) WHERE distance(n.pos, point({lon}, {lat})) <= {rayon} \
             RETURN distance(n.pos, point({lon}, {lat})) AS d"
        );
        let mut par_moteur: Vec<f64> = db
            .execute(&requete)
            .expect("le prédicat distance() doit s'évaluer en GQL")
            .rows
            .iter()
            .filter_map(|l| {
                l.first()
                    .and_then(|v| v.as_float64().or_else(|| v.as_int64().map(|i| i as f64)))
            })
            .collect();

        assert_eq!(
            par_index.len(),
            par_moteur.len(),
            "rayon {rayon} : {} par l'index, {} par le moteur",
            par_index.len(),
            par_moteur.len()
        );

        // Les deux listes sont triées par distance croissante : on compare
        // terme à terme, au millionième de mètre.
        par_index.sort_by(|a, b| a.total_cmp(b));
        par_moteur.sort_by(|a, b| a.total_cmp(b));
        for (k, (a, b)) in par_index.iter().zip(par_moteur.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "rayon {rayon}, {k}e distance : index {a} m, moteur {b} m"
            );
        }
        assert!(
            par_index.windows(2).all(|w| w[0] <= w[1]),
            "rayon {rayon} : l'index rend des distances non monotones"
        );
    }
}

#[test]
fn lindex_survit_a_une_reouverture() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let chemin = tmp.path().join("base");
    {
        let db = ObrainDB::open(&chemin).expect("ouverture");
        peupler(&db, "Lieu");
        creer_index(&db);
        assert_eq!(
            db.spatial_search("lieux_pos", 2.0, 48.0, 1.0)
                .unwrap()
                .len(),
            1,
            "avant fermeture : un seul lieu à moins d'un mètre"
        );
    }
    // Rouverte : la structure n'est pas en mémoire, mais la DÉCLARATION l'est —
    // l'index doit se rebâtir à la première requête, sous le même nom.
    let db = ObrainDB::open(&chemin).expect("réouverture");
    let noms = db.spatial_index_names();
    assert_eq!(noms.len(), 1, "le nom doit survivre : {noms:?}");
    assert_eq!(noms[0].0, "lieux_pos");
    let r = db
        .spatial_search("lieux_pos", 2.0, 48.0, 1.0)
        .expect("l'index déclaré doit se rebâtir à la demande");
    assert_eq!(r.len(), 1, "même réponse après réouverture : {r:?}");
}

#[test]
fn nom_inconnu_est_une_erreur_qui_le_nomme() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    let err = db
        .spatial_search("fantome", 2.0, 48.0, 10.0)
        .expect_err("un nom inconnu doit être une erreur, pas un résultat vide")
        .to_string();
    assert!(
        err.contains("fantome"),
        "l'erreur doit nommer l'index : {err}"
    );
}

#[test]
fn une_ecriture_apres_la_creation_est_vue_puis_un_deplacement_et_une_suppression() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);

    // 1. Création APRÈS l'index : la surcouche doit la porter.
    let ajoute = db.create_node_with_props(&["Lieu"], [("pos", point(2.0005, 48.0005))]);
    let vus = db.spatial_search("lieux_pos", 2.0, 48.0, 100.0).unwrap();
    assert!(
        vus.iter().any(|(id, _)| id.0 == ajoute.0),
        "un nœud créé après l'index doit être visible : {vus:?}"
    );

    // 2. Déplacement : la NOUVELLE position compte, l'ancienne ne répond plus.
    db.set_node_property(ajoute, "pos", point(3.0, 49.0));
    let ancien_lieu = db.spatial_search("lieux_pos", 2.0, 48.0, 100.0).unwrap();
    assert!(
        !ancien_lieu.iter().any(|(id, _)| id.0 == ajoute.0),
        "l'ancienne position ne doit plus répondre : {ancien_lieu:?}"
    );
    let nouveau_lieu = db.spatial_search("lieux_pos", 3.0, 49.0, 100.0).unwrap();
    assert!(
        nouveau_lieu.iter().any(|(id, _)| id.0 == ajoute.0),
        "la nouvelle position doit répondre : {nouveau_lieu:?}"
    );

    // 3. Propriété retirée : le nœud sort de l'index (il n'a plus de lieu).
    db.set_node_property(ajoute, "pos", Value::Null);
    let apres = db.spatial_search("lieux_pos", 3.0, 49.0, 1_000.0).unwrap();
    assert!(
        !apres.iter().any(|(id, _)| id.0 == ajoute.0),
        "un nœud sans point ne doit plus être dans l'index : {apres:?}"
    );

    // 4. Suppression.
    db.set_node_property(ajoute, "pos", point(3.0, 49.0));
    db.delete_node(ajoute);
    let apres = db.spatial_search("lieux_pos", 3.0, 49.0, 1_000.0).unwrap();
    assert!(
        !apres.iter().any(|(id, _)| id.0 == ajoute.0),
        "un nœud supprimé ne doit plus répondre : {apres:?}"
    );
}

#[test]
fn un_index_sur_un_autre_label_ne_recoit_pas_le_noeud() {
    let db = ObrainDB::new_in_memory();
    peupler(&db, "Lieu");
    creer_index(&db);
    // Même nom de propriété, autre label : il ne doit PAS entrer dans l'index.
    db.create_node_with_props(&["Batiment"], [("pos", point(2.0, 48.0))]);
    let r = db.spatial_search("lieux_pos", 2.0, 48.0, 1.0).unwrap();
    assert_eq!(r.len(), 1, "seul le :Lieu compte, pas le :Batiment : {r:?}");
}

#[test]
fn le_ddl_refuse_deux_proprietes_et_le_dit() {
    let db = ObrainDB::new_in_memory();
    let err = db
        .execute("CREATE INDEX trop FOR (n:Lieu) ON (n.pos, n.nom) USING SPATIAL")
        .expect_err("un index spatial porte sur UNE propriété");
    assert!(
        err.to_string().contains("UNE propriété"),
        "l'erreur doit être explicite : {err}"
    );
}
