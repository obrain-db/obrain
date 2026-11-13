//! Un `MATCH` qui repart d'un nœud lié ailleurs que sur le dernier nœud
//! parcouru, et les genres des variables liées (nœud, arête, valeur).
//!
//! - **Ancrage hors du dernier nœud** : le motif repart de `a` alors que le
//!   plan courant vient d'arriver en `b`. Le planificateur prenait deux
//!   `Expand` à un saut consécutifs pour une chaîne factorisée et faisait
//!   partir le second de `b` (la colonne source des sauts suivants est la
//!   cible du précédent, `from_variable` ignoré) : résultat faux, plan affiché
//!   juste. Les nœuds de ces tests n'ont PAS de label — un `Filter hasLabel`
//!   entre les deux `Expand` masquait le défaut.
//! - **Genres** : une variable liée à une valeur qui n'est pas un nœud (un
//!   entier, un texte, une liste, `id(n)`) ne peut pas l'être dans un motif :
//!   erreur de type, comme sous Neo4j, au lieu de joindre l'entier 1 au nœud
//!   n° 1. Un genre inconnu (élément d'`UNWIND` d'une liste de nœuds) passe.
//! - **NULL** : un `OPTIONAL MATCH` sans correspondance puis un `MATCH` qui
//!   reprend sa variable ne produit aucune ligne (et n'échoue pas).
//!
//! Valeurs attendues écrites à la main d'après les graphes de [`base`].
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test match_ancrage_et_genres
//! ```

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Trois commits, trois fichiers, une chaîne de quatre étapes.
///
/// | arête   | c1      | c2   | c3 |
/// |---------|---------|------|----|
/// | TOUCHES | x, y, z | x, y | x  |
/// | REVIEWS | x       | z    |    |
///
/// Fichiers : `NEXT` x -> y -> z. Étapes (`Step`) : `NEXT` A -> B -> C -> D.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let fichier = |chemin: &str| {
        session.create_node_with_props(&["File"], [("path", Value::String(chemin.into()))])
    };
    let (x, y, z) = (fichier("x"), fichier("y"), fichier("z"));
    session.create_edge(x, y, "NEXT");
    session.create_edge(y, z, "NEXT");
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
    let etapes: Vec<_> = ["A", "B", "C", "D"]
        .iter()
        .map(|n| session.create_node_with_props(&["Step"], [("name", Value::String((*n).into()))]))
        .collect();
    for paire in etapes.windows(2) {
        session.create_edge(paire[0], paire[1], "NEXT");
    }
    db
}

#[derive(Clone, Copy, Debug)]
enum Langage {
    Cypher,
    Gql,
}

fn executer(db: &ObrainDB, langage: Langage, requete: &str) -> Result<Vec<Vec<Value>>, String> {
    let session = db.session();
    let resultat = match langage {
        Langage::Cypher => session.execute_cypher(requete),
        Langage::Gql => session.execute(requete),
    };
    resultat.map(|r| r.rows).map_err(|e| e.to_string())
}

/// Lignes triées (l'ordre n'est pas garanti sans `ORDER BY`).
fn lignes(db: &ObrainDB, langage: Langage, requete: &str) -> Vec<Vec<Value>> {
    let mut l = executer(db, langage, requete)
        .unwrap_or_else(|e| panic!("[{langage:?}] {requete}\n  -> {e}"));
    l.sort_by_key(|ligne| format!("{ligne:?}"));
    l
}

fn l(valeurs: &[&str]) -> Vec<Value> {
    valeurs.iter().map(|v| Value::String((*v).into())).collect()
}

fn entier(n: i64) -> Vec<Vec<Value>> {
    vec![vec![Value::Int64(n)]]
}

/// Le plan logique (`EXPLAIN`, Cypher).
fn plan(db: &ObrainDB, requete: &str) -> String {
    db.session()
        .execute_cypher(&format!("EXPLAIN {requete}"))
        .unwrap()
        .rows[0][0]
        .to_string()
}

// ============================================================================
// Ancrage ailleurs que sur le dernier nœud — nœuds SANS label
// ============================================================================

/// `a` est lié, `b` est le dernier nœud parcouru. Le second motif repart de
/// `a` (le plan arrive en `b`) : x est le prédécesseur de `a`. Paires (a, b) :
/// (A,B), (B,C), (C,D) ; prédécesseur de `a` : B <- A, C <- B (A n'en a pas).
/// Attendu : (x, a, b) = (A, B, C) et (B, C, D).
fn ancre_sur_un_noeud_qui_n_est_pas_le_dernier(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Step)-[:NEXT]->(b) MATCH (x)-[:NEXT]->(a) \
             RETURN x.name, a.name, b.name",
        ),
        vec![l(&["A", "B", "C"]), l(&["B", "C", "D"])],
        "{langage:?}"
    );
}

