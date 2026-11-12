//! GQL — motifs de CYCLE (fermeture d'une chaîne sur une variable déjà liée).
//!
//! Un cycle `(a)-[:T]->(b)-[:T]->(c)-[:T]->(a)` lie `a` deux fois : le
//! 3ᵉ saut doit se fermer sur le `a` initial (contrainte d'égalité), pas
//! créer une colonne fantôme. Le traducteur pose cette égalité
//! (`gql/pattern.rs`, variable `closes_on`) : les deux tests qui documentaient
//! le défaut sous `--cfg defauts_connus` tournent maintenant dans la suite.
//!
//!   `cargo test -p obrain-engine --test gql_17_cycles`

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
/// projeter CE nœud doit renvoyer le nœud du cycle, pas une erreur
/// d'exécution « Column not found ».
///
/// Le motif n'a pas de label : `a` peut être chacun des trois nœuds du cycle,
/// et chaque départ a exactement un tour — trois lignes, a, b, c (les trois
/// rotations que compte le test suivant). L'attente d'origine (« 1 ligne »)
/// n'avait jamais tourné et contredisait ce compte.
#[test]
fn fermer_un_cycle_sur_une_variable_deja_liee_projette_le_noeud() {
    // Avant la fermeture : l'`Expand` du 3ᵉ saut poussait une colonne `a`
    // dupliquée, sans égalité -> « OBRAIN-V001: Column not found: Column 6 ».
    let db = cycle_a_b_c_a();
    let s = db.session();
    let r = s
        .execute(
            "MATCH (a)-[:depends_on]->(b)-[:depends_on]->(c)-[:depends_on]->(a) \
             RETURN a.qn",
        )
        .unwrap();
    let mut departs: Vec<String> = r.rows.iter().map(|l| l[0].to_string()).collect();
    departs.sort();
    assert_eq!(
        departs,
        ["a", "b", "c"],
        "le cycle a->b->c->a se ferme sur son départ, une fois par rotation"
    );
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
#[test]
fn un_graphe_sans_cycle_ne_renvoie_rien_pour_un_motif_de_cycle() {
    // Avant la fermeture : le saut de fermeture ne contraignait pas a2 == a,
    // et ce motif rendait 1 ligne (faux positif) sur ce graphe sans 3-cycle.
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
