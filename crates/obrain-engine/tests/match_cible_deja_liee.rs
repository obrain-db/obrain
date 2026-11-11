//! Un `MATCH` dont le motif retombe sur une variable DÉJÀ LIÉE par une clause
//! précédente doit se fermer sur elle, pas en lier une nouvelle.
//!
//! ```text
//! MATCH (c:Commit)-[:TOUCHES]->(f:File)
//! MATCH (c2:Commit)-[:TOUCHES]->(f)      -- f est la cible, et f est déjà lié
//! ```
//!
//! Le second `MATCH` ne garde que les commits qui touchent CE fichier. Sans la
//! fermeture, il rend le produit de toutes les arêtes par toutes les arêtes.
//!
//! Chaque forme est jouée en Cypher et, quand GQL la connaît, en GQL ; les
//! valeurs attendues sont écrites à la main d'après le graphe de [`base`].
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test match_cible_deja_liee
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Trois commits, trois fichiers.
///
/// | arête   | c1      | c2   | c3 |
/// |---------|---------|------|----|
/// | TOUCHES | x, y, z | x, y | x  |
/// | REVIEWS | x       | z    |    |
///
/// Par fichier : x est touché par {c1, c2, c3}, y par {c1, c2}, z par {c1}.
/// x porte aussi le label `Hot`. Trois arêtes `OWNS` portent une propriété :
/// c1 -> x (since 2020), c2 -> y (since 2021), c3 -> y (since 2020).
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let fichier = |chemin: &str, labels: &[&str]| {
        session.create_node_with_props(labels, [("path", Value::String(chemin.into()))])
    };
    let x = fichier("x", &["File", "Hot"]);
    let (y, z) = (fichier("y", &["File"]), fichier("z", &["File"]));
    for (sha, touches, relit) in [
        ("c1", vec![x, y, z], vec![x]),
        ("c2", vec![x, y], vec![z]),
        ("c3", vec![x], vec![]),
    ] {
        let commit =
            session.create_node_with_props(&["Commit"], [("sha", Value::String(sha.into()))]);
        for f in touches {
            session.create_edge(commit, f, "TOUCHES");
        }
        for f in relit {
            session.create_edge(commit, f, "REVIEWS");
        }
    }
    for (sha, chemin, depuis) in [("c1", "x", 2020), ("c2", "y", 2021), ("c3", "y", 2020)] {
        session
            .execute(&format!(
                "MATCH (c:Commit {{sha: '{sha}'}}), (f:File {{path: '{chemin}'}}) \
                 INSERT (c)-[:OWNS {{since: {depuis}}}]->(f)"
            ))
            .unwrap();
    }
    db
}

/// Le plan logique d'une requête Cypher, tel que `EXPLAIN` le rend.
fn plan(db: &ObrainDB, requete: &str) -> String {
    let r = db
        .session()
        .execute_cypher(&format!("EXPLAIN {requete}"))
        .unwrap();
    r.rows[0][0].to_string()
}

#[derive(Clone, Copy, Debug)]
enum Langage {
    Cypher,
    Gql,
}

/// Lignes du résultat, triées (l'ordre n'est pas garanti sans `ORDER BY`).
fn lignes(db: &ObrainDB, langage: Langage, requete: &str) -> Vec<Vec<Value>> {
    let session = db.session();
    let resultat = match langage {
        Langage::Cypher => session.execute_cypher(requete),
        Langage::Gql => session.execute(requete),
    };
    let mut l = match resultat {
        Ok(r) => r.rows,
        Err(e) => panic!("[{langage:?}] {requete}\n  -> {e}"),
    };
    l.sort_by_key(|ligne| format!("{ligne:?}"));
    l
}

/// Une ligne de chaînes.
fn l(valeurs: &[&str]) -> Vec<Value> {
    valeurs.iter().map(|v| Value::String((*v).into())).collect()
}

