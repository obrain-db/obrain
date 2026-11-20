//! Chemin de longueur variable vers une cible EXACTE (obrain-db/obrain#189).
//!
//! `MATCH (t:File {path: $e}) MATCH p = (n:Note)-[:LINKED_TO]->(s)-[:CALLS*0..2]->(t)`
//! énumérait tous les chemins de `k` sauts de CHAQUE source avant de ne garder
//! que ceux qui finissent sur `t` : mémoire sans borne, aucune ligne rendue
//! (`get_propagated_notes` de PO, 25 à 28 Go). L'expansion est désormais guidée
//! par la cible : seuls les chemins qui la joignent sont énumérés.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test chemin_cible_exacte_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_core::execution::operators::chemins_enumeres;
use obrain_engine::ObrainDB;

const NOTES: usize = 30;
const EVENTAIL: usize = 30;

/// `NOTES` notes liées chacune à une source ; chaque source appelle `EVENTAIL`
/// fonctions qui en appellent chacune `EVENTAIL` autres (impasses) : `*0..2`
/// depuis une source compte ~`EVENTAIL²` chemins, dont aucun n'atteint la
/// cible sauf ceux de la source 0 (`s0 -> mid -> target`) et la note liée
/// directement à la cible.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let target = s.create_node_with_props(&["File"], [("path", Value::String("t.rs".into()))]);
    let mid = s.create_node_with_props(&["Fn"], [("k", Value::Int64(0))]);
    s.create_edge(mid, target, "CALLS");
    let mut sources = Vec::new();
    for i in 0..NOTES {
        let src = s.create_node_with_props(&["Fn"], [("k", Value::Int64(i as i64))]);
        if i == 0 {
            s.create_edge(src, mid, "CALLS");
        }
        for _ in 0..EVENTAIL {
            let a = s.create_node_with_props(&["Fn"], [("k", Value::Int64(-1))]);
            s.create_edge(src, a, "CALLS");
            for _ in 0..EVENTAIL {
                let b = s.create_node_with_props(&["Fn"], [("k", Value::Int64(-2))]);
                s.create_edge(a, b, "CALLS");
            }
        }
        sources.push(src);
    }
    for src in sources {
        let n = s.create_node_with_props(&["Note"], [("status", Value::String("active".into()))]);
        s.create_edge(n, src, "LINKED_TO");
    }
    // Une note liée DIRECTEMENT à la cible : chemin de longueur 0.
    let n = s.create_node_with_props(&["Note"], [("status", Value::String("active".into()))]);
    s.create_edge(n, target, "LINKED_TO");
    db
}

fn compte(db: &ObrainDB, saut: &str) -> (i64, u64) {
    let q = format!(
        "MATCH (target:File {{path: 't.rs'}}) \
         MATCH path=(n:Note)-[:LINKED_TO]->(source)-[:CALLS{saut}]->(target) \
         WHERE n.status = 'active' RETURN count(path) AS c"
    );
    let avant = chemins_enumeres();
    let r = db
        .session()
        .execute_cypher(&q)
        .unwrap_or_else(|e| panic!("{q}\n{e}"));
    let enumeres = chemins_enumeres() - avant;
    match r.rows[0][0] {
        Value::Int64(c) => (c, enumeres),
        ref v => panic!("count inattendu : {v:?}"),
    }
}

#[test]
fn enumere_les_chemins_qui_joignent_la_cible_seulement() {
    let db = base();
    // `*0..2` : note0 -> s0 -> mid -> target (2 sauts) + note directe (0 saut).
    let (c, enumeres) = compte(&db, "*0..2");
    assert_eq!(c, 2, "résultat");
    // Sans guidage : ~NOTES * EVENTAIL² = 27 000 chemins énumérés.
    // (Le saut `LINKED_TO` du chemin nommé en énumère `NOTES + 1`, un par note.)
    assert!(
        enumeres <= 2 * NOTES as u64,
        "{enumeres} chemins énumérés pour 2 retenus : l'expansion ne part pas de la cible"
    );
}

#[test]
fn meme_resultat_pour_les_autres_bornes() {
    let db = base();
    assert_eq!(compte(&db, "*1..2").0, 1);
    assert_eq!(compte(&db, "*0..1").0, 1);
    assert_eq!(compte(&db, "*2").0, 1);
    assert_eq!(compte(&db, "*3..4").0, 0);
}