/// Trois sauts depuis `b` lié, sans label : A -> B -> C -> D.
fn ancre_au_milieu_de_trois_sauts_sans_label(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (b:Step {name: 'B'}) \
             MATCH (a)-[:NEXT]->(b)-[:NEXT]->(c)-[:NEXT]->(d) \
             RETURN a.name, b.name, c.name, d.name",
        ),
        vec![l(&["A", "B", "C", "D"])],
        "{langage:?}"
    );
}

/// Deux sauts autour de `b` lié, sans label : A -> B -> C.
fn ancre_au_milieu_de_deux_sauts_sans_label(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (b:Step {name: 'B'}) \
             MATCH (a)-[:NEXT]->(b)-[:NEXT]->(c) \
             RETURN a.name, b.name, c.name",
        ),
        vec![l(&["A", "B", "C"])],
        "{langage:?}"
    );
}

/// Chaque couple (commit, fichier) touché, puis le voisin `NEXT` du fichier
/// (sans orientation), puis les commits qui touchent le MÊME fichier.
/// Voisins : x : {y} ; y : {x, z} ; z : {y}. Lignes du premier motif : x 3 x 1,
/// y 2 x 2, z 1 x 1 = 8 ; fois les commits du fichier : x 3 x 3 = 9,
/// y 4 x 2 = 8, z 1 x 1 = 1. Total 18.
fn ancre_apres_un_chemin_de_deux_sauts(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (c:Commit)-[:TOUCHES]->(f)-[:NEXT]-(s) \
             MATCH (c2)-[:TOUCHES]->(f) \
             RETURN count(*)",
        ),
        entier(18),
        "{langage:?}"
    );
}

/// L'ancre est la CIBLE du premier saut d'un motif à deux sauts qui finit sur
/// un nœud lié : REVIEWS entre dans z par c2 ; NEXT entre dans z par y.
fn ancre_au_milieu_avec_arete_entrante(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (f:File {path: 'z'}) \
             MATCH (a)-[:REVIEWS]->(f)<-[:NEXT]-(s) \
             RETURN a.sha, s.path",
        ),
        vec![l(&["c2", "y"])],
        "{langage:?}"
    );
}

// ============================================================================
// Genres des variables liées
// ============================================================================

/// Une valeur qui n'est pas un nœud ne se joint pas à un nœud : erreur de
/// type, jamais un résultat (l'entier 1 ne désigne pas le nœud n° 1).
fn variable_liee_a_une_valeur_est_une_erreur_de_type(langage: Langage) {
    let db = base();
    for requete in [
        "UNWIND [0] AS z WITH 1 AS f MATCH (c:Commit)-[:TOUCHES]->(f) RETURN count(*)",
        "UNWIND [1] AS f MATCH (c:Commit)-[:TOUCHES]->(f) RETURN count(*)",
        "MATCH (n:File) WITH id(n) AS f MATCH (c:Commit)-[:TOUCHES]->(f) RETURN count(*)",
        "UNWIND [0] AS z WITH 1 AS f MATCH (f)<-[:TOUCHES]-(c:Commit) RETURN count(*)",
        "MATCH (c:Commit) WITH c.sha AS f MATCH (x)-[:TOUCHES]->(f) RETURN count(*)",
        "MATCH (c:Commit) WITH collect(c) AS fs MATCH (x)-[:TOUCHES]->(fs) RETURN count(*)",
        "UNWIND [0] AS z WITH 1 AS r MATCH ()-[r:TOUCHES]->() RETURN count(*)",
    ] {
        let erreur = executer(&db, langage, requete)
            .expect_err(&format!("[{langage:?}] {requete} devait échouer"));
        assert!(
            erreur.contains("Type mismatch"),
            "[{langage:?}] {requete}\n  erreur sans nom de type : {erreur}"
        );
    }
}

/// Un genre INCONNU passe : l'élément d'un `UNWIND` d'une liste de nœuds est
/// un nœud, en cible comme au départ — 6 arêtes.
fn variable_d_unwind_de_noeuds_reste_un_noeud(langage: Langage) {
    let db = base();
    for requete in [
        "MATCH (f:File) WITH collect(f) AS fs UNWIND fs AS f \
         MATCH (c:Commit)-[:TOUCHES]->(f) RETURN count(*)",
        "MATCH (f:File) WITH collect(f) AS fs UNWIND fs AS f \
         MATCH (f)<-[:TOUCHES]-(c:Commit) RETURN count(*)",
    ] {
        assert_eq!(
            lignes(&db, langage, requete),
            entier(6),
            "{langage:?} {requete}"
        );
    }
}

