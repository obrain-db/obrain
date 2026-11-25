//! Les trois régimes de la décroissance : sous le seuil, au seuil, au-dessus.
//!
//! Une loi exponentielle n'a pas de « cas particuliers » — mais un code qui
//! clampe, quantifie en Q1.15 et compare à un seuil en a trois. Ces tests
//! mesurent les valeurs EXACTES et refusent le masquage : un plan de
//! décroissance faux et un plan juste doivent donner des nombres différents,
//! sinon le test ne prouve rien.
//!
//! Mesures obtenues avec `--nocapture` (2026-11-24, worktree ~/wt-decay).

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{
    DecayLaw, epoch_secs_to_micros, micros_to_epoch_secs, unix_micros,
};
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const H: Duration = Duration::from_secs(3600);

fn config() -> EnergyConfig {
    EnergyConfig {
        default_half_life: H,
        ..Default::default()
    }
}

fn store(td: &tempfile::TempDir, nom: &str) -> Arc<SubstrateStore> {
    Arc::new(SubstrateStore::create(td.path().join(nom)).unwrap())
}

/// Place l'ancre d'un nœud `k` demi-vies dans le passé (le temps écoulé est
/// réel : c'est la même grandeur physique, seulement décalée).
fn ancre_il_y_a(sub: &SubstrateStore, id: obrain_common::types::NodeId, demi_vies: f64) {
    let recul = (demi_vies * H.as_micros() as f64) as i64;
    decay_clock::persist_anchor(sub, id, unix_micros() - recul);
}

// ---------------------------------------------------------------------------
// Régime 1 — SOUS le seuil
// ---------------------------------------------------------------------------

#[test]
fn sous_le_seuil_l_energie_continue_de_decroitre_et_ne_devient_jamais_negative() {
    let td = tempfile::tempdir().unwrap();
    let sub = store(&td, "r1");
    let id = sub.create_node(&["n"]);
    sub.set_node_energy_f32(id, 0.008).unwrap();
    let es = EnergyStore::with_substrate(config(), sub.clone());

    let seuil = es.config().min_energy;
    let e_colonne = sub.get_node_energy_f32(id).unwrap().unwrap() as f64;
    let e_aujourd_hui = es.get_energy(id);
    ancre_il_y_a(&sub, id, 1.0);
    let e_apres_1h = es.get_energy(id);
    ancre_il_y_a(&sub, id, 1000.0);
    let e_apres_1000h = es.get_energy(id);

    println!(
        "RÉGIME 1 (sous le seuil {seuil}) : colonne={e_colonne} t0={e_aujourd_hui} 1H={e_apres_1h} 1000H={e_apres_1000h}"
    );
    assert!(e_colonne < seuil, "0.008 doit être sous le seuil {seuil}");
    assert!(
        (e_apres_1h - 0.004).abs() < 1e-3,
        "une demi-vie divise par 2 : {e_apres_1h}"
    );
    assert!(
        (0.0..1e-9).contains(&e_apres_1000h),
        "jamais négatif : {e_apres_1000h}"
    );
    // Sous le seuil = candidat à l'archivage, et il le reste.
    assert!(es.list_low_energy(seuil).contains(&id));
}

// ---------------------------------------------------------------------------
// Régime 2 — AU seuil (exactement)
// ---------------------------------------------------------------------------

#[test]
fn au_seuil_la_comparaison_est_stricte_et_la_demi_vie_la_fait_basculer() {
    let td = tempfile::tempdir().unwrap();
    let sub = store(&td, "r2");
    let id = sub.create_node(&["n"]);
    let es = EnergyStore::with_substrate(config(), sub.clone());
    let seuil = es.config().min_energy;

    // Q1.15 ne sait pas écrire 0.01 exactement : 328/32768 = 0.010009765625.
    sub.set_node_energy_f32(id, seuil as f32).unwrap();
    let e_pose = sub.get_node_energy_f32(id).unwrap().unwrap() as f64;
    let e_t0 = es.get_energy(id);
    let bas_avant = es.list_low_energy(seuil).contains(&id);

    ancre_il_y_a(&sub, id, 1.0);
    let e_1h = es.get_energy(id);
    let bas_apres = es.list_low_energy(seuil).contains(&id);

    println!(
        "RÉGIME 2 (au seuil {seuil}) : posé={e_pose} t0={e_t0} 1H={e_1h} low_avant={bas_avant} low_après={bas_apres}"
    );
    // Au seuil (0.010009765625) n'est PAS sous le seuil : la comparaison `<`.
    assert!(e_t0 >= seuil);
    assert!(!bas_avant, "un nœud AU seuil n'est pas « bas »");
    // Une demi-vie plus tard, il l'est.
    assert!(e_1h < seuil);
    assert!(bas_apres, "une demi-vie plus tard, il est « bas »");
    assert!((e_1h - e_t0 / 2.0).abs() < 1e-3);
}

