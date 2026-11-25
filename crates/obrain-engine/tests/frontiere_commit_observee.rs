#![cfg(feature = "cognitive")]
//! **Contrôles discriminants de la frontière de commit** (banc du validateur this-harness).
//!
//! Question : à l'acquittement, la frame du commit est-elle couverte par la frontière
//! durable, même quand un observateur cognitif écrit dans le même WAL ?
//!
//! L'ancien oracle comparait `wal_octets_durables()` à `taille_wal()` — invalide dès qu'un
//! second écrivain ajoute des frames. Ici on compare à la **fin de la frame de CE commit**,
//! publiée par `seal_commit` depuis le `(position, longueur)` d'`append_encoded`.
//!
//! Trois contrôles :
//!   A. commit durable + écritures cognitives concurrentes → l'oracle doit **réussir** ;
//!   B. marqueur de commit écrit mais **non fsyncé** (`SyncMode::Never`) → l'oracle doit
//!      **échouer** (le contrôle négatif est le point : un oracle qui ne sait pas dire
//!      « non » ne prouve rien) ;
//!   C. deux fils, chacun son commit : chaque valeur de frontière appartient à **son**
//!      marqueur (aucune contamination croisée).

use obrain_engine::ObrainDB;

fn dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "obrain_frontiere_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Vérifie, pour un commit donné, que sa frame est couverte par la frontière durable.
fn verifier(fin: Option<u64>, durable: u64, etiquette: &str) -> bool {
    match fin {
        Some(fin) => {
            let couvert = fin <= durable;
            if !couvert {
                eprintln!("VIOLATION {etiquette} : fin_commit={fin} > durable={durable}");
            }
            couvert
        }
        None => {
            eprintln!("VIOLATION {etiquette} : aucun marqueur de commit enregistré");
            false
        }
    }
}

#[test]
fn a_commit_durable_avec_cognition_concurrente_doit_reussir() {
    let d = dir("a");
    let db = ObrainDB::open_substrate_observed(&d).expect("ouverture observée");
    let mut s = db.session();
    let mut violations = 0;
    for i in 0..20 {
        s.begin_transaction().expect("begin");
        s.create_node(&["T"]);
        s.commit().expect("commit");
        let fin = db.substrate_handle().and_then(|x| x.frontiere_du_commit_du_fil());
        let durable = db.wal_octets_durables().unwrap_or(0);
        if !verifier(fin, durable, &format!("A k={i}")) {
            violations += 1;
        }
    }
    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(
        violations, 0,
        "en mode durable (EveryCommit), chaque commit acquitté doit être couvert"
    );
}

#[test]
fn b_marqueur_non_fsynce_doit_etre_detecte() {
    let d = dir("b");
    // `SyncMode::Never` : la frame du commit est ÉCRITE (marqueur présent, octets présents)
    // mais aucun fsync n'est émis — c'est l'acquittement prématuré, sans toucher au format.
    let db = ObrainDB::open_substrate_with_mode(&d, obrain_substrate::wal_io::SyncMode::Never)
        .expect("ouverture SyncMode::Never");
    let mut s = db.session();
    s.begin_transaction().expect("begin");
    s.create_node(&["T"]);
    s.commit().expect("commit");
    let fin = db.substrate_handle().and_then(|x| x.frontiere_du_commit_du_fil());
    let durable = db.wal_octets_durables().unwrap_or(0);
    let couvert = verifier(fin, durable, "B");
    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&d);
    assert!(
        fin.is_some(),
        "le marqueur de commit doit EXISTER (sinon on ne teste pas la durabilité)"
    );
    assert!(
        !couvert && fin.unwrap() > durable,
        "un commit écrit mais non fsyncé DOIT être vu comme non couvert : fin={fin:?} durable={durable}"
    );
}

#[test]
fn c_deux_fils_evaluent_chacun_son_propre_marqueur() {
    let mut poignees = Vec::new();
    for i in 0..2 {
        poignees.push(std::thread::spawn(move || {
            let d = dir(&format!("c{i}"));
            let db = ObrainDB::open_substrate_observed(&d).expect("ouverture observée");
            let mut s = db.session();
            let mut resultats = Vec::new();
            for _ in 0..5 {
                s.begin_transaction().expect("begin");
                s.create_node(&["T"]);
                s.commit().expect("commit");
                let fin = db
                    .substrate_handle()
                    .and_then(|x| x.frontiere_du_commit_du_fil())
                    .expect("marqueur présent");
                let durable = db.wal_octets_durables().unwrap_or(0);
                assert!(
                    fin <= durable,
                    "fil {i} : fin={fin} > durable={durable}"
                );
                resultats.push(fin);
            }
            db.close().expect("close");
            let _ = std::fs::remove_dir_all(&d);
            resultats
        }));
    }
    for p in poignees {
        let r = p.join().expect("fil");
        assert!(r.windows(2).all(|w| w[0] <= w[1]), "frontières croissantes");
        assert!(r.iter().all(|v| *v > 0));
    }
}
