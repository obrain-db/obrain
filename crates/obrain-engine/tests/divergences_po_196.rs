//! Deux divergences silencieuses relevées sur les requêtes réelles de PO
//! (obrain-db/obrain#196, familles D1 et D2).
//!
//! **D1** — un nœud (ou une arête) mêlé à une agrégation sortait en
//! IDENTIFIANT INTERNE (entier) au lieu d'une entité : `RETURN fg, count(e)`
//! rendait `fg = 3`, et `collect(f)` une liste d'entiers. Côté PO,
//! `row.get::<Node>` échoue, et sur une liste `unwrap_or_default()` en fait une
//! liste VIDE, sans erreur. L'oracle est le moteur lui-même : l'entité rendue
//! après agrégation doit être celle que rend `MATCH (fg) RETURN fg`.
//!
//! Témoins : la chaîne `collect(f)` → `UNWIND` → `MATCH` doit continuer de
//! marcher (la liste reste une liste d'identifiants DANS la requête ; seule la
//! sortie est résolue), et les agrégats sur propriétés ne changent pas.
//!
//! **D2** — une propriété vectorielle ABSENTE revenait comme un vecteur de
//! zéros : la colonne dense réserve un emplacement à chaque nœud situé sous
//! son plus haut emplacement écrit, et la lecture unitaire rendait ces octets
//! nuls comme une valeur. `n.embedding IS NULL` était donc faux pour ces nœuds.
//! Neo4j rend NULL.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test divergences_po_196
//! ```
#![cfg(feature = "cypher")]

use std::sync::Arc;

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

fn champ<'a>(v: &'a Value, cle: &str) -> Option<&'a Value> {
    match v {
        Value::Map(m) => m.get(&PropertyKey::new(cle)),
        _ => None,
    }
}

/// Deux `FunctionGraph` (g1 contient deux fichiers, g2 un seul).
fn base_d1() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (g1:FunctionGraph {name: 'g1'}), (g2:FunctionGraph {name: 'g2'}), \
             (a:File {path: 'a'}), (b:File {path: 'b'}), (c:File {path: 'c'}), \
             (g1)-[:CONTAINS {w: 1}]->(a), (g1)-[:CONTAINS {w: 2}]->(b), \
             (g2)-[:CONTAINS {w: 3}]->(c)",
        )
        .unwrap();
    db
}

/// L'entité telle que le moteur la rend hors agrégation.
fn noeud(db: &ObrainDB, label: &str, cle: &str, valeur: &str) -> Value {
    let r = lignes(
        db,
        &format!("MATCH (n:{label}) WHERE n.{cle} = '{valeur}' RETURN n"),
    );
    assert_eq!(r.len(), 1, "{label} {cle}={valeur}");
    let n = r[0][0].clone();
    assert!(
        matches!(champ(&n, "_labels"), Some(Value::List(_))),
        "témoin : RETURN n hors agrégation rend une entité, reçu {n:?}"
    );
    n
}

/// Trie les lignes par une propriété texte de l'entité de la colonne 0.
fn par_nom(mut r: Vec<Vec<Value>>, cle: &str) -> Vec<Vec<Value>> {
    r.sort_by_key(|l| format!("{:?}", champ(&l[0], cle)));
    r
}

#[test]
fn d1_noeud_en_cle_de_groupe_sort_en_noeud() {
    let db = base_d1();
    let g1 = noeud(&db, "FunctionGraph", "name", "g1");
    let g2 = noeud(&db, "FunctionGraph", "name", "g2");
    // La requête de l'issue.
    let r = par_nom(
        lignes(
            &db,
            "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) RETURN fg, count(e) AS n",
        ),
        "name",
    );
    assert_eq!(
        r,
        vec![
            vec![g1.clone(), Value::Int64(2)],
            vec![g2.clone(), Value::Int64(1)]
        ]
    );
    // Même chose après un WITH, et sous un ORDER BY sur l'agrégat.
    let r = lignes(
        &db,
        "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) \
         WITH fg, count(e) AS n RETURN fg, n ORDER BY n DESC",
    );
    assert_eq!(
        r,
        vec![
            vec![g1.clone(), Value::Int64(2)],
            vec![g2.clone(), Value::Int64(1)]
        ]
    );
    // Tri sur une propriété de la clé, rendue entité. (obrain ajoute la clé
    // de tri en colonne de sortie — famille « acceptable » de #196, PO lit
    // par nom : on ne compare que les colonnes demandées.)
    let r: Vec<Vec<Value>> = lignes(
        &db,
        "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) \
         RETURN fg, count(e) AS n ORDER BY fg.name DESC",
    )
    .into_iter()
    .map(|l| l[..2].to_vec())
    .collect();
    assert_eq!(
        r,
        vec![
            vec![g2.clone(), Value::Int64(1)],
            vec![g1.clone(), Value::Int64(2)]
        ]
    );
    // Alias de la clé.
    let r = lignes(
        &db,
        "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) \
         WITH fg AS g, count(e) AS n WHERE n > 1 RETURN g",
    );
    assert_eq!(r, vec![vec![g1]]);
}

