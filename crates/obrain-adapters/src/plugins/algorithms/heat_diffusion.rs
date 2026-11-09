//! Heat kernel diffusion on Hilbert feature space.
//!
//! Propagates heat from seed nodes through the graph using Gaussian kernel
//! weights derived from weighted Hilbert distances. This enables
//! similarity-aware signal propagation that respects the multi-facette
//! structure of the feature space.
//!
//! ## Algorithm
//!
//! ```text
//! heat(v, t+1) = α × heat(v, t) + (1-α) × Σ_u∈N(v) w(u,v) × heat(u, t) / Σ_u∈N(v) w(u,v)
//! ```
//!
//! Where `w(u,v) = exp(-d² / σ²)` with `d = weighted_hilbert_distance(u, v, weights, levels)`.

use std::collections::HashMap;
use std::sync::OnceLock;

use obrain_common::types::{NodeId, Value};
use obrain_common::utils::error::Result;
use obrain_core::graph::{Direction, GraphStore};

use super::super::{AlgorithmResult, ParameterDef, ParameterType, Parameters};
use super::hilbert_features::{FacetteWeights, HilbertFeaturesResult, weighted_hilbert_distance};
use super::traits::GraphAlgorithm;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for heat kernel diffusion.
#[derive(Debug, Clone)]
pub struct HeatKernelConfig {
    /// Number of diffusion iterations (default: 4).
    pub iterations: usize,
    /// Retention factor: how much of a node's own heat is kept each step (default: 0.4).
    pub alpha: f32,
    /// Gaussian kernel bandwidth. `None` triggers auto-calibration from edge distances.
    pub sigma_sq: Option<f32>,
}

impl Default for HeatKernelConfig {
    fn default() -> Self {
        Self {
            iterations: 4,
            alpha: 0.4,
            sigma_sq: None,
        }
    }
}

// ============================================================================
// Auto-calibration
// ============================================================================

/// Auto-calibrate σ² by sampling edge distances.
///
/// Samples up to 1000 edges, computes the squared weighted Hilbert distance
/// for each, and returns the median as σ². Falls back to 1.0 if no edges
/// are found or all distances are zero.
pub fn calibrate_sigma(
    store: &dyn GraphStore,
    features: &HilbertFeaturesResult,
    weights: &FacetteWeights,
    levels: usize,
) -> f32 {
    let node_ids = store.node_ids();
    let mut dist_sq_samples: Vec<f32> = Vec::with_capacity(1000);

    'outer: for &nid in &node_ids {
        if let Some(feat_a) = features.features.get(&nid) {
            for (neighbor, _) in store.edges_from(nid, Direction::Outgoing) {
                if let Some(feat_b) = features.features.get(&neighbor) {
                    let d = weighted_hilbert_distance(feat_a, feat_b, weights, levels);
                    dist_sq_samples.push(d * d);
                    if dist_sq_samples.len() >= 1000 {
                        break 'outer;
                    }
                }
            }
        }
    }

    if dist_sq_samples.is_empty() {
        return 1.0;
    }

    // Median of squared distances
    dist_sq_samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = dist_sq_samples[dist_sq_samples.len() / 2];

    if median <= 0.0 || !median.is_finite() {
        1.0
    } else {
        median
    }
}

// ============================================================================
// Core diffusion
// ============================================================================

/// Run heat kernel diffusion from seed nodes.
///
/// Seeds start with heat 1.0; all other nodes start at 0.0. At each iteration,
/// heat is mixed with neighbors using Gaussian kernel weights derived from
/// weighted Hilbert distance.
///
/// Returns nodes sorted by heat descending. Nodes with zero heat are excluded.
/// (2026-08-15) TABLE DES VOISINS MISE EN CACHE — elle ne depend pas des graines.
///
/// `heat_kernel_diffusion` reconstruisait `neighbor_weights` pour les 480 000
/// noeuds A CHAQUE APPEL. Mesure : la phase `retrieve` d'une construction de
/// prompt coutait 3466 ms de mediane (p90 6122 ms), soit 89 % des 3,6 s de
/// construction — et donc 89 % de l'occupation des 16 places de la fabrique,
/// qui plafonne le debit du systeme entier.
///
/// Or cette table ne depend QUE de la topologie, des features et des poids.
/// Seul le vecteur de chaleur initial depend des graines. On recalculait 99 %
/// d'un resultat invariant entre deux requetes.
struct CacheVoisins {
    empreinte: u64,
    voisins: HashMap<NodeId, Vec<(NodeId, f32)>>,
}

