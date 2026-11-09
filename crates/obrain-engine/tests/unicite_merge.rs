//! Unicite MERGE (mission perf, feature 2.1) : un MERGE ne cree plus
//! JAMAIS un second porteur vivant du meme motif — y compris sous
//! concurrence (le chemin de secours createur des 46 doublons par id
//! logique mesures sur la base agent).
//!
//! CREATE explicite reste PERMISSIF (outil imperatif, nomme) : seul MERGE
//! porte la garantie.

#![cfg(feature = "gql")]

use obrain_engine::ObrainDB;

/// 16 fils x 12 MERGE du meme id : UN seul porteur a la fin, zero echec.
#[test]
fn merge_concurrent_ne_cree_jamais_de_doublon() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    let mut hs = Vec::new();
    for t in 0..16 {
        let db = std::sync::Arc::clone(&db);
        hs.push(std::thread::spawn(move || {
            let s = db.session();
            for k in 0..12 {
                s.execute(&format!(
                    "MERGE (n:Cible {{id: \x27unique\x27}}) SET n.v = {t}{k} RETURN n.id"
                ))
                .expect("merge concurrent : zero echec attendu (re-tentative auto-commit)");
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let porteurs = db.graph_store().nodes_by_label("Cible");
    assert_eq!(
        porteurs.len(),
        1,
        "16 fils x 12 MERGE du meme id doivent laisser UN porteur, trouve {}",
        porteurs.len()
    );
}

/// Idem avec index de propriete (le chemin chaud de production).
#[test]
fn merge_concurrent_avec_index_reste_unique() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    db.create_property_index("id");
    let mut hs = Vec::new();
    for t in 0..16 {
        let db = std::sync::Arc::clone(&db);
        hs.push(std::thread::spawn(move || {
            let s = db.session();
            for k in 0..12 {
                s.execute(&format!(
                    "MERGE (n:CibleIdx {{id: \x27unique\x27}}) SET n.v = {t}{k} RETURN n.id"
                ))
                .expect("merge indexe");
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(db.graph_store().nodes_by_label("CibleIdx").len(), 1);
}

/// (CONC-1) Le PERDANT d un MERGE concurrent attend le porteur actif au
/// lieu de remonter `WriteConflict` : une session explicite tient une
/// ecriture sur le noeud pendant 150 ms (plus que les 22-45 ms que
/// couvraient les 12 re-tentatives historiques) ; le MERGE auto-commit de
/// l autre session doit finir par REUTILISER ce noeud et poser son SET.
/// Deterministe : le conflit est injecte, pas provoque par la charge.
#[test]
fn merge_perdant_attend_le_porteur_actif_et_reutilise_son_noeud() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    db.create_property_index("id");
    let mut porteur = db.session();
    porteur
        .execute("CREATE (:Porteur {id: \x27x\x27, v: 0})")
        .expect("create porteur");
    porteur.begin_transaction().expect("begin");
    porteur
        .execute("MATCH (n:Porteur {id: \x27x\x27}) SET n.v = 1")
        .expect("set sous transaction explicite (tient l ecriture)");

    let db2 = std::sync::Arc::clone(&db);
    let perdant = std::thread::spawn(move || {
        let s = db2.session();
        let debut = std::time::Instant::now();
        let r = s.execute("MERGE (n:Porteur {id: \x27x\x27}) SET n.v = 2 RETURN n.id");
        (r, debut.elapsed())
    });

    std::thread::sleep(std::time::Duration::from_millis(150));
    porteur.commit().expect("commit du porteur");
    let (r, attente) = perdant.join().unwrap();
    let r = r.expect("le MERGE perdant doit reussir une fois le porteur valide");
    assert_eq!(r.rows.len(), 1);
    assert!(
        attente >= std::time::Duration::from_millis(100),
        "le perdant a du ATTENDRE le porteur (conflit write-write detecte), attente {attente:?}"
    );
    assert_eq!(db.graph_store().nodes_by_label("Porteur").len(), 1);
    let v = db
        .session()
        .execute("MATCH (n:Porteur {id: \x27x\x27}) RETURN n.v")
        .expect("lecture");
    assert_eq!(
        v.rows[0][0],
        obrain_common::types::Value::Int64(2),
        "le SET du perdant, rejoue apres le commit du porteur, est le dernier"
    );
}

/// (CONC-1) La FILE des rejoues : deux perdants bloques par le meme
/// porteur explicite (150 ms) se rangent l un derriere l autre et passent
/// TOUS LES DEUX une fois le porteur valide — aucun n echoue, un seul
/// porteur du motif a la fin, et le dernier SET valide est l un des deux.
#[test]
fn deux_perdants_en_file_passent_l_un_apres_l_autre() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    db.create_property_index("id");
    let mut porteur = db.session();
    porteur
        .execute("CREATE (:Filee {id: \x27x\x27, v: 0})")
        .expect("create");
    porteur.begin_transaction().expect("begin");
    porteur
        .execute("MATCH (n:Filee {id: \x27x\x27}) SET n.v = 1")
        .expect("set tenu");

    let perdants: Vec<_> = [2, 3]
        .into_iter()
        .map(|v| {
            let db = std::sync::Arc::clone(&db);
            std::thread::spawn(move || {
                db.session()
                    .execute(&format!(
                        "MERGE (n:Filee {{id: \x27x\x27}}) SET n.v = {v} RETURN n.id"
                    ))
                    .map(|r| r.rows.len())
            })
        })
        .collect();
    std::thread::sleep(std::time::Duration::from_millis(150));
    porteur.commit().expect("commit du porteur");
    for p in perdants {
        assert_eq!(p.join().unwrap().expect("chaque perdant passe"), 1);
    }
    assert_eq!(db.graph_store().nodes_by_label("Filee").len(), 1);
    let v = db
        .session()
        .execute("MATCH (n:Filee {id: \x27x\x27}) RETURN n.v")
        .expect("lecture");
    assert!(
        matches!(v.rows[0][0], obrain_common::types::Value::Int64(2 | 3)),
        "le dernier SET valide est celui d un perdant : {:?}",
        v.rows[0][0]
    );
}

/// (CONC-1) `query_timeout` PLAFONNE le budget de re-tentative : face a un
/// porteur qui ne lache pas, le MERGE echoue a l echeance de la requete
/// (200 ms), bien avant le budget d une seconde.
#[test]
fn merge_en_conflit_soutenu_echoue_a_l_echeance_de_la_requete() {
    let db = std::sync::Arc::new(
        ObrainDB::new_in_memory().with_query_timeout(Some(std::time::Duration::from_millis(200))),
    );
    let mut porteur = db.session();
    porteur
        .execute("CREATE (:Tenu {id: \x27x\x27})")
        .expect("create");
    porteur.begin_transaction().expect("begin");
    porteur
        .execute("MATCH (n:Tenu {id: \x27x\x27}) SET n.v = 1")
        .expect("set tenu");

    let db2 = std::sync::Arc::clone(&db);
    let perdant = std::thread::spawn(move || {
        let s = db2.session();
        let debut = std::time::Instant::now();
        let r = s.execute("MERGE (n:Tenu {id: \x27x\x27}) SET n.v = 2 RETURN n.id");
        (r, debut.elapsed())
    });
    let (r, attente) = perdant.join().unwrap();
    assert!(r.is_err(), "porteur jamais valide : l erreur doit remonter");
    assert!(
        attente >= std::time::Duration::from_millis(150)
            && attente < std::time::Duration::from_millis(900),
        "l echeance de la requete (200 ms) borne l attente, pas le budget d une seconde : {attente:?}"
    );
    porteur.commit().expect("commit tardif");
}

/// (CONC-1) Sans `query_timeout`, le budget d une seconde reste une ERREUR
/// franche : une transaction explicite oubliee ouverte 1,5 s fait echouer
/// le MERGE concurrent apres ~1 s, pas jamais.
#[test]
fn merge_en_conflit_soutenu_echoue_apres_le_budget() {
    let db = std::sync::Arc::new(ObrainDB::new_in_memory());
    let mut porteur = db.session();
    porteur
        .execute("CREATE (:Oublie {id: \x27x\x27})")
        .expect("create");
    porteur.begin_transaction().expect("begin");
    porteur
        .execute("MATCH (n:Oublie {id: \x27x\x27}) SET n.v = 1")
        .expect("set tenu");

    let db2 = std::sync::Arc::clone(&db);
    let perdant = std::thread::spawn(move || {
        let s = db2.session();
        let debut = std::time::Instant::now();
        let r = s.execute("MERGE (n:Oublie {id: \x27x\x27}) SET n.v = 2 RETURN n.id");
        (r, debut.elapsed())
    });
    std::thread::sleep(std::time::Duration::from_millis(1500));
    porteur.commit().expect("commit tardif");
    let (r, attente) = perdant.join().unwrap();
    let e = r.expect_err("porteur tenu 1,5 s : le budget d une seconde est epuise");
    assert!(
        e.to_string().contains("OBRAIN-T001") || e.to_string().contains("Write conflict"),
        "l erreur remontee est le conflit lui-meme : {e}"
    );
    assert!(
        attente >= std::time::Duration::from_millis(1000)
            && attente < std::time::Duration::from_millis(1500),
        "attente attendue entre 1 s (budget) et 1,5 s (commit du porteur) : {attente:?}"
    );
}

/// Sur des doublons DEJA presents (pollution historique), MERGE cible
/// toujours le MEME porteur (plus petit NodeId) — stable et convergent.
#[test]
fn merge_sur_doublons_existants_est_deterministe() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    // Trois doublons poses par CREATE explicite (permissif, nomme).
    for k in 0..3 {
        s.execute(&format!("CREATE (:Doublon {{id: \x27dup\x27, ne: {k}}})"))
            .expect("create doublon");
    }
    assert_eq!(db.graph_store().nodes_by_label("Doublon").len(), 3);
    for _ in 0..5 {
        s.execute("MERGE (n:Doublon {id: \x27dup\x27}) SET n.touche = true RETURN n.id")
            .expect("merge");
    }
    // Le porteur touche est UNIQUE et c est le plus petit NodeId.
    let r = s
        .execute("MATCH (n:Doublon) WHERE n.touche = true RETURN n.ne")
        .expect("verif");
    assert_eq!(r.rows.len(), 1, "un seul porteur doit recevoir les SET");
    assert_eq!(
        r.rows[0][0],
        obrain_common::types::Value::Int64(0),
        "regle min-NodeId : le premier-ne est la cible stable"
    );
}

/// CREATE explicite : deux CREATE du meme id posent DEUX noeuds —
/// tolerance nommee (outil imperatif), la garantie ne porte que sur MERGE.
#[test]
fn create_explicite_reste_permissif_et_nomme() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("CREATE (:Libre {id: \x27meme\x27})")
        .expect("create 1");
    s.execute("CREATE (:Libre {id: \x27meme\x27})")
        .expect("create 2");
    assert_eq!(db.graph_store().nodes_by_label("Libre").len(), 2);
}
