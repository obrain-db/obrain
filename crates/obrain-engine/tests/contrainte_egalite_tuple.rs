//! L'unicité d'un tuple reconnaît les doublons que le LANGAGE tient pour égaux,
//! quelle que soit la façon dont l'index d'égalité range les valeurs.
//!
//! Pour le langage `'2026-11-11T10:00:00Z' = '2026-11-11T12:00:00+02:00'` et
//! `5 = 5.0`. L'index d'égalité du store, lui, range ces valeurs sous des clés
//! distinctes : un candidat tiré d'une telle colonne ne BORNE pas les porteurs.
//! La recherche d'une contrainte n'utilise donc l'index que pour une colonne à
//! clé exacte (booléen, chaîne qui n'est pas une date-heure) et balaie le label
//! sinon. Chaque cas est joué dans les DEUX ordres de déclaration, avec 30
//! nœuds de remplissage (l'index n'est jamais « vide » ni « petit »).

#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

const DT_Z: &str = "2026-11-11T10:00:00Z";
const DT_PLUS2: &str = "2026-11-11T12:00:00+02:00";

/// Les deux déclarations d'un même tuple `(a, b)`.
const ORDRES: [&str; 2] = [
    "CREATE CONSTRAINT u FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE",
    "CREATE CONSTRAINT u FOR (n:T) REQUIRE (n.b, n.a) IS UNIQUE",
];

fn base(contrainte: &str) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher(contrainte).unwrap();
    for i in 0..30 {
        s.execute(&format!(
            "INSERT (:T {{a: {}, b: 'remplissage{i}'}})",
            100 + i
        ))
        .unwrap();
    }
    db
}

fn insere(db: &ObrainDB, a: &str, b: &str) -> bool {
    db.session()
        .execute(&format!("INSERT (:T {{a: {a}, b: '{b}'}})"))
        .is_ok()
}

#[test]
fn meme_instant_dans_deux_fuseaux_sur_une_colonne_du_tuple() {
    for contrainte in ORDRES {
        let db = base(contrainte);
        assert!(insere(&db, "1", DT_Z));
        assert!(
            !insere(&db, "1", DT_PLUS2),
            "`{contrainte}` : (1, …Z) = (1, …+02:00), doublon"
        );
        assert_eq!(db.verifier_contraintes().violations_totales, 0);
    }
}

#[test]
fn entier_flottant_avec_une_colonne_date_heure() {
    for contrainte in ORDRES {
        let db = base(contrainte);
        assert!(insere(&db, "5.0", DT_Z));
        assert!(
            !insere(&db, "5", DT_Z),
            "`{contrainte}` : (5, dt) = (5.0, dt), doublon"
        );
        assert_eq!(db.verifier_contraintes().violations_totales, 0);
    }
}

#[test]
fn flottant_non_entier_et_date_heure_dans_les_deux_graphies() {
    for contrainte in ORDRES {
        let db = base(contrainte);
        assert!(insere(&db, "1.5", DT_Z));
        assert!(
            !insere(&db, "1.5", DT_PLUS2),
            "`{contrainte}` : (1.5, …Z) = (1.5, …+02:00), doublon"
        );
        // Et dans l'autre sens d'insertion.
        let db = base(contrainte);
        assert!(insere(&db, "1.5", DT_PLUS2));
        assert!(!insere(&db, "1.5", DT_Z), "`{contrainte}` : sens inverse");
        assert_eq!(db.verifier_contraintes().violations_totales, 0);
    }
}

#[test]
fn colonne_date_heure_seule() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher("CREATE CONSTRAINT u FOR (n:T) REQUIRE n.b IS UNIQUE")
        .unwrap();
    for i in 0..30 {
        s.execute(&format!("INSERT (:T {{b: 'remplissage{i}'}})"))
            .unwrap();
    }
    s.execute(&format!("INSERT (:T {{b: '{DT_Z}'}})")).unwrap();
    assert!(
        s.execute(&format!("INSERT (:T {{b: '{DT_PLUS2}'}})"))
            .is_err(),
        "UNIQUE b seul : le même instant écrit dans un autre fuseau est un doublon"
    );
    assert_eq!(db.verifier_contraintes().violations_totales, 0);
}

/// Une chaîne qui RESSEMBLE à une date-heure sans en être une n'est égale qu'à
/// elle-même : distincte d'une vraie date-heure, acceptée. (Vert avant comme
/// après : fixe un comportement, ne discrimine pas.)
#[test]
fn comportement_fixe_chaine_qui_ressemble_a_une_date_heure_reste_distincte() {
    for contrainte in ORDRES {
        let db = base(contrainte);
        assert!(insere(&db, "1", "2026-11-11 10:00"));
        assert!(insere(&db, "1", DT_Z), "`{contrainte}` : forme différente");
        assert!(insere(&db, "1", "2026-11-11 10:00:01"));
        assert!(
            !insere(&db, "1", "2026-11-11 10:00"),
            "`{contrainte}` : la même chaîne, doublon"
        );
    }
}

/// Entier face à flottant sur une colonne SEULE : l'index d'égalité ne range
/// pas `5.0` avec `5`, le contrôle balaie et refuse dans les deux sens.
#[test]
fn entier_flottant_colonne_seule_dans_les_deux_sens() {
    for (premier, second) in [("5", "5.0"), ("5.0", "5")] {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher("CREATE CONSTRAINT u FOR (n:T) REQUIRE n.a IS UNIQUE")
            .unwrap();
        for i in 0..30 {
            s.execute(&format!("INSERT (:T {{a: {}}})", 100 + i))
                .unwrap();
        }
        s.execute(&format!("INSERT (:T {{a: {premier}}})")).unwrap();
        assert!(
            s.execute(&format!("INSERT (:T {{a: {second}}})")).is_err(),
            "{premier} puis {second} : doublon"
        );
    }
}