#[test]
fn d1_arete_en_cle_de_groupe_sort_en_arete() {
    let db = base_d1();
    let r = lignes(
        &db,
        "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) WHERE e.w = 3 RETURN e, count(f) AS n",
    );
    assert_eq!(r.len(), 1);
    assert_eq!(
        champ(&r[0][0], "_type"),
        Some(&Value::String("CONTAINS".into())),
        "arête attendue, reçu {:?}",
        r[0][0]
    );
    assert_eq!(champ(&r[0][0], "w"), Some(&Value::Int64(3)));
    assert_eq!(r[0][1], Value::Int64(1));
}

#[test]
fn d1_collect_de_noeuds_sort_en_liste_de_noeuds() {
    let db = base_d1();
    let a = noeud(&db, "File", "path", "a");
    let b = noeud(&db, "File", "path", "b");
    let c = noeud(&db, "File", "path", "c");
    let tri = |v: &Value| -> Vec<Value> {
        let Value::List(l) = v else {
            panic!("liste attendue, reçu {v:?}")
        };
        let mut l = l.to_vec();
        l.sort_by_key(|x| format!("{:?}", champ(x, "path")));
        l
    };
    // Sans clé de groupe.
    let r = lignes(&db, "MATCH (f:File) RETURN collect(f) AS files");
    assert_eq!(tri(&r[0][0]), vec![a.clone(), b.clone(), c.clone()]);
    // Avec clé de groupe, et à travers un WITH.
    for q in [
        "MATCH (fg:FunctionGraph)-[:CONTAINS]->(f:File) RETURN fg.name AS g, collect(f) AS files",
        "MATCH (fg:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH fg.name AS g, collect(f) AS fs RETURN g, fs AS files",
    ] {
        let mut r = lignes(&db, q);
        r.sort_by_key(|l| format!("{:?}", l[0]));
        assert_eq!(r.len(), 2, "{q}");
        assert_eq!(r[0][0], Value::String("g1".into()), "{q}");
        assert_eq!(tri(&r[0][1]), vec![a.clone(), b.clone()], "{q}");
        assert_eq!(tri(&r[1][1]), vec![c.clone()], "{q}");
    }
    // collect d'arêtes.
    let r = lignes(
        &db,
        "MATCH (:FunctionGraph)-[e:CONTAINS]->() RETURN collect(e) AS es",
    );
    let Value::List(es) = &r[0][0] else {
        panic!("liste attendue, reçu {:?}", r[0][0])
    };
    assert_eq!(es.len(), 3);
    for e in es.iter() {
        assert_eq!(
            champ(e, "_type"),
            Some(&Value::String("CONTAINS".into())),
            "arête attendue, reçu {e:?}"
        );
    }
}

