//! Une chaîne de PLUS DE 256 OCTETS part en colonne blob et n'est JAMAIS
//! écrite dans l'index d'égalité — ni à l'écriture, ni à l'import en lot, ni à
//! la reconstruction depuis la zone v2. Un contrôle d'unicité qui bornait les
//! porteurs par cet index (« la clé est exacte pour une chaîne ») rendait un
//! jeu de candidats VIDE et laissait passer le doublon, sur tous les chemins.
//!
//! La règle est désormais en un seul endroit (`index_borne_les_porteurs`) :
//! l'index ne borne que si la classe d'égalité est la clé ET que toute valeur
//! de cette classe est garantie indexée ET que la clé est enregistrée ET que
//! l'index est bâti. Une chaîne blob-routable ⇒ balayage du label.
//!
//! Matrice réduite de la sonde de relecture : deux valeurs longues (400 o,
//! et longue à PRÉFIXE date-heure), les deux ordres de déclaration et la
//! colonne seule, chaque chemin d'écriture, la réouverture (doublon écrit
//! avant la fermeture, contrainte créée avant ou après), l'import en lot et
//! l'API directe suivis de `CREATE CONSTRAINT`.

#![cfg(feature = "cypher")]

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

const DECLARATIONS: [&str; 3] = [
    "CREATE CONSTRAINT u FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE",
    "CREATE CONSTRAINT u FOR (n:T) REQUIRE (n.b, n.a) IS UNIQUE",
    "CREATE CONSTRAINT u FOR (n:T) REQUIRE n.b IS UNIQUE",
];

/// 400 octets de `x` ; et 400 octets qui COMMENCENT comme une date-heure
/// (`instant_de_chaine` renifle le préfixe : la chaîne ne doit ni passer pour
/// une date, ni échapper à la règle du blob).
fn valeurs_longues() -> Vec<(&'static str, String)> {
    vec![
        ("400 x", "x".repeat(400)),
        (
            "prefixe date-heure",
            format!("2026-11-11T10:00:00Z{}", "y".repeat(380)),
        ),
    ]
}

fn remplir(db: &ObrainDB) {
    let s = db.session();
    for i in 0..30 {
        s.execute(&format!(
            "INSERT (:T {{a: {}, b: 'remplissage{i}'}})",
            100 + i
        ))
        .unwrap();
    }
}

fn nb_porteurs(db: &ObrainDB, b: &str) -> usize {
    db.graph_store()
        .find_nodes_by_properties_for_label("T", &[("b", Value::from(b))])
        .len()
}

/// Chaque chemin d'écriture, sur une base en mémoire.
#[test]
fn chaine_longue_en_doublon_est_refusee_sur_chaque_chemin() {
    for (nom, longue) in valeurs_longues() {
        for decl in DECLARATIONS {
            let db = ObrainDB::new_in_memory();
            let s = db.session();
            s.execute_cypher(decl).unwrap();
            remplir(&db);
            s.execute(&format!("INSERT (:T {{a: 1, b: '{longue}'}})"))
                .expect("première écriture");
            let contexte = format!("[{nom}] `{decl}`");

            // INSERT / CREATE
            assert!(
                s.execute(&format!("INSERT (:T {{a: 1, b: '{longue}'}})"))
                    .is_err(),
                "{contexte} INSERT"
            );
            assert!(
                s.execute_cypher(&format!("CREATE (:T {{a: 1, b: '{longue}'}})"))
                    .is_err(),
                "{contexte} CREATE"
            );
            // SET d'une colonne, SET par carte
            s.execute("INSERT (:T {a: 1, b: 'autre'})").unwrap();
            assert!(
                s.execute_cypher(&format!("MATCH (n:T {{b: 'autre'}}) SET n.b = '{longue}'"))
                    .is_err(),
                "{contexte} SET"
            );
            assert!(
                s.execute_cypher(&format!(
                    "MATCH (n:T {{b: 'autre'}}) SET n += {{b: '{longue}'}}"
                ))
                .is_err(),
                "{contexte} SET +="
            );
            // MERGE qui crée (motif distinct, ON CREATE écrit le doublon)
            assert!(
                s.execute_cypher(&format!(
                    "MERGE (n:T {{a: 1, b: 'neuf'}}) ON CREATE SET n.b = '{longue}'"
                ))
                .is_err(),
                "{contexte} MERGE création"
            );
            // MERGE ON MATCH SET
            assert!(
                s.execute_cypher(&format!(
                    "MERGE (n:T {{b: 'autre'}}) ON MATCH SET n.b = '{longue}'"
                ))
                .is_err(),
                "{contexte} MERGE ON MATCH SET"
            );
            assert_eq!(nb_porteurs(&db, &longue), 1, "{contexte} : un seul porteur");
            assert_eq!(
                db.verifier_contraintes().violations_totales,
                0,
                "{contexte}"
            );
        }
    }
}

