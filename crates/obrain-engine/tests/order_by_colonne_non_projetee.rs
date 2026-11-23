//! `ORDER BY` sur une propriété d'une variable DÉJÀ projetée par `RETURN` :
//! `RETURN e ORDER BY e.seq` ne doit rendre qu'UNE colonne (#353).
//!
//! Avant le correctif, la clé de tri `e.seq` était matérialisée comme colonne
//! `e_seq` pour trier, puis jamais retirée : le résultat avait deux colonnes
//! (`e`, `e_seq`) là où Neo4j n'en rend qu'une. Le cas touche 85 des 307
//! requêtes comparables de Project Orchestrator (`n_created_at`, `n_id`,
//! `d_decided_at`, `p_created_at`…).
//!
//! La clé n'était retirée que dans la branche « la variable n'est PAS projetée »
//! (`RETURN e.name ORDER BY e.seq`), jamais dans la branche « la variable est
//! projetée », où `needs_pre_return` vaut `false`.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test order_by_colonne_non_projetee
//! ```

#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const LANGAGES: [&str; 2] = ["cypher", "gql"];

/// Trois événements insérés dans le désordre : seq 3, 1, 2.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    for seq in [3_i64, 1, 2] {
        session.create_node_with_props(&["E"], vec![("seq", Value::Int64(seq))]);
    }
    db
}

fn jouer(db: &ObrainDB, langage: &str, requete: &str) -> obrain_engine::database::QueryResult {
    db.session()
        .execute_language(requete, langage, None)
        .unwrap_or_else(|e| panic!("[{langage}] {requete}\n{e}"))
}

#[test]
fn variable_projetee_une_seule_colonne() {
    let db = base();
    for langage in LANGAGES {
        let r = jouer(&db, langage, "MATCH (e:E) RETURN e ORDER BY e.seq");
        assert_eq!(
            r.columns.len(),
            1,
            "[{langage}] colonnes rendues : {:?} (attendu : une seule, `e`)",
            r.columns
        );
        assert!(
            r.rows.iter().all(|l| l.len() == 1),
            "[{langage}] chaque ligne doit avoir une seule valeur : {:?}",
            r.rows
        );
        assert_eq!(r.rows.len(), 3);
    }
}

#[test]
fn l_ordre_est_celui_de_la_cle_meme_sans_la_colonne() {
    let db = base();
    for langage in LANGAGES {
        // Référence : la clé est projetée, donc visible, et prouve l'ordre.
        let reference = jouer(&db, langage, "MATCH (e:E) RETURN e, e.seq ORDER BY e.seq");
        let seqs: Vec<&Value> = reference.rows.iter().map(|l| &l[1]).collect();
        assert_eq!(
            seqs,
            vec![&Value::Int64(1), &Value::Int64(2), &Value::Int64(3)],
            "[{langage}] la référence doit être triée par seq"
        );
        let attendu: Vec<&Value> = reference.rows.iter().map(|l| &l[0]).collect();

        let r = jouer(&db, langage, "MATCH (e:E) RETURN e ORDER BY e.seq");
        let obtenu: Vec<&Value> = r.rows.iter().map(|l| &l[0]).collect();
        assert_eq!(obtenu, attendu, "[{langage}] même ordre que la référence");
    }
}

#[test]
fn ordre_decroissant_et_limite() {
    let db = base();
    for langage in LANGAGES {
        let ref_desc = jouer(
            &db,
            langage,
            "MATCH (e:E) RETURN e, e.seq ORDER BY e.seq DESC",
        );
        let r = jouer(
            &db,
            langage,
            "MATCH (e:E) RETURN e ORDER BY e.seq DESC LIMIT 2",
        );
        assert_eq!(r.columns.len(), 1, "[{langage}] {:?}", r.columns);
        assert_eq!(r.rows.len(), 2, "[{langage}] LIMIT 2");
        assert_eq!(
            r.rows[0][0], ref_desc.rows[0][0],
            "[{langage}] premier = seq 3"
        );
        assert_eq!(
            r.rows[1][0], ref_desc.rows[1][0],
            "[{langage}] second = seq 2"
        );
    }
}

#[test]
fn la_cle_projetee_reste_visible() {
    // Non-régression : quand la clé EST demandée, elle reste une colonne.
    let db = base();
    for langage in LANGAGES {
        let r = jouer(&db, langage, "MATCH (e:E) RETURN e, e.seq ORDER BY e.seq");
        assert_eq!(r.columns.len(), 2, "[{langage}] {:?}", r.columns);
    }
}

#[test]
fn variable_non_projetee_reste_a_une_colonne() {
    // Non-régression : la branche déjà correcte (`RETURN e.seq` n'est pas `e`).
    let db = base();
    for langage in LANGAGES {
        let r = jouer(&db, langage, "MATCH (e:E) RETURN e.seq AS s ORDER BY e.seq");
        assert_eq!(r.columns.len(), 1, "[{langage}] {:?}", r.columns);
        let ordre: Vec<&Value> = r.rows.iter().map(|l| &l[0]).collect();
        assert_eq!(
            ordre,
            vec![&Value::Int64(1), &Value::Int64(2), &Value::Int64(3)]
        );
    }
}