/// Les formes exactes de PO (`task.rs`, `environment.rs`, `plan.rs`) :
/// `OPTIONAL MATCH` + `collect(DISTINCT x)`, tranche d'une liste collectée,
/// concaténation filtrée de deux listes collectées.
#[test]
fn d1_formes_po() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (t1:Task {id: 't1'}), (t2:Task {id: 't2'}), \
             (s1:Step {order: 1}), (s2:Step {order: 2}), \
             (t1)-[:HAS_STEP]->(s1), (t1)-[:HAS_STEP]->(s2), \
             (e:Environment {name: 'prod'}), \
             (d1:Deployment {n: 1}), (d2:Deployment {n: 2}), (d3:Deployment {n: 3}), \
             (e)-[:HAS_DEPLOYMENT]->(d1), (e)-[:HAS_DEPLOYMENT]->(d2), \
             (e)-[:HAS_DEPLOYMENT]->(d3), \
             (m:Milestone {id: 'm'}), (p:Plan), \
             (m)-[:INCLUDES_TASK]->(t1), (p)-[:TARGETS_MILESTONE]->(m), \
             (p)-[:HAS_TASK]->(t1), (p)-[:HAS_TASK]->(t2)",
        )
        .unwrap();
    let t1 = noeud(&db, "Task", "id", "t1");
    let t2 = noeud(&db, "Task", "id", "t2");
    let ordre = |v: &Value, cle: &str| -> Vec<Value> {
        let Value::List(l) = v else {
            panic!("liste attendue, reçu {v:?}")
        };
        l.iter()
            .map(|x| match champ(x, cle) {
                Some(v) => v.clone(),
                None => panic!("entité attendue, reçu {x:?}"),
            })
            .collect()
    };

    // task.rs — une tâche avec ses étapes, une sans.
    for (id, attendu) in [("t1", vec![1, 2]), ("t2", vec![])] {
        let r = lignes(
            &db,
            &format!(
                "MATCH (t:Task {{id: '{id}'}}) OPTIONAL MATCH (t)-[:HAS_STEP]->(s:Step) \
                 RETURN t, collect(DISTINCT s) AS steps"
            ),
        );
        assert_eq!(r.len(), 1, "{id}");
        assert_eq!(r[0][0], if id == "t1" { t1.clone() } else { t2.clone() });
        let mut ordres = ordre(&r[0][1], "order");
        ordres.sort_by_key(|v| format!("{v:?}"));
        assert_eq!(
            ordres,
            attendu.into_iter().map(Value::Int64).collect::<Vec<_>>(),
            "{id}"
        );
    }

    // environment.rs — les deux derniers déploiements.
    let r = lignes(
        &db,
        "MATCH (e:Environment) OPTIONAL MATCH (e)-[:HAS_DEPLOYMENT]->(d:Deployment) \
         WITH e, d ORDER BY d.n DESC \
         WITH e, collect(d) AS deployments \
         RETURN e, deployments[0..2] AS recent ORDER BY e.name ASC",
    );
    assert_eq!(r.len(), 1);
    assert_eq!(champ(&r[0][0], "name"), Some(&Value::String("prod".into())));
    assert_eq!(ordre(&r[0][1], "n"), vec![Value::Int64(3), Value::Int64(2)]);
    // Un élément de la liste.
    let r = lignes(
        &db,
        "MATCH (e:Environment)-[:HAS_DEPLOYMENT]->(d:Deployment) \
         WITH e, d ORDER BY d.n DESC WITH collect(d) AS ds \
         RETURN ds[0] AS dernier, head(ds) AS tete",
    );
    assert_eq!(
        champ(&r[0][0], "n"),
        Some(&Value::Int64(3)),
        "{:?}",
        r[0][0]
    );
    assert_eq!(r[0][0], r[0][1]);

    // plan.rs — `a + [y IN b WHERE NOT y IN a]` : les deux moitiés sont
    // désormais justes, mesurées sur des entiers.
    //  * `[1,2] + [2,3]` rendait NULL : la concaténation de listes manquait dans
    //    `BinaryFilterOp::Add`. Corrigée par obrain#230 (liste+liste,
    //    liste+élément, élément+liste).
    //  * `[y IN [2,3] WHERE NOT y IN [1,2]]` rend `[3]` — déjà correct, la note
    //    précédente qui annonçait `[]` était périmée.
    // La forme complète reste hors de ce test-ci, qui porte sur les entités.
}