/// (fichier, commit, commit) : pour chaque fichier, toutes les paires de
/// commits qui le touchent. x : 3 x 3 = 9 ; y : 2 x 2 = 4 ; z : 1. Total 14.
fn paires_de_commits_par_fichier() -> Vec<Vec<Value>> {
    let mut attendu = Vec::new();
    for (fichier, commits) in [
        ("x", vec!["c1", "c2", "c3"]),
        ("y", vec!["c1", "c2"]),
        ("z", vec!["c1"]),
    ] {
        for a in &commits {
            for b in &commits {
                attendu.push(l(&[fichier, a, b]));
            }
        }
    }
    assert_eq!(attendu.len(), 14);
    attendu
}

/// (fichier, commit) : les six arêtes TOUCHES.
fn aretes_touches() -> Vec<Vec<Value>> {
    vec![
        l(&["x", "c1"]),
        l(&["x", "c2"]),
        l(&["x", "c3"]),
        l(&["y", "c1"]),
        l(&["y", "c2"]),
        l(&["z", "c1"]),
    ]
}

// ============================================================================
// La variable liée est la CIBLE du second motif
// ============================================================================

/// Arête sortante : `(c2)-[:TOUCHES]->(f)`, f lié.
fn cible_liee_arete_sortante(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             MATCH (c2:Commit)-[:TOUCHES]->(f) \
             RETURN f.path, c.sha, c2.sha",
        ),
        paires_de_commits_par_fichier(),
        "{langage:?}"
    );
}

/// Arête entrante : `(f2)<-[:TOUCHES]-(c)`, c lié. Par commit, toutes les
/// paires de fichiers qu'il touche : c1 : 3 x 3 = 9 ; c2 : 4 ; c3 : 1. Total 14.
fn cible_liee_arete_entrante(langage: Langage) {
    let db = base();
    let mut attendu = Vec::new();
    for (commit, fichiers) in [
        ("c1", vec!["x", "y", "z"]),
        ("c2", vec!["x", "y"]),
        ("c3", vec!["x"]),
    ] {
        for a in &fichiers {
            for b in &fichiers {
                attendu.push(l(&[commit, a, b]));
            }
        }
    }
    assert_eq!(attendu.len(), 14);
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (f:File)<-[:TOUCHES]-(c:Commit) \
             MATCH (f2:File)<-[:TOUCHES]-(c) \
             RETURN c.sha, f.path, f2.path",
        ),
        attendu,
        "{langage:?}"
    );
}

/// Les DEUX bouts du second motif sont liés : il ne reste que les couples
/// (commit, fichier) reliés par TOUCHES et par REVIEWS. c1 relit x, qu'il
/// touche ; c2 relit z, qu'il ne touche pas. Une seule ligne.
fn depart_et_cible_lies(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             MATCH (c)-[:REVIEWS]->(f) \
             RETURN c.sha, f.path",
        ),
        vec![l(&["c1", "x"])],
        "{langage:?}"
    );
}

/// Trois `MATCH` qui retombent sur le même fichier : par fichier, les triplets
/// de commits. x : 3^3 = 27 ; y : 2^3 = 8 ; z : 1. Total 36.
fn trois_match_sur_la_meme_cible(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Commit)-[:TOUCHES]->(f:File) \
             MATCH (b:Commit)-[:TOUCHES]->(f) \
             MATCH (c:Commit)-[:TOUCHES]->(f) \
             RETURN f.path, count(*) AS n",
        ),
        vec![
            vec![Value::String("x".into()), Value::Int64(27)],
            vec![Value::String("y".into()), Value::Int64(8)],
            vec![Value::String("z".into()), Value::Int64(1)],
        ],
        "{langage:?}"
    );
}

/// Garde-fou : repartir de la variable liée (elle est le DÉPART du second
/// motif) rendait déjà le bon résultat.
fn depart_lie(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             MATCH (f)<-[:TOUCHES]-(c2:Commit) \
             RETURN f.path, c.sha, c2.sha",
        ),
        paires_de_commits_par_fichier(),
        "{langage:?}"
    );
}

