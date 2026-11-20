//! `OPTIONAL MATCH` et `EXISTS { … }` CORRÉLÉS à la ligne de gauche (F1 de
//! l'audit de complexité, obrain-db/obrain#189).
//!
//! Un `OPTIONAL MATCH` (ou un `EXISTS`) dont le motif repart d'un nœud déjà
//! lié construisait son côté droit SANS la ligne de gauche : `NodeScan(x:*)`
//! sur tout le graphe puis expansion, joint par hachage. O(V + E) par clause,
//! même pour un nœud de gauche unique (2 à 6 s sur les 3,9 M nœuds de la base
//! PO). Le côté droit part désormais de la ligne de gauche (`Apply` optionnel,
//! semi-jointure pilotée par la ligne) : O(voisinage) par ligne.
//!
//! Deux familles de tests :
//! - le COÛT, observé à l'exécution : nœuds balayés (`noeuds_balayes`) et
//!   chemins énumérés (`chemins_enumeres`) par la requête ;
//! - la SÉMANTIQUE, celle de Neo4j : NULL sans correspondance, une ligne par
//!   correspondance, `WHERE` de l'optionnel gardé DANS l'optionnel, variables
//!   partagées multiples, optionnels enchaînés (sur un NULL), `EXISTS` sous
//!   `NOT` et dans une expression composite, au plus une ligne par `EXISTS`.
//!
//! ```bash
//! cargo test -p obrain-engine --features cypher --test correlation_optionnel_exists_f1
//! ```
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_core::execution::operators::{chemins_enumeres, noeuds_balayes};
use obrain_engine::ObrainDB;

/// Nœuds de bruit : ni `File`, ni `Function`, ni `Project`, reliés entre eux
/// par `CONTAINS`, `CALLS` et `IMPORTS`. Un côté droit construit sur tout le
/// graphe les balaie tous ; un côté droit corrélé n'en voit aucun.
const BRUIT: usize = 400;

/// ```text
/// p1 -CONTAINS-> a.rs -CONTAINS-> fn1, fn2 (Function), st1 (Struct)
/// p2 -CONTAINS-> b.rs -CONTAINS-> fn3 (Function)
///                c.rs (vide ; importé par b.rs qui l'est par a.rs)
/// fn2 -CALLS-> fn1 <-CALLS- fn3
/// a.rs -DEPENDS {protocol: 'http'}-> b.rs
/// ```
fn base() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher(
            "CREATE (p1:Project {id: 'p1'}), (p2:Project {id: 'p2'}), \
                    (a:File {path: 'a.rs'}), (b:File {path: 'b.rs'}), (c:File {path: 'c.rs'}), \
                    (fn1:Function {name: 'fn1'}), (fn2:Function {name: 'fn2'}), \
                    (fn3:Function {name: 'fn3'}), (st1:Struct {name: 'st1'}), \
                    (p1)-[:CONTAINS]->(a), (p2)-[:CONTAINS]->(b), \
                    (a)-[:CONTAINS]->(fn1), (a)-[:CONTAINS]->(fn2), (a)-[:CONTAINS]->(st1), \
                    (b)-[:CONTAINS]->(fn3), \
                    (fn2)-[:CALLS]->(fn1), (fn3)-[:CALLS]->(fn1), \
                    (b)-[:IMPORTS]->(c), (a)-[:IMPORTS]->(b), \
                    (a)-[:DEPENDS {protocol: 'http'}]->(b)",
        )
        .unwrap();
    let s = db.session();
    let mut precedent = None;
    for i in 0..BRUIT {
        let n = s.create_node_with_props(&["Bruit"], [("k", Value::Int64(i as i64))]);
        if let Some(p) = precedent {
            s.create_edge(p, n, "CONTAINS");
            s.create_edge(p, n, "CALLS");
            s.create_edge(p, n, "IMPORTS");
        }
        precedent = Some(n);
    }
    db
}

fn lignes(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    db.session()
        .execute_cypher(requete)
        .unwrap_or_else(|e| panic!("{requete}\n{e}"))
        .rows
}

