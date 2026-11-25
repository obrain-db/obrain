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

    /// RÉGRESSION (25/11) : la formation ne doit pas DUPLIQUER un même
    /// ensemble. L'intermédiaire {1,2} PEUT subsister (primitive légitime,
    /// architecture compositionnelle) ; on interdit DEUX engrammes de membres
    /// IDENTIQUES ({1,2,3} en double).
    #[tokio::test]
    async fn le_triangle_ne_duplique_pas_un_meme_ensemble() {
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
        // {1,2,3} doit exister.
        assert!(
            tri.contains(&vec![1, 2, 3]),
            "{{1,2,3}} doit exister (obtenu {tri:?})"
        );
        // Aucun doublon d'ensemble : chaque ensemble distinct apparaît une fois.
        let mut uniques = tri.clone();
        uniques.dedup();
        assert_eq!(
            tri.len(),
            uniques.len(),
            "aucun doublon d'ensemble de membres (obtenu {tri:?})"
        );
    }

    /// CAUSE MESURÉE de l'écart de score entre les deux ABC (25/11) : la
    /// formation incrémentale leur attribue des POIDS RELATIFS différents.
    #[tokio::test]
    async fn la_formation_donne_des_poids_differents_aux_deux_abc() {
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
        let mut abc: Vec<Vec<(u64, f64)>> = mgr
            .store()
            .list()
            .iter()
            .filter(|e| e.ensemble.len() == 3)
            .map(|e| {
                let mut v: Vec<(u64, f64)> = e.ensemble.iter().map(|(n, w)| (n.0, *w)).collect();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v
            })
            .collect();
        assert!(abc.len() >= 1, "au moins un engramme ABC : {abc:?}");
        if abc.len() < 2 {
            return; // pas de doublon : rien à caractériser (comportement corrigé)
        }
        // Normaliser (échelle-invariant) puis comparer les RAPPORTS de poids.
        let norm = |v: &[(u64, f64)]| {
            let s: f64 = v.iter().map(|(_, w)| *w).sum();
            v.iter().map(|(_, w)| w / s).collect::<Vec<f64>>()
        };
        let w0 = norm(&abc[0]);
        let w1 = norm(&abc[1]);
        let l1: f64 = w0.iter().zip(&w1).map(|(a, b)| (a - b).abs()).sum();
        assert!(
            l1 > 1e-9,
            "les deux ABC ont des poids relatifs DIFFÉRENTS (l1={l1:e}) : {w0:?} vs {w1:?}"
        );
    }

    /// ÉGALISATION (décisive) : égaliser les poids des deux ABC rend leurs
    /// signatures IDENTIQUES, prouvant que la différence de score vient des
    /// poids, pas d'une autre variable d'état.
    #[tokio::test]
    async fn egaliser_les_poids_egalise_les_signatures() {
        use crate::engram::spectral::SpectralEncoder;
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
        let enc = SpectralEncoder::new();
        let abc: Vec<Vec<(NodeId, f64)>> = mgr
            .store()
            .list()
            .iter()
            .filter(|e| e.ensemble.len() == 3)
            .map(|e| e.ensemble.clone())
            .collect();
        assert!(abc.len() >= 2, "deux ABC attendus : {}", abc.len());
        let l1 = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>();
        let sig_orig: Vec<Vec<f64>> = abc.iter().map(|ens| enc.encode(ens)).collect();
        let l1_orig = l1(&sig_orig[0], &sig_orig[1]);
        let egal = |ens: &[(NodeId, f64)]| {
            let nodes: Vec<(NodeId, f64)> = ens.iter().map(|(n, _)| (*n, 2.0 / 3.0)).collect();
            enc.encode(&nodes)
        };
        let l1_egal = l1(&egal(&abc[0]), &egal(&abc[1]));
        eprintln!("E_EGAL l1_orig={l1_orig:e} l1_egal={l1_egal:e}");
        assert!(
            l1_orig > 1e-9,
            "poids d'origine différents -> signatures différentes"
        );
        assert!(l1_egal < 1e-12, "poids égalisés -> signatures identiques");
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