/// Le départ lié garde les labels que le second motif lui écrit : `f` est un
/// `File`, jamais un `Commit` — aucune ligne. Avec le bon label, les 14 paires.
fn depart_lie_garde_ses_labels(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             MATCH (f:Commit)<-[:TOUCHES]-(c2:Commit) \
             RETURN f.path, c.sha, c2.sha",
        ),
        Vec::<Vec<Value>>::new(),
        "{langage:?}"
    );
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f) \
             MATCH (f:File)<-[:TOUCHES]-(c2:Commit) \
             RETURN f.path, c.sha, c2.sha",
        ),
        paires_de_commits_par_fichier(),
        "{langage:?}"
    );
}

/// La variable liée est au MILIEU du second motif : il repart d'elle dans les
/// deux sens. x est relu par c1 et touché par c1, c2, c3.
fn ancre_au_milieu_du_motif(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (f:File {path: 'x'}) \
             MATCH (a:Commit)-[:REVIEWS]->(f)<-[:TOUCHES]-(b:Commit) \
             RETURN a.sha, b.sha",
        ),
        vec![l(&["c1", "c1"]), l(&["c1", "c2"]), l(&["c1", "c3"])],
        "{langage:?}"
    );
}

/// Un motif qui revient sur SA PROPRE variable se ferme dessus : les commits
/// qui touchent ET relisent le même fichier. c1 relit x, qu'il touche ; c2
/// relit z, qu'il ne touche pas.
fn cycle_dans_un_seul_motif(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Commit)-[:TOUCHES]->(f:File)<-[:REVIEWS]-(a) RETURN a.sha, f.path",
        ),
        vec![l(&["c1", "x"])],
        "{langage:?}"
    );
}

// ============================================================================
// Contraintes écrites sur la cible et sur la relation
// ============================================================================

/// Les propriétés en ligne du nœud d'arrivée filtrent : seuls les commits qui
/// touchent x.
fn proprietes_en_ligne_de_la_cible(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Commit)-[:TOUCHES]->(f:File {path: 'x'}) RETURN a.sha",
        ),
        vec![l(&["c1"]), l(&["c2"]), l(&["c3"])],
        "{langage:?}"
    );
}

/// TOUS les labels du nœud d'arrivée filtrent : seul x est `File` et `Hot` ;
/// aucun nœud n'est `File` et `Commit`.
fn tous_les_labels_de_la_cible(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Commit)-[:TOUCHES]->(f:File:Hot) RETURN a.sha, f.path",
        ),
        vec![l(&["c1", "x"]), l(&["c2", "x"]), l(&["c3", "x"])],
        "{langage:?}"
    );
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Commit)-[:TOUCHES]->(f:File:Commit) RETURN a.sha, f.path",
        ),
        Vec::<Vec<Value>>::new(),
        "{langage:?}"
    );
}

/// Les propriétés en ligne d'une relation NOMMÉE filtrent : une seule arête
/// `OWNS` date de 2021.
fn proprietes_en_ligne_d_une_relation_nommee(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[o:OWNS {since: 2021}]->(f:File) RETURN c.sha, f.path",
        ),
        vec![l(&["c2", "y"])],
        "{langage:?}"
    );
}

/// Les propriétés en ligne d'une relation ANONYME filtrent aussi : deux arêtes
/// `OWNS` datent de 2020.
fn proprietes_en_ligne_d_une_relation_anonyme(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:OWNS {since: 2020}]->(f:File) RETURN c.sha, f.path",
        ),
        vec![l(&["c1", "x"]), l(&["c3", "y"])],
        "{langage:?}"
    );
}

// ============================================================================
// Le plan : on repart de la variable liée
// ============================================================================

/// Le second `MATCH` ne balaie pas les commits et ne joint pas deux
/// sous-plans : il parcourt, depuis `f`, ses arêtes entrantes.
#[test]
fn le_second_match_repart_de_la_variable_liee() {
    let db = base();
    let p = plan(
        &db,
        "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
         MATCH (c2:Commit)-[:TOUCHES]->(f) \
         RETURN c.sha, c2.sha",
    );
    assert!(p.contains("Expand (f)<-[:TOUCHES]<-(c2)"), "{p}");
    assert!(!p.contains("Join"), "{p}");
    assert!(!p.contains("NodeScan (c2"), "{p}");
}

