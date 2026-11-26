//! CONTRE-ÉPREUVE INDÉPENDANTE du miroir énergétique sous CONCURRENCE.
//!
//! Les trois tests historiques couvrent la DURABILITÉ : `decay_atomicite_test`
//! (les quatre combinaisons), `ancre_composite_kill9` (SIGKILL entre ancre et
//! miroir) et `panne_miroir_nul_test` (abort réel + réconciliation). Tous
//! mesurent une PANNE (abort) ou une divergence STATIQUE posée à la main —
//! jamais l'ENTRELACEMENT de DEUX écrivains vivants sur le même nœud.
//!
//! Ce banc est le discriminant manquant : A publie l'ancre 0,3 puis SUSPEND
//! (il ne meurt pas) avant d'écrire le miroir ; B publie l'ancre 0,4 ET son
//! miroir ; A reprend et écrit son miroir (0,3). Aucune panne, aucun `sleep` :
//! l'entrelacement est reproduit DÉTERMINISTEMENT avec les primitives publiques
//! du moteur (`persist_anchor_explicit` = « ancre seule », `reinforce` =
//! « ancre + miroir », `set_node_energy_f32` = « miroir seul »).
//!
//! VERDICT ATTENDU (rouge) : l'ancre est protégée par l'autorité conditionnelle
//! native (Strict) — une publication périmée est un REFUS CERTAIN
//! (`ConflitPrecondition`). Le miroir, lui, est une écriture ABSOLUE non gardée
//! (`set_node_energy_f32`) : le miroir périmé de A écrase celui, plus récent,
//! de B. Les lecteurs du miroir (`get_node_energy_f32`, la colonne Q1.15 que
//! lisent les chemins directs du moteur) observent donc une valeur qui DIVERGE
//! de l'ancre (autorité) de façon observable.
//!
//! Run: cargo test -p obrain-cognitive --test contre_epreuve_miroir_concurrent \
//!        --features substrate -- --nocapture
#![cfg(feature = "substrate")]

use std::sync::Arc;
use std::time::Duration;

use obrain_cognitive::decay_clock::{self, EnergyRead};
use obrain_cognitive::decay_law::DecayLaw;
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::{IssuePublication, ModePublication, SubstrateStore};

/// Instant fixe (micros Unix) : aucune décroissance entre les étapes (dt = 0).
const BASE: i64 = 1_000_000_000_000_000;
const H: Duration = Duration::from_secs(64);
const ANCRE: &str = "_cog_energy_anchor";

fn law() -> DecayLaw {
    DecayLaw::new(H)
}

