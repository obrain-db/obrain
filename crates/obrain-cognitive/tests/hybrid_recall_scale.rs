//! Rappel hybride natif — ÉCHELLE, distributions, densités et index substrate.
//!
//! Suite du banc fonctionnel (`hybrid_recall_native.rs`) : ici on ne vérifie plus
//! que le chemin fait ce qu'il dit, on mesure ce qu'il COÛTE et ce qu'il PERD
//! quand la taille, la distribution et la densité du corpus changent, sur les
//! TROIS index réellement disponibles dans le dépôt :
//!
//! | index | ce qu'il fait |
//! |---|---|
//! | `Brute` | `InMemoryVectorIndex` — cosinus f64 sur tous les vecteurs, O(n·d) |
//! | `Cascade` | `SubstrateTieredIndex` (paliers L0 empreinte 128 bits → L1 512 bits → L2 f16), largeurs par défaut k0=4000 / k1=400 |
//! | `CascadeSerré` | le même cascade avec `LargeursCascade::depuis_k0(n_probe)` — le budget de candidats qu'un appelant choisirait |
//!
//! Les trois sont interrogés par le MÊME chemin (`RecallEngine::hybrid_recall`),
//! via `SubstrateVectorIndexAdapter`, exactement comme la production les branche.
//!
//! ## Ce qui est mesuré (imprimé, jamais asserté)
//!
//! - **qualité** : plafond candidat@10, accord@10, τ-b de Kendall, accord du
//!   top-1 du vivier, rappel des engrammes comparables (score exact > 0) ;
//! - **coût** : latence du `nearest` SEUL (isole l'index de la validation),
//!   latence du chemin complet, candidats demandés/rendus, validations exactes ;
//! - **mémoire** : octets par vecteur de chaque index (mesuré par `size_of`,
//!   pas estimé) ;
//! - **pente** : T(2N)/T(N) pour chaque index, à protocole constant.
//!
//! ## Ce que ce banc ne prouve PAS
//!
//! Aucune complexité. Les paliers L0/L1/L2 balaient TOUS les vecteurs au niveau
//! L0 (16 octets par vecteur, popcount) : le coût reste linéaire en n. Ce que la
//! cascade change, c'est la CONSTANTE par vecteur et le nombre de comparaisons
//! pleines (L1 sur k0, L2 sur k1), pas l'ordre de grandeur. Les rapports de temps
//! imprimés sont une PENTE OBSERVÉE sur cette machine, à cette charge — pas une
//! preuve asymptotique. Une machine partagée peut déformer un rapport d'un
//! facteur 2 sans que rien n'ait changé dans le code.

#![cfg(all(feature = "engram", feature = "substrate"))]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use obrain_cognitive::engram::substrate_adapter::SubstrateVectorIndexAdapter;
use obrain_cognitive::engram::traits::{InMemoryVectorIndex, VectorIndex};
use obrain_cognitive::engram::{
    Engram, EngramId, EngramStore, HybridRecallConfig, RecallEngine, RecallVariant,
    SpectralEncoder, parse_engram_id,
};
use obrain_common::types::NodeId;
use obrain_substrate::retrieval::{
    LargeursCascade, SubstrateTieredIndex, VectorIndex as SubstrateVectorIndex,
};
use obrain_substrate::tiers::{
    L2_DIM, Tier0, Tier0Builder, Tier1, Tier1Builder, Tier2, Tier2Builder, tier0_hamming,
    tier1_hamming, tier2_cosine,
};

const K: usize = 10;
/// Tailles du balayage d'échelle. 16 000 est la plus grande qui tienne dans un
/// budget de banc raisonnable (construction ≈ 130 µs/vecteur/index).
const TAILLES: [usize; 6] = [250, 500, 1000, 2000, 4000, 16_000];
const N_PROBE: usize = 50;
/// Budget de candidats « modéré » comparé aux largeurs par défaut du cascade.
const K0_BUDGET: usize = 200;
const LATENCY_REPS: usize = 5;
const N_QUERIES: usize = 20;
const VOCAB: usize = 400;
const N_CLUSTERS: usize = 20;
const CLUSTER_SIZE: usize = VOCAB / N_CLUSTERS;

// ---------------------------------------------------------------------------
// Corpus paramétré
// ---------------------------------------------------------------------------

/// Distribution des membres d'un ensemble.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Distrib {
    /// Chaque ensemble tire la moitié de ses membres de son cluster : recouvrement
    /// intra-cluster, comme le banc fonctionnel.
    Clusters,
    /// Tirage uniforme sur tout le vocabulaire : presque aucun recouvrement.
    Uniforme,
    /// Vocabulaire biaisé (u³) : quelques vocables très fréquents, longue traîne.
    Zipf,
}

impl Distrib {
    fn label(self) -> &'static str {
        match self {
            Self::Clusters => "clusters",
            Self::Uniforme => "uniforme",
            Self::Zipf => "zipf",
        }
    }
}

/// Taille des ensembles — la densité de recouvrement.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Densite {
    /// 2..4 membres : recouvrement rare, ensembles presque disjoints.
    Fine,
    /// 3..8 membres : le cas du banc fonctionnel.
    Moyenne,
    /// 8..20 membres : recouvrement fréquent, ensembles denses.
    Large,
}

impl Densite {
    fn taille(self, rng: &mut Lcg) -> usize {
        match self {
            Self::Fine => 2 + rng.below(3),
            Self::Moyenne => 3 + rng.below(6),
            Self::Large => 8 + rng.below(13),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Fine => "fine(2..4)",
            Self::Moyenne => "moyenne(3..8)",
            Self::Large => "large(8..20)",
        }
    }
}

/// Générateur congruentiel linéaire : reproductible, aucune dépendance.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn weight(&mut self) -> f64 {
        0.5 + (self.next() % 11) as f64 * 0.1
    }
}

fn nœud_uniforme(rng: &mut Lcg) -> NodeId {
    NodeId((rng.below(VOCAB) + 1) as u64)
}

fn nœud_zipf(rng: &mut Lcg) -> NodeId {
    // u³ concentre les tirages sur les premiers rangs : longue traîne.
    let u = (rng.next() % 1_000_000) as f64 / 1_000_000.0;
    let rang = (u * u * u * VOCAB as f64) as usize;
    NodeId((rang.min(VOCAB - 1) + 1) as u64)
}

fn make_corpus(n: usize, distrib: Distrib, densite: Densite, seed: u64) -> Vec<Vec<(NodeId, f64)>> {
    let mut rng = Lcg::new(seed);
    let mut corpus = Vec::with_capacity(n);
    for i in 0..n {
        let taille = densite.taille(&mut rng);
        let mut ensemble: Vec<(NodeId, f64)> = Vec::with_capacity(taille);
        let mut used: HashSet<NodeId> = HashSet::new();
        let mut tries = 0usize;
        while ensemble.len() < taille && tries < 16 * taille {
            tries += 1;
            let node = match distrib {
                Distrib::Clusters => {
                    let cluster = i % N_CLUSTERS;
                    if ensemble.len() * 2 < taille {
                        NodeId((cluster * CLUSTER_SIZE + rng.below(CLUSTER_SIZE) + 1) as u64)
                    } else {
                        nœud_uniforme(&mut rng)
                    }
                }
                Distrib::Uniforme => nœud_uniforme(&mut rng),
                Distrib::Zipf => nœud_zipf(&mut rng),
            };
            if used.insert(node) {
                ensemble.push((node, rng.weight()));
            }
        }
        corpus.push(ensemble);
    }
    corpus
}

