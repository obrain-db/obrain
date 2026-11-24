//! Point 5 — cohérence de la matérialisation : même état initial, même instant
//! final, avec (a) 0 tique, (b) N tiques, (c) balayage partiel.
//!
//! La quantification Q1.15 casse l'identité `Q(Q(E·a)·b) = Q(E·ab)` : chaque
//! matérialisation arrondit au quantum 1/32768. La tique SKIP quand la valeur
//! ne change pas au bit près (l'ancre ne bouge pas, `Δt` s'accumule), ce qui
//! évite la dérive « 30 quanta restent 30 » ; il reste l'arrondi de chaque
//! matérialisation effective, borné à 1 quantum par matérialisation.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{DecayLaw, unix_micros};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

// H = 64 s : la tique vaut exactement 1 s, et le facteur par tique est
// 2^(-1/64). L'instant final T = t0 + H vaut une demi-vie (0,5 → 0,25).
const H: Duration = Duration::from_secs(64);

fn run(td: &tempfile::TempDir, nom: &str, max_nodes: usize, n_ticks: u32) -> f64 {
    let sub = Arc::new(SubstrateStore::create(td.path().join(nom)).unwrap());
    let law = DecayLaw::new(H);
    let t0 = unix_micros();
    let t_h = t0 + H.as_micros() as i64;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = sub.create_node(&["n"]);
        sub.set_node_energy_f32(id, 0.5).unwrap();
        decay_clock::persist_anchor_explicit(&sub, id, 0.5, t0, H);
        ids.push(id);
    }
    let pas = H.as_micros() as i64 / 64;
    for k in 1..=n_ticks {
        let _ = decay_clock::advance_all(&sub, &law, max_nodes, t0 + (k as i64) * pas).unwrap();
    }
    let somme: f64 = ids
        .iter()
        .map(|id| decay_clock::energy_at(&sub, *id, &law, t_h).unwrap())
        .sum();
    somme / ids.len() as f64
}

#[test]
fn les_trois_cadences_donnent_la_meme_valeur_a_la_borne_pres() {
    let td = tempfile::tempdir().unwrap();
    let a = run(&td, "zero", 0, 0); // 0 tique
    let b = run(&td, "plein", 0, 64); // 64 tiques, sans borne
    let c = run(&td, "partiel", 1, 64); // balayage partiel : 1 nœud / tique, même fenêtre

    println!(
        "CADENCE (3 nœuds, H=64 s, T=une demi-vie, exact=0.25) : \
         0 tique={a:.9} 64 tiques={b:.9} partiel(max_nodes=1)={c:.9}"
    );

    // Borne publiée : 1 quantum (1/32768 ≈ 3,05e-5) par matérialisation
    // effective, soit ≤ 64/32768 ≈ 0,002 en absolu pour 64 matérialisations.
    let borne = 0.002;
    let exact = 0.25;
    for (nom, v) in [("0 tique", a), ("64 tiques", b), ("partiel", c)] {
        assert!(
            (v - exact).abs() <= borne,
            "{nom} : {v} s'écarte de l'exact {exact} au-delà de la borne {borne}"
        );
    }
    // Les trois cadences coïncident entre elles (le balayage partiel ne
    // rattrape pas moins bien que la tique pleine).
    assert!((a - b).abs() <= borne, "0 tique vs N tiques : {a} vs {b}");
    assert!((b - c).abs() <= borne, "N tiques vs partiel : {b} vs {c}");
}

/// Le contre-exemple de la revue : 30 quanta × 2^(-1/64) s'arrondit à 30. La
/// tique SKIP quand le f32 ne change pas, donc l'ancre ne bouge pas et `Δt`
/// s'accumule : après une demi-vie, le nœud tombe à ~15 quanta — il ne reste
/// PAS bloqué à 30.
#[test]
fn trente_quanta_ne_restent_pas_bloques_a_trente() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("quanta")).unwrap());
    let law = DecayLaw::new(H);
    let id = sub.create_node(&["n"]);
    let trente_quanta = 30.0 / 32768.0;
    sub.set_node_energy_f32(id, trente_quanta as f32).unwrap();
    let t0 = unix_micros();
    decay_clock::persist_anchor_explicit(&sub, id, trente_quanta, t0, H);

    let pas = H.as_micros() as i64 / 64;
    for k in 1..=64 {
        let _ = decay_clock::advance_all(&sub, &law, 0, t0 + (k as i64) * pas).unwrap();
    }
    let t_h = t0 + H.as_micros() as i64;
    let lue = decay_clock::energy_at(&sub, id, &law, t_h).unwrap();
    println!("QUANTA : 30 quanta après une demi-vie = {lue:.9} (attendu ≈ 15/32768 = 0,0004578)");
    // Une demi-vie divise par deux : ~15 quanta, pas 30.
    assert!(
        lue < trente_quanta * 0.7,
        "le nœud est resté bloqué à ~30 quanta : {lue}"
    );
    assert!(
        (lue - 15.0 / 32768.0).abs() < 2.0 / 32768.0,
        "une demi-vie → 15 quanta, mesuré {lue}"
    );
}
