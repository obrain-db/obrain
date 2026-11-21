//! Graine d'index corrélée armée par l'index ADAPTATIF (obrain-db/obrain#189, F3).
//!
//! `UNWIND $paths AS p1 MATCH (n1 {path: p1})-[r:IMPORTS]->(n2)` : la clé
//! `n1.path = p1` dépend de la ligne externe. `try_plan_correlated_index_seek`
//! n'était armé que par `has_property_index` (index DÉJÀ en mémoire), jamais
//! par `peut_indexer_propriete` (index que le substrat bâtit à la 2e demande,
//! comme le chemin littéral depuis #248). L'index n'était donc jamais bâti
//! pour ce chemin : produit croisé |paths| × V à CHAQUE appel (code_46 de PO :
//! 12 à 59 s).
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test graine_correlee_adaptative_189
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_core::execution::operators::noeuds_balayes;
use obrain_engine::ObrainDB;

const FICHIERS: usize = 400;

/// `FICHIERS` fichiers en chaîne d'imports, plus autant de nœuds sans `path`.
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let fichiers: Vec<_> = (0..FICHIERS)
        .map(|i| {
            s.create_node_with_props(
                &["File"],
                [("path", Value::String(format!("f{i}.rs").into()))],
            )
        })
        .collect();
    for w in fichiers.windows(2) {
        s.create_edge(w[0], w[1], "IMPORTS");
    }
    for i in 0..FICHIERS {
        s.create_node_with_props(&["Function"], [("name", Value::Int64(i as i64))]);
    }
    db
}

const REQUETE: &str = "UNWIND ['f3.rs', 'f4.rs', 'f5.rs', 'f9.rs', 'absent.rs'] AS p1 \
     MATCH (n1 {path: p1})-[r:IMPORTS]->(n2) \
     WHERE n2.path IN ['f3.rs', 'f4.rs', 'f5.rs', 'f9.rs', 'absent.rs'] \
     RETURN DISTINCT n1.path AS de, type(r) AS t, n2.path AS vers";

fn executer(db: &ObrainDB) -> (Vec<String>, u64) {
    let avant = noeuds_balayes();
    let r = db
        .session()
        .execute_cypher(REQUETE)
        .unwrap_or_else(|e| panic!("{REQUETE}\n{e}"));
    let balayes = noeuds_balayes() - avant;
    let mut lignes: Vec<String> = r.rows.iter().map(|l| format!("{l:?}")).collect();
    lignes.sort();
    (lignes, balayes)
}

#[test]
fn la_cle_correlee_finit_par_etre_servie_par_l_index() {
    let db = base();
    let (premiere, _) = executer(&db);
    assert_eq!(premiere.len(), 2, "f3->f4, f4->f5 : {premiere:?}");
    // Le substrat bâtit l'index à la 2e demande de la même propriété ; à
    // partir de là, plus aucun balayage de nœuds.
    let (deuxieme, _) = executer(&db);
    let (troisieme, balayes) = executer(&db);
    assert_eq!(deuxieme, premiere, "résultat");
    assert_eq!(troisieme, premiere, "résultat");
    assert_eq!(
        balayes, 0,
        "{balayes} nœuds balayés au 3e appel : la graine corrélée n'est pas servie par l'index"
    );
}