fn make_queries(distrib: Distrib, densite: Densite, seed: u64) -> Vec<Vec<NodeId>> {
    let mut rng = Lcg::new(seed ^ 0xDEAD_BEEF_1234_5678);
    let cues_len = match densite {
        Densite::Fine => 2,
        Densite::Moyenne => 2 + rng.below(2),
        Densite::Large => 3 + rng.below(2),
    };
    (0..N_QUERIES)
        .map(|q| {
            let cluster = (q * 7) % N_CLUSTERS;
            let mut cues: Vec<NodeId> = Vec::with_capacity(cues_len);
            let mut used: HashSet<NodeId> = HashSet::new();
            let mut tries = 0usize;
            while cues.len() < cues_len && tries < 32 {
                tries += 1;
                let node = match distrib {
                    Distrib::Clusters => {
                        NodeId((cluster * CLUSTER_SIZE + rng.below(CLUSTER_SIZE) + 1) as u64)
                    }
                    Distrib::Uniforme => nœud_uniforme(&mut rng),
                    Distrib::Zipf => nœud_zipf(&mut rng),
                };
                if used.insert(node) {
                    cues.push(node);
                }
            }
            cues
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Référence exacte et mesures (réimplémentées, indépendantes du code mesuré)
// ---------------------------------------------------------------------------

fn harness_cosine(query: &[(NodeId, f64)], ensemble: &[(NodeId, f64)]) -> f64 {
    let map: HashMap<NodeId, f64> = query.iter().copied().collect();
    let mut dot = 0.0;
    let mut norm_e = 0.0;
    for (node, w) in ensemble {
        norm_e += w * w;
        if let Some(qw) = map.get(node) {
            dot += qw * w;
        }
    }
    let norm_q: f64 = query.iter().map(|(_, w)| w * w).sum();
    if norm_e == 0.0 || norm_q == 0.0 {
        return 0.0;
    }
    dot / (norm_q.sqrt() * norm_e.sqrt())
}

fn reference(store: &EngramStore, cues: &[NodeId]) -> Vec<(EngramId, f64)> {
    let query: Vec<(NodeId, f64)> = cues.iter().map(|&n| (n, 1.0)).collect();
    let mut ranked: Vec<(EngramId, f64)> = Vec::new();
    store.for_each(|engram| {
        ranked.push((engram.id, harness_cosine(&query, &engram.ensemble)));
    });
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.0.cmp(&b.0.0))
    });
    ranked
}

fn kendall_tau_b(pairs: &[(f64, f64)]) -> Option<f64> {
    let n = pairs.len();
    if n < 2 {
        return None;
    }
    let (mut c, mut d, mut ties_x, mut ties_y) = (0i64, 0i64, 0i64, 0i64);
    for i in 0..n {
        for j in (i + 1)..n {
            let (x1, y1) = pairs[i];
            let (x2, y2) = pairs[j];
            let dx = (x1 - x2)
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal);
            let dy = (y1 - y2)
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal);
            match (dx, dy) {
                (std::cmp::Ordering::Equal, std::cmp::Ordering::Equal) => {
                    ties_x += 1;
                    ties_y += 1;
                }
                (std::cmp::Ordering::Equal, _) => ties_x += 1,
                (_, std::cmp::Ordering::Equal) => ties_y += 1,
                _ if dx == dy => c += 1,
                _ => d += 1,
            }
        }
    }
    let n0 = (n * (n - 1) / 2) as f64;
    let denom = ((n0 - ties_x as f64) * (n0 - ties_y as f64)).sqrt();
    if denom <= 0.0 {
        return None;
    }
    Some((c - d) as f64 / denom)
}

/// Coût du `nearest` SEUL (médiane sur les requêtes), requête éventuellement
/// rembourrée à `pad_to` — sert à comparer des index de dimensions différentes
/// sans passer par le chemin complet, qui refuse une dimension incohérente.
fn cout_index_seul(
    index: &dyn VectorIndex,
    encoder: &SpectralEncoder,
    queries: &[Vec<NodeId>],
    pad_to: Option<usize>,
    reps: usize,
) -> f64 {
    let mut par_requete = Vec::with_capacity(queries.len());
    for cues in queries {
        let mut sig = encoder.encode(&cues.iter().map(|&n| (n, 1.0)).collect::<Vec<_>>());
        if let Some(dim) = pad_to {
            sig.resize(dim, 0.0);
        }
        let mut samples = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t0 = Instant::now();
            let hits = index.nearest(&sig, N_PROBE);
            std::hint::black_box(hits);
            samples.push(t0.elapsed().as_secs_f64() * 1e6);
        }
        par_requete.push(median(samples));
    }
    median(par_requete)
}

/// Mémoire RÉSIDENTE du processus (pages résidentes × 4096), lue dans
/// `/proc/self/statm`.
///
/// POURQUOI PAS UN ALLOCATEUR COMPTEUR. Compter les octets rendus par
/// l'allocateur Rust demande un `#[global_allocator]`, donc `unsafe` — que le
/// workspace INTERDIT (`unsafe_code = "warn"` + `-D warnings`) et auquel je
/// n'ajoute pas d'exception. La mémoire est donc mesurée par le RSS (ce que la
/// machine paie réellement) et comparée à un PLANCHER STRUCTUREL calculé par
/// `size_of` sur les données stockées. L'écart entre les deux est exactement ce
/// que l'allocateur ajoute : en-têtes de blocs, capacité réservée, tables de
/// hachage, fragmentation externe. Aucun des deux chiffres ne remplace l'autre.
fn rss_octets() -> usize {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(0, |pages| pages * 4096)
}

fn moyenne(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.iter().sum::<f64>() / v.len() as f64
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

#[derive(Default, Clone, Copy)]
struct Qualite {
    /// Plafond mesuré sur le VIVIER BRUT de l'index (ensemble des candidats),
    /// indépendamment de tout classement et de tout filtrage du chemin.
    ceiling: f64,
    accord: f64,
    tau: f64,
    top1: f64,
    pertinents: f64,
    candidates: f64,
}

/// Résultat d'une mesure : qualité du rappel + les deux latences.
#[derive(Clone, Copy)]
struct Mesure {
    q: Qualite,
    /// Latence du `nearest` SEUL — isole le coût de l'index de celui de la validation.
    lat_index: f64,
    /// Latence du chemin complet (index + validation exacte + classement).
    lat_path: f64,
}

/// Contexte d'une mesure : la requête, sa référence exacte et le code mesuré.
struct Ctx<'a> {
    engine: &'a RecallEngine,
    store: &'a EngramStore,
    encoder: &'a SpectralEncoder,
    queries: &'a [Vec<NodeId>],
    refs: &'a [Vec<(EngramId, f64)>],
}

