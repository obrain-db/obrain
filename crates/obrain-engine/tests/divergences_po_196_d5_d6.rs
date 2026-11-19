//! #196, familles **D5** et **D6** : deux divergences classées à l'ouverture du
//! ticket, mesurées sur `76230b9b`, et dont le mécanisme n'avait jamais été
//! réduit (« déduit du dump, non réduit »).
//!
//! Ce fichier les réduit et les **épingle**. Sur `main` au 19/11/2026
//! (`b7d32cde`), les deux se comportent comme Neo4j : les tests passent sans
//! aucun correctif. Ils restent utiles comme garde-fou — c'est exactement le
//! genre de régression qu'une optimisation de planneur casse en silence.
//!
//! **D5** — `WITH d, head(collect({…})) AS m` rendait un groupe au lieu de N.
//! **D6** — une variable déjà liée, réutilisée en cible d'un motif suivant, ne
//! contraignait pas ce motif (produit cartésien au lieu d'une fermeture).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test divergences_po_196_d5_d6
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

fn texte(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        other => format!("{other:?}"),
    }
}

/// Deux fonction-graphes : g1 contient deux fichiers, g2 un seul.
fn base_d5() -> ObrainDB {
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

// ---------------------------------------------------------------------------
// D5 — les groupes survivent à `head(collect({…}))`
// ---------------------------------------------------------------------------

#[test]
fn d5_head_collect_de_map_rend_une_ligne_par_groupe() {
    let db = base_d5();
    let r = lignes(
        &db,
        "MATCH (g:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH g, head(collect({path: f.path})) AS premier \
         RETURN g.name AS nom, premier.path AS chemin ORDER BY nom",
    );
    assert_eq!(r.len(), 2, "Neo4j rend une ligne par groupe : {r:?}");
    assert_eq!(texte(&r[0][0]), "g1");
    assert_eq!(texte(&r[1][0]), "g2");
    // g1 a deux fichiers : la carte rendue doit être l'une des deux, jamais vide.
    assert!(["a", "b"].contains(&texte(&r[0][1]).as_str()), "g1 : {r:?}");
    assert_eq!(texte(&r[1][1]), "c", "g2 n'a qu'un fichier : {r:?}");
}

#[test]
fn d5_map_a_deux_entrees_ne_perd_pas_la_cle_de_groupe() {
    let db = base_d5();
    let r = lignes(
        &db,
        "MATCH (g:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH g, head(collect({path: f.path, poids: 1})) AS m \
         RETURN g.name AS nom, m.path AS chemin, m.poids AS poids ORDER BY nom",
    );
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(texte(&r[1][1]), "c", "{r:?}");
    assert!(matches!(r[1][2], Value::Int64(1)), "{r:?}");
}

#[test]
fn d5_collect_distinct_de_map_garde_les_groupes() {
    let db = base_d5();
    let r = lignes(
        &db,
        "MATCH (g:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH g, head(collect(DISTINCT {path: f.path})) AS m \
         RETURN g.name AS nom, m.path AS chemin ORDER BY nom",
    );
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(texte(&r[1][1]), "c", "{r:?}");
}

#[test]
fn d5_temoin_head_collect_de_scalaire() {
    let db = base_d5();
    let r = lignes(
        &db,
        "MATCH (g:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH g, head(collect(f.path)) AS premier \
         RETURN g.name AS nom, premier AS chemin ORDER BY nom",
    );
    assert_eq!(r.len(), 2, "{r:?}");
}

#[test]
fn d5_temoin_group_by_avec_count() {
    let db = base_d5();
    let r = lignes(
        &db,
        "MATCH (g:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH g, count(f) AS n RETURN g.name AS nom, n ORDER BY nom",
    );
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(
        matches!(r[0][1], Value::Int64(2)),
        "g1 contient 2 fichiers : {r:?}"
    );
    assert!(
        matches!(r[1][1], Value::Int64(1)),
        "g2 en contient 1 : {r:?}"
    );
}

#[test]
fn d5_collect_de_map_sans_head_garde_les_groupes() {
    let db = base_d5();
    let r = lignes(
        &db,
        "MATCH (g:FunctionGraph)-[:CONTAINS]->(f:File) \
         WITH g, collect({path: f.path}) AS ms \
         RETURN g.name AS nom, size(ms) AS combien ORDER BY nom",
    );
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(matches!(r[0][1], Value::Int64(2)), "{r:?}");
    assert!(matches!(r[1][1], Value::Int64(1)), "{r:?}");
}

// ---------------------------------------------------------------------------
// D6 — une variable liée plus tôt contraint la fermeture du motif suivant
// ---------------------------------------------------------------------------

/// Trois commits, trois fichiers :
/// c1 touche x, y, z ; c2 touche x, y ; c3 touche x.
fn base_d6() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (c1:Commit {sha: 'c1'}), (c2:Commit {sha: 'c2'}), (c3:Commit {sha: 'c3'}), \
             (x:File {path: 'x'}), (y:File {path: 'y'}), (z:File {path: 'z'}), \
             (c1)-[:TOUCHES]->(x), (c1)-[:TOUCHES]->(y), (c1)-[:TOUCHES]->(z), \
             (c2)-[:TOUCHES]->(x), (c2)-[:TOUCHES]->(y), \
             (c3)-[:TOUCHES]->(x)",
        )
        .unwrap();
    db
}