/// Les deux bouts liés : une expansion depuis `c`, puis l'égalité avec `f`.
/// Aucun nouveau balayage.
#[test]
fn les_deux_bouts_lies_sont_une_verification() {
    let db = base();
    let p = plan(
        &db,
        "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
         MATCH (c)-[:REVIEWS]->(f) \
         RETURN c.sha, f.path",
    );
    assert!(!p.contains("Join"), "{p}");
    assert_eq!(p.matches("NodeScan (c:Commit)").count(), 1, "{p}");
    assert!(p.contains("Eq f)"), "{p}");
}

// ============================================================================
// Avec un WITH entre les deux MATCH
// ============================================================================

/// `WITH f` garde une ligne par arête (6) ; chacune se ferme sur les commits
/// de son fichier : x : 3 lignes x 3 commits ; y : 2 x 2 ; z : 1. Total 14.
fn cible_liee_apres_with(langage: Langage) {
    let db = base();
    let mut attendu = Vec::new();
    for (fichier, commits) in [
        ("x", vec!["c1", "c2", "c3"]),
        ("y", vec!["c1", "c2"]),
        ("z", vec!["c1"]),
    ] {
        for _ in &commits {
            for b in &commits {
                attendu.push(l(&[fichier, b]));
            }
        }
    }
    attendu.sort_by_key(|ligne| format!("{ligne:?}"));
    assert_eq!(attendu.len(), 14);
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH f \
             MATCH (c2:Commit)-[:TOUCHES]->(f) \
             RETURN f.path, c2.sha",
        ),
        attendu,
        "{langage:?}"
    );
}

/// `WITH DISTINCT f` : une ligne par fichier, puis ses commits — les 6 arêtes.
fn cible_liee_apres_with_distinct(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH DISTINCT f \
             MATCH (c2:Commit)-[:TOUCHES]->(f) \
             RETURN f.path, c2.sha",
        ),
        aretes_touches(),
        "{langage:?}"
    );
}

/// Le `WITH` a fait sortir `c` de la portée : le `c` du second `MATCH` est une
/// variable NEUVE, il ne se ferme pas sur l'ancien. Même résultat que
/// [`cible_liee_apres_with_distinct`].
fn variable_sortie_de_portee_par_with_n_est_plus_liee(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH DISTINCT f \
             MATCH (c:Commit)-[:TOUCHES]->(f) \
             RETURN f.path, c.sha",
        ),
        aretes_touches(),
        "{langage:?}"
    );
}

/// `WITH` agrégé : le nombre de commits par fichier (x : 3, y : 2, z : 1),
/// puis chaque commit du fichier.
fn cible_liee_apres_with_agrege(langage: Langage) {
    let db = base();
    let ligne = |f: &str, nb: i64, c: &str| {
        vec![
            Value::String(f.into()),
            Value::Int64(nb),
            Value::String(c.into()),
        ]
    };
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH f, count(*) AS nb \
             MATCH (c2:Commit)-[:TOUCHES]->(f) \
             RETURN f.path, nb, c2.sha",
        ),
        vec![
            ligne("x", 3, "c1"),
            ligne("x", 3, "c2"),
            ligne("x", 3, "c3"),
            ligne("y", 2, "c1"),
            ligne("y", 2, "c2"),
            ligne("z", 1, "c1"),
        ],
        "{langage:?}"
    );
}

/// Un `MATCH` écrit après un `WITH` agrégé part des colonnes du `WITH` : il ne
/// multiplie pas son compte. 3 commits ; 3 fichiers x, y, z. `n` reste 3 sur
/// chacune des trois lignes (planifié avant le `WITH`, le `MATCH` donnait
/// `n` = 9).
fn match_apres_with_agrege_ne_multiplie_pas_le_compte(langage: Langage) {
    let db = base();
    let ligne = |chemin: &str| vec![Value::Int64(3), Value::String(chemin.into())];
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit) WITH count(*) AS n MATCH (f:File) RETURN n, f.path",
        ),
        vec![ligne("x"), ligne("y"), ligne("z")],
        "{langage:?}"
    );
}

