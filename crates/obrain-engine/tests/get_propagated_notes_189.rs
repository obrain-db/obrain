//! Les notes propagées (`get_propagated_notes` de PO, obrain-db/obrain#189)
//! sur un graphe de même forme que la copie PO : des MILLIERS de notes liées
//! au MÊME nœud (un projet, un fichier central), qui atteint la cible par
//! plusieurs chemins.
//!
//! Deux coûts, tous deux mesurés sur la copie PO (3,9 M nœuds, note_23 :
//! 409 524 lignes `(note, source)` en entrée, 4,9 M lignes de chemin) :
//!
//! 1. l'expansion refaisait la BFS de la source pour CHAQUE note qui s'y lie,
//!    alors que le résultat ne dépend que de `(source, cible)` ;
//! 2. `[node IN nodes(path) | coalesce(node.name, node.path, node.id)]`
//!    hydratait le nœud ENTIER (toutes ses propriétés, le `content` d'une note
//!    pèse des ko) pour lire UNE propriété, par élément et par ligne : 84 %
//!    du temps de la requête.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test get_propagated_notes_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_core::execution::operators::chemins_enumeres;
use obrain_engine::ObrainDB;
use obrain_substrate::noeuds_hydrates;

const NOTES: usize = 200;
const INTERMEDIAIRES: usize = 20;

/// `NOTES` notes actives liées au MÊME nœud `hub`, qui atteint la cible
/// `t.rs` par `INTERMEDIAIRES` chemins de 2 sauts (`hub -> mid_i -> t.rs`).
/// Chaque note porte un `content` volumineux.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let cible = s.create_node_with_props(&["File"], [("path", Value::String("t.rs".into()))]);
    let hub = s.create_node_with_props(&["Fn"], [("name", Value::String("hub".into()))]);
    for i in 0..INTERMEDIAIRES {
        let mid = s.create_node_with_props(
            &["Fn"],
            [("name", Value::String(format!("mid_{i}").into()))],
        );
        s.create_edge(hub, mid, "CALLS");
        s.create_edge(mid, cible, "CALLS");
    }
    for i in 0..NOTES {
        let n = s.create_node_with_props(
            &["Note"],
            [
                ("status", Value::String("active".into())),
                ("content", Value::String("x".repeat(20_000).into())),
                ("rang", Value::Int64(i as i64)),
            ],
        );
        s.create_edge(n, hub, "LINKED_TO");
    }
    db
}

const REQUETE: &str = "MATCH (target:File {path: 't.rs'}) \
     MATCH path = (n:Note)-[:LINKED_TO]->(source)-[:CONTAINS|IMPORTS|CALLS*0..3]->(target) \
     WHERE n.status = 'active' \
     WITH n, source, path, length(path) - 1 AS distance, \
          [node IN nodes(path) | coalesce(node.name, node.path, node.id)] AS path_names \
     RETURN count(*) AS lignes, sum(size(path_names)) AS noms, sum(distance) AS distances";

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu, reçu {autre:?}"),
    }
}

#[test]
fn notes_liees_au_meme_noeud_meme_resultat() {
    let db = base();
    let r = db.session().execute_cypher(REQUETE).expect("requête");
    let ligne = &r.rows[0];
    // Une ligne par (note, chemin hub -> mid_i -> t.rs).
    let lignes = (NOTES * INTERMEDIAIRES) as i64;
    assert_eq!(entier(&ligne[0]), lignes, "lignes");
    // Chaque chemin : note, hub, mid, cible = 4 noms. (obrain#189 : `nodes(path)`
    // couvrait avant la seule expansion de longueur variable, le saut LINKED_TO
    // de la note etait absent du chemin nomme. Corrige : chemin compose complet.)
    assert_eq!(entier(&ligne[1]), 4 * lignes, "noms de chemin");
    // `length(path)` = 3 (LINKED_TO + 2 sauts) : `length(path) - 1` = 2.
    assert_eq!(entier(&ligne[2]), 2 * lignes, "distances");
}

#[test]
fn la_bfs_d_une_source_n_est_pas_refaite_par_note() {
    let db = base();
    let avant = chemins_enumeres();
    db.session().execute_cypher(REQUETE).expect("requête");
    let enumeres = chemins_enumeres() - avant;
    // Sans partage : NOTES (saut LINKED_TO) + NOTES * INTERMEDIAIRES chemins.
    // Avec : NOTES + INTERMEDIAIRES (la BFS du hub, faite une seule fois).
    assert!(
        enumeres <= (NOTES + 2 * INTERMEDIAIRES) as u64,
        "{enumeres} chemins énumérés : la BFS du hub est refaite pour chacune des {NOTES} notes"
    );
}

#[test]
fn lire_une_propriete_d_un_element_de_chemin_n_hydrate_pas_le_noeud() {
    let db = base();
    let avant = noeuds_hydrates();
    db.session().execute_cypher(REQUETE).expect("requête");
    let hydrates = noeuds_hydrates() - avant;
    // `nodes(path)` : 3 éléments x 3 propriétés par ligne, soit des dizaines
    // de milliers d'hydratations de nœud entier sans le lecteur ciblé.
    assert!(
        hydrates <= (NOTES + INTERMEDIAIRES) as u64,
        "{hydrates} nœuds hydratés en entier pour lire des noms"
    );
}
