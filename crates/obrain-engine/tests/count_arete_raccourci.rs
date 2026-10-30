//! Le raccourci de comptage d'arêtes — et surtout ce qu'il doit REFUSER.
//!
//! `plan_aggregate` annonçait depuis longtemps « MATCH ()-[r]->() RETURN
//! count(r) → store.edge_count() in O(1) » sans que le raccourci existe :
//! `try_plan_count_shortcut` ne filtrait que `NodeScan`.
//!
//! Un raccourci de comptage trop large ne rend pas un résultat lent, il rend
//! un NOMBRE FAUX. Ce dépôt en porte le précédent, consigné dans la doc
//! d'`edge_counts_by_type` : un `count: 0` inventé avait fait conclure à un
//! agent qu'il n'avait aucun moyen de créer des arêtes, alors que la même base
//! répondait `MATCH ()-[r]->()` avec douze types distincts.
//!
//! Chaque test ci-dessous confronte donc le nombre rendu à une VÉRITÉ connue
//! par construction, et les formes qui doivent tomber dans le plan générique
//! sont vérifiées elles aussi — un raccourci qui se déclenche à tort ne se
//! verrait nulle part ailleurs.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Graphe de référence, dont les comptes sont connus par construction :
///
/// * 4 arêtes `LIE`, 2 `AUTRE`, 1 `RARE` — soit **7 vivantes**.
/// * Une boucle `(n2)-[:LIE]->(n2)`, pour que le cas ne soit pas seulement
///   théorique.
fn monter() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..5 {
        s.execute(&format!("INSERT (:N {{id: {i}}})")).unwrap();
    }
    let lien = |a: i64, b: i64, t: &str| {
        format!("MATCH (x:N {{id: {a}}}), (y:N {{id: {b}}}) CREATE (x)-[:{t}]->(y)")
    };
    for (a, b, t) in [
        (0, 1, "LIE"),
        (1, 2, "LIE"),
        (2, 2, "LIE"), // boucle
        (3, 4, "LIE"),
        (0, 2, "AUTRE"),
        (1, 3, "AUTRE"),
        (4, 0, "RARE"),
    ] {
        s.execute(&lien(a, b, t)).unwrap();
    }
    db
}

/// Extrait l'unique entier d'un résultat scalaire.
fn scalaire(db: &ObrainDB, requete: &str) -> i64 {
    let r = db.session().execute(requete).unwrap_or_else(|e| {
        panic!("requête refusée : {requete}\n  {e:?}");
    });
    assert_eq!(
        r.rows.len(),
        1,
        "{requete} doit rendre UNE ligne : {:?}",
        r.rows
    );
    match r.rows[0].first() {
        Some(Value::Int64(n)) => *n,
        autre => panic!("{requete} : entier attendu, obtenu {autre:?}"),
    }
}

/// Le comptage par le chemin GÉNÉRIQUE, qui ne prend jamais le raccourci :
/// on rend les lignes et on les compte. C'est la vérité de référence.
fn verite_par_lignes(db: &ObrainDB, motif: &str) -> usize {
    db.session()
        .execute(motif)
        .unwrap_or_else(|e| panic!("requête refusée : {motif}\n  {e:?}"))
        .rows
        .len()
}

#[test]
fn count_r_sans_type_egale_la_verite() {
    let db = monter();
    assert_eq!(
        scalaire(&db, "MATCH ()-[r]->() RETURN count(r) AS n"),
        7,
        "sept arêtes vivantes, boucle comprise"
    );
}

#[test]
fn count_r_avec_un_type_egale_la_verite() {
    let db = monter();
    assert_eq!(
        scalaire(&db, "MATCH ()-[r:LIE]->() RETURN count(r) AS n"),
        4
    );
    assert_eq!(
        scalaire(&db, "MATCH ()-[r:AUTRE]->() RETURN count(r) AS n"),
        2
    );
    assert_eq!(
        scalaire(&db, "MATCH ()-[r:RARE]->() RETURN count(r) AS n"),
        1
    );
}

/// Un type jamais interné doit rendre ZÉRO, pas une erreur et pas un total.
#[test]
fn count_r_sur_un_type_inconnu_rend_zero() {
    let db = monter();
    assert_eq!(
        scalaire(&db, "MATCH ()-[r:JAMAIS_VU]->() RETURN count(r) AS n"),
        0
    );
}

