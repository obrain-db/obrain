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
use super::observation::{
    AtomicEventLedger, CognitiveObservationAdapter, InMemoryAtomicEventLedger,
    ObservationDefinition, ObservationId, ObservationOutcome, ResolvedState, ResultingState,
    SourceRevision,
};
use super::observe::EngramMetricsCollector;
use super::recall::{
    reconsolidate, reconsolidate_adaptive, RecallEngine, RecallResult, WarmupConfig,
    WarmupSelector,
};
use super::spectral::SpectralEncoder;
use super::store::{EngramStore, LoadReport, MaterializeOutcome, StoreError};
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
    /// Registre transactionnel d'identité d'observation (contrat RFC d59af717).
    /// Par défaut l'implémentation de RÉFÉRENCE en mémoire — la primitive
    /// durable du substrat la remplacera via [`Self::with_observation_ledger`].
    observation_ledger: Arc<dyn AtomicEventLedger>,
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

/// Un pas d un OPERATEUR DE REVISION natif, parametre STRUCTURELLEMENT : la
/// CIBLE n est pas un identifiant concret mais un SELECTEUR resolu contre
/// l etat COURANT de l engramme au moment de l application.
#[derive(Debug, Clone)]
pub struct RevisionStep {
    pub target: RevisionTarget,
    pub prediction_error: f64,
    pub scope: ObservationScope,
    pub params: AdaptiveRevisionParams,
}

/// Cible d un pas : concrete (identifiants) ou STRUCTURELLE (selecteur).
#[derive(Debug, Clone)]
pub enum RevisionTarget {
    Concrete(Vec<NodeId>),
    Structural(NodeSelector),
}

/// Selecteur STRUCTUREL, INDEPENDANT des identifiants concrets : le meme
/// operateur s applique a des engrams differents.
#[derive(Debug, Clone)]
pub enum NodeSelector {
    /// Tous les membres courants.
    All,
    /// Les k membres de plus fort poids.
    TopK(usize),
    /// Les membres dont le poids depasse `w`.
    AboveWeight(f64),
    /// Les membres dont le poids est inferieur a `w`.
    BelowWeight(f64),
}

/// Erreur d application : RESTITUE le prefixe deja publie et l indice du pas fautif.
#[derive(Debug)]
pub struct RevisionApplyError {
    pub applied: Vec<EngramRevision>,
    pub failed_step: usize,
    pub cause: RevisionError,
}

/// Un OPERATEUR NATIF REUTILISABLE : une sequence ORDONNEE de pas appliques
/// contre l etat courant. `compose` est une simple CONCATENATION : l
/// associativite est ACQUISE PAR CONSTRUCTION, pas « decouverte ». Ce n est PAS
/// un operateur abstrait transferable : les selecteurs le rendent reutilisable
/// sur des situations nouvelles, rien de plus.
#[derive(Debug, Clone, Default)]
pub struct RevisionOp {
    pub steps: Vec<RevisionStep>,
}