/// Lignes, nœuds balayés et chemins énumérés par la requête.
fn mesure(db: &ObrainDB, requete: &str) -> (Vec<Vec<Value>>, u64, u64) {
    let (n0, c0) = (noeuds_balayes(), chemins_enumeres());
    let r = lignes(db, requete);
    (r, noeuds_balayes() - n0, chemins_enumeres() - c0)
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

// ─────────────────────────────── COÛT ───────────────────────────────

/// Forme de `code_32` : le seek du fichier balaie les 3 `File`, l'`OPTIONAL
/// MATCH` ne doit rien balayer de plus. Le côté droit indépendant balayait
/// tout le graphe (≥ `BRUIT` nœuds).
#[test]
fn optional_correle_ne_balaie_pas_le_graphe() {
    let db = base();
    let (r, balayes, _) = mesure(
        &db,
        "MATCH (f:File {path: 'a.rs'}) \
         OPTIONAL MATCH (f)-[:CONTAINS]->(x:Function) \
         OPTIONAL MATCH (f)-[:CONTAINS]->(y:Struct) \
         RETURN count(DISTINCT x) AS fonctions, count(DISTINCT y) AS structures",
    );
    assert_eq!(r, vec![vec![Value::Int64(2), Value::Int64(1)]]);
    assert!(
        balayes <= 3,
        "l'OPTIONAL MATCH a balayé {balayes} nœuds (attendu : les 3 File du seek seulement)"
    );
}

/// Forme de `task_05` : longueur variable depuis le nœud lié. Le côté droit
/// indépendant énumérait les chemins `IMPORTS*1..3` de TOUS les nœuds.
#[test]
fn optional_longueur_variable_n_enumere_que_depuis_le_noeud_lie() {
    let db = base();
    let (r, balayes, chemins) = mesure(
        &db,
        "MATCH (f:File {path: 'c.rs'}) \
         OPTIONAL MATCH (f)<-[:IMPORTS*1..3]-(d:File) \
         RETURN f.path AS fichier, count(DISTINCT d) AS dependants",
    );
    assert_eq!(r, vec![vec![s("c.rs"), Value::Int64(2)]]);
    assert!(balayes <= 3, "{balayes} nœuds balayés");
    assert!(
        chemins <= 2,
        "{chemins} chemins énumérés (attendu : c<-b, c<-b<-a)"
    );
}

/// Forme de `code_33` / `code_24` : `EXISTS` dont les deux extrémités sont
/// liées. La semi-jointure par hachage construisait son côté droit depuis
/// `NodeScan(x:*)`.
#[test]
fn exists_correle_ne_balaie_pas_le_graphe() {
    let db = base();
    let (r, balayes, _) = mesure(
        &db,
        "MATCH (p:Project {id: 'p1'}) MATCH (x:Function) \
         WHERE EXISTS { MATCH (x)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) } \
         RETURN x.name AS nom ORDER BY nom",
    );
    assert_eq!(r, vec![vec![s("fn1")], vec![s("fn2")]]);
    assert!(
        balayes <= 2 + 3,
        "l'EXISTS a balayé {balayes} nœuds (attendu : 2 Project + 3 Function)"
    );
}

/// Forme de `code_45` : `EXISTS` à longueur variable vers un nœud lié.
#[test]
fn exists_longueur_variable_n_enumere_que_depuis_la_ligne() {
    let db = base();
    let (r, balayes, chemins) = mesure(
        &db,
        "MATCH (p:Project {id: 'p1'}) MATCH (x:Function) \
         WHERE EXISTS { MATCH (x)<-[:CONTAINS*1..2]-(p) } \
         RETURN x.name AS nom ORDER BY nom",
    );
    assert_eq!(r, vec![vec![s("fn1")], vec![s("fn2")]]);
    assert!(balayes <= 2 + 3, "{balayes} nœuds balayés");
    assert!(
        chemins <= 3 * 2,
        "{chemins} chemins énumérés (attendu : au plus 2 par Function)"
    );
}

// ───────────────────────────── SÉMANTIQUE ─────────────────────────────

#[test]
fn optional_zero_une_plusieurs_correspondances() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) OPTIONAL MATCH (f)-[:CONTAINS]->(x:Function) \
             RETURN f.path AS p, x.name AS n ORDER BY p, n"
        ),
        vec![
            vec![s("a.rs"), s("fn1")],
            vec![s("a.rs"), s("fn2")],
            vec![s("b.rs"), s("fn3")],
            vec![s("c.rs"), Value::Null],
        ]
    );
}

