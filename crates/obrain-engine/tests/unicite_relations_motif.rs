//! Unicité des relations dans une même clause MATCH (obrain-db/obrain#182).
//!
//! openCypher / Neo4j : deux relations à un saut d'un MÊME motif désignent
//! toujours deux relations distinctes (isomorphisme de relations). Deux
//! clauses MATCH distinctes, elles, peuvent réemprunter la même relation.
//!
//! Forme de Project Orchestrator (`proj_07`) : 1 939 lignes au lieu de 95
//! avant correctif, à cause des faux cycles `(a)-[r1]->(b)<-[r2]-(a)`.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test unicite_relations_motif
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

fn compter(db: &ObrainDB, langage: &str, requete: &str) -> i64 {
    let r = db
        .session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"));
    match r.rows.as_slice() {
        [ligne] => match ligne.as_slice() {
            [Value::Int64(n)] => *n,
            autre => panic!("[{langage}] {requete} : entier attendu, reçu {autre:?}"),
        },
        autre => panic!("[{langage}] {requete} : une ligne attendue, reçu {autre:?}"),
    }
}

/// Un aller simple `a -R-> b`.
fn un_aller() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (a:X {k: 'a'})-[:R]->(b:X {k: 'b'})")
        .unwrap();
    db
}

/// `a` a trois sorties `R` : vers b, c et d.
fn etoile() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (a:X {k: 'a'}), (b:X {k: 'b'}), (c:X {k: 'c'}), (d:X {k: 'd'}), \
             (a)-[:R]->(b), (a)-[:R]->(c), (a)-[:R]->(d)",
        )
        .unwrap();
    db
}

/// Le faux cycle : une même relation ne peut pas servir d'aller ET de retour.
#[test]
fn le_meme_aller_ne_sert_pas_d_aller_retour() {
    let db = un_aller();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[:R]->(b:X)<-[:R]-(a) RETURN count(b)"
            ),
            0,
            "[{langage}] anonyme"
        );
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[r1:R]->(b:X)<-[r2:R]-(a) RETURN count(b)"
            ),
            0,
            "[{langage}] nommé"
        );
    }
}

/// Deux motifs séparés par une virgule dans une même clause.
#[test]
fn virgule_meme_clause_meme_relation() {
    let db = un_aller();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[r1:R]->(b:X), (a)-[r2:R]->(b) RETURN count(b)"
            ),
            0,
            "[{langage}]"
        );
    }
}

/// Trois arêtes sortantes de `a`. Deux relations à un saut d'une même clause
/// sont distinctes : les couples ordonnés (r1, r2) avec r1 ≠ r2 font 3 × 2 = 6.
/// Sans correctif, les couples réemployant la même arête (3 de plus) sortent.
#[test]
fn etoile_couples_de_relations_distinctes() {
    let db = etoile();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[r1:R]->(x:X), (a)-[r2:R]->(y:X) RETURN count(*)"
            ),
            6,
            "[{langage}]"
        );
    }
}

/// Une seule relation : aucune contrainte ajoutée, le résultat ne bouge pas.
#[test]
fn une_seule_relation_inchangee() {
    let db = etoile();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[:R]->(x:X) RETURN count(*)"
            ),
            3,
            "[{langage}]"
        );
    }
}

/// Deux clauses MATCH : la relation peut être réemployée (sémantique Neo4j).
#[test]
fn deux_clauses_match_reemploient_la_relation() {
    let db = un_aller();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[r1:R]->(b:X) MATCH (b)<-[r2:R]-(a) RETURN count(b)"
            ),
            1,
            "[{langage}]"
        );
    }
}

/// Une boucle sur soi `a -R-> a` (une seule arête).
fn boucle() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (a:X {k: 'a'})-[:R]->(a)")
        .unwrap();
    db
}

/// Un vrai cycle `a -R-> b -R-> a` (deux arêtes distinctes).
fn cycle() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (a:X {k: 'a'})-[:R]->(b:X {k: 'b'})-[:R]->(a)")
        .unwrap();
    db
}

/// Longueur variable : la même relation ne sert pas deux fois dans un chemin
/// (cas signalé par PO dans le fil de #182). Neo4j : `*2..2` sur la boucle
/// rend 0 ; `*1..3` rend 1 (le seul chemin d'une arête).
#[test]
fn longueur_variable_une_relation_ne_sert_qu_une_fois() {
    let db = boucle();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH p = (a:X {k:'a'})-[:R*2..2]->(a) RETURN count(p)"
            ),
            0,
            "[{langage}] *2..2"
        );
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH p = (a:X {k:'a'})-[:R*1..3]->(a) RETURN count(p)"
            ),
            1,
            "[{langage}] *1..3"
        );
    }
}

/// Témoin positif (PO) : un vrai cycle à deux arêtes distinctes reste trouvé,
/// en longueur fixe 2 comme en `*2..2`.
#[test]
fn un_vrai_cycle_reste_trouve() {
    let db = cycle();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (a:X {k:'a'})-[:R]->()-[:R]->(a) RETURN count(*)"
            ),
            1,
            "[{langage}] longueur fixe 2"
        );
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH p = (a:X {k:'a'})-[:R*2..2]->(a) RETURN count(p)"
            ),
            1,
            "[{langage}] *2..2"
        );
    }
}

/// Une relation à un saut et une relation de longueur variable d'une même
/// clause ne réemploient pas la même arête (demande de PO, fil de #182).
/// Boucle `a -R-> a` : `(x)-[r:R]->(y)-[:R*1..1]->(z)` ne peut emprunter la
/// boucle qu'une fois → 0 ligne, comme Neo4j.
#[test]
fn saut_fixe_et_longueur_variable_ne_reemploient_pas_l_arete() {
    let db = boucle();
    for langage in LANGAGES {
        assert_eq!(
            compter(
                &db,
                langage,
                "MATCH (x:X {k:'a'})-[r:R]->(y)-[:R*1..1]->(z) RETURN count(*)"
            ),
            0,
            "[{langage}] une boucle ne peut pas servir deux fois"
        );
    }
}
