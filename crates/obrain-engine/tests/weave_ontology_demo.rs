#![cfg(all(feature = "cdc", feature = "algos"))]
//! DÉMONSTRATEUR — le « tissage EdgeBasis^* » (ontologie de topologie : claims,
//! preuves, causalité, contradictions, saillance, jumeaux structurels) EST
//! déjà exécutable sur obrain, sans aucune extension moteur : on instancie les
//! motifs dans un store d'essai et on vérifie que les formes de recherche
//! répondent de façon COMPLÉMENTAIRE :
//!
//!   - `search_delta`   : vue « ce qui a changé » (toutes les écritures) ;
//!   - `search_conflicts`: la contradiction A→B→A (le claim qui se dément) ;
//!   - `search_surprise` : la saillance (le nœud qui s'active ≫ son attente) ;
//!   - `search_causes/effects` : la chaîne causale directionnelle ;
//!   - `search_by_role` : les jumeaux structurels (même rôle, contenu différent).
//!
//! Ce test est aussi une régression de COHÉRENCE entre les formes : chaque
//! motif du scénario est détecté par la forme qui lui correspond.

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

fn edge(db: &ObrainDB, a: NodeId, b: NodeId, t: &str) {
    db.create_edge(a, b, t);
}

/// Motifs : claim avec preuves contradictoires + flip A→B→A ; chaîne causale
/// X→Y→Z ; montée en saillance d'un nœud « nouveau hub ».
#[test]
fn tissage_claims_preuves_causalite_saillance() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-weave");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");

    // ── Monde des preuves : claim C, evidence e1/e2 SUPPORTS, e3 CONTRADICTS.
    let c = db.create_node(&["Claim"]);
    let e1 = db.create_node(&["Evidence"]);
    let e2 = db.create_node(&["Evidence"]);
    let e3 = db.create_node(&["Evidence"]);
    db.set_node_property(e1, "source", Value::String("src-A".into()));
    db.set_node_property(e2, "source", Value::String("src-B".into()));
    edge(&db, e1, c, "SUPPORTS");
    edge(&db, e2, c, "SUPPORTS");
    edge(&db, e3, c, "CONTRADICTS");
    // Épistémique explicite (jamais fusionner support/confiance/vérité).
    db.set_node_property(c, "epistemic_status", Value::String("CONTESTED".into()));

    // ── Contradiction temporelle A→B→A sur un attribut du claim.
    db.set_node_property(c, "status", Value::Int64(0));
    for v in [1i64, 2, 1, 2] {
        db.set_node_property(c, "status", Value::Int64(v));
    }

    // ── Chaîne causale X→Y→Z (directionnelle).
    let x = db.create_node(&["Event"]);
    let y = db.create_node(&["Event"]);
    let z = db.create_node(&["Event"]);
    edge(&db, x, y, "CAUSES");
    edge(&db, y, z, "CAUSES");

    // ── Diff (1) : tout ce qui a changé (méthode engine).
    let all = db.search_delta(0, false, false).expect("delta");
    assert!(all.len() >= 10, "écritures nombreuses, got {}", all.len());

    // ── Contradiction (2) : le flip A→B→A… est détecté sur (c, status).
    let rows = db.search_conflicts(0, 100).expect("conflicts");
    assert!(
        rows.iter()
            .any(|r| r.node_id == c.0 && r.property == "status"),
        "reversal A→B→A sur (c,status) détecté: {rows:?}"
    );

    // ── Causalité (4) : effets(X) = Y puis Z ; causes(Z) = Y puis X.
    let fx = db.search_effects(x.0, 10, 3).expect("effects(X)");
    assert_eq!(fx[0].node_id, y.0);
    assert_eq!(fx[1].node_id, z.0);
    let cz = db.search_causes(z.0, 10, 3).expect("causes(Z)");
    assert_eq!(cz[0].node_id, y.0);
    assert_eq!(cz[1].node_id, x.0);

    // ── Saillance (3) : après la pose, une fenêtre propre où un « nouveau
    // hub » (degré initial ~0) reçoit une rafale → top surprise.
    // (feuilles préexistantes à degré initial 1 pour un bruit ~nul)
    let mut leaves: Vec<NodeId> = Vec::new();
    let mut dummies: Vec<NodeId> = Vec::new();
    for _ in 0..8 {
        leaves.push(db.create_node(&["Doc"]));
        dummies.push(db.create_node(&["Doc"]));
    }
    for leaf in &leaves {
        edge(&db, *leaf, dummies[0], "T");
    }
    let hub = db.create_node(&["Doc"]);
    std::thread::sleep(Duration::from_millis(3));
    let since = now_ms();
    for leaf in &leaves {
        edge(&db, *leaf, hub, "T");
    }
    let surprise = db.search_surprise(since, 20, false).expect("surprise");
    assert!(!surprise.is_empty(), "résultat attendu non vide");
    let top = &surprise[0];
    assert_eq!(
        top.node_id, hub.0,
        "le nouveau hub domine la surprise (activity {}, expected {})",
        top.activity, top.expected
    );
    assert!(
        top.surprise > 1.0,
        "surprise positive marquée: {}",
        top.surprise
    );
}

/// Jumeaux structurels : deux étoiles identiques de forme (labels différents)
/// — même rôle ailleurs, contenu différent (search_by_role).
#[test]
fn tissage_jumeaux_structurels_cross_contenu() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb-weave-twins");
    let db = ObrainDB::open_substrate(&path).expect("open_substrate");
    let star = |label: &str| -> Vec<NodeId> {
        let centre = db.create_node(&[label]);
        let leaves: Vec<NodeId> = (0..4).map(|_| db.create_node(&[label])).collect();
        for l in &leaves {
            edge(&db, centre, *l, "T");
        }
        let mut v = leaves;
        v.push(centre);
        v
    };
    let s1 = star("StarA");
    let s2 = star("StarB");
    // Une chaîne (forme différente) pour le contraste.
    let ch0 = db.create_node(&["Chain"]);
    let ch1 = db.create_node(&["Chain"]);
    let ch2 = db.create_node(&["Chain"]);
    edge(&db, ch0, ch1, "T");
    edge(&db, ch1, ch2, "T");

    let hub1 = s1.last().expect("centre A");
    let rows = db.search_by_role(hub1.0, 100, None).expect("by_role");
    // Un centre de l'étoile B (même rôle, autre contenu) figure avec une
    // distance faible ; l'extrémité de chaîne est PLUS LOIN.
    let twin = rows
        .iter()
        .find(|r| r.node_id == s2.last().expect("centre B").0)
        .expect("jumeau B présent");
    assert!(twin.distance < 0.35, "jumeau proche: {}", twin.distance);
    if let Some(ch_row) = rows.iter().find(|r| r.node_id == ch0.0) {
        assert!(
            twin.distance + 0.02 < ch_row.distance,
            "jumeau ({}) nettement plus proche que la chaîne ({})",
            twin.distance,
            ch_row.distance
        );
    }
    let _ = (s1, s2, ch1, ch2);
}
