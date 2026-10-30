//! ── UN APPEL D'OUTIL NE PEUT PAS DÉPASSER SON BUDGET ───────────────────────
//!
//! Défaut mesuré le 2026-09-17 : l'échéance du pont (`OBRAIN_QUERY_TIMEOUT_MS`,
//! 30 s par défaut) n'était armée QUE pour les requêtes GQL. Les appels
//! d'OUTILS — `import_events`, `auto_consolider` → `search_surprise` /
//! `search_conflicts` / `search_relief`, chacune relisant toute la fenêtre CDC —
//! tournaient **hors de toute portée d'annulation** : `cancel::check()` n'y
//! voyait jamais d'échéance, donc un travail parti ne pouvait plus être arrêté.
//! Un `obrain bridge` a été observé brûlant un cœur pendant vingt minutes sans
//! qu'aucune garde ne s'en aperçoive.
//!
//! Ce fichier éprouve la garantie inverse :
//!
//! * une échéance DÉJÀ dépassée coupe la consolidation **avant** le travail, avec
//!   une erreur qui le dit (`QueryCancelled`), au lieu de payer la lecture ;
//! * une échéance inexistante ou large la laisse passer — la garde ne doit pas
//!   refuser un travail légitime.
#![cfg(feature = "cdc")]

use std::time::Duration;
use web_time::Instant;

use obrain_engine::ObrainDB;

/// Une base avec de la matière CDC : sans événements, la lecture serait vide et
/// le test ne prouverait rien (un refus sur une lecture vide serait un faux
/// positif).
fn base_avec_cdc() -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    for i in 0..20 {
        s.execute(&format!("CREATE (:EC {{v: {i}}})"))
            .expect("create");
    }
    db
}

#[test]
fn une_echeance_depassee_coupe_la_consolidation() {
    let db = base_avec_cdc();
    // Échéance DÉJÀ expirée : c'est le cas d'un budget consommé par une étape
    // précédente de la même consolidation.
    let expiree = Instant::now() - Duration::from_secs(1);
    let _portee = obrain_common::utils::cancel::frontiere(Some(false), Some(expiree));

    let err = db
        .search_surprise(0, 5, false)
        .expect_err("une échéance dépassée doit couper AVANT le travail");
    let message = format!("{err:?}");
    assert!(
        message.contains("annul") || message.contains("Cancel") || message.contains("Q007"),
        "l'erreur doit DIRE l'annulation (pas une erreur de lecture) : {message}"
    );
}

#[test]
fn une_echeance_large_laisse_passer_la_consolidation() {
    let db = base_avec_cdc();
    let _portee = obrain_common::utils::cancel::frontiere(
        Some(false),
        Some(Instant::now() + Duration::from_secs(60)),
    );
    // Le contrôle ne doit pas devenir un refus permanent : une échéance large
    // laisse le travail se faire.
    db.search_surprise(0, 5, false)
        .expect("une échéance large ne doit rien refuser");
}

#[test]
fn sans_echeance_ar_m_e_rien_n_est_refuse() {
    let db = base_avec_cdc();
    // Aucune portée posée : le comportement d'avant (aucune échéance) reste
    // intact pour les appelants qui n'en veulent pas (tests, outils hors pont).
    db.search_surprise(0, 5, false)
        .expect("sans échéance, aucune annulation");
    db.search_conflicts(0, 5)
        .expect("sans échéance, aucune annulation");
    db.search_relief(0, 5)
        .expect("sans échéance, aucune annulation");
}

#[test]
fn les_trois_lectures_de_la_consolidation_consultent_l_echeance() {
    let db = base_avec_cdc();
    // Les TROIS, une par une : c'est leur somme que paie une consolidation, et
    // en oublier une suffirait à laisser un appel hors budget.
    for (nom, appel) in [
        ("search_surprise", 0u8),
        ("search_conflicts", 1),
        ("search_relief", 2),
    ] {
        let expiree = Instant::now() - Duration::from_secs(1);
        let _portee = obrain_common::utils::cancel::frontiere(Some(false), Some(expiree));
        let coupe = match appel {
            0 => db.search_surprise(0, 5, false).is_err(),
            1 => db.search_conflicts(0, 5).is_err(),
            _ => db.search_relief(0, 5).is_err(),
        };
        assert!(coupe, "{nom} doit consulter l'échéance de l'appelant");
    }
}