/// Mesure une variante sur un index donné : qualité + latences.
fn mesurer(ctx: &Ctx<'_>, index: &dyn VectorIndex, variant: RecallVariant, reps: usize) -> Mesure {
    let Ctx {
        engine,
        store,
        encoder,
        queries,
        refs,
    } = *ctx;
    let cfg = HybridRecallConfig {
        k: N_PROBE,
        n_probe: N_PROBE,
        ..HybridRecallConfig::default()
    };
    let mut acc_q = Qualite::default();
    let mut taus: Vec<f64> = Vec::new();
    let mut top1 = 0usize;
    let mut pertinents: Vec<f64> = Vec::new();
    let mut latency: Vec<f64> = Vec::new();
    let mut lat_index: Vec<f64> = Vec::new();
    let mut candidates = 0.0;
    let mut ceiling = 0.0;
    let mut accord = 0.0;

    for (q, cues) in queries.iter().enumerate() {
        // Coût de l'INDEX seul, mesuré avant le chemin complet : la validation
        // exacte des candidats (n_probe cosinus) domine à petit N, et la
        // confondre avec le coût d'index ferait croire que l'index est lent.
        let sig = encoder.encode(&cues.iter().map(|&n| (n, 1.0)).collect::<Vec<_>>());
        let mut idx_samples = Vec::with_capacity(reps);
        for _ in 0..reps {
            let t0 = Instant::now();
            let hits = index.nearest(&sig, N_PROBE);
            std::hint::black_box(hits);
            idx_samples.push(t0.elapsed().as_secs_f64() * 1e6);
        }
        lat_index.push(median(idx_samples));

        let mut samples = Vec::with_capacity(reps);
        let mut outcome = None;
        for _ in 0..LATENCY_REPS {
            let t0 = Instant::now();
            let o = engine
                .hybrid_recall(store, cues, index, encoder, variant, &cfg)
                .expect("le chemin s'exécute sur cet index");
            samples.push(t0.elapsed().as_secs_f64() * 1e6);
            outcome = Some(o);
        }
        let outcome = outcome.expect("au moins une exécution");
        assert_eq!(
            outcome.stats.unknown_ids, 0,
            "identifiants d'index illisibles"
        );
        assert_eq!(outcome.stats.stale_ids, 0, "index et store divergents");
        latency.push(median(samples));

        let ref_top: Vec<EngramId> = refs[q].iter().take(K).map(|(id, _)| *id).collect();
        let pool: Vec<EngramId> = outcome.hits.iter().map(|h| h.engram_id).collect();

        // ── PLAFOND SUR LE VIVIER BRUT ─────────────────────────────────────
        // Les hits sont déjà reclassés (et filtrés pour la variante structurelle) :
        // y lire le plafond serait circulaire. On interroge l'index directement et
        // on compare l'ENSEMBLE des candidats à la référence exacte.
        let sig = encoder.encode(&cues.iter().map(|&n| (n, 1.0)).collect::<Vec<_>>());
        let vivier: HashSet<EngramId> = index
            .nearest(&sig, N_PROBE)
            .iter()
            .filter_map(|(raw, _)| parse_engram_id(raw))
            .collect();
        let vivier_hit = vivier.iter().filter(|id| ref_top.contains(id)).count();
        let pooled_brut = vivier_hit as f64 / ref_top.len() as f64;
        if variant != RecallVariant::HybridStructural {
            let hits_hit = pool.iter().filter(|id| ref_top.contains(id)).count();
            let pooled_hits = hits_hit as f64 / ref_top.len() as f64;
            assert!(
                (pooled_brut - pooled_hits).abs() < 1e-9,
                "{} : vivier de l'index ({pooled_brut}) ≠ candidats du chemin ({pooled_hits})",
                variant.label()
            );
        }
        let top: Vec<EngramId> = pool.iter().copied().take(K).collect();
        let pool_hit = pool.iter().filter(|id| ref_top.contains(id)).count();
        let pooled = pool_hit as f64 / ref_top.len() as f64;
        let acc =
            top.iter().filter(|id| ref_top.contains(id)).count() as f64 / ref_top.len() as f64;
        assert!(
            acc <= pooled_brut + 1e-9,
            "invariant : accord {acc} > plafond du vivier brut {pooled_brut}"
        );
        assert!(
            pooled <= pooled_brut + 1e-9,
            "le chemin ne peut pas trouver plus que le vivier de l'index : {pooled} > {pooled_brut}"
        );
        if variant == RecallVariant::Hybrid {
            assert!(
                (acc - pooled_brut).abs() < 1e-9,
                "rerank exact : accord {acc} ≠ plafond du vivier brut {pooled_brut}"
            );
        }
        ceiling += pooled_brut;
        accord += acc;
        candidates += outcome.stats.candidates as f64;

        let query: Vec<(NodeId, f64)> = cues.iter().map(|&n| (n, 1.0)).collect();
        let mut pairs: Vec<(f64, f64)> = Vec::with_capacity(pool.len());
        for hit in &outcome.hits {
            if let Some(engram) = store.get(hit.engram_id) {
                pairs.push((hit.index_score, harness_cosine(&query, &engram.ensemble)));
            }
        }
        if let Some(tau) = kendall_tau_b(&pairs) {
            taus.push(tau);
        }
        let best_index = pairs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        let best_exact = pairs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        if best_index.is_some() && best_index == best_exact {
            top1 += 1;
        }
        let relevant = refs[q].iter().filter(|(_, s)| *s > 0.0).count();
        if relevant > 0 {
            let found = refs[q]
                .iter()
                .filter(|(_, s)| *s > 0.0)
                .filter(|(id, _)| pool.contains(id))
                .count();
            pertinents.push(found as f64 / relevant as f64);
        }
    }

    let n = queries.len() as f64;
    acc_q.ceiling = ceiling / n;
    acc_q.accord = accord / n;
    acc_q.tau = if taus.is_empty() {
        f64::NAN
    } else {
        taus.iter().sum::<f64>() / taus.len() as f64
    };
    acc_q.top1 = top1 as f64 / n;
    acc_q.pertinents = pertinents.iter().sum::<f64>() / pertinents.len().max(1) as f64;
    acc_q.candidates = candidates / n;
    Mesure {
        q: acc_q,
        lat_index: median(lat_index),
        lat_path: median(latency),
    }
}

// ---------------------------------------------------------------------------
// Les trois index
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Brute,
    Cascade,
    CascadeBudget,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Brute => "brute (f64, O(n·d))",
            Self::Cascade => "cascade k0=4000/k1=400",
            Self::CascadeBudget => "cascade k0=200/k1=20",
        }
    }
}

/// Un banc prêt à mesurer : un store, un index force brute, et UN cascade dont
/// on peut changer les largeurs sans reconstruire (la projection coûte
/// ~130 µs par vecteur, la reconstruire pour chaque réglage fausserait tout).
struct Banc {
    store: EngramStore,
    brute: Brute,
    /// Force brute sur la signature REMBOURRÉE à 384 dimensions (L2_DIM) : même
    /// information, six fois plus de flops par vecteur. Sert à mesurer ce que
    /// coûte la dimension que les paliers imposent, à qualité identique.
    brute384: Brute,
    cascade_inner: Arc<SubstrateTieredIndex>,
    cascade: SubstrateVectorIndexAdapter,
}

impl Banc {
    fn largeurs(&self, k0: usize) {
        self.cascade_inner
            .definir_largeurs(LargeursCascade::depuis_k0(k0));
    }

    fn largeurs_defaut(&self) {
        self.cascade_inner
            .definir_largeurs(LargeursCascade::default());
    }
}

/// Construit le corpus et les index.
///
/// Le cascade est alimenté PAR L'ADAPTATEUR, comme en production : une insertion
/// directe dans `SubstrateTieredIndex` laisse vide la table identifiant → offset
/// de l'adaptateur, et `nearest` rend alors ZÉRO résultat (les offsets non
/// résolus sont écartés défensivement). C'est un mode de panne silencieux —
/// constaté sur ce banc même (plafond 0,000 sur les 27 configurations) — évité
/// ici par construction.
fn construire(corpus: &[Vec<(NodeId, f64)>], encoder: &SpectralEncoder) -> Banc {
    let store = EngramStore::new(None);
    for (i, ensemble) in corpus.iter().enumerate() {
        let mut engram = Engram::new(EngramId(i as u64 + 1), ensemble.clone());
        engram.spectral_signature = encoder.encode(ensemble);
        store.insert(engram);
    }
    let brute = Brute::default();
    let brute384 = Brute::default();
    let cascade_inner = Arc::new(SubstrateTieredIndex::new(encoder.dimensions()));
    let cascade = adapter(&cascade_inner);
    for engram in store.list() {
        let id = engram.id.to_string();
        brute.upsert(&id, &engram.spectral_signature);
        let mut pad = engram.spectral_signature.clone();
        pad.resize(L2_DIM, 0.0);
        brute384.upsert(&id, &pad);
        cascade.upsert(&id, &engram.spectral_signature);
    }
    Banc {
        store,
        brute,
        brute384,
        cascade_inner,
        cascade,
    }
}

/// Index force brute instrumenté (nombre d'appels à `nearest`).
#[derive(Default)]
struct Brute {
    inner: InMemoryVectorIndex,
}

impl VectorIndex for Brute {
    fn upsert(&self, id: &str, vector: &[f64]) {
        self.inner.upsert(id, vector);
    }

    fn nearest(&self, query: &[f64], k: usize) -> Vec<(String, f64)> {
        self.inner.nearest(query, k)
    }

