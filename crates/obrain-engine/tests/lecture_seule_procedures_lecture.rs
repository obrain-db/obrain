//! #354 — en lecture seule (`ObrainDB::open_read_only`), les procédures de
//! LECTURE doivent répondre : `db.labels()`, `db.relationshipTypes()` et
//! `db.index.vector.queryNodes()`.
//!
//! Constat de l'issue : les trois échouaient avec
//! `OBRAIN-T003: Cannot write in read-only transaction`. 9 requêtes de
//! Project Orchestrator sur 325 (`schema_01`, `schema_02`, `note_31/32/33/35`,
//! `dec_04`, `chat_07`).
//!
//! Chaque requête est jouée DEUX fois, sur la même base persistante :
//! en lecture-écriture (référence : elle répond) puis en lecture seule. Le
//! résultat doit être identique. Une vraie écriture reste refusée.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher,vector-index,algos --test lecture_seule_procedures_lecture
//! ```

#![cfg(all(feature = "cypher", feature = "vector-index", feature = "algos"))]

use std::path::{Path, PathBuf};

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, TransactionError};
use obrain_engine::ObrainDB;

const LABELS: &str = "CALL db.labels() YIELD label RETURN label ORDER BY label";
const TYPES: &str = "CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType ORDER BY relationshipType";
const VECTORIELLE: &str = "CALL db.index.vector.queryNodes('N_emb', 2, [0.1, 0.2, 0.3, 0.4]) \
     YIELD node AS n, score RETURN n.name AS name, score ORDER BY score DESC";

/// La base de l'issue : deux nœuds `N` reliés par `R`, un index vectoriel
/// `N(emb)`. Écrite, indexée, FERMÉE : la lecture seule la rouvre ensuite.
fn base_persistante(dir: &Path) -> PathBuf {
    let chemin = dir.join("base");
    let db = ObrainDB::open(&chemin).expect("ouverture en écriture");
    // Construite par l'API et non par `CREATE (…{emb: [0.1, …]})` : un littéral de
    // liste Cypher est stocké comme LISTE, et l'index vectoriel n'indexe que des
    // valeurs de type VECTEUR (un index sur des listes reste vide).
    let a = db.create_node(&["N"]);
    db.set_node_property(a, "name", Value::String("a".into()));
    db.set_node_property(a, "emb", Value::Vector(vec![0.1f32, 0.2, 0.3, 0.4].into()));
    let b = db.create_node(&["N"]);
    db.set_node_property(b, "name", Value::String("b".into()));
    db.set_node_property(b, "emb", Value::Vector(vec![0.4f32, 0.3, 0.2, 0.1].into()));
    db.create_edge(a, b, "R");
    // Un index NOMMÉ : `queryNodes('N_emb', …)` résout le nom, et un index créé
    // par `create_vector_index` n'en porte aucun (« index nommés connus : [] »).
    db.create_named_vector_index("N_emb", "N", "emb", Some(4), Some("cosine"), None, None)
        .expect("index vectoriel nommé");
    db.close().expect("fermeture");
    chemin
}

fn jouer(db: &ObrainDB, requete: &str) -> Result<Vec<Vec<Value>>, Error> {
    db.session().execute_cypher(requete).map(|r| r.rows)
}

/// Rejoue `requete` en lecture-écriture puis en lecture seule sur la MÊME
/// base, et exige la même réponse.
fn identique_en_lecture_seule(requete: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin = base_persistante(dir.path());

    let rw = ObrainDB::open(&chemin).expect("réouverture en écriture");
    let reference = jouer(&rw, requete)
        .unwrap_or_else(|e| panic!("référence (lecture-écriture) : {requete}\n{e}"));
    assert!(
        !reference.is_empty(),
        "la référence doit rendre des lignes : {requete}"
    );
    rw.close().expect("fermeture");

    let ro = ObrainDB::open_read_only(&chemin).expect("ouverture en lecture seule");
    let obtenu =
        jouer(&ro, requete).unwrap_or_else(|e| panic!("EN LECTURE SEULE : {requete}\n{e}"));
    assert_eq!(
        obtenu, reference,
        "même réponse qu'en lecture-écriture : {requete}"
    );
}

#[test]
fn labels_en_lecture_seule() {
    identique_en_lecture_seule(LABELS);
}

#[test]
fn relationship_types_en_lecture_seule() {
    identique_en_lecture_seule(TYPES);
}

#[test]
fn query_nodes_en_lecture_seule() {
    identique_en_lecture_seule(VECTORIELLE);
}

#[test]
fn une_vraie_ecriture_reste_refusee_en_lecture_seule() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin = base_persistante(dir.path());
    let ro = ObrainDB::open_read_only(&chemin).expect("ouverture en lecture seule");
    let e = jouer(&ro, "CREATE (:N {name: 'intrus'})")
        .expect_err("une écriture doit être refusée en lecture seule");
    assert!(
        matches!(e, Error::Transaction(TransactionError::ReadOnly)),
        "refus typé attendu, reçu : {e}"
    );
}
