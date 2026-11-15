//! Un mot-clé du langage comme nom de paramètre (`$limit`, `$skip`, `$match`,
//! `$null`…) ou comme nom de propriété (`s.order`, `{order: 1}`,
//! `SET s.order = 2`). Issue obrain-db/obrain#177.
//!
//! Formes dont dépend Project Orchestrator (migration Neo4j → obrain) :
//! partout où une liste est paginée, et la propriété `Step.order` :
//!
//! ```text
//! MATCH (n:Note) RETURN n.name ORDER BY n.name SKIP $offset LIMIT $limit
//! MATCH (s:Step) RETURN s.id, s.order ORDER BY s.order LIMIT 3
//! ```
//!
//! Avant le correctif, le front-end Cypher refusait `$limit` et `.order`
//! (« syntax error: Expected identifier ») alors qu'il acceptait `$offset` :
//! le nom choisi par l'appelant décidait du verdict. Un paramètre est
//! toujours précédé de `$`, une propriété de `.` ou suivie de `:` dans une
//! map : rien ne peut les confondre avec le mot-clé ; Neo4j les accepte
//! sans échappement.
//!
//! Chaque forme est jouée en Cypher et en GQL, avec des valeurs réelles.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test parametre_nom_mot_cle
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;
use std::collections::HashMap;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// Les mots-clés qui étaient refusés après `$`, plus quelques-uns qui
/// passaient déjà (témoins : le correctif ne doit rien leur retirer).
const NOMS: [&str; 24] = [
    "limit", "skip", "match", "order", "where", "return", "with", "set", "delete", "create",
    "merge", "null", "true", "false", "distinct", "as", "by", "and", "or", "not",
    // témoins, acceptés avant le correctif
    "offset", "count", "id", "name",
];

fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    for nom in ["a", "b", "c", "d", "e"] {
        session.create_node_with_props(&["Note"], [("name", Value::String(nom.into()))]);
    }
    db
}

fn executer(
    db: &ObrainDB,
    langage: &str,
    requete: &str,
    params: HashMap<String, Value>,
) -> Vec<Vec<Value>> {
    db.session()
        .execute_language(requete, langage, Some(params))
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"))
        .rows
}

fn noms(lignes: &[Vec<Value>]) -> Vec<String> {
    lignes
        .iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("nom attendu, reçu {autre:?}"),
        })
        .collect()
}

#[test]
fn un_parametre_nomme_comme_un_mot_cle_est_lu() {
    let db = base();
    for langage in LANGAGES {
        for nom in NOMS {
            let mut params = HashMap::new();
            params.insert(nom.to_string(), Value::Int64(7));
            let lignes = executer(&db, langage, &format!("RETURN ${nom} AS v"), params);
            assert_eq!(lignes, vec![vec![Value::Int64(7)]], "[{langage}] ${nom}");
        }
    }
}

#[test]
fn pagination_skip_offset_limit_limit() {
    let db = base();
    for langage in LANGAGES {
        let mut params = HashMap::new();
        params.insert("offset".to_string(), Value::Int64(1));
        params.insert("limit".to_string(), Value::Int64(2));
        let lignes = executer(
            &db,
            langage,
            "MATCH (n:Note) RETURN n.name AS name ORDER BY n.name SKIP $offset LIMIT $limit",
            params,
        );
        assert_eq!(noms(&lignes), ["b", "c"], "[{langage}]");
    }
}

#[test]
fn limit_parametre_dans_un_with() {
    let db = base();
    for langage in LANGAGES {
        let mut params = HashMap::new();
        params.insert("limit".to_string(), Value::Int64(2));
        let lignes = executer(
            &db,
            langage,
            "MATCH (n:Note) WITH n ORDER BY n.name DESC LIMIT $limit RETURN n.name AS name",
            params,
        );
        assert_eq!(noms(&lignes), ["e", "d"], "[{langage}]");
    }
}

#[test]
fn un_parametre_mot_cle_dans_un_predicat() {
    let db = base();
    for langage in LANGAGES {
        let mut params = HashMap::new();
        params.insert("match".to_string(), Value::String("c".into()));
        let lignes = executer(
            &db,
            langage,
            "MATCH (n:Note) WHERE n.name = $match RETURN n.name AS name",
            params,
        );
        assert_eq!(noms(&lignes), ["c"], "[{langage}]");
    }
}

