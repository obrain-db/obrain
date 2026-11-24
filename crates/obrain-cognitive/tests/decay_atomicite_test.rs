//! ATOMICITÉ de (énergie de référence, ancre) — point 1 de la revue.
//!
//! Le correctif précédent écrivait l'énergie (colonne) PUIS l'ancre
//! (propriété) : deux écritures. Un `kill -9` entre les deux exposait un
//! couple incohérent — `(nouvelle énergie, ancienne ancre)` = 0,375 ou
//! `(ancienne énergie, nouvelle ancre)` = 1,0 — au lieu de 0,5 ou 0,75.
//!
//! Le correctif porte l'état temporel dans UNE SEULE propriété composite
//! `(E_ref, t_ref, t_hi, H)`. La colonne n'est plus qu'un miroir best-effort.
//! Ces tests mesurent les quatre combinaisons et l'état disque d'un kill entre
//! la propriété et le miroir.

#![cfg(feature = "substrate")]

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{DecayLaw, unix_micros};
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;
use std::time::Duration;

const H: Duration = Duration::from_secs(3600);

fn law() -> DecayLaw {
    DecayLaw::new(H)
}

/// Le tableau des quatre combinaisons (colonne, ancre) mesuré au temps `t = H`.
///
/// Dans l'ancien code, deux combinaisons étaient INVALIDES (0,375 et 1,0).
/// Ici l'ancre composite est la source de vérité : les combinaisons déchirées
/// se résolvent sur l'ancre, donc seules 0,5 et 0,75 sont exposées.
#[test]
fn les_quatre_combinaisons_n_exposent_que_des_etats_valides() {
    let td = tempfile::tempdir().unwrap();
    let sub = Arc::new(SubstrateStore::create(td.path().join("atom")).unwrap());
    let id = sub.create_node(&["n"]);
    let t0 = unix_micros();
    let t_h = t0 + H.as_micros() as i64;

    let mut vus = Vec::new();
    let mut combinaison = |col: f32, e_ref: f64, t_ref: i64| -> f64 {
        sub.set_node_energy_f32(id, col).unwrap();
        decay_clock::persist_anchor_explicit(&sub, id, e_ref, t_ref, H);
        decay_clock::energy_at(&sub, id, &law(), t_h).unwrap()
    };

    let ancienne_ancienne = combinaison(1.0, 1.0, t0); // état d'avant
    let nouvelle_nouvelle = combinaison(0.75, 0.75, t_h); // état d'après
    let nouvelle_ancienne = combinaison(0.75, 1.0, t0); // déchiré (exposait 0,375)
    let ancienne_nouvelle = combinaison(1.0, 0.75, t_h); // déchiré (exposait 1,0)

    vus.extend([
        ancienne_ancienne,
        nouvelle_nouvelle,
        nouvelle_ancienne,
        ancienne_nouvelle,
    ]);

    println!(
        "COMBINAISONS : avant={ancienne_ancienne} après={nouvelle_nouvelle} \
         déchiré(nouvelle/ancienne)={nouvelle_ancienne} déchiré(ancienne/nouvelle)={ancienne_nouvelle}"
    );

    assert!(
        (ancienne_ancienne - 0.5).abs() < 1e-9,
        "avant : {ancienne_ancienne}"
    );
    assert!(
        (nouvelle_nouvelle - 0.75).abs() < 1e-9,
        "après : {nouvelle_nouvelle}"
    );
    // Les deux combinaisons déchirées ne sont plus 0,375 ni 1,0 :
    assert!(
        (nouvelle_ancienne - 0.5).abs() < 1e-9,
        "colonne neuve + ancre ancienne → 0,5 (l'ancre prime), mesuré {nouvelle_ancienne}"
    );
    assert!(
        (ancienne_nouvelle - 0.75).abs() < 1e-9,
        "colonne ancienne + ancre neuve → 0,75 (l'ancre prime), mesuré {ancienne_nouvelle}"
    );
    // Aucune valeur invalide n'a été exposée.
    for v in vus {
        assert!(
            (v - 0.5).abs() < 1e-9 || (v - 0.75).abs() < 1e-9,
            "valeur invalide exposée : {v}"
        );
    }
}

/// Un `kill -9` entre l'écriture de l'ancre composite et le miroir de colonne
/// (état disque reproduit par `drop` sans flush, comme dans
/// `obrain-substrate/tests/atomicite_proprietes_tx.rs`) laisse la lecture sur
/// l'ancre : 0,75 — jamais 0,375 ni 1,0.
#[test]
fn un_kill_entre_ancre_et_miroir_laisse_une_valeur_valide() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("kill");

    let t0 = unix_micros();
    let t_h = t0 + H.as_micros() as i64;
    let id;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).unwrap());
        id = sub.create_node(&["n"]);
        // État d'avant : (E_ref = 1, t_ref = t0).
        sub.set_node_energy_f32(id, 1.0).unwrap();
        decay_clock::persist_anchor_explicit(&sub, id, 1.0, t0, H);
        // `reinforce` à t = H ajoute 0,25 : l'ancre composite passe à
        // (0,75, t_h) — UNE mutation. On simule le kill JUSTE après, sans
        // écrire le miroir de colonne (qui reste 1,0).
        decay_clock::persist_anchor_explicit(&sub, id, 0.75, t_h, H);
        // PAS de set_node_energy_f32 : le miroir reste à l'ancienne valeur.
        drop(sub); // = kill -9 sur l'état disque
    }

    let sub = Arc::new(SubstrateStore::open(&chemin).unwrap());
    let colonne = sub.get_node_energy_f32(id).unwrap().unwrap() as f64;
    let lue = decay_clock::energy_at(&sub, id, &law(), t_h).unwrap();
    println!("KILL : colonne(miroir périmé)={colonne} lue(ancre)={lue}");
    assert!(
        (colonne - 1.0).abs() < 1e-6,
        "le miroir n'a pas été écrit : {colonne}"
    );
    assert!(
        (lue - 0.75).abs() < 1e-9,
        "la lecture expose l'ancre (0,75), pas le miroir : {lue}"
    );
}
