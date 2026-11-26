//! Rappel hybride NATIF — premier chemin expérimental (suite de #422).
//!
//! ## Pourquoi ce module existe
//!
//! Les mesures de fidélité déjà publiées (`tests/spectral_fidelity.rs`, puis
//! `spectral::ranking_fidelity` et `spectral::hybrid_recall`) comparent des
//! signatures entre elles **dans un test** : elles balaient le corpus en
//! mémoire et n'interrogent jamais l'index. Un balayage exhaustif de signatures
//! n'est pas un rappel indexé, et il ne dit rien du coût réel.
//!
//! Ce module construit le chemin réel :
//!
//! ```text
//! indices ─► SpectralEncoder ─► VectorIndex::nearest(n_probe)   ← l'INDEX décide
//!          ─► validation exacte des candidats (cosinus pondéré)
//!          ─► classement final
//! ```
//!
//! ## Défaut du chemin existant, constaté avant d'écrire
//!
//! `RecallEngine::recall` n'utilise `VectorIndex::nearest` que si la matrice
//! Hopfield est **vide** (`recall_inner`). Dès qu'un engramme porte une
//! signature, la matrice est non vide et l'index natif n'est **jamais**
//! interrogé : le « rappel spectral » devient un produit scalaire sur une
//! matrice reconstruite à chaque changement de révision. Ce module ne corrige
//! pas ce chemin — il en ajoute un autre, mesurable, et le test d'intégration
//! mesure le premier (`nearest` appelé 0 fois) contre le second (1 fois).
//!
//! ## Quatre variantes comparées
//!
//! | Variante | Candidats | Validation exacte | Classement |
//! |---|---|---|---|
//! | `Exact` | tout le store (référence) | oui, exhaustive | score exact |
//! | `Spectral` | index natif | non | similarité de signature |
//! | `Hybrid` | index natif | oui, sur les candidats | score pondéré exact/signature |
//! | `HybridStructural` | index natif, filtré par la clôture des indices | oui | score pondéré |
//!
//! La porte structurelle de `HybridStructural` n'utilise **aucune** texture
//! lexicale : elle exige qu'un candidat partage au moins `structural_min_shared`
//! nœuds avec la clôture de co-appartenance des indices dans les engrammes
//! (1 saut par défaut). C'est une preuve de voisinage dans le graphe, pas une
//! similarité de surface.
//!
//! ## Limites assumées
//!
//! La variante `Exact` balaie le store : son coût est linéaire en nombre
//! d'engrammes et en taille d'ensemble. Les variantes indexées n'ont de gain que
//! si l'index coûte MOINS PAR COMPARAISON (empreintes compactes, quantification)
//! ou s'il évite des comparaisons — deux choses différentes, qu'aucune latence
//! seule ne départage. **Aucun index du dépôt n'est sous-linéaire en n** : les
//! paliers substrate balaient tous les vecteurs au niveau L0 (16 octets,
//! popcount) et ne gagnent donc qu'une constante, dans certaines configurations.
//! Les mesures du banc disent laquelle des deux causes a produit quel chiffre.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use obrain_common::types::NodeId;
use web_time::Instant;

use super::recall::RecallEngine;
use super::spectral::SpectralEncoder;
use super::store::EngramStore;
use super::traits::VectorIndex;
use super::types::{Engram, EngramId};

/// Budget de candidats demandé par défaut à l'index natif.
pub const DEFAULT_N_PROBE: usize = 32;

/// Borne dure du nombre de nœuds de la clôture structurelle.
///
/// Sans borne, une clôture à 2 sauts sur un graphe dense peut parcourir le
/// store entier ; le dépassement est rapporté (`closure_cap_hit`) au lieu
/// d'être silencieux.
pub const STRUCTURAL_CLOSURE_CAP: usize = 8192;

// ---------------------------------------------------------------------------
// Variantes
// ---------------------------------------------------------------------------

/// Les quatre chemins de rappel comparés par le banc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallVariant {
    /// (a) Balayage exact exhaustif du store — la référence de vérité.
    Exact,
    /// (b) Candidats de l'index natif classés par signature, sans validation.
    Spectral,
    /// (c) Candidats de l'index natif, validés exactement, rerankés.
    Hybrid,
    /// (d) (c) + porte structurelle sur la clôture des indices.
    HybridStructural,
}

