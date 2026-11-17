//! #209 — `CALL db.index.vector.queryNodes(index_name, k, query_vector)
//! YIELD node, score` : la forme Neo4j de la recherche vectorielle, en
//! DÉLÉGATION vers `ObrainDB::vector_search`.
//!
//! Ce que ce fichier prouve :
//!   (a) les lignes de la procédure ÉGALENT celles de `vector_search` —
//!       mêmes nœuds, même ordre, `score == 1 - distance` au bit près — en
//!       768 dimensions (celles de PO, projection apprise) comme en 16 ;
//!   (b) la requête PO complète (`YIELD node AS n, score WHERE … RETURN n,
//!       score ORDER BY score DESC LIMIT`) rend ce que rend `vector_search`
//!       filtré à la main ;
//!   (c) un nom inconnu, un index retiré, une dimension fausse : erreurs
//!       explicites qui nomment l'index — jamais un résultat vide ;
//!   (d) le nom est DURABLE : il survit à la réouverture, et la procédure
//!       marche sur une base rouverte sans appel préalable à `vector_search` ;
//!   (e) un catalogue écrit AVANT #209 (cinq colonnes) se relit à l'identique.
//!
//! Le rouge sur main (procédure inconnue) est dans `vector_query_nodes_209_rouge.rs`.

#![cfg(all(feature = "cypher", feature = "vector-index", feature = "algos"))]

use std::collections::HashMap;
use std::path::Path;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_engine::ObrainDB;

struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn signe(&mut self) -> f32 {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) as f32 * 2.0 - 1.0
    }
    fn vecteur(&mut self, d: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..d).map(|_| self.signe()).collect();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v {
            *x /= n;
        }
        v
    }
}

const STATUTS: [&str; 3] = ["active", "needs_review", "archived"];
const PROJETS: [&str; 2] = ["p1", "p2"];

/// `n` notes en `d` dimensions, statut et projet tournants, `id` = rang.
fn notes(db: &ObrainDB, n: usize, d: usize, graine: u64) -> Vec<NodeId> {
    let mut rng = SplitMix(graine);
    (0..n)
        .map(|i| {
            let id = db.create_node(&["Note"]);
            db.set_node_property(id, "embedding", Value::Vector(rng.vecteur(d).into()));
            db.set_node_property(id, "status", Value::String(STATUTS[i % 3].into()));
            db.set_node_property(id, "project_id", Value::String(PROJETS[i % 2].into()));
            db.set_node_property(id, "id", Value::Int64(i as i64));
            id
        })
        .collect()
}

fn liste(v: &[f32]) -> Value {
    Value::List(
        v.iter()
            .map(|x| Value::Float64(f64::from(*x)))
            .collect::<Vec<_>>()
            .into(),
    )
}

/// `(nœud, score)` de `CALL … YIELD node, score` (colonne `node` relue par `id(node)`).
fn query_nodes(db: &ObrainDB, nom: &str, nombre: i64, vecteur: &[f32]) -> Vec<(u64, f64)> {
    let params = HashMap::from([
        ("nombre".to_string(), Value::Int64(nombre)),
        ("vecteur".to_string(), liste(vecteur)),
    ]);
    let resultat = db
        .execute_cypher_with_params(
            &format!(
                "CALL db.index.vector.queryNodes('{nom}', $nombre, $vecteur) YIELD node, score \
                 RETURN id(node) AS identifiant, score"
            ),
            params,
        )
        .expect("CALL db.index.vector.queryNodes");
    resultat
        .rows
        .iter()
        .map(|ligne| {
            let identifiant = match &ligne[0] {
                Value::Int64(valeur) => *valeur as u64,
                autre => panic!("id(node) doit être Int64, reçu {autre:?}"),
            };
            let score = match &ligne[1] {
                Value::Float64(valeur) => *valeur,
                autre => panic!("score doit être Float64, reçu {autre:?}"),
            };
            (identifiant, score)
        })
        .collect()
}

/// Ce que `vector_search` rend, exprimé en `(nœud, 1 - distance)`.
fn attendu(db: &ObrainDB, q: &[f32], k: usize) -> Vec<(u64, f64)> {
    db.vector_search("Note", "embedding", q, k, None, None)
        .expect("vector_search")
        .into_iter()
        .map(|(id, d)| (id.0, f64::from(1.0_f32 - d)))
        .collect()
}

