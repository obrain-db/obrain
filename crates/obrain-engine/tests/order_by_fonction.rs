//! `ORDER BY` sur un appel de fonction qui n'est pas projeté :
//! `ORDER BY coalesce(n.priority, 0) DESC`, `ORDER BY size(xs)`,
//! `ORDER BY toLower(n.name)`.
//!
//! Formes dont dépend Project Orchestrator (23 requêtes de `src/neo4j/`,
//! dont `task.rs`, `alert.rs`, `code.rs`, `feature_graph.rs`), par exemple :
//!
//! ```text
//! ORDER BY coalesce(a.priority, 0.0) DESC, coalesce(a.last_seen, a.created_at) DESC
//! ORDER BY COALESCE(f.pagerank, 0) DESC, connections DESC
//! ORDER BY size(cycle)
//! ```
//!
//! Avant le correctif, une telle clé était cherchée PAR NOM parmi les colonnes
//! projetées et la requête mourait en erreur interne (« Function column
//! 'coalesce' not found for ORDER BY »), alors que `ORDER BY n.v + 1` ou un
//! `CASE` passaient déjà. La clé est désormais projetée comme n'importe quelle
//! expression, puis retirée du résultat.
//!
//! Chaque forme est jouée en Cypher et en GQL, et comparée à la forme
//! équivalente où la fonction est projetée sous un alias puis triée par cet
//! alias — qui passait déjà et sert de référence —, en plus d'une attente
//! calculée à la main.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test order_by_fonction
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// Cinq notes. `priority` manque à c et e ; `last_seen` manque à b, c et e.
///
/// | name | priority | last_seen | created_at | pagerank |
/// |------|----------|-----------|------------|----------|
/// | a    | 3        | t5        | t1         | 0.5      |
/// | b    | 1        | —         | t2         | —        |
/// | c    | —        | —         | t3         | 0.9      |
/// | d    | 2        | t1        | t4         | —        |
/// | e    | —        | —         | t4         | —        |
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let notes: [(&str, Option<i64>, Option<&str>, &str, Option<f64>); 5] = [
        ("a", Some(3), Some("t5"), "t1", Some(0.5)),
        ("b", Some(1), None, "t2", None),
        ("c", None, None, "t3", Some(0.9)),
        ("d", Some(2), Some("t1"), "t4", None),
        ("e", None, None, "t4", None),
    ];
    for (name, priority, last_seen, created_at, pagerank) in notes {
        let mut props = vec![
            ("name", Value::String(name.into())),
            ("created_at", Value::String(created_at.into())),
        ];
        if let Some(p) = priority {
            props.push(("priority", Value::Int64(p)));
        }
        if let Some(v) = last_seen {
            props.push(("last_seen", Value::String(v.into())));
        }
        if let Some(r) = pagerank {
            props.push(("pagerank", Value::Float64(r)));
        }
        session.create_node_with_props(&["Note"], props);
    }
    db
}

