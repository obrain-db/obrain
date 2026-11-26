#![allow(clippy::many_single_char_names)]
//! (2026-11-24) TESTS DE LA COMBINAISON — décroissance persistante × dédup synapses.
//!
//! Ces tests n appartiennent à AUCUNE des deux branches : ils n ont de sens
//! qu une fois `feat/persistent-decay-clock` (57b...) et
//! `fix/synapse-double-count` réunis. Ils vérifient que les deux correctifs ne
//! se neutralisent pas :
//!
//!   1. `for_each_synapse` émet UNE fois par arête sur une base rouverte,
//!      pendant que la machinerie de décroissance est active (ancres
//!      composites, tique) ;
//!   2. `normalize_outgoing` persiste ses poids ET la décroissance d énergie
//!      ne les écrase pas (colonnes distinctes : nœud vs arête) ;
//!   3. une arête normalisée, relue après fermeture/réouverture, rend les
//!      mêmes poids à INSTANT CONTRÔLÉ (`weight_at(instant)`) — la loi
//!      `2^(-Δt/H)` est vérifiée à la demi-vie près, sans horloge murale ;
//!   4. les deux états persistés (énergie de nœud par ancre composite, poids
//!      de synapse par colonne) survivent ENSEMBLE à une seule réouverture.
//!
//! Run: cargo test --release -p obrain-cognitive \
//!        --test combinaison_decroissance_synapses \
//!        --features substrate,engram,thinker -- --nocapture
#![cfg(all(feature = "synapse", feature = "substrate", feature = "energy"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{DecayLaw, unix_micros};
use obrain_cognitive::energy::{EnergyConfig, EnergyStore};
use obrain_cognitive::synapse::{SynapseConfig, SynapseStore};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;

const H_ENERGIE: Duration = Duration::from_secs(3600);
const H_SYN: Duration = Duration::from_secs(7 * 24 * 3600);

/// Config de synapse qui déclenche la normalisation dès deux arêtes de 0,7
/// (`max_total_outgoing_weight = 1.0 < 1.4`).
fn config_syn() -> SynapseConfig {
    SynapseConfig {
        max_total_outgoing_weight: 1.0,
        max_synapse_weight: 10.0,
        initial_weight: 0.1,
        default_half_life: H_SYN,
        ..SynapseConfig::default()
    }
}

fn config_energie() -> EnergyConfig {
    EnergyConfig {
        default_half_life: H_ENERGIE,
        ..Default::default()
    }
}

/// Voisinage synapse de `node` : voisin -> liste des poids ÉMIS.
fn emissions(store: &SynapseStore, node: NodeId) -> BTreeMap<u64, Vec<f64>> {
    let mut out: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
    store.for_each_synapse(node, |n, w| {
        out.entry(n.0).or_default().push(w);
    });
    out
}

/// Poids bruts (colonne Q0.16 × max_synapse_weight) de toutes les arêtes
/// SYNAPSE, indexés par paire canonique (min, max). Lecture de la COLONNE,
/// pas du cache : c est la preuve de durabilité.
fn poids_colonne(sub: &SubstrateStore) -> BTreeMap<(u64, u64), f64> {
    let max_w = 10.0_f64;
    let mut out = BTreeMap::new();
    if let Ok(rows) = sub.scan_typed_edge_weights("SYNAPSE") {
        for (_eid, src, dst, w) in rows {
            let (s, d) = (src.0, dst.0);
            let key = if s <= d { (s, d) } else { (d, s) };
            out.insert(key, (w as f64) * max_w);
        }
    }
    out
}

/// Étoile hub—{a,b} : deux arêtes, toutes deux persistées.
fn etoile(gs: &Arc<dyn GraphStoreMut>) -> (NodeId, NodeId, NodeId) {
    let hub = gs.create_node(&["n"]);
    let a = gs.create_node(&["n"]);
    let b = gs.create_node(&["n"]);
    (hub, a, b)
}

