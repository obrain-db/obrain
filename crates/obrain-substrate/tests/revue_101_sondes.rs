//! Sondes de la relecture indépendante de la PR #101 (tour 5), reprises comme
//! tests de régression — API publique seulement.
//!
//! Chacune éprouve une famille de la règle « une clé à colonne blob n'est
//! jamais crue depuis la zone persistée » et de la discipline d'écriture
//! (entrée d'index neuve avant la valeur, retrait de l'ancienne après, un
//! écrivain par nœud) : durabilité après kill, drop interrompu, écritures
//! pendant la construction, drop pendant la construction, lecture seule.

use std::sync::Arc;

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::{DeclarationIndex, SubstrateStore};

fn longue_de(court: &str) -> String {
    format!("{}.{}Z", court.trim_end_matches('Z'), "0".repeat(300))
}

fn court(i: i64) -> String {
    format!(
        "2024-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        1 + i % 12,
        1 + i % 28,
        i % 24,
        i % 60,
        (i * 7) % 60
    )
}

fn idx_of(v: &[(NodeId, String)], slot: u64) -> usize {
    v.iter().position(|(n, _)| n.0 == slot).unwrap()
}

/// S1 — durabilité : blob (> 256 o) et courts sur la même clé, construction,
/// flush, puis réécritures post-flush (court→blob, blob→court, blob→blob,
/// nouveaux nœuds courts), kill, réouverture : chaque nœud est trouvé par la
/// forme canonique de sa valeur courante, et jamais par son ancienne.
#[test]
fn s1_durabilite_blob_et_court_meme_cle_apres_kill() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path().join("kb");
    let mut attendu: Vec<(NodeId, String)> = Vec::new();
    let mut fantomes: Vec<(NodeId, String)> = Vec::new();
    let attendu_x = (0..3000i64)
        .filter(|i| !(i % 11 == 0 && i % 7 != 0))
        .count()
        + 50;
    {
        let s = SubstrateStore::create(&racine).unwrap();
        for i in 0..3000i64 {
            let n = s.create_node(&["Evt"]);
            let c = court(i);
            if i % 7 == 0 {
                s.set_node_property(n, "d", Value::from(longue_de(&c).as_str()));
            } else if i % 11 == 0 {
                s.set_node_property(
                    n,
                    "d",
                    Value::from(format!("texte-{i}-{}", "x".repeat(300)).as_str()),
                );
                continue;
            } else {
                s.set_node_property(n, "d", Value::from(c.as_str()));
            }
            attendu.push((n, c));
            if i % 2 == 0 {
                s.set_node_property(n, "x", Value::Int64(5));
            } else {
                s.set_node_property(n, "x", Value::Float64(5.0));
            }
        }
        s.create_property_index("d");
        s.create_property_index("x");
        s.flush().unwrap();
        let (n0, c0) = attendu[1].clone();
        let c0b = "2030-01-01T00:00:00Z".to_string();
        s.set_node_property(n0, "d", Value::from(longue_de(&c0b).as_str()));
        attendu[1] = (n0, c0b);
        fantomes.push((n0, c0));
        let (n1, c1) = attendu[0].clone(); // i = 0 : blob
        let c1b = "2031-01-01T00:00:00Z".to_string();
        s.set_node_property(n1, "d", Value::from(c1b.as_str()));
        attendu[0] = (n1, c1b);
        fantomes.push((n1, c1));
        let idx7 = idx_of(&attendu, 8); // i = 7 : blob
        let (n2, c2) = attendu[idx7].clone();
        let c2b = "2032-01-01T00:00:00Z".to_string();
        s.set_node_property(n2, "d", Value::from(longue_de(&c2b).as_str()));
        attendu[idx7] = (n2, c2b);
        fantomes.push((n2, c2));
        for i in 0..50i64 {
            let n = s.create_node(&["Evt"]);
            let c = format!("2040-01-01T00:00:{i:02}Z");
            s.set_node_property(n, "d", Value::from(c.as_str()));
            s.set_node_property(n, "x", Value::Float64(5.0));
            attendu.push((n, c));
        }
        assert_eq!(
            s.find_nodes_by_property("x", &Value::Int64(5)).len(),
            attendu_x,
            "5.0 écrit après la construction, en session"
        );
        s.fsync_wal().unwrap();
        s.deverrouiller_pour_simuler_un_kill();
        std::mem::forget(s);
    }
    let s2 = SubstrateStore::open(&racine).unwrap();
    let mut manquants = Vec::new();
    for (n, c) in &attendu {
        let autre = format!("{}+00:00", c.trim_end_matches('Z'));
        if !s2
            .find_nodes_by_property("d", &Value::from(autre.as_str()))
            .contains(n)
        {
            manquants.push((
                *n,
                c.clone(),
                s2.get_node_property(*n, &PropertyKey::new("d")),
            ));
        }
    }
    assert!(
        manquants.is_empty(),
        "porteurs manquants après kill : {manquants:?}"
    );
    for (n, c) in &fantomes {
        assert!(
            !s2.find_nodes_by_property("d", &Value::from(c.as_str()))
                .contains(n),
            "fantôme {n:?} sous {c}"
        );
    }
    let n5 = s2.find_nodes_by_property("x", &Value::Int64(5)).len();
    let n5f = s2.find_nodes_by_property("x", &Value::Float64(5.0)).len();
    assert_eq!((n5, n5f), (attendu_x, attendu_x));
    let hits = s2.find_nodes_by_property(
        "d",
        &Value::from(longue_de("2032-01-01T00:00:00Z").as_str()),
    );
    assert_eq!(hits, vec![attendu[idx_of(&attendu, 8)].0]);
}