#[test]
fn d6_cible_deja_liee_contraint_le_second_motif() {
    // Sans fermeture : 4 arêtes c1 et 4 arêtes c2 → 16 lignes.
    // Avec fermeture : c1 et c2 ne se rejoignent que sur x et y → 2 lignes.
    let db = base_d6();
    let r = lignes(
        &db,
        "MATCH (c1:Commit)-[:TOUCHES]->(f:File) \
         MATCH (c2:Commit)-[:TOUCHES]->(f) \
         RETURN count(*) AS n",
    );
    // Couples ordonnés (c1, c2) touchant le même fichier, sans contrainte
    // c1 != c2 : x est touché par 3 commits (9 couples), y par 2 (4), z par 1 (1).
    assert!(
        matches!(r[0][0], Value::Int64(14)),
        "attendu 14 couples, reçu {r:?}"
    );
}

#[test]
fn d6_cible_deja_liee_rend_les_bons_fichiers() {
    let db = base_d6();
    let r = lignes(
        &db,
        "MATCH (c1:Commit)-[:TOUCHES]->(f:File) \
         MATCH (c2:Commit)-[:TOUCHES]->(f) \
         RETURN DISTINCT f.path AS p ORDER BY p",
    );
    let chemins: Vec<String> = r.iter().map(|l| texte(&l[0])).collect();
    // Tout fichier touché par au moins un commit apparaît : (z, z) est un couple
    // valide, la requête n'interdit pas c1 = c2.
    assert_eq!(
        chemins,
        vec!["x".to_string(), "y".to_string(), "z".to_string()]
    );
}

#[test]
fn d6_temoin_depart_lie_contraint_le_second_motif() {
    let db = base_d6();
    let r = lignes(
        &db,
        "MATCH (c1:Commit {sha: 'c3'})-[:TOUCHES]->(f:File) \
         MATCH (c1)-[:TOUCHES]->(g:File) \
         RETURN count(*) AS n",
    );
    assert!(
        matches!(r[0][0], Value::Int64(1)),
        "c3 ne touche que x : {r:?}"
    );
}

#[test]
fn d6_temoin_motif_unique() {
    let db = base_d6();
    let r = lignes(
        &db,
        "MATCH (c:Commit)-[:TOUCHES]->(f:File) RETURN count(*) AS n",
    );
    assert!(
        matches!(r[0][0], Value::Int64(6)),
        "6 arêtes TOUCHES : {r:?}"
    );
}

#[test]
fn d6_label_de_la_cible_est_respecte_apres_liaison() {
    let db = base_d6();
    let r = lignes(
        &db,
        "MATCH (c1:Commit)-[:TOUCHES]->(f:File) \
         MATCH (c2:Commit)-[:TOUCHES]->(f) \
         WHERE c1.sha = 'c1' AND c2.sha = 'c2' \
         RETURN collect(DISTINCT f.path) AS chemins",
    );
    let Value::List(chemins) = &r[0][0] else {
        panic!("attendu une liste : {r:?}");
    };
    let mut noms: Vec<String> = chemins.iter().map(texte).collect();
    noms.sort();
    assert_eq!(noms, vec!["x".to_string(), "y".to_string()]);
}

#[test]
fn d6_propriete_lue_sur_la_variable_liee() {
    let db = base_d6();
    let r = lignes(
        &db,
        "MATCH (c1:Commit)-[:TOUCHES]->(f:File) \
         MATCH (c2:Commit)-[:TOUCHES]->(f) \
         RETURN DISTINCT f.path AS p, c2.sha AS sha ORDER BY p, sha",
    );
    // x est touché par c1, c2, c3 (3 lignes) ; y par c1, c2 (2) ; z par c1 (1).
    assert_eq!(r.len(), 6, "{r:?}");
    assert!(
        r.iter().all(|l| {
            let m = &l[0];
            matches!(m, Value::Map(map) if map.contains_key(&PropertyKey::new("path")))
                || matches!(m, Value::String(_))
        }),
        "{r:?}"
    );
}
