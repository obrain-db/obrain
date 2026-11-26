//! EngramManager — the main coordinator for the engram system.
//!
//! Owns all subsystems (formation, decay, recall, homeostasis, metrics)
//! and provides a unified API for the rest of obrain-cognitive.

use std::collections::HashMap;
use std::sync::Arc;
use web_time::SystemTime;

use obrain_common::types::NodeId;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use super::decay::{FsrsConfig, FsrsScheduler, ReviewRating};
use super::formation::{CoActivationSnapshot, FormationConfig, HebbianWithSurprise};
use super::homeostasis::{HomeostasisConfig, HomeostasisEngine};
use super::hopfield::{
    Prediction, PredictionErrorCalculator, PredictiveModel, bayesian_update,
    compute_prediction_error,
};
use super::observe::EngramMetricsCollector;
use super::recall::{
    reconsolidate, reconsolidate_adaptive, RecallEngine, RecallResult, WarmupConfig,
    WarmupSelector,
};
use super::spectral::SpectralEncoder;
use super::store::{EngramStore, StoreError};
use super::traits::VectorIndex;
use super::types::{
    AdaptiveRevisionParams, Engram, EngramHorizon, EngramId, ObservationScope, RecallEvent,
    RecallFeedback,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Top-level configuration for the [`EngramManager`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngramManagerConfig {
    /// Configuration for Hebbian formation with surprise modulation.
    pub formation: FormationConfig,
    /// Configuration for the FSRS decay scheduler.
    pub fsrs: FsrsConfig,
    /// Configuration for homeostatic regulation.
    pub homeostasis: HomeostasisConfig,
    /// Configuration for warm-up recall selection.
    pub warmup: WarmupConfig,
}

impl Default for EngramManagerConfig {
    fn default() -> Self {
        Self {
            formation: FormationConfig::default(),
            fsrs: FsrsConfig::default(),
            homeostasis: HomeostasisConfig::default(),
            warmup: WarmupConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// EngramManager
// ---------------------------------------------------------------------------

/// Central coordinator for the engram memory system.
///
/// Holds all subsystems behind `Arc` or by value and exposes a
/// `&self`-only API suitable for concurrent use.
pub struct EngramManager {
    /// The shared engram store (DashMap-backed).
    store: Arc<EngramStore>,
    /// Hebbian + surprise formation engine (behind Mutex for interior mutability).
    formation: Mutex<HebbianWithSurprise>,
    /// FSRS-based decay scheduler.
    fsrs: FsrsScheduler,
    /// Spectral signature encoder.
    spectral: SpectralEncoder,
    /// Content-addressable recall engine.
    recall_engine: RecallEngine,
    /// Warmup configuration.
    warmup_config: WarmupConfig,
    /// Homeostatic regulation engine.
    homeostasis: HomeostasisEngine,
    /// Lock-free metrics collector.
    metrics: Arc<EngramMetricsCollector>,
    /// Vector index for spectral similarity search.
    vector_index: Arc<dyn VectorIndex>,
    /// PROTECTION PROVISOIRE par engramme : sérialise mutate+lecture+upsert de la
    /// projection dérivée. À retirer quand la discipline native du store couvrira
    /// aussi la publication des projections (voir note de reprise).
    revise_locks: Mutex<HashMap<EngramId, Arc<Mutex<()>>>>,
}

/// Résultat d'une révision cohérente : identifie le nouvel état (révision,
/// membres pondérés, signature) pour vérification et comparaison.
#[derive(Debug, Clone)]
pub struct EngramRevision {
    pub engram_id: EngramId,
    /// Révision GLOBALE du store (borne inférieure) — n'identifie pas à elle
    /// seule le snapshot ; l'état réellement publié est `members`+`signature`.
    pub revision: u64,
    pub members: Vec<(NodeId, f64)>,
    pub signature: Vec<f64>,
}

/// Erreurs de révision, distinguées (pas un simple Option).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionError {
    EngramAbsent(EngramId),
    InvalidObservation(String),
    /// Parametres de revision invalides (non finis, bornes incoherentes).
    InvalidParams(String),
    ValidationFailed(String),
    PersistenceFailed(String),
}

/// Erreur d un pas de boucle predictive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredictiveStepError {
    /// Dimension de l embedding predit differente de la dimension observee :
    /// aucune distance L2 n est calculee entre deux espaces heterogenes.
    DimensionMismatch { predicted: usize, observed: usize },
    /// Observation vide : aucune erreur calculable.
    EmptyObservation,
}

/// Politique de formation / revision d un pas predictif.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictivePolicy {
    /// Formation Hebbienne + reconsolidation ADDITIVE (historique).
    Consolidating,
    /// Formation Hebbienne + revision NON MONOTONE. La portee decide si
    /// l ABSENCE d un noeud est une preuve (Complete) ou une non-observation
    /// (Partial, qui preserve).
    Adaptive(ObservationScope),
    /// Formation EPISODIQUE : un engramme par ensemble observe, ce qui DIFFERENCIE
    /// deux contextes partageant des participants (la formation par composantes
    /// connexes les fusionne).
    Episodic,
}

/// Strategie de reconsolidation, traitee par UNE SEULE autorite (`revise_with`) :
/// memes garanties de validation, persistance, coherence spectrale, publication
/// d index, propagation d erreur et rejeu pour toutes les variantes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReconsolidationStrategy {
    /// Additive (historique) : ajoute les noeuds manquants, ne retire rien.
    Additive,
    /// Non monotone : aligne l ensemble sur l observation selon la portee et
    /// les parametres de decroissance/retrait.
    Adaptive(ObservationScope, AdaptiveRevisionParams),
}

/// Action effectuee par un pas de boucle predictive.
#[derive(Debug, Clone)]
pub enum PredictiveAction {
    /// Aucun engramme recoupe : l observation a nourri la formation.
    FedFormation,
    /// Revision appliquee avec succes.
    Revised(EngramRevision),
    /// Revision demandee mais refusee (erreur conservee, jamais masquee).
    RevisionFailed(RevisionError),
}

/// Resultat d un pas de boucle predictive native.
#[derive(Debug, Clone)]
pub struct PredictiveStep {
    /// Embedding predit AVANT l observation (vecteur nul si aucune prediction).
    pub predicted: Vec<f64>,
    /// Embedding de l observation, produit independamment de la prediction.
    pub observed: Vec<f64>,
    /// Erreur L2 reelle (dimensions verifiees).
    pub error: f64,
    /// `predict()` a-t-il rendu une prediction ?
    pub had_prediction: bool,
    /// Engramme dominant de l attention (si prediction).
    pub dominant: Option<EngramId>,
    /// Action effectuee et son resultat.
    pub action: PredictiveAction,
    /// Nombre d engrams presents apres le pas.
    pub engram_count: usize,
}

impl EngramManager {
    /// Creates a new `EngramManager` from the given configuration.
    ///
    /// The `vector_index` is injected so callers can provide an
    /// in-memory brute-force index or a production HNSW backend.
    pub fn new(config: EngramManagerConfig, vector_index: Arc<dyn VectorIndex>) -> Self {
        let store = Arc::new(EngramStore::new(None));
        let metrics = Arc::new(EngramMetricsCollector::new());

        Self {
            store,
            formation: Mutex::new(HebbianWithSurprise::new(config.formation)),
            fsrs: FsrsScheduler::new(config.fsrs),
            spectral: SpectralEncoder::new(),
            recall_engine: RecallEngine::new(),
            warmup_config: config.warmup,
            homeostasis: HomeostasisEngine::new(config.homeostasis),
            metrics,
            vector_index,
            revise_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Returns a reference to the shared engram store.
    pub fn store(&self) -> &Arc<EngramStore> {
        &self.store
    }

    /// (2026-08-31) Attache le writer substrate à l'EngramStore : les engrams
    /// formés par `record_activation` sont PERSISTÉS dans les zones
    /// `engram_members`/`engram_bitset` (survivent au redémarrage —
    /// prod-readiness §9.5 point 2). Au boot, recharge les engrams persistés.
    #[cfg(feature = "substrate")]
    pub fn attach_substrate(&self, writer: Arc<obrain_substrate::Writer>) {
        self.store.attach_substrate(writer);
        let loaded = self.store.load_from_substrate();
        if loaded > 0 {
            // Re-dériver les signatures spectrales (projection de l'ensemble
            // pondéré restauré) et ré-upserter l'index vectoriel — le rappel
            // spectral est opérationnel dès la réouverture.
            for engram in self.store.list() {
                let sig = self.spectral.encode(&engram.ensemble);
                self.vector_index.upsert(&engram.id.to_string(), &sig);
                // Reconstruction de projection DÉRIVÉE : cache seul, pas de
                // réécriture durable (update() miroir le substrate).
                self.store.rehydrate_signature(engram.id, sig);
            }
            tracing::info!(loaded, "cognitive: engrams rechargés depuis le substrate");
        }
    }

    /// Returns a reference to the metrics collector.
    pub fn metrics(&self) -> &Arc<EngramMetricsCollector> {
        &self.metrics
    }

    /// (2026-08-31) Accès à l'index vectoriel du rappel spectral — nécessaire
    /// pour câbler `set_engram_subsystem` (le moteur cognitif expose le même
    /// index via `vector_index()`).
    pub fn vector_index_ref(&self) -> &Arc<dyn VectorIndex> {
        &self.vector_index
    }

    /// (2026-08-21) Instantané de la co-activation hebbienne.
    ///
    /// Source du signal A de la facette F10 (« attention ») des features
    /// Hilbert : les paires de nœuds qui reviennent ENSEMBLE à travers les
    /// rappels. Jusqu'ici le moteur accumulait cette carte sans que rien ne
    /// puisse la lire — `formation` est privé et aucun accesseur ne l'ouvrait.
    ///
    /// ── COÛT, ET POURQUOI IL NE FAUT PAS APPELER CECI SUR LE CHEMIN CHAUD ──
    ///
    /// `snapshot()` CLONE la carte des paires. Elle croît avec l'historique
    /// (42 781 engrammes formés en production au 2026-08-21, ~1 000 de plus
    /// par tick), et le clone se fait SOUS le même `Mutex` que
    /// `record_activation` — c'est-à-dire sous le verrou que prend CHAQUE
    /// rappel de l'exocortex. Un appel par rappel sérialiserait les rappels
    /// derrière une copie de la carte entière.
    ///
    /// L'usage prévu est UN appel par passe de calcul des features (toutes
    /// les deux heures côté obrain-tmn), jamais par requête.
    pub fn co_activation_snapshot(&self) -> CoActivationSnapshot {
        self.formation.lock().snapshot().detector
    }

    // -----------------------------------------------------------------
    // Activation & Formation
    // -----------------------------------------------------------------

    /// Records a co-activation event for the given node ensemble.
    ///
    /// Delegates to the formation engine and, if threshold is crossed,
    /// creates new engrams, encodes spectral signatures, and indexes
    /// them in the vector index.
    #[instrument(skip(self), fields(nodes = nodes.len(), prediction_error))]
    pub fn record_activation(&self, nodes: &[NodeId], prediction_error: f64) {
        let mut formation = self.formation.lock();
        formation.record_activation(nodes, prediction_error);

        // Check if any patterns are ready to become engrams.
        let candidates = formation.should_form_engram();
        drop(formation);
        for ensemble in candidates {
            // (2026-08-31) Substrate attaché → `form()` persiste les membres
            // dans la zone `engram_members` + bitset (survie au restart).
            // Sinon → `insert()` mémoire (fallback).
            #[cfg(feature = "substrate")]
            let id = {
                let nids: Vec<NodeId> = ensemble.iter().map(|(n, _)| *n).collect();
                match self.store.form(&nids) {
                    Ok(eid) => eid,
                    Err(_) => {
                        let id = self.store.next_id();
                        let mut e = Engram::new(id, ensemble.clone());
                        let sig = self.spectral.encode(&ensemble);
                        e.spectral_signature.clone_from(&sig);
                        self.store.insert(e);
                        self.vector_index.upsert(&id.to_string(), &sig);
                        self.metrics.record_formation();
                        debug!(%id, "engram formed (memory fallback)");
                        continue;
                    }
                }
            };
            #[cfg(not(feature = "substrate"))]
            let id = self.store.next_id();

            let mut engram = Engram::new(id, ensemble.clone());

            // Encode spectral signature.
            let signature = self.spectral.encode(&ensemble);
            engram.spectral_signature.clone_from(&signature);

            // Insert into store and vector index.
            self.store.insert(engram);
            self.vector_index.upsert(&id.to_string(), &signature);

            self.metrics.record_formation();
            debug!(%id, "engram formed");
        }

        // JEPA — codage prédictif (Layer 3+4). `Engram.predictive_model`
        // (types.rs) n'était JAMAIS alimenté : on branche ici
        // `bayesian_update`/`compute_prediction_error` sur le chemin
        // d'enregistrement. Chaque activation met à jour le modèle prédictif
        // des engrams rappelés au lieu de laisser le champ mort.
        self.update_predictive_models(nodes);
    }

    /// JEPA — met à jour les modèles prédictifs des engrams correspondant aux
    /// nœuds activés, avec l'outcome = signature spectrale de l'activation.
    ///
    /// Réutilise [`bayesian_update`]/[`compute_prediction_error`] (morts
    /// jusqu'ici) : pour chaque engram rappelé, on calcule l'erreur de
    /// prédiction entre son modèle et l'outcome observé, puis on corrige le
    /// modèle. Un engram sans modèle voit son `PredictiveModel` initialisé à
    /// partir de ce premier outcome.
    fn update_predictive_models(&self, nodes: &[NodeId]) {
        if nodes.is_empty() {
            return;
        }
        // Outcome observé = signature spectrale de l'ensemble activé.
        let ensemble: Vec<(NodeId, f64)> = nodes.iter().map(|&n| (n, 1.0)).collect();
        let outcome = self.spectral.encode(&ensemble);
        if outcome.is_empty() {
            return;
        }

        let recalled = self.recall_engine.recall(
            &self.store,
            nodes,
            self.vector_index.as_ref(),
            &self.spectral,
            8,
        );
        for r in recalled {
            let eid = r.engram_id;
            self.store.update(eid, |e| {
                match e.predictive_model.as_mut() {
                    Some(pm) => {
                        if let Some(pe) = compute_prediction_error(pm, &outcome) {
                            bayesian_update(pm, &outcome, &pe);
                        }
                    }
                    None => {
                        // Premier outcome observé → initialise le modèle prédictif.
                        if let Some(pm) =
                            PredictiveModel::from_observations(std::slice::from_ref(&outcome))
                        {
                            e.predictive_model = Some(pm);
                        }
                    }
                }
            });
        }
    }

    /// Révision COHÉRENTE d'un engramme : applique la reconsolidation native,
    /// recompute la signature spectrale à partir du nouvel ensemble pondéré,
    /// met à jour l'index vectoriel, puis retourne un `EngramRevision` identifiant
    /// le nouvel état. Les membres+poids sont persistés atomiquement par
    /// `set_engram_state` (via le miroir substrate de `update`).
    ///
    /// Distinct de `record_activation` (formation) : ici on révise un engramme
    /// EXISTANT à partir d'une observation indépendante.
    pub fn revise(
        &self,
        engram_id: EngramId,
        new_nodes: &[NodeId],
        prediction_error: f64,
    ) -> Result<EngramRevision, RevisionError> {
        self.revise_with(
            engram_id,
            new_nodes,
            prediction_error,
            ReconsolidationStrategy::Additive,
        )
    }

    /// AUTORITE UNIQUE de reconsolidation : toutes les variantes passent ici
    /// (memes garanties AU NIVEAU DE L API : validation, verrou par engramme,
    /// mutate candidat, persistance, signature spectrale recomputee, publication
    /// d index, propagation d erreur, rejeu).
    ///
    /// PORTEE EXACTE : c est une autorite d API, PAS une garantie d isolation
    /// transactionnelle durable. La durabilite, les contrats substrate/WAL et la
    /// reconciliation des projections relevent de #418 (lot transactionnel) ; ce
    /// point d entree ne les remplace ni ne les etend. Le verrou `revise_locks`
    /// reste une PROTECTION PROVISOIRE.
    pub fn revise_with(
        &self,
        engram_id: EngramId,
        new_nodes: &[NodeId],
        prediction_error: f64,
        strategy: ReconsolidationStrategy,
    ) -> Result<EngramRevision, RevisionError> {
        if !prediction_error.is_finite() || prediction_error < 0.0 {
            return Err(RevisionError::InvalidObservation(
                "erreur de prédiction non finie ou négative".into(),
            ));
        }
        if new_nodes.is_empty() {
            return Err(RevisionError::InvalidObservation("observation vide".into()));
        }
        // VALIDATION DES PARAMETRES AVANT TOUT CALCUL (erreur typee, pas de
        // calcul silencieux sur des bornes incoherentes).
        if let ReconsolidationStrategy::Adaptive(_, params) = strategy {
            params.validate().map_err(RevisionError::InvalidParams)?;
        }
        // PROTECTION PROVISOIRE : sérialise mutate + lecture + upsert de la
        // projection, jusqu'à ce que la discipline native couvre la publication.
        // Sans elle, A peut lire sigma1 puis publier sigma1 APRÈS que B a publié
        // sigma2 (projection obsolète réintroduite).
        let lock = {
            let mut m = self.revise_locks.lock();
            m.entry(engram_id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock();
        self.store
            .mutate(engram_id, |e| {
                let mut before: Vec<NodeId> = e.node_ids();
                match strategy {
                    ReconsolidationStrategy::Additive => {
                        reconsolidate(e, new_nodes, prediction_error)
                    }
                    ReconsolidationStrategy::Adaptive(scope, params) => {
                        reconsolidate_adaptive(e, new_nodes, prediction_error, scope, params)
                    }
                }
                let mut after: Vec<NodeId> = e.node_ids();
                before.sort_unstable();
                after.sort_unstable();
                if before != after && e.structural_key.is_some() {
                    // La definition (participants) a CHANGE : la cle fournie par
                    // l appelant ne la decrit plus. Marque PERIMEE, jamais effacee.
                    e.structural_key_stale = true;
                }
                // Projection dérivée : signature recalculée sur l'ensemble
                // pondéré révisé, PAS conservée périmée.
                e.spectral_signature = self.spectral.encode(&e.ensemble);
            })
            .map_err(|e| match e {
                StoreError::EngramAbsent(id) => RevisionError::EngramAbsent(id),
                StoreError::ValidationFailed(m) => RevisionError::ValidationFailed(m),
                StoreError::PersistenceFailed(m) => RevisionError::PersistenceFailed(m),
            })?;
        let engram = self
            .store
            .get(engram_id)
            .ok_or(RevisionError::EngramAbsent(engram_id))?;
        let signature = engram.spectral_signature.clone();
        // Index vectoriel : le rappel spectral doit voir la nouvelle signature.
        self.vector_index.upsert(&engram_id.to_string(), &signature);
        Ok(EngramRevision {
            engram_id,
            revision: self.store.revision(),
            members: engram.ensemble.clone(),
            signature,
        })
    }

    /// Encode une observation (ensemble de noeuds, poids uniformes) dans
    /// l espace spectral du manager. Primitive generique : aucune logique
    /// propre a une experience.
    pub fn encode_observation(&self, nodes: &[NodeId]) -> Vec<f64> {
        let weighted: Vec<(NodeId, f64)> = nodes.iter().map(|&n| (n, 1.0)).collect();
        self.spectral.encode(&weighted)
    }

    /// Erreur L2 entre deux embeddings, DIMENSION VERIFIEE : jamais de distance
    /// entre deux espaces de dimensions differentes (metriques non homogenes).
    pub fn prediction_error(
        &self,
        predicted: &[f64],
        observed: &[f64],
    ) -> Result<f64, PredictiveStepError> {
        if predicted.len() != observed.len() {
            return Err(PredictiveStepError::DimensionMismatch {
                predicted: predicted.len(),
                observed: observed.len(),
            });
        }
        Ok(predicted
            .iter()
            .zip(observed)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>()
            .sqrt())
    }

    /// Un pas de boucle predictive NATIVE : predit AVANT d observer, encode
    /// l observation independamment, calcule l erreur reelle, puis revise
    /// l engramme recoupe (reconsolidation) ou nourrit la formation.
    ///
    /// Primitive GENERIQUE : le choix revise/former repose sur le seul
    /// recoupement entre l observation et l engramme rappele, jamais sur une
    /// regle propre a une experience. Le resultat de `revise` est CONSERVE
    /// dans `PredictiveAction` (jamais ignore).
    pub fn predictive_step(
        &self,
        cues: &[NodeId],
        observed: &[NodeId],
        k: usize,
    ) -> Result<PredictiveStep, PredictiveStepError> {
        self.predictive_step_with_policy(cues, observed, k, PredictivePolicy::Consolidating)
    }

    /// Un pas predictif selon une POLITIQUE de formation/revision. Le choix
    /// revise/former reste generique (recoupement observation/engramme). Le
    /// resultat de la revision est toujours conserve dans `PredictiveAction`.
    pub fn predictive_step_with_policy(
        &self,
        cues: &[NodeId],
        observed: &[NodeId],
        k: usize,
        policy: PredictivePolicy,
    ) -> Result<PredictiveStep, PredictiveStepError> {
        if observed.is_empty() {
            return Err(PredictiveStepError::EmptyObservation);
        }
        let dim = self.spectral.dimensions();
        let prediction = self.predict(cues, k);
        let (predicted, dominant) = match &prediction {
            Some(p) => (p.predicted.clone(), p.attention.first().map(|(id, _)| *id)),
            None => (vec![0.0; dim], None),
        };
        let observed_emb = self.encode_observation(observed);
        let error = self.prediction_error(&predicted, &observed_emb)?;

        let revise_target = dominant.filter(|eid| {
            self.store
                .get(*eid)
                .map(|e| e.node_ids().iter().any(|n| observed.contains(n)))
                .unwrap_or(false)
        });
        let action = match policy {
            PredictivePolicy::Consolidating => match revise_target {
                Some(eid) => match self.revise(eid, observed, error) {
                    Ok(rev) => PredictiveAction::Revised(rev),
                    Err(err) => PredictiveAction::RevisionFailed(err),
                },
                None => {
                    self.record_activation(observed, error);
                    PredictiveAction::FedFormation
                }
            },
            PredictivePolicy::Adaptive(scope) => match revise_target {
                Some(eid) => match self.revise_adaptive(eid, observed, error, scope) {
                    Ok(rev) => PredictiveAction::Revised(rev),
                    Err(err) => PredictiveAction::RevisionFailed(err),
                },
                None => {
                    self.record_activation(observed, error);
                    PredictiveAction::FedFormation
                }
            },
            PredictivePolicy::Episodic => {
                let eid = self.form_observation(observed);
                match self.revise_adaptive(eid, observed, error, ObservationScope::DeclaredComplete) {
                    Ok(rev) => PredictiveAction::Revised(rev),
                    Err(err) => PredictiveAction::RevisionFailed(err),
                }
            }
        };
        Ok(PredictiveStep {
            predicted,
            observed: observed_emb,
            error,
            had_prediction: prediction.is_some(),
            dominant,
            action,
            engram_count: self.store.count(),
        })
    }

    /// Formation EPISODIQUE : cree (ou retrouve) un engramme dont l ensemble est
    /// EXACTEMENT l observation. Deux contextes partageant des participants
    /// restent deux engrams distincts (pas de fusion par composante connexe).
    pub fn form_observation(&self, nodes: &[NodeId]) -> EngramId {
        self.form_observation_with(nodes, None)
    }

    /// Formation EPISODIQUE avec IDENTITE STRUCTURELLE optionnelle. Deux
    /// ensembles de MEMES participants mais de cles structurelles differentes
    /// (relations/directions/roles/contexte) donnent DEUX engrams distincts.
    pub fn form_observation_with(
        &self,
        nodes: &[NodeId],
        structural_key: Option<&str>,
    ) -> EngramId {
        let mut uniq: Vec<NodeId> = Vec::new();
        for &n in nodes {
            if !uniq.contains(&n) {
                uniq.push(n);
            }
        }
        let mut key: Vec<u64> = uniq.iter().map(|n| n.0).collect();
        key.sort_unstable();
        for e in self.store.list() {
            let mut k: Vec<u64> = e.node_ids().iter().map(|n| n.0).collect();
            k.sort_unstable();
            if k == key && e.structural_key.as_deref() == structural_key {
                return e.id;
            }
        }
        let id = self.store.next_id();
        let ensemble: Vec<(NodeId, f64)> = uniq.iter().map(|&n| (n, 1.0)).collect();
        let mut engram = Engram::new(id, ensemble);
        engram.structural_key = structural_key.map(|s| s.to_string());
        let signature = self.spectral.encode(&engram.ensemble);
        engram.spectral_signature = signature.clone();
        self.store.insert(engram);
        self.vector_index.upsert(&id.to_string(), &signature);
        self.metrics.record_formation();
        id
    }

    /// (Re)definit la cle structurelle d un engramme et levee l etat PERIME.
    /// A appeler apres une revision qui a change les participants.
    pub fn set_structural_key(&self, engram_id: EngramId, key: Option<&str>) {
        self.store.update(engram_id, |e| {
            e.structural_key = key.map(|s| s.to_string());
            e.structural_key_stale = false;
        });
    }

    /// Revision NON MONOTONE : comme `revise`, mais l ensemble est ALIGNE sur
    /// l observation (noeuds absents decroisses puis retires). Corrige la limite
    /// additive mesuree.
    pub fn revise_adaptive(
        &self,
        engram_id: EngramId,
        new_nodes: &[NodeId],
        prediction_error: f64,
        scope: ObservationScope,
    ) -> Result<EngramRevision, RevisionError> {
        self.revise_adaptive_with(
            engram_id,
            new_nodes,
            prediction_error,
            scope,
            AdaptiveRevisionParams::default(),
        )
    }

    /// Variante PARAMETRABLE (pour MESURER l effet des constantes), meme
    /// autorite `revise_with` donc memes garanties.
    pub fn revise_adaptive_with(
        &self,
        engram_id: EngramId,
        new_nodes: &[NodeId],
        prediction_error: f64,
        scope: ObservationScope,
        params: AdaptiveRevisionParams,
    ) -> Result<EngramRevision, RevisionError> {
        self.revise_with(
            engram_id,
            new_nodes,
            prediction_error,
            ReconsolidationStrategy::Adaptive(scope, params),
        )
    }

    // -----------------------------------------------------------------
    // Recall
    // -----------------------------------------------------------------

    /// Performs content-addressable recall from the given cue nodes.
    ///
    /// Returns the top `k` matching engrams ordered by relevance.
    #[instrument(skip(self), fields(cues = cues.len(), k))]
    pub fn recall(&self, cues: &[NodeId], k: usize) -> Vec<RecallResult> {
        let results = self.recall_engine.recall(
            &self.store,
            cues,
            self.vector_index.as_ref(),
            &self.spectral,
            k,
        );
        for _ in &results {
            self.metrics.record_recall(true);
        }
        results
    }

    /// Warm-up recall: proactively retrieve engrams likely to be useful
    /// given the current cue context, with MMR diversity.
    ///
    /// Returns [`super::recall::ActivatedEngram`]s with the dominant engram in full detail
    /// and secondaries in summary form.
    #[instrument(skip(self), fields(cues = cues.len()))]
    pub fn warmup(&self, cues: &[NodeId]) -> Vec<super::recall::ActivatedEngram> {
        WarmupSelector::select_warmup_engrams(
            &self.store,
            cues,
            self.vector_index.as_ref(),
            &self.spectral,
            &self.warmup_config,
        )
    }

    // -----------------------------------------------------------------
    // JEPA — Prédiction
    // -----------------------------------------------------------------

    /// Prédit l'embedding suivant : centroïde spectral pondéré des engrams
    /// rappelés via le recall Hopfield existant, plus la surprise (erreur de
    /// prédiction) associée.
    ///
    /// La surprise est calculée en réutilisant
    /// [`PredictionErrorCalculator::compute_from_engrams`] avec le centroïde
    /// prédit comme « outcome » : elle mesure à quel point la prédiction
    /// s'écarte de ce que les modèles prédictifs des engrams rappelés
    /// attendaient. Une surprise forte est le signal qui doit faire basculer
    /// la couche de décision vers l'exploration
    /// (`crate::decision::acting_source`).
    ///
    /// Retourne `None` si aucun engram n'est rappelé ou si aucune signature
    /// spectrale exploitable n'est disponible.
    #[instrument(skip(self), fields(cues = cues.len(), k))]
    pub fn predict(&self, cues: &[NodeId], k: usize) -> Option<Prediction> {
        let results = self.recall_engine.recall(
            &self.store,
            cues,
            self.vector_index.as_ref(),
            &self.spectral,
            k,
        );
        if results.is_empty() {
            return None;
        }

        let dim = self.spectral.dimensions();
        let mut predicted = vec![0.0f64; dim];
        let mut total_weight = 0.0f64;
        let mut attention: Vec<(EngramId, f64)> = Vec::with_capacity(results.len());

        for r in &results {
            let sig = &r.engram.spectral_signature;
            if sig.is_empty() || sig.len() != dim {
                continue;
            }
            let w = r.confidence.max(0.0);
            total_weight += w;
            attention.push((r.engram_id, w));
            for (i, &v) in sig.iter().enumerate() {
                predicted[i] += w * v;
            }
        }

        if total_weight <= 0.0 {
            return None;
        }
        for v in &mut predicted {
            *v /= total_weight;
        }

        // Normalise les poids d'attention et trie par poids décroissant.
        for (_, w) in &mut attention {
            *w /= total_weight;
        }
        attention.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let ids: Vec<EngramId> = attention.iter().map(|(id, _)| *id).collect();
        let surprise =
            PredictionErrorCalculator::compute_from_engrams(&self.store, &ids, &predicted);

        Some(Prediction {
            predicted,
            surprise,
            attention,
        })
    }

    // -----------------------------------------------------------------
    // Feedback
    // -----------------------------------------------------------------

    /// Mark an engram as having been used (positive feedback).
    ///
    /// Strengthens the engram, records a recall event, and feeds the
    /// homeostasis meta-plasticity tracker.
    pub fn mark_used(&self, engram_id: EngramId) {
        self.store.update(engram_id, |e| {
            e.recall_count += 1;
            e.last_activated = SystemTime::now();
            e.recall_history.push(RecallEvent {
                timestamp: SystemTime::now(),
                cues: Vec::new(),
                feedback: Some(RecallFeedback::Used),
            });
            // Boost strength slightly (clamped by homeostasis later).
            e.strength = (e.strength + 0.05).min(1.0);
        });
        self.metrics.record_recall(true);
        self.homeostasis.record_recall_outcome(true);
    }

    /// Mark an engram as rejected (negative feedback).
    ///
    /// Weakens the engram slightly and records the rejection.
    pub fn mark_rejected(&self, engram_id: EngramId) {
        self.store.update(engram_id, |e| {
            e.recall_history.push(RecallEvent {
                timestamp: SystemTime::now(),
                cues: Vec::new(),
                feedback: Some(RecallFeedback::Rejected),
            });
            // Slight strength penalty.
            e.strength = (e.strength - 0.02).max(0.0);
        });
        self.metrics.record_recall(false);
        self.homeostasis.record_recall_outcome(false);
    }

    // -----------------------------------------------------------------
    // Periodic Maintenance
    // -----------------------------------------------------------------

    /// Periodic maintenance tick.
    ///
    /// Should be called on a timer (e.g., every N seconds). Runs:
    /// 1. Homeostasis sweep (synaptic scaling).
    /// 2. FSRS decay for engrams that haven't been reviewed recently.
    /// 3. Horizon promotion/demotion based on strength.
    /// 4. Metrics update.
    #[instrument(skip(self))]
    pub fn tick(&self) {
        // 1. Homeostasis sweep.
        self.homeostasis.sweep(&self.store);
        self.metrics.record_homeostasis_sweep();

        // 2. FSRS decay pass — apply decay to engrams past their review date.
        let engrams = self.store.list();
        let now = SystemTime::now();
        for engram in &engrams {
            let elapsed_days = engram
                .fsrs_state
                .last_review
                .and_then(|lr| now.duration_since(lr).ok())
                .map_or(0.0, |d| d.as_secs_f64() / 86400.0);

            // Check if past scheduled review (or if never reviewed and old enough).
            let is_due = engram
                .fsrs_state
                .next_review
                .map_or(elapsed_days > 1.0, |nr| now >= nr);

            if is_due {
                let id = engram.id;
                self.store.update(id, |e| {
                    let days = e
                        .fsrs_state
                        .last_review
                        .and_then(|lr| now.duration_since(lr).ok())
                        .map_or(1.0, |d| d.as_secs_f64() / 86400.0);
                    let new_state = self.fsrs.review(&e.fsrs_state, ReviewRating::Again, days);
                    e.fsrs_state = new_state;
                    e.strength = (e.strength * 0.9).max(0.0);
                });

                // Check if engram has decayed below threshold.
                if let Some(updated) = self.store.get(id)
                    && updated.strength < 0.05
                {
                    self.store.remove(id);
                    self.vector_index.remove(&id.to_string());
                    self.metrics.record_decay();
                    debug!(%id, "engram decayed and removed");
                }
            }
        }

        // 3. Horizon promotion/demotion.
        let engrams = self.store.list();
        for engram in &engrams {
            let id = engram.id;
            let new_horizon = if engram.strength >= 0.8 && engram.recall_count >= 3 {
                EngramHorizon::Consolidated
            } else if engram.strength < 0.2 {
                EngramHorizon::Archived
            } else {
                EngramHorizon::Operational
            };

            if new_horizon != engram.horizon {
                self.store.update(id, |e| {
                    e.horizon = new_horizon;
                });

                if let Some(updated) = self.store.get(id)
                    && updated.is_crystallization_candidate()
                {
                    self.metrics.record_crystallization();
                    debug!(%id, "engram crystallization candidate");
                }
            }
        }

        // 4. Update mean strength metric.
        let engrams = self.store.list();
        if !engrams.is_empty() {
            let sum: f64 = engrams.iter().map(|e| e.strength).sum();
            let mean = sum / engrams.len() as f64;
            self.metrics.update_mean_strength(mean);
        }

        debug!(active = self.store.count(), "tick complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::traits::InMemoryVectorIndex;

    fn make_manager() -> EngramManager {
        let config = EngramManagerConfig::default();
        let index = Arc::new(InMemoryVectorIndex::new());
        EngramManager::new(config, index)
    }

    #[test]
    fn manager_can_be_constructed() {
        let _m = make_manager();
    }

    #[test]
    fn mark_used_strengthens_engram() {
        let m = make_manager();
        let id = m.store.next_id();
        let mut e = Engram::new(id, vec![(NodeId(1), 1.0)]);
        e.strength = 0.5;
        m.store.insert(e);

        m.mark_used(id);

        let updated = m.store.get(id).unwrap();
        assert!(updated.strength > 0.5);
        assert_eq!(updated.recall_count, 1);
        assert_eq!(
            updated.recall_history.last().unwrap().feedback,
            Some(RecallFeedback::Used)
        );
    }

    #[test]
    fn mark_rejected_weakens_engram() {
        let m = make_manager();
        let id = m.store.next_id();
        let mut e = Engram::new(id, vec![(NodeId(1), 1.0)]);
        e.strength = 0.5;
        m.store.insert(e);

        m.mark_rejected(id);

        let updated = m.store.get(id).unwrap();
        assert!(updated.strength < 0.5);
    }

    #[test]
    fn tick_does_not_panic_on_empty_store() {
        let m = make_manager();
        m.tick();
    }

    #[test]
    fn tick_promotes_strong_engrams() {
        let m = make_manager();
        let id = m.store.next_id();
        let mut e = Engram::new(id, vec![(NodeId(1), 1.0)]);
        e.strength = 0.85;
        e.recall_count = 5;
        m.store.insert(e);

        m.tick();

        let updated = m.store.get(id).unwrap();
        assert_eq!(updated.horizon, EngramHorizon::Consolidated);
    }

    #[test]
    fn tick_archives_weak_engrams() {
        let m = make_manager();
        let id = m.store.next_id();
        let mut e = Engram::new(id, vec![(NodeId(1), 1.0)]);
        e.strength = 0.15;
        m.store.insert(e);

        m.tick();

        let updated = m.store.get(id).unwrap();
        assert_eq!(updated.horizon, EngramHorizon::Archived);
    }

    #[test]
    fn metrics_reflect_operations() {
        let m = make_manager();
        let id = m.store.next_id();
        let e = Engram::new(id, vec![(NodeId(1), 1.0)]);
        m.store.insert(e);

        m.mark_used(id);
        m.mark_rejected(id);

        let snap = m.metrics.snapshot();
        assert_eq!(snap.recalls_successful, 1);
        assert_eq!(snap.recalls_rejected, 1);
        assert_eq!(snap.recalls_attempted, 2);
    }

    #[test]
    fn predict_computes_embedding_and_surprise() {
        let m = make_manager();
        let dim = m.spectral.dimensions();

        // Deux engrams aux ensembles disjoints, avec signatures spectrales.
        let id1 = m.store.next_id();
        let nodes1 = vec![(NodeId(1), 1.0), (NodeId(2), 1.0)];
        let mut e1 = Engram::new(id1, nodes1.clone());
        e1.spectral_signature = m.spectral.encode(&nodes1);
        m.store.insert(e1);

        let id2 = m.store.next_id();
        let nodes2 = vec![(NodeId(3), 1.0), (NodeId(4), 1.0)];
        let mut e2 = Engram::new(id2, nodes2.clone());
        e2.spectral_signature = m.spectral.encode(&nodes2);
        m.store.insert(e2);

        // (a) la prédiction est calculée pour des cues qui recoupent l'engram 1.
        let prediction = m
            .predict(&[NodeId(1), NodeId(2)], 4)
            .expect("prédiction calculée");
        assert_eq!(prediction.predicted.len(), dim);
        assert!(
            prediction.predicted.iter().any(|&v| v.abs() > 1e-9),
            "le centroïde spectral prédit doit être non trivial"
        );
        // La surprise est normalisée dans [0, 1].
        assert!(
            (0.0..=1.0).contains(&prediction.surprise),
            "surprise hors de [0,1] : {}",
            prediction.surprise
        );
        assert!(
            !prediction.attention.is_empty(),
            "résultat attendu non vide"
        );
    }

    #[test]
    fn record_activation_feeds_predictive_model() {
        let m = make_manager();
        // Co-activations répétées du même trio → formation d'engrams, et le
        // chemin JEPA doit alimenter leur PredictiveModel (champ jusque-là mort).
        let nodes = vec![NodeId(10), NodeId(11), NodeId(12)];
        for _ in 0..5 {
            m.record_activation(&nodes, 0.5);
        }
        let engrams = m.store.list();
        assert!(
            engrams.iter().any(|e| e.predictive_model.is_some()),
            "record_activation doit alimenter le PredictiveModel des engrams"
        );
    }
}