/// `OPTIONAL MATCH` sans correspondance : la variable est NULL ; le `MATCH`
/// qui la reprend — en cible ou au départ — ne produit aucune ligne.
fn variable_nulle_ne_produit_aucune_ligne(langage: Langage) {
    let db = base();
    for requete in [
        "MATCH (a:Step {name: 'A'}) OPTIONAL MATCH (a)-[:TOUCHES]->(f:File) \
         MATCH (c:Commit)-[:TOUCHES]->(f) RETURN count(*)",
        "MATCH (a:Step {name: 'A'}) OPTIONAL MATCH (a)-[:TOUCHES]->(f:File) \
         MATCH (f)<-[:TOUCHES]-(c:Commit) RETURN count(*)",
    ] {
        assert_eq!(
            lignes(&db, langage, requete),
            entier(0),
            "{langage:?} {requete}"
        );
    }
}

// ============================================================================
// `CALL { WITH f MATCH (f)… }` : l'import garde l'expansion
// ============================================================================

/// Pour chaque fichier, le nombre de commits qui le touchent : x 3, y 2, z 1.
fn call_avec_import_repart_de_la_variable(langage: Langage) {
    let db = base();
    let ligne = |chemin: &str, n: i64| vec![Value::String(chemin.into()), Value::Int64(n)];
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (f:File) \
             CALL { WITH f MATCH (f)<-[:TOUCHES]-(c:Commit) RETURN count(*) AS n } \
             RETURN f.path, n",
        ),
        vec![ligne("x", 3), ligne("y", 2), ligne("z", 1)],
        "{langage:?}"
    );
}

/// Le plan : une expansion depuis `f` importé, aucune jointure avec un
/// balayage complet (sinon O(N + E) par ligne extérieure).
#[test]
fn le_call_avec_import_n_est_pas_une_jointure() {
    let db = base();
    let p = plan(
        &db,
        "MATCH (f:File) \
         CALL { WITH f MATCH (f)<-[:TOUCHES]-(c:Commit) RETURN count(*) AS n } \
         RETURN f.path, n",
    );
    assert!(!p.contains("Join"), "{p}");
}

// ============================================================================
// Variable d'arête déjà liée : discriminant
// ============================================================================

/// Les arêtes TOUCHES qui entrent dans y : c1 -> y et c2 -> y. `r` lié, le
/// motif suivant doit retrouver CES arêtes : (c1, c1) et (c2, c2) — pas le
/// produit (4 lignes) d'une arête neuve.
fn arete_liee_retrouve_sa_propre_arete(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (a:Commit)-[r:TOUCHES]->(f:File {path: 'y'}) \
             WITH a, r, f \
             MATCH (b)-[r]->(f) \
             RETURN a.sha, b.sha",
        ),
        vec![l(&["c1", "c1"]), l(&["c2", "c2"])],
        "{langage:?}"
    );
}

// ============================================================================
// Ce que laissent visible les clauses d'écriture
// ============================================================================

/// `MERGE` lie un nœud : le `MATCH` suivant en repart (3 commits touchent x).
fn merge_puis_match_sur_le_noeud(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MERGE (f:File {path: 'x'}) WITH f \
             MATCH (c:Commit)-[:TOUCHES]->(f) RETURN count(*)",
        ),
        entier(3),
        "{langage:?}"
    );
}

/// Les clauses d'écriture suivent un `WITH` : `CREATE`, puis `SET`, puis
/// `DELETE`, chacune à sa position.
fn clauses_d_ecriture_apres_un_with(langage: Langage) {
    let db = base();
    executer(
        &db,
        langage,
        "MATCH (f:File {path: 'z'}) WITH f SET f.vu = 1 RETURN count(*)",
    )
    .unwrap_or_else(|e| panic!("[{langage:?}] SET : {e}"));
    assert_eq!(
        lignes(&db, langage, "MATCH (f:File) WHERE f.vu = 1 RETURN f.path"),
        vec![l(&["z"])],
        "{langage:?} SET"
    );
    executer(
        &db,
        langage,
        "MATCH (f:File {path: 'z'}) WITH f CREATE (t:Tag {k: 'nouveau'}) RETURN count(*)",
    )
    .unwrap_or_else(|e| panic!("[{langage:?}] CREATE : {e}"));
    assert_eq!(
        lignes(&db, langage, "MATCH (t:Tag) RETURN count(*)"),
        entier(1),
        "{langage:?} CREATE"
    );
    executer(
        &db,
        langage,
        "MATCH (t:Tag) WITH t DETACH DELETE t RETURN count(*)",
    )
    .unwrap_or_else(|e| panic!("[{langage:?}] DELETE : {e}"));
    assert_eq!(
        lignes(&db, langage, "MATCH (t:Tag) RETURN count(*)"),
        entier(0),
        "{langage:?} DELETE"
    );
}