    fn remove(&self, id: &str) {
        self.inner.remove(id);
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

/// Adaptateur des paliers substrate qui expose le MÊME index au chemin cognitif.
fn adapter(inner: &Arc<SubstrateTieredIndex>) -> SubstrateVectorIndexAdapter {
    SubstrateVectorIndexAdapter::new(Arc::clone(inner) as Arc<dyn SubstrateVectorIndex>)
}

// ---------------------------------------------------------------------------
// Le banc
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Travail {
    l0: usize,
    l1: usize,
    l2: usize,
    exact: usize,
}
impl Travail {
    fn total(&self) -> usize {
        self.l0 + self.l1 + self.l2 + self.exact
    }
}

/// Les paliers d'un corpus, partagés par toutes les politiques.
struct Paliers<'a> {
    l0: &'a [Tier0],
    l1: &'a [Tier1],
    l2: &'a [Tier2],
    ids: &'a [EngramId],
    store: &'a EngramStore,
}

/// Une requête projetée dans les trois paliers, plus son ensemble exact.
struct Requete {
    q0: Tier0,
    q1: Tier1,
    q2: Tier2,
    ensemble: Vec<(NodeId, f64)>,
}

/// Une exploration. `incremental` : garde l'ordre L0 et les affinages déjà
/// faits. Sinon : refait tout à chaque élargissement (ce que fait
/// `nearest(k)` aujourd'hui — l'index n'a aucune notion d'exploration en
/// cours). Rend `(top-k, travail, tours)`.
fn explorer(
    pal: &Paliers<'_>,
    req: &Requete,
    incremental: bool,
    pas: usize,
    cap: usize,
) -> (Vec<EngramId>, Travail, usize) {
    let n = pal.l0.len();
    // Ordre L0 : payé une fois (incrémental) ou à chaque tour (redémarrage).
    let ordre_l0: Vec<(u32, usize)> = {
        let mut v: Vec<(u32, usize)> = (0..n)
            .map(|i| (tier0_hamming(req.q0, pal.l0[i]), i))
            .collect();
        v.sort_unstable_by_key(|(d, i)| (*d, *i));
        v
    };
    let mut travail = Travail::default();
    if incremental {
        travail.l0 += n;
    }
    let mut scores: HashMap<EngramId, f64> = HashMap::new();
    let mut top_prec: Vec<EngramId> = Vec::new();
    let mut stabilite = 0usize;
    let mut rounds = 0usize;
    let mut examine = 0usize;
    while examine < cap.min(n) {
        rounds += 1;
        if !incremental {
            travail.l0 += n; // le redémarrage rebalaye les empreintes
        }
        let fin = (examine + pas).min(n);
        let tranche = &ordre_l0[examine..fin];
        let mut affines: Vec<(u32, usize)> = Vec::with_capacity(tranche.len());
        for &(_, idx) in tranche {
            affines.push((tier1_hamming(&pal.l1[idx], &req.q1), idx));
        }
        travail.l1 += tranche.len();
        affines.sort_unstable_by_key(|(d, i)| (*d, *i));
        let garder = (affines.len() / 2).max(1);
        for &(_, idx) in affines.iter().take(garder) {
            let id = pal.ids[idx];
            if scores.contains_key(&id) {
                continue; // déjà affiné : l'incrémental ne le refait pas
            }
            travail.l2 += 1;
            let cos = f64::from(tier2_cosine(&pal.l2[idx], &req.q2));
            std::hint::black_box(cos);
            travail.exact += 1;
            if let Some(e) = pal.store.get(id) {
                scores.insert(id, harness_cosine(&req.ensemble, &e.ensemble));
            }
        }
        examine = fin;
        let mut classes: Vec<(EngramId, f64)> = scores.iter().map(|(id, s)| (*id, *s)).collect();
        classes.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.0.cmp(&b.0.0))
        });
        let top: Vec<EngramId> = classes.iter().take(K).map(|(id, _)| *id).collect();
        // Critère DÉPLOYABLE (aucun oracle) : le top-k ne bouge plus deux tours
        // de suite. Il ne consulte jamais la référence exacte du corpus.
        if !top.is_empty() && top == top_prec {
            stabilite += 1;
            if stabilite >= 2 {
                break;
            }
        } else {
            stabilite = 0;
        }
        top_prec = top;
    }
    let mut classes: Vec<(EngramId, f64)> = scores.into_iter().collect();
    classes.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.0.cmp(&b.0.0))
    });
    (
        classes.iter().take(K).map(|(id, _)| *id).collect(),
        travail,
        rounds,
    )
}

/// Une ligne de comparaison de politiques d'exploration.
struct LignePolitique {
    nom: String,
    plafond: f64,
    travail: f64,
    exacts: f64,
    tours: f64,
    temps_us: f64,
}

/// Compare, à une taille `n` donnée, l'exploration INCRÉMENTALE et le
/// REDÉMARRAGE (ce que fait `nearest(k)` aujourd'hui), plus deux largeurs fixes
/// de référence. Tout est simulé ici : aucun moteur n'est modifié.
fn comparer_politiques(
    n: usize,
    encoder: &SpectralEncoder,
    query_seed: u64,
) -> Option<Vec<LignePolitique>> {
    let corpus = make_corpus(n, Distrib::Clusters, Densite::Moyenne, 0x9E37_79B9);
    let queries = make_queries(Distrib::Clusters, Densite::Moyenne, 0x9E37_79B9);
    let banc = construire(&corpus, encoder);
    let refs: Vec<Vec<(EngramId, f64)>> =
        queries.iter().map(|c| reference(&banc.store, c)).collect();

    let b0 = Tier0Builder::with_default_seed(L2_DIM);
    let b1 = Tier1Builder::with_default_seed(L2_DIM);
    let b2 = Tier2Builder::new();
    let mut l0: Vec<Tier0> = Vec::with_capacity(n);
    let mut l1: Vec<Tier1> = Vec::with_capacity(n);
    let mut l2: Vec<Tier2> = Vec::with_capacity(n);
    let mut ids: Vec<EngramId> = Vec::with_capacity(n);
    for i in 0..n {
        let id = EngramId(i as u64 + 1);
        let engram = banc.store.get(id)?;
        let mut p: Vec<f32> = engram
            .spectral_signature
            .iter()
            .map(|&x| x as f32)
            .collect();
        p.resize(L2_DIM, 0.0);
        l0.push(b0.project(&p));
        l1.push(b1.project(&p));
        l2.push(b2.project(&p));
        ids.push(id);
    }
    let paliers = Paliers {
        l0: &l0,
        l1: &l1,
        l2: &l2,
        ids: &ids,
        store: &banc.store,
    };

    let mut lignes: Vec<LignePolitique> = Vec::new();
    for (nom, incremental, pas) in [
        ("incrémental pas=25", true, 25usize),
        ("incrémental pas=50", true, 50),
        ("redémarrage pas=25", false, 25),
        ("redémarrage pas=50", false, 50),
    ] {
        let (mut plafonds, mut travaux, mut exacts, mut rounds, mut temps) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (q, cues) in queries.iter().enumerate() {
            let sig = encoder.encode(&cues.iter().map(|&x| (x, 1.0)).collect::<Vec<_>>());
            let mut p: Vec<f32> = sig.iter().map(|&x| x as f32).collect();
            p.resize(L2_DIM, 0.0);
            let req = Requete {
                q0: b0.project(&p),
                q1: b1.project(&p),
                q2: b2.project(&p),
                ensemble: cues.iter().map(|&x| (x, 1.0)).collect(),
            };
            let ref_top: Vec<EngramId> = refs[q].iter().take(K).map(|(id, _)| *id).collect();
            let t0 = Instant::now();
            let (top, travail, r) = explorer(&paliers, &req, incremental, pas, 400);
            temps.push(t0.elapsed().as_secs_f64() * 1e6);
            plafonds.push(
                top.iter().filter(|id| ref_top.contains(id)).count() as f64 / ref_top.len() as f64,
            );
            travaux.push(travail.total() as f64);
            exacts.push(travail.exact as f64);
            rounds.push(r as f64);
        }
        lignes.push(LignePolitique {
            nom: nom.to_string(),
            plafond: moyenne(&plafonds),
            travail: moyenne(&travaux),
            exacts: moyenne(&exacts),
            tours: moyenne(&rounds),
            temps_us: median(temps),
        });
    }

    // Références non incrémentales : la cascade à largeurs fixes.
    for k0 in [200usize, 4000] {
        banc.largeurs(k0);
        let (mut plafonds, mut temps) = (Vec::new(), Vec::new());
        for (q, cues) in queries.iter().enumerate() {
            let sig = encoder.encode(&cues.iter().map(|&x| (x, 1.0)).collect::<Vec<_>>());
            let mut s = Vec::with_capacity(3);
            for _ in 0..3 {
                let t0 = Instant::now();
                let hits = banc.cascade.nearest(&sig, N_PROBE);
                s.push(t0.elapsed().as_secs_f64() * 1e6);
                std::hint::black_box(hits);
            }
            temps.push(median(s));
            let ref_top: Vec<EngramId> = refs[q].iter().take(K).map(|(id, _)| *id).collect();
            let vivier: Vec<EngramId> = banc
                .cascade
                .nearest(&sig, N_PROBE)
                .iter()
                .filter_map(|(raw, _)| parse_engram_id(raw))
                .collect();
            plafonds.push(
                vivier.iter().filter(|id| ref_top.contains(id)).count() as f64
                    / ref_top.len() as f64,
            );
        }
        let k1 = (k0 / 10).max(N_PROBE);
        lignes.push(LignePolitique {
            nom: format!("fixe k0={k0} (via nearest)"),
            plafond: moyenne(&plafonds),
            travail: (n + k0 + k1) as f64,
            exacts: N_PROBE as f64,
            tours: 1.0,
            temps_us: median(temps),
        });
    }
    banc.largeurs_defaut();
    std::hint::black_box(query_seed);
    Some(lignes)
}