// ---------------------------------------------------------------------------
// 1. UNE émission par arête sur base rouverte, décroissance active
// ---------------------------------------------------------------------------
#[test]
fn une_emission_par_arete_sur_base_rouverte_avec_decroissance_active() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");

    let (hub, a, b);
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn GraphStoreMut> = sub.clone();
        let (h, x, y) = etoile(&gs);
        (hub, a, b) = (h, x, y);

        // Les deux sous-systèmes travaillent sur la MÊME base.
        let syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));
        let nrg = EnergyStore::with_substrate(config_energie(), Arc::clone(&sub));
        syn.reinforce(hub, a, 0.6);
        syn.reinforce(hub, b, 0.6); // déclenche la normalisation
        nrg.boost(hub, 1.0);
        nrg.boost(a, 0.8);
        // Tique de décroissance RÉELLE avant fermeture (ancre composite écrite).
        let avances = nrg.decay_tick(0);
        println!("[session 1] noeuds avances par la tique = {avances}");
        syn.flush_pending_metadata(); // persiste l epoch de last_reinforced
        assert_eq!(syn.len(), 2, "deux synapses persistees");
    } // fermeture

    // ── Réouverture ────────────────────────────────────────────────────
    let sub = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));

    let vu = emissions(&syn, hub);
    println!("[rouvert] emissions du voisinage de {hub:?} = {vu:?}");
    assert_eq!(
        vu.keys().copied().collect::<Vec<_>>(),
        vec![a.0, b.0],
        "le voisinage doit etre exactement {{a, b}} — mesure : {vu:?}"
    );
    for (n, poids) in &vu {
        assert_eq!(
            poids.len(),
            1,
            "voisin {n} emis {} fois (attendu 1) — la combinaison doit conserver \
             la deduplication de for_each_synapse",
            poids.len()
        );
    }
    // Le contrat de dédup vaut aussi pour la somme : deux émissions
    // doubleraient l énergie propagée par `spread` (qui ADDITIONNE).
    let total_emis: f64 = vu.values().flatten().sum();
    let colonne = poids_colonne(&sub);
    let total_colonne: f64 = colonne.values().sum();
    println!("[rouvert] total emis = {total_emis:.6}  total colonne = {total_colonne:.6}");
    assert!(
        (total_emis - total_colonne).abs() < 1e-3,
        "la somme emise doit egaler la somme en colonne (une emission par arete) \
         — emis {total_emis:.6} vs colonne {total_colonne:.6}"
    );
}

// ---------------------------------------------------------------------------
// 2. La décroissance d énergie n écrase pas les poids normalisés
// ---------------------------------------------------------------------------
#[test]
fn la_decroissance_d_energie_n_ecrase_pas_les_poids_normalises() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");

    let (hub, a, b);
    let colonne_avant_tique;
    let colonne_apres_tique;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn GraphStoreMut> = sub.clone();
        (hub, a, b) = etoile(&gs);
        let syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));
        let nrg = EnergyStore::with_substrate(config_energie(), Arc::clone(&sub));

        syn.reinforce(hub, a, 0.6);
        syn.reinforce(hub, b, 0.6); // normalisation : 0,7 -> 0,5 par arête
        nrg.boost(hub, 1.0);
        nrg.boost(a, 1.0);

        colonne_avant_tique = poids_colonne(&sub);
        // La tique de décroissance d énergie parcourt TOUS les nœuds vivants
        // et réécrit leur colonne d énergie. Elle ne doit pas toucher l arête.
        let avances = nrg.decay_tick(0);
        colonne_apres_tique = poids_colonne(&sub);
        println!(
            "[session 1] tique = {avances} noeuds ; colonne avant = {colonne_avant_tique:?} ; apres = {colonne_apres_tique:?}"
        );
        assert_eq!(
            colonne_avant_tique, colonne_apres_tique,
            "la tique de decroissance d energie a modifie les poids de synapse"
        );

        // Contrat numérique : chaque arête normalisée vaut 0,5 (0,7 × 1/1,4),
        // la colonne porte 0,5/10 = 0,05.
        for (paire, w) in &colonne_avant_tique {
            assert!(
                (w - 0.5).abs() < 1e-3,
                "arete {paire:?} : normalisee a {w:.6}, attendu 0,5"
            );
        }
        syn.flush_pending_metadata();
    } // fermeture

    // ── Réouverture : les deux états sont relus ────────────────────────
    let sub = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let _syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));
    let nrg = EnergyStore::with_substrate(config_energie(), Arc::clone(&sub));

    let colonne = poids_colonne(&sub);
    println!("[rouvert] colonne = {colonne:?}");
    for (paire, w) in &colonne {
        assert!(
            (w - 0.5).abs() < 1e-3,
            "arete {paire:?} relue a {w:.6} apres reouverture — la normalisation \
             n a pas survecu (attendu 0,5)"
        );
    }
    // Et l énergie de nœud est, elle, bien passée par l ancre composite.
    for n in [hub, a] {
        let ancre = decay_clock::read_anchor(&sub, n);
        assert!(
            ancre.is_some(),
            "le noeud {n:?} doit porter une ancre composite apres la tique"
        );
        let e = nrg.get_energy(n);
        println!("[rouvert] energie de {n:?} = {e:.6} (ancre = {ancre:?})");
        assert!(e > 0.0 && e <= 1.0, "energie hors bornes : {e}");
    }
}