/// (2026-08-15) UNE ENTREE PAR EMPREINTE, PAS UNE SEULE.
///
/// Premiere version : `Option<CacheVoisins>`. Mesure sur 200 appels de
/// production : 93 CHAUD / 72 FROID, soit 56 % seulement. Deux chemins
/// appellent la diffusion avec des poids differents —
/// `ExocortexConfig::default()` pour l'endpoint, `WeightProfile::Proximity`
/// pour la construction de prompt — et chacun evincait l'autre. 44 % des
/// constructions repayaient les 3,3 s de table.
///
/// Quatre places : de quoi loger les deux profils connus avec de la marge.
/// ~8 Mo l'entree (352 562 aretes x 2 sens), donc 32 Mo au pire.
static CACHE_VOISINS: std::sync::OnceLock<std::sync::RwLock<Vec<CacheVoisins>>> =
    std::sync::OnceLock::new();

/// Nombre d'empreintes gardees simultanement. Deux profils sont en usage ;
/// quatre laisse de la marge sans laisser la table croitre sans fin.
const CACHE_PLACES: usize = 4;

/// Un seul constructeur a la fois. Les autres NE DEGRADENT PAS : ils
/// recalculent en entier. Servir un index partiel etait acceptable ailleurs
/// (« perime, PAS faux ») ; servir une diffusion partielle donnerait un
/// resultat FAUX en silence.
static CACHE_EN_CONSTRUCTION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Empreinte de CONTENU, pas d'adresse.
///
/// Deux allocations successives peuvent tomber sur la meme adresse memoire ;
/// se fier a un pointeur ferait servir la table d'anciennes features — une
/// faute silencieuse qu'on passe des jours a chercher. On melange donc les
/// tailles, les poids, et les vecteurs des NodeId extremes (deterministes et
/// trouves en une passe).
fn empreinte_features(
    features: &HilbertFeaturesResult,
    weights: &FacetteWeights,
    n_noeuds: usize,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // (2026-08-15) LE NOMBRE DE NOEUDS EST VOLONTAIREMENT EXCLU.
    //
    // Premiere version : il en faisait partie. Mesure : « cache FROID » sur
    // 100 % des appels — la base agent grandit en continu (371 995
    // Perception, une de plus a chaque pas), donc l'empreinte changeait a
    // chaque requete. Un cache dont la clef ne peut jamais se repeter.
    //
    // Et il n'a rien a faire ici : la table n'indexe que les noeuds PRESENTS
    // DANS LES FEATURES (`features.features.get(&nid)` ecarte les autres).
    // Un noeud arrive depuis le calcul des features n'y contribuerait pas,
    // qu'on invalide ou non. La clef suit donc les FEATURES, dont le TTL de
    // 120 s est exactement la duree de vie voulue pour ce cache.
    let _ = n_noeuds;
    features.dimensions.hash(&mut h);
    features.features.len().hash(&mut h);
    features.raw_facettes.len().hash(&mut h);
    for w in &weights.weights {
        w.to_bits().hash(&mut h);
    }
    // Les deux extremes : deterministes quel que soit l'ordre de la table.
    let mut mini: Option<(&NodeId, &Vec<f32>)> = None;
    let mut maxi: Option<(&NodeId, &Vec<f32>)> = None;
    for (id, v) in &features.features {
        match mini {
            Some((m, _)) if m <= id => {}
            _ => mini = Some((id, v)),
        }
        match maxi {
            Some((m, _)) if m >= id => {}
            _ => maxi = Some((id, v)),
        }
    }
    for extreme in [mini, maxi].into_iter().flatten() {
        extreme.0.hash(&mut h);
        for x in extreme.1.iter().take(16) {
            x.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// Diffuse la chaleur depuis `seeds` sur le graphe de `store`, avec un noyau
/// gaussien sur la distance de Hilbert pondérée entre voisins (`weights`).
///
/// Rend les nœuds de chaleur finale non nulle après `config.iterations` pas,
/// triés par chaleur décroissante ; vide si `seeds` ou le graphe est vide.
/// La table des voisins est mise en cache par empreinte des features (voir
/// `CacheVoisins`).
pub fn heat_kernel_diffusion(
    store: &dyn GraphStore,
    features: &HilbertFeaturesResult,
    seeds: &[NodeId],
    weights: &FacetteWeights,
    config: &HeatKernelConfig,
) -> Vec<(NodeId, f32)> {
    if seeds.is_empty() {
        return Vec::new();
    }

    // (2026-08-15) CHRONOMETRAGE PAR PHASE — on mesure au lieu de supposer.
    // Trois attributions fausses en une nuit ont suffi : famine CPU, plafond
    // d'allumage, table des voisins. Le journal tranchera.
    let chrono = std::env::var("OBRAIN_TMN_CHRONO_CHALEUR").is_ok();
    let t_ids = web_time::Instant::now();
    let node_ids = store.node_ids();
    let ms_ids = t_ids.elapsed().as_millis() as u64;
    if node_ids.is_empty() {
        return Vec::new();
    }

    // Determine σ²
    let levels = if weights.is_empty() {
        0
    } else {
        let sample_len = features.features.values().next().map_or(0, |v| v.len());
        if !weights.is_empty() {
            sample_len / weights.len()
        } else {
            0
        }
    };

    let sigma_sq = config
        .sigma_sq
        .unwrap_or_else(|| calibrate_sigma(store, features, weights, levels));

    // Guard: if sigma_sq is zero or non-finite, use 1.0
    let sigma_sq = if sigma_sq <= 0.0 || !sigma_sq.is_finite() {
        1.0
    } else {
        sigma_sq
    };
    let ms_sigma = t_ids.elapsed().as_millis() as u64 - ms_ids;

    // ── (2026-08-15) LA TABLE EST-ELLE DEJA LA ? ──
    //
    // Elle ne depend pas des graines : meme features + memes poids + meme
    // graphe => meme table. Lecture BREVE sous verrou partage, puis on rend
    // la main. Si la clef correspond, on saute les 480 000 reconstructions.
    let empreinte = empreinte_features(features, weights, node_ids.len());
    let cache = CACHE_VOISINS.get_or_init(|| std::sync::RwLock::new(Vec::new()));
    {
        let lu = match cache.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(c) = lu.iter().find(|c| c.empreinte == empreinte) {
            {
                let t_diff = web_time::Instant::now();
                let r = diffuser(&node_ids, seeds, &c.voisins, config);
                if chrono {
                    tracing::info!(
                        target: "obrain_tmn::exochrono",
                        "CHRONO CHALEUR (cache CHAUD) ids={}ms sigma={}ms empreinte={}ms diffusion={}ms",
                        ms_ids, ms_sigma,
                        t_ids.elapsed().as_millis() as u64 - ms_ids - ms_sigma
                            - t_diff.elapsed().as_millis() as u64,
                        t_diff.elapsed().as_millis() as u64
                    );
                }
                return r;
            }
        }
    }

    // Pre-compute neighbor lists + kernel weights
    let mut neighbor_weights: HashMap<NodeId, Vec<(NodeId, f32)>> =
        HashMap::with_capacity(node_ids.len());
    for &nid in &node_ids {
        // (2026-07-18) Direction::Both, pas Outgoing : un noyau de chaleur
        // est un operateur SYMETRIQUE (Laplacien de graphe) — la chaleur
        // diffuse dans les deux sens. En Outgoing seul, la diffusion ne
        // \"tirait\" que le long des aretes sortantes : toute graine
        // puits/source (Evidence, feuille d un DAG...) ne rayonnait vers
        // personne, et le resultat degenera it en {graines x alpha^iters}
        // uniforme — mesure en prod sur la base agent : TOUTES les
        // requetes exocortex.recall retournaient un plat 0.125 = 0.5^3
        // exactement (alpha=0.5, 3 iterations), rerank structurel inerte.
        let edges = store.edges_from(nid, Direction::Both);
        let mut nw: Vec<(NodeId, f32)> = Vec::with_capacity(edges.len());
        if let Some(feat_v) = features.features.get(&nid) {
            for (neighbor, _) in &edges {
                if let Some(feat_u) = features.features.get(neighbor) {
                    let d = weighted_hilbert_distance(feat_v, feat_u, weights, levels);
                    let d_sq = d * d;
                    let w = (-d_sq / sigma_sq).exp();
                    // w is guaranteed in (0, 1] due to exp of non-positive value
                    if w.is_finite() {
                        nw.push((*neighbor, w));
                    }
                }
            }
        }
        neighbor_weights.insert(nid, nw);
    }

    // ── POSE DU CACHE : verrou d'ecriture BREF, apres le calcul ──
    //
    // La table vient d'etre construite HORS de tout verrou (boucle
    // ci-dessus). On ne prend l'ecriture que pour la deposer.
    //
    // UN SEUL DEPOSANT : les autres viennent de recalculer la meme chose,
    // les faire attendre n'apporterait rien. Ils repartent avec leur propre
    // table — correct, simplement pas mis en cache.
    if CACHE_EN_CONSTRUCTION
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
    {
        {
            let mut w = match cache.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            // Un autre fil a pu poser la meme empreinte pendant qu'on
            // calculait : on ne la duplique pas.
            if !w.iter().any(|c| c.empreinte == empreinte) {
                // Eviction de la plus ancienne. Avec deux profils actifs et
                // quatre places elle ne se declenchera pas en pratique ;
                // elle existe pour qu'un troisieme profil ne fasse pas
                // croitre la table sans fin.
                if w.len() >= CACHE_PLACES {
                    w.remove(0);
                }
                w.push(CacheVoisins {
                    empreinte,
                    voisins: neighbor_weights.clone(),
                });
            }
        }
        CACHE_EN_CONSTRUCTION.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    let _ = sigma_sq;

    let t_diff = web_time::Instant::now();
    let r = diffuser(&node_ids, seeds, &neighbor_weights, config);
    if chrono {
        tracing::info!(
            target: "obrain_tmn::exochrono",
            "CHRONO CHALEUR (cache FROID) ids={}ms sigma={}ms table={}ms diffusion={}ms total={}ms",
            ms_ids, ms_sigma,
            t_ids.elapsed().as_millis() as u64 - ms_ids - ms_sigma
                - t_diff.elapsed().as_millis() as u64,
            t_diff.elapsed().as_millis() as u64,
            t_ids.elapsed().as_millis() as u64
        );
    }
    r
}

/// La diffusion proprement dite — extraite pour etre partagee entre le
/// chemin « cache chaud » et le chemin « on vient de construire ».
///
/// C'est la SEULE partie qui depend des graines, et elle est bon marche :
/// `iterations` passes sur une table deja construite. Tout le reste etait
/// invariant, et c'est tout le reste qu'on recalculait.
fn diffuser(
    node_ids: &[NodeId],
    seeds: &[NodeId],
    neighbor_weights: &HashMap<NodeId, Vec<(NodeId, f32)>>,
    config: &HeatKernelConfig,
) -> Vec<(NodeId, f32)> {
    let mut heat: HashMap<NodeId, f32> = HashMap::with_capacity(node_ids.len());
    for &nid in node_ids {
        heat.insert(nid, 0.0);
    }
    for &seed in seeds {
        if heat.contains_key(&seed) {
            heat.insert(seed, 1.0);
        }
    }
    let alpha = config.alpha.clamp(0.0, 1.0);

    // Iterative diffusion
    for _ in 0..config.iterations {
        let mut new_heat: HashMap<NodeId, f32> = HashMap::with_capacity(node_ids.len());

        for &nid in node_ids {
            let self_heat = *heat.get(&nid).unwrap_or(&0.0);

            let Some(neighbors) = neighbor_weights.get(&nid) else {
                new_heat.insert(nid, alpha * self_heat);
                continue;
            };

            if neighbors.is_empty() {
                new_heat.insert(nid, alpha * self_heat);
                continue;
            }

            let mut weighted_sum = 0.0f32;
            let mut weight_total = 0.0f32;
            for &(neighbor, w) in neighbors {
                let neighbor_heat = *heat.get(&neighbor).unwrap_or(&0.0);
                weighted_sum += w * neighbor_heat;
                weight_total += w;
            }

            let diffused = if weight_total > 0.0 {
                weighted_sum / weight_total
            } else {
                0.0
            };

            let h = alpha * self_heat + (1.0 - alpha) * diffused;
            // Clamp to avoid any floating point drift
            let h = if h.is_finite() { h } else { 0.0 };
            new_heat.insert(nid, h);
        }

        heat = new_heat;
    }

    // Collect non-zero, sort descending
    let mut result: Vec<(NodeId, f32)> = heat
        .into_iter()
        .filter(|(_, h)| *h > 0.0 && h.is_finite())
        .collect();
    result.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    result
}

// ============================================================================
// GraphAlgorithm trait implementation
// ============================================================================

/// Heat kernel diffusion algorithm, registered as a `GraphAlgorithm`.
pub struct HeatKernelAlgorithm;

static HEAT_KERNEL_PARAMS: OnceLock<Vec<ParameterDef>> = OnceLock::new();

fn heat_kernel_params() -> &'static [ParameterDef] {
    HEAT_KERNEL_PARAMS.get_or_init(|| {
        vec![
            ParameterDef {
                name: "seed_nodes".to_string(),
                description: "Comma-separated node IDs to seed with heat".to_string(),
                param_type: ParameterType::String,
                required: true,
                default: None,
            },
            ParameterDef {
                name: "iterations".to_string(),
                description: "Number of diffusion iterations".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("4".to_string()),
            },
            ParameterDef {
                name: "alpha".to_string(),
                description: "Retention factor (0.0-1.0)".to_string(),
                param_type: ParameterType::Float,
                required: false,
                default: Some("0.4".to_string()),
            },
            ParameterDef {
                name: "sigma_sq".to_string(),
                description: "Gaussian kernel bandwidth (omit for auto-calibration)".to_string(),
                param_type: ParameterType::Float,
                required: false,
                default: None,
            },
        ]
    })
}

impl GraphAlgorithm for HeatKernelAlgorithm {
    fn name(&self) -> &str {
        "obrain.heat_kernel_diffusion"
    }

    fn description(&self) -> &str {
        "Heat kernel diffusion on Hilbert feature space"
    }

    fn parameters(&self) -> &[ParameterDef] {
        heat_kernel_params()
    }

    fn execute(&self, store: &dyn GraphStore, params: &Parameters) -> Result<AlgorithmResult> {
        use super::hilbert_features::{HilbertFeaturesConfig, hilbert_features};

        // Parse seed nodes from comma-separated string
        let seed_str = params.get_string("seed_nodes").unwrap_or("");
        let seeds: Vec<NodeId> = seed_str
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    None
                } else {
                    s.parse::<u64>().ok().map(NodeId)
                }
            })
            .collect();

        let iterations = params.get_int("iterations").unwrap_or(4) as usize;
        let alpha = params.get_float("alpha").unwrap_or(0.4) as f32;
        let sigma_sq = params.get_float("sigma_sq").map(|v| v as f32);

        let config = HeatKernelConfig {
            iterations,
            alpha,
            sigma_sq,
        };

        // Compute Hilbert features
        let feat_config = HilbertFeaturesConfig::default();
        let features = hilbert_features(store, &feat_config);
        let weights = FacetteWeights::uniform(8);

        let result = heat_kernel_diffusion(store, &features, &seeds, &weights, &config);

        let mut algo_result = AlgorithmResult::new(vec!["node_id".to_string(), "heat".to_string()]);
        for (node_id, heat) in &result {
            algo_result.add_row(vec![
                Value::Int64(node_id.0 as i64),
                Value::Float64(*heat as f64),
            ]);
        }
        Ok(algo_result)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::many_single_char_names)]