impl RecallVariant {
    /// Nom court, pour les rapports de mesure.
    pub fn label(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Spectral => "spectral",
            Self::Hybrid => "hybride",
            Self::HybridStructural => "hybride+structurel",
        }
    }

    /// Vrai si la variante interroge l'index natif pour produire ses candidats.
    pub fn uses_index(self) -> bool {
        !matches!(self, Self::Exact)
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Réglages du chemin hybride.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridRecallConfig {
    /// Nombre de résultats rendus.
    pub k: usize,
    /// Nombre de candidats demandés à l'index natif.
    pub n_probe: usize,
    /// Poids du score exact dans le classement final, dans `[0, 1]`.
    ///
    /// `1.0` (défaut) = rerank exact pur, la similarité de signature ne sert
    /// plus qu'à départager les ex æquo. `0.0` = la validation exacte est
    /// calculée mais n'influence plus l'ordre (témoin du coût de la validation).
    pub exact_weight: f64,
    /// Profondeur de la clôture structurelle (`0` = porte désactivée).
    pub structural_hops: usize,
    /// Nombre minimal de nœuds partagés avec la clôture pour être retenu.
    pub structural_min_shared: usize,
    /// Fraction minimale de l'ensemble du candidat couverte par les indices.
    ///
    /// `0.0` (défaut) n'impose rien. C'est le seul réglage structurel qui
    /// DISCRIMINE : exiger `min_shared >= 1` revient, la clôture contenant les
    /// indices, à exiger un score exact non nul ; et à 2 sauts la clôture
    /// couvre le vocabulaire, donc le critère booléen devient vacueux (mesuré).
    /// Un seuil sur la fraction normalisée, lui, retire des candidats
    /// pertinents — c'est un compromis précision/rappel, pas une garantie.
    pub structural_min_overlap: f64,
}

impl Default for HybridRecallConfig {
    fn default() -> Self {
        Self {
            k: 10,
            n_probe: DEFAULT_N_PROBE,
            exact_weight: 1.0,
            structural_hops: 1,
            structural_min_shared: 1,
            structural_min_overlap: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Résultat
// ---------------------------------------------------------------------------

/// Un candidat retenu, avec toutes les preuves qui ont servi à le classer.
///
/// Les trois scores sont conservés séparément : un hit dont `final_score` est
/// confondu avec `index_score` cacherait le fait que la validation exacte n'a
/// rien changé, et un hit sans `shared_nodes` cacherait qu'il ne partage aucun
/// nœud avec la requête.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridHit {
    /// L'engramme retenu.
    pub engram_id: EngramId,
    /// Score de classement (combinaison pondérée exact/signature).
    pub final_score: f64,
    /// Cosinus pondéré exact entre l'ensemble de requête et l'ensemble de l'engramme.
    ///
    /// `None` signifie que **ce chemin n'a pas validé** : la variante `Spectral`
    /// classe sur la seule signature et n'exécute aucune validation. Un `0.0`
    /// aurait laissé croire qu'elle a validé et trouvé zéro.
    pub exact_score: Option<f64>,
    /// Similarité de signature rendue par l'index (`1 - distance`), bornée à `[0, 1]`.
    pub index_score: f64,
    /// Fraction de l'ensemble de l'engramme couverte par les indices.
    pub direct_overlap: f64,
    /// Nœuds de l'ensemble présents dans les indices.
    pub shared_nodes: usize,
    /// Nœuds de l'ensemble présents dans la clôture structurelle.
    ///
    /// `Some` seulement pour la variante `HybridStructural`, qui est la seule à
    /// construire la clôture ; `None` dit « non mesuré par ce chemin ».
    pub closure_shared: Option<usize>,
}

impl HybridHit {
    /// Vrai si l'engramme partage au moins un nœud avec les indices.
    ///
    /// Constaté sur les ensembles réels, pas sur la signature : un engramme qui
    /// ne partage aucun indice n'a aucune preuve de voisinage, quelle que soit
    /// sa similarité de projection.
    pub fn shares_node_with_query(&self) -> bool {
        self.shared_nodes > 0
    }
}

/// Compteurs d'un appel : c'est la partie « coût » de la mesure.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridStats {
    /// Variante exécutée.
    pub variant: RecallVariant,
    /// Nombre d'indices distincts après dédoublonnage.
    pub cues: usize,
    /// Appels à `VectorIndex::nearest` (0 pour `Exact`, 1 sinon).
    pub index_calls: usize,
    /// Dimension déclarée par l'index au moment de l'appel.
    pub index_dimensions: usize,
    /// Entrées rendues par l'index.
    pub candidates: usize,
    /// Identifiants d'index illisibles (ni `N` ni `engram:N`).
    pub unknown_ids: usize,
    /// Identifiants lisibles mais absents du store (index et store divergents).
    pub stale_ids: usize,
    /// Cosinus exacts réellement calculés.
    pub exact_validations: usize,
    /// Validations exactes à zéro : le candidat ne partage aucun nœud avec la requête.
    pub zero_score: usize,
    /// Candidats ayant passé la porte structurelle.
    pub structurally_valid: usize,
    /// Candidats rejetés par la porte structurelle.
    pub rejected_structural: usize,
    /// Taille de la clôture structurelle.
    pub closure_nodes: usize,
    /// Vrai si la clôture a atteint `STRUCTURAL_CLOSURE_CAP`.
    pub closure_cap_hit: bool,
    /// Durée mesurée du chemin complet.
    pub elapsed: Duration,
}

impl Default for HybridStats {
    fn default() -> Self {
        Self {
            variant: RecallVariant::Exact,
            cues: 0,
            index_calls: 0,
            index_dimensions: 0,
            candidates: 0,
            unknown_ids: 0,
            stale_ids: 0,
            exact_validations: 0,
            zero_score: 0,
            structurally_valid: 0,
            rejected_structural: 0,
            closure_nodes: 0,
            closure_cap_hit: false,
            elapsed: Duration::ZERO,
        }
    }
}

/// Résultat complet d'un appel : les hits et les compteurs de coût.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridOutcome {
    /// Hits classés, tronqués à `cfg.k`.
    pub hits: Vec<HybridHit>,
    /// Compteurs de coût et d'anomalie pour cet appel.
    pub stats: HybridStats,
}

// ---------------------------------------------------------------------------
// Erreurs
// ---------------------------------------------------------------------------

/// Erreur d'un appel hybride.
///
/// Un rappel vide n'est **pas** une erreur : c'est un résultat. Une requête
/// vide, un index de dimension incompatible ou un index vide alors que le store
/// ne l'est pas en sont : les rendre comme « aucun résultat » confondrait
/// « rien de similaire » et « le chemin n'a pas pu s'exécuter ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HybridRecallError {
    /// Aucun indice fourni.
    EmptyQuery,
    /// Budget nul : rien à demander, rien à rendre.
    EmptyBudget {
        /// Nombre de résultats demandés.
        k: usize,
        /// Nombre de candidats demandés à l'index.
        n_probe: usize,
    },
    /// La dimension de l'index diffère de celle de l'encodeur.
    DimensionMismatch {
        /// Dimension déclarée par l'index.
        index: usize,
        /// Dimension produite par l'encodeur pour la requête.
        query: usize,
    },
    /// L'index ne contient aucun vecteur alors que le store contient des engrammes.
    IndexEmpty {
        /// Nombre d'engrammes présents dans le store.
        store_engrams: usize,
    },
}

