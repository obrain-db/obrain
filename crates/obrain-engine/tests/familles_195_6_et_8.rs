//! Familles 6 et 8 de obrain-db/obrain#195 — les deux que l'issue n'avait jamais
//! réduites, et qu'aucun test n'épinglait.
//!
//! ## Famille 6 — `rels[i].propriété` (requête PO `chat_11`)
//!
//! ```cypher
//! MATCH path = (root:ChatSession {id: $id})<-[:SPAWNED_BY*0..10]-(child:ChatSession)
//! WITH child, relationships(path) AS rels
//! RETURN child.id, startNode(rels[size(rels)-1]).id AS self_id
//! ```
//!
//! Message d'origine : « l'identifiant 1498208 désigne autre chose » —
//! l'indexation d'une liste de relations n'était pas résolue comme un élément.
//!
//! Sémantique Neo4j, qui fait foi ici : `relationships(path)` rend les arêtes
//! dans l'ordre du chemin (de la racine vers l'enfant), mais `startNode()` rend
//! le nœud d'origine de l'arête **telle qu'elle est stockée** — et l'arête est
//! stockée de l'enfant vers son parent (`(child)-[:SPAWNED_BY]->(root)`).
//! `startNode(rels[-1])` vaut donc l'enfant lui-même, pas son parent. C'est le
//! comportement mesuré, et il est conforme : la première version de ce test
//! attendait le parent, elle avait tort. Si le PO voulait le parent, c'est
//! `endNode` qu'il lui faut — ce n'est pas la même question.
//!
//! Sur le chemin de longueur nulle (l'enfant EST la racine), `rels` est vide et
//! `rels[-1]` vaut `null` : `startNode(null)` vaut alors `null`.
//!
//! ## Famille 8 — `WITH *` (requête PO `ana_11`)
//!
//! `WITH … WITH * … ORDER BY` rendait « Expected end of query ». Les fonctions
//! `apoc.coll.sort` / `apoc.coll.toSet` n'existent pas dans obrain, et l'issue
//! dit explicitement que c'est attendu ; mais l'erreur de SYNTAXE sur `WITH *`
//! survenait avant, et c'est elle qui est épinglée ici. L'absence d'`apoc` doit
//! se manifester comme une fonction inconnue, pas comme une fin de requête
//! inattendue.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test familles_195_6_et_8
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// r1 ← r2 ← r3, l'arête allant de l'enfant vers son parent.
fn sessions() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:ChatSession {id: 'r1'}), (b:ChatSession {id: 'r2'}), (c:ChatSession {id: 'r3'}), \
             (b)-[:SPAWNED_BY]->(a), (c)-[:SPAWNED_BY]->(b)",
        )
        .unwrap();
    db
}

const CHAT_11: &str = "MATCH path = (root:ChatSession {id: 'r1'})<-[:SPAWNED_BY*0..10]-(child:ChatSession) \
     WITH child, relationships(path) AS rels \
     RETURN child.id, startNode(rels[size(rels)-1]).id AS self_id";

#[test]
fn famille_6_rels_indexee_puis_start_node() {
    let db = sessions();
    let r = db
        .session()
        .execute_cypher(CHAT_11)
        .expect("chat_11 doit s'analyser et s'exécuter");
    assert_eq!(
        r.columns,
        vec!["child.id".to_string(), "self_id".to_string()]
    );

    // L'arête étant stockée de l'enfant vers son parent, `startNode(rels[-1])`
    // est l'enfant lui-même ; la racine, atteinte par un chemin vide, rend null.
    let attendu: Vec<(String, Option<String>)> = vec![
        ("r1".to_string(), None),
        ("r2".to_string(), Some("r2".to_string())),
        ("r3".to_string(), Some("r3".to_string())),
    ];
    for (enfant, self_id) in attendu {
        let ligne = r
            .rows
            .iter()
            .find(|l| matches!(l.first(), Some(Value::String(s)) if s.as_ref() == enfant))
            .unwrap_or_else(|| panic!("enfant {enfant} absent des lignes : {:?}", r.rows));
        let vu = match ligne.get(1) {
            Some(Value::Null) | None => None,
            Some(Value::String(s)) => Some(s.to_string()),
            autre => panic!("self_id inattendu pour {enfant} : {autre:?}"),
        };
        assert_eq!(vu, self_id, "self_id de {enfant}");
    }
}