/// Réouverture : le porteur a été écrit AVANT la fermeture (il est en colonne
/// blob, pas dans l'index rebâti depuis la zone) ; la contrainte est créée
/// avant ou après la donnée.
#[test]
fn chaine_longue_en_doublon_est_refusee_apres_reouverture() {
    for (nom, longue) in valeurs_longues() {
        for decl in DECLARATIONS {
            for contrainte_avant in [true, false] {
                let td = tempfile::tempdir().unwrap();
                let chemin = td.path().join("c");
                {
                    let db = ObrainDB::open_substrate(&chemin).unwrap();
                    let s = db.session();
                    if contrainte_avant {
                        s.execute_cypher(decl).unwrap();
                    }
                    remplir(&db);
                    s.execute(&format!("INSERT (:T {{a: 1, b: '{longue}'}})"))
                        .unwrap();
                    if !contrainte_avant {
                        s.execute_cypher(decl).unwrap();
                    }
                }
                let db = ObrainDB::open_substrate(&chemin).unwrap();
                let s = db.session();
                assert!(
                    s.execute(&format!("INSERT (:T {{a: 1, b: '{longue}'}})"))
                        .is_err(),
                    "[{nom}] `{decl}` contrainte_avant={contrainte_avant} : doublon après réouverture"
                );
                s.execute(&format!("INSERT (:T {{a: 2, b: '{longue}z'}})"))
                    .expect("valeur longue distincte acceptée");
            }
        }
    }
}

/// Import en lot et API directe : hors contrainte par contrat, mais
/// `CREATE CONSTRAINT` sur les doublons qu'ils ont posés est REFUSÉ, et
/// l'index d'égalité (que l'import n'alimente pas) ne borne plus rien.
#[test]
fn chaine_longue_import_en_lot_et_api_directe() {
    for (nom, longue) in valeurs_longues() {
        for decl in DECLARATIONS {
            // Import en lot (store NEUF, par contrat) : deux porteurs
            // identiques aux slots 1 et 2, puis la contrainte.
            let td = tempfile::tempdir().unwrap();
            let db = ObrainDB::open_substrate(td.path().join("c")).unwrap();
            let (n, _) = db
                .bulk_import(&["T".into(), "T".into()], &[], &[], &[])
                .expect("import des nœuds");
            assert_eq!(n, 2);
            let props: Vec<Vec<(&str, Value)>> = vec![
                vec![("a", Value::Int64(1)), ("b", Value::from(longue.as_str()))],
                vec![("a", Value::Int64(1)), ("b", Value::from(longue.as_str()))],
            ];
            db.bulk_import_props(1, &props)
                .expect("import des propriétés");
            assert_eq!(
                nb_porteurs(&db, &longue),
                2,
                "[{nom}] import : deux porteurs"
            );
            let s = db.session();
            assert!(
                s.execute_cypher(decl).is_err(),
                "[{nom}] `{decl}` : CREATE CONSTRAINT sur des doublons importés"
            );

            // API directe : même chose sur une base en mémoire, avec un index
            // bâti AVANT l'écriture directe (la règle vérifie la présence du
            // porteur, pas seulement la classe de la valeur).
            let db = ObrainDB::new_in_memory();
            remplir(&db);
            db.create_property_index("b");
            let store = db.graph_store();
            let cle = |k: &str| PropertyKey::new(k);
            for _ in 0..2 {
                store.create_node_with_props(
                    &["T"],
                    &[
                        (cle("a"), Value::Int64(1)),
                        (cle("b"), Value::from(longue.as_str())),
                    ],
                );
            }
            let s = db.session();
            assert!(
                s.execute_cypher(decl).is_err(),
                "[{nom}] `{decl}` : CREATE CONSTRAINT sur des doublons posés par l'API"
            );
            s.execute_cypher("MATCH (n:T {a: 1}) DETACH DELETE n")
                .unwrap();
            store.create_node_with_props(
                &["T"],
                &[
                    (cle("a"), Value::Int64(1)),
                    (cle("b"), Value::from(longue.as_str())),
                ],
            );
            s.execute_cypher(decl)
                .expect("un seul porteur : contrainte acceptée");
            assert!(
                s.execute(&format!("INSERT (:T {{a: 1, b: '{longue}'}})"))
                    .is_err(),
                "[{nom}] `{decl}` : le porteur posé par l'API est vu"
            );
        }
    }
}