// ---------------------------------------------------------------------------
// 3. Poids identiques à INSTANT CONTRÔLÉ après fermeture/réouverture
// ---------------------------------------------------------------------------
#[test]
fn arete_normalisee_relue_apres_reouverture_meme_poids_a_instant_controle() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");

    let (hub, a, b);
    let (ab_avant, ac_avant);
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn GraphStoreMut> = sub.clone();
        (hub, a, b) = etoile(&gs);
        let syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));
        syn.reinforce(hub, a, 0.6);
        syn.reinforce(hub, b, 0.6);
        ab_avant = syn.get_synapse(hub, a).map(|s| s.raw_weight()).unwrap();
        ac_avant = syn.get_synapse(hub, b).map(|s| s.raw_weight()).unwrap();
        println!("[avant fermeture] hub-a={ab_avant:.6} hub-b={ac_avant:.6}");
        syn.flush_pending_metadata();
    }

    let sub = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));

    for (nom, n, avant) in [("hub-a", a, ab_avant), ("hub-b", b, ac_avant)] {
        let s = syn
            .get_synapse(hub, n)
            .unwrap_or_else(|| panic!("{nom} doit etre relue apres reouverture"));
        let brut = s.raw_weight();
        let t = s.last_reinforced();
        // INSTANT CONTRÔLÉ : `weight_at` prend l instant en paramètre, donc la
        // loi est vérifiée sans horloge murale ni sleep.
        let a_t = s.weight_at(t);
        let a_t_plus_h = s.weight_at(t + H_SYN);
        let a_t_plus_2h = s.weight_at(t + 2 * H_SYN);
        println!(
            "[rouvert] {nom} : brut relu={brut:.6} (avant {avant:.6}) ; \
             weight_at(t)={a_t:.6} ; weight_at(t+H)={a_t_plus_h:.6} ; weight_at(t+2H)={a_t_plus_2h:.6}"
        );

        assert!(
            (brut - avant).abs() < 1e-3,
            "{nom} : brut normalise a {avant:.6} puis relu {brut:.6} — la \
             normalisation n est pas durable sous la combinaison"
        );
        assert!(
            (a_t - brut).abs() < 1e-12,
            "{nom} : weight_at(t) doit valoir le poids brut {brut:.9}, mesure {a_t:.9}"
        );
        assert!(
            (a_t_plus_h - brut / 2.0).abs() < 1e-9,
            "{nom} : a une demi-vie, weight_at doit valoir brut/2 = {:.9}, mesure {a_t_plus_h:.9}",
            brut / 2.0
        );
        assert!(
            (a_t_plus_2h - brut / 4.0).abs() < 1e-9,
            "{nom} : a deux demi-vies, weight_at doit valoir brut/4 = {:.9}, mesure {a_t_plus_2h:.9}",
            brut / 4.0
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Les deux états persistent ENSEMBLE, lus à instant injecté
// ---------------------------------------------------------------------------
#[test]
fn energie_de_noeud_et_poids_normalise_survivent_ensemble_a_la_reouverture() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");

    let (hub, a, _b);
    let t0;
    {
        let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
        let gs: Arc<dyn GraphStoreMut> = sub.clone();
        let (h, x, y) = etoile(&gs);
        (hub, a, _b) = (h, x, y);
        t0 = unix_micros();

        // Énergie : ancre composite EXPLICITE (E_ref=1, t_ref=t0, H figée).
        decay_clock::persist_anchor_explicit(&sub, hub, 1.0, t0, H_ENERGIE);
        sub.set_node_energy_f32(hub, 1.0).unwrap();

        // Synapses : normalisation + persistance.
        let syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));
        syn.reinforce(hub, a, 0.6);
        syn.reinforce(hub, y, 0.6);
        syn.flush_pending_metadata();
    }

    let sub = Arc::new(SubstrateStore::open(&chemin).expect("open"));
    let gs: Arc<dyn GraphStoreMut> = sub.clone();
    let _syn = SynapseStore::with_substrate(config_syn(), Arc::clone(&gs), Arc::clone(&sub));
    let law = DecayLaw::new(H_ENERGIE);

    // Énergie à instant INJECTÉ : exactement une demi-vie après t0.
    let e_1h = decay_clock::energy_at(&sub, hub, &law, t0 + H_ENERGIE.as_micros() as i64).unwrap();
    let e_0 = decay_clock::energy_at(&sub, hub, &law, t0).unwrap();
    // Poids de synapse relu (colonne).
    let colonne = poids_colonne(&sub);
    println!("[combinaison] energie(t0)={e_0:.9} energie(t0+H)={e_1h:.9} colonne={colonne:?}");

    assert!(
        (e_0 - 1.0).abs() < 1e-9,
        "l ancre doit rendre E_ref a t_ref : {e_0}"
    );
    assert!(
        (e_1h - 0.5).abs() < 1e-9,
        "a une demi-vie, l ancre doit rendre 0,5 : {e_1h}"
    );
    assert_eq!(
        colonne.len(),
        2,
        "les deux aretes doivent etre relues : {colonne:?}"
    );
    for (paire, w) in &colonne {
        assert!(
            (w - 0.5).abs() < 1e-3,
            "arete {paire:?} relue {w:.6}, attendu 0,5"
        );
    }
}
