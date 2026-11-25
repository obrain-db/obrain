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

/// Taille du fichier WAL — ce que l'ANCIEN oracle compare à la frontière durable.
fn taille_wal(base: &std::path::Path) -> u64 {
    // Pas de `unwrap_or(0)` : une lecture impossible doit se voir, pas devenir « zéro ».
    std::fs::metadata(base.join("substrate.wal"))
        .expect("le WAL doit exister et être lisible")
        .len()
}

/// **Le scénario discriminant** : commit applicatif durable, puis écritures cognitives
/// concurrentes. Ancien oracle et nouvel oracle jugent LA MÊME exécution.
///
/// Ce qui déclenche RÉELLEMENT des écritures cognitives : la flotte de thinkers
/// (`open_substrate_with_thinkers`). `open_substrate_observed` seul n'écrit rien — mesuré.
// MESURÉ : dans cette configuration (create_node/create_edge + flotte standard), AUCUNE
// écriture cognitive n'atteint le WAL — ni avec `open_substrate_observed`, ni avec
// `open_substrate_with_thinkers` et 5 s d'attente (`taille finale == fin du marqueur`).
// Le scénario « commit durable puis écriture cognitive non synchronisée » n'est donc PAS
// reproduit ici ; le test échoue volontairement plutôt que de faire semblant. Il reste
// `#[ignore]` tant que le déclencheur réel des écritures cognitives n'est pas établi.
#[ignore = "aucune écriture cognitive observée dans cette configuration : scénario non établi"]
#[test]
fn d_ancien_oracle_contre_nouveau_sur_la_meme_execution() {
    let d = dir("d");
    let db = ObrainDB::open_substrate_with_thinkers(
        &d,
        obrain_cognitive::thinker::ThinkersConfig::default(),
    )
    .expect("ouverture avec thinkers");
    let mut s = db.session();
    let mut ancien_signalerait = 0;
    let mut nouvel_signalerait = 0;
    let mut cognition_presente_a_la_mesure = 0;
    let mut mesures = Vec::new();

    for _ in 0..10 {
        s.begin_transaction().expect("begin");
        let a = s.create_node(&["T"]);
        let b = s.create_node(&["T"]);
        s.create_edge(a, b, "LIEN");
        s.commit().expect("commit");

        let fin = db
            .substrate_handle()
            .and_then(|x| x.frontiere_du_commit_du_fil())
            .expect("le marqueur du commit doit être publié");
        let durable = db.wal_octets_durables().expect("base persistante");
        let total = taille_wal(&d);

        if fin > durable {
            nouvel_signalerait += 1;
        }
        if durable < total {
            ancien_signalerait += 1;
        }
        if total > fin {
            cognition_presente_a_la_mesure += 1;
        }
        mesures.push((fin, durable, total));
    }

    // On laisse la flotte de thinkers travailler, en attendant un VRAI signe d'écriture
    // cognitive (le WAL dépasse le dernier marqueur applicatif) — pas un délai arbitraire.
    let fin_dernier = mesures.last().map(|m| m.0).unwrap_or(0);
    let mut cognition_apres_coup = 0;
    for _ in 0..50 {
        if taille_wal(&d) > fin_dernier {
            cognition_apres_coup = 1;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let total_final = taille_wal(&d);
    db.close().expect("close");

    println!(
        "10 commits — cognition présente à la mesure : {cognition_presente_a_la_mesure}/10 ; \
         ancien oracle signalerait : {ancien_signalerait} ; nouvel oracle signalerait : {nouvel_signalerait} ; \
         écriture cognitive après le dernier marqueur : {cognition_apres_coup} (taille finale {total_final} > fin {fin_dernier})"
    );

    assert!(
        cognition_apres_coup == 1,
        "les thinkers n'ont rien écrit dans le WAL : le scénario n'a toujours pas eu lieu"
    );
    assert_eq!(
        nouvel_signalerait, 0,
        "le nouvel oracle signale {nouvel_signalerait} commit(s) acquitté(s) non couvert(s)"
    );
    assert!(
        ancien_signalerait + cognition_apres_coup > 0,
        "l'ancien oracle devrait signaler une violation dès que la cognition écrit après le marqueur"
    );

    let _ = std::fs::remove_dir_all(&d);
}


/// **Contrôle minimal de l'oracle, sans cognition** : un écrivain secondaire emprunte le
/// vrai chemin WAL (`Writer::reinforce_energy`, `flags: 0`) après un commit applicatif
/// durable. On établit `C <= D` puis on MESURE si `D < T` — sans présumer que `flags: 0`
/// implique l'absence de synchronisation.
#[test]
fn e_ecrivain_secondaire_controle_apres_commit_durable() {
    let d = dir("e");
    let db = ObrainDB::open_substrate(&d).expect("ouverture");
    let mut s = db.session();
    s.begin_transaction().expect("begin");
    s.create_node(&["T"]);
    s.commit().expect("commit");

    let c = db
        .substrate_handle()
        .and_then(|x| x.frontiere_du_commit_du_fil())
        .expect("marqueur du commit publié");
    let d_apres_ack = db.wal_octets_durables().expect("frontière durable");
    let t_apres_ack = taille_wal(&d);
    assert!(
        c <= d_apres_ack,
        "le commit applicatif doit être durable à son acquittement : C={c} > D={d_apres_ack}"
    );

    // Écriture secondaire par le VRAI writer du même store, même WAL.
    let sub = db.substrate_handle().expect("substrate");
    sub.writer()
        .reinforce_energy(0, 1000)
        .expect("écriture secondaire d'énergie");

    let t_final = taille_wal(&d);
    let d_final = db.wal_octets_durables().expect("frontière durable après écriture secondaire");
    db.close().expect("close");
    let _ = std::fs::remove_dir_all(&d);

    println!(
        "commit acquitté : C={c} D={d_apres_ack} T={t_apres_ack} | après écriture secondaire : \
         T'={t_final} D'={d_final} | ancien oracle signalerait : {}",
        if d_final < t_final { "OUI (D' < T')" } else { "non (D' >= T')" }
    );

    assert!(t_final > t_apres_ack, "l'écriture secondaire doit étendre le WAL");
    assert!(
        c <= d_apres_ack,
        "l'oracle corrigé doit rester satisfait (C={c} <= D={d_apres_ack})"
    );
}
