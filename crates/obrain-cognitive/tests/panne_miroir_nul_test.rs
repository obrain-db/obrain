//! Panne réelle (abort) entre l'ancre et le miroir, avec miroir initial NUL :
//! après réouverture, l'ancre positive a survécu, le miroir est obsolète (0),
//! et la consolidation à temps constant réconcilie le miroir sans écriture
//! supplémentaire ensuite.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::DecayLaw;
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const ENV_ENFANT: &str = "OBRAIN_KILL9_MIROIR_NUL";
const H: Duration = Duration::from_secs(64);
fn law() -> DecayLaw { DecayLaw::new(H) }
fn unix_micros() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64
}

/// Enfant : miroir nul, ancre positive (0,5), renforce → abort entre l'ancre
/// (0,75) et le miroir (qui reste 0).
#[test]
fn enfant() {
    let Ok(dir) = std::env::var(ENV_ENFANT) else { return };
    let chemin = Path::new(&dir).join("kb");
    let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
    let id = sub.create_node(&["n"]);
    let now = unix_micros();
    // Miroir initial NUL, ancre positive.
    decay_clock::persist_anchor_explicit(&sub, id, 0.5, now, H);
    sub.set_node_energy_f32(id, 0.0).expect("colonne nulle");
    std::fs::write(Path::new(&dir).join("pret"), b"ok").expect("marqueur");

    obrain_common::utils::panne::armer_arret_entre_ancre_et_miroir();
    let _ = decay_clock::reinforce(&sub, id, 0.25, &law(), 1.0, now);
    std::process::exit(3); // atteint seulement si l'injection n'est pas sur le chemin
}

#[test]
fn panne_entre_ancre_et_miroir_puis_reconciliation() {
    let td = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().expect("exe");
    let mut cmd = Command::new(exe);
    cmd.env(ENV_ENFANT, td.path()).arg("--exact").arg("enfant").arg("--nocapture");
    let statut = cmd.status().expect("spawn enfant");
    assert!(!statut.success() && statut.code() != Some(3), "enfant mort sur injection : {statut:?}");
    assert!(td.path().join("pret").exists(), "marqueur pret présent");

    // Réouverture.
    let sub = Arc::new(SubstrateStore::open(td.path().join("kb")).expect("open"));
    let id = NodeId(1);
    let now = unix_micros();

    // L'ancre positive a survécu (0,5 ou 0,75) ; le miroir est 0 (obsolète).
    let ancre = decay_clock::read_anchor(&sub, id).expect("ancre lisible");
    assert!(ancre.e_ref > 0.0, "ancre positive conservée : {}", ancre.e_ref);
    let miroir = sub.get_node_energy_f32(id).unwrap().unwrap();
    eprintln!("après réouverture : ancre e_ref={} miroir={}", ancre.e_ref, miroir);

    // Consolidation à temps constant : réconciliation du miroir (0 → Q1.15(ancre)).
    let stats = decay_clock::advance_all(sub.as_ref(), &law(), 0, now).unwrap();
    eprintln!("stats: {:?}", stats);
    assert!(stats.reconciled >= 1, "miroir obsolète réconcilié : {stats:?}");

    // Seconde consolidation : aucune écriture supplémentaire.
    let stats2 = decay_clock::advance_all(sub.as_ref(), &law(), 0, now).unwrap();
    assert_eq!(stats2.advanced + stats2.reconciled, 0, "aucune écriture superflue : {stats2:?}");
}
