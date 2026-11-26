//! Pont observation → engramme : nom, surprise par épisode (observable), et
//! traduction de CHAQUE variante de mutation en co-activation via `on_event`.
//! Couverture #414 — périmètre A : pont E2E du sous-système engramme.

#![cfg(feature = "engram")]

use std::sync::Arc;

use obrain_cognitive::InMemoryVectorIndex;
use obrain_cognitive::engram::{EngramManager, EngramManagerConfig, EngramObservationBridge};
use obrain_common::types::{EdgeId, NodeId};
use obrain_reactive::{EdgeSnapshot, MutationEvent, NodeSnapshot};

fn manager() -> Arc<EngramManager> {
    Arc::new(EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    ))
}

fn noeud(id: u64) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId(id),
        labels: Default::default(),
        properties: Vec::new(),
    }
}

fn arete(id: u64, src: u64, dst: u64) -> EdgeSnapshot {
    EdgeSnapshot {
        id: EdgeId(id),
        src: NodeId(src),
        dst: NodeId(dst),
        edge_type: Default::default(),
        properties: Vec::new(),
    }
}

#[test]
fn le_pont_expose_son_nom() {
    let mgr = manager();
    let bridge = EngramObservationBridge::new(Arc::clone(&mgr));
    assert_eq!(bridge.name(), "cognitive:engram");
}

/// Le constructeur `with_prediction_error` a un effet OBSERVABLE : une
/// surprise sous le seuil (`min_prediction_error`, défaut 0.3) ne franchit
/// jamais le gate de formation, quelle que soit la co-occurrence. C'est la
/// preuve que la valeur est réellement injectée, pas seulement stockée.
#[tokio::test]
async fn surprise_sous_le_seuil_ne_forme_pas_d_engramme() {
    let mgr = manager();
    let bridge = EngramObservationBridge::with_prediction_error(Arc::clone(&mgr), 0.0);

    for _ in 0..5 {
        bridge
            .on_batch(&[MutationEvent::EdgeCreated {
                edge: arete(1, 10, 11),
            }])
            .await;
    }

    assert!(
        mgr.store().list().is_empty(),
        "surprise cumulée 0.0 < 0.3 : aucun engramme ne doit se former"
    );
}

/// À surprise par défaut (0.5), la même co-occurrence franchit le seuil et
/// forme l'engramme — discrimine le chemin nominal.
#[tokio::test]
async fn on_event_traite_toutes_les_variantes_de_mutation() {
    let mgr = manager();
    let bridge = EngramObservationBridge::new(Arc::clone(&mgr));

    // Singletons : aucune formation possible, quel que soit le type.
    bridge
        .on_event(&MutationEvent::NodeCreated { node: noeud(1) })
        .await;
    bridge
        .on_event(&MutationEvent::NodeUpdated {
            before: noeud(2),
            after: noeud(2),
        })
        .await;
    bridge
        .on_event(&MutationEvent::NodeDeleted { node: noeud(3) })
        .await;

    // Arête {10,11} : chaque événement d'arête est SA PROPRE co-activation.
    for _ in 0..3 {
        bridge
            .on_event(&MutationEvent::EdgeCreated {
                edge: arete(1, 10, 11),
            })
            .await;
        bridge
            .on_event(&MutationEvent::EdgeUpdated {
                before: arete(2, 10, 11),
                after: arete(2, 10, 11),
            })
            .await;
        bridge
            .on_event(&MutationEvent::EdgeDeleted {
                edge: arete(3, 10, 11),
            })
            .await;
    }

    let engrams = mgr.store().list();
    assert!(
        engrams.iter().any(|e| {
            let mut ids: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
            ids.sort_unstable();
            ids == vec![10, 11]
        }),
        "un engramme {{10,11}} doit se former via on_event, engrams={engrams:?}"
    );
}