impl fmt::Display for HybridRecallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyQuery => write!(f, "requête vide : aucun indice fourni"),
            Self::EmptyBudget { k, n_probe } => {
                write!(f, "budget nul (k={k}, n_probe={n_probe}) : rien à demander")
            }
            Self::DimensionMismatch { index, query } => write!(
                f,
                "dimension incompatible : l'index porte {index} dimensions, la requête en \
                 encode {query}"
            ),
            Self::IndexEmpty { store_engrams } => write!(
                f,
                "index vide alors que le store contient {store_engrams} engrammes : les \
                 signatures n'ont pas été indexées"
            ),
        }
    }
}

impl std::error::Error for HybridRecallError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Interprète un identifiant d'index comme [`EngramId`].
///
/// Deux formes coexistent dans le code : `EngramManager` indexe
/// `EngramId::to_string()`, soit `"engram:7"`, alors que `recall_inner` ne sait
/// lire que `"7"` (`id_str.parse::<u64>()`, qui échoue donc en silence sur tout
/// ce que le manager a écrit). Les deux formes sont acceptées ici, et le test
/// compte les identifiants illisibles au lieu de les jeter.
pub fn parse_engram_id(raw: &str) -> Option<EngramId> {
    raw.strip_prefix("engram:")
        .unwrap_or(raw)
        .parse::<u64>()
        .ok()
        .map(EngramId)
}

/// Cosinus pondéré **exact** entre un ensemble de requête et un ensemble d'engramme.
///
/// C'est la référence de vérité de ce module : elle lit les nœuds et les poids
/// réels, là où la signature ne lit qu'une projection. Un poids non fini est
/// ignoré plutôt que propagé — une valeur `NaN` rendrait tout le classement
/// incomparable sans le dire.
fn exact_cosine(query: &HashMap<NodeId, f64>, ensemble: &[(NodeId, f64)]) -> f64 {
    if query.is_empty() || ensemble.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_e = 0.0;
    for (node, weight) in ensemble {
        if !weight.is_finite() {
            continue;
        }
        norm_e += weight * weight;
        if let Some(qw) = query.get(node) {
            dot += qw * weight;
        }
    }
    let norm_q: f64 = query
        .values()
        .filter(|w| w.is_finite())
        .map(|w| w * w)
        .sum();
    if norm_e <= 0.0 || norm_q <= 0.0 {
        return 0.0;
    }
    (dot / (norm_q.sqrt() * norm_e.sqrt())).clamp(-1.0, 1.0)
}