/// LA VÉRIFICATION QUI COMPTE : le raccourci doit rendre EXACTEMENT ce que
/// rend le chemin générique. S'ils divergent, le raccourci ment — et c'est le
/// seul endroit où cela se verrait.
#[test]
fn le_raccourci_egale_le_comptage_par_lignes() {
    let db = monter();
    for (motif_compte, motif_lignes) in [
        (
            "MATCH ()-[r]->() RETURN count(r) AS n",
            "MATCH ()-[r]->() RETURN r",
        ),
        (
            "MATCH ()-[r:LIE]->() RETURN count(r) AS n",
            "MATCH ()-[r:LIE]->() RETURN r",
        ),
        (
            "MATCH ()-[r:AUTRE]->() RETURN count(r) AS n",
            "MATCH ()-[r:AUTRE]->() RETURN r",
        ),
    ] {
        let par_raccourci = scalaire(&db, motif_compte);
        let par_lignes = verite_par_lignes(&db, motif_lignes) as i64;
        assert_eq!(
            par_raccourci, par_lignes,
            "divergence sur {motif_compte} : raccourci {par_raccourci}, \
             lignes {par_lignes}"
        );
    }
}

/// Une suppression doit se voir immédiatement. Un compteur figé rendrait
/// l'ancien total avec l'aplomb d'une mesure.
#[test]
fn le_raccourci_suit_les_suppressions() {
    let db = monter();
    assert_eq!(scalaire(&db, "MATCH ()-[r]->() RETURN count(r) AS n"), 7);
    db.session()
        .execute("MATCH ()-[r:RARE]->() DELETE r")
        .unwrap();
    assert_eq!(
        scalaire(&db, "MATCH ()-[r]->() RETURN count(r) AS n"),
        6,
        "le compteur doit suivre la suppression"
    );
    assert_eq!(
        scalaire(&db, "MATCH ()-[r:RARE]->() RETURN count(r) AS n"),
        0
    );
}

// ───────────────────────────────────────────────────────────────────────────
//  LES FORMES QUI DOIVENT TOMBER DANS LE PLAN GÉNÉRIQUE
// ───────────────────────────────────────────────────────────────────────────
//
// Chacune est une manière dont le compteur ne répondrait PAS à la question
// posée. On ne peut pas observer directement quel plan a été choisi ; on
// vérifie donc ce qui compte vraiment — que le nombre reste JUSTE.

/// Un label sur la source filtre les arêtes par leur origine. Le compteur
/// global ne sait rien de ce filtre.
#[test]
fn un_label_sur_la_source_ne_doit_pas_prendre_le_raccourci() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:A {id: 0})").unwrap();
    s.execute("INSERT (:B {id: 1})").unwrap();
    s.execute("INSERT (:B {id: 2})").unwrap();
    s.execute("MATCH (x:A {id: 0}), (y:B {id: 1}) CREATE (x)-[:LIE]->(y)")
        .unwrap();
    s.execute("MATCH (x:B {id: 1}), (y:B {id: 2}) CREATE (x)-[:LIE]->(y)")
        .unwrap();

    assert_eq!(
        scalaire(&db, "MATCH ()-[r:LIE]->() RETURN count(r) AS n"),
        2,
        "sans label : les deux arêtes"
    );
    assert_eq!(
        scalaire(&db, "MATCH (a:A)-[r:LIE]->() RETURN count(r) AS n"),
        1,
        "avec un label sur la source : UNE seule — si le raccourci s'était \
         déclenché, il aurait rendu 2"
    );
}

/// `count(a)` porte sur les NŒUDS de la source, pas sur les arêtes. Le motif
/// en rend un par arête traversée, ce qui n'est pas le compte des arêtes dès
/// qu'un nœud a plusieurs arêtes sortantes — mais surtout, ce n'est pas la
/// même question.
#[test]
fn compter_la_variable_de_source_ne_doit_pas_prendre_le_raccourci() {
    let db = monter();
    let par_lignes = verite_par_lignes(&db, "MATCH (a)-[r]->() RETURN a") as i64;
    assert_eq!(
        scalaire(&db, "MATCH (a)-[r]->() RETURN count(a) AS n"),
        par_lignes,
        "count(a) doit compter les lignes du motif, pas les arêtes"
    );
}

/// Un motif à deux sauts compte des CHEMINS, pas des arêtes.
#[test]
fn deux_sauts_ne_doivent_pas_prendre_le_raccourci() {
    let db = monter();
    let par_lignes = verite_par_lignes(&db, "MATCH ()-[r1]->()-[r2]->() RETURN r2") as i64;
    assert_eq!(
        scalaire(&db, "MATCH ()-[r1]->()-[r2]->() RETURN count(r2) AS n"),
        par_lignes,
        "un motif à deux sauts compte des chemins"
    );
}

// ───────────────────────────────────────────────────────────────────────────
//  LES COMPTEURS DOIVENT SUIVRE LES VARIATIONS, DANS LES DEUX SENS
// ───────────────────────────────────────────────────────────────────────────
//
// Le raccourci NŒUD (`MATCH (n) RETURN count(n)`) existait déjà et répond
// depuis `node_count` / `node_count_by_label` — les mêmes compteurs atomiques
// que ceux des arêtes, avec la même dérive possible : une valeur fausse écrite
// une fois est restaurée de l'instantané et re-persistée à l'identique,
// indéfiniment, sans que rien ne la confronte au disque.
//
// Rien ne vérifiait qu'ils suivent une CRÉATION puis une SUPPRESSION. Un
// compteur figé rendrait l'ancien total avec l'aplomb d'une mesure, et le
// raccourci est justement le seul chemin où le scan ne viendrait pas le
// démentir.

