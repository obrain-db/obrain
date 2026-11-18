//! Motif mixte : relation simple et relation de longueur variable dans le
//! MÊME `MATCH` (obrain-db/obrain#189, plan T7).
//!
//! `MATCH (a)-[r:T]->(m)-[rs:T*1..2]->(t)` rendait ZÉRO ligne : le filtre
//! d'unicité `all(x IN rs WHERE x <> r)` (#182/#207) recevait la dernière
//! arête du chemin au lieu de la liste, et un prédicat de liste sur une
//! non-liste élimine la ligne. Les trois `get_propagated_notes` de PO sont
//! de cette forme.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test motif_mixte_unicite
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// a -> b, a -> c, b -> a (cycle a/b).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:N {id: 'a'})-[:T]->(b:N {id: 'b'}), (a)-[:T]->(c:N {id: 'c'}), \
             (b)-[:T]->(a)",
        )
        .expect("CREATE");
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

fn texte(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        autre => panic!("texte attendu, reçu {autre:?}"),
    }
}

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, reçu {autre:?}"),
    }
}

fn paires(r: &[Vec<Value>]) -> Vec<(String, String)> {
    let mut p: Vec<_> = r.iter().map(|l| (texte(&l[0]), texte(&l[1]))).collect();
    p.sort();
    p
}

#[test]
fn motif_mixte_rend_les_chemins_sans_reemploi_de_la_relation_simple() {
    let db = base();
    // Depuis a : r = a->b (m = b). rs depuis b : b->a (t = a) ; b->a->c (t = c) ;
    // b->a->b réutilise r : EXCLU. m = c : aucune sortie.
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'})-[r:T]->(m)-[rs:T*1..2]->(t) RETURN m.id, t.id",
    );
    assert_eq!(
        paires(&r),
        vec![("b".into(), "a".into()), ("b".into(), "c".into())],
        "lignes : {r:?}"
    );
}

#[test]
fn motif_mixte_anonyme_rend_autant_que_deux_clauses_moins_les_reemplois() {
    let db = base();
    let mixte = lignes(
        &db,
        "MATCH (a:N {id: 'a'})-[:T]->(m)-[:T*1..2]->(t) RETURN m.id, t.id",
    );
    let deux = lignes(
        &db,
        "MATCH (a:N {id: 'a'})-[r:T]->(m) MATCH (m)-[rs:T*1..2]->(t) \
         WHERE none(x IN rs WHERE x = r) RETURN m.id, t.id",
    );
    assert!(!mixte.is_empty(), "le motif mixte ne doit plus être vide");
    assert_eq!(paires(&mixte), paires(&deux));
}

#[test]
fn variable_de_longueur_variable_est_une_liste() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'})-[rs:T*1..2]->(t) RETURN t.id, size(rs) ORDER BY size(rs), t.id",
    );
    // a->b, a->c (1 saut), a->b->a (2 sauts). (`[x IN rs | type(x)]` : plan #189, T8.)
    assert_eq!(r.len(), 3, "{r:?}");
    assert_eq!((texte(&r[0][0]), entier(&r[0][1])), ("b".into(), 1));
    assert_eq!((texte(&r[1][0]), entier(&r[1][1])), ("c".into(), 1));
    assert_eq!((texte(&r[2][0]), entier(&r[2][1])), ("a".into(), 2));
    // RETURN rs : une liste de relations (cartes), pas une relation seule.
    let r = lignes(&db, "MATCH (a:N {id: 'a'})-[rs:T*2..2]->(t) RETURN rs");
    assert_eq!(r.len(), 1);
    match &r[0][0] {
        Value::List(rels) => {
            assert_eq!(rels.len(), 2, "{rels:?}");
            assert!(rels.iter().all(|v| matches!(v, Value::Map(_))), "{rels:?}");
        }
        autre => panic!("RETURN rs doit rendre une liste : {autre:?}"),
    }
}

#[test]
fn cas_po_notes_propagees_vers_une_cible_liee() {
    // La forme exacte des trois `get_propagated_notes` de PO.
    let db = base();
    db.session()
        .execute_cypher("MATCH (a:N {id: 'a'}) CREATE (n:Note {id: 'n'})-[:LINKED_TO]->(a)")
        .expect("CREATE Note");
    let r = lignes(
        &db,
        "MATCH (target:N {id: 'a'}) \
         MATCH path = (n:Note)-[:LINKED_TO]->(source)-[:T*0..2]->(target) \
         RETURN n.id, length(path) ORDER BY length(path)",
    );
    // Longueur 0 (source = target) et longueur 2 (a -> b -> a).
    let longueurs: Vec<i64> = r.iter().map(|l| entier(&l[1])).collect();
    assert!(!r.is_empty(), "la note doit être trouvée");
    assert!(
        longueurs.contains(&0),
        "chemin de longueur 0 attendu : {r:?}"
    );
}