mod tests {
    use super::super::hilbert_features::{HilbertFeaturesConfig, hilbert_features};
    use super::*;
    use obrain_core::graph::GraphStoreMut;
    use obrain_substrate::SubstrateStore;

    /// Build a 5-node line graph: A - B - C - D - E (bidirectional edges).
    fn make_line_graph() -> (SubstrateStore, Vec<NodeId>) {
        let store = SubstrateStore::open_tempfile().unwrap();
        let a = store.create_node(&["Node"]);
        let b = store.create_node(&["Node"]);
        let c = store.create_node(&["Node"]);
        let d = store.create_node(&["Node"]);
        let e = store.create_node(&["Node"]);

        for &(src, dst) in &[(a, b), (b, c), (c, d), (d, e)] {
            store.create_edge(src, dst, "LINK");
            store.create_edge(dst, src, "LINK");
        }
        (store, vec![a, b, c, d, e])
    }

    fn compute_features(store: &SubstrateStore) -> (HilbertFeaturesResult, FacetteWeights) {
        let config = HilbertFeaturesConfig::default();
        let features = hilbert_features(store, &config);
        let n_facettes = features
            .features
            .values()
            .next()
            .map_or(8, |v| v.len() / config.levels);
        let weights = FacetteWeights::uniform(n_facettes);
        (features, weights)
    }