/// Témoins : ce qui marchait doit continuer de marcher.
#[test]
fn d1_temoins_chainage_et_scalaires() {
    let db = base_d1();
    // La liste collectée reste utilisable comme entités dans la requête.
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) WITH collect(f) AS fs UNWIND fs AS g \
             MATCH (x)-[:CONTAINS]->(g) RETURN count(x) AS n"
        ),
        vec![vec![Value::Int64(3)]]
    );
    // La clé de groupe reste utilisable comme nœud dans la requête.
    let mut r = lignes(
        &db,
        "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) WITH fg, count(e) AS n \
         MATCH (fg)-[:CONTAINS]->(y) RETURN fg.name AS g, n, count(y) AS m",
    );
    r.sort_by_key(|l| format!("{:?}", l[0]));
    assert_eq!(
        r,
        vec![
            vec![Value::String("g1".into()), Value::Int64(2), Value::Int64(2)],
            vec![Value::String("g2".into()), Value::Int64(1), Value::Int64(1)]
        ]
    );
    // Agrégats de propriétés : inchangés.
    let mut r = lignes(
        &db,
        "MATCH (fg:FunctionGraph)-[e:CONTAINS]->(f:File) \
         RETURN fg.name AS g, collect(f.path) AS ps, sum(e.w) AS s",
    );
    r.sort_by_key(|l| format!("{:?}", l[0]));
    assert_eq!(r[0][0], Value::String("g1".into()));
    assert_eq!(r[0][2], Value::Int64(3));
    assert_eq!(r[1][2], Value::Int64(3));
    let Value::List(ps) = &r[1][1] else {
        panic!("{:?}", r[1][1])
    };
    assert_eq!(ps.to_vec(), vec![Value::String("c".into())]);
    // Un entier groupé reste un entier.
    assert_eq!(
        lignes(
            &db,
            "UNWIND [1, 1, 2] AS x RETURN x, count(*) AS n ORDER BY x"
        ),
        vec![
            vec![Value::Int64(1), Value::Int64(2)],
            vec![Value::Int64(2), Value::Int64(1)]
        ]
    );
    assert_eq!(
        lignes(&db, "UNWIND [1, 2] AS x RETURN collect(x) AS l"),
        vec![vec![Value::List(
            vec![Value::Int64(1), Value::Int64(2)].into()
        )]]
    );
}

/// Trois notes : seule la DEUXIÈME a un embedding. La première occupe donc un
/// emplacement de la colonne dense sous le plus haut emplacement écrit ; la
/// troisième est au-delà.
fn base_d2() -> ObrainDB {
    base_d2_ids().0
}

fn base_d2_ids() -> (ObrainDB, [obrain_common::types::NodeId; 3]) {
    let db = ObrainDB::new_in_memory();
    let n0 = db.create_node(&["Note"]);
    db.set_node_property(n0, "name", Value::String("n0".into()));
    let n1 = db.create_node(&["Note"]);
    db.set_node_property(n1, "name", Value::String("n1".into()));
    db.set_node_property(
        n1,
        "embedding",
        Value::Vector(Arc::from(vec![0.5_f32, 0.25, 1.0, -1.0])),
    );
    let n2 = db.create_node(&["Note"]);
    db.set_node_property(n2, "name", Value::String("n2".into()));
    (db, [n0, n1, n2])
}

#[test]
fn d2_embedding_absent_vaut_null() {
    let db = base_d2();
    let r = lignes(
        &db,
        "MATCH (n:Note) RETURN n.name AS name, n.embedding AS e ORDER BY name",
    );
    assert_eq!(
        r,
        vec![
            vec![Value::String("n0".into()), Value::Null],
            vec![
                Value::String("n1".into()),
                Value::Vector(Arc::from(vec![0.5_f32, 0.25, 1.0, -1.0]))
            ],
            vec![Value::String("n2".into()), Value::Null],
        ]
    );
    // `list_notes_without_embedding` de PO.
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Note) WHERE n.embedding IS NULL RETURN n.name AS name ORDER BY name"
        ),
        vec![
            vec![Value::String("n0".into())],
            vec![Value::String("n2".into())]
        ]
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (n:Note) WHERE n.embedding IS NOT NULL RETURN n.name AS name"
        ),
        vec![vec![Value::String("n1".into())]]
    );
}

#[test]
fn d2_lecture_unitaire_du_store() {
    let (db, [n0, n1, n2]) = base_d2_ids();
    let cle = PropertyKey::new("embedding");
    assert_eq!(db.store().get_node_property(n0, &cle), None, "n0");
    assert!(matches!(
        db.store().get_node_property(n1, &cle),
        Some(Value::Vector(_))
    ));
    assert_eq!(db.store().get_node_property(n2, &cle), None, "n2");
}
