//! L'unicité d'un tuple tient aussi sous `SET` et `MERGE … ON MATCH/ON CREATE SET`.
//!
//! `main` refusait un `SET` d'une colonne d'un `UNIQUE (a, b)` (trop : il
//! testait chaque colonne seule). Le correctif du tuple ne devait pas le laisser
//! passer en silence : le validateur recevait seulement les propriétés du `SET`,
//! jugeait le tuple INCOMPLET, et ne contrôlait rien. Ces tests fixent le
//! contrôle sur l'état COMPLET du nœud (les colonnes non écrites sont lues sur
//! le nœud, le nœud lui-même est exclu de la recherche).

#![cfg(feature = "cypher")]

use obrain_engine::{ObrainDB, Session};

fn base(contraintes: &[&str]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    for c in contraintes {
        session.execute_cypher(c).expect("CREATE CONSTRAINT");
    }
    db
}

const U_A: &str = "CREATE CONSTRAINT u_a FOR (n:T) REQUIRE n.a IS UNIQUE";
const U_AB: &str = "CREATE CONSTRAINT u_ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE";

/// Nombre de nœuds `:T` qui répondent au motif `{…}` donné.
fn nombre(session: &Session, motif: &str) -> usize {
    session
        .execute(&format!("MATCH (n:T {motif}) RETURN n.a"))
        .expect("MATCH de comptage")
        .rows
        .len()
}

fn refuse(session: &Session, requete: &str, pourquoi: &str) {
    let r = session.execute_cypher(requete);
    assert!(r.is_err(), "{pourquoi} — `{requete}` a répondu {r:?}");
    let texte = r.unwrap_err().to_string();
    assert!(
        texte.contains("UNIQUE"),
        "{pourquoi} — l'erreur doit nommer la contrainte d'unicité, reçu : {texte}"
    );
}

fn accepte(session: &Session, requete: &str, pourquoi: &str) {
    let r = session.execute_cypher(requete);
    assert!(
        r.is_ok(),
        "{pourquoi} — `{requete}` a échoué : {:?}",
        r.err()
    );
}

// ───────────────────────── 1. SET d'une propriété ─────────────────────────

#[test]
fn set_ne_cree_pas_de_doublon_mono_colonne() {
    let db = base(&[U_A]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, m: 'premier'})").unwrap();
    s.execute("INSERT (:T {a: 2, m: 'second'})").unwrap();

    refuse(
        &s,
        "MATCH (n:T {a: 2}) SET n.a = 1",
        "SET a = 1 alors que a = 1 existe",
    );
    assert_eq!(nombre(&s, "{a: 1}"), 1, "aucun doublon écrit");
    assert_eq!(nombre(&s, "{a: 2}"), 1, "le nœud visé est resté tel quel");

    accepte(&s, "MATCH (n:T {a: 2}) SET n.a = 3", "valeur libre");
    assert_eq!(nombre(&s, "{a: 3}"), 1);
}

/// Un `SET` d'UNE colonne du tuple est contrôlé contre l'état COMPLET du
/// nœud : les autres colonnes sont lues sur le nœud, pas dans l'instruction.
#[test]
fn set_d_une_colonne_controle_le_tuple_complet() {
    let db = base(&[U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 2, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 2, b: 'y'})").unwrap();

    refuse(
        &s,
        "MATCH (n:T {a: 2, b: 'x'}) SET n.a = 1",
        "(2,'x') → (1,'x') : le tuple existe déjà",
    );
    refuse(
        &s,
        "MATCH (n:T {a: 2, b: 'y'}) SET n.b = 'x'",
        "(2,'y') → (2,'x') : le tuple existe déjà",
    );
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 1);
    assert_eq!(nombre(&s, "{a: 2, b: 'x'}"), 1);

    // Partager UNE colonne n'est pas un doublon.
    accepte(
        &s,
        "MATCH (n:T {a: 2, b: 'y'}) SET n.a = 1",
        "(2,'y') → (1,'y') : tuple neuf",
    );
    assert_eq!(nombre(&s, "{a: 1, b: 'y'}"), 1);
}