    /// LE CHEMIN CHAUD REND LE MEME RESULTAT QUE LE CHEMIN FROID.
    ///
    /// Premier appel : la table des voisins est construite et mise en cache.
    /// Second appel : elle est servie depuis le cache. Les deux resultats
    /// doivent etre IDENTIQUES, valeur par valeur — pas seulement de meme
    /// taille ou de meme ordre.
    ///
    /// Sans ce test, le cache pourrait servir une table subtilement fausse
    /// sans jamais planter : la diffusion rendrait des nombres plausibles.
    #[test]
    fn le_cache_rend_exactement_le_meme_resultat() {
        let (store, nodes) = make_line_graph();
        let (features, weights) = compute_features(&store);
        let config = HeatKernelConfig {
            iterations: 3,
            alpha: 0.5,
            sigma_sq: None,
        };
        let seeds = vec![nodes[0]];

        let froid = heat_kernel_diffusion(&store, &features, &seeds, &weights, &config);
        let chaud = heat_kernel_diffusion(&store, &features, &seeds, &weights, &config);

        assert_eq!(
            froid.len(),
            chaud.len(),
            "le cache change le nombre de noeuds chauffes"
        );
        for (a, b) in froid.iter().zip(chaud.iter()) {
            assert_eq!(a.0, b.0, "le cache change l'ordre des noeuds");
            assert!(
                (a.1 - b.1).abs() < 1e-6,
                "le cache change la chaleur de {:?} : {} vs {}",
                a.0,
                a.1,
                b.1
            );
        }
    }

