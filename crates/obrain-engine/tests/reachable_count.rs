#![cfg(feature = "algos")]
//! `obrain.reachable_count` — comptage ensembliste d'extrémités à k sauts :
//! parité avec les requêtes `MATCH … RETURN count(DISTINCT d)` explicites du
//! moteur (formes positionnelle ET map), sur un petit graphe connu
//! (cycle + branche + chaîne détachée + puits).

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

fn e(db: &ObrainDB, a: u64, b: u64) {
    db.create_edge(NodeId(a), NodeId(b), "T");
}

/// Compte DISTINCT des extrémités du motif à `hops` sauts, par MATCH explicite.
fn match_distinct_endpoints(db: &ObrainDB, hops: usize) -> i64 {
    let mut vars: Vec<String> = (0..hops).map(|i| format!("x{i}")).collect();
    vars.push("d".to_string());
    let mut pattern = String::new();
    for (i, v) in vars.iter().enumerate() {
        if i > 0 {
            pattern.push_str("-[:T]->");
        }
        pattern.push('(');
        pattern.push_str(v);
        pattern.push_str(":L)");
    }
    let q = format!("MATCH {pattern} RETURN count(DISTINCT d) AS n");
    let s = db.session();
    let r = s.execute(&q).expect("MATCH count(DISTINCT d)");
    match &r.rows[0][0] {
        Value::Int64(n) => *n,
        other => panic!("count(DISTINCT d) inattendu: {other:?}"),
    }
}

/// Lit (hop_count, distinct_count) depuis les lignes du CALL.
fn rows_of(db: &ObrainDB, call: &str) -> Vec<(i64, i64)> {
    let s = db.session();
    let r = s
        .execute(&format!(
            "{call} YIELD hop_count, distinct_count RETURN hop_count, distinct_count"
        ))
        .expect("CALL reachable_count");
    r.rows
        .iter()
        .map(|row| match (&row[0], &row[1]) {
            (Value::Int64(h), Value::Int64(c)) => (*h, *c),
            other => panic!("ligne inattendue: {other:?}"),
        })
        .collect()
}

fn build_graph() -> ObrainDB {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-reachable");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    // Nœuds 1..=9, tous label L.
    for _ in 0..9 {
        db.create_node(&["L"]);
    }
    // Cycle 1→2→3→1 ; branche 2→4→5→6 (6 = puits) ; chaîne détachée 7→8→9
    // (7 = source pure). 9 n'a aucune arête sortante vers un nœud L de plus.
    e(&db, 1, 2);
    e(&db, 2, 3);
    e(&db, 3, 1);
    e(&db, 2, 4);
    e(&db, 4, 5);
    e(&db, 5, 6);
    e(&db, 7, 8);
    e(&db, 8, 9);
    db
}

#[test]
fn reachable_count_egale_match_explicite_saut_par_saut() {
    let db = build_graph();

    // Forme positionnelle : (from_label, edge_type, min_hops, max_hops).
    let rows = rows_of(&db, "CALL obrain.reachable_count('L', 'T', 1, 3)");
    // Attendus à la main (marches, sommets répétables — voir le module) :
    // F1=8, F2=7, F3=6 (le cycle maintient 1..6 ; 8 sort à F2, 9 à F3).
    assert_eq!(rows, vec![(1, 8), (2, 7), (3, 6)], "formes positionnelles");

    // Parité avec la requête MATCH explicite du moteur, saut par saut.
    for (hop, _count) in &rows {
        let matched = match_distinct_endpoints(&db, *hop as usize);
        assert_eq!(
            matched, *_count as i64,
            "saut {hop} : reachable_count vs MATCH count(DISTINCT d)"
        );
    }
}