/// Encode l'ancre composite `(e_ref, t_ref, t_hi, H)` en 32 octets LE.
fn ancre_bytes(e_ref: f64) -> Value {
    let t = (BASE as f64) / 1e6;
    let mut b = [0u8; 32];
    for (i, x) in [e_ref, t, t, H.as_secs_f64()].iter().enumerate() {
        b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    Value::Bytes(Arc::from(b.as_slice()))
}

/// Quantification Q1.15 d'une valeur `[0, 1]` — le miroir stocké est la
/// projection `q1_15_to_f32(f32_to_q1_15(v))`.
fn q115(v: f32) -> f32 {
    obrain_substrate::record::q1_15_to_f32(obrain_substrate::record::f32_to_q1_15(v))
}

/// LE SCÉNARIO EXACT : A ancre 0,3 → SUSPEND ; B ancre 0,4 + miroir ; A reprend
/// et écrit son miroir 0,3. Aucune panne, aucun sleep.
#[test]
fn le_miroir_peut_diverger_de_l_ancre_sous_concurrence_sans_panne() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);
    let law = law();

    // État de départ : miroir nul, pas encore d'ancre.
    sub.set_node_energy_f32(n, 0.0).unwrap();

    // --- A : publie l'ancre 0,3 PUIS SUSPEND avant d'écrire le miroir. ---
    // `persist_anchor_explicit` écrit UNIQUEMENT l'ancre composite (aucune
    // écriture de miroir) : c'est exactement l'état « A suspendu entre l'ancre
    // et le miroir » que `reinforce` atteindrait au point d'injection — mais
    // SANS abort.
    decay_clock::persist_anchor_explicit(&sub, n, 0.3, BASE, H);
    let ancre_apres_a = decay_clock::read_anchor(&sub, n).unwrap();
    let miroir_apres_a = sub.get_node_energy_f32(n).unwrap().unwrap();
    eprintln!(
        "A (ancre seule) : ancre e_ref={} miroir={}",
        ancre_apres_a.e_ref, miroir_apres_a
    );
    assert!(
        (ancre_apres_a.e_ref - 0.3).abs() < 1e-9,
        "A a publié l'ancre 0,3 : {}",
        ancre_apres_a.e_ref
    );
    assert!(
        (miroir_apres_a - 0.0).abs() < 1e-6,
        "A suspendu : le miroir n'est PAS écrit (reste 0) : {miroir_apres_a}"
    );

    // --- B : publie l'ancre 0,4 ET son miroir (renforce 0,3 + 0,1). ---
    let rendu_b = decay_clock::reinforce(&sub, n, 0.1, &law, 1.0, BASE);
    let ancre_apres_b = decay_clock::read_anchor(&sub, n).unwrap();
    let miroir_apres_b = sub.get_node_energy_f32(n).unwrap().unwrap();
    eprintln!(
        "B (ancre + miroir) : rendu={rendu_b:?} ancre e_ref={} miroir={}",
        ancre_apres_b.e_ref, miroir_apres_b
    );
    assert_eq!(rendu_b, Some(0.4), "B renforce 0,3 + 0,1 = 0,4");
    assert!(
        (ancre_apres_b.e_ref - 0.4).abs() < 1e-9,
        "B a publié l'ancre 0,4 : {}",
        ancre_apres_b.e_ref
    );
    assert!(
        (miroir_apres_b - q115(0.4)).abs() < 1e-4,
        "B a écrit son miroir Q1.15(0,4) = {}",
        q115(0.4)
    );

    // --- A REPREND : écrit son miroir périmé (0,3), sans repasser l'autorité. ---
    // `reinforce` aurait écrit `set_node_energy_f32(n, 0.3)` juste après
    // l'ancre : c'est l'écriture ABSOLUE, non gardée, qui écrase le miroir de B.
    sub.set_node_energy_f32(n, 0.3).unwrap();

    // --- Mesures finales, SÉPARÉES. ---
    let ancre_finale = decay_clock::read_anchor(&sub, n).unwrap();
    let miroir_final = sub.get_node_energy_f32(n).unwrap().unwrap();
    let lecteur_ancre = decay_clock::energy_at(&sub, n, &law, BASE).unwrap();
    let lecteur_qualifie = decay_clock::read_energy_qualified(&sub, n, &law, BASE);
    let lecteur_miroir = sub.get_node_energy_f32(n).unwrap().unwrap();

    eprintln!(
        "FINAL : ancre={} miroir={} energy_at={} qualifie={:?} lecteur_miroir={}",
        ancre_finale.e_ref, miroir_final, lecteur_ancre, lecteur_qualifie, lecteur_miroir
    );

    // L'ancre (autorité) porte bien la valeur de B : 0,4.
    assert!(
        (ancre_finale.e_ref - 0.4).abs() < 1e-9,
        "ancre finale = 0,4 (autorité), mesuré {}",
        ancre_finale.e_ref
    );
    // Les lecteurs de l'ANCRE observent 0,4.
    assert!(
        (lecteur_ancre - 0.4).abs() < 1e-9,
        "lecteur de l'ancre (energy_at) = 0,4, mesuré {lecteur_ancre}"
    );
    assert!(
        matches!(lecteur_qualifie, EnergyRead::Effective(e) if (e - 0.4).abs() < 1e-3),
        "lecture qualifiée = Effective(0,4), mesuré {lecteur_qualifie:?}"
    );

    // Le miroir final porte la valeur PÉRIMÉE de A : Q1.15(0,3) ≈ 0,29999.
    assert!(
        (miroir_final - q115(0.3)).abs() < 1e-4,
        "miroir final = Q1.15(0,3) ≈ {}, mesuré {miroir_final}",
        q115(0.3)
    );
    assert!(
        (miroir_final - q115(0.4)).abs() > 0.05,
        "le miroir final N'EST PAS celui de B (0,4) : {miroir_final}"
    );

    // --- VERDICT : divergence OBSERVABLE entre l'ancre et le miroir. ---
    // Les lecteurs du miroir (colonne Q1.15, chemins directs du moteur)
    // observent 0,3 alors que l'ancre dit 0,4.
    assert_eq!(lecteur_miroir, miroir_final, "le lecteur du miroir lit la colonne");
    assert!(
        (ancre_finale.e_ref - lecteur_miroir as f64).abs() > 0.05,
        "ROUGE : le miroir ({} lu) diverge de l'ancre ({} autorité) de façon observable",
        lecteur_miroir,
        ancre_finale.e_ref
    );
}