/// Le nom `x` désigne une ARÊTE avant le `WITH`, un NŒUD après : le `SET` qui
/// suit écrit sur le nœud. (Cypher décidait par le nom, globalement : il
/// écrivait sur l'arête n° 1.)
fn nom_d_arete_reemploye_comme_noeud_apres_un_with(langage: Langage) {
    let db = base();
    executer(
        &db,
        langage,
        "MATCH ()-[x:TOUCHES]->() WITH count(x) AS c \
         MATCH (x:File {path: 'z'}) SET x.p = 5 RETURN c",
    )
    .unwrap_or_else(|e| panic!("[{langage:?}] {e}"));
    assert_eq!(
        lignes(&db, langage, "MATCH (f:File) WHERE f.p = 5 RETURN f.path"),
        vec![l(&["z"])],
        "{langage:?} le nœud porte p"
    );
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH ()-[r:TOUCHES]->() WHERE r.p = 5 RETURN count(*)"
        ),
        entier(0),
        "{langage:?} aucune arête ne porte p"
    );
}

/// Le `WHERE` écrit après un `OPTIONAL MATCH` qui suit un `WITH` appartient à
/// cet `OPTIONAL MATCH` : les fichiers sans relecteur c1 restent, avec NULL.
/// REVIEWS : c1 -> x, c2 -> z. Avec `c.sha = 'c1'` : x garde c1 ; y et z, NULL.
fn where_d_un_optional_match_apres_un_with(langage: Langage) {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            langage,
            "MATCH (f:File) WITH f \
             OPTIONAL MATCH (c:Commit)-[:REVIEWS]->(f) WHERE c.sha = 'c1' \
             RETURN f.path, c.sha",
        ),
        vec![
            l(&["x", "c1"]),
            vec![Value::String("y".into()), Value::Null],
            vec![Value::String("z".into()), Value::Null],
        ],
        "{langage:?}"
    );
}

/// Les propriétés en ligne d'une relation de longueur variable : refusées
/// franchement (elles étaient ignorées : des chemins qui ne les respectent pas
/// étaient rendus).
#[test]
fn proprietes_en_ligne_d_une_relation_de_longueur_variable_refusees() {
    let db = base();
    let erreur = executer(
        &db,
        Langage::Cypher,
        "MATCH (a)-[:NEXT*1..2 {w: 1}]->(b) RETURN count(*)",
    )
    .expect_err("devait échouer");
    assert!(erreur.contains("longueur variable"), "{erreur}");
}

macro_rules! cas_en {
    ($module:ident, $langage:expr, [$($cas:ident),+ $(,)?]) => {
        mod $module {
            $(#[test] fn $cas() { super::$cas($langage); })+
        }
    };
}

macro_rules! les_deux_langages {
    ($($cas:ident),+ $(,)?) => {
        cas_en!(cypher, super::Langage::Cypher, [$($cas),+]);
        cas_en!(gql, super::Langage::Gql, [$($cas),+]);
    };
}

les_deux_langages!(
    ancre_sur_un_noeud_qui_n_est_pas_le_dernier,
    ancre_au_milieu_de_trois_sauts_sans_label,
    ancre_au_milieu_de_deux_sauts_sans_label,
    ancre_apres_un_chemin_de_deux_sauts,
    ancre_au_milieu_avec_arete_entrante,
    variable_liee_a_une_valeur_est_une_erreur_de_type,
    variable_d_unwind_de_noeuds_reste_un_noeud,
    variable_nulle_ne_produit_aucune_ligne,
    call_avec_import_repart_de_la_variable,
    arete_liee_retrouve_sa_propre_arete,
    merge_puis_match_sur_le_noeud,
    clauses_d_ecriture_apres_un_with,
    nom_d_arete_reemploye_comme_noeud_apres_un_with,
    where_d_un_optional_match_apres_un_with,
);