/// Nombre de nœuds de l'ensemble présents dans `set`.
fn shared_with(ensemble: &[(NodeId, f64)], set: &HashSet<NodeId>) -> usize {
    ensemble
        .iter()
        .filter(|(node, _)| set.contains(node))
        .count()
}

/// Clôture de co-appartenance : indices, plus les nœuds des engrammes qui les contiennent.
///
/// Rend `(nœuds, cap_atteint)`. `hops == 0` rend les indices seuls.
fn structural_closure(
    store: &EngramStore,
    cues: &[NodeId],
    hops: usize,
    cap: usize,
) -> (HashSet<NodeId>, bool) {
    let mut closure: HashSet<NodeId> = cues.iter().copied().collect();
    if hops == 0 {
        return (closure, false);
    }
    let mut frontier: Vec<NodeId> = cues.to_vec();
    for _ in 0..hops {
        let mut next: Vec<NodeId> = Vec::new();
        for node in &frontier {
            for engram_id in store.find_by_node(*node) {
                let Some(engram) = store.get(engram_id) else {
                    continue;
                };
                for (member, _) in &engram.ensemble {
                    if closure.insert(*member) {
                        next.push(*member);
                        if closure.len() >= cap {
                            return (closure, true);
                        }
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    (closure, false)
}

/// Un candidat scoré, avant conversion en [`HybridHit`].
struct Scored {
    engram: Engram,
    index_score: f64,
    exact_score: Option<f64>,
    direct_overlap: f64,
    shared_nodes: usize,
    closure_shared: Option<usize>,
}

impl Scored {
    fn into_hit(self, final_score: f64) -> HybridHit {
        HybridHit {
            engram_id: self.engram.id,
            final_score,
            exact_score: self.exact_score,
            index_score: self.index_score,
            direct_overlap: self.direct_overlap,
            shared_nodes: self.shared_nodes,
            closure_shared: self.closure_shared,
        }
    }
}

/// Balayage exact exhaustif — la référence.
fn exact_scan(
    store: &EngramStore,
    query: &HashMap<NodeId, f64>,
    query_set: &HashSet<NodeId>,
    stats: &mut HybridStats,
) -> Vec<Scored> {
    let mut scored: Vec<Scored> = Vec::new();
    store.for_each(|engram| {
        let exact = exact_cosine(query, &engram.ensemble);
        stats.exact_validations += 1;
        if exact <= 0.0 {
            stats.zero_score += 1;
            return;
        }
        let shared = shared_with(&engram.ensemble, query_set);
        let total = engram.ensemble.len().max(1);
        scored.push(Scored {
            index_score: 0.0,
            exact_score: Some(exact),
            direct_overlap: shared as f64 / total as f64,
            shared_nodes: shared,
            closure_shared: None,
            engram: engram.clone(),
        });
    });
    scored
}

/// Contexte d'une génération de candidats.
///
/// Regroupé pour que le chemin reste lisible : les sept références décrivent
/// UNE requête, pas sept réglages indépendants.
struct CandidateSource<'a> {
    store: &'a EngramStore,
    vector_index: &'a dyn VectorIndex,
    spectral: &'a SpectralEncoder,
    query_nodes: &'a [NodeId],
    query: &'a HashMap<NodeId, f64>,
    query_set: &'a HashSet<NodeId>,
    cfg: &'a HybridRecallConfig,
    /// Vrai si le chemin doit calculer le score exact des candidats.
    validate: bool,
}

/// Candidats produits par l'index natif, validés exactement.
fn index_candidates(
    src: &CandidateSource<'_>,
    stats: &mut HybridStats,
) -> Result<Vec<Scored>, HybridRecallError> {
    let CandidateSource {
        store,
        vector_index,
        spectral,
        query_nodes,
        query,
        query_set,
        cfg,
        validate,
    } = *src;
    let query_ensemble: Vec<(NodeId, f64)> = query_nodes.iter().map(|&n| (n, 1.0)).collect();
    let query_vec = spectral.encode(&query_ensemble);

    let dims = vector_index.dimensions();
    stats.index_dimensions = dims;
    if dims == 0 {
        let store_engrams = store.count();
        if store_engrams > 0 {
            return Err(HybridRecallError::IndexEmpty { store_engrams });
        }
        return Ok(Vec::new());
    }
    if dims != query_vec.len() {
        return Err(HybridRecallError::DimensionMismatch {
            index: dims,
            query: query_vec.len(),
        });
    }

    stats.index_calls += 1;
    let neighbours = vector_index.nearest(&query_vec, cfg.n_probe);
    stats.candidates = neighbours.len();

    let mut scored: Vec<Scored> = Vec::with_capacity(neighbours.len());
    for (raw_id, distance) in neighbours {
        let Some(engram_id) = parse_engram_id(&raw_id) else {
            stats.unknown_ids += 1;
            continue;
        };
        let Some(engram) = store.get(engram_id) else {
            stats.stale_ids += 1;
            continue;
        };
        // La validation exacte est le travail que la variante `Spectral` ne fait
        // PAS : la décider ici, et non après coup, évite de mesurer un coût que
        // le chemin n'a pas payé.
        let exact = if validate {
            let value = exact_cosine(query, &engram.ensemble);
            stats.exact_validations += 1;
            if value <= 0.0 {
                stats.zero_score += 1;
            }
            Some(value)
        } else {
            None
        };
        let shared = shared_with(&engram.ensemble, query_set);
        let total = engram.ensemble.len().max(1);
        scored.push(Scored {
            index_score: (1.0 - distance).clamp(0.0, 1.0),
            exact_score: exact,
            direct_overlap: shared as f64 / total as f64,
            shared_nodes: shared,
            closure_shared: None,
            engram,
        });
    }
    Ok(scored)
}

/// Classement par similarité de signature seule.
fn rank_spectral(mut scored: Vec<Scored>, k: usize) -> Vec<HybridHit> {
    scored.sort_by(|a, b| {
        b.index_score
            .partial_cmp(&a.index_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.engram.id.0.cmp(&b.engram.id.0))
    });
    scored.truncate(k);
    scored
        .into_iter()
        .map(|s| {
            let score = s.index_score;
            s.into_hit(score)
        })
        .collect()
}

/// Classement pondéré exact/signature.
fn rank_hybrid(mut scored: Vec<Scored>, cfg: &HybridRecallConfig, k: usize) -> Vec<HybridHit> {
    let w_exact = cfg.exact_weight.clamp(0.0, 1.0);
    let w_index = 1.0 - w_exact;
    let mut ranked: Vec<(f64, Scored)> = scored
        .drain(..)
        .map(|s| {
            let exact = s.exact_score.unwrap_or(0.0);
            let score = (w_exact * exact + w_index * s.index_score).clamp(0.0, 1.0);
            (score, s)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.engram.id.0.cmp(&b.1.engram.id.0))
    });
    ranked.truncate(k);
    ranked
        .into_iter()
        .map(|(score, s)| s.into_hit(score))
        .collect()
}

// ---------------------------------------------------------------------------
// Le chemin, greffé sur RecallEngine
// ---------------------------------------------------------------------------

impl RecallEngine {
    /// Rappel hybride : candidats de l'index natif, validation exacte, classement final.
    ///
    /// Le chemin est celui annoncé en tête de module :
    /// `indices → encodeur → VectorIndex::nearest → validation exacte → classement`.
    /// Rien n'est balayé exhaustivement hors de la variante `Exact`, qui est la
    /// référence par construction.
    ///
    /// # Erreurs
    ///
    /// Voir [`HybridRecallError`] : une requête vide, un budget nul, une
    /// dimension d'index incompatible et un index vide sur un store non vide
    /// sont rapportés, jamais maquillés en « aucun résultat ».
    pub fn hybrid_recall(
        &self,
        store: &EngramStore,
        cues: &[NodeId],
        vector_index: &dyn VectorIndex,
        spectral: &SpectralEncoder,
        variant: RecallVariant,
        cfg: &HybridRecallConfig,
    ) -> Result<HybridOutcome, HybridRecallError> {
        let started = Instant::now();
        if cues.is_empty() {
            return Err(HybridRecallError::EmptyQuery);
        }
        if cfg.k == 0 || cfg.n_probe == 0 {
            return Err(HybridRecallError::EmptyBudget {
                k: cfg.k,
                n_probe: cfg.n_probe,
            });
        }

        let mut seen: HashSet<NodeId> = HashSet::with_capacity(cues.len());
        let query_nodes: Vec<NodeId> = cues
            .iter()
            .copied()
            .filter(|node| seen.insert(*node))
            .collect();
        let query: HashMap<NodeId, f64> = query_nodes.iter().map(|&n| (n, 1.0)).collect();
        let query_set: HashSet<NodeId> = query_nodes.iter().copied().collect();

        let mut stats = HybridStats {
            variant,
            cues: query_nodes.len(),
            ..HybridStats::default()
        };

        let hits = match variant {
            RecallVariant::Exact => {
                let scored = exact_scan(store, &query, &query_set, &mut stats);
                rank_hybrid(scored, cfg, cfg.k)
            }
            RecallVariant::Spectral => {
                let scored = index_candidates(
                    &CandidateSource {
                        store,
                        vector_index,
                        spectral,
                        query_nodes: &query_nodes,
                        query: &query,
                        query_set: &query_set,
                        cfg,
                        validate: false,
                    },
                    &mut stats,
                )?;
                rank_spectral(scored, cfg.k)
            }
            RecallVariant::Hybrid => {
                let scored = index_candidates(
                    &CandidateSource {
                        store,
                        vector_index,
                        spectral,
                        query_nodes: &query_nodes,
                        query: &query,
                        query_set: &query_set,
                        cfg,
                        validate: true,
                    },
                    &mut stats,
                )?;
                rank_hybrid(scored, cfg, cfg.k)
            }
            RecallVariant::HybridStructural => {
                let scored = index_candidates(
                    &CandidateSource {
                        store,
                        vector_index,
                        spectral,
                        query_nodes: &query_nodes,
                        query: &query,
                        query_set: &query_set,
                        cfg,
                        validate: true,
                    },
                    &mut stats,
                )?;
                let (closure, cap_hit) = structural_closure(
                    store,
                    &query_nodes,
                    cfg.structural_hops,
                    STRUCTURAL_CLOSURE_CAP,
                );
                stats.closure_nodes = closure.len();
                stats.closure_cap_hit = cap_hit;
                let min_shared = cfg.structural_min_shared.max(1);
                let min_overlap = cfg.structural_min_overlap.clamp(0.0, 1.0);
                let mut kept: Vec<Scored> = Vec::with_capacity(scored.len());
                for mut candidate in scored {
                    let in_closure = shared_with(&candidate.engram.ensemble, &closure);
                    candidate.closure_shared = Some(in_closure);
                    // La porte structurelle exige les deux preuves : appartenance à
                    // la clôture ET densité de recouvrement suffisante.
                    if in_closure >= min_shared && candidate.direct_overlap >= min_overlap {
                        kept.push(candidate);
                    } else {
                        stats.rejected_structural += 1;
                    }
                }
                stats.structurally_valid = kept.len();
                rank_hybrid(kept, cfg, cfg.k)
            }
        };

        stats.elapsed = started.elapsed();
        Ok(HybridOutcome { hits, stats })
    }
}

// ---------------------------------------------------------------------------
// Tests unitaires du module
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::traits::InMemoryVectorIndex;
    use std::sync::Arc;

    fn ensemble(pairs: &[(u64, f64)]) -> Vec<(NodeId, f64)> {
        pairs.iter().map(|&(n, w)| (NodeId(n), w)).collect()
    }

    fn map_of(ensemble: &[(NodeId, f64)]) -> HashMap<NodeId, f64> {
        ensemble.iter().copied().collect()
    }

    fn store_with(ensembles: &[Vec<(NodeId, f64)>]) -> EngramStore {
        let store = EngramStore::new(None);
        for (i, ensemble) in ensembles.iter().enumerate() {
            let mut engram = Engram::new(EngramId(i as u64 + 1), ensemble.clone());
            engram.spectral_signature = SpectralEncoder::new().encode(ensemble);
            store.insert(engram);
        }
        store
    }

    #[test]
    fn exact_cosine_identity_and_disjoint() {
        let e = ensemble(&[(1, 1.0), (2, 0.5)]);
        let q = map_of(&e);
        assert!((exact_cosine(&q, &e) - 1.0).abs() < 1e-12, "identité = 1");

        let other = ensemble(&[(3, 1.0), (4, 1.0)]);
        assert!(exact_cosine(&q, &other).abs() < 1e-12, "disjoint = 0");
    }

    #[test]
    fn exact_cosine_ranks_partial_overlap_below_partial_heavy() {
        let q = map_of(&ensemble(&[(1, 1.0), (2, 1.0), (3, 1.0)]));
        let light = ensemble(&[(1, 0.2), (7, 1.0)]);
        let heavy = ensemble(&[(1, 1.0), (2, 1.0), (9, 1.0)]);
        assert!(exact_cosine(&q, &heavy) > exact_cosine(&q, &light));
    }

    #[test]
    fn exact_cosine_ignores_non_finite_weights() {
        let q = map_of(&ensemble(&[(1, 1.0)]));
        let bad = ensemble(&[(1, f64::NAN), (2, 1.0)]);
        let score = exact_cosine(&q, &bad);
        assert!(
            score.is_finite(),
            "un poids NaN ne doit pas rendre le score NaN"
        );
    }

    #[test]
    fn parse_accepts_both_id_forms() {
        assert_eq!(parse_engram_id("7"), Some(EngramId(7)));
        assert_eq!(parse_engram_id("engram:7"), Some(EngramId(7)));
        assert_eq!(parse_engram_id("engramme"), None);
        assert_eq!(parse_engram_id(""), None);
    }

    #[test]
    fn structural_closure_reaches_co_members() {
        let store = store_with(&[
            ensemble(&[(1, 1.0), (2, 1.0)]),
            ensemble(&[(2, 1.0), (3, 1.0)]),
        ]);
        let (one_hop, cap) = structural_closure(&store, &[NodeId(1)], 1, 1024);
        assert!(!cap);
        assert!(one_hop.contains(&NodeId(2)), "co-membre à 1 saut");
        assert!(
            !one_hop.contains(&NodeId(3)),
            "2 sauts ne sont pas dans la clôture 1 saut"
        );

        let (two_hops, _) = structural_closure(&store, &[NodeId(1)], 2, 1024);
        assert!(two_hops.contains(&NodeId(3)), "co-membre à 2 sauts");

        let (no_hops, _) = structural_closure(&store, &[NodeId(1)], 0, 1024);
        assert_eq!(no_hops.len(), 1, "hops=0 rend les indices seuls");
    }

    #[test]
    fn structural_closure_reports_cap() {
        let store = store_with(&[ensemble(&[(1, 1.0), (2, 1.0), (3, 1.0), (4, 1.0)])]);
        let (closure, cap) = structural_closure(&store, &[NodeId(1)], 1, 2);
        assert!(cap, "le dépassement de borne est rapporté");
        assert!(closure.len() >= 2);
    }

    #[test]
    fn empty_query_is_an_error_not_an_empty_result() {
        let store = store_with(&[ensemble(&[(1, 1.0)])]);
        let index = InMemoryVectorIndex::new();
        let engine = RecallEngine::new();
        let err = engine
            .hybrid_recall(
                &store,
                &[],
                &index,
                &SpectralEncoder::new(),
                RecallVariant::Hybrid,
                &HybridRecallConfig::default(),
            )
            .expect_err("requête vide = erreur");
        assert_eq!(err, HybridRecallError::EmptyQuery);
    }

    #[test]
    fn empty_index_over_non_empty_store_is_an_error() {
        let store = store_with(&[ensemble(&[(1, 1.0), (2, 1.0)])]);
        let index = InMemoryVectorIndex::new(); // jamais alimenté
        let engine = RecallEngine::new();
        let err = engine
            .hybrid_recall(
                &store,
                &[NodeId(1)],
                &index,
                &SpectralEncoder::new(),
                RecallVariant::Hybrid,
                &HybridRecallConfig::default(),
            )
            .expect_err("index vide + store non vide = erreur");
        assert_eq!(err, HybridRecallError::IndexEmpty { store_engrams: 1 });
    }

    #[test]
    fn hybrid_path_really_uses_the_index_and_validates_exactly() {
        let ensembles = vec![
            ensemble(&[(1, 1.0), (2, 1.0)]),
            ensemble(&[(2, 1.0), (3, 1.0)]),
            ensemble(&[(90, 1.0), (91, 1.0)]),
        ];
        let store = store_with(&ensembles);
        let encoder = SpectralEncoder::new();
        let index = InMemoryVectorIndex::new();
        for e in &ensembles {
            let id = store
                .list()
                .into_iter()
                .find(|engram| engram.ensemble == *e)
                .map(|engram| engram.id)
                .expect("engramme présent");
            index.upsert(&id.to_string(), &encoder.encode(e));
        }
        let engine = RecallEngine::new();
        let cfg = HybridRecallConfig {
            k: 3,
            n_probe: 3,
            ..HybridRecallConfig::default()
        };
        let outcome = engine
            .hybrid_recall(
                &store,
                &[NodeId(1), NodeId(2)],
                &index,
                &encoder,
                RecallVariant::Hybrid,
                &cfg,
            )
            .expect("le chemin s'exécute");

        assert_eq!(
            outcome.stats.index_calls, 1,
            "l'index natif est interrogé une fois"
        );
        assert_eq!(outcome.stats.candidates, 3);
        assert_eq!(
            outcome.stats.stale_ids, 0,
            "les ids « engram:N » sont lisibles"
        );
        assert_eq!(outcome.hits.len(), 3, "les trois candidats sont retenus");

        // Le score exact rendu est bien le cosinus pondéré recalculé ici.
        let query = map_of(&ensemble(&[(1, 1.0), (2, 1.0)]));
        for hit in &outcome.hits {
            let engram = store.get(hit.engram_id).expect("engramme présent");
            let expected = exact_cosine(&query, &engram.ensemble);
            let validated = hit
                .exact_score
                .expect("la variante hybride valide exactement");
            assert!(
                (validated - expected).abs() < 1e-12,
                "exact_score doit être le cosinus exact, pas la distance de l'index"
            );
        }
        assert_eq!(
            outcome.hits[0].engram_id,
            EngramId(1),
            "l'engramme qui partage les deux indices arrive en tête"
        );
    }

    #[test]
    fn structural_gate_rejects_candidates_without_shared_node() {
        let ensembles = vec![
            ensemble(&[(1, 1.0), (2, 1.0)]),
            ensemble(&[(90, 1.0), (91, 1.0)]),
        ];
        let store = store_with(&ensembles);
        let encoder = SpectralEncoder::new();
        let index = InMemoryVectorIndex::new();
        for engram in store.list() {
            index.upsert(
                &format!("engram:{}", engram.id.0),
                &engram.spectral_signature,
            );
        }
        let engine = RecallEngine::new();
        let cfg = HybridRecallConfig {
            k: 5,
            n_probe: 5,
            structural_hops: 1,
            structural_min_shared: 1,
            ..HybridRecallConfig::default()
        };
        let outcome = engine
            .hybrid_recall(
                &store,
                &[NodeId(1)],
                &index,
                &encoder,
                RecallVariant::HybridStructural,
                &cfg,
            )
            .expect("le chemin s'exécute");
        assert!(
            outcome.hits.iter().all(|hit| hit.shares_node_with_query()),
            "aucun hit sans recouvrement structurel"
        );
        assert!(
            outcome
                .hits
                .iter()
                .all(|hit| hit.closure_shared.is_some_and(|n| n >= 1)),
            "la porte mesure le recouvrement avec la clôture, pas seulement les indices"
        );
        assert_eq!(
            outcome.stats.rejected_structural,
            outcome.stats.candidates - outcome.hits.len(),
            "tout candidat non retenu est compté comme rejeté"
        );
    }

    #[test]
    fn exact_variant_never_touches_the_index() {
        let store = store_with(&[ensemble(&[(1, 1.0), (2, 1.0)])]);
        let index = InMemoryVectorIndex::new();
        let engine = RecallEngine::new();
        let outcome = engine
            .hybrid_recall(
                &store,
                &[NodeId(1)],
                &index,
                &SpectralEncoder::new(),
                RecallVariant::Exact,
                &HybridRecallConfig::default(),
            )
            .expect("la référence ne dépend pas de l'index");
        assert_eq!(outcome.stats.index_calls, 0);
        assert_eq!(outcome.stats.exact_validations, 1);
    }

    #[test]
    fn dimension_mismatch_is_an_error() {
        let store = store_with(&[ensemble(&[(1, 1.0), (2, 1.0)])]);
        let encoder = SpectralEncoder::new();
        let index = InMemoryVectorIndex::new();
        index.upsert("1", &[0.0; 8]); // 8 dims, l'encodeur en produit 64
        let engine = RecallEngine::new();
        let err = engine
            .hybrid_recall(
                &store,
                &[NodeId(1)],
                &index,
                &encoder,
                RecallVariant::Spectral,
                &HybridRecallConfig::default(),
            )
            .expect_err("dimension incompatible = erreur");
        assert_eq!(
            err,
            HybridRecallError::DimensionMismatch {
                index: 8,
                query: encoder.dimensions()
            }
        );
    }

    #[test]
    fn spectral_variant_reports_a_degenerate_score_without_validation() {
        let store = store_with(&[ensemble(&[(1, 1.0), (2, 1.0)])]);
        let encoder = SpectralEncoder::new();
        let index: Arc<dyn VectorIndex> = Arc::new(InMemoryVectorIndex::new());
        index.upsert(
            "engram:1",
            &encoder.encode(&ensemble(&[(1, 1.0), (2, 1.0)])),
        );
        let engine = RecallEngine::new();
        let outcome = engine
            .hybrid_recall(
                &store,
                &[NodeId(1)],
                index.as_ref(),
                &encoder,
                RecallVariant::Spectral,
                &HybridRecallConfig::default(),
            )
            .expect("le chemin s'exécute");
        assert_eq!(
            outcome.stats.exact_validations, 0,
            "aucune validation en spectral"
        );
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(
            outcome.hits[0].exact_score, None,
            "le chemin spectral ne valide pas : None dit « non mesuré », 0.0 dirait « mesuré à zéro »"
        );
    }
}
