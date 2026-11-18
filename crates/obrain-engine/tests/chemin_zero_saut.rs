//! Chemin de longueur 0 dans un motif `*0..n` (obrain-db/obrain#189).
//!
//! `MATCH p = (a)-[:T*0..1]->(b)` doit rendre le chemin vide (`b = a`,
//! `length(p) = 0`, `relationships(p) = []`) EN PLUS des voisins. L'opérateur
//! n'émettait qu'à partir de la profondeur 1 : les notes liées DIRECTEMENT au
//! projet (`(source)-[*0..3]->(target)`, `source = target`) manquaient.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test chemin_zero_saut
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

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

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, reçu {autre:?}"),
    }
}

fn texte(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        autre => panic!("texte attendu, reçu {autre:?}"),
    }
}

#[test]
fn zero_saut_compte_le_chemin_vide() {
    let db = base();
    // Depuis a (degré sortant 2) : a lui-même, b, c.
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'}) MATCH p = (a)-[:T*0..1]->(b) RETURN count(p)",
    );
    assert_eq!(entier(&r[0][0]), 3, "*0..1 : chemin vide + 2 voisins");
    // *0..0 : exactement le nœud lui-même.
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'}) MATCH p = (a)-[:T*0..0]->(b) RETURN b.id",
    );
    assert_eq!(r.len(), 1);
    assert_eq!(texte(&r[0][0]), "a");
    // *1..1 : pas de chemin vide.
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'}) MATCH p = (a)-[:T*1..1]->(b) RETURN count(p)",
    );
    assert_eq!(entier(&r[0][0]), 2);
}

#[test]
fn chemin_vide_longueur_zero_et_sans_relation() {
    let db = base();
    let r = lignes(
        &db,
        "MATCH (a:N {id: 'a'}) MATCH p = (a)-[:T*0..1]->(b) WHERE b = a \
         RETURN length(p), size(relationships(p)), size(nodes(p)), b.id",
    );
    assert_eq!(r.len(), 1, "un seul chemin vide depuis a");
    assert_eq!(entier(&r[0][0]), 0, "length(p)");
    assert_eq!(entier(&r[0][1]), 0, "relationships(p) vide");
    assert_eq!(entier(&r[0][2]), 1, "nodes(p) = [a]");
    assert_eq!(texte(&r[0][3]), "a");
}

#[test]
fn zero_saut_vers_une_cible_deja_liee() {
    // Le cas de PO : la cible est liée AVANT le motif ; `source = target`
    // doit compter (notes liées directement au projet). Le saut `LINKED_TO`
    // est dans sa propre clause : dans le MÊME MATCH, le filtre d'unicité
    // entre relation simple et relation de longueur variable vide tout le
    // résultat — défaut distinct, suivi à part (plan #189, T7).
    let db = base();
    db.session()
        .execute_cypher("MATCH (a:N {id: 'a'}) CREATE (n:Note {id: 'n'})-[:LINKED_TO]->(a)")
        .expect("CREATE Note");
    let r = lignes(
        &db,
        "MATCH (target:N {id: 'a'}) MATCH (n:Note)-[:LINKED_TO]->(source) \
         MATCH p = (source)-[:T*0..2]->(target) \
         RETURN n.id, length(p) ORDER BY length(p)",
    );
    // Longueur 0 (source = target) et longueur 2 (a -> b -> a).
    let longueurs: Vec<i64> = r.iter().map(|l| entier(&l[1])).collect();
    assert_eq!(
        longueurs,
        vec![0, 2],
        "chemins depuis la source vers la cible liée : {r:?}"
    );
    assert!(r.iter().all(|l| texte(&l[0]) == "n"));
}