/// S2 — drop d'une clé à blob, écriture, puis kill entre la zone et le
/// catalogue (déclaration restaurée) : reconstruit juste.
#[test]
fn s2_drop_cle_blob_kill_entre_zone_et_catalogue() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path().join("kb");
    let (n, m) = {
        let s = SubstrateStore::create(&racine).unwrap();
        let n = s.create_node(&["R"]);
        s.set_node_property(n, "d", Value::from("2020-01-01T00:00:00Z"));
        let m = s.create_node(&["R"]);
        s.set_node_property(
            m,
            "d",
            Value::from(longue_de("2021-01-01T00:00:00Z").as_str()),
        );
        s.create_property_index("d");
        s.flush().unwrap();
        assert!(s.drop_property_index("d"));
        s.set_node_property(n, "d", Value::from("2022-01-01T00:00:00Z"));
        s.declarer_index(DeclarationIndex {
            genre: "property".into(),
            label: String::new(),
            propriete: "d".into(),
            dimension: None,
            metrique: None,
        })
        .unwrap();
        s.fsync_wal().unwrap();
        s.deverrouiller_pour_simuler_un_kill();
        std::mem::forget(s);
        (n, m)
    };
    let s2 = SubstrateStore::open(&racine).unwrap();
    assert_eq!(
        s2.find_nodes_by_property("d", &Value::from("2022-01-01T00:00:00Z")),
        vec![n]
    );
    assert!(
        s2.find_nodes_by_property("d", &Value::from("2020-01-01T00:00:00Z"))
            .is_empty()
    );
    assert_eq!(
        s2.find_nodes_by_property("d", &Value::from("2021-01-01T00:00:00Z")),
        vec![m]
    );
}

/// S3a — écritures (courtes ET blob) pendant la construction de l'index de
/// la même clé, avec une seconde construction concurrente par requête.
#[test]
fn s3a_ecritures_pendant_construction_scalaire_et_blob() {
    let td = tempfile::tempdir().unwrap();
    let s = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let mut avant: Vec<(NodeId, String)> = Vec::new();
    for i in 0..30_000i64 {
        let n = s.create_node(&["Evt"]);
        let c = court(i);
        if i % 50 == 0 {
            s.set_node_property(n, "d", Value::from(longue_de(&c).as_str()));
        } else {
            s.set_node_property(n, "d", Value::from(c.as_str()));
        }
        avant.push((n, c));
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s_w, stop_w) = (Arc::clone(&s), Arc::clone(&stop));
    let avant_w: Vec<NodeId> = avant.iter().map(|(n, _)| *n).collect();
    let ecrivain = std::thread::spawn(move || {
        let mut ecrits: Vec<(NodeId, String)> = Vec::new();
        let mut i = 0i64;
        while !stop_w.load(std::sync::atomic::Ordering::Acquire) || i < 200 {
            let n = s_w.create_node(&["Evt"]);
            let c = format!(
                "2050-01-01T{:02}:{:02}:{:02}Z",
                (i / 3600) % 24,
                (i / 60) % 60,
                i % 60
            );
            if i % 3 == 0 {
                s_w.set_node_property(n, "d", Value::from(longue_de(&c).as_str()));
            } else {
                s_w.set_node_property(n, "d", Value::from(c.as_str()));
            }
            if i % 5 == 0 {
                let v = avant_w[(i as usize * 37) % avant_w.len()];
                let c2 = format!("2060-01-01T00:{:02}:{:02}Z", (i / 60) % 60, i % 60);
                if i % 10 == 0 {
                    s_w.set_node_property(v, "d", Value::from(longue_de(&c2).as_str()));
                } else {
                    s_w.set_node_property(v, "d", Value::from(c2.as_str()));
                }
                ecrits.push((v, c2));
            }
            ecrits.push((n, c));
            i += 1;
        }
        ecrits
    });
    let s_q = Arc::clone(&s);
    let requeteur = std::thread::spawn(move || {
        for _ in 0..20 {
            let _ = s_q.find_nodes_by_property("d", &Value::from("2024-01-01T00:00:00Z"));
        }
    });
    s.create_property_index("d");
    stop.store(true, std::sync::atomic::Ordering::Release);
    let ecrits = ecrivain.join().unwrap();
    requeteur.join().unwrap();
    assert!(s.has_property_index("d"));
    let mut manquants = 0usize;
    for (n, _) in avant.iter().chain(ecrits.iter()) {
        let Some(Value::String(sv)) = s.get_node_property(*n, &PropertyKey::new("d")) else {
            continue;
        };
        let c = if sv.len() > 256 {
            format!("{}Z", &sv[..19])
        } else {
            sv.to_string()
        };
        if !s
            .find_nodes_by_property("d", &Value::from(c.as_str()))
            .contains(n)
        {
            manquants += 1;
        }
    }
    assert_eq!(manquants, 0, "valeurs courantes absentes de l'index");
}

