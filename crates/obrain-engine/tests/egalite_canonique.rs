//! L'égalité CANONIQUE de bout en bout (PR #101) : `5` et `5.0`, deux
//! écritures d'un même instant, désignent le même porteur pour `MATCH`,
//! pour `MERGE`, pour une jointure par sonde d'index — avec ou sans index
//! d'égalité déclaré, en mémoire comme après réouverture.
//!
//! Avant : l'index d'égalité rangeait ces valeurs dans des seaux distincts
//! et `MERGE` revérifiait par l'égalité stricte de `Value`. `MATCH` trouvait
//! le nœud (par balayage) là où `MERGE` en créait un second.

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn compte(db: &ObrainDB, label: &str) -> usize {
    db.graph_store().nodes_by_label(label).len()
}

fn un_entier(db: &ObrainDB, requete: &str) -> i64 {
    let r = db.session().execute(requete).expect(requete);
    match r.rows.first().and_then(|l| l.first()) {
        Some(Value::Int64(n)) => *n,
        autre => panic!("{requete} : attendu un entier, reçu {autre:?}"),
    }
}

/// `MERGE` sur `5` ne crée pas un second porteur quand le nœud porte `5.0`
/// (et inversement), index déclaré ou non.
#[test]
fn merge_entier_contre_flottant_ne_cree_pas_de_doublon() {
    for indexe in [false, true] {
        let db = ObrainDB::new_in_memory();
        if indexe {
            db.create_property_index("x");
        }
        let s = db.session();
        s.execute("CREATE (:Num {x: 5.0})").unwrap();
        s.execute("CREATE (:Num {x: 7})").unwrap();
        s.execute("MERGE (n:Num {x: 5}) RETURN n.x").unwrap();
        s.execute("MERGE (n:Num {x: 7.0}) RETURN n.x").unwrap();
        assert_eq!(
            compte(&db, "Num"),
            2,
            "indexe={indexe} : MERGE a créé un doublon d'un nœud que MATCH trouve"
        );
        assert_eq!(
            un_entier(&db, "MATCH (n:Num {x: 5}) RETURN count(n)"),
            1,
            "indexe={indexe}"
        );
        // Un flottant NON entier reste un autre porteur.
        s.execute("MERGE (n:Num {x: 5.5}) RETURN n.x").unwrap();
        assert_eq!(compte(&db, "Num"), 3, "indexe={indexe}");
    }
}

/// `MERGE` sur une date écrite dans un autre fuseau retrouve le porteur du
/// même instant.
#[test]
fn merge_meme_instant_autre_fuseau_ne_cree_pas_de_doublon() {
    for indexe in [false, true] {
        let db = ObrainDB::new_in_memory();
        if indexe {
            db.create_property_index("d");
        }
        let s = db.session();
        s.execute("CREATE (:Evt {d: '2024-01-01T10:00:00Z'})")
            .unwrap();
        s.execute("MERGE (n:Evt {d: '2024-01-01T12:00:00+02:00'}) RETURN n.d")
            .unwrap();
        assert_eq!(compte(&db, "Evt"), 1, "indexe={indexe}");
        // Un autre instant est un autre porteur — y compris hors de la plage
        // où l'ancien instant saturait.
        s.execute("MERGE (n:Evt {d: '3000-01-01T00:00:00Z'}) RETURN n.d")
            .unwrap();
        s.execute("MERGE (n:Evt {d: '9999-12-31T23:59:59Z'}) RETURN n.d")
            .unwrap();
        assert_eq!(compte(&db, "Evt"), 3, "indexe={indexe}");
    }
}

/// Jointure par sonde d'index corrélée : une clé externe flottante entière
/// trouve le porteur entier, et inversement.
#[test]
fn jointure_par_sonde_d_index_avec_cle_flottante_entiere() {
    let db = ObrainDB::new_in_memory();
    db.create_property_index("k");
    let s = db.session();
    s.execute("CREATE (:Ext {fk: 5.0})").unwrap();
    s.execute("CREATE (:Ext {fk: 7})").unwrap();
    s.execute("CREATE (:Ext {fk: 8.5})").unwrap();
    s.execute("CREATE (:Cible {k: 5})").unwrap();
    s.execute("CREATE (:Cible {k: 7.0})").unwrap();
    s.execute("CREATE (:Cible {k: 8})").unwrap();
    assert_eq!(
        un_entier(
            &db,
            "MATCH (a:Ext) MATCH (b:Cible {k: a.fk}) RETURN count(b)"
        ),
        2,
        "5.0 ⋈ 5 et 7 ⋈ 7.0 ; 8.5 ne joint rien"
    );
}

/// Base sur disque dont le fichier de zone d'égalité a disparu : à la
/// réouverture, l'index déclaré n'est couvert par rien. Le moteur le prépare
/// hors du chemin des requêtes ; la requête est juste dans tous les cas, et
/// la base se ferme puis se rouvre normalement (le fil de préparation ne
/// retient pas le substrat).
#[test]
fn index_declare_non_couvert_est_prepare_et_la_base_se_referme() {
    let d = tempfile::tempdir().unwrap();
    let chemin = d.path().join("base");
    {
        let db = ObrainDB::open(&chemin).unwrap();
        // Au-dessus de `SEUIL_NOEUDS_PASSE_HATIVE` : l'ouverture ne
        // bâtit PAS les index déclarés d'emblée, c'est le fil de préparation
        // qui doit le faire. En dessous, une passe à l'ouverture le ferait et
        // le fil n'aurait rien à prouver.
        let store = db.graph_store_mut();
        let n = i64::try_from(obrain_substrate::store::SEUIL_NOEUDS_PASSE_HATIVE + 1).unwrap();
        for i in 0..n {
            let n = store.create_node(&["Ligne"]);
            store.set_node_property(n, "id", Value::Int64(i));
            #[allow(clippy::cast_precision_loss)]
            store.set_node_property(n, "f", Value::Float64(i as f64));
        }
        db.create_property_index("id");
        db.create_property_index("f");
        db.close().unwrap();
    }
    let zone = chemin.join("substrate.props.eq.idx.v2");
    assert!(zone.exists(), "la zone d'égalité a été persistée");
    std::fs::remove_file(&zone).unwrap();
    for tour in 0..2 {
        let db = ObrainDB::open(&chemin).unwrap();
        // Au PREMIER tour il y a quelque chose à préparer et un fil le fait,
        // AVANT toute requête ; au second, la zone réécrite couvre tout et
        // rien n'est lancé. Sans ce fil, la première requête reconstruirait à
        // la demande et `close()` persisterait : le résultat serait le même,
        // mais pas le chemin (revue 3, M1).
        let fil = db.attendre_preparation_index();
        assert_eq!(fil, tour == 0, "tour {tour} : fil de préparation");
        assert!(
            !db.index_egalite_a_preparer(),
            "tour {tour} : plus rien à préparer avant la première requête"
        );
        assert_eq!(
            un_entier(&db, "MATCH (n:Ligne {id: 123}) RETURN count(n)"),
            1,
            "tour {tour}"
        );
        // `f` porte des flottants entiers : une recherche d'entier les trouve.
        assert_eq!(
            un_entier(&db, "MATCH (n:Ligne {f: 123}) RETURN count(n)"),
            1,
            "tour {tour}"
        );
        db.close().unwrap();
    }
    assert!(zone.exists(), "la préparation a réécrit la zone");
}