#[test]
fn reachable_count_forme_map_et_defauts() {
    let db = build_graph();
    // Forme map avec min_hops = max_hops = 3 (le cas mesuré : un seul saut).
    let rows = rows_of(
        &db,
        "CALL obrain.reachable_count({from_label: 'L', edge_type: 'T', \
         min_hops: 3, max_hops: 3})",
    );
    assert_eq!(rows, vec![(3, 6)]);
    // Défauts : min_hops=1, max_hops=3 → 3 lignes.
    let rows = rows_of(&db, "CALL obrain.reachable_count('L', 'T')");
    assert_eq!(rows.len(), 3);
    // min_hops > max_hops → erreur explicite.
    let s = db.session();
    assert!(
        s.execute("CALL obrain.reachable_count('L', 'T', 5, 3) YIELD hop_count, distinct_count RETURN hop_count")
            .is_err()
    );
}

#[test]
fn reachable_count_marche_sur_les_deux_hop_extremes() {
    let db = build_graph();
    // La procédure méthode moteur est cohérente avec le CALL.
    let engine_rows = db
        .reachable_count("L", "T", 1, 4)
        .expect("reachable_count engine");
    let pairs: Vec<(usize, u64)> = engine_rows
        .iter()
        .map(|r| (r.hop_count, r.distinct_count))
        .collect();
    assert_eq!(pairs, vec![(1, 8), (2, 7), (3, 6), (4, 6)]);
    // Le MATCH à 4 sauts existe aussi (motif a→b→c→e→d) — parité.
    let matched = match_distinct_endpoints(&db, 4);
    assert_eq!(matched, 6);
}

// ===========================================================================
// (2026-09-08) PUSHDOWN PLANNER — chaînes homogènes
// `MATCH (a:L)-[:E]->…->(d:L) RETURN count(DISTINCT d)` réécrites vers le
// noyau `reachable_count` (voir query/planner/lpg/reachable_pushdown.rs).
//
// Les tests ci-dessous vérifient la SÉMANTIQUE des deux côtés :
//   * formes réécrites : le résultat du MATCH agrégé == oracle ÉNUMÉRATIF
//     indépendant (`RETURN DISTINCT d`, qui ne passe par AUCUN agrégat donc
//     jamais par la réécriture) == noyau reachable_count ;
//   * formes REFUSÉES (labels hétérogènes, types mixtes, direction entrante,
//     count sans DISTINCT, k hors 1..=10) : le MATCH suit l'ancien chemin —
//     résultat == oracle énumératif, et CONTRASTÉ avec la valeur que le noyau
//     donnerait (un mauvais déclenchement rendrait un nombre différent).
// ===========================================================================

/// Exécute `q` et rend le premier entier Int64 de la première ligne.
fn scalar_i64(db: &ObrainDB, q: &str) -> i64 {
    let s = db.session();
    let r = s.execute(q).unwrap_or_else(|e| panic!("{q}: {e}"));
    match &r.rows[0][0] {
        Value::Int64(n) => *n,
        other => panic!("attendu Int64, obtenu {other:?} ({q})"),
    }
}

/// Nombre de lignes de `q` (oracle énumératif).
fn row_count(db: &ObrainDB, q: &str) -> i64 {
    let s = db.session();
    let r = s.execute(q).unwrap_or_else(|e| panic!("{q}: {e}"));
    r.rows.len() as i64
}

/// Crée un nœud au label donné, rend son id substrate (1, 2, 3, …).
fn node(db: &ObrainDB, label: &str) -> u64 {
    db.create_node(&[label]).0
}

fn edge_typed(db: &ObrainDB, a: u64, b: u64, t: &str) {
    db.create_edge(NodeId(a), NodeId(b), t);
}

/// Motif `(x0:L)-[:T]->…->(xk:L)` (k sauts), tous nœuds `:L`.
fn homogeneous_pattern(k: usize, label: &str, edge_type: &str) -> String {
    let mut vars: Vec<String> = (0..=k).map(|i| format!("x{i}")).collect();
    if vars.len() > 1 {
        vars[k] = "d".to_string();
    }
    let mut pattern = String::new();
    for (i, v) in vars.iter().enumerate() {
        if i > 0 {
            pattern.push_str(&format!("-[:{edge_type}]->"));
        }
        pattern.push_str(&format!("({v}:{label})"));
    }
    pattern
}