fn executer(db: &ObrainDB, langage: &str, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_language(requete, langage, None)
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

/// Joue `requete` (fonction dans ORDER BY) et `reference` (même fonction
/// projetée sous alias, tri par l'alias), et exige la même liste de noms que
/// `attendu`, dans les deux langages.
fn verifier(requete: &str, reference: &str, attendu: &[&str]) {
    let db = base();
    for langage in LANGAGES {
        let obtenu = noms(&executer(&db, langage, requete));
        let temoin = noms(&executer(&db, langage, reference));
        assert_eq!(obtenu, attendu, "[{langage}] {requete}");
        assert_eq!(temoin, attendu, "[{langage}] référence : {reference}");
    }
}

#[test]
fn coalesce_desc_puis_nom() {
    // priorités : a 3, d 2, b 1, c 0, e 0 → ex æquo c/e départagés par le nom.
    verifier(
        "MATCH (n:Note) RETURN n.name AS name ORDER BY coalesce(n.priority, 0) DESC, n.name",
        "MATCH (n:Note) RETURN n.name AS name, coalesce(n.priority, 0) AS k ORDER BY k DESC, name",
        &["a", "d", "b", "c", "e"],
    );
}

#[test]
fn forme_alert_rs_deux_coalesce() {
    // ORDER BY coalesce(a.priority, 0.0) DESC, coalesce(a.last_seen, a.created_at) DESC
    // priorités a 3, d 2, b 1, puis c/e à 0 : coalesce(last_seen, created_at) → c t3, e t4 → e avant c.
    verifier(
        "MATCH (a:Note) RETURN a.name AS name \
         ORDER BY coalesce(a.priority, 0.0) DESC, coalesce(a.last_seen, a.created_at) DESC",
        "MATCH (a:Note) RETURN a.name AS name, coalesce(a.priority, 0.0) AS k1, \
         coalesce(a.last_seen, a.created_at) AS k2 ORDER BY k1 DESC, k2 DESC",
        &["a", "d", "b", "e", "c"],
    );
}

#[test]
fn forme_code_rs_coalesce_majuscule_puis_alias() {
    // ORDER BY COALESCE(f.pagerank, 0) DESC, connections DESC
    // pagerank : c 0.9, a 0.5, les autres 0 ; connections = priority : d 2, b 1, e NULL.
    // Sous DESC, NULL vient en tête (convention Neo4j) : e avant d avant b.
    verifier(
        "MATCH (f:Note) RETURN f.name AS name, f.priority AS connections \
         ORDER BY COALESCE(f.pagerank, 0) DESC, connections DESC",
        "MATCH (f:Note) RETURN f.name AS name, f.priority AS connections, \
         COALESCE(f.pagerank, 0) AS k ORDER BY k DESC, connections DESC",
        &["c", "a", "e", "d", "b"],
    );
}

#[test]
fn size_et_tolower() {
    verifier(
        "MATCH (n:Note) RETURN n.name AS name ORDER BY toLower(n.name) DESC",
        "MATCH (n:Note) RETURN n.name AS name, toLower(n.name) AS k ORDER BY k DESC",
        &["e", "d", "c", "b", "a"],
    );
    // size(created_at) vaut 2 partout : le tri par taille ne départage rien,
    // la seconde clé décide. Vérifie qu'une fonction au résultat constant ne
    // casse pas l'ordre des clés suivantes.
    verifier(
        "MATCH (n:Note) RETURN n.name AS name ORDER BY size(n.created_at), n.name DESC",
        "MATCH (n:Note) RETURN n.name AS name, size(n.created_at) AS k ORDER BY k, name DESC",
        &["e", "d", "c", "b", "a"],
    );
}

#[test]
fn la_cle_de_tri_ne_fuit_pas_dans_le_resultat() {
    let db = base();
    for langage in LANGAGES {
        let lignes = executer(
            &db,
            langage,
            "MATCH (n:Note) RETURN n.name AS name ORDER BY coalesce(n.priority, 0) DESC, n.name",
        );
        assert!(
            lignes.iter().all(|l| l.len() == 1),
            "[{langage}] une seule colonne attendue, reçu {lignes:?}"
        );
    }
}

#[test]
fn limite_sur_tri_par_fonction() {
    let db = base();
    for langage in LANGAGES {
        let lignes = executer(
            &db,
            langage,
            "MATCH (n:Note) RETURN n.name AS name ORDER BY coalesce(n.priority, 0) DESC, n.name LIMIT 2",
        );
        assert_eq!(noms(&lignes), ["a", "d"], "[{langage}]");
    }
}

/// Restes connus, hors des formes utilisées par Project Orchestrator (0
/// occurrence dans `src/neo4j/`), consignés pour ne pas être oubliés :
/// une fonction dans l'ORDER BY d'un `WITH` (le tri ne s'appuie pas sur un
/// `Return`, le Return augmenté ne s'applique pas).
#[test]
#[ignore = "DÉFAUT : WITH … ORDER BY coalesce(…) — le tri d'un WITH n'a pas de Return augmenté"]
fn with_order_by_fonction() {
    verifier(
        "MATCH (n:Note) WITH n ORDER BY coalesce(n.priority, 0) DESC, n.name RETURN n.name AS name",
        "MATCH (n:Note) WITH n, coalesce(n.priority, 0) AS k ORDER BY k DESC, n.name RETURN n.name AS name",
        &["a", "d", "b", "c", "e"],
    );
}

/// En Cypher, un agrégat dans ORDER BY (`ORDER BY count(*) DESC`) passe par
/// un opérateur Aggregate séparé ; le Return augmenté ne sait pas y rattacher
/// la clé. GQL le fait déjà. Aucune requête de Project Orchestrator n'a cette
/// forme (elles trient par l'alias : `count(*) AS c … ORDER BY c`).
#[test]
#[ignore = "DÉFAUT : Cypher ORDER BY count(*) — l'agrégat de la clé n'est pas rattaché à l'Aggregate"]
fn cypher_order_by_agregat() {
    let db = base();
    let lignes = executer(
        &db,
        "cypher",
        "MATCH (n:Note) RETURN n.created_at AS t, count(*) AS c ORDER BY count(*) DESC, t",
    );
    assert_eq!(lignes.len(), 5);
}

#[test]
fn gql_order_by_agregat() {
    let db = base();
    let lignes = executer(
        &db,
        "gql",
        "MATCH (n:Note) RETURN n.created_at AS t, count(*) AS c ORDER BY count(*) DESC, t",
    );
    // t4 porte deux notes (d et e), les autres une.
    assert_eq!(
        lignes[0],
        vec![Value::String("t4".into()), Value::Int64(2)],
        "{lignes:?}"
    );
    assert_eq!(lignes.len(), 4);
}
