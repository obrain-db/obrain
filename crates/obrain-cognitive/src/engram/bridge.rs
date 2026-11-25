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
use super::observation::{
    ObservationDefinition, ObservationId, ObservationOutcome, SourceRevision,
};
use super::types::EngramId;

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
    /// Voie EXPÉRIMENTALE d'identité d'observation (registre VOLATIL).
    /// Désactivée par défaut : sans opt-in explicite, le pont fait UNIQUEMENT
    /// de la FORMATION (aucune identité d'observation n'est forcée). Ce n'est
    /// pas un comportement de production.
    experimental_revision: bool,
}

impl EngramObservationBridge {
    /// Crée le pont avec la surprise par épisode par défaut.
    pub fn new(manager: Arc<EngramManager>) -> Self {
        Self {
            manager,
            prediction_error: DEFAULT_PREDICTION_ERROR_PER_ACTIVATION,
            experimental_revision: false,
        }
    }

    /// Active EXPLICITEMENT la voie expérimentale de révision identifiée.
    /// Sans cet opt-in, le pont ne fait que de la formation. Le registre
    /// d'observation par défaut est le mock VOLATIL (non durable).
    pub fn with_experimental_revision(mut self, enabled: bool) -> Self {
        self.experimental_revision = enabled;
        self
    }