/// Oracle énumératif : `MATCH <pattern> RETURN DISTINCT d`, compté en lignes
/// (jamais d'agrégat ⇒ jamais réécrit ⇒ sémantique pré-pushdown).
fn oracle_distinct_rows(db: &ObrainDB, pattern: &str) -> i64 {
    row_count(db, &format!("MATCH {pattern} RETURN DISTINCT d"))
}

/// Ouvre une base substrate ÉPHÉMÈRE ; le TempDir (qui porte les fichiers du
/// store) doit être gardé vivant tant que la base écrit : sans lui, les
/// appends WAL tombent sur un répertoire disparu.
fn fresh_db() -> (ObrainDB, tempfile::TempDir) {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("pushdown")).expect("open_substrate");
    (db, td)
}

/// (a) Formes RÉÉCRITES : k=1..4, cycle + branches + chaîne — le MATCH
/// agrégé (chemin pushdown) rend la même chose que l'énumération DISTINCT
/// (chemin historique) et que le noyau, saut par saut.
#[test]
fn pushdown_chain_homogene_egale_oracle_et_noyau() {
    let db = build_graph();
    for k in 1..=4usize {
        let pattern = homogeneous_pattern(k, "L", "T");
        let matched = scalar_i64(
            &db,
            &format!("MATCH {pattern} RETURN count(DISTINCT d) AS n"),
        );
        let oracle = oracle_distinct_rows(&db, &pattern);
        let kernel =
            db.reachable_count("L", "T", k, k).expect("reachable_count")[0].distinct_count as i64;
        assert_eq!(matched, oracle, "k={k}: pushdown vs énumération");
        assert_eq!(matched, kernel, "k={k}: pushdown vs noyau");
        // Valeurs connues du graphe : 8, 7, 6, 6.
        let connu: i64 = [8, 7, 6, 6][k - 1];
        assert_eq!(matched, connu, "k={k}: valeur connue");
    }
}

/// (b1) REFUSÉ — labels hétérogènes `(a:L)-[:T]->(b:M)-[:T]->(c:M)` :
/// le MATCH suit l'ancien chemin (1 extrémité distincte `{c=M}`), alors que
/// le noyau depuis L rendrait 0 (b:M filtré dès le 1er saut).
#[test]
fn pushdown_refuse_labels_heterogenes() {
    let (db, _td) = fresh_db();
    let a = node(&db, "L");
    let b = node(&db, "M");
    let c = node(&db, "M");
    edge_typed(&db, a, b, "T");
    edge_typed(&db, b, c, "T");

    let q = "MATCH (a:L)-[:T]->(b:M)-[:T]->(c:M) RETURN count(DISTINCT c) AS n";
    assert_eq!(scalar_i64(&db, q), 1, "ancien chemin : {c} seul");
    assert_eq!(
        row_count(&db, "MATCH (a:L)-[:T]->(b:M)-[:T]->(c:M) RETURN DISTINCT c"),
        1,
        "oracle énumératif"
    );
    // Contraste : le noyau depuis L ne voit rien (preuve que la forme n'a pas
    // été réécrite — un faux déclenchement rendrait 0).
    let kernel = db.reachable_count("L", "T", 2, 2).expect("kernel")[0].distinct_count;
    assert_eq!(
        kernel, 0,
        "le noyau L→L donnerait 0 : la requête doit rester sur l'ancien chemin"
    );
}

/// (b2) REFUSÉ — deux types d'arêtes `-[:T]->…-[:U]->` : ancien chemin (1),
/// le noyau T-seul donnerait 0.
#[test]
fn pushdown_refuse_types_d_aretes_mixtes() {
    let (db, _td) = fresh_db();
    let a = node(&db, "L");
    let b = node(&db, "L");
    let c = node(&db, "L");
    edge_typed(&db, a, b, "T");
    edge_typed(&db, b, c, "U");

    let q = "MATCH (a:L)-[:T]->(b:L)-[:U]->(c:L) RETURN count(DISTINCT c) AS n";
    assert_eq!(scalar_i64(&db, q), 1, "ancien chemin : {c} seul");
    assert_eq!(
        row_count(&db, "MATCH (a:L)-[:T]->(b:L)-[:U]->(c:L) RETURN DISTINCT c"),
        1
    );
    let kernel = db.reachable_count("L", "T", 2, 2).expect("kernel")[0].distinct_count;
    assert_eq!(kernel, 0, "le noyau T-seul donnerait 0 : pas de réécriture");
}