/// S3b — drop pendant une construction (deux fils), puis recréation : l'état
/// final est cohérent (index présent ⇒ exact ; absent ⇒ balayage exact).
#[test]
fn s3b_drop_pendant_construction() {
    let td = tempfile::tempdir().unwrap();
    let s = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let mut tous: Vec<(NodeId, String)> = Vec::new();
    for i in 0..30_000i64 {
        let n = s.create_node(&["Evt"]);
        let c = court(i);
        if i % 50 == 0 {
            s.set_node_property(n, "d", Value::from(longue_de(&c).as_str()));
        } else {
            s.set_node_property(n, "d", Value::from(c.as_str()));
        }
        tous.push((n, c));
    }
    for tour in 0..4u64 {
        let s_c = Arc::clone(&s);
        let createur = std::thread::spawn(move || s_c.create_property_index("d"));
        let s_d = Arc::clone(&s);
        let dropper = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5 * tour));
            s_d.drop_property_index("d")
        });
        createur.join().unwrap();
        let _ = dropper.join().unwrap();
        for (n, c) in tous.iter().step_by(97) {
            assert!(
                s.find_nodes_by_property("d", &Value::from(c.as_str()))
                    .contains(n),
                "tour {tour} : {n:?} manque"
            );
        }
        s.create_property_index("d");
        assert!(s.has_property_index("d"));
        for (n, c) in tous.iter().step_by(97) {
            assert!(
                s.find_nodes_by_property("d", &Value::from(c.as_str()))
                    .contains(n)
            );
        }
        s.flush().unwrap();
        assert!(s.drop_property_index("d"));
    }
}

/// S5 — coût : la reconstruction d'une clé à blob est payée UNE fois par
/// processus ; les requêtes suivantes ne la refont pas.
#[test]
fn s5_la_reconstruction_d_une_cle_a_blob_est_payee_une_fois_par_processus() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path().join("kb");
    // Au-dessus du seuil de passe hâtive : c'est la PREMIÈRE requête qui bâtit.
    let total = i64::try_from(obrain_substrate::store::SEUIL_NOEUDS_PASSE_HATIVE + 1).unwrap();
    {
        let s = SubstrateStore::create(&racine).unwrap();
        for i in 0..total {
            let n = s.create_node(&["Evt"]);
            let c = court(i);
            if i % 100 == 0 {
                s.set_node_property(n, "d", Value::from(longue_de(&c).as_str()));
            } else {
                s.set_node_property(n, "d", Value::from(c.as_str()));
            }
        }
        s.create_property_index("d");
        s.flush().unwrap();
        s.close().unwrap();
    }
    let s = SubstrateStore::open(&racine).unwrap();
    // COMPTEUR, pas temps mural (fragile sous charge) : le nombre de passes
    // complètes de construction d'index depuis l'ouverture.
    let avant = s.passes_de_construction_index();
    assert!(
        s.find_nodes_by_property("d", &Value::from(court(100).as_str()))
            .contains(&NodeId(101))
    );
    let apres_la_premiere = s.passes_de_construction_index();
    assert_eq!(
        apres_la_premiere - avant,
        1,
        "la première requête sur une clé à blob bâtit l'index : une passe"
    );
    for i in 0..200i64 {
        let _ = s.find_nodes_by_property("d", &Value::from(court(i).as_str()));
    }
    assert_eq!(
        s.passes_de_construction_index(),
        apres_la_premiere,
        "200 requêtes de plus : aucune nouvelle passe de construction"
    );
}