/// Le `WHERE` d'un `OPTIONAL MATCH` en fait partie : il ne retire pas la
/// ligne de gauche, il rend NULL.
#[test]
fn optional_where_reste_dans_l_optionnel() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) OPTIONAL MATCH (f)-[:CONTAINS]->(x) WHERE x.name = 'fn2' \
             RETURN f.path AS p, x.name AS n ORDER BY p"
        ),
        vec![
            vec![s("a.rs"), s("fn2")],
            vec![s("b.rs"), Value::Null],
            vec![s("c.rs"), Value::Null],
        ]
    );
    // Un `EXISTS` dans ce `WHERE`, qui cite une variable de GAUCHE absente
    // du motif (`p`) : évalué dans l'optionnel, sur la ligne.
    assert_eq!(
        lignes(
            &db,
            "MATCH (p:Project)-[:CONTAINS]->(f:File) \
             OPTIONAL MATCH (f)-[:CONTAINS]->(x:Function) \
             WHERE EXISTS { MATCH (x)<-[:CALLS]-(:Function)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) } \
             RETURN p.id AS p, x.name AS n ORDER BY p"
        ),
        vec![vec![s("p1"), s("fn1")], vec![s("p2"), Value::Null]]
    );
}

#[test]
fn optional_plusieurs_variables_partagees() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (a:Function {name: 'fn2'}), (b:Function {name: 'fn1'}) \
             OPTIONAL MATCH (a)-[r:CALLS]->(b) \
             OPTIONAL MATCH (b)-[r2:CALLS]->(a) \
             RETURN type(r) AS aller, type(r2) AS retour"
        ),
        vec![vec![s("CALLS"), Value::Null]]
    );
    // Deux nœuds partagés, la correspondance n'existe que pour g = f.
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File)-[:CONTAINS]->(x:Function), (g:File) \
             OPTIONAL MATCH (g)-[c:CONTAINS]->(x) \
             RETURN x.name AS x, g.path AS g, c IS NOT NULL AS contient ORDER BY x, g"
        ),
        vec![
            vec![s("fn1"), s("a.rs"), Value::Bool(true)],
            vec![s("fn1"), s("b.rs"), Value::Bool(false)],
            vec![s("fn1"), s("c.rs"), Value::Bool(false)],
            vec![s("fn2"), s("a.rs"), Value::Bool(true)],
            vec![s("fn2"), s("b.rs"), Value::Bool(false)],
            vec![s("fn2"), s("c.rs"), Value::Bool(false)],
            vec![s("fn3"), s("a.rs"), Value::Bool(false)],
            vec![s("fn3"), s("b.rs"), Value::Bool(true)],
            vec![s("fn3"), s("c.rs"), Value::Bool(false)],
        ]
    );
}

/// Le second optionnel repart d'une variable que le premier a pu laisser à
/// NULL : NULL à son tour, la ligne reste.
#[test]
fn optionnels_enchaines() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) \
             OPTIONAL MATCH (f)-[:CONTAINS]->(x:Function) \
             OPTIONAL MATCH (x)<-[:CALLS]-(c:Function) \
             RETURN f.path AS f, x.name AS x, c.name AS c ORDER BY f, x, c"
        ),
        vec![
            vec![s("a.rs"), s("fn1"), s("fn2")],
            vec![s("a.rs"), s("fn1"), s("fn3")],
            vec![s("a.rs"), s("fn2"), Value::Null],
            vec![s("b.rs"), s("fn3"), Value::Null],
            vec![s("c.rs"), Value::Null, Value::Null],
        ]
    );
}

/// Une ligne de gauche répétée a chacune ses correspondances.
#[test]
fn optional_lignes_de_gauche_repetees() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "UNWIND [1, 2] AS i MATCH (f:File {path: 'b.rs'}) \
             OPTIONAL MATCH (f)-[:CONTAINS]->(x) RETURN i, x.name AS n ORDER BY i"
        ),
        vec![
            vec![Value::Int64(1), s("fn3")],
            vec![Value::Int64(2), s("fn3")]
        ]
    );
}

/// Une propriété d'ARÊTE de l'optionnel se lit après lui (forme de `ws_19`).
#[test]
fn optional_propriete_d_arete() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) OPTIONAL MATCH (f)-[d:DEPENDS]->(g:File) \
             RETURN f.path AS f, d.protocol AS protocole, g.path AS g ORDER BY f"
        ),
        vec![
            vec![s("a.rs"), s("http"), s("b.rs")],
            vec![s("b.rs"), Value::Null, Value::Null],
            vec![s("c.rs"), Value::Null, Value::Null],
        ]
    );
}