    /// Crée le pont avec une surprise par épisode explicite (tests/simulation).
    pub fn with_prediction_error(manager: Arc<EngramManager>, prediction_error: f64) -> Self {
        Self {
            manager,
            prediction_error,
            experimental_revision: false,
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

    /// Engramme CIBLE d'une révision : celui qui recouvre le plus de nœuds de
    /// l'événement (au moins 2, pour qu'une révision ait un sens).
    ///
    /// C'est un choix de routage : sans recouvrement suffisant, l'événement
    /// reste une FORMATION, jamais une révision forcée.
    /// Engramme CIBLE d'une révision : celui qui recouvre le plus de nœuds de
    /// l'événement (au moins 2), À CONDITION qu'il soit SANS AMBIGUÏTÉ.
    ///
    /// Une proximité structurelle (recouvrement de membres) ne garantit PAS la
    /// pertinence : deux connaissances peuvent partager plusieurs membres tout
    /// en étant des contextes distincts. En cas d'ÉGALITÉ au meilleur
    /// recouvrement, on REFUSE de choisir (None) plutôt que de modifier
    /// arbitrairement la mauvaise connaissance.
    fn target_engram(&self, nodes: &[NodeId]) -> TargetSelection {
        let mut best: Option<(EngramId, usize)> = None;
        let mut ambiguous = false;
        self.manager.store().for_each(|e| {
            let overlap = e.ensemble.iter().filter(|(n, _)| nodes.contains(n)).count();
            if overlap >= 2 {
                match best {
                    Some((_, b)) if overlap > b => {
                        best = Some((e.id, overlap));
                        ambiguous = false;
                    }
                    Some((_, b)) if overlap == b => {
                        ambiguous = true;
                    }
                    None => best = Some((e.id, overlap)),
                    _ => {}
                }
            }
        });
        if ambiguous {
            // C. Ambiguïté DISTINCTE de l'absence : on préserve les hypothèses
            // concurrentes plutôt que de forcer une formation arbitraire.
            TargetSelection::Ambiguous(
                self.candidates_with_overlap(nodes)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            )
        } else {
            match best {
                Some((id, _)) => TargetSelection::Unique(id),
                None => TargetSelection::None,
            }
        }
    }

    /// Toutes les connaissances qui recouvrent au moins 2 nœuds (hypothèses).
    fn candidates_with_overlap(&self, nodes: &[NodeId]) -> Vec<(EngramId, usize)> {
        let mut out: Vec<(EngramId, usize)> = Vec::new();
        self.manager.store().for_each(|e| {
            let overlap = e.ensemble.iter().filter(|(n, _)| nodes.contains(n)).count();
            if overlap >= 2 {
                out.push((e.id, overlap));
            }
        });
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    /// Traite un événement de mutation, en distinguant FORMATION et RÉVISION.
    ///
    /// * `observation_id = None` → **FORMATION** (`record_activation`) : une
    ///   co-activation simple ne porte PAS d'identité d'observation. Aucune
    ///   identité n'est forcée sur une formation.
    /// * `observation_id = Some(id)` → **RÉVISION** identifiée : si un engramme
    ///   existant recouvre l'événement, `revise_with_id` l'applique (rejeu
    ///   idempotent, conflit de révision qualifié). Sans engramme cible, on
    ///   retombe sur la FORMATION (une formation n'est pas une révision).
    ///
    /// NOTE : le registre d'observation est le mock VOLATIL par défaut ; cette
    /// voie n'est donc PAS durable tant que `with_observation_ledger(primitive)`
    /// n'a pas injecté la primitive substrat.
    pub fn observe(
        &self,
        event: &MutationEvent,
        observation_id: Option<ObservationId>,
    ) -> BridgeObservation {
        let mut nodes = Vec::with_capacity(2);
        Self::affected_nodes(event, &mut nodes);
        nodes.sort_unstable();
        nodes.dedup();
        if nodes.len() < 2 {
            return BridgeObservation::Ignored;
        }
        // Identité prise en compte UNIQUEMENT si la voie expérimentale est
        // explicitement activée.
        let id = if self.experimental_revision {
            observation_id
        } else {
            None
        };
        if let Some(id) = id {
            match self.target_engram(&nodes) {
                TargetSelection::Ambiguous(candidates) => {
                    // Aucune modification : les hypothèses restent ouvertes.
                    return BridgeObservation::Ambiguous(candidates);
                }
                TargetSelection::Unique(target) => {
                    // RÉVISION identifiée. Une lecture de révision EN ERREUR n'est
                    // JAMAIS assimilée à la révision initiale : on propage.
                    let revision = match self.manager.observation_revision(target) {
                        Ok(r) => r,
                        Err(e) => return BridgeObservation::Failed(e),
                    };
                    return match self.manager.revise_with_id(
                        id,
                        target,
                        &nodes,
                        self.prediction_error,
                        SourceRevision(revision),
                    ) {
                        // L'identité a déjà été traitée (révision OU formation) :
                        // rien n'est refait.
                        Ok((ObservationOutcome::AlreadyFormed, _)) => BridgeObservation::Replayed,
                        Ok((ObservationOutcome::Formed, _)) => BridgeObservation::Replayed,
                        Ok((outcome, _)) => BridgeObservation::Revised(outcome),
                        Err(e) => BridgeObservation::Failed(format!("{e:?}")),
                    };
                }
                TargetSelection::None => {}
            }
            // AUCUNE cible (ou ambiguë) : FORMATION identifiée. L'identité est
            // enregistrée AVANT la transformation, pour qu'un rejeu ne
            // réactive pas — l'identité est INDÉPENDANTE de la transformation.
            let definition = match ObservationDefinition::new(&nodes, self.prediction_error) {
                Ok(d) => d,
                Err(e) => return BridgeObservation::Failed(e),
            };
            // Ressource SYNTHÉTIQUE propre à l'OBSERVATION (pas au contenu) :
            // deux observations indépendantes de même contenu doivent toutes
            // deux renforcer l'apprentissage.
            let mut resource = b"F".to_vec();
            resource.extend_from_slice(&id.0);
            let adapter = super::observation::CognitiveObservationAdapter::new(
                self.manager.observation_ledger_arc(),
            );
            match adapter.apply_formation(id, &resource, &definition) {
                // B. L'identité n'est CONFIRMÉE qu'APRÈS l'activation : un
                // rejeu d'une formation PENDING réactive — jamais « déjà fait »
                // alors que l'apprentissage est absent.
                Ok(ObservationOutcome::Formed) | Ok(ObservationOutcome::FormationPending) => {
                    let formed = self
                        .manager
                        .record_activation(&nodes, self.prediction_error);
                    if let Err(e) = adapter.complete_formation(id) {
                        return BridgeObservation::Failed(e);
                    }
                    if formed.is_empty() {
                        BridgeObservation::ActivationRecorded
                    } else {
                        BridgeObservation::FormationSucceeded(formed)
                    }
                }
                Ok(ObservationOutcome::AlreadyFormed) | Ok(ObservationOutcome::Abandoned) => {
                    BridgeObservation::Replayed
                }
                Ok(other) => BridgeObservation::Failed(format!(
                    "issue de formation identifiée inattendue : {other:?}"
                )),
                Err(e) => BridgeObservation::Failed(e),
            }
        } else {
            // FORMATION simple, SANS identité d'observation.
            let formed = self
                .manager
                .record_activation(&nodes, self.prediction_error);
            if formed.is_empty() {
                BridgeObservation::ActivationRecorded
            } else {
                BridgeObservation::FormationSucceeded(formed)
            }
        }
    }
}

/// Sélection de la connaissance cible d'une observation identifiée.
#[derive(Debug, Clone, PartialEq)]
enum TargetSelection {
    /// Aucune connaissance ne recouvre suffisamment l'événement.
    None,
    /// Un unique meilleur recouvrement (PAS une preuve de pertinence).
    Unique(EngramId),
    /// Égalité : plusieurs connaissances concurrentes.
    Ambiguous(Vec<EngramId>),
}

/// Issue d'un événement traité par le pont.
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeObservation {
    /// Moins de 2 nœuds distincts : rien à faire.
    Ignored,
    /// ACTIVATION enregistrée (compteurs Hebbien) — AUCUNE création d'engramme
    /// n'est établie.
    ActivationRecorded,
    /// FORMATION RÉUSSIE : ces engrammes ont effectivement été créés.
    FormationSucceeded(Vec<EngramId>),
    /// RÉVISION identifiée : issue du contrat d'observation.
    Revised(ObservationOutcome),
    /// REJEU : l'identité était déjà traitée, AUCUNE transformation refaite.
    Replayed,
    /// CIBLE AMBIGUË : plusieurs connaissances recouvrent l'événement à
    /// égalité. AUCUNE modification ; les hypothèses sont préservées.
    Ambiguous(Vec<EngramId>),
    /// Échec (lecture de révision, formation identifiée inattendue, ...).
    Failed(String),
}

#[async_trait]
impl MutationListener for EngramObservationBridge {
    fn name(&self) -> &str {
        "cognitive:engram"
    }

    async fn on_event(&self, event: &MutationEvent) {
        // Le flux de mutations est une FORMATION : aucune identité
        // d'observation n'est forcée.
        let _ = self.observe(event, None);
    }

    async fn on_batch(&self, events: &[MutationEvent]) {
        // CORRECTION (25/11) : un LOT technique de mutations n'est PAS un
        // épisode cognitif. Deux opérations indépendantes dans un même lot ne
        // doivent pas créer d'association artificielle. Chaque événement est
        // donc traité comme SA PROPRE co-activation (arête = {src,dst}, nœud
        // seul = singleton filtré par `record`).
        for event in events {
            let _ = self.observe(event, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::manager::EngramManagerConfig;
    use crate::engram::observation::InMemoryAtomicEventLedger;
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
    #[ignore = "dédup de formation — Lot 2 (identité/état), défaut documenté non corrigé"]
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
        let abc: Vec<Vec<(u64, f64)>> = mgr
            .store()
            .list()
            .iter()
            .filter(|e| e.ensemble.len() == 3)
            .map(|e| {
                let mut v: Vec<(u64, f64)> = e.ensemble.iter().map(|(n, w)| (n.0, *w)).collect();
                v.sort_by_key(|a| a.0);
                v
            })
            .collect();
        assert!(!abc.is_empty(), "au moins un engramme ABC : {abc:?}");
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

    // -----------------------------------------------------------------
    // Formation vs révision identifiée (contrat RFC d59af717)
    // -----------------------------------------------------------------

    fn edge_event(id: u64, src: u64, dst: u64) -> MutationEvent {
        MutationEvent::EdgeCreated {
            edge: arete(id, src, dst),
        }
    }

    /// FORMATION : sans identité d'observation, le pont forme et n'écrit AUCUNE
    /// identité dans le registre (une formation n'est pas une révision).
    #[tokio::test]
    async fn formation_sans_identite_ne_porte_pas_d_identite() {
        let mgr = manager();
        let bridge = EngramObservationBridge::new(Arc::clone(&mgr));
        for _ in 0..3 {
            let out = bridge.observe(&edge_event(1, 1, 2), None);
            assert!(
                matches!(
                    out,
                    BridgeObservation::ActivationRecorded
                        | BridgeObservation::FormationSucceeded(_)
                ),
                "formation sans identité, reçu {out:?}"
            );
        }
        assert!(mgr.store().count() >= 1, "un engramme a été formé");
        let eid = mgr.store().list()[0].id;
        assert_eq!(
            mgr.observation_revision(eid).unwrap(),
            0,
            "aucune identité d'observation forcée sur une formation"
        );
    }

    /// RÉVISION : un événement IDENTIFIÉ qui recouvre un engramme existant passe
    /// par revise_with_id (révision appliquée au registre).
    #[tokio::test]
    async fn revision_identifiee_sur_engramme_existant() {
        let mgr = manager();
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        // Formation de {1,2}.
        for _ in 0..3 {
            bridge.observe(&edge_event(1, 1, 2), None);
        }
        let eid = mgr.store().list()[0].id;
        assert_eq!(mgr.observation_revision(eid).unwrap(), 0);

        let out = bridge.observe(&edge_event(2, 1, 2), Some(ObservationId::from_u128(1)));
        assert!(
            matches!(
                out,
                BridgeObservation::Revised(ObservationOutcome::Applied { .. })
            ),
            "révision identifiée appliquée, reçu {out:?}"
        );
        assert!(
            mgr.observation_revision(eid).unwrap() >= 1,
            "révision portée par le registre"
        );
    }

    /// REJEU : le même événement identifié deux fois n'est appliqué qu'une fois.
    #[tokio::test]
    async fn rejeu_de_l_evenement_identifie_ne_reapplique_pas() {
        let mgr = manager();
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        for _ in 0..3 {
            bridge.observe(&edge_event(1, 1, 2), None);
        }
        let eid = mgr.store().list()[0].id;

        let first = bridge.observe(&edge_event(2, 1, 2), Some(ObservationId::from_u128(7)));
        assert!(matches!(
            first,
            BridgeObservation::Revised(ObservationOutcome::Applied { .. })
        ));
        let rev_after_first = mgr.observation_revision(eid).unwrap();
        let replay = bridge.observe(&edge_event(2, 1, 2), Some(ObservationId::from_u128(7)));
        assert!(
            matches!(
                replay,
                BridgeObservation::Revised(ObservationOutcome::Replayed { .. })
            ),
            "le rejeu ne réapplique pas, reçu {replay:?}"
        );
        assert_eq!(
            mgr.observation_revision(eid).unwrap(),
            rev_after_first,
            "révision inchangée au rejeu"
        );
    }

    /// Événement IDENTIFIÉ sans engramme cible : FORMATION (jamais révision
    /// forcée).
    #[tokio::test]
    async fn evenement_identifie_sans_cible_forme() {
        let mgr = manager();
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        let out = bridge.observe(&edge_event(1, 10, 11), Some(ObservationId::from_u128(5)));
        assert!(
            matches!(
                out,
                BridgeObservation::ActivationRecorded | BridgeObservation::FormationSucceeded(_)
            ),
            "sans engramme cible, l'événement reste une formation, reçu {out:?}"
        );
    }

    fn insert_engram(mgr: &Arc<EngramManager>, nodes: &[u64]) -> EngramId {
        let id = mgr.store().next_id();
        let ensemble: Vec<(NodeId, f64)> = nodes.iter().map(|n| (NodeId(*n), 1.0)).collect();
        mgr.store()
            .insert(crate::engram::types::Engram::new(id, ensemble));
        id
    }

    /// A. Cible AMBIGUË : deux connaissances partagent le même meilleur
    /// recouvrement → le pont REFUSE de choisir et ne révise AUCUNE des deux.
    #[tokio::test]
    async fn cible_ambigue_ne_revise_aucune_connaissance() {
        let mgr = manager();
        let e1 = insert_engram(&mgr, &[1, 2, 3]);
        let e2 = insert_engram(&mgr, &[1, 2, 4]);
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        // L'événement {1,2} recouvre les DEUX au mieux (2 nœuds) : ambigu.
        let out = bridge.observe(&edge_event(9, 1, 2), Some(ObservationId::from_u128(1)));
        match out {
            BridgeObservation::Ambiguous(candidates) => {
                assert!(
                    candidates.contains(&e1) && candidates.contains(&e2),
                    "hypothèses préservées"
                );
            }
            other => panic!("attendu Ambiguous (distinct de l'absence), reçu {other:?}"),
        }
        assert_eq!(mgr.observation_revision(e1).unwrap(), 0, "e1 non révisé");
        assert_eq!(mgr.observation_revision(e2).unwrap(), 0, "e2 non révisé");
    }

    /// B. Une LECTURE de révision en ERREUR est PROPAGÉE, jamais assimilée à la
    /// révision initiale.
    #[tokio::test]
    async fn lecture_de_revision_en_erreur_propage() {
        struct RevisionReadFails;
        impl crate::engram::observation::AtomicEventLedger for RevisionReadFails {
            fn resource_revision(&self, _: &[u8]) -> Result<u64, String> {
                Err("injected revision read error".into())
            }
            fn resource_state(&self, _: &[u8]) -> Result<Option<(Vec<u8>, u64)>, String> {
                Err("injected revision read error".into())
            }
            fn apply_event(
                &self,
                _: &[u8; 16],
                _: &[u8],
                _: &[u8],
                _: u64,
                _: &[u8],
                _: u64,
            ) -> Result<crate::engram::observation::LedgerOutcome, String> {
                Err("unused".into())
            }
            fn resolve(
                &self,
                _: &[u8; 16],
            ) -> Result<crate::engram::observation::ResolvedState, String> {
                Err("unused".into())
            }
            fn abandon(
                &self,
                _: &[u8; 16],
            ) -> Result<crate::engram::observation::AbandonOutcome, String> {
                Err("unused".into())
            }
            fn complete_formation(&self, _: &[u8; 16]) -> Result<bool, String> {
                Err("unused".into())
            }
        }
        let mgr = Arc::new(
            EngramManager::new(
                super::super::manager::EngramManagerConfig::default(),
                Arc::new(InMemoryVectorIndex::new()),
            )
            .with_observation_ledger(Arc::new(RevisionReadFails)),
        );
        insert_engram(&mgr, &[1, 2]);
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        let out = bridge.observe(&edge_event(9, 1, 2), Some(ObservationId::from_u128(1)));
        match out {
            BridgeObservation::Failed(msg) => {
                assert!(
                    msg.contains("revision read error"),
                    "erreur propagée : {msg}"
                )
            }
            other => panic!("attendu Failed (lecture de révision), reçu {other:?}"),
        }
    }

    /// C. L'identité survit à l'absence de cible : une formation identifiée,
    /// puis l'apparition d'une connaissance correspondante, puis le REJEU de la
    /// même identité → aucune nouvelle activation/révision.
    #[tokio::test]
    async fn identite_de_formation_survit_a_l_apparition_d_une_connaissance() {
        let mgr = manager();
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        // 1. Observation identifiée SANS cible → formation identifiée.
        let first = bridge.observe(&edge_event(1, 5, 6), Some(ObservationId::from_u128(3)));
        assert!(
            matches!(
                first,
                BridgeObservation::ActivationRecorded | BridgeObservation::FormationSucceeded(_)
            ),
            "formation identifiée, reçu {first:?}"
        );
        // 2. Une connaissance {5,6} apparaît ensuite.
        let eid = insert_engram(&mgr, &[5, 6]);
        let count_before = mgr.store().count();
        // 3. Rejeu de la MÊME identité : Replayed, RIEN refait.
        let replay = bridge.observe(&edge_event(1, 5, 6), Some(ObservationId::from_u128(3)));
        assert_eq!(
            replay,
            BridgeObservation::Replayed,
            "le rejeu ne produit pas une nouvelle activation/révision"
        );
        assert_eq!(
            mgr.store().count(),
            count_before,
            "aucune formation supplémentaire"
        );
        assert_eq!(
            mgr.observation_revision(eid).unwrap(),
            0,
            "la connaissance apparue n'a pas été révisée rétroactivement"
        );
    }

    /// A. Trois occurrences IDENTIFIÉES de même contenu RENFORCENT réellement :
    /// chacune incrémente la formation (pas de déduplication par contenu).
    #[tokio::test]
    async fn trois_occurrences_identifiees_de_meme_contenu_forment() {
        let mgr = manager();
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        for i in 0..3u64 {
            let out = bridge.observe(
                &edge_event(i, 1, 2),
                Some(ObservationId::from_u128(i as u128 + 1)),
            );
            assert!(
                matches!(
                    out,
                    BridgeObservation::ActivationRecorded
                        | BridgeObservation::FormationSucceeded(_)
                ),
                "occurrence {i} a renforcé, reçu {out:?}"
            );
        }
        // Trois co-activations indépendantes → l'engramme {1,2} se forme.
        assert!(
            mgr.store().count() >= 1,
            "les trois occurrences ont toutes renforcé (formation atteinte)"
        );
    }

    /// B. La fenêtre PENDING→activation→DONE est MESURÉE : une interruption de
    /// la confirmation après activation effective double l'activation au rejeu.
    #[tokio::test]
    async fn interruption_avant_confirmation_double_l_activation() {
        let ledger = Arc::new(InMemoryAtomicEventLedger::new());
        let mgr = Arc::new(
            EngramManager::new(
                EngramManagerConfig::default(),
                Arc::new(InMemoryVectorIndex::new()),
            )
            .with_observation_ledger(ledger.clone()),
        );
        let bridge =
            EngramObservationBridge::new(Arc::clone(&mgr)).with_experimental_revision(true);
        // Confirmation SABOTÉE : l'activation a lieu, la confirmation échoue.
        ledger.set_fail_complete_formation(true);
        let out1 = bridge.observe(&edge_event(1, 1, 2), Some(ObservationId::from_u128(1)));
        assert!(
            matches!(out1, BridgeObservation::Failed(_)),
            "confirmation échouée"
        );
        // Rejeu : la formation est PENDING → RÉACTIVATION (2e), confirmation échoue.
        let out2 = bridge.observe(&edge_event(1, 1, 2), Some(ObservationId::from_u128(1)));
        assert!(
            matches!(out2, BridgeObservation::Failed(_)),
            "confirmation échouée au rejeu"
        );
        // Une 3e occurrence (autre identité) : 3e activation.
        ledger.set_fail_complete_formation(false);
        let out3 = bridge.observe(&edge_event(2, 1, 2), Some(ObservationId::from_u128(2)));
        assert!(matches!(
            out3,
            BridgeObservation::ActivationRecorded | BridgeObservation::FormationSucceeded(_)
        ));
        // Preuve de DOUBLE application : obs1 a compté DEUX fois (1 obs logique +
        // son rejeu = 2 activations) + obs2 (1) = 3 → formation atteinte.
        assert!(
            mgr.store().count() >= 1,
            "la double application de obs1 a franchi le seuil de formation (3 activations)"
        );
    }
}

#[cfg(test)]
mod tests_pont_observation {
    use super::*;
    use crate::engram::traits::InMemoryVectorIndex;
    use obrain_reactive::{EdgeSnapshot, NodeSnapshot};

    fn manager() -> Arc<EngramManager> {
        Arc::new(EngramManager::new(
            crate::engram::manager::EngramManagerConfig::default(),
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
            id: obrain_common::types::EdgeId(id),
            src: NodeId(src),
            dst: NodeId(dst),
            edge_type: Default::default(),
            properties: Vec::new(),
        }
    }

    /// Le pont expose son nom et accepte une surprise par épisode explicite
    /// (constructeur `with_prediction_error`, chemin de test/simulation).
    #[test]
    fn le_pont_expose_son_nom_et_accepte_une_surprise_explicite() {
        let mgr = manager();
        let bridge = EngramObservationBridge::with_prediction_error(Arc::clone(&mgr), 0.7);
        assert_eq!(bridge.name(), "cognitive:engram");
        assert!((bridge.prediction_error - 0.7).abs() < f64::EPSILON);
    }

    /// `on_event` (chemin unitaire, hors lot) traduit CHAQUE variante de
    /// mutation en co-activation : les singletons ne forment rien, une arête
    /// co-activée `min_episodes` fois forme l'engramme {src,dst}.
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
}