// ---------------------------------------------------------------------------
// Régime 3 — AU-DESSUS du seuil
// ---------------------------------------------------------------------------

#[test]
fn au_dessus_du_seuil_le_temps_de_bascule_est_predit_par_la_loi() {
    let td = tempfile::tempdir().unwrap();
    let sub = store(&td, "r3");
    let id = sub.create_node(&["n"]);
    let es = EnergyStore::with_substrate(config(), sub.clone());
    let seuil = es.config().min_energy;
    let law = DecayLaw::new(H);

    sub.set_node_energy_f32(id, 0.5).unwrap();
    let e0 = es.get_energy(id);
    // 0.5 -> 0.01 : log2(50) demi-vies = 5,6438…
    let bascule = law.elapsed_to_reach(e0, seuil);
    let e_juste_avant = law.energy_at(e0, bascule.checked_sub(Duration::from_secs(1)).unwrap());
    let e_juste_apres = law.energy_at(e0, bascule + Duration::from_secs(1));

    ancre_il_y_a(&sub, id, bascule.as_secs_f64() / H.as_secs_f64());
    let e_mesure = es.get_energy(id);

    println!(
        "RÉGIME 3 (au-dessus de {seuil}) : e0={e0} bascule={bascule:?} avant={e_juste_avant} après={e_juste_apres} mesuré={e_mesure}"
    );
    assert!(
        (e_mesure - seuil).abs() < 5e-4,
        "au temps de bascule prédit : {e_mesure}"
    );
    assert!(!es.list_low_energy(seuil).contains(&id) || e_mesure < seuil);
}

// ---------------------------------------------------------------------------
// Saturation de la colonne Q1.15 — les trois régimes, et le choix assumé
// ---------------------------------------------------------------------------

#[test]
fn saturation_trois_regimes_et_perte_assumee() {
    let td = tempfile::tempdir().unwrap();
    let sub = store(&td, "sat");
    let a = sub.create_node(&["n"]);
    let b = sub.create_node(&["n"]);
    let c = sub.create_node(&["n"]);
    let es = EnergyStore::with_substrate(config(), sub.clone());

    // Sous le plafond : additif.
    es.boost(a, 0.5);
    es.boost(a, 0.2);
    let sous = es.get_energy(a);
    // Exactement au plafond : 0,5 + 0,5 = 1,0.
    es.boost(b, 0.5);
    es.boost(b, 0.5);
    let au = es.get_energy(b);
    // Au-dessus : 0,9 + 0,5 clampe à 1,0, et le boost suivant aussi.
    es.boost(c, 0.9);
    es.boost(c, 0.5);
    let au_dessus = es.get_energy(c);
    es.boost(c, 0.5);
    let encore = es.get_energy(c);

    println!(
        "SATURATION : sous_plafond={sous} au_plafond={au} au_dessus={au_dessus} reboost={encore}"
    );
    // Tolérance = la quantification Q1.15 (1/32768 ≈ 3,05e-5) : la colonne ne
    // sait pas porter 0,7 exactement, et chaque boost arrondit.
    assert!(
        (sous - 0.7).abs() < 1e-4,
        "sous le plafond la valeur s'accumule : {sous}"
    );
    assert!((au - 1.0).abs() < 1e-6, "au plafond : {au}");
    // CHOIX ASSUMÉ : au-delà du plafond la colonne perd l'information (elle ne
    // peut pas dépasser 1,0). On ne la masque pas en trichant sur l'énergie ni
    // sur l'ancre : la valeur est honnêtement 1,0, et ce qui distingue un nœud
    // saturé d'un nœud fraîchement boosté est sa date d'activation, pas son
    // énergie. Un nœud saturé REDEVIENT lisible en décroissant.
    assert!(
        (au_dessus - 1.0).abs() < 1e-6,
        "au-delà du plafond : {au_dessus}"
    );
    assert!(
        (encore - 1.0).abs() < 1e-6,
        "un second boost saturé ne s'accumule pas : {encore}"
    );
    ancre_il_y_a(&sub, c, 1.0);
    let apres_une_demi_vie = es.get_energy(c);
    println!("SATURATION : saturé puis 1 demi-vie plus tard = {apres_une_demi_vie}");
    assert!(
        (apres_une_demi_vie - 0.5).abs() < 1e-3,
        "un nœud saturé se décroche : {apres_une_demi_vie}"
    );
}

// ---------------------------------------------------------------------------
// Pas de masquage : plan juste ≠ plan faux
// ---------------------------------------------------------------------------