/// Réécrire son propre tuple à l'identique n'est pas un doublon de soi-même.
#[test]
fn set_a_l_identique_ne_se_heurte_pas_a_lui_meme() {
    let db = base(&[U_A, U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();

    accepte(&s, "MATCH (n:T {a: 1}) SET n.a = 1", "même valeur");
    accepte(
        &s,
        "MATCH (n:T {a: 1}) SET n.a = 1, n.b = 'x'",
        "même tuple",
    );
    accepte(&s, "MATCH (n:T {a: 1}) SET n.autre = 5", "colonne libre");
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 1);
}

/// Retirer un membre du tuple (SET … = null) ne peut pas créer de doublon.
#[test]
fn set_a_null_sort_le_noeud_de_la_contrainte() {
    let db = base(&[U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 1, b: 'y'})").unwrap();
    accepte(&s, "MATCH (n:T {b: 'y'}) SET n.b = null", "b retiré");
    // …et le remettre à une valeur prise est refusé.
    refuse(
        &s,
        "MATCH (n:T) WHERE n.b IS NULL SET n.b = 'x'",
        "(1, absent) → (1,'x') : le tuple existe déjà",
    );
}

/// Un `SET` sur plusieurs nœuds qui les ferait converger est refusé.
#[test]
fn set_sur_plusieurs_noeuds_ne_les_fait_pas_converger() {
    let db = base(&[U_A]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1})").unwrap();
    s.execute("INSERT (:T {a: 2})").unwrap();
    refuse(
        &s,
        "MATCH (n:T) SET n.a = 9",
        "deux nœuds recevraient a = 9",
    );
}

// ─────────────── 2. cartes, MERGE … ON MATCH SET, ajout de label ───────────────

#[test]
fn set_par_carte_est_controle() {
    let db = base(&[U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 2, b: 'y', c: 0})").unwrap();

    refuse(
        &s,
        "MATCH (n:T {a: 2}) SET n = {a: 1, b: 'x'}",
        "SET n = {…} écrit un tuple existant",
    );
    refuse(
        &s,
        "MATCH (n:T {a: 2}) SET n += {a: 1, b: 'x'}",
        "SET n += {…} écrit un tuple existant",
    );
    // `+=` d'une seule colonne : l'autre est lue sur le nœud.
    s.execute("INSERT (:T {a: 1, b: 'y'})").unwrap();
    refuse(
        &s,
        "MATCH (n:T {a: 2}) SET n += {a: 1}",
        "(2,'y') += {a: 1} → (1,'y') existe",
    );
    assert_eq!(nombre(&s, "{a: 2, b: 'y'}"), 1, "nœud visé intact");

    // `=` REMPLACE : une carte sans `b` sort le nœud de la contrainte.
    accepte(
        &s,
        "MATCH (n:T {a: 2}) SET n = {a: 1}",
        "remplacement sans b : tuple incomplet",
    );
    // Une carte à l'identique ne se heurte pas à elle-même.
    accepte(
        &s,
        "MATCH (n:T {a: 1, b: 'x'}) SET n += {a: 1, b: 'x'}",
        "même tuple",
    );
}

#[test]
fn merge_on_match_set_est_controle() {
    let db = base(&[U_A]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1})").unwrap();
    s.execute("INSERT (:T {a: 2})").unwrap();

    refuse(
        &s,
        "MERGE (n:T {a: 2}) ON MATCH SET n.a = 1",
        "ON MATCH SET a = 1 alors que a = 1 existe",
    );
    assert_eq!(nombre(&s, "{a: 1}"), 1);
    assert_eq!(nombre(&s, "{a: 2}"), 1);

    accepte(
        &s,
        "MERGE (n:T {a: 2}) ON MATCH SET n.a = 2, n.vu = true",
        "réécriture à l'identique",
    );
    accepte(
        &s,
        "MERGE (n:T {a: 2}) ON MATCH SET n.a = 5",
        "valeur libre",
    );
    assert_eq!(nombre(&s, "{a: 5}"), 1);
}

