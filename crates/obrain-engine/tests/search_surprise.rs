#![cfg(all(feature = "cdc", feature = "algos"))]
//! `obrain.search_surprise` — « ce qui s'active contre les attentes » :
//! (1) méthode `ObrainDB::search_surprise` ; (2) procédure CALL ; (3) tri
//! signé/absolu et limite ; (4) lecture pure (read-only).
//!
//! Scénario synthétique : des hubs A et C ont un degré de DÉBUT de fenêtre
//! élevé (arêtes posées AVANT `since_ms`) ; dans la fenêtre, A reçoit une
//! rafale d'arêtes (activité ≫ attente → surprise positive) et C une seule
//! (activité ≪ attente → surprise négative) ; les nœuds feuilles ont une
//! activité anecdotique (~zéro de surprise).

use std::time::Duration;
use web_time::SystemTime;

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("horloge")
        .as_millis() as u64
}

/// Monte le graphe : A (degré initial FAIBLE : 2 arêtes), C (degré initial
/// ÉLEVÉ : 60), 40 feuilles (degré initial 1). La fenêtre porte une rafale
/// d'activité sur A (40 arêtes entrantes) — A s'active ≫ son degré ne le
/// prédit (surprise POSITIVE maximale) ; C ne reçoit qu'une arête (≪ son
/// degré → surprise NÉGATIVE) ; les feuilles ~1 arête (~zéro de surprise).
fn setup_graph(db: &ObrainDB) -> (NodeId, NodeId, Vec<NodeId>) {
    let a = db.create_node(&["L"]);
    let c = db.create_node(&["L"]);
    // Cibles de base pour les degrés initiaux.
    let mut dummies: Vec<NodeId> = Vec::new();
    for _ in 0..64 {
        dummies.push(db.create_node(&["L"]));
    }
    // A : 2 arêtes sortantes. C : 60.
    db.create_edge(a, dummies[0], "T");
    db.create_edge(a, dummies[1], "T");
    for i in 0..60 {
        db.create_edge(c, dummies[i], "T");
    }
    // 40 feuilles, degré initial 1 chacune.
    let leaves: Vec<NodeId> = (0..40).map(|_| db.create_node(&["L"])).collect();
    for (i, leaf) in leaves.iter().enumerate() {
        db.create_edge(*leaf, dummies[2 + (i % 62)], "T");
    }
    (a, c, leaves)
}

#[test]
fn search_surprise_rank_les_ecarts_d_activite() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-surprise");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let (a, c, leaves) = setup_graph(&db);
    // La fenêtre commence APRÈS la pose des arêtes de base.
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    // Rafale : 40 arêtes feuille → A (A = cible 40×) + 1 arête A → C.
    for leaf in &leaves {
        db.create_edge(*leaf, a, "T");
    }
    db.create_edge(a, c, "T");

    let rows = db
        .search_surprise(since, 100, false)
        .expect("search_surprise");
    assert!(!rows.is_empty(), "fenêtre d'activité non vide");
    let ra = rows.iter().find(|r| r.node_id == a.0).expect("A actif");
    let rc = rows.iter().find(|r| r.node_id == c.0).expect("C actif");
    assert!(ra.activity > 0, "A a reçu la rafale");
    assert!(rc.activity >= 1);
    // A : activé bien plus que son degré initial ne le prédit (positif max) ;
    // C : bien moins (négatif marqué).
    assert!(
        ra.surprise > 5.0,
        "A surprise positive forte, got {}",
        ra.surprise
    );
    assert!(
        rc.surprise < -2.0,
        "C surprise négative, got {}",
        rc.surprise
    );
    // Tri par |surprise| : A (positif max) domine.
    assert_eq!(
        rows[0].node_id, a.0,
        "A = max |surprise| (positif), got {} / {:?}",
        rows[0].node_id, rows[0]
    );
    // Les feuilles (activité ~1, degré initial 1) ont une surprise ~nulle.
    for leaf in &leaves {
        if let Some(rl) = rows.iter().find(|r| r.node_id == leaf.0) {
            assert!(
                rl.surprise.abs() < 1.0,
                "feuille ~neutre, got {}",
                rl.surprise
            );
        }
    }
    // limit respecté + tri signé (décroissant → A en tête, positifs d'abord).
    let top1 = db.search_surprise(since, 1, false).expect("limit");
    assert_eq!(top1.len(), 1, "limit appliquée");
    assert_eq!(top1[0].node_id, a.0);
    let signed_rows = db.search_surprise(since, 100, true).expect("signé");
    assert_eq!(
        signed_rows[0].node_id, a.0,
        "tri signé : A (positif max) en tête"
    );
    let mut prev = f64::INFINITY;
    for r in &signed_rows {
        assert!(r.surprise <= prev, "tri signé décroissant");
        prev = r.surprise;
    }
}