fn egalite_avec_vector_search(d: usize) {
    let db = ObrainDB::new_in_memory();
    notes(&db, 300, d, 7);
    db.create_named_vector_index(
        "note_embeddings",
        "Note",
        "embedding",
        Some(d),
        Some("cosine"),
        None,
        None,
    )
    .expect("index nommé");
    let mut rng = SplitMix(99);
    for k in [1usize, 5, 20, 300] {
        for _ in 0..5 {
            let q = rng.vecteur(d);
            let obtenu = query_nodes(&db, "note_embeddings", k as i64, &q);
            let attendu = attendu(&db, &q, k);
            assert_eq!(
                obtenu, attendu,
                "d={d} k={k} : la procédure doit rendre EXACTEMENT vector_search"
            );
            assert!(
                obtenu.windows(2).all(|w| w[0].1 >= w[1].1),
                "scores par ordre décroissant : {obtenu:?}"
            );
        }
    }
}

// (a) Égalité stricte avec vector_search, en 768 dimensions (PO, nomic-embed-text).
#[test]
fn resultats_egaux_a_vector_search_768() {
    egalite_avec_vector_search(768);
}

// (a') Et en 16 dimensions (projection identité).
#[test]
fn resultats_egaux_a_vector_search_16() {
    egalite_avec_vector_search(16);
}

/// La requête de PO (backend/src/neo4j/note.rs), mot pour mot.
const REQUETE_PO: &str = "CALL db.index.vector.queryNodes('note_embeddings', $query_limit, $embedding) \
     YIELD node AS n, score \
     WHERE n.status IN ['active', 'needs_review'] AND n.project_id = $project_id \
     RETURN n, score ORDER BY score DESC LIMIT $limit";

// (b) La requête PO complète : AS, WHERE après YIELD, ORDER BY, LIMIT, paramètres.
#[test]
fn requete_po_complete() {
    let db = ObrainDB::new_in_memory();
    let ids = notes(&db, 200, 32, 3);
    db.create_vector_index("Note", "embedding", Some(32), Some("cosine"), None, None)
        .expect("index (migrateur : sans nom)");
    db.name_vector_index("note_embeddings", "Note", "embedding")
        .expect("nommer l'index existant");

    let q = SplitMix(5).vecteur(32);
    let (query_limit, limit) = (40usize, 7usize);
    let params = HashMap::from([
        ("query_limit".to_string(), Value::Int64(query_limit as i64)),
        ("embedding".to_string(), liste(&q)),
        ("project_id".to_string(), Value::String("p1".into())),
        ("limit".to_string(), Value::Int64(limit as i64)),
    ]);
    let r = db
        .execute_cypher_with_params(REQUETE_PO, params.clone())
        .expect("requête PO");
    assert_eq!(r.columns, vec!["n".to_string(), "score".to_string()]);

    // L'oracle : vector_search (top query_limit), filtré à la main comme le WHERE.
    let rang = |id: u64| ids.iter().position(|x| x.0 == id).expect("nœud connu");
    let mut oracle: Vec<(u64, f64)> = attendu(&db, &q, query_limit)
        .into_iter()
        .filter(|(id, _)| {
            let i = rang(*id);
            STATUTS[i % 3] != "archived" && PROJETS[i % 2] == "p1"
        })
        .collect();
    oracle.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    oracle.truncate(limit);
    assert!(
        oracle.len() == limit,
        "le banc doit avoir assez de candidats"
    );

    // `n` est un NŒUD résolu (ses propriétés), pas un identifiant scalaire.
    let obtenu: Vec<(u64, f64)> = r
        .rows
        .iter()
        .map(|row| {
            let Value::Map(m) = &row[0] else {
                panic!("n doit être un nœud résolu, reçu {:?}", row[0]);
            };
            let id = match m.get(&PropertyKey::new("_id")) {
                Some(Value::Int64(i)) => *i as u64,
                autre => panic!("nœud sans _id : {m:?} ({autre:?})"),
            };
            let Value::Float64(s) = row[1] else {
                panic!("score Float64 attendu");
            };
            (id, s)
        })
        .collect();
    assert_eq!(obtenu, oracle, "requête PO ≠ vector_search filtré");

    // Les propriétés de `n` se lisent aussi en projection.
    let r2 = db
        .execute_cypher_with_params(
            "CALL db.index.vector.queryNodes('note_embeddings', $query_limit, $embedding) \
             YIELD node AS n, score \
             WHERE n.status IN ['active', 'needs_review'] AND n.project_id = $project_id \
             RETURN n.id AS id, n.status AS status, score ORDER BY score DESC LIMIT $limit",
            params,
        )
        .expect("projection de propriétés");
    let ids2: Vec<u64> = r2
        .rows
        .iter()
        .map(|row| match &row[0] {
            Value::Int64(i) => ids[*i as usize].0,
            autre => panic!("n.id : {autre:?}"),
        })
        .collect();
    assert_eq!(ids2, oracle.iter().map(|(i, _)| *i).collect::<Vec<_>>());
    assert!(
        r2.rows
            .iter()
            .all(|row| matches!(&row[1], Value::String(s) if s.as_str() != "archived"))
    );
}

