//! EngramObservationBridge — pont E2E « observation → engramme » (feature `engram`).
//!
//! # Le trou que ce fichier bouche
//!
//! `EngramManager::record_activation` (formation Hebbienne + FSRS, Layer 0)
//! n'avait AUCUN appelant hors tests : la couche événementielle
//! (`CognitiveObserver` → `MutationBus` → `Scheduler` → listeners) portait la
//! vraie donnée (les mutations) mais ne la transmettait jamais au sous-système
//! engramme. La formation d'engrams était donc morte en production.
//!
//! Ce listener ferme la chaîne : chaque **événement de mutation** est une
//! co-activation **atomique** — une arête co-active {src,dst}, un nœud seul est
//! une activation (filtrée). Un LOT n'est JAMAIS fusionné en un épisode : deux
//! opérations indépendantes regroupées par le `Scheduler` restent distinctes.
//! Au bout de `min_episodes` co-occurrences du même ensemble (défaut 3), le
//! détecteur hebbien forme l'engramme et le persiste (substrate si attaché).
//!
//! # Pourquoi un pont dédié, et pas un appel depuis Energy/Synapse
//!
//! - **Gating propre** : ce module vit sous `#[cfg(feature = "engram")]`, sans
//!   faire dépendre `energy`/`synapse` de `engram`.
//! - **Sémantique claire** : un épisode de co-activation est « des nœuds touchés
//!   ensemble », pas « un nœud énergisé ».
//! - **Miroir d'`EpisodeRecorder`** (`episodic.rs`) : même motif « un lot = un
//!   épisode », mais ciblé engramme plutôt que mémoire épisodique.
//!
//! # Non-régression : pas de boucle
//!
//! `record_activation` → `form()` écrit dans les zones substrate
//! `engram_members`/`engram_bitset` (écritures de ZONE, pas des mutations
//! nœud/arête observées par `CognitiveObserver`) : la formation ne republie
//! donc pas d'événement et ne relance pas ce listener.
//!
//! # Coût
//!
//! Le dispatch est hors plan chaud (tâche `Scheduler`). `record_activation`
//! prend un `Mutex` sur le moteur de formation — même verrou que le rappel —
//! mais le travail est borné par la taille du lot (≤ `max_batch_size`).

use std::sync::Arc;

use async_trait::async_trait;
use obrain_common::types::NodeId;
use obrain_reactive::{MutationEvent, MutationListener};

use super::manager::EngramManager;

/// Surprise (erreur de prédiction) attribuée à chaque épisode de co-activation.
///
/// En l'absence d'un modèle prédictif câblé au flux de mutations, une constante
/// est un substitut honnête : elle fait franchir le seuil de surprise
/// (`FormationConfig::min_prediction_error`, défaut 0.3) dès la première
/// co-occurrence, laissant `min_episodes` (défaut 3) être le vrai seuil.
pub const DEFAULT_PREDICTION_ERROR_PER_ACTIVATION: f64 = 0.5;

/// `MutationListener` qui traduit les mutations en co-activations d'engramme.
pub struct EngramObservationBridge {
    manager: Arc<EngramManager>,
    prediction_error: f64,
}

impl EngramObservationBridge {
    /// Crée le pont avec la surprise par épisode par défaut.
    pub fn new(manager: Arc<EngramManager>) -> Self {
        Self {
            manager,
            prediction_error: DEFAULT_PREDICTION_ERROR_PER_ACTIVATION,
        }
    }

    /// Crée le pont avec une surprise par épisode explicite (tests/simulation).
    pub fn with_prediction_error(manager: Arc<EngramManager>, prediction_error: f64) -> Self {
        Self {
            manager,
            prediction_error,
        }
    }

    /// Nœuds distincts touchés par un événement de mutation.
    fn affected_nodes(event: &MutationEvent, out: &mut Vec<NodeId>) {
        match event {
            MutationEvent::NodeCreated { node } => out.push(node.id),
            MutationEvent::NodeUpdated { after, .. } => out.push(after.id),
            MutationEvent::NodeDeleted { node } => out.push(node.id),
            MutationEvent::EdgeCreated { edge } => {
                out.push(edge.src);
                out.push(edge.dst);
            }
            MutationEvent::EdgeUpdated { after, .. } => {
                out.push(after.src);
                out.push(after.dst);
            }
            MutationEvent::EdgeDeleted { edge } => {
                out.push(edge.src);
                out.push(edge.dst);
            }
        }
    }

    /// Enregistre une co-activation si l'ensemble compte au moins 2 nœuds
    /// distincts (un singleton ne peut pas former d'engramme).
    fn record(&self, mut nodes: Vec<NodeId>) {
        nodes.sort_unstable();
        nodes.dedup();
        if nodes.len() >= 2 {
            self.manager
                .record_activation(&nodes, self.prediction_error);
        }
    }
}

#[async_trait]
impl MutationListener for EngramObservationBridge {
    fn name(&self) -> &str {
        "cognitive:engram"
    }

    async fn on_event(&self, event: &MutationEvent) {
        let mut nodes = Vec::with_capacity(2);
        Self::affected_nodes(event, &mut nodes);
        self.record(nodes);
    }