/// Le `WHERE` écrit après ce `MATCH` le filtre : x est écarté.
fn where_apres_match_apres_with(langage: Langage) {
    let db = base();
    let ligne = |chemin: &str| vec![Value::Int64(3), Value::String(chemin.into())];
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit) WITH count(*) AS n \
             MATCH (f:File) WHERE f.path <> 'x' \
             RETURN n, f.path",
        ),
        vec![ligne("y"), ligne("z")],
        "{langage:?}"
    );
}

/// `WITH … ORDER BY … LIMIT` : le tri et la limite portent sur la sortie du
/// `WITH`. Nombre de commits par fichier : x 3, y 2, z 1. Le plus touché est x ;
/// le moins touché est z ; avec `SKIP 1`, le deuxième plus touché est y.
fn with_order_by_skip_limit(langage: Langage) {
    let db = base();
    let ligne = |chemin: &str, nb: i64| vec![Value::String(chemin.into()), Value::Int64(nb)];
    for (fin, attendu) in [
        ("ORDER BY nb DESC LIMIT 1", vec![ligne("x", 3)]),
        ("ORDER BY nb ASC LIMIT 1", vec![ligne("z", 1)]),
        ("ORDER BY nb DESC SKIP 1 LIMIT 1", vec![ligne("y", 2)]),
        ("ORDER BY nb DESC LIMIT 2 WHERE nb < 3", vec![ligne("y", 2)]),
    ] {
        let requete = format!(
            "MATCH (c:Commit)-[:TOUCHES]->(f:File) \
             WITH f, count(*) AS nb {fin} \
             RETURN f.path, nb"
        );
        assert_eq!(
            lignes(&db, langage, &requete),
            attendu,
            "{langage:?} {requete}"
        );
    }
}

// ============================================================================
// Chaque cas, une fois par langage
// ============================================================================

/// Déclare un module de tests : chaque cas listé y devient un `#[test]` joué
/// dans le langage donné.
macro_rules! cas_en {
    ($module:ident, $langage:expr, [$($cas:ident),+ $(,)?]) => {
        mod $module {
            $(#[test] fn $cas() { super::$cas($langage); })+
        }
    };
}

cas_en!(
    cypher,
    super::Langage::Cypher,
    [
        cible_liee_arete_sortante,
        cible_liee_arete_entrante,
        depart_et_cible_lies,
        trois_match_sur_la_meme_cible,
        depart_lie,
        depart_lie_garde_ses_labels,
        ancre_au_milieu_du_motif,
        cycle_dans_un_seul_motif,
        proprietes_en_ligne_de_la_cible,
        tous_les_labels_de_la_cible,
        proprietes_en_ligne_d_une_relation_nommee,
        proprietes_en_ligne_d_une_relation_anonyme,
        cible_liee_apres_with,
        cible_liee_apres_with_distinct,
        variable_sortie_de_portee_par_with_n_est_plus_liee,
        cible_liee_apres_with_agrege,
        match_apres_with_agrege_ne_multiplie_pas_le_compte,
        where_apres_match_apres_with,
        with_order_by_skip_limit,
    ]
);

cas_en!(
    gql,
    super::Langage::Gql,
    [
        cible_liee_arete_sortante,
        cible_liee_arete_entrante,
        depart_et_cible_lies,
        trois_match_sur_la_meme_cible,
        depart_lie,
        depart_lie_garde_ses_labels,
        ancre_au_milieu_du_motif,
        cycle_dans_un_seul_motif,
        proprietes_en_ligne_de_la_cible,
        tous_les_labels_de_la_cible,
        proprietes_en_ligne_d_une_relation_nommee,
        proprietes_en_ligne_d_une_relation_anonyme,
        cible_liee_apres_with,
        cible_liee_apres_with_distinct,
        variable_sortie_de_portee_par_with_n_est_plus_liee,
        cible_liee_apres_with_agrege,
        match_apres_with_agrege_ne_multiplie_pas_le_compte,
        where_apres_match_apres_with,
        with_order_by_skip_limit,
    ]
);
