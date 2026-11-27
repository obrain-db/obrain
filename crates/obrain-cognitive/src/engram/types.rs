//! Core engram types — data model for the engram system.

use obrain_common::types::NodeId;
use serde::{Deserialize, Serialize};
use std::fmt;
use web_time::SystemTime;

// ---------------------------------------------------------------------------
// EngramId
// ---------------------------------------------------------------------------

/// Unique identifier for an engram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngramId(pub u64);

impl fmt::Display for EngramId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "engram:{}", self.0)
    }
}

/// Unique identifier for an episode (links to episodic memory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EpisodeId(pub u64);

// ---------------------------------------------------------------------------
// Engram — the core data structure
// ---------------------------------------------------------------------------

/// An engram is a consolidated memory trace formed from repeated co-activation.
///
/// It represents a group of nodes that consistently appear together across
/// multiple episodes, with associated strength, valence, and spectral signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engram {
    /// Unique engram identifier.
    pub id: EngramId,

    /// The node ensemble: (NodeId, contribution_weight) ordered temporally.
    /// The contribution weight reflects how central each node is to this engram.
    pub ensemble: Vec<(NodeId, f64)>,

    /// Spectral signature — a compact vector representation of this engram's
    /// "shape" in the graph. Used for similarity search and Hopfield retrieval.
    pub spectral_signature: Vec<f64>,

    /// Strength ∈ [0.0, 1.0] — how consolidated this engram is.
    /// Increases with recalls, decays via FSRS when not accessed.
    pub strength: f64,

    /// Valence ∈ [-1.0, 1.0] — emotional/outcome coloring.
    /// Negative = error/bug pattern, positive = successful pattern.
    pub valence: f64,

    /// Precision β for Modern Hopfield retrieval.
    /// Higher precision = sharper pattern matching, lower = fuzzier.
    pub precision: f64,

    /// History of recalls (when and in what context this engram was retrieved).
    pub recall_history: Vec<RecallEvent>,

    /// Source episodes that contributed to this engram's formation.
    pub source_episodes: Vec<EpisodeId>,

    /// Current memory horizon (operational, consolidated, archived).
    pub horizon: EngramHorizon,

    /// When this engram was first formed.
    pub created_at: SystemTime,

    /// When this engram was last recalled or reinforced.
    pub last_activated: SystemTime,

    /// FSRS scheduling state for spaced repetition decay.
    pub fsrs_state: FsrsState,

    /// Number of times this engram has been recalled.
    pub recall_count: u32,

    /// Révision canonique PERSISTANTE de CETTE connaissance (autorité unique du
    /// contrat d'identité d'observation). Avancée par toute mutation qui change
    /// l'état canonique (voir EngramStore::mutate). `serde(default)` = 0 pour
    /// les engrams antérieurs au champ.
    #[serde(default)]
    pub revision: u64,

    /// Source databases that contributed nodes to this engram's ensemble.
    /// When an engram is formed from nodes across multiple knowledge bases,
    /// this tracks which databases were involved (e.g., ["megalaw", "chess-kb"]).
    #[serde(default)]
    pub db_sources: Vec<String>,

    /// Cle d IDENTITE STRUCTURELLE optionnelle (relations/directions/roles/
    /// contexte). Deux engrams aux MEMES participants mais de cles structurelles
    /// differentes NE FUSIONNENT PAS (voir `form_observation_with`).
    #[serde(default)]
    pub structural_key: Option<String>,

    /// Vrai quand la cle structurelle ne correspond PLUS aux participants
    /// (une revision a change l ensemble). La definition et la cle doivent
    /// rester coherentes ; l appelant doit redefinir la cle.
    #[serde(default)]
    pub structural_key_stale: bool,

    /// Predictive model — P(outcome | context) for this engram.
    /// Stores mean + variance of observed outcomes. Used by the predictive
    /// coding layer (Layer 3+4) to compute prediction errors.
    #[cfg(feature = "engram")]
    #[serde(default)]
    pub predictive_model: Option<super::hopfield::PredictiveModel>,
}

impl Engram {
    /// Create a new engram with default values.
    pub fn new(id: EngramId, ensemble: Vec<(NodeId, f64)>) -> Self {
        let now = SystemTime::now();
        Self {
            id,
            ensemble,
            spectral_signature: Vec::new(),
            strength: 0.5,
            valence: 0.0,
            precision: 1.0,
            recall_history: Vec::new(),
            source_episodes: Vec::new(),
            horizon: EngramHorizon::Operational,
            created_at: now,
            last_activated: now,
            fsrs_state: FsrsState::default(),
            recall_count: 0,
            revision: 0,
            db_sources: Vec::new(),
            structural_key: None,
            structural_key_stale: false,
            #[cfg(feature = "engram")]
            predictive_model: None,
        }
    }

    /// Create a new engram from nodes across multiple databases.
    ///
    /// This is used when an engram's ensemble spans nodes from different
    /// knowledge bases (e.g., cross-base synapses formed via retrieval).
    pub fn new_multi_base(
        id: EngramId,
        ensemble: Vec<(NodeId, f64)>,
        db_sources: Vec<String>,
    ) -> Self {
        let mut engram = Self::new(id, ensemble);
        engram.db_sources = db_sources;
        engram
    }

