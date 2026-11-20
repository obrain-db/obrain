//! `UNWIND labels(n)` doit dérouler la liste des labels (rendait 0 ligne).
//!
//! Cause : `plan_unwind` (src/query/planner/lpg/mutation.rs) ne calculait la
//! liste que pour une variable, un littéral ou un accès de propriété ; un appel
//! de fonction (`labels(n)`, `keys(n)`, `range(..)`…) retombait sur le repli
//! « la colonne 0 contient la liste » — or la colonne 0 est le nœud `n`, un
//! scalaire, donc rien à dérouler.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test unwind_labels_liste
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

#[test]
fn unwind_labels_rend_un_element_par_label() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher("CREATE (:A:B {x: 1})").unwrap();

    let r = lignes(&db, "MATCH (n) UNWIND labels(n) AS l RETURN count(l)");
    assert_eq!(r, vec![vec![Value::Int64(2)]]);

    let mut noms: Vec<String> = lignes(&db, "MATCH (n) UNWIND labels(n) AS l RETURN l")
        .into_iter()
        .map(|l| format!("{:?}", l[0]))
        .collect();
    noms.sort();
    assert_eq!(noms.len(), 2, "{noms:?}");
    assert!(noms[0].contains('A') && noms[1].contains('B'), "{noms:?}");
}

#[test]
fn unwind_appel_de_fonction_sur_nœud_garde_les_autres_colonnes() {
    let db = ObrainDB::new_in_memory();
    db.session().execute_cypher("CREATE (:A:B {x: 1})").unwrap();
    let r = lignes(&db, "MATCH (n) UNWIND labels(n) AS l RETURN n.x, l");
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(r.iter().all(|l| l[0] == Value::Int64(1)), "{r:?}");
}