#[test]
fn call_search_surprise_expose_le_classement() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-surprise-call");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let (a, _c, pool) = setup_graph(&db);
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    for i in 0..30 {
        db.create_edge(a, pool[i % 64], "T");
    }
    let s = db.session();
    let r = s
        .execute(&format!("CALL obrain.search_surprise({since}, 10, false) YIELD node_id, activity RETURN node_id, activity"))
        .expect("CALL search_surprise");
    assert!(!r.rows.is_empty(), "CALL rend des lignes");
    // Colonnes attendues.
    let col = &r.columns[0];
    assert_eq!(col, "node_id", "colonne node_id, got {col}");
    for row in r.rows.iter().take(5) {
        assert!(
            matches!(row[0], Value::Int64(_)),
            "node_id Int64: {:?}",
            row[0]
        );
    }
}

#[test]
fn search_surprise_est_une_lecture_pure() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-surprise-ro");
    {
        let w = ObrainDB::open_substrate(&path).expect("open rw");
        let (a, _c, pool) = setup_graph(&w);
        std::thread::sleep(Duration::from_millis(3));
        let since = now_ms();
        for i in 0..10 {
            w.create_edge(a, pool[i % 64], "T");
        }
        let rows = w.search_surprise(since, 50, false).expect("rw ok");
        assert_ne!(rows.len(), 0);
    }
    let db_ro = ObrainDB::open_read_only(&path).expect("open ro");
    let since = 0u64; // recharge la fenêtre persistée (aucune mutation possible)
    let rows = db_ro
        .search_surprise(since, 50, false)
        .expect("search_surprise en read-only");
    assert!(!rows.is_empty(), "read-only doit rendre le classement");
    let s = db_ro.session();
    let r = s
        .execute("CALL obrain.search_surprise(0, 5, false) YIELD node_id RETURN node_id")
        .expect("CALL en read-only");
    assert!(!r.rows.is_empty(), "CALL read-only rend des lignes");
}

