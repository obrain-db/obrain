//! Reconstruction du rouge INDÉPENDANT (sync, note d436a6e0, base a2ec54e) sur
//! le VRAI chemin `decay_clock::reinforce` (pas la décomposition manuelle).
//!
//! Séquence mesurée par sync :
//!   1. A : reinforce(+0,1) depuis une ancre à 0,3 → publie l'ancre 0,4 → SUSPEND
//!      avant le miroir (hook bloquant `armer_blocage_entre_ancre_et_miroir`).
//!   2. B : reinforce(+0,1) depuis l'ancre 0,4 (celle de A) → publie l'ancre 0,5
//!      ET son miroir 0,5 → termine.
//!   3. A : reprend → tente d'écrire son miroir obsolète 0,4.
//!
//! AVANT le correctif (028e97e) : miroir final = 0,399993896484375 (= Q1.15(0,4))
//! alors que l'ancre canonique vaut 0,5 — EXIT=101. APRÈS le correctif, la garde
//! `set_node_energy_f32_si_propriete_intacte` refuse le miroir obsolète : le miroir
//! final reste 0,5, COHÉRENT avec l'ancre canonique.

#![cfg(feature = "substrate")]

use std::sync::Arc;
use std::time::Duration;

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{unix_micros, DecayLaw};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;

const H: Duration = Duration::from_secs(3600);

/// Q1.15(0.4) : la valeur EXACTE du miroir obsolète observé par sync.
fn q1_15_de_0_4() -> f32 {
    obrain_substrate::record::q1_15_to_f32(obrain_substrate::record::f32_to_q1_15(0.4))
}

#[test]
fn le_miroir_reste_coherent_avec_l_ancre_canonique_0_5() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id: NodeId = sub.create_node(&["n"]);
    let now = unix_micros();
    let law = DecayLaw::new(H);

    // État initial cohérent : ancre 0,3 + miroir 0,3.
    decay_clock::persist_anchor_explicit(&sub, id, 0.3, now, H);
    sub.set_node_energy_f32(id, 0.3).unwrap();
    sub.flush().unwrap();

    // Arme le blocage (test) entre l'ancre et le miroir.
    let (liberation, arrivee) =
        obrain_common::utils::panne::armer_blocage_entre_ancre_et_miroir();

    // A : renforce 0,1 → 0,4, puis se bloque entre ancre et miroir.
    let sub_a = sub.clone();
    let a = std::thread::spawn(move || decay_clock::reinforce(&sub_a, id, 0.1, &law, 1.0, now));

    // Attend que A ait publié son ancre (0,4) et soit bloqué.
    arrivee.recv().unwrap();

    // B : renforce 0,1 → 0,5 (lit l'ancre 0,4 de A), publie ancre + miroir.
    let r_b = decay_clock::reinforce(&sub, id, 0.1, &law, 1.0, now);
    assert!(
        (r_b.unwrap() - 0.5).abs() < 1e-6,
        "B rend 0,5 (lu depuis l'ancre 0,4 de A)"
    );

    // Libère A : il tente d'écrire son miroir obsolète (0,4).
    liberation.send(()).unwrap();
    let r_a = a.join().unwrap();
    assert!(
        (r_a.unwrap() - 0.4).abs() < 1e-6,
        "A rend la valeur qu'il a publiée (0,4)"
    );

    // L'ancre canonique est 0,5 (celle de B, la plus récente).
    let ancre = decay_clock::read_anchor(&sub, id).unwrap();
    eprintln!("ancre_canonique = {}", ancre.e_ref);
    assert!(
        (ancre.e_ref - 0.5).abs() < 1e-6,
        "ancre canonique = 0,5, relu {}",
        ancre.e_ref
    );

    // Le miroir ne doit PAS être 0,399993896484375 (l'obsolète de A) : il
    // reflète 0,5, cohérent avec l'ancre canonique.
    let miroir = sub.get_node_energy_f32(id).unwrap().unwrap();
    eprintln!("miroir_final = {miroir} (Q1.15(0,4) obsolète = {})", q1_15_de_0_4());
    assert!(
        (miroir - 0.5).abs() < 1e-3,
        "miroir = 0,5 (pas 0,4) : {miroir}"
    );
    assert!(
        (miroir - q1_15_de_0_4()).abs() > 1e-6,
        "le miroir obsolète de A (0,399993896484375) n'a pas été écrit : {miroir}"
    );

    // La lecture qualifiée rend 0,5 (l'ancre), jamais le miroir obsolète.
    let r = decay_clock::read_energy_qualified(&sub, id, &law, now);
    assert!(
        matches!(r, decay_clock::EnergyRead::Effective(e) if (e - 0.5).abs() < 1e-3),
        "lecture qualifiée = 0,5 (ancre), pas le miroir : {r:?}"
    );
}