    async fn on_batch(&self, events: &[MutationEvent]) {
        // CORRECTION (25/11) : un LOT technique de mutations n'est PAS un
        // épisode cognitif. Deux opérations indépendantes dans un même lot ne
        // doivent pas créer d'association artificielle. Chaque événement est
        // donc traité comme SA PROPRE co-activation (arête = {src,dst}, nœud
        // seul = singleton filtré par `record`).
        for event in events {
            let mut nodes = Vec::with_capacity(2);
            Self::affected_nodes(event, &mut nodes);
            self.record(nodes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::traits::InMemoryVectorIndex;
    use obrain_reactive::NodeSnapshot;

    fn manager() -> Arc<EngramManager> {
        Arc::new(EngramManager::new(
            super::super::manager::EngramManagerConfig::default(),
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

    fn arete(id: u64, src: u64, dst: u64) -> obrain_reactive::EdgeSnapshot {
        obrain_reactive::EdgeSnapshot {
            id: obrain_common::types::EdgeId(id),
            src: NodeId(src),
            dst: NodeId(dst),
            edge_type: Default::default(),
            properties: Vec::new(),
        }
    }

    #[tokio::test]
    async fn trois_co_activations_du_meme_trio_forment_un_engramme() {
        let mgr = manager();
        let bridge = EngramObservationBridge::new(Arc::clone(&mgr));

        // Trois épisodes du triangle {1,2,3} : chaque arête est SA propre
        // co-activation {src,dst}. Les paires (1,2),(2,3),(1,3) co-occurrent
        // 3 fois → composante connexe {1,2,3}.
        for _ in 0..3 {
            bridge
                .on_batch(&[
                    MutationEvent::EdgeCreated {
                        edge: arete(1, 1, 2),
                    },
                    MutationEvent::EdgeCreated {
                        edge: arete(2, 2, 3),
                    },
                    MutationEvent::EdgeCreated {
                        edge: arete(3, 1, 3),
                    },
                ])
                .await;
        }

        let engrams = mgr.store().list();
        assert!(
            engrams.iter().any(|e| {
                let mut ids: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
                ids.sort_unstable();
                ids == vec![1, 2, 3]
            }),
            "un engramme {{1,2,3}} doit se former, engrams={engrams:?}"
        );
    }

    #[tokio::test]
    async fn un_singleton_ne_forme_rien() {
        let mgr = manager();
        let bridge = EngramObservationBridge::new(Arc::clone(&mgr));

        for _ in 0..5 {
            bridge
                .on_batch(&[MutationEvent::NodeUpdated {
                    before: noeud(1),
                    after: noeud(1),
                }])
                .await;
        }
        assert!(
            mgr.store().list().is_empty(),
            "un singleton ne forme pas d'engramme"
        );
    }

    #[tokio::test]
    async fn deux_operations_independantes_dans_un_lot_ne_se_contaminent_pas() {
        let mgr = manager();
        let bridge = EngramObservationBridge::new(Arc::clone(&mgr));

        // {1,2} (ex. physique) et {3,4} (ex. compta) : deux arêtes INDÉPENDANTES
        // dans le MÊME lot. Elles ne doivent PAS fusionner en un ensemble de 4.
        for _ in 0..3 {
            bridge
                .on_batch(&[
                    MutationEvent::EdgeCreated {
                        edge: arete(1, 1, 2),
                    },
                    MutationEvent::EdgeCreated {
                        edge: arete(2, 3, 4),
                    },
                ])
                .await;
        }

        for e in mgr.store().list() {
            let ids: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
            let a = ids.contains(&1) || ids.contains(&2);
            let b = ids.contains(&3) || ids.contains(&4);
            assert!(
                !(a && b),
                "contamination : l'engramme {ids:?} mélange deux opérations indépendantes"
            );
        }
    }

    /// RÉGRESSION (25/11) : la formation incrémentale d'un triangle ne doit
    /// produire QU'UN engramme {1,2,3} — ni intermédiaire {1,2}, ni doublon.
    /// ROUGE tant que record_activation ne déduplique pas (défaut mesuré).
    #[tokio::test]
    async fn le_triangle_ne_forme_ni_intermediaire_ni_doublon() {
        let mgr = manager();
        let bridge = EngramObservationBridge::new(Arc::clone(&mgr));
        for _ in 0..3 {
            for (s, d) in [(1, 2), (2, 3), (1, 3)] {
                bridge
                    .on_batch(&[MutationEvent::EdgeCreated {
                        edge: arete(s * 10 + d, s, d),
                    }])
                    .await;
            }
        }
        let mut tri: Vec<Vec<u64>> = mgr
            .store()
            .list()
            .iter()
            .map(|e| {
                let mut v: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
                v.sort_unstable();
                v
            })
            .collect();
        tri.sort();
        assert_eq!(
            tri,
            vec![vec![1, 2, 3]],
            "exactement UN engramme {{1,2,3}} (obtenu {tri:?})"
        );
    }

    #[tokio::test]
    async fn une_arete_compte_comme_co_activation() {
        let mgr = manager();
        let bridge = EngramObservationBridge::new(Arc::clone(&mgr));

        // Trois créations d'arête distinctes {a,b} {a,c} {b,c} : chaque paire
        // n'apparaît qu'une fois, donc aucune composante à 3 nœuds ne se forme.
        for (s, d) in [(1, 2), (1, 3), (2, 3)] {
            bridge
                .on_batch(&[MutationEvent::EdgeCreated {
                    edge: arete(100 + s, s, d),
                }])
                .await;
        }
        let engrams = mgr.store().list();
        assert!(
            !engrams
                .iter()
                .any(|e| e.node_ids().iter().map(|n| n.0).collect::<Vec<_>>().len() >= 3),
            "une seule occurrence de chaque paire ne doit pas former de trio"
        );
    }
}