/// `persist_anchor_explicit` n'écrit QUE l'ancre : le miroir n'est jamais
/// touché. C'est la primitive qui matérialise « A suspendu avant le miroir ».
#[test]
fn persist_anchor_explicit_ecrit_l_ancre_sans_toucher_le_miroir() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);
    sub.set_node_energy_f32(n, 0.5).unwrap();

    decay_clock::persist_anchor_explicit(&sub, n, 0.3, BASE, H);

    let ancre = decay_clock::read_anchor(&sub, n).unwrap();
    let miroir = sub.get_node_energy_f32(n).unwrap().unwrap();
    assert!((ancre.e_ref - 0.3).abs() < 1e-9, "ancre = 0,3 : {}", ancre.e_ref);
    assert!(
        (miroir - q115(0.5)).abs() < 1e-4,
        "miroir inchangé (0,5) : {miroir}"
    );
}

/// L'asymétrie qui fait le discriminant : l'ancre est protégée (refus certain),
/// le miroir ne l'est pas (écriture absolue qui écrase).
#[test]
fn l_ancre_est_protegee_mais_pas_le_miroir() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("kb")).unwrap());
    let n: NodeId = sub.create_node(&["n"]);

    // État initial : ancre 0,3 + miroir 0,3.
    decay_clock::persist_anchor_explicit(&sub, n, 0.3, BASE, H);
    sub.set_node_energy_f32(n, 0.3).unwrap();

    // A capture la précondition (l'ancre vaut encore 0,3).
    let pre = sub.capturer_precondition(n, &[ANCRE]).unwrap();

    // B fait avancer l'ancre à 0,4 (et son miroir) pendant que A est suspendu.
    decay_clock::persist_anchor_explicit(&sub, n, 0.4, BASE, H);
    sub.set_node_energy_f32(n, 0.4).unwrap();

    // A reprend : il veut publier son ancre PÉRIMÉE (0,3) avec sa vieille
    // précondition, en mode Strict.
    let prep = sub
        .preparer_transformation(n, &[(ANCRE, ancre_bytes(0.3))], &pre, ModePublication::Strict)
        .unwrap();
    let issue = sub.publier_si_precondition_intacte(prep).unwrap();

    // REFUS CERTAIN : la dépendance a changé depuis la capture → aucun effet.
    assert_eq!(
        issue,
        IssuePublication::ConflitPrecondition,
        "l'ancre périmée est refusée de façon certaine (pas d'effet)"
    );
    let ancre = decay_clock::read_anchor(&sub, n).unwrap();
    assert!(
        (ancre.e_ref - 0.4).abs() < 1e-9,
        "l'ancre reste 0,4 (B), pas 0,3 : {}",
        ancre.e_ref
    );

    // Contraste : le miroir, lui, n'a AUCUN refus certain. A écrit sa valeur
    // périmée (0,3) par-dessus le miroir de B (0,4), et l'écriture « réussit ».
    let ok = sub.set_node_energy_f32(n, 0.3);
    assert!(ok.is_ok(), "le miroir périmé s'écrit sans refus : {ok:?}");
    let miroir = sub.get_node_energy_f32(n).unwrap().unwrap();
    assert!(
        (miroir - q115(0.3)).abs() < 1e-4,
        "miroir écrasé à 0,3 (périmé) : {miroir}"
    );
    // L'ancre dit 0,4 ; le miroir dit 0,3 : divergence observable, sans panne.
    assert!(
        (ancre.e_ref - miroir as f64).abs() > 0.05,
        "divergence ancre(0,4)/miroir(0,3) observable"
    );
}
