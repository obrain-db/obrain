//! Mot-clé comme label ou type de relation après `:` (`[:IN]`, `(n:Order)`).
//!
//! Neo4j les accepte sans échappement ; obrain refusait « Expected
//! identifier ». Position sans ambiguïté : après `:` vient toujours un nom.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test label_mot_cle
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn compte(db: &ObrainDB, requete: &str) -> i64 {
    match db
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows[0][0]
    {
        Value::Int64(n) => n,
        ref autre => panic!("{autre:?}"),
    }
}

#[test]
fn type_de_relation_mot_cle() {
    let db = ObrainDB::new_in_memory();
    db.execute_cypher("CREATE (:Note)-[:IN]->(:Order)-[:MATCH]->(:Order)")
        .unwrap();
    assert_eq!(compte(&db, "MATCH (:Note)-[:IN]->(o:Order) RETURN count(o)"), 1);
    assert_eq!(compte(&db, "MATCH ()-[r:IN|MATCH]->() RETURN count(r)"), 2);
    assert_eq!(compte(&db, "MATCH (n) WHERE n:Order RETURN count(n)"), 2);
    db.execute_cypher("MATCH (n:Note) SET n:Limit").unwrap();
    assert_eq!(compte(&db, "MATCH (n:Limit) RETURN count(n)"), 1);
    db.execute_cypher("MATCH (n:Limit) REMOVE n:Limit").unwrap();
    assert_eq!(compte(&db, "MATCH (n:Limit) RETURN count(n)"), 0);
}