    /// UN AUTRE GRAPHE NE DOIT PAS RECEVOIR LA TABLE DU PREMIER.
    ///
    /// L'empreinte est une empreinte de CONTENU, pas une adresse memoire :
    /// deux allocations successives peuvent tomber au meme endroit, et servir
    /// la table d'anciennes features serait une faute silencieuse.
    ///
    /// Un graphe en LIGNE et un graphe en ETOILE n'ont pas la meme topologie :
    /// diffuser depuis leur centre ne peut pas donner la meme distribution.
    #[test]
    fn un_autre_graphe_ninvalide_pas_le_cache_en_silence() {
        let config = HeatKernelConfig {
            iterations: 3,
            alpha: 0.5,
            sigma_sq: None,
        };

        // On chauffe d'abord le cache avec la ligne.
        let (ligne, n_ligne) = make_line_graph();
        let (f_ligne, w_ligne) = compute_features(&ligne);
        let r_ligne = heat_kernel_diffusion(&ligne, &f_ligne, &[n_ligne[0]], &w_ligne, &config);

        // Puis une etoile : centre relie a quatre feuilles.
        let etoile = SubstrateStore::open_tempfile().unwrap();
        let centre = etoile.create_node(&["Node"]);
        let feuilles: Vec<NodeId> = (0..4).map(|_| etoile.create_node(&["Node"])).collect();
        for &f in &feuilles {
            etoile.create_edge(centre, f, "LINK");
            etoile.create_edge(f, centre, "LINK");
        }
        let (f_etoile, w_etoile) = compute_features(&etoile);
        let r_etoile = heat_kernel_diffusion(&etoile, &f_etoile, &[centre], &w_etoile, &config);

        // Dans l'etoile, les quatre feuilles sont a EGALE distance du centre :
        // leur chaleur doit etre identique. Dans la ligne, elle decroit.
        // Si le cache de la ligne avait ete servi a l'etoile, cette egalite
        // serait rompue.
        let chaleurs: Vec<f32> = feuilles
            .iter()
            .filter_map(|f| r_etoile.iter().find(|(id, _)| id == f).map(|(_, h)| *h))
            .collect();
        assert_eq!(
            chaleurs.len(),
            4,
            "les quatre feuilles doivent etre chauffees"
        );
        for c in &chaleurs {
            assert!(
                (c - chaleurs[0]).abs() < 1e-5,
                "feuilles a egale distance mais chaleurs differentes {:?} — \
                 la table de la LIGNE a ete servie a l'ETOILE",
                chaleurs
            );
        }
        assert!(!r_ligne.is_empty(), "la ligne doit aussi avoir chauffe");
    }