// (b') La même procédure répond en GQL.
#[test]
fn query_nodes_en_gql() {
    let db = ObrainDB::new_in_memory();
    notes(&db, 50, 8, 11);
    db.create_named_vector_index("idx", "Note", "embedding", Some(8), None, None, None)
        .expect("index nommé");
    let q = SplitMix(1).vecteur(8);
    let params = HashMap::from([("v".to_string(), liste(&q))]);
    let r = db
        .execute_with_params(
            "CALL db.index.vector.queryNodes('idx', 5, $v) YIELD node, score",
            params,
        )
        .expect("GQL");
    assert_eq!(r.rows.len(), 5);
    assert_eq!(r.columns, vec!["node".to_string(), "score".to_string()]);
}

// (c) Erreurs explicites : nom inconnu, index retiré, dimension, arguments.
#[test]
fn erreurs_explicites() {
    let db = ObrainDB::new_in_memory();
    notes(&db, 30, 8, 2);
    db.create_named_vector_index(
        "note_embeddings",
        "Note",
        "embedding",
        Some(8),
        None,
        None,
        None,
    )
    .expect("index nommé");
    let q = SplitMix(4).vecteur(8);
    let essai = |requete: &str, v: &[f32]| {
        db.execute_cypher_with_params(requete, HashMap::from([("v".to_string(), liste(v))]))
    };

    let e = essai(
        "CALL db.index.vector.queryNodes('decision_embedding', 3, $v) YIELD node, score RETURN node",
        &q,
    )
    .expect_err("nom inconnu");
    let e = e.to_string();
    assert!(e.contains("'decision_embedding'"), "{e}");
    assert!(e.contains("note_embeddings"), "liste les noms connus : {e}");

    let e = essai(
        "CALL db.index.vector.queryNodes('note_embeddings', 3, $v) YIELD node, score RETURN node",
        &q[..4],
    )
    .expect_err("dimension")
    .to_string();
    assert!(e.contains("Dimension incoherente"), "{e}");

    let e = essai(
        "CALL db.index.vector.queryNodes('note_embeddings', 0, $v) YIELD node, score RETURN node",
        &q,
    )
    .expect_err("k nul")
    .to_string();
    assert!(e.contains("(k)"), "{e}");

    // Un nom déjà pris par un autre (label, propriété) est refusé.
    let n = db.create_node(&["Decision"]);
    db.set_node_property(n, "embedding", Value::Vector(q.clone().into()));
    let e = db
        .create_named_vector_index(
            "note_embeddings",
            "Decision",
            "embedding",
            None,
            None,
            None,
            None,
        )
        .expect_err("nom en double")
        .to_string();
    assert!(e.contains("note_embeddings"), "{e}");
    assert!(
        db.vector_index_names()
            == vec![(
                "note_embeddings".to_string(),
                "Note".to_string(),
                "embedding".to_string()
            )],
        "le refus n'écrit rien"
    );
    // Nommer un index qui n'existe pas est refusé.
    assert!(db.name_vector_index("x", "Absent", "embedding").is_err());

    // Retirer l'index retire le nom : la procédure le dit.
    assert!(db.drop_vector_index("Note", "embedding"));
    let e = essai(
        "CALL db.index.vector.queryNodes('note_embeddings', 3, $v) YIELD node, score RETURN node",
        &q,
    )
    .expect_err("index retiré")
    .to_string();
    assert!(e.contains("'note_embeddings'"), "{e}");
    assert_eq!(
        db.vector_index_names(),
        Vec::<(String, String, String)>::new(),
        "le nom part avec l'index retiré"
    );
}