/// Forme de `code_33` : l'`EXISTS` du `WHERE` de l'optionnel cite `p`, lié à
/// gauche.
#[test]
fn optional_puis_exists_lie_a_gauche() {
    let db = base();
    // fn1 : appelé par fn2 (p1) et fn3 (p2) ; fn2 : appelé par personne.
    for (fonction, projet, attendu) in [("fn1", "p1", 1), ("fn1", "p2", 1), ("fn2", "p1", 0)] {
        assert_eq!(
            lignes(
                &db,
                &format!(
                    "MATCH (f:Function {{name: '{fonction}'}}) \
                     MATCH (p:Project {{id: '{projet}'}}) \
                     OPTIONAL MATCH (caller:Function)-[:CALLS]->(f) \
                     WHERE caller IS NOT NULL AND EXISTS {{ MATCH (caller)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) }} \
                     RETURN count(caller) AS n"
                )
            ),
            vec![vec![Value::Int64(attendu)]],
            "{fonction} {projet}"
        );
    }
}

#[test]
fn exists_sous_not() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (p:Project {id: 'p1'}) MATCH (x:Function) \
             WHERE NOT EXISTS { MATCH (x)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) } \
             RETURN x.name AS nom ORDER BY nom"
        ),
        vec![vec![s("fn3")]]
    );
}

#[test]
fn exists_dans_une_expression_composite() {
    let db = base();
    let exists = "EXISTS { MATCH (x)<-[:CONTAINS]-(:File)<-[:CONTAINS]-(p) }";
    assert_eq!(
        lignes(
            &db,
            &format!(
                "MATCH (p:Project {{id: 'p1'}}) MATCH (x:Function) \
                 WHERE x.name = 'fn3' OR {exists} RETURN x.name AS nom ORDER BY nom"
            )
        ),
        vec![vec![s("fn1")], vec![s("fn2")], vec![s("fn3")]]
    );
    assert_eq!(
        lignes(
            &db,
            &format!(
                "MATCH (p:Project {{id: 'p1'}}) MATCH (x:Function) \
                 WHERE x.name <> 'fn1' AND {exists} RETURN x.name AS nom ORDER BY nom"
            )
        ),
        vec![vec![s("fn2")]]
    );
    assert_eq!(
        lignes(
            &db,
            &format!(
                "MATCH (p:Project {{id: 'p1'}}) MATCH (x:Function) \
                 WHERE NOT {exists} OR x.name = 'fn1' RETURN x.name AS nom ORDER BY nom"
            )
        ),
        vec![vec![s("fn1")], vec![s("fn3")]]
    );
}

/// Une semi-jointure ne duplique pas : `fn1` a deux appelants, il sort une
/// fois.
#[test]
fn exists_au_plus_une_ligne_par_ligne_externe() {
    let db = base();
    assert_eq!(
        lignes(
            &db,
            "MATCH (x:Function) \
             WHERE EXISTS { MATCH (x)<-[:CALLS]-(c:Function)<-[:CONTAINS]-(:File) } \
             RETURN x.name AS nom"
        ),
        vec![vec![s("fn1")]]
    );
}

/// Chaîne de sauts SANS filtre intermédiaire : le planificateur en fait une
/// chaîne factorisée (`LazyFactorizedChainOperator`), qu'un sous-plan
/// corrélé rejoue pour chaque ligne externe. Elle ne se laissait pas
/// remettre à zéro : vide dès la première ligne (`EXISTS` faux partout,
/// `OPTIONAL MATCH` à NULL, `CALL { WITH … }` à 0).
#[test]
fn chaine_factorisee_rejouee_par_ligne() {
    let db = base();
    // a.rs -CONTAINS-> fn2 -CALLS-> fn1 ; b.rs -CONTAINS-> fn3 -CALLS-> fn1.
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) WHERE EXISTS { MATCH (f)-[:CONTAINS]->(x)-[:CALLS]->(y) } \
             RETURN f.path AS f ORDER BY f"
        ),
        vec![vec![s("a.rs")], vec![s("b.rs")]]
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) OPTIONAL MATCH (f)-[:CONTAINS]->(x)-[:CALLS]->(y) \
             RETURN f.path AS f, x.name AS x, y.name AS y ORDER BY f"
        ),
        vec![
            vec![s("a.rs"), s("fn2"), s("fn1")],
            vec![s("b.rs"), s("fn3"), s("fn1")],
            vec![s("c.rs"), Value::Null, Value::Null],
        ]
    );
    assert_eq!(
        lignes(
            &db,
            "MATCH (f:File) CALL { WITH f MATCH (f)-[:CONTAINS]->(x)-[:CALLS]->(y) \
             RETURN count(*) AS k } RETURN f.path AS f, k ORDER BY f"
        ),
        vec![
            vec![s("a.rs"), Value::Int64(1)],
            vec![s("b.rs"), Value::Int64(1)],
            vec![s("c.rs"), Value::Int64(0)],
        ]
    );
}
