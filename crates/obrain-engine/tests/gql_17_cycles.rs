//! GQL — motifs de CYCLE (fermeture d'une chaîne sur une variable déjà liée).
//!
//! Un cycle `(a)-[:T]->(b)-[:T]->(c)-[:T]->(a)` lie `a` deux fois : le
//! 3ᵉ saut doit se fermer sur le `a` initial (contrainte d'égalité), pas
//! créer une colonne fantôme. Ce fichier documente l'état ACTUEL (défaut
//! connu, pré-existant — vérifié avec le binaire d'origine le 2026-09-01,
//! pas une régression des chantiers en cours) et la propriété exigée.
//!
//!   `cargo test -p obrain-engine --test gql_17_cycles`
//!       la suite par défaut : verte (les défauts ne sont pas compilés).
//!   `cargo test -p obrain-engine --test gql_17_cycles --features defauts-connus`
//!       le défaut tourne et ÉCHoue — légitimement. C'est le test à faire
//!       passer, pas à supprimer.

#![cfg(feature = "gql")]

use obrain_engine::ObrainDB;

/// Fixture : 3 composants en cycle `a -> b -> c -> a` (vocabulaire causal
/// aletheia — le graphe de causalité du L1, `spec_graph`).
fn cycle_a_b_c_a() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:C {qn: 'a'})").unwrap();
    s.execute("INSERT (:C {qn: 'b'})").unwrap();
    s.execute("INSERT (:C {qn: 'c'})").unwrap();
    s.execute("MATCH (a:C {qn:'a'}), (b:C {qn:'b'}) MERGE (a)-[:depends_on]->(b)")
        .unwrap();
    s.execute("MATCH (b:C {qn:'b'}), (c:C {qn:'c'}) MERGE (b)-[:depends_on]->(c)")
        .unwrap();
    s.execute("MATCH (c:C {qn:'c'}), (a:C {qn:'a'}) MERGE (c)-[:depends_on]->(a)")
        .unwrap();
    db
}

/// PROPRIÉTÉ EXIGÉE : fermer un cycle sur une variable déjà liée et
/// projeter CE nœud doit renvoyer le nœud du cycle (1 ligne), pas une
/// erreur d'exécution « Column not found ».
#[cfg(feature = "defauts-connus")]
#[test]
fn fermer_un_cycle_sur_une_variable_deja_liee_projette_le_noeud() {
    // DÉFAUT: obrain-engine/src/query/planner/lpg/expand.rs:196
    // (`plan_expand_chain`) et :108 (`plan_expand`) — `columns.push(
    // expand.to_variable.clone())` pousse une colonne DUPLIQUÉE quand le
    // 3ᵉ saut se ferme sur `a` déjà lié, sans contrainte d'égalité. Le
    // RETURN de `a.qualified_name` résout `a` vers l'index 6, absent de
    // l'output de l'opérateur chaîné → « OBRAIN-V001: Column not found:
    // Column 6 ». `RETURN b.qualified_name` et le non-cycle `(a)->(b)->(c)
    // ->(d)` fonctionnent — seul le saut de FERMETURE casse.
    //
    // Vérifié pré-existant : même échec avec le binaire d'origine
    // (`obrain.pre-degrees-fix-*`, 2026-09-01) sur le store causal —
    // pas une régression des chantiers CH8b/CH9.
    let db = cycle_a_b_c_a();
    let s = db.session();
    let r = s
        .execute(
            "MATCH (a)-[:depends_on]->(b)-[:depends_on]->(c)-[:depends_on]->(a) \
             RETURN a.qn",
        )
        .unwrap();
    assert_eq!(
        r.rows.len(),
        1,
        "le cycle a->b->c->a doit fermer sur a (1 ligne), pas échouer"
    );
    assert_eq!(r.rows[0][0].to_string(), "a");
}

/// Contre-épreuve (verte aujourd'hui) : `RETURN count(*)` sur le même motif
/// fermé ne projette pas la variable dupliquée → ne doit PAS rougir. Sur un
/// cycle réel a→b→c→a, la sémantique GQL compte les 3 rotations du cycle
/// (a,b,c), (b,c,a), (c,a,b) — 3 lignes attendues.
#[test]
fn fermer_un_cycle_compte_les_lignes_sans_projeter_la_variable() {
    let db = cycle_a_b_c_a();
    let s = db.session();
    let r = s
        .execute(
            "MATCH (a)-[:depends_on]->(b)-[:depends_on]->(c)-[:depends_on]->(a) \
             RETURN count(*)",
        )
        .unwrap();
    assert_eq!(r.rows[0][0].to_string(), "3");
}

/// PROPRIÉTÉ EXIGÉE : un graphe SANS cycle ne doit renvoyer AUCUNE ligne
/// pour un motif de cycle. Le défaut renvoie des FAUX POSITIFS (le saut de
/// fermeture n'impose pas l'égalité a == a2 — le 3ᵉ saut re-parcourt la
/// 2ᵉ arête via la colonne source erronée).
#[cfg(feature = "defauts-connus")]
#[test]
fn un_graphe_sans_cycle_ne_renvoie_rien_pour_un_motif_de_cycle() {
    // DÉFAUT: obrain-engine/src/query/planner/lpg/expand.rs:196
    // (`plan_expand_chain`) — la colonne source des sauts 3+ est index 1
    // (cible du 1er saut) au lieu de la cible du saut précédent ; le saut de
    // fermeture ne contraint pas a2 == a. Résultat vérifié sur le store
    // causal (aucun 3-cycle : consensus→l1, economy→l1, harness→consensus,
    // harness→economy — l1 est un puits) :
    //   - `...->(a) RETURN count(*)` → 1 (attendu 0) — FAUX POSITIF
    //   - `RETURN a.qualified_name` → erreur « Column 6 » ; `RETURN b.…` → consensus
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for qn in ["l1", "consensus", "economy", "harness"] {
        s.execute(&format!("INSERT (:C {{qn: '{qn}'}})")).unwrap();
    }
    for (from, to) in [
        ("consensus", "l1"),
        ("economy", "l1"),
        ("harness", "consensus"),
        ("harness", "economy"),
    ] {
        s.execute(&format!(
            "MATCH (a:C {{qn:'{from}'}}), (b:C {{qn:'{to}'}}) MERGE (a)-[:depends_on]->(b)"
        ))
        .unwrap();
    }
    let r = s
        .execute(
            "MATCH (a)-[:depends_on]->(b)-[:depends_on]->(c)-[:depends_on]->(a) \
             RETURN count(*)",
        )
        .unwrap();
    assert_eq!(
        r.rows[0][0].to_string(),
        "0",
        "aucun 3-cycle dans ce graphe — le motif ne doit rien matcher"
    );
}