/// (b3) REFUSÉ — direction ENTRANTE `(a:L)<-[:T]-(b:L)<-[:T]-(c:L)` : ancien
/// chemin (extrémités = {c=1} → 1), le noyau sortant donnerait 2.
#[test]
fn pushdown_refuse_direction_entrante() {
    let (db, _td) = fresh_db();
    // 1→2, 2→3, 2→4 (4 nœuds L).
    for _ in 0..4 {
        node(&db, "L");
    }
    edge_typed(&db, 1, 2, "T");
    edge_typed(&db, 2, 3, "T");
    edge_typed(&db, 2, 4, "T");

    let q = "MATCH (a:L)<-[:T]-(b:L)<-[:T]-(c:L) RETURN count(DISTINCT c) AS n";
    assert_eq!(
        scalar_i64(&db, q),
        1,
        "ancien chemin : seul 1 a une marche sortante de 2 sauts"
    );
    assert_eq!(
        row_count(&db, "MATCH (a:L)<-[:T]-(b:L)<-[:T]-(c:L) RETURN DISTINCT c"),
        1
    );
    let kernel = db.reachable_count("L", "T", 2, 2).expect("kernel")[0].distinct_count;
    assert_eq!(
        kernel, 2,
        "le noyau sortant donnerait 2 : pas de réécriture"
    );
}

/// (b4) REFUSÉ — count SANS DISTINCT : nombre de MARCHES (2), pas
/// d'extrémités distinctes (1). k=0 (nœud seul) et k=11 (hors borne 1..=10)
/// restent aussi sur l'ancien chemin.
#[test]
fn pushdown_refuse_count_sans_distinct_et_k_hors_borne() {
    // 1→2→3 et 4→2 : deux marches de 2 sauts, une seule extrémité distincte.
    let (db, _td) = fresh_db();
    for _ in 0..4 {
        node(&db, "L");
    }
    edge_typed(&db, 1, 2, "T");
    edge_typed(&db, 2, 3, "T");
    edge_typed(&db, 4, 2, "T");

    let pat2 = "(a:L)-[:T]->(b:L)-[:T]->(c:L)";
    assert_eq!(
        scalar_i64(&db, &format!("MATCH {pat2} RETURN count(c) AS n")),
        2,
        "count sans DISTINCT = marches"
    );
    assert_eq!(
        row_count(&db, &format!("MATCH {pat2} RETURN c")),
        2,
        "oracle : lignes énumérées"
    );
    // Le DISTINCT correspondant, lui, est réécrit → 1 (dédoublonné).
    assert_eq!(
        scalar_i64(&db, &format!("MATCH {pat2} RETURN count(DISTINCT c) AS n")),
        1
    );

    // k = 0 : (a:L) seul → count(DISTINCT a) = nombre de nœuds L (9 du graphe
    // connu), jamais une chaîne vide.
    let db9 = build_graph();
    assert_eq!(
        scalar_i64(&db9, "MATCH (a:L) RETURN count(DISTINCT a) AS n"),
        9
    );

    // k = 11 (> REACHABLE_MAX_HOPS) : refus → chemin générique ; le MATCH
    // agrégé doit s'accorder avec l'énumération DISTINCT.
    let pat11 = homogeneous_pattern(11, "L", "T");
    assert_eq!(
        scalar_i64(
            &db9,
            &format!("MATCH {pat11} RETURN count(DISTINCT d) AS n")
        ),
        oracle_distinct_rows(&db9, &pat11),
        "k=11 : pas de réécriture, résultat de l'ancien chemin"
    );
}