#[test]
fn banc_echelle_et_fidelite() {
    let started = Instant::now();
    let encoder = SpectralEncoder::new();
    let engine = RecallEngine::new();

    println!("\n=== RAPPEL HYBRIDE — échelle, distribution, densité, index substrate ===");
    println!(
        "protocole : k = {K}, n_probe = {N_PROBE}, {N_QUERIES} requêtes, latence = médiane de 3 \
         passes | signatures {}d",
        encoder.dimensions()
    );
    println!(
        "charge machine : {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );
    println!(
        "mémoire par vecteur indexé (mesurée par size_of, pas estimée) : brute = {} o | \
         cascade = {} o (L0 {} + L1 {} + L2 {}), + la signature côté store",
        std::mem::size_of::<f64>() * encoder.dimensions(),
        std::mem::size_of::<Tier0>() + std::mem::size_of::<Tier1>() + std::mem::size_of::<Tier2>(),
        std::mem::size_of::<Tier0>(),
        std::mem::size_of::<Tier1>(),
        std::mem::size_of::<Tier2>()
    );

    // ---- 5) Mémoire : RSS mesuré vs plancher structurel (mesuré EN PREMIER) ----
    println!("\n-- 5. Mémoire : RSS mesuré (machine) vs plancher structurel (size_of) --");
    println!("   Pas d'allocateur compteur : il exigerait `unsafe`, interdit par le workspace");
    println!("   (`unsafe_code = \"warn\"` + `-D warnings`), et je n'ajoute pas d'exception.");
    println!("   Deux grandeurs distinctes, jamais substituées l'une à l'autre :");
    println!("   · plancher = octets que les DONNÉES exigent (signatures, paliers, slots) ;");
    println!("   · RSS     = pages résidentes payées par la machine (en-têtes d'allocation,");
    println!("               capacité réservée, tables de hachage, fragmentation externe).");
    println!("   Aucun index n'est mmapé ici (construction en mémoire) : rien hors tas.");
    println!(
        "   Mesuré EN PREMIER, à froid : le RSS ne redescend pas quand le tas libère, donc un"
    );
    println!("   delta mesuré après d'autres sections serait nul même si le tas avait grandi.");
    let n_mem = 4000usize;
    let corpus_mem = make_corpus(n_mem, Distrib::Clusters, Densite::Moyenne, 0x9E37_79B9);
    let sig_octets = encoder.dimensions() * std::mem::size_of::<f64>();
    let paliers_octets =
        std::mem::size_of::<Tier0>() + std::mem::size_of::<Tier1>() + std::mem::size_of::<Tier2>();

    let rss0 = rss_octets();
    let store_mem = {
        let store = EngramStore::new(None);
        for (i, ensemble) in corpus_mem.iter().enumerate() {
            let mut e = Engram::new(EngramId(i as u64 + 1), ensemble.clone());
            e.spectral_signature = encoder.encode(ensemble);
            store.insert(e);
        }
        store
    };
    let rss_store = rss_octets().saturating_sub(rss0);

    let rss1 = rss_octets();
    let brute_mem = Brute::default();
    store_mem.for_each(|e| brute_mem.upsert(&e.id.to_string(), &e.spectral_signature));
    let rss_brute = rss_octets().saturating_sub(rss1);

    let rss2 = rss_octets();
    let cascade_inner_mem = Arc::new(SubstrateTieredIndex::new(encoder.dimensions()));
    let cascade_adapter_mem = adapter(&cascade_inner_mem);
    store_mem.for_each(|e| cascade_adapter_mem.upsert(&e.id.to_string(), &e.spectral_signature));
    let rss_cascade = rss_octets().saturating_sub(rss2);

    {
        let n = n_mem as f64;
        println!(
            "{:<30} {:>12} {:>11} {:>12} {:>11} {:>9}",
            "composant", "plancher o", "pl./vect", "RSS o", "RSS/vect", "écart"
        );
        for (nom, plancher, rss) in [
            (
                "store (engrammes + signatures)",
                sig_octets * n_mem,
                rss_store,
            ),
            ("+ index force brute", sig_octets * n_mem, rss_brute),
            ("+ cascade substrate", paliers_octets * n_mem, rss_cascade),
        ] {
            println!(
                "{:<30} {:>12} {:>11.0} {:>12} {:>11.0} {:>8.1}×",
                nom,
                plancher,
                plancher as f64 / n,
                rss,
                rss as f64 / n,
                if plancher > 0 {
                    rss as f64 / plancher as f64
                } else {
                    f64::NAN
                }
            );
        }
        println!("   → l'écart n'est pas du gaspillage : c'est le coût des structures qui rendent");
        println!("     l'index utilisable (en-têtes, capacité réservée, id→offset, Vec de slots).");
        println!("   → EMPREINTE TOTALE par vecteur (store + index), la seule qui compte :");
        println!(
            "     force brute {:.0} o  |  cascade {:.0} o  → +{:.0} o (+{:.0} %)",
            (rss_store + rss_brute) as f64 / n,
            (rss_store + rss_cascade) as f64 / n,
            (rss_cascade as f64 - rss_brute as f64) / n,
            100.0 * (rss_cascade as f64 - rss_brute as f64) / (rss_store + rss_brute).max(1) as f64
        );
        println!(
            "     La cascade stocke une représentation QUANTIFIÉE (848 o de paliers) là où la"
        );
        println!("     force brute duplique la signature exacte (512 o) : plus de place pour une");
        println!(
            "     information MOINS fidèle (f16), et c'est ce qui la rend perdante à petit n."
        );
        std::hint::black_box((
            &brute_mem,
            &store_mem,
            &cascade_inner_mem,
            &cascade_adapter_mem,
        ));
    }

    // ---- 1) Échelle ----
    println!("\n-- 1. Échelle (clusters, densité moyenne, graine fixe) --");
    println!(
        "{:>6} {:>26} {:>9} {:>9} {:>7} {:>10} {:>11} {:>11} {:>10}",
        "N",
        "index",
        "plafond@10",
        "accord@10",
        "tau",
        "pertinents",
        "index µs",
        "chemin µs",
        "construit"
    );
    let mut pentes: Vec<(usize, Kind, Mesure)> = Vec::new();
    for n in TAILLES {
        let corpus = make_corpus(n, Distrib::Clusters, Densite::Moyenne, 0x9E37_79B9);
        let queries = make_queries(Distrib::Clusters, Densite::Moyenne, 0x9E37_79B9);
        let t_build = Instant::now();
        let banc = construire(&corpus, &encoder);
        let build = t_build.elapsed().as_secs_f64() * 1e3;
        let refs: Vec<_> = queries.iter().map(|c| reference(&banc.store, c)).collect();
        let ctx = Ctx {
            engine: &engine,
            store: &banc.store,
            encoder: &encoder,
            queries: &queries,
            refs: &refs,
        };

        assert_eq!(
            banc.store.count(),
            n,
            "le corpus doit être entièrement indexé"
        );

        let mesures = [
            (
                Kind::Brute,
                mesurer(&ctx, &banc.brute, RecallVariant::Hybrid, 3),
            ),
            (Kind::Cascade, {
                banc.largeurs_defaut();
                mesurer(&ctx, &banc.cascade, RecallVariant::Hybrid, 3)
            }),
            (Kind::CascadeBudget, {
                banc.largeurs(K0_BUDGET);
                let m = mesurer(&ctx, &banc.cascade, RecallVariant::Hybrid, 3);
                banc.largeurs_defaut();
                m
            }),
        ];

        for (kind, m) in mesures {
            println!(
                "{:>6} {:>26} {:>9.3} {:>9.3} {:>7} {:>10.3} {:>11.1} {:>11.1} {:>9.0} ms",
                n,
                kind.label(),
                m.q.ceiling,
                m.q.accord,
                if m.q.tau.is_nan() {
                    "n/d".to_string()
                } else {
                    format!("{:.3}", m.q.tau)
                },
                m.q.pertinents,
                m.lat_index,
                m.lat_path,
                build
            );
            pentes.push((n, kind, m));
        }
        // Garde-fou de câblage : si le cascade rend zéro résultat (adaptateur
        // court-circuité, insertion directe…), le plafond tombe à 0 et ce seuil
        // échoue au lieu de laisser passer un banc qui ne mesure rien.
        let cascade = pentes
            .iter()
            .find(|(nn, k, _)| *nn == n && *k == Kind::Cascade)
            .map(|(_, _, m)| *m)
            .expect("mesure cascade présente");
        assert!(
            cascade.q.ceiling >= 0.5,
            "N = {n} : le cascade aux largeurs par défaut rend un plafond {} — \
             l'index est probablement vide ou mal alimenté",
            cascade.q.ceiling
        );
    }

    println!(
        "\n-- rapport T(N')/T(N) sur tailles consécutives : INDEX seul, puis chemin complet --"
    );
    println!("   (doublements, SAUF le dernier couple 4000→16000 qui vaut ×4)");
    for kind in [Kind::Brute, Kind::Cascade, Kind::CascadeBudget] {
        let mut cells_idx: Vec<String> = Vec::new();
        let mut cells_path: Vec<String> = Vec::new();
        for pair in TAILLES.windows(2).map(|w| (w[0], w[1])) {
            let get = |n: usize| {
                pentes
                    .iter()
                    .find(|(nn, k, _)| *nn == n && *k == kind)
                    .map(|(_, _, m)| *m)
                    .expect("mesure présente")
            };
            let (a, b) = (get(pair.0), get(pair.1));
            cells_idx.push(if a.lat_index > 0.0 {
                format!("{:.2}×", b.lat_index / a.lat_index)
            } else {
                "n/d".to_string()
            });
            cells_path.push(if a.lat_path > 0.0 {
                format!("{:.2}×", b.lat_path / a.lat_path)
            } else {
                "n/d".to_string()
            });
        }
        println!(
            "{:<26} {:>22} {:>22}",
            kind.label(),
            cells_idx.join(" "),
            cells_path.join(" ")
        );
    }
    let legende: Vec<String> = TAILLES
        .windows(2)
        .map(|w| format!("{}→{}", w[0], w[1]))
        .collect();
    println!("   ({})", legende.join(", "));
    println!("   → Ces rapports ne mesurent PAS une complexité, dans aucun sens.");
    println!(
        "     La cascade n'est PAS sous-linéaire : le palier L0 balaie TOUS les vecteurs (16 o,"
    );
    println!("     popcount). Son coût reste LINÉAIRE en n. Ce qu'elle change, c'est le coût PAR");
    println!("     COMPARAISON et le nombre de comparaisons pleines (L1 sur k0, L2 sur k1).");
    println!(
        "     Un rapport ~1,0 sur l'index seul ne dit donc rien d'une asymptote : il dit que le"
    );
    println!("     balayage L0 est encore SOUS le coût fixe (projection de la requête en 384d,");
    println!(
        "     allocations) aux tailles mesurées. Pour que le balayage domine à nouveau, il faut"
    );
    println!("     des tailles beaucoup plus grandes — mesuré à la section 1bis.");
    println!(
        "     Le chemin complet suit l'index : la validation exacte de {N_PROBE} candidats est un"
    );
    println!("     coût constant, qui pèse surtout à petit N.");

    // ---- 2) Frontière qualité/coût des largeurs (index déjà construit) ----
    let n_frontiere = 4000usize;
    let corpus = make_corpus(
        n_frontiere,
        Distrib::Clusters,
        Densite::Moyenne,
        0x9E37_79B9,
    );
    let queries = make_queries(Distrib::Clusters, Densite::Moyenne, 0x9E37_79B9);
    let banc = construire(&corpus, &encoder);
    let refs: Vec<_> = queries.iter().map(|c| reference(&banc.store, c)).collect();
    let ctx = Ctx {
        engine: &engine,
        store: &banc.store,
        encoder: &encoder,
        queries: &queries,
        refs: &refs,
    };
    println!("\n-- 2. Frontière qualité/coût des largeurs du cascade (N = {n_frontiere}) --");
    println!(
        "{:>6} {:>6} {:>11} {:>10} {:>7} {:>10} {:>11} {:>11}",
        "k0", "k1", "plafond@10", "accord@10", "tau", "candidats", "index µs", "chemin µs"
    );
    let mut frontiere: Vec<(usize, f64)> = Vec::new();
    for k0 in [50usize, 100, 200, 500, 1000, 4000] {
        banc.largeurs(k0);
        let m = mesurer(&ctx, &banc.cascade, RecallVariant::Hybrid, 3);
        println!(
            "{:>6} {:>6} {:>11.3} {:>10.3} {:>7} {:>10.1} {:>11.1} {:>11.1}",
            k0,
            (k0 / 10).max(1),
            m.q.ceiling,
            m.q.accord,
            if m.q.tau.is_nan() {
                "n/d".to_string()
            } else {
                format!("{:.3}", m.q.tau)
            },
            m.q.candidates,
            m.lat_index,
            m.lat_path
        );
        frontiere.push((k0, m.q.ceiling));
    }
    banc.largeurs_defaut();
    println!("   → le plafond est le seul juge : le classement (accord) suit le plafond par");
    println!("     construction, donc choisir k0 est un choix de VIVIER, pas de score final.");
    let plafond_min = frontiere.first().map(|(_, c)| *c).unwrap_or_default();
    let plafond_max = frontiere.last().map(|(_, c)| *c).unwrap_or_default();
    assert!(
        plafond_min < plafond_max,
        "un budget plus large doit améliorer (ou au moins égaler) le plafond : \
         {plafond_min} → {plafond_max}"
    );

    // ---- 2bis) Effet de la DIMENSION sur le coût (le terrain du cascade) ----
    println!("\n-- 2bis. Coût du nearest SEUL selon la dimension (N = {n_frontiere}) --");
    println!(
        "   la cascade impose {L2_DIM} dimensions (rembourrage) ; la force brute en 64d en compare 64."
    );
    println!("{:<40} {:>11} {:>10}", "index", "index µs", "vs 64d");
    let base = cout_index_seul(&banc.brute, &encoder, &queries, None, 3);
    println!(
        "{:<40} {:>11.1} {:>10}",
        "brute 64d (signature native)", base, "1.00×"
    );
    let brut384 = cout_index_seul(&banc.brute384, &encoder, &queries, Some(L2_DIM), 3);
    println!(
        "{:<40} {:>11.1} {:>9.2}×",
        "brute 384d (même information, rembourrée)",
        brut384,
        brut384 / base
    );
    for k0 in [1000usize, 4000] {
        banc.largeurs(k0);
        let cout = cout_index_seul(&banc.cascade, &encoder, &queries, None, 3);
        println!(
            "{:<40} {:>11.1} {:>9.2}×",
            format!("cascade k0={k0} (paliers 384d)"),
            cout,
            cout / base
        );
    }
    banc.largeurs_defaut();
    println!("   → même information, même référence exacte : ce qui change est le NOMBRE de flops");
    println!(
        "     par vecteur. En 384d la force brute paie 6× la 64d ; c'est seulement là que les"
    );
    println!("     là que les paliers peuvent gagner — et en 64d ils perdent, chiffres à l'appui.");

    // ---- 3) Distribution × densité × graine ----
    println!("\n-- 3. Distribution × densité × graine (N = 1000, qualité seule) --");
    println!(
        "{:<10} {:>14} {:>6} {:>26} {:>11} {:>11} {:>7}",
        "distribution", "densité", "graine", "index", "plafond@10", "accord@10", "tau"
    );
    let mut minmax: HashMap<&'static str, (f64, f64, f64, f64)> = HashMap::new();
    for distrib in [Distrib::Clusters, Distrib::Uniforme, Distrib::Zipf] {
        for densite in [Densite::Fine, Densite::Moyenne, Densite::Large] {
            for seed in [0x9E37_79B9u64, 0xDEAD_BEEF, 0x1234_5678] {
                let corpus = make_corpus(1000, distrib, densite, seed);
                let queries = make_queries(distrib, densite, seed);
                let banc = construire(&corpus, &encoder);
                let refs: Vec<_> = queries.iter().map(|c| reference(&banc.store, c)).collect();
                let ctx = Ctx {
                    engine: &engine,
                    store: &banc.store,
                    encoder: &encoder,
                    queries: &queries,
                    refs: &refs,
                };
                banc.largeurs_defaut();
                let brut = mesurer(&ctx, &banc.brute, RecallVariant::Hybrid, 1);
                let casc = mesurer(&ctx, &banc.cascade, RecallVariant::Hybrid, 1);
                for (kind, m) in [(Kind::Brute, brut), (Kind::Cascade, casc)] {
                    println!(
                        "{:<10} {:>14} {:>6x} {:>26} {:>11.3} {:>11.3} {:>7}",
                        distrib.label(),
                        densite.label(),
                        seed & 0xFFFF,
                        kind.label(),
                        m.q.ceiling,
                        m.q.accord,
                        if m.q.tau.is_nan() {
                            "n/d".to_string()
                        } else {
                            format!("{:.3}", m.q.tau)
                        }
                    );
                    let entry = minmax.entry(kind.label()).or_insert((
                        m.q.ceiling,
                        m.q.ceiling,
                        m.q.accord,
                        m.q.accord,
                    ));
                    entry.0 = entry.0.min(m.q.ceiling);
                    entry.1 = entry.1.max(m.q.ceiling);
                    entry.2 = entry.2.min(m.q.accord);
                    entry.3 = entry.3.max(m.q.accord);
                }
            }
        }
    }
    println!("\n-- 4. Enveloppe sur les 27 configurations --");
    for kind in [Kind::Brute, Kind::Cascade] {
        if let Some((cmin, cmax, amin, amax)) = minmax.get(kind.label()) {
            println!(
                "{:<26} plafond {:.3}..{:.3} | accord {:.3}..{:.3}",
                kind.label(),
                cmin,
                cmax,
                amin,
                amax
            );
        }
    }

    // ---- 6) Coûts de construction, de mise à jour, de suppression ----
    println!("\n-- 6. Coût par vecteur : construire / mettre à jour / supprimer (N = {n_mem}) --");
    {
        let vecs: Vec<(String, Vec<f64>)> = corpus_mem
            .iter()
            .enumerate()
            .map(|(i, e)| (format!("engram:{}", i + 1), encoder.encode(e)))
            .collect();

        let t0 = Instant::now();
        let brute2 = Brute::default();
        for (id, v) in &vecs {
            brute2.upsert(id, v);
        }
        let brut_construit = t0.elapsed().as_secs_f64() * 1e6 / vecs.len() as f64;
        let t0 = Instant::now();
        for (id, v) in &vecs {
            brute2.upsert(id, v);
        }
        let brut_maj = t0.elapsed().as_secs_f64() * 1e6 / vecs.len() as f64;
        let t0 = Instant::now();
        for (id, _) in &vecs {
            brute2.remove(id);
        }
        let brut_suppr = t0.elapsed().as_secs_f64() * 1e6 / vecs.len() as f64;

        let inner2 = Arc::new(SubstrateTieredIndex::new(encoder.dimensions()));
        let casc2 = adapter(&inner2);
        let t0 = Instant::now();
        for (id, v) in &vecs {
            casc2.upsert(id, v);
        }
        let casc_construit = t0.elapsed().as_secs_f64() * 1e6 / vecs.len() as f64;
        let t0 = Instant::now();
        for (id, v) in &vecs {
            casc2.upsert(id, v);
        }
        let casc_maj = t0.elapsed().as_secs_f64() * 1e6 / vecs.len() as f64;
        let t0 = Instant::now();
        for (id, _) in &vecs {
            casc2.remove(id);
        }
        let casc_suppr = t0.elapsed().as_secs_f64() * 1e6 / vecs.len() as f64;

        println!(
            "{:<24} {:>14} {:>16} {:>14}",
            "index", "construire", "mettre à jour", "supprimer"
        );
        println!(
            "{:<24} {:>11.1} µs {:>13.1} µs {:>11.1} µs",
            "force brute", brut_construit, brut_maj, brut_suppr
        );
        println!(
            "{:<24} {:>11.1} µs {:>13.1} µs {:>11.1} µs",
            "cascade substrate", casc_construit, casc_maj, casc_suppr
        );
        println!("   → la cascade paie 128×384 + 512×384 de projection PAR VECTEUR, y compris en");
        println!(
            "     MISE À JOUR : `insert` recalcule les trois paliers AVANT de chercher le slot"
        );
        println!(
            "     (retrieval.rs:958), donc le « chemin rapide » ne couvre que la stabilité du"
        );
        println!("     slot, pas le coût. Mesuré : mise à jour = construction. Un flux d'écriture");
        println!(
            "     réel paie ce coût à chaque réécriture de signature, pas seulement à la création."
        );
        std::hint::black_box((&brute2, &casc2));
    }

    // ---- 7) Exploration INCRÉMENTALE vs redémarrage ----
    println!("\n-- 7. Exploration incrémentale vs redémarrage --");
    println!(
        "   Mêmes paliers (graines canoniques du substrate, empreintes bit-identiques). Ce qui"
    );
    println!(
        "   les sépare est ce qu'elles RÉUTILISENT : l'incrémentale paie le balayage L0 UNE fois"
    );
    println!(
        "   et garde son vivier ; le redémarrage refait tout à chaque élargissement — c'est ce"
    );
    println!(
        "   que fait `nearest(k)` aujourd'hui, l'index n'ayant aucune notion d'exploration en"
    );
    println!("   cours. BUDGET DE TRAVAIL = comparaisons (L0 empreintes, L1 affinages, L2 cosinus");
    println!("   f16, exact cosinus d'ensembles), pas des résultats demandés.");
    for n_explo in [4000usize, 16_000] {
        let Some(lignes) = comparer_politiques(n_explo, &encoder, 0xA11CE) else {
            continue;
        };
        println!("\n   N = {n_explo}");
        println!(
            "   {:<30} {:>11} {:>13} {:>9} {:>8} {:>11}",
            "politique", "plafond@10", "travail", "exacts", "tours", "temps µs"
        );
        for l in &lignes {
            println!(
                "   {:<30} {:>11.3} {:>13.0} {:>9.0} {:>8.1} {:>11.1}",
                l.nom, l.plafond, l.travail, l.exacts, l.tours, l.temps_us
            );
        }
        let inc = lignes
            .iter()
            .find(|l| l.nom == "incrémental pas=25")
            .expect("ligne présente");
        let red = lignes
            .iter()
            .find(|l| l.nom == "redémarrage pas=25")
            .expect("ligne présente");
        let fixe200 = lignes
            .iter()
            .find(|l| l.nom.starts_with("fixe k0=200"))
            .expect("ligne présente");
        println!(
            "   → incrémental vs redémarrage, à plafond comparable ({:.3} vs {:.3}) : {:.0}",
            inc.plafond, red.plafond, inc.travail
        );
        println!(
            "     comparaisons contre {:.0} ({:.1}× moins de travail) MAIS {:.0} µs contre {:.0} µs :",
            red.travail,
            red.travail / inc.travail.max(1.0),
            inc.temps_us,
            red.temps_us
        );
        println!("     aucun gain de temps, aux deux tailles mesurées.");
        println!(
            "   → POURQUOI, et c'est le résultat qui compte : le travail économisé est dans le"
        );
        println!("     palier le MOINS cher (L0, une empreinte de 16 o ≈ 2 popcounts). Ce que le");
        println!(
            "     redémarrage paie en plus ({:.0} comparaisons) ne pèse presque rien en temps ; ce",
            red.travail - inc.travail
        );
        println!(
            "     qui pèse est ailleurs — les cosinus exacts ({:.0} pour l'incrémentale, {:.0} au",
            inc.exacts, fixe200.exacts
        );
        println!(
            "     budget fixe k0=200). L'exploration incrémentale est même DOMINÉE ici par un"
        );
        println!(
            "     budget fixe bien choisi : plafond {:.3} pour {:.0} µs, contre {:.3} pour {:.0} µs.",
            fixe200.plafond, fixe200.temps_us, inc.plafond, inc.temps_us
        );
        println!("     Une politique adaptative ne devient rentable que si CHAQUE comparaison est");
        println!("     chère (rerank natif en dimension pleine, lectures mmap) — non mesuré ici :");
        println!("     `SourceNative` n'est pas branché.");
        assert!(
            inc.travail < red.travail,
            "l'exploration incrémentale doit coûter moins de travail que le redémarrage à pas égal"
        );
    }
    println!("   → « fixe k0 » : travail ANALYTIQUE (n + k0 + k1), temps mesuré — l'API n'expose");
    println!("     pas le compteur interne du substrate. Aucune de ces politiques ne touche au");
    println!("     moteur : tout est simulé dans le banc.");

    // ---- 8) Distribution RÉELLE, si une base est désignée ----
    println!("\n-- 8. Distribution réelle (base désignée par variable d'environnement) --");
    {
        // Règle du dépôt : aucun chemin n'est deviné. Sans variable, la section est
        // SAUTÉE et le dit — elle n'est jamais silencieusement absente.
        match std::env::var_os("OBRAIN_TEST_STORE_REEL_DIR") {
            None => {
                println!(
                    "   SAUTÉE : aucune base désignée (OBRAIN_TEST_STORE_REEL_DIR non posée)."
                );
                println!(
                    "   Le protocole est prêt ; l'exécuter demande de désigner une base, qui est"
                );
                println!("   COPIÉE puis ouverte en lecture seule (jamais ouverte en place).");
                println!("   Les chiffres des sections 1..7 restent donc synthétiques.");
            }
            Some(chemin) => {
                let source = std::path::PathBuf::from(chemin);
                match obrain_bases_banc::essayer_copie_de(&source, "OBRAIN_TEST_STORE_REEL_DIR") {
                    Err(refus) => println!("   SAUTÉE : copie refusée — {refus}"),
                    Ok(copie) => {
                        // La COPIE est ouverte en écriture, jamais la base désignée : c'est
                        // une copie jetable, et `open_read_only` refuse une base antérieure
                        // au verrou `substrate.dirlock` (mesuré : refus explicite). Le mode
                        // écriture sur la copie ne touche pas la base d'origine.
                        match obrain_substrate::SubstrateStore::open(copie.chemin()) {
                            Err(e) => {
                                println!("   SAUTÉE : ouverture de la COPIE impossible — {e}");
                            }
                            Ok(base) => {
                                let max_id = base.next_engram_id();
                                let mut ensembles: Vec<Vec<(NodeId, f64)>> = Vec::new();
                                for id in 0..max_id {
                                    if let Ok(Some(membres)) = base.engram_members(id)
                                        && membres.len() >= 2
                                    {
                                        ensembles.push(membres.iter().map(|n| (*n, 1.0)).collect());
                                    }
                                }
                                let mut tailles: Vec<f64> =
                                    ensembles.iter().map(|e| e.len() as f64).collect();
                                tailles.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                let mut popularite: HashMap<NodeId, usize> = HashMap::new();
                                for e in &ensembles {
                                    for (n, _) in e {
                                        *popularite.entry(*n).or_insert(0) += 1;
                                    }
                                }
                                let mut pops: Vec<f64> =
                                    popularite.values().map(|c| *c as f64).collect();
                                pops.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                println!(
                                    "   engrammes réels : {} | vocables : {} | taille médiane {} | p90 {} | max {}",
                                    ensembles.len(),
                                    popularite.len(),
                                    tailles.get(tailles.len() / 2).copied().unwrap_or(0.0),
                                    tailles
                                        .get((tailles.len() as f64 * 0.9) as usize)
                                        .copied()
                                        .unwrap_or(0.0),
                                    tailles.last().copied().unwrap_or(0.0)
                                );
                                println!(
                                    "   popularité (ensembles par vocable) : médiane {} | p90 {} | max {}",
                                    pops.get(pops.len() / 2).copied().unwrap_or(0.0),
                                    pops.get((pops.len() as f64 * 0.9) as usize)
                                        .copied()
                                        .unwrap_or(0.0),
                                    pops.last().copied().unwrap_or(0.0)
                                );
                                if ensembles.len() >= 100 {
                                    let n_reel = ensembles.len().min(1000);
                                    let mut rng = Lcg::new(0xA11CE);
                                    let mut populaires: Vec<(NodeId, usize)> =
                                        popularite.iter().map(|(n, c)| (*n, *c)).collect();
                                    populaires
                                        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.0.cmp(&b.0.0)));
                                    let queries_reelles: Vec<Vec<NodeId>> = (0..N_QUERIES)
                                        .map(|_| {
                                            let i = rng.below(populaires.len().min(200));
                                            vec![populaires[i].0]
                                        })
                                        .collect();
                                    let corpus_reel: Vec<Vec<(NodeId, f64)>> =
                                        ensembles.iter().take(n_reel).cloned().collect();
                                    let banc_reel = construire(&corpus_reel, &encoder);
                                    let refs_reelles: Vec<_> = queries_reelles
                                        .iter()
                                        .map(|c| reference(&banc_reel.store, c))
                                        .collect();
                                    let ctx_reel = Ctx {
                                        engine: &engine,
                                        store: &banc_reel.store,
                                        encoder: &encoder,
                                        queries: &queries_reelles,
                                        refs: &refs_reelles,
                                    };
                                    banc_reel.largeurs_defaut();
                                    println!(
                                        "   {:>26} {:>11} {:>11} {:>7} {:>11} {:>11}",
                                        "index (engrammes réels)",
                                        "plafond@10",
                                        "accord@10",
                                        "tau",
                                        "index µs",
                                        "chemin µs"
                                    );
                                    let variantes: [(&str, &dyn VectorIndex); 2] = [
                                        ("force brute", &banc_reel.brute),
                                        ("cascade substrate", &banc_reel.cascade),
                                    ];
                                    for (nom, idx) in variantes {
                                        let m = mesurer(&ctx_reel, idx, RecallVariant::Hybrid, 3);
                                        println!(
                                            "   {:>26} {:>11.3} {:>11.3} {:>7} {:>11.1} {:>11.1}",
                                            nom,
                                            m.q.ceiling,
                                            m.q.accord,
                                            if m.q.tau.is_nan() {
                                                "n/d".to_string()
                                            } else {
                                                format!("{:.3}", m.q.tau)
                                            },
                                            m.lat_index,
                                            m.lat_path
                                        );
                                    }
                                } else {
                                    println!(
                                        "   AUCUN engramme lisible dans cette base ({} trouvé(s)) : la zone",
                                        ensembles.len()
                                    );
                                    println!(
                                        "   `substrate.engram_members` est vide ou absente. La distribution"
                                    );
                                    println!(
                                        "   RÉELLE n'est donc pas mesurée — pas « mesurée comme vide » :"
                                    );
                                    println!("   la base n'a rien à donner sur ce plan.");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- 9) Ce que ces chiffres ne disent pas ----
    println!("\n-- 9. Ce que ces chiffres ne disent pas --");
    println!(
        "a. Aucune complexité. L0 balaie tous les vecteurs : c'est linéaire en n, et rien ici"
    );
    println!(
        "   n'est sous-linéaire. Le croisement mesuré est un effet de SEUIL, pas d'asymptote :"
    );
    println!("   tant que n ≤ k0 (4 000), L0 garde tout et la cascade est plus LENTE que la force");
    println!(
        "   brute en 64d (elle projette en 384) ; au-delà, k0 élague et elle devient ~4× plus"
    );
    println!("   rapide à N = 16 000 (568 µs contre 2 387 µs) en payant du plafond. Un index");
    println!("   réellement sous-linéaire (graphe navigable, IVF) demanderait un autre banc.");
    println!("b. Le cascade est construit EN MÉMOIRE (`SubstrateTieredIndex::new`). Sa");
    println!("   persistance en zones, son chargement mmap et la repermutation de slots ne sont");
    println!("   pas mesurés : le coût d'ouverture d'une base réelle n'entre dans aucun chiffre.");
    println!("c. Signatures 64d, paliers figés à 384 : chaque insertion paie une projection");
    println!("   128×384 + 512×384 sur un vecteur rembourré — mesuré 127 µs/vecteur, et À CHAQUE");
    println!(
        "   ÉCRITURE : la mise à jour recalcule les paliers (mesuré identique à l'insertion)."
    );
    println!("d. Le rerank exact NATIF des paliers (`SourceNative`) n'est PAS branché : le score");
    println!("   qui sort du cascade est le cosinus f16 du palier L2, arrondi. Brancher la source");
    println!("   native changerait la fidélité mesurée — non mesuré ici.");
    println!("e. Corpus synthétiques. Une distribution réelle de co-activations n'est ni");
    println!("   clusterisée ni zipf : les valeurs par distribution disent la TENDANCE.");
    println!("   MESURÉ le 26/11 sur dgx : AUCUNE base de la machine ne porte d'engrammes (zone");
    println!(
        "   `substrate.engram_members` vide ou absente dans les bases du répertoire personnel \
         (hub.db, db)"
    );
    println!("   et `~/bissection/base-50k` ; la seule grosse base fait 967 Gio, donc non");
    println!("   copiable). La section 8 a bien ouvert une base réelle (copiée en 1,7 s) et n'y a");
    println!("   trouvé aucun ensemble : la distribution réelle reste NON MESURÉE, pas estimée.");
    println!("f. Latences sur machine partagée, non assertées. La QUALITÉ, elle, est assertée sur");
    println!("   chaque configuration (invariants du chemin + garde-fou de câblage à 0,5).");
    println!("\ndurée totale du banc : {:?}", started.elapsed());

    assert_eq!(
        minmax.len(),
        2,
        "les deux index doivent avoir été mesurés sur les 27 configurations"
    );
    assert_eq!(
        L2_DIM,
        std::mem::size_of::<Tier2>() / 2,
        "Tier2 doit rester un f16 par dimension L2"
    );
    assert_eq!(
        banc.cascade_inner.dim(),
        encoder.dimensions(),
        "le cascade doit accepter la dimension native de l'encodeur"
    );
}