#[test]
fn le_compte_de_noeuds_suit_une_augmentation_puis_une_diminution() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();

    assert_eq!(
        scalaire(&db, "MATCH (n) RETURN count(n) AS n"),
        0,
        "base vide"
    );

    for i in 0..6 {
        s.execute(&format!("INSERT (:N {{id: {i}}})")).unwrap();
        assert_eq!(
            scalaire(&db, "MATCH (n) RETURN count(n) AS n"),
            i + 1,
            "le compte doit suivre CHAQUE création"
        );
    }

    for i in 0..4 {
        s.execute(&format!("MATCH (n:N {{id: {i}}}) DETACH DELETE n"))
            .unwrap();
        assert_eq!(
            scalaire(&db, "MATCH (n) RETURN count(n) AS n"),
            6 - (i + 1),
            "le compte doit suivre CHAQUE suppression"
        );
    }

    // Et il doit toujours s'accorder avec le comptage par lignes, qui ne
    // prend jamais le raccourci.
    assert_eq!(
        scalaire(&db, "MATCH (n) RETURN count(n) AS n"),
        verite_par_lignes(&db, "MATCH (n) RETURN n") as i64
    );
}

/// Même exigence par LABEL, où le compteur est un atomique distinct par bit.
/// C'est celui dont la dérive a été mesurée en production — 1220 annoncés
/// contre 1840 réels.
#[test]
fn le_compte_par_label_suit_les_variations_et_ne_deborde_pas_sur_les_voisins() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..5 {
        s.execute(&format!("INSERT (:Alpha {{id: {i}}})")).unwrap();
    }
    for i in 0..3 {
        s.execute(&format!("INSERT (:Beta {{id: {i}}})")).unwrap();
    }
    assert_eq!(scalaire(&db, "MATCH (n:Alpha) RETURN count(n) AS n"), 5);
    assert_eq!(scalaire(&db, "MATCH (n:Beta) RETURN count(n) AS n"), 3);
    assert_eq!(scalaire(&db, "MATCH (n) RETURN count(n) AS n"), 8);

    s.execute("MATCH (n:Alpha {id: 0}) DETACH DELETE n")
        .unwrap();
    s.execute("MATCH (n:Alpha {id: 1}) DETACH DELETE n")
        .unwrap();

    assert_eq!(
        scalaire(&db, "MATCH (n:Alpha) RETURN count(n) AS n"),
        3,
        "Alpha diminue"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:Beta) RETURN count(n) AS n"),
        3,
        "Beta ne doit PAS bouger — une dérive qui déborde sur le voisin ne se \
         verrait pas sans ce contrôle"
    );
    assert_eq!(scalaire(&db, "MATCH (n) RETURN count(n) AS n"), 6);

    // Réaugmentation : un compteur qui ne sait que décroître existe aussi.
    s.execute("INSERT (:Alpha {id: 99})").unwrap();
    assert_eq!(scalaire(&db, "MATCH (n:Alpha) RETURN count(n) AS n"), 4);
    assert_eq!(
        scalaire(&db, "MATCH (n:Alpha) RETURN count(n) AS n"),
        verite_par_lignes(&db, "MATCH (n:Alpha) RETURN n") as i64,
        "et le raccourci doit toujours égaler le comptage par lignes"
    );
}

/// Supprimer un NŒUD emporte ses arêtes. Les deux familles de compteurs
/// doivent bouger ensemble — c'est le point où une désynchronisation entre
/// `total_live_nodes` et `total_live_edges` deviendrait visible.
#[test]
fn supprimer_un_noeud_fait_baisser_le_compte_d_aretes() {
    let db = monter();
    assert_eq!(scalaire(&db, "MATCH (n) RETURN count(n) AS n"), 5);
    assert_eq!(scalaire(&db, "MATCH ()-[r]->() RETURN count(r) AS n"), 7);

    // Le nœud 2 porte : LIE entrant depuis 1, la boucle LIE, AUTRE entrant
    // depuis 0. Trois arêtes doivent disparaître avec lui.
    db.session()
        .execute("MATCH (n:N {id: 2}) DETACH DELETE n")
        .unwrap();

    assert_eq!(scalaire(&db, "MATCH (n) RETURN count(n) AS n"), 4);
    let restantes = scalaire(&db, "MATCH ()-[r]->() RETURN count(r) AS n");
    assert_eq!(
        restantes,
        verite_par_lignes(&db, "MATCH ()-[r]->() RETURN r") as i64,
        "après DETACH DELETE, le raccourci doit toujours égaler le comptage \
         par lignes — c'est ici qu'un compteur non décrémenté se verrait"
    );
    assert_eq!(restantes, 4, "sept arêtes moins les trois du nœud 2");
}
