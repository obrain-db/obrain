//! `count(DISTINCT labels(n)[0])` : raccourci par premier label (obrain#191).
//!
//! Avant : la clé `labels(n)[0]` — un `IndexAccess` sur `labels(n)` — n'était
//! pas reconnue par le raccourci de comptage : la requête balayait chaque nœud,
//! projetait sa liste de labels, puis hachait le premier. Sur l'export PO du
//! 17/11 (3 901 829 nœuds, 64 labels) : **1 870 ms**, contre 389 ms à Neo4j.
//!
//! Après : le cardinal des premiers labels présents se lit dans
//! `node_counts_by_first_label()`, la primitive du raccourci groupé.
//!
//! Ce fichier vérifie la correction, pas la vitesse. Chaque cas compare le
//! raccourci à la forme groupée équivalente — un chemin qui ne partage pas
//! le même code de planification — et à une valeur écrite à la main.
#![cfg(feature = "cypher")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn entier(db: &ObrainDB, requete: &str) -> i64 {
    let lignes = db
        .session()
        .execute_cypher(requete)
        .unwrap_or_else(|erreur| panic!("{requete}\n{erreur}"))
        .rows;
    match lignes.first().and_then(|l| l.first()) {
        Some(Value::Int64(valeur)) => *valeur,
        autre => panic!("entier attendu pour {requete}, reçu {autre:?}"),
    }
}

/// Le témoin indépendant : les clés non nulles de la forme groupée.
fn groupes_non_nuls(db: &ObrainDB) -> i64 {
    db.session()
        .execute_cypher("MATCH (n) RETURN labels(n)[0] AS l, count(n) AS c")
        .unwrap()
        .rows
        .iter()
        .filter(|l| !matches!(l.first(), Some(Value::Null) | None))
        .count() as i64
}

const DISTINCT: &str = "MATCH (n) RETURN count(DISTINCT labels(n)[0]) AS c";

/// Un label par nœud : trois labels, trois valeurs distinctes ; le nœud sans
/// label (`null`) ne compte pas, comme dans tout `count(DISTINCT ...)`.
#[test]
fn mono_etiquette_et_noeud_sans_label() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A), (:A), (:B), (:C), ({k: 1})")
        .unwrap();
    assert_eq!(entier(&db, DISTINCT), 3);
    assert_eq!(entier(&db, DISTINCT), groupes_non_nuls(&db));
}

/// Plusieurs labels par nœud : seul le PREMIER compte. `count(DISTINCT
/// labels(n)[0])` ne vaut pas le nombre de labels du registre.
#[test]
fn multi_etiquettes_seul_le_premier_compte() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A:B), (:A:B), (:A), (:C)")
        .unwrap();
    let c = entier(&db, DISTINCT);
    assert_eq!(c, groupes_non_nuls(&db));
    // `B` est porté par deux nœuds mais n'est jamais premier (`A` est entré au
    // registre avant lui) : deux valeurs distinctes, `A` et `C`, pour trois
    // labels au registre.
    assert_eq!(entier(&db, "MATCH (n:B) RETURN count(n) AS c"), 2);
    assert_eq!(c, 2);
}

/// Après écriture : un label ajouté, puis un nœud supprimé, se voient.
#[test]
fn apres_ecriture() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A {k: 1}), (:B {k: 2})")
        .unwrap();
    assert_eq!(entier(&db, DISTINCT), 2);

    db.session().execute_cypher("CREATE (:C {k: 3})").unwrap();
    assert_eq!(entier(&db, DISTINCT), 3);

    db.session()
        .execute_cypher("MATCH (n:B) DETACH DELETE n")
        .unwrap();
    assert_eq!(entier(&db, DISTINCT), 2);
    assert_eq!(entier(&db, DISTINCT), groupes_non_nuls(&db));
}

/// Base vide : zéro, pas d'erreur.
#[test]
fn base_vide() {
    let db = ObrainDB::new_in_memory();
    assert_eq!(entier(&db, DISTINCT), 0);
}

/// `MATCH (n:L)` restreint aux porteurs de `L`, dont le premier label peut
/// être un autre : le raccourci doit RENONCER, le chemin générique répond.
#[test]
fn scan_etiquete_reste_juste() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A:B), (:B), (:C)")
        .unwrap();
    let c = entier(&db, "MATCH (n:B) RETURN count(DISTINCT labels(n)[0]) AS c");
    let attendu = db
        .session()
        .execute_cypher("MATCH (n:B) RETURN labels(n)[0] AS l, count(*) AS c")
        .unwrap()
        .rows
        .len() as i64;
    assert_eq!(c, attendu);
}

/// Un autre indice n'est pas la même question : pas de raccourci, et la
/// réponse reste celle du balayage.
#[test]
fn indice_un_non_raccourci() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A:B), (:A:B), (:C)")
        .unwrap();
    assert_eq!(
        entier(&db, "MATCH (n) RETURN count(DISTINCT labels(n)[1]) AS c"),
        1
    );
}

/// Témoin : `count(n)` et `count(n:L)` gardent leurs compteurs.
#[test]
fn comptes_de_noeuds_inchanges() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A:B), (:A), (:C), ()")
        .unwrap();
    assert_eq!(entier(&db, "MATCH (n) RETURN count(n) AS c"), 4);
    assert_eq!(entier(&db, "MATCH (n:A) RETURN count(n) AS c"), 2);
    assert_eq!(entier(&db, "MATCH (n:B) RETURN count(n) AS c"), 1);
}

/// Régression trouvée en écrivant ce raccourci : `type(n)` sur un NŒUD n'a pas
/// de valeur, mais le booléen « types distincts » tombait dans la branche
/// nœuds et rendait `node_count()` — mesuré 3 901 829 sur l'export PO, au lieu
/// de 0. Le raccourci renonce désormais ; le chemin générique répond.
#[test]
fn type_d_un_noeud_ne_compte_pas_les_noeuds() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute_cypher("CREATE (:A), (:B), (:C)")
        .unwrap();
    assert_eq!(
        entier(&db, "MATCH (n) RETURN count(DISTINCT type(n)) AS c"),
        0
    );
}
