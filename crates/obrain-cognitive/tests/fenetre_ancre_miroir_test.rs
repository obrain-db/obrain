//! Fenêtre ancre→miroir sous contention (scénario A/B) : le miroir de colonne
//! est DÉRIVÉ, jamais autorité. A publie l'ancre 0,3 puis SUSPEND avant
//! d'écrire son miroir ; B publie l'ancre 0,4 ET son miroir ; A reprend et
//! tente d'écrire son ancien miroir (0,3). L'écriture du miroir est
//! conditionnée à l'estampille de l'ancre : le miroir de A (obsolète) est
//! REFUSÉ, le miroir final reste 0,4, cohérent avec l'ancre canonique.

#![cfg(feature = "substrate")]

use std::sync::Arc;
use std::time::Duration;

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{DecayLaw, unix_micros};
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{IssuePublication, ModePublication, SubstrateStore};

const H: Duration = Duration::from_secs(3600);

fn ancre_value(e: f64) -> Value {
    let t = 1_000_000_000_000_000f64 / 1e6; // epoch secs
    let mut b = [0u8; 32];
    for (i, x) in [e, t, t, 64.0].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(b.as_slice()))
}

/// Contre-épreuve déterministe de la PRIMITIVE : `set_node_energy_f32_si_
/// propriete_intacte` refuse d'écrire la colonne quand l'estampille de l'ancre
/// n'est plus la plus récente.
#[test]
fn la_garde_du_miroir_refuse_une_estampille_perimee() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);
    let cle = "_cog_energy_anchor";

    // Ancre A (0,3) publiée sous l'autorité conditionnelle.
    let pre_a = sub.capturer_precondition(n, &[cle]).unwrap();
    let prep_a = sub
        .preparer_transformation(
            n,
            &[(cle, ancre_value(0.3))],
            &pre_a,
            ModePublication::Strict,
        )
        .unwrap();
    let (auteur_a, horloge_a) = prep_a.estampille_effet();
    assert!(matches!(
        sub.publier_si_precondition_intacte(prep_a).unwrap(),
        IssuePublication::PublieIntegrale
    ));

    // L'estampille courante de A peut écrire le miroir.
    assert!(
        sub.set_node_energy_f32_si_propriete_intacte(n, 0.3, cle, Some((auteur_a, horloge_a)))
            .unwrap()
    );

    // Ancre B (0,4) plus récente, puis son miroir.
    let pre_b = sub.capturer_precondition(n, &[cle]).unwrap();
    let prep_b = sub
        .preparer_transformation(
            n,
            &[(cle, ancre_value(0.4))],
            &pre_b,
            ModePublication::Strict,
        )
        .unwrap();
    let (auteur_b, horloge_b) = prep_b.estampille_effet();
    assert!(matches!(
        sub.publier_si_precondition_intacte(prep_b).unwrap(),
        IssuePublication::PublieIntegrale
    ));
    assert!(
        sub.set_node_energy_f32_si_propriete_intacte(n, 0.4, cle, Some((auteur_b, horloge_b)))
            .unwrap()
    );

    // L'estampille PÉRIMÉE de A ne doit PLUS pouvoir écrire le miroir.
    assert!(
        !sub.set_node_energy_f32_si_propriete_intacte(n, 0.3, cle, Some((auteur_a, horloge_a)))
            .unwrap(),
        "l'estampille périmée de A doit être refusée"
    );
    let miroir = sub.get_node_energy_f32(n).unwrap().unwrap();
    assert!(
        (miroir as f64 - 0.4).abs() < 1e-3,
        "miroir reste 0,4 (pas 0,3) : {miroir}"
    );
}

/// Scénario A/B/miroir de bout en bout : deux `reinforce` concurrents, A
/// suspendu entre la publication de son ancre et l'écriture de son miroir.
#[test]
fn le_miroir_obsolete_de_a_n_ecrase_pas_le_miroir_de_b() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let id: NodeId = sub.create_node(&["n"]);
    let now = unix_micros();
    let law = DecayLaw::new(H);

    // État initial cohérent : ancre 0,2 + miroir 0,2.
    decay_clock::persist_anchor_explicit(&sub, id, 0.2, now, H);
    sub.set_node_energy_f32(id, 0.2).unwrap();
    sub.flush().unwrap();

    // Arme le blocage (test) entre l'ancre et le miroir.
    let (liberation, arrivee) = obrain_common::utils::panne::armer_blocage_entre_ancre_et_miroir();

    // A : renforce 0,1 → 0,3, puis se bloque entre ancre et miroir.
    let sub_a = sub.clone();
    let a = std::thread::spawn(move || decay_clock::reinforce(&sub_a, id, 0.1, &law, 1.0, now));

    // Attend que A ait publié son ancre (0,3) et soit bloqué.
    arrivee.recv().unwrap();

    // B : renforce 0,1 → 0,4 (lit l'ancre 0,3 de A), publie ancre + miroir.
    let r_b = decay_clock::reinforce(&sub, id, 0.1, &law, 1.0, now);
    assert!(
        (r_b.unwrap() - 0.4).abs() < 1e-6,
        "B rend 0,4 (lu depuis l'ancre 0,3 de A)"
    );

    // Libère A : il tente d'écrire son miroir obsolète (0,3).
    liberation.send(()).unwrap();
    let r_a = a.join().unwrap();
    assert!(
        (r_a.unwrap() - 0.3).abs() < 1e-6,
        "A rend la valeur qu'il a publiée (0,3)"
    );

    // L'ancre canonique est 0,4 (celle de B, la plus récente).
    let ancre = decay_clock::read_anchor(&sub, id).unwrap();
    assert!(
        (ancre.e_ref - 0.4).abs() < 1e-6,
        "ancre canonique = 0,4, relu {}",
        ancre.e_ref
    );

    // Le miroir ne doit PAS être 0,3 (l'obsolète de A) : il reflète 0,4.
    let miroir = sub.get_node_energy_f32(id).unwrap().unwrap();
    assert!(
        (miroir as f64 - 0.4).abs() < 1e-3,
        "miroir = 0,4 (pas 0,3) : {miroir}"
    );
    assert!(
        (miroir as f64 - 0.3).abs() > 0.05,
        "le miroir obsolète de A (0,3) n'a pas été écrit : {miroir}"
    );

    // La lecture qualifiée rend 0,4 (l'ancre), jamais le miroir obsolète.
    let r = decay_clock::read_energy_qualified(&sub, id, &law, now);
    assert!(
        matches!(r, decay_clock::EnergyRead::Effective(e) if (e - 0.4).abs() < 1e-3),
        "lecture qualifiée = 0,4 (ancre), pas le miroir : {r:?}"
    );
}