fn ouvrir(dir: &Path) -> ObrainDB {
    ObrainDB::open_substrate(dir).expect("ouverture de la base substrat")
}

// (d) Le nom survit à la réouverture ; la procédure marche sur une base
// rouverte SANS appel préalable à vector_search (préchargement des index nommés).
#[test]
fn nom_durable_et_base_rouverte() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let q = SplitMix(8).vecteur(24);
    let avant = {
        let db = ouvrir(tmp.path());
        notes(&db, 120, 24, 21);
        db.create_named_vector_index(
            "note_embeddings",
            "Note",
            "embedding",
            Some(24),
            Some("cosine"),
            None,
            None,
        )
        .expect("index nommé");
        query_nodes(&db, "note_embeddings", 10, &q)
    };
    let db = ouvrir(tmp.path());
    assert_eq!(
        db.vector_index_names(),
        vec![(
            "note_embeddings".to_string(),
            "Note".to_string(),
            "embedding".to_string()
        )]
    );
    // Première chose faite sur la base rouverte : la procédure.
    let apres = query_nodes(&db, "note_embeddings", 10, &q);
    assert_eq!(apres.len(), 10);
    assert_eq!(apres, avant, "mêmes résultats après réouverture");
    assert_eq!(apres, attendu(&db, &q, 10));
    drop(db);

    // Lecture pure : une base ouverte en LECTURE SEULE (mode serveur de
    // lecture) sert la procédure au lieu de la refuser comme une écriture.
    let ro = ObrainDB::open_read_only(tmp.path()).expect("ouverture en lecture seule");
    assert_eq!(query_nodes(&ro, "note_embeddings", 10, &q), avant);
}

/// Le catalogue tel que l'écrivait obrain AVANT #209 : en-tête et cinq colonnes.
const CATALOGUE_ANCIEN: &str = "# catalogue d'index obrain — genre\tlabel\tpropriete\tdimension\tmetrique\n\
     property\t\tstatus\t\t\n\
     vector\tNote\tembedding\t16\tcosine\n";

// (e) Rétrocompatibilité : un catalogue d'avant #209 se relit, ses
// déclarations sont intactes, et on peut nommer son index après coup.
#[test]
fn catalogue_ancien_relu() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let q = SplitMix(13).vecteur(16);
    {
        let db = ouvrir(tmp.path());
        notes(&db, 80, 16, 17);
        db.create_property_index("status");
        db.create_vector_index("Note", "embedding", Some(16), Some("cosine"), None, None)
            .expect("index");
    }
    // Remplace le catalogue par l'octet près de ce qu'écrivait l'ancienne version.
    let chemin = tmp.path().join("substrate.indexcat");
    assert!(chemin.exists(), "le catalogue doit exister");
    std::fs::write(&chemin, CATALOGUE_ANCIEN).expect("écrire l'ancien catalogue");

    {
        let db = ouvrir(tmp.path());
        assert_eq!(
            db.vector_index_names(),
            Vec::<(String, String, String)>::new(),
            "un ancien catalogue n'a pas de nom"
        );
        // Les déclarations d'avant se relisent : l'index vectoriel répond…
        let r = attendu(&db, &q, 5);
        assert_eq!(r.len(), 5);
        // … et l'index de propriété aussi.
        assert_eq!(
            db.find_nodes_by_property("status", &Value::String("active".into()))
                .len(),
            27,
            "80 notes, une sur trois active"
        );
        // Nommer l'index de l'ancien catalogue.
        db.name_vector_index("note_embeddings", "Note", "embedding")
            .expect("nommer");
        assert_eq!(query_nodes(&db, "note_embeddings", 5, &q), r);
    }
    // Le catalogue réécrit garde les déclarations d'avant, plus le nom.
    let texte = std::fs::read_to_string(&chemin).expect("catalogue");
    let lignes: Vec<&str> = texte.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        lignes,
        vec![
            "property\t\tstatus\t\t",
            "vector\tNote\tembedding\t16\tcosine\tnote_embeddings"
        ],
        "catalogue : {texte}"
    );
    let db = ouvrir(tmp.path());
    assert_eq!(
        query_nodes(&db, "note_embeddings", 5, &q),
        attendu(&db, &q, 5)
    );
}