    /// Returns the nodes in this engram's ensemble.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.ensemble.iter().map(|(id, _)| *id).collect()
    }

    /// Returns true if the engram's strength is above the consolidation threshold.
    pub fn is_consolidated(&self) -> bool {
        self.strength >= 0.6
    }

    /// Returns true if the engram is a candidate for crystallization (→ note).
    pub fn is_crystallization_candidate(&self) -> bool {
        self.strength > 0.85 && self.recall_count >= 5
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Memory horizon for engrams (similar to node MemoryHorizon but engram-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EngramHorizon {
    /// Hot: recently formed or actively recalled.
    Operational,
    /// Warm: stable, confirmed pattern.
    Consolidated,
    /// Cold: decaying, rarely recalled.
    Archived,
}

impl fmt::Display for EngramHorizon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operational => write!(f, "operational"),
            Self::Consolidated => write!(f, "consolidated"),
            Self::Archived => write!(f, "archived"),
        }
    }
}

/// A record of when and in what context an engram was recalled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallEvent {
    /// When the recall happened.
    pub timestamp: SystemTime,
    /// The cues that triggered this recall (e.g., file paths, node IDs).
    pub cues: Vec<String>,
    /// How useful the recall was (feedback from the consumer).
    pub feedback: Option<RecallFeedback>,
}

/// Feedback on a recall event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallFeedback {
    /// The recalled engram was useful.
    Used,
    /// The recalled engram was not useful.
    Rejected,
}

/// FSRS scheduling state for an engram.
///
/// Implements the Free Spaced Repetition Scheduler algorithm for
/// memory decay and optimal review scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsrsState {
    /// Stability — how long until the memory decays to 90% recall probability.
    pub stability: f64,
    /// Difficulty ∈ [0.0, 1.0] — how hard this pattern is to remember.
    pub difficulty: f64,
    /// Number of successful recalls (reviews).
    pub reps: u32,
    /// Number of consecutive lapses (forgetting events).
    pub lapses: u32,
    /// Last review timestamp.
    pub last_review: Option<SystemTime>,
    /// Scheduled next review timestamp.
    pub next_review: Option<SystemTime>,
}

impl Default for FsrsState {
    fn default() -> Self {
        Self {
            stability: 1.0,
            difficulty: 0.3,
            reps: 0,
            lapses: 0,
            last_review: None,
            next_review: None,
        }
    }
}

/// Prediction error from the predictive coding layer.
///
/// When the system predicts what should happen next and reality differs,
/// the prediction error is the main learning trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionError {
    /// The predicted outcome (expected node activations).
    pub predicted: Vec<NodeId>,
    /// The actual outcome (observed node activations).
    pub actual: Vec<NodeId>,
    /// Magnitude of the error ∈ [0.0, ∞).
    pub magnitude: f64,
    /// Surprise factor — how unexpected was this? (normalized 0-1)
    pub surprise: f64,
}

// ---------------------------------------------------------------------------
// ObservationScope — portee epistemique d une observation
// ---------------------------------------------------------------------------

/// Portee EPISTEMIQUE d une observation : decide si l ABSENCE d un noeud est
/// une preuve de disparition ou une simple NON-OBSERVATION.
///
/// C est le seul signal qui permet de differencier absence / ignorance /
/// contradiction sans categorie fixe : c est le locuteur (l appelant) qui sait
/// s il a observe la connaissance ENTIEREMENT ou seulement en partie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationScope {
    /// L appelant DECLARE une observation COMPLETE : un noeud absent serait
    /// reellement disparu. C est une DECLARATION, PAS une preuve : une
    /// declaration erronee detruit de la connaissance (voir tests).
    DeclaredComplete,
    /// L appelant DECLARE une observation PARTIELLE : un noeud absent n a
    /// simplement pas ete vu. La revision PRESERVE les non observes.
    DeclaredPartial,
}

// ---------------------------------------------------------------------------
// AdaptiveRevisionParams — constantes MESURABLES de la revision non monotone
// ---------------------------------------------------------------------------

/// Parametres de la revision NON MONOTONE. Les valeurs par defaut ne sont PAS
/// universelles : ce struct permet de MESURER leur effet (vitesse de
/// decroissance, plancher de retrait) au lieu de les tenir pour acquises.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AdaptiveRevisionParams {
    /// Taux d alignement de base (renforcement vers l observation).
    pub lr_base: f64,
    /// Gain du taux d alignement par l erreur de prediction.
    pub lr_error_gain: f64,
    /// Plafond du taux d alignement.
    pub lr_max: f64,
    /// Plancher de retrait : un noeud decroisse sous ce poids est retire.
    pub floor: f64,
}

impl Default for AdaptiveRevisionParams {
    fn default() -> Self {
        Self {
            lr_base: 0.3,
            lr_error_gain: 0.5,
            lr_max: 0.8,
            floor: 0.05,
        }
    }
}

impl AdaptiveRevisionParams {
    /// VALIDE les parametres AVANT tout calcul. Une valeur non finie, un taux
    /// negatif, un `lr_max` hors ]0,1], un `lr_base > lr_max` ou un plancher
    /// hors ]0,1] sont REJETES (erreur typee), jamais calcules silencieusement.
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("lr_base", self.lr_base),
            ("lr_error_gain", self.lr_error_gain),
            ("lr_max", self.lr_max),
            ("floor", self.floor),
        ] {
            if !v.is_finite() {
                return Err(format!("{name} non fini ({v})"));
            }
        }
        if self.lr_base < 0.0 || self.lr_error_gain < 0.0 {
            return Err("taux negatif".into());
        }
        if self.lr_max <= 0.0 || self.lr_max > 1.0 {
            return Err(format!("lr_max hors ]0,1] ({})", self.lr_max));
        }
        if self.lr_base > self.lr_max {
            return Err(format!("lr_base ({}) > lr_max ({})", self.lr_base, self.lr_max));
        }
        if self.floor <= 0.0 || self.floor > 1.0 {
            return Err(format!("floor hors ]0,1] ({})", self.floor));
        }
        Ok(())
    }
}
