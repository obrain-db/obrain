//! (2026-11-24) CONTRAT DE DURABILITÉ DE `normalize_outgoing`.
//!
//! INVARIANT À ÉTABLIR. Pour une même version logique et un même instant, les
//! chemins publics (`get_synapse`, `list_synapses`, `for_each_synapse`)
//! produisent des résultats compatibles avec le contrat numérique annoncé :
//! la normalisation compétitive de Hebb est un APPRENTISSAGE DURABLE — les
//! poids normalisés doivent être retrouvés APRÈS RÉOUVERTURE, pas seulement
//! présents dans le cache chaud de la session qui a normalisé.
//!
//! LE DÉFAUT CONCRET. `normalize_outgoing` applique `scale_weight` aux entrées
//! EN MÉMOIRE (`self.synapses`), sans persister la colonne substrate des arêtes
//! qu'il réduit — sauf l'arête courante, réécrite par le write-through de
//! `reinforce` APRÈS normalisation. Les autres arêtes du voisinage gardent leur
//! ancien poids en colonne : à la réouverture, `load_from_substrate_column`
//! réhydrate le cache depuis la colonne et la normalisation est PERDUE pour
//! elles. Mesuré : A relié à B et C, deux `reinforce` qui déclenchent la
//! normalisation → B repasse de 0.5 à 0.7 à la réouverture.
//!
//! CE QUE CE TEST MESURE. Le poids brut (sans décroissance, `raw_weight()` =
//! l'instant contrôlé) de chaque arête, relevé via le chemin public
//! `get_synapse`, avant fermeture puis après réouverture.
//!
//! Run: cargo test --release -p obrain-cognitive --test normalize_outgoing_durabilite
//!        --features synapse,substrate -- --nocapture
#![cfg(all(feature = "synapse", feature = "substrate"))]

use std::sync::Arc;

use obrain_cognitive::synapse::{SynapseConfig, SynapseStore};
use obrain_substrate::SubstrateStore;

/// Config qui déclenche la normalisation dès deux arêtes de 0.7 :
/// `max_total_outgoing_weight = 1.0` < 1.4.
fn config_normalisante() -> SynapseConfig {
    SynapseConfig {
        max_total_outgoing_weight: 1.0,
        max_synapse_weight: 10.0,
        initial_weight: 0.1,
        ..SynapseConfig::default()
    }
}

#[test]
fn normalisation_est_retrouvee_apres_reouverture() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");

    // ── Session 1 : construction + normalisation ────────────────────────
    let (a, b, c, ab_avant, ac_avant) = {
        let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
        let a = gs.create_node(&["n"]);
        let b = gs.create_node(&["n"]);
        let c = gs.create_node(&["n"]);
        let store = SynapseStore::with_substrate(config_normalisante(), gs, substrate);
        store.reinforce(a, b, 0.6);
        store.reinforce(a, c, 0.6); // déclenche la normalisation de {AB, AC}

        // Relever les poids normalisés AVANT fermeture (instant contrôlé :
        // `raw_weight()`, sans décroissance).
        let ab_avant = store.get_synapse(a, b).map(|s| s.raw_weight()).unwrap();
        let ac_avant = store.get_synapse(a, c).map(|s| s.raw_weight()).unwrap();
        println!("[avant fermeture] AB={ab_avant:.6}  AC={ac_avant:.6}");
        (a, b, c, ab_avant, ac_avant)
    }; // store + substrat détruits ici : c'est la fermeture.

    // ── Session 2 : réouverture ─────────────────────────────────────────
    let substrate = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
    let store = SynapseStore::with_substrate(config_normalisante(), gs, substrate);

    let ab_apres = store.get_synapse(a, b).map(|s| s.raw_weight()).unwrap();
    let ac_apres = store.get_synapse(a, c).map(|s| s.raw_weight()).unwrap();
    println!("[apres reouverture] AB={ab_apres:.6}  AC={ac_apres:.6}");

    // CONTRAT DURABLE : les poids normalisés sont retrouvés après réouverture.
    // Tolérance 1e-3 : la colonne est Q0.16 (`weight_u16/65535`, ×10 par
    // `max_synapse_weight`) — la quantification virgule fixe introduit un
    // résidu mesurable (~4e-5 ici, documenté dans la synthèse 24/11 :
    // « un poids 0.300000 devient 0.299992 »). « Durable » ne veut pas dire
    // « bit-identique », mais « à la quantification près ». Le défaut, lui,
    // était de 0,2 (0,5 → 0,7) : deux ordres de grandeur au-dessus.
    assert!(
        (ab_avant - ab_apres).abs() < 1e-3,
        "AB : normalisé à {ab_avant:.6} puis relu {ab_apres:.6} — la normalisation n'est pas durable"
    );
    assert!(
        (ac_avant - ac_apres).abs() < 1e-3,
        "AC : normalisé à {ac_avant:.6} puis relu {ac_apres:.6} — la normalisation n'est pas durable"
    );

    // INVARIANT NUMÉRIQUE : la somme des poids sortants reste ≤ max_total.
    // La normalisation impose `somme = max_total` en f64, mais la colonne
    // Q0.16 arrondit AU PLUS PROCHE : chaque poids peut être arrondi vers le
    // HAUT, et la somme réouverte dépasse le plafond de ~deg × ½ ulp
    // (~7,6e-5 ici). C'est un artefact de quantification PRÉEXISTANT (le
    // write-through de `reinforce` arrondit déjà au plus proche), pas un
    // défaut du correctif : la tolérance ci-dessous absorbe l'ulp, pas le
    // défaut — qui lui était de 0,2.
    let somme: f64 = [ab_apres, ac_apres].iter().sum();
    assert!(
        somme <= 1.0 + 1e-3,
        "somme sortante {somme:.6} dépasse max_total_outgoing_weight=1.0 après réouverture"
    );
}