/// `Step.order` : la forme exacte de Project Orchestrator (lecture, tri),
/// puis l'écriture (`CREATE {order: …}`, `SET s.order`, `REMOVE s.limit`),
/// un prédicat et une map littérale.
#[test]
fn une_propriete_nommee_comme_un_mot_cle() {
    for langage in LANGAGES {
        let db = ObrainDB::new_in_memory();
        let session = db.session();
        for (id, order) in [("s1", 2), ("s2", 1), ("s3", 3)] {
            session.create_node_with_props(
                &["Step"],
                [
                    ("id", Value::String(id.into())),
                    ("order", Value::Int64(order)),
                    ("limit", Value::Int64(9)),
                ],
            );
        }
        let lecture = executer(
            &db,
            langage,
            "MATCH (s:Step) RETURN s.id, s.order ORDER BY s.order LIMIT 3",
            HashMap::new(),
        );
        assert_eq!(
            lecture,
            vec![
                vec![Value::String("s2".into()), Value::Int64(1)],
                vec![Value::String("s1".into()), Value::Int64(2)],
                vec![Value::String("s3".into()), Value::Int64(3)],
            ],
            "[{langage}] lecture"
        );

        let predicat = executer(
            &db,
            langage,
            "MATCH (s:Step) WHERE s.order > 1 RETURN s.id AS id ORDER BY id",
            HashMap::new(),
        );
        assert_eq!(noms(&predicat), ["s1", "s3"], "[{langage}] prédicat");

        executer(
            &db,
            langage,
            "MATCH (s:Step {id: 's2'}) SET s.order = 7 REMOVE s.limit",
            HashMap::new(),
        );
        let apres = executer(
            &db,
            langage,
            "MATCH (s:Step {id: 's2'}) RETURN s.order, s.limit",
            HashMap::new(),
        );
        assert_eq!(
            apres,
            vec![vec![Value::Int64(7), Value::Null]],
            "[{langage}] SET/REMOVE"
        );

        executer(
            &db,
            langage,
            "CREATE (:Step {id: 's4', order: 4, match: 'x'})",
            HashMap::new(),
        );
        let cree = executer(
            &db,
            langage,
            "MATCH (s:Step {order: 4}) RETURN s.id, s.match",
            HashMap::new(),
        );
        assert_eq!(
            cree,
            vec![vec![Value::String("s4".into()), Value::String("x".into())]],
            "[{langage}] CREATE avec clés mots-clés"
        );

        let map = executer(
            &db,
            langage,
            "RETURN {order: 1, limit: 2} AS m",
            HashMap::new(),
        );
        assert_eq!(map.len(), 1, "[{langage}] map littérale : {map:?}");
        match &map[0][0] {
            Value::Map(m) => {
                let cles: Vec<(String, Value)> = m
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.clone()))
                    .collect();
                assert_eq!(
                    cles,
                    vec![
                        ("limit".to_string(), Value::Int64(2)),
                        ("order".to_string(), Value::Int64(1)),
                    ],
                    "[{langage}]"
                );
            }
            autre => panic!("[{langage}] map attendue, reçu {autre:?}"),
        }
    }
}

/// Un `$` suivi d'autre chose qu'un nom reste une erreur de syntaxe, et une
/// erreur FRANCHE : ni panique, ni requête exécutée avec un paramètre vide.
#[test]
fn un_dollar_sans_nom_est_refuse() {
    let db = base();
    let session = db.session();
    for requete in [
        "RETURN $ AS v",
        "RETURN $( AS v",
        "MATCH (n:Note) RETURN n LIMIT $",
    ] {
        let erreur = session
            .execute_language(requete, "cypher", Some(HashMap::new()))
            .err()
            .unwrap_or_else(|| panic!("{requete} devrait être refusée"));
        let texte = erreur.to_string();
        assert!(
            texte.contains("syntax error"),
            "{requete} : erreur de syntaxe attendue, reçu {texte}"
        );
    }
}