#[test]
fn famille_6_temoin_sans_indexation() {
    // Témoin : la même requête sans l'indexation ne doit pas régresser non plus.
    let db = sessions();
    let r = db
        .session()
        .execute_cypher(
            "MATCH path = (root:ChatSession {id: 'r1'})<-[:SPAWNED_BY*0..10]-(child:ChatSession) \
             WITH child, relationships(path) AS rels RETURN child.id, size(rels) AS n",
        )
        .expect("requête témoin");
    let mut n: Vec<(String, i64)> = r
        .rows
        .iter()
        .map(|l| match (&l[0], &l[1]) {
            (Value::String(s), Value::Int64(k)) => (s.to_string(), *k),
            autre => panic!("ligne inattendue : {autre:?}"),
        })
        .collect();
    n.sort();
    assert_eq!(
        n,
        vec![
            ("r1".to_string(), 0),
            ("r2".to_string(), 1),
            ("r3".to_string(), 2)
        ]
    );
}

const ANA_11: &str = "MATCH (f:File) \
     WITH f, f.path AS p \
     WITH * \
     RETURN p \
     ORDER BY p";

#[test]
fn famille_8_with_etoile_puis_order_by() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:File {path: 'b'}), (:File {path: 'a'})")
        .unwrap();
    let r = db
        .session()
        .execute_cypher(ANA_11)
        .expect("`WITH *` suivi d'un ORDER BY doit s'analyser");
    let vus: Vec<String> = r
        .rows
        .iter()
        .map(|l| match &l[0] {
            Value::String(s) => s.to_string(),
            autre => panic!("ligne inattendue : {autre:?}"),
        })
        .collect();
    assert_eq!(
        vus,
        vec!["a".to_string(), "b".to_string()],
        "ORDER BY appliqué"
    );
}

#[test]
fn famille_8_with_etoile_ne_projette_rien_de_moins() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:File {path: 'x', size: 7})")
        .unwrap();
    // `WITH *` conserve les variables, y compris celles que `WITH f, p` a
    // laissées de côté : la propriété reste lisible après.
    let r = db
        .session()
        .execute_cypher("MATCH (f:File) WITH f, f.path AS p WITH * RETURN f.size AS taille")
        .expect("`WITH *` doit propager les variables");
    assert_eq!(r.rows, vec![vec![Value::Int64(7)]]);
}

/// L'absence des fonctions `apoc.*` est attendue, mais elle doit se dire comme
/// une fonction inconnue — jamais comme « Expected end of query », qui était le
/// symptôme de la famille 8.
#[test]
fn famille_8_apoc_absent_est_une_fonction_inconnue() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:File {path: 'a'})")
        .unwrap();
    match db.session().execute_cypher(
        "MATCH (f:File) WITH f, collect(f.path) AS ps RETURN apoc.coll.sort(ps) AS tri",
    ) {
        Ok(r) => panic!("apoc.coll.sort ne devrait pas exister, obtenu {:?}", r.rows),
        Err(e) => {
            let msg = format!("{e}").to_lowercase();
            assert!(
                msg.contains("inconnue") || msg.contains("unknown") || msg.contains("not found"),
                "l'erreur doit nommer la fonction inconnue, pas la syntaxe : {e}"
            );
            assert!(
                !msg.contains("expected end of query"),
                "« Expected end of query » est le symptôme de la famille 8 : {e}"
            );
        }
    }
}
