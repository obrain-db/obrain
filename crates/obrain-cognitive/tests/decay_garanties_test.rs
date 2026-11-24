//! Points 2, 3 et 4 de la revue : horloge monotone, demi-vie figée, et
//! migration des bases sans ancre.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{DecayLaw, unix_micros};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const H24: Duration = Duration::from_secs(24 * 3600);
const H1: Duration = Duration::from_secs(3600);

fn h24_micros() -> i64 {
    H24.as_micros() as i64
}

/// Point 2 — la garde monotone survit à la fermeture/réouverture.
///
/// Ancre (1, t0), matérialisée à t0 + 24 h (le high-water mark vaut 24 h),
/// base fermée puis rouverte, horloge revenue à t0 + 12 h : la lecture doit
/// donner 0,5 (pas de remontée à 0,7071) parce que t_hi = 24 h est persisté.
#[test]
fn le_high_water_mark_survit_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("mono");

    let t0 = unix_micros();
    let t_24h = t0 + h24_micros();
    let t_12h = t0 + h24_micros() / 2;
    let id;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        id = sub.create_node(&["n"]);
        sub.set_node_energy_f32(id, 1.0).unwrap();
        // Matérialisation à t0 + 24 h : (0,5, 24 h, 24 h, H). t_hi = 24 h.
        decay_clock::persist_anchor_explicit(&sub, id, 0.5, t_24h, H24);
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // Réouverture, horloge revenue à t0 + 12 h.
    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let law = DecayLaw::new(H24);
    let lue = decay_clock::energy_at(&sub, id, &law, t_12h).unwrap();
    println!("MONOTONIE après réouverture : lecture à 12 h = {lue} (attendu 0,5)");
    assert!(
        (lue - 0.5).abs() < 1e-9,
        "recul d'horloge après réouverture : {lue}, attendu 0,5 (pas 0,7071)"
    );
}

/// Point 3 — la demi-vie est FIGÉE dans l'ancre : changer H après coup ne
/// réécrit pas le passé.
#[test]
fn la_demi_vie_est_figee_dans_l_ancre() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("hf");

    let t0 = unix_micros();
    let t_1h = t0 + H1.as_micros() as i64;
    let id;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        id = sub.create_node(&["n"]);
        sub.set_node_energy_f32(id, 1.0).unwrap();
        // Ancre figée H = 1 h, posée une demi-vie dans le passé.
        decay_clock::persist_anchor_explicit(&sub, id, 1.0, t_1h - H1.as_micros() as i64, H1);
        sub.flush().unwrap();
        sub.close().unwrap();
    }

    // Réouverture avec une AUTRE demi-vie (H = 24 h) : la valeur lue doit
    // rester celle de H = 1 h (figée), soit 0,5 — pas 2^(-1/24) ≈ 0,9715.
    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let law_24 = DecayLaw::new(H24);
    let lue = decay_clock::energy_at(&sub, id, &law_24, t_1h).unwrap();
    println!("H FIGÉE : lecture avec H=24 h sur une ancre H=1 h = {lue} (attendu 0,5)");
    assert!(
        (lue - 0.5).abs() < 1e-9,
        "la demi-vie a été recalculée rétroactivement : {lue}"
    );
    assert!(
        (lue - 0.9715).abs() > 0.01,
        "H=24 h n'aurait pas dû s'appliquer : {lue}"
    );
}

/// Point 4 — une base sans ancre est ANCRÉE explicitement par la tique, pas
/// remise à zéro silencieusement à la lecture.
#[test]
fn une_base_sans_ancre_est_ancree_par_la_tique() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("sans")).unwrap());
    let id = sub.create_node(&["n"]);
    // Colonne posée sans aucune ancre (ni composite, ni propriété héritée).
    sub.set_node_energy_f32(id, 1.0).unwrap();

    let law = DecayLaw::new(H1);
    // À la lecture, l'âge est inconnu : on rend E_ref sans l'inventer.
    let avant = decay_clock::current_energy(&sub, id, &law).unwrap();
    assert!(
        (avant - 1.0).abs() < 1e-6,
        "lecture d'une base sans ancre : {avant}"
    );
    assert!(
        decay_clock::read_anchor(&sub, id).is_none(),
        "pas encore ancré"
    );

    // La tique ANCRE le nœud (migration explicite) et le compte.
    let stats = decay_clock::advance_all(&sub, &law, 0, unix_micros()).unwrap();
    println!("MIGRATION : unanchored={} (attendu 1)", stats.unanchored);
    assert_eq!(stats.unanchored, 1, "le nœud sans ancre doit être ancré");
    assert!(
        decay_clock::read_anchor(&sub, id).is_some(),
        "ancré par la tique"
    );

    // Désormais ancré à maintenant : il décroît normalement à partir d'ici.
    let t = unix_micros();
    let lue = decay_clock::energy_at(&sub, id, &law, t + H1.as_micros() as i64).unwrap();
    assert!(
        (lue - 0.5).abs() < 1e-6,
        "après ancrage, une demi-vie divise par 2 : {lue}"
    );
}