impl RevisionOp {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn push(mut self, step: RevisionStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Resout la cible d un pas contre l etat COURANT de l engramme.
    pub fn resolve_target(
        &self,
        m: &EngramManager,
        eid: EngramId,
        target: &RevisionTarget,
    ) -> Vec<NodeId> {
        match target {
            RevisionTarget::Concrete(nodes) => nodes.clone(),
            RevisionTarget::Structural(sel) => {
                let e = m.store().get(eid).expect("engramme absent");
                let mut ens = e.ensemble.clone();
                ens.sort_by(|x, y| {
                    y.1.partial_cmp(&x.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                match sel {
                    NodeSelector::All => ens.iter().map(|(n, _)| *n).collect(),
                    NodeSelector::TopK(k) => ens.iter().take(*k).map(|(n, _)| *n).collect(),
                    NodeSelector::AboveWeight(w) => {
                        ens.iter().filter(|(_, x)| *x > *w).map(|(n, _)| *n).collect()
                    }
                    NodeSelector::BelowWeight(w) => {
                        ens.iter().filter(|(_, x)| *x < *w).map(|(n, _)| *n).collect()
                    }
                }
            }
        }
    }

    /// Applique la sequence. S ARRETE a la premiere erreur ; le `Err` RESTITUE le
    /// prefixe deja publie (`applied`) et l indice du pas fautif.
    pub fn apply(
        &self,
        m: &EngramManager,
        eid: EngramId,
    ) -> Result<Vec<EngramRevision>, RevisionApplyError> {
        let mut applied = Vec::with_capacity(self.steps.len());
        for (i, s) in self.steps.iter().enumerate() {
            let nodes = self.resolve_target(m, eid, &s.target);
            match m.revise_adaptive_with(eid, &nodes, s.prediction_error, s.scope, s.params) {
                Ok(rev) => applied.push(rev),
                Err(cause) => {
                    return Err(RevisionApplyError {
                        applied,
                        failed_step: i,
                        cause,
                    })
                }
            }
        }
        Ok(applied)
    }

    /// Composition MATERIELISEE = concatenation des pas.
    pub fn compose(a: &RevisionOp, b: &RevisionOp) -> RevisionOp {
        RevisionOp {
            steps: a.steps.iter().chain(b.steps.iter()).cloned().collect(),
        }
    }
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
        let spectral = SpectralEncoder::new();
        // CONTRAT DE PROJECTION : le store recalcule la signature depuis
        // l'ensemble (poids compris) via un encodeur DÉDIÉ (même spec
        // déterministe que `spectral`). Aucun encodeur n'est imposé au store.
        {
            let proj_encoder = Arc::new(SpectralEncoder::new());
            store.set_projection(Arc::new(move |ens| proj_encoder.encode(ens)));
        }

        Self {
            store,
            formation: Mutex::new(HebbianWithSurprise::new(config.formation)),
            fsrs: FsrsScheduler::new(config.fsrs),
            spectral,
            recall_engine: RecallEngine::new(),
            warmup_config: config.warmup,
            homeostasis: HomeostasisEngine::new(config.homeostasis),
            metrics,
            vector_index,
            revise_locks: Mutex::new(HashMap::new()),
            observation_ledger: Arc::new(InMemoryAtomicEventLedger::new()),
        }
    }

    /// Remplace le registre d'observations (injecter la primitive durable du
    /// substrat quand elle existe ; sinon l'implémentation de RÉFÉRENCE en
    /// mémoire reste active et n'est JAMAIS présentée comme durable).
    pub fn with_observation_ledger(mut self, ledger: Arc<dyn AtomicEventLedger>) -> Self {
        self.observation_ledger = ledger;
        self
    }

    /// Révision canonique COURANTE d'une connaissance, telle que connue du
    /// registre transactionnel. À passer comme `source_revision` à
    /// [`Self::revise_with_id`].
    pub fn observation_revision(&self, engram_id: EngramId) -> Result<u64, String> {
        self.observation_ledger
            .resource_revision(&engram_id.0.to_be_bytes())
    }

    /// Accès (crate) à la primitive d'observation, pour le pont.
    pub(crate) fn observation_ledger_arc(&self) -> Arc<dyn AtomicEventLedger> {
        self.observation_ledger.clone()
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
    pub fn attach_substrate(&self, writer: Arc<obrain_substrate::Writer>) -> LoadReport {
        self.store.attach_substrate(writer);
        let report = self.store.load_from_substrate();
        if report.coherent > 0 || !report.degraded.is_empty() {
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
            tracing::info!(
                coherent = report.coherent,
                degraded = report.degraded.len(),
                rejected = report.rejected.len(),
                "cognitive: engrams rechargés depuis le substrate"
            );
        }
        report
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
    pub fn record_activation(&self, nodes: &[NodeId], prediction_error: f64) -> Vec<EngramId> {
        let mut formed: Vec<EngramId> = Vec::new();
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
                        formed.push(id);
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
            formed.push(id);
            debug!(%id, "engram formed");
        }

        // JEPA — codage prédictif (Layer 3+4). `Engram.predictive_model`
        // (types.rs) n'était JAMAIS alimenté : on branche ici
        // `bayesian_update`/`compute_prediction_error` sur le chemin
        // d'enregistrement. Chaque activation met à jour le modèle prédictif
        // des engrams rappelés au lieu de laisser le champ mort.
        self.update_predictive_models(nodes);
        formed
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
        // RATTRAPAGE : une mutation sans identité ne doit JAMAIS partir d'un
        // état en retard sur le registre (un effet accepté mais non matérialisé
        // serait perdu par le bump de révision).
        self.catch_up_store(engram_id)?;
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

    /// Encode un ensemble PONDERE (poids reels). Permet au temoin de calculer la
    /// signature ATTENDUE sur les poids reels et de verifier que la signature
    /// PUBLIEE est exactement celle-la (pas seulement « differente »).
    pub fn encode_weighted(&self, ensemble: &[(NodeId, f64)]) -> Vec<f64> {
        self.spectral.encode(ensemble)
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
            // Une cle PERIMEE ne decrit plus la definition : elle ne peut PAS
            // servir d'identite de deduplication (sinon on reutilise un engramme
            // dont la cle ne correspond plus aux participants).
            if k == key
                && e.structural_key.as_deref() == structural_key
                && !e.structural_key_stale
            {
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

    /// (Re)DECLARE la cle structurelle d un engramme et leve l etat PERIME.
    ///
    /// Ce n est PAS une verification : la cle reste une etiquette opaque
    /// fournie par l appelant. Lever `stale` signifie « l appelant re-declare
    /// cette cle pour les participants COURANTS », PAS « la cle est prouvee ».
    /// L autorite de DEFINITION reste #416 (Composition) ; ce champ ne la
    /// remplace pas et ne cree pas d autorite parallele.
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

    // Révision avec identité d'observation (contrat RFC d59af717)
    // -----------------------------------------------------------------

    /// Applique une observation IDENTIFIÉE, avec précondition de révision.
    ///
    /// * `observation_id` : identité LOGIQUE de l'événement, distincte des
    ///   tentatives (un rejeu technique la réutilise).
    /// * `source_revision` : révision observée par l'appelant (voir
    ///   [`Self::observation_revision`]). Si la connaissance a avancé côté
    ///   registre, l'appel rend `RevisionConflict` SANS rien appliquer.
    /// * Un rejeu rend `Replayed` ET **vérifie la matérialisation** : si le
    ///   cache est EN RETARD sur la révision enregistrée (échec de
    ///   matérialisation après application au registre), il est RÉPARÉ avec
    ///   l'état ENREGISTRÉ. Jamais de restauration d'un ancien candidat sur une
    ///   connaissance plus récente (`engram.revision > recorded.revision`).
    /// * `Indeterminate` est résolu par interrogation de l'état transactionnel ;
    ///   la résolution restitue l'état EFFECTIVEMENT enregistré.
    ///
    /// NOTE : le registre par défaut est le mock VOLATIL ; la primitive durable
    /// du substrat portera l'atomicité. La transformation reçoit EXACTEMENT les
    /// nœuds canonisés de la définition.
    /// RATTRAPAGE : si le store est EN RETARD sur l'état enregistré au registre
    /// (échec de matérialisation antérieur), on le répare AVANT tout nouveau
    /// calcul. Sans cela, une nouvelle observation partirait d'un état obsolète
    /// et l'apprentissage déjà accepté serait perdu.
    fn catch_up_store(&self, engram_id: EngramId) -> Result<(), RevisionError> {
        let resource = engram_id.0.to_be_bytes();
        let Some((effect, rec_rev)) = self
            .observation_ledger
            .resource_state(&resource)
            .map_err(RevisionError::PersistenceFailed)?
        else {
            return Ok(());
        };
        let live = self
            .store
            .get(engram_id)
            .ok_or(RevisionError::EngramAbsent(engram_id))?;
        if live.revision >= rec_rev {
            return Ok(());
        }
        let body = match effect.first() {
            Some(&1) => &effect[9..],
            _ => {
                return Err(RevisionError::PersistenceFailed(
                    "effet de ressource non-révision (formation)".into(),
                ));
            }
        };
        let recorded =
            ResultingState::from_canonical_bytes(body).map_err(RevisionError::PersistenceFailed)?;
        let sig = self.spectral.encode(&recorded.ensemble);
        let repaired = self
            .store
            .repair_state(
                engram_id,
                rec_rev,
                recorded.ensemble.clone(),
                recorded.strength,
                recorded.recall_count,
                sig.clone(),
            )
            .map_err(|e| match e {
                StoreError::EngramAbsent(id) => RevisionError::EngramAbsent(id),
                StoreError::ValidationFailed(m) => RevisionError::ValidationFailed(m),
                StoreError::PersistenceFailed(m) => RevisionError::PersistenceFailed(m),
            })?;
        if repaired {
            self.vector_index.upsert(&engram_id.to_string(), &sig);
        }
        Ok(())
    }

    /// Applique une observation IDENTIFIÉE (rejeu idempotent, conflit qualifié).
    ///
    /// `source_revision` est la révision d'OBSERVATION vue par l'appelant (voir
    /// [`Self::observation_revision`]) ; elle sert de précondition au registre.
    /// Sur `RevisionConflict`, l'appelant recalcule depuis l'état courant avec
    /// la MÊME identité logique. Un rejeu rend `Replayed` ET répare une
    /// matérialisation manquante.
    pub fn revise_with_id(
        &self,
        observation_id: ObservationId,
        engram_id: EngramId,
        new_nodes: &[NodeId],
        prediction_error: f64,
        source_revision: SourceRevision,
    ) -> Result<(ObservationOutcome, Option<EngramRevision>), RevisionError> {
        if !prediction_error.is_finite() || prediction_error < 0.0 {
            return Err(RevisionError::InvalidObservation(
                "erreur de prédiction non finie ou négative".into(),
            ));
        }
        if new_nodes.is_empty() {
            return Err(RevisionError::InvalidObservation("observation vide".into()));
        }
        // Définition canonique versionnée (refuse non-fini / canonise).
        let definition = ObservationDefinition::new(new_nodes, prediction_error)
            .map_err(RevisionError::InvalidObservation)?;
        // La transformation reçoit les nœuds CANONISÉS : définition et effet
        // coïncident exactement.
        let canonical_nodes: Vec<NodeId> = definition.canonical_nodes().to_vec();

        let lock = {
            let mut m = self.revise_locks.lock();
            m.entry(engram_id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock();

        let adapter = CognitiveObservationAdapter::new(self.observation_ledger.clone());
        // RATTRAPAGE avant tout calcul : une nouvelle observation ne doit
        // JAMAIS partir d'un état obsolète.
        self.catch_up_store(engram_id)?;
        let current = self
            .store
            .get(engram_id)
            .ok_or(RevisionError::EngramAbsent(engram_id))?;
        // La révision de la CONNAISSANCE (store) est l'autorité pour la
        // matérialisation ; `source_revision` (registre) sert de précondition
        // d'OBSERVATION.
        let live_revision = current.revision;
        let mut candidate = current;
        reconsolidate(&mut candidate, &canonical_nodes, prediction_error);
        let signature = self.spectral.encode(&candidate.ensemble);
        let resulting_state = ResultingState {
            ensemble: candidate.ensemble.clone(),
            strength: candidate.strength,
            recall_count: candidate.recall_count,
        };
        let resulting_revision = live_revision.saturating_add(1);

        let outcome = adapter
            .apply(
                observation_id,
                engram_id,
                &engram_id.0.to_be_bytes(),
                &definition,
                source_revision,
                &resulting_state,
                resulting_revision,
            )
            .map_err(RevisionError::PersistenceFailed)?;

        let map_store = |e: StoreError| match e {
            StoreError::EngramAbsent(id) => RevisionError::EngramAbsent(id),
            StoreError::ValidationFailed(m) => RevisionError::ValidationFailed(m),
            StoreError::PersistenceFailed(m) => RevisionError::PersistenceFailed(m),
        };

        match outcome {
            ObservationOutcome::Applied { .. } => {
                // Injection de faute (test) : échec de matérialisation APRÈS
                // l'application au registre → incohérence réparable au rejeu.
                #[cfg(feature = "fault-injection")]
                if self.store.take_fail_materialise() {
                    return Err(RevisionError::PersistenceFailed(
                        "injected materialisation failure".into(),
                    ));
                }
                match self
                    .store
                    .materialize_state(
                        engram_id,
                        live_revision,
                        resulting_state.ensemble.clone(),
                        resulting_state.strength,
                        resulting_state.recall_count,
                        signature.clone(),
                        resulting_revision,
                    )
                    .map_err(map_store)?
                {
                    MaterializeOutcome::Applied => {}
                    MaterializeOutcome::Conflict { current_revision } => {
                        // B. La connaissance a avancé HORS verrou APRÈS que le
                        // registre a accepté l'effet : on ne présente PAS un
                        // effet enregistré comme un refus sans effet.
                        return Ok((
                            ObservationOutcome::RecordedNotMaterialized { current_revision },
                            None,
                        ));
                    }
                }
                self.vector_index.upsert(&engram_id.to_string(), &signature);
                let engram = self
                    .store
                    .get(engram_id)
                    .ok_or(RevisionError::EngramAbsent(engram_id))?;
                Ok((
                    ObservationOutcome::Applied {
                        state: resulting_state,
                        revision: resulting_revision,
                    },
                    Some(EngramRevision {
                        engram_id,
                        revision: self.store.revision(),
                        members: engram.ensemble.clone(),
                        signature: engram.spectral_signature.clone(),
                    }),
                ))
            }
            ObservationOutcome::Replayed {
                state: recorded,
                revision: rec_rev,
                target: rec_target,
            } => {
                // D. La CIBLE enregistrée doit être CETTE connaissance : ne
                // jamais matérialiser un ancien état dans une autre.
                if rec_target != engram_id {
                    // Vérifier la matérialisation ; réparer si le cache est en
                    // retard. Jamais au-delà d'une révision plus récente.
                    return Err(RevisionError::ValidationFailed(format!(
                        "observation enregistrée pour la connaissance {:?}, pas {:?}",
                        rec_target, engram_id
                    )));
                }
                let live = self
                    .store
                    .get(engram_id)
                    .ok_or(RevisionError::EngramAbsent(engram_id))?;
                if live.revision < rec_rev {
                    let sig = self.spectral.encode(&recorded.ensemble);
                    if self
                        .store
                        .repair_state(
                            engram_id,
                            rec_rev,
                            recorded.ensemble.clone(),
                            recorded.strength,
                            recorded.recall_count,
                            sig.clone(),
                        )
                        .map_err(map_store)?
                    {
                        // Index mis à jour SEULEMENT si la réparation a eu lieu :
                        // jamais d'ancienne projection réintroduite sur refus.
                        self.vector_index.upsert(&engram_id.to_string(), &sig);
                    }
                }
                Ok((
                    ObservationOutcome::Replayed {
                        state: recorded,
                        revision: rec_rev,
                        target: rec_target,
                    },
                    None,
                ))
            }
            ObservationOutcome::IdentityConflict { recorded } => {
                Ok((ObservationOutcome::IdentityConflict { recorded }, None))
            }
            ObservationOutcome::RevisionConflict { current_revision } => Ok((
                ObservationOutcome::RevisionConflict { current_revision },
                None,
            )),
            ObservationOutcome::RecordedNotMaterialized { current_revision } => Ok((
                ObservationOutcome::RecordedNotMaterialized { current_revision },
                None,
            )),
            ObservationOutcome::AlreadyFormed => {
                // L'identité a été enregistrée comme FORMATION : ne pas la
                // transformer rétroactivement en révision.
                Ok((ObservationOutcome::AlreadyFormed, None))
            }
            ObservationOutcome::Formed => Ok((ObservationOutcome::Formed, None)),
            ObservationOutcome::FormationPending => {
                Ok((ObservationOutcome::FormationPending, None))
            }
            ObservationOutcome::Abandoned => Ok((ObservationOutcome::Abandoned, None)),
            ObservationOutcome::Indeterminate => {
                match adapter
                    .resolve(observation_id)
                    .map_err(RevisionError::PersistenceFailed)?
                {
                    ResolvedState::NotApplied => Ok((ObservationOutcome::Indeterminate, None)),
                    ResolvedState::Indeterminate { .. } => {
                        // Engagement INDÉTERMINÉ (jamais engagé OU preuve
                        // compactée, indistinguable) : ne PAS rejouer en aveugle.
                        Ok((ObservationOutcome::Indeterminate, None))
                    }
                    ResolvedState::Applied {
                        definition: rec_def,
                        state: rec_state,
                        revision: rec_rev,
                    } => {
                        // La résolution restitue l'état EFFECTIVEMENT enregistré
                        // pour `Applied` : l'effet a été ENGAGÉ historiquement
                        // (éventuellement supplanté depuis). L'effet enregistré
                        // est l'effet canonique à (ré)matérialiser si le store
                        // est en retard (`live.revision < rec_rev`).
                        if rec_def != definition.canonical_bytes() {
                            return Err(RevisionError::ValidationFailed(
                                "définition enregistrée différente de la définition tentée".into(),
                            ));
                        }
                        // L'effet enregistré porte un préfixe : 0x01 révision
                        // (suit l'état sérialisé) ou 0x02 formation.
                        let recorded = match rec_state.first() {
                            Some(&1) => ResultingState::from_canonical_bytes(&rec_state[9..])
                                .map_err(RevisionError::PersistenceFailed)?,
                            _ => {
                                return Err(RevisionError::PersistenceFailed(
                                    "effet enregistré non-révision (formation)".into(),
                                ));
                            }
                        };
                        let live = self
                            .store
                            .get(engram_id)
                            .ok_or(RevisionError::EngramAbsent(engram_id))?;
                        if live.revision < rec_rev {
                            let sig = self.spectral.encode(&recorded.ensemble);
                            if self
                                .store
                                .repair_state(
                                    engram_id,
                                    rec_rev,
                                    recorded.ensemble.clone(),
                                    recorded.strength,
                                    recorded.recall_count,
                                    sig.clone(),
                                )
                                .map_err(map_store)?
                            {
                                self.vector_index.upsert(&engram_id.to_string(), &sig);
                            }
                        }
                        Ok((
                            ObservationOutcome::Replayed {
                                state: recorded,
                                revision: rec_rev,
                                target: engram_id,
                            },
                            None,
                        ))
                    }
                    ResolvedState::Abandoned => Ok((ObservationOutcome::Abandoned, None)),
                }
            }
        }
    }

    // -----------------------------------------------------------------

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

    /// Le RECHARGEMENT depuis le substrat est ANNONCÉ : avec un souscripteur
    /// installé, les champs de l'événement (cohérents, dégradés, rejetés) sont
    /// ÉVALUÉS — un `tracing::info!` dont les champs ne sont jamais calculés ne
    /// serait qu'un commentaire déguisé.
    #[cfg(feature = "substrate")]
    #[test]
    fn le_rechargement_annonce_les_engrammes_restaures() {
        use std::sync::Mutex;
        use tracing::field::Visit;
        use tracing::subscriber::with_default;

        struct Journal(Mutex<Vec<(tracing::Level, String)>>);

        struct CapteurMessage<'a>(&'a mut Option<String>);
        impl Visit for CapteurMessage<'_> {
            fn record_debug(
                &mut self,
                champ: &tracing::field::Field,
                valeur: &dyn std::fmt::Debug,
            ) {
                if champ.name() == "message" {
                    *self.0 = Some(format!("{valeur:?}"));
                }
            }
        }

        impl tracing::Subscriber for Journal {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, evenement: &tracing::Event<'_>) {
                let mut message = None;
                evenement.record(&mut CapteurMessage(&mut message));
                if let Some(m) = message {
                    self.0
                        .lock()
                        .unwrap()
                        .push((*evenement.metadata().level(), m));
                }
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
            fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
                Some(tracing::level_filters::LevelFilter::TRACE)
            }
        }

        let td = tempfile::tempdir().unwrap();
        let chemin = td.path().join("kb");

        // Première session : un engramme FORMÉ, persisté dans le substrat.
        {
            let m = make_manager();
            let sous = obrain_substrate::SubstrateFile::create(&chemin).unwrap();
            let w = obrain_substrate::Writer::new(sous, obrain_substrate::SyncMode::Never).unwrap();
            m.attach_substrate(Arc::new(w));
            m.store()
                .form(&[NodeId(1), NodeId(2)])
                .expect("formation engramme");
        } // drop → les zones sont sur disque

        // Réouverture : le rapport de chargement doit être ANNONCÉ.
        let journal = Arc::new(Journal(Mutex::new(Vec::new())));
        let rapport = with_default(journal.clone(), || {
            let m = make_manager();
            let sous = obrain_substrate::SubstrateFile::open(&chemin).unwrap();
            let w = obrain_substrate::Writer::new(sous, obrain_substrate::SyncMode::Never).unwrap();
            m.attach_substrate(Arc::new(w))
        });

        assert!(
            rapport.coherent > 0,
            "un engramme cohérent est rechargé (rapport = {rapport:?})"
        );
        let evenements = journal.0.lock().unwrap();
        assert!(
            evenements
                .iter()
                .any(|(niveau, message)| *niveau == tracing::Level::INFO
                    && message.contains("engrams rechargés")),
            "le rechargement est annoncé : {evenements:?}"
        );
    }
}