/// PROPRIÉTÉ : oracle indépendant — topologie aléatoire (nœuds pré-créés,
/// rafale d'arêtes dans la fenêtre), oracle calculé depuis les SEULES
/// écritures du test (activité par extrémité, degré de début de fenêtre =
/// degré courant − arêtes de la fenêtre). Le moteur doit rendre les mêmes
/// (activité, expected, surprise) pour chaque nœud actif.
#[test]
fn search_surprise_concorde_avec_l_oracle() {
    use std::time::Duration;
    use web_time::SystemTime;
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-surp-prop");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let sub = db.substrate_handle().expect("substrate");
    let deg = |n: u64| {
        sub.out_degree(obrain_common::types::NodeId(n)) as u64
            + sub.in_degree(obrain_common::types::NodeId(n)) as u64
    };

    // RNG déterministe.
    let mut st: u64 = 0xABCD_EF01_2345_6789;
    let mut rnd = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        st
    };

    for graine in 0..10u64 {
        // Nœuds pré-créés + arêtes de « ligne de base » (AVANT la fenêtre).
        let n: u64 = 12 + (rnd() % 12);
        let nodes: Vec<u64> = (0..n).map(|_| db.create_node(&["L"]).0).collect();
        let mut prior: Vec<(usize, usize)> = Vec::new();
        for _ in 0..(3 + rnd() % 8) {
            let a = (rnd() % n) as usize;
            let b = (rnd() % n) as usize;
            if a != b {
                prior.push((a, b));
                db.create_edge(
                    obrain_common::types::NodeId(nodes[a]),
                    obrain_common::types::NodeId(nodes[b]),
                    "T",
                );
            }
        }
        std::thread::sleep(Duration::from_millis(3));
        let since = now_ms();
        // Rafale dans la fenêtre.
        let mut act: Vec<u64> = vec![0; n as usize];
        let mut burst: Vec<(usize, usize)> = Vec::new();
        let m = 3 + rnd() % 10;
        for _ in 0..m {
            let a = (rnd() % n) as usize;
            let b = (rnd() % n) as usize;
            if a != b {
                burst.push((a, b));
                db.create_edge(
                    obrain_common::types::NodeId(nodes[a]),
                    obrain_common::types::NodeId(nodes[b]),
                    "T",
                );
                act[a] += 1;
                act[b] += 1;
            }
        }
        if burst.is_empty() {
            continue;
        }
        // Oracle (indépendant) : degré de début = degré courant − arêtes de la
        // fenêtre dont le nœud est une extrémité.
        let total: f64 = act.iter().map(|c| *c as f64).sum();
        let mut start: Vec<u64> = vec![0; n as usize];
        for i in 0..n as usize {
            let mut outw = 0u64;
            let mut inw = 0u64;
            for (a, b) in &burst {
                if *a == i {
                    outw += 1;
                }
                if *b == i {
                    inw += 1;
                }
            }
            start[i] = deg(nodes[i]).saturating_sub(outw + inw);
        }
        // Le modèle du moteur somme sur les nœuds ACTIFS uniquement.
        let denom: f64 = (0..n as usize)
            .filter(|i| act[*i] > 0)
            .map(|i| start[i] as f64 + 1.0)
            .sum();
        let oracle: Vec<(u64, u64, f64, f64)> = (0..n as usize)
            .filter(|i| act[*i] > 0)
            .map(|i| {
                let exp = if denom > 0.0 {
                    total * (start[i] as f64 + 1.0) / denom
                } else {
                    0.0
                };
                let z = (act[i] as f64 - exp) / (exp + 1.0).sqrt();
                (nodes[i], act[i], exp, z)
            })
            .collect();
        let rows = db.search_surprise(since, 10_000, false).expect("surprise");
        // Chaque nœud actif rend la MÊME ligne que l'oracle.
        assert_eq!(rows.len(), oracle.len(), "graine {graine}: nb nœuds actifs");
        for (node, a, e, z) in oracle {
            let r = rows
                .iter()
                .find(|x| x.node_id == node)
                .unwrap_or_else(|| panic!("graine {graine}: nœud {node} absent"));
            assert_eq!(r.activity, a, "graine {graine}: activité {node}");
            assert!(
                (r.expected - e).abs() < 1e-6 && (r.surprise - z).abs() < 1e-6,
                "graine {graine}: nœud {node} exp {e} vs {} ; z {z} vs {}",
                r.expected,
                r.surprise
            );
        }
        // Tri par |surprise| décroissant.
        for w in rows.windows(2) {
            assert!(
                w[0].surprise.abs() >= w[1].surprise.abs(),
                "graine {graine}: tri |surprise|"
            );
        }
    }
}

#[test]
fn search_surprise_sans_activite_rend_vide() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-surprise-vide");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let _a = db.create_node(&["L"]); // activité avant toute fenêtre
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    let rows = db.search_surprise(since, 50, false).expect("vide");
    assert!(rows.is_empty(), "aucune activité dans la fenêtre → vide");
}