    #[test]
    fn test_heat_kernel_basic() {
        let (store, nodes) = make_line_graph();
        let (features, weights) = compute_features(&store);

        let config = HeatKernelConfig {
            iterations: 4,
            alpha: 0.4,
            sigma_sq: None,
        };

        // Seed node A only
        let result = heat_kernel_diffusion(&store, &features, &[nodes[0]], &weights, &config);

        // Result should be non-empty
        assert!(!result.is_empty(), "diffusion should produce results");

        // The seed node should have the highest heat
        let heat_map: HashMap<NodeId, f32> = result.iter().copied().collect();
        let seed_heat = heat_map.get(&nodes[0]).copied().unwrap_or(0.0);
        assert!(seed_heat > 0.0, "seed should have positive heat");

        // Heat should generally decrease with graph distance from seed
        // (nodes[0] is seed, nodes[4] is furthest)
        let heat_a = heat_map.get(&nodes[0]).copied().unwrap_or(0.0);
        let heat_e = heat_map.get(&nodes[4]).copied().unwrap_or(0.0);
        assert!(
            heat_a >= heat_e,
            "heat at seed ({}) should be >= heat at far end ({})",
            heat_a,
            heat_e
        );

        // No NaN or Inf
        for (_, h) in &result {
            assert!(h.is_finite(), "heat values must be finite");
        }
    }