#[test]
fn un_plan_faux_et_un_plan_juste_donnent_des_resultats_differents() {
    // 6 heures écoulées, demi-vie 24 h.
    let law = DecayLaw::new(Duration::from_secs(24 * 3600));
    let juste = law.energy_at(1.0, Duration::from_secs(6 * 3600));
    // Le câblage naïf du stub : 0,99 toutes les 60 s, 360 fois.
    let faux = 0.99_f64.powi(360);
    println!(
        "PLANS : juste(6h, H=24h)={juste} faux(0,99^360)={faux} écart={}",
        (juste - faux).abs()
    );
    assert!(
        (juste - 0.8409).abs() < 1e-3,
        "la loi de 24 h perd 16 % en 6 h : {juste}"
    );
    assert!(faux < 0.05, "le facteur fixe a tout mangé en 6 h : {faux}");
    // Aucun masquage possible : les deux nombres ne peuvent pas être confondus.
    assert!((juste - faux).abs() > 0.7);
    // Et l'écart est exactement le rapport de demi-vies (20,9×).
    let h_faux = DecayLaw::implied_half_life(0.99, Duration::from_secs(60));
    assert!(
        (h_faux.as_secs_f64() - 4139.0).abs() < 10.0,
        "H_eff = {h_faux:?}"
    );
}

// ---------------------------------------------------------------------------
// Mesure verbatim (lancée avec --nocapture pour le rapport)
// ---------------------------------------------------------------------------

#[test]
fn mesures_verbatim_des_trois_regimes() {
    let td = tempfile::tempdir().unwrap();
    let sub = store(&td, "mesure");
    let id = sub.create_node(&["n"]);
    let es = EnergyStore::with_substrate(config(), sub.clone());
    let law = DecayLaw::new(H);
    let plans: [(&str, f32, f64); 4] = [
        ("R1 sous le seuil 0.008, 1 demi-vie", 0.008, 1.0),
        ("R2 au seuil 0.01, 1 demi-vie", 0.01, 1.0),
        ("R3 au-dessus 0.5, 1 demi-vie", 0.5, 1.0),
        (
            "R3 au-dessus 0.5, log2(50)=5.6438 demi-vies (pile au seuil)",
            0.5,
            law.half_lives_to(0.5, 0.01),
        ),
    ];
    for (nom, pose, demi_vies) in plans {
        sub.set_node_energy_f32(id, pose).unwrap();
        let colonne = sub.get_node_energy_f32(id).unwrap().unwrap() as f64;
        ancre_il_y_a(&sub, id, demi_vies);
        let lu = es.get_energy(id);
        println!(
            "MESURE {nom} : posé {pose} -> colonne {colonne:.9} -> lu {lu:.9} (demi-vies {demi_vies:.4})"
        );
    }
    // L'aller-retour d'unité (micros <-> secondes epoch f64) ne perd pas le micro.
    let t = unix_micros();
    assert!((epoch_secs_to_micros(micros_to_epoch_secs(t)) - t).abs() <= 1);
}

// ---------------------------------------------------------------------------
// Coût d'une lecture : une lecture de colonne + une lecture d'ancre
// ---------------------------------------------------------------------------

/// Mesure le surcoût de l'ancre. Ne prétend pas être un banc : borne
/// seulement l'explosion (le vrai chiffre est imprimé pour le rapport).
#[test]
fn une_lecture_d_energie_ne_coute_pas_plus_qu_un_acces_memoire() {
    let td = tempfile::tempdir().unwrap();
    let sub = store(&td, "cout");
    let mut ids = Vec::new();
    for _ in 0..256 {
        let id = sub.create_node(&["n"]);
        sub.set_node_energy_f32(id, 0.5).unwrap();
        ancre_il_y_a(&sub, id, 0.5);
        ids.push(id);
    }
    let es = EnergyStore::with_substrate(config(), sub.clone());

    const N: usize = 20_000;
    let t0 = std::time::Instant::now();
    let mut somme = 0.0f64;
    for i in 0..N {
        somme += sub
            .get_node_energy_f32(ids[i % ids.len()])
            .unwrap()
            .unwrap() as f64;
    }
    let colonne = t0.elapsed();

    let t1 = std::time::Instant::now();
    for i in 0..N {
        somme += es.get_energy(ids[i % ids.len()]);
    }
    let avec_ancre = t1.elapsed();

    let par_colonne = colonne.as_secs_f64() * 1e9 / N as f64;
    let par_lecture = avec_ancre.as_secs_f64() * 1e9 / N as f64;
    println!(
        "COÛT sur {N} lectures (256 nœuds, ancre à 0.5 demi-vie) : colonne seule {par_colonne:.0} ns/lecture, get_energy (colonne + ancre) {par_lecture:.0} ns/lecture, facteur {:.1}×  (somme={somme:.1})",
        par_lecture / par_colonne
    );
    assert!(
        par_lecture < 20_000.0,
        "lecture pathologique : {par_lecture:.0} ns"
    );
}
