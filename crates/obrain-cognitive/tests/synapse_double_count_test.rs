//! (2026-11-23) RÉGRESSION — `for_each_synapse` émettait chaque arête DEUX fois
//! dès que la base avait été rouverte depuis le substrat.
//!
//! LE DÉFAUT. `SynapseStore::for_each_synapse` itère deux sources :
//!   Deux sources : l'index d'incidence persistant (`substrate.synapse_edges_of`
//!   + `read_edge`), alimenté par `create_synapse_slot(..)` pour **src ET dst**,
//!   et la sur-couche chaude `node_to_keys` + DashMap `synapses`.
//! Sur une base fraîchement ouverte, `load_from_substrate_column()` hydrate la
//! DashMap ET `node_to_keys` : les deux sources couvrent alors exactement le
//! même ensemble, et le callback est invoqué 2x par voisin. Le spread
//! (`activation.rs`, `*activation.entry(neighbor).or_insert(0.0) += propagated`)
//! ADDITIONNE : l'énergie propagée doublait sur toute base rechargée, et ne
//! doublait pas sur une base en mémoire.
//!
//! CE QUE CES TESTS MESURENT. Le nombre de visites PAR VOISIN de
//! `for_each_synapse`, sur le même graphe (3 nœuds, 2 arêtes) construit
//! - en mémoire pure (`SynapseStore::new`, pas de substrat), et
//! - sur substrat, fermé puis ROUVERT (le chemin du redémarrage réel).
//!
//! Run: cargo test -p obrain-cognitive --test synapse_double_count_test
//!        --features synapse,substrate -- --nocapture
#![cfg(all(feature = "synapse", feature = "substrate"))]

use std::collections::BTreeMap;
use std::sync::Arc;

use obrain_cognitive::synapse::{SynapseConfig, SynapseStore};
use obrain_common::types::NodeId;
use obrain_substrate::SubstrateStore;

/// Compte les visites par voisin du voisinage synapse de `node`.
fn visites(store: &SynapseStore, node: NodeId) -> BTreeMap<u64, usize> {
    let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
    store.for_each_synapse(node, |n, _w| {
        *counts.entry(n.0).or_insert(0) += 1;
    });
    counts
}

/// Construit le petit graphe de référence : 1 — 2 — 3.
/// Retourne les trois NodeId alloués dans le graph store.
fn trois_noeuds_deux_aretes(
    gs: &Arc<dyn obrain_core::graph::GraphStoreMut>,
) -> (NodeId, NodeId, NodeId) {
    let a = gs.create_node(&["n"]);
    let b = gs.create_node(&["n"]);
    let c = gs.create_node(&["n"]);
    (a, b, c)
}

/// TÉMOIN — base EN MÉMOIRE (aucun substrat) : chaque voisin est visité 1 fois.
/// Ce test passe sur le code fautif : il fixe la sémantique attendue.
#[test]
fn memoire_chaque_voisin_est_visite_une_fois() {
    let store = SynapseStore::new(SynapseConfig::default());
    let (a, b, c) = (NodeId(1), NodeId(2), NodeId(3));
    store.reinforce(a, b, 0.25);
    store.reinforce(b, c, 0.25);

    let vu = visites(&store, b);
    println!("[memoire] visites du voisinage de {b:?} = {vu:?}");
    assert_eq!(
        vu,
        BTreeMap::from([(a.0, 1usize), (c.0, 1usize)]),
        "en memoire, chaque voisin doit etre visite exactement 1 fois — mesure : {vu:?}"
    );
}

/// ROUGE SUR LE CODE FAUTIF — base fermée puis ROUVERTE depuis le substrat.
/// Le voisinage doit être identique au témoin en mémoire : 1 visite par voisin.
#[test]
fn substrat_rouvert_chaque_voisin_est_visite_une_fois() {
    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");

    // ── Session 1 : construction + persistance ──────────────────────────
    let (a, b, c) = {
        let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create substrate"));
        let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
        let (a, b, c) = trois_noeuds_deux_aretes(&gs);
        let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
        store.reinforce(a, b, 0.25);
        store.reinforce(b, c, 0.25);
        // La synapse doit exister sur la colonne avant la fermeture : c'est
        // `create_synapse_slot` (via `reinforce` -> `ensure_edge`) qui écrit le
        // slot ET ses deux postings d'incidence (src, dst).
        assert_eq!(store.len(), 2, "deux synapses doivent etre persistees");
        (a, b, c)
    }; // store + substrat détruits ici : c'est la fermeture.

    // ── Session 2 : réouverture depuis le substrat ──────────────────────
    let substrate = Arc::new(SubstrateStore::open(&chemin).expect("open substrate"));
    let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
    assert_eq!(
        store.len(),
        2,
        "les deux synapses doivent etre relues depuis la colonne"
    );

    let vu = visites(&store, b);
    println!("[substrat rouvert] visites du voisinage de {b:?} = {vu:?}");
    assert_eq!(
        vu,
        BTreeMap::from([(a.0, 1usize), (c.0, 1usize)]),
        "apres reouverture, chaque voisin doit etre visite exactement 1 fois \
         (meme contrat qu'en memoire) — mesure : {vu:?}"
    );
}

/// Le même graphe, en mémoire et rouvert, doit rendre le MÊME voisinage —
/// c'est le contrat comparatif qui met le défaut en évidence.
#[test]
fn memoire_et_substrat_rouvert_rendent_le_meme_voisinage() {
    let memoire = SynapseStore::new(SynapseConfig::default());
    memoire.reinforce(NodeId(1), NodeId(2), 0.25);
    memoire.reinforce(NodeId(2), NodeId(3), 0.25);
    let vu_memoire = visites(&memoire, NodeId(2));

    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");
    {
        let substrate = Arc::new(SubstrateStore::create(&chemin).expect("create substrate"));
        let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
        let (a, b, c) = trois_noeuds_deux_aretes(&gs);
        let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
        store.reinforce(a, b, 0.25);
        store.reinforce(b, c, 0.25);
    }
    let substrate = Arc::new(SubstrateStore::open(&chemin).expect("open substrate"));
    let gs: Arc<dyn obrain_core::graph::GraphStoreMut> = substrate.clone();
    let store = SynapseStore::with_substrate(SynapseConfig::default(), gs, substrate);
    let vu_substrat = visites(&store, NodeId(2));

    println!("[memoire] = {vu_memoire:?}  [substrat rouvert] = {vu_substrat:?}");
    assert_eq!(
        vu_substrat, vu_memoire,
        "le voisinage d'une base rouverte doit etre identique a celui d'une base en memoire \
         — memoire : {vu_memoire:?}, substrat rouvert : {vu_substrat:?}"
    );
}