    #[test]
    fn test_heat_kernel_convergence() {
        let (store, nodes) = make_line_graph();
        let (features, weights) = compute_features(&store);

        let config_8 = HeatKernelConfig {
            iterations: 8,
            alpha: 0.4,
            sigma_sq: None,
        };
        let config_10 = HeatKernelConfig {
            iterations: 10,
            alpha: 0.4,
            sigma_sq: None,
        };

        let result_8 = heat_kernel_diffusion(&store, &features, &[nodes[0]], &weights, &config_8);
        let result_10 = heat_kernel_diffusion(&store, &features, &[nodes[0]], &weights, &config_10);

        let heat_8: HashMap<NodeId, f32> = result_8.into_iter().collect();
        let heat_10: HashMap<NodeId, f32> = result_10.into_iter().collect();

        // After many iterations, heat should be relatively stable between iter 8 and 10
        for &nid in &nodes {
            let h8 = heat_8.get(&nid).copied().unwrap_or(0.0);
            let h10 = heat_10.get(&nid).copied().unwrap_or(0.0);
            let diff = (h8 - h10).abs();
            assert!(
                diff < 0.15,
                "heat should stabilize: node {:?} had diff {} (h8={}, h10={})",
                nid,
                diff,
                h8,
                h10
            );
        }
    }

    #[test]
    fn test_heat_kernel_zero_distance() {
        // Create nodes that will have identical features (isolated nodes with same label)
        let store = SubstrateStore::open_tempfile().unwrap();
        let a = store.create_node(&["Same"]);
        let b = store.create_node(&["Same"]);
        // Connect them so they are neighbors
        store.create_edge(a, b, "LINK");
        store.create_edge(b, a, "LINK");

        let (features, weights) = compute_features(&store);

        let config = HeatKernelConfig {
            iterations: 4,
            alpha: 0.4,
            sigma_sq: None,
        };

        let result = heat_kernel_diffusion(&store, &features, &[a], &weights, &config);

        // No NaN or Inf — this is the primary assertion
        for (_, h) in &result {
            assert!(h.is_finite(), "heat must be finite even with zero distance");
            assert!(!h.is_nan(), "heat must not be NaN");
        }
    }

    #[test]
    fn test_heat_kernel_no_seeds() {
        let (store, _nodes) = make_line_graph();
        let (features, weights) = compute_features(&store);

        let config = HeatKernelConfig::default();
        let result = heat_kernel_diffusion(&store, &features, &[], &weights, &config);

        assert!(result.is_empty(), "no seeds should produce empty result");
    }

    #[test]
    fn test_calibrate_sigma() {
        let (store, _nodes) = make_line_graph();
        let (features, weights) = compute_features(&store);

        let levels = features
            .features
            .values()
            .next()
            .map_or(8, |v| v.len() / weights.len());

        let sigma = calibrate_sigma(&store, &features, &weights, levels);
        assert!(
            sigma > 0.0,
            "calibrated sigma² must be positive, got {}",
            sigma
        );
        assert!(sigma.is_finite(), "calibrated sigma² must be finite");
    }
}