/// Le scénario de la revue : `UNIQUE (a, b)`, nœuds (1,'x') et (2,'y').
/// `SET n.a = 1` sur le second passe (tuple (1,'y') neuf) ; ensuite
/// `SET n.b = 'x'` ferait (1,'x') en double et doit être refusé — comme
/// `MERGE … ON MATCH SET`, qui passe par le même opérateur.
#[test]
fn set_colonne_par_colonne_ne_fabrique_pas_de_doublon_de_tuple() {
    let db = base(&[U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:T {a: 2, b: 'y'})").unwrap();

    accepte(
        &s,
        "MATCH (n:T {a: 2}) SET n.a = 1",
        "(2,'y') → (1,'y') : tuple neuf",
    );
    refuse(
        &s,
        "MATCH (n:T {b: 'y'}) SET n.b = 'x'",
        "(1,'y') → (1,'x') : le tuple existe déjà",
    );
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 1);
    assert_eq!(nombre(&s, "{a: 1, b: 'y'}"), 1);

    // L'autre colonne d'abord : (1,'y') → (3,'y') passe, puis (3,'y') → (1,'y')
    // n'a plus rien à heurter ; mais (3,'y') vers a = 1 AVEC b = 'x' non.
    accepte(&s, "MATCH (n:T {b: 'y'}) SET n.a = 3", "(1,'y') → (3,'y')");
    s.execute("INSERT (:T {a: 3, b: 'z'})").unwrap();
    refuse(
        &s,
        "MATCH (n:T {a: 3, b: 'z'}) SET n.b = 'y'",
        "(3,'z') → (3,'y') : le tuple existe déjà",
    );

    // MERGE … ON MATCH SET d'UNE colonne.
    refuse(
        &s,
        "MERGE (n:T {a: 3, b: 'y'}) ON MATCH SET n.a = 1, n.b = 'x'",
        "ON MATCH SET réécrit le tuple en (1,'x')",
    );
    refuse(
        &s,
        "MERGE (n:T {a: 3, b: 'z'}) ON MATCH SET n.a = 1, n.b = 'x'",
        "ON MATCH SET réécrit le tuple en (1,'x')",
    );
    let db2 = base(&[U_A]);
    let s2 = db2.session();
    s2.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s2.execute("INSERT (:T {a: 2, b: 'x'})").unwrap();
    refuse(
        &s2,
        "MERGE (n:T {a: 2}) ON MATCH SET n.a = 1",
        "ON MATCH SET d'une colonne unique vers une valeur prise",
    );
    refuse(
        &s,
        "MERGE (n:T {a: 3, b: 'z'}) ON MATCH SET n.b = 'y'",
        "ON MATCH SET d'UNE colonne du tuple : (3,'y') existe",
    );
}

/// Le tuple composite est réparti entre le motif du `MERGE` et `ON CREATE SET`
/// : `MERGE (n:T {a: 1}) ON CREATE SET n.b = 'x'` écrit (1,'x'). Le contrôle
/// doit voir le tuple COMPLET (motif + `ON CREATE`), pas la seule clé du motif.
#[test]
fn merge_on_create_set_repartit_le_tuple_entre_motif_et_on_create() {
    let db = base(&[U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();

    // Le motif {a: 1} trouve le nœud existant : c'est ON MATCH qui s'applique,
    // aucune création. Un motif qui ne trouve rien, lui, crée (2,'x') puis (1,…).
    accepte(
        &s,
        "MERGE (n:T {a: 2}) ON CREATE SET n.b = 'x'",
        "(2,'x') est un tuple neuf",
    );
    assert_eq!(nombre(&s, "{a: 2, b: 'x'}"), 1);
    // `a: 1` existe déjà avec b = 'x' : MERGE trouve, ne crée pas, ne viole rien.
    accepte(
        &s,
        "MERGE (n:T {a: 1}) ON CREATE SET n.b = 'x'",
        "le motif trouve le nœud existant",
    );
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 1);

    // Un nœud (1, absent) ne matche pas {a: 1, b: 'q'} : MERGE crée, le tuple
    // (1,'q') est neuf ; puis ON CREATE SET qui réécrit b vers une valeur prise
    // par le même `a` est refusé.
    s.execute("INSERT (:T {a: 5, b: 'p'})").unwrap();
    refuse(
        &s,
        "MERGE (n:T {a: 5, b: 'q'}) ON CREATE SET n.b = 'p'",
        "le motif demande (5,'q'), ON CREATE écrit (5,'p') qui existe",
    );
    assert_eq!(nombre(&s, "{a: 5, b: 'p'}"), 1);
}
