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
    Engram, EngramId, EngramStore, HybridRecallConfig, RecallEngine, RecallVariant, SpectralEncoder,
};
use obrain_common::types::NodeId;
use obrain_substrate::retrieval::{
    LargeursCascade, SubstrateTieredIndex, VectorIndex as SubstrateVectorIndex,
};
use obrain_substrate::tiers::{L2_DIM, Tier0, Tier1, Tier2};

const K: usize = 10;
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

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

#[derive(Default, Clone, Copy)]
struct Qualite {
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
        let top: Vec<EngramId> = pool.iter().copied().take(K).collect();
        let pool_hit = pool.iter().filter(|id| ref_top.contains(id)).count();
        let pooled = pool_hit as f64 / ref_top.len() as f64;
        let acc =
            top.iter().filter(|id| ref_top.contains(id)).count() as f64 / ref_top.len() as f64;
        assert!(
            acc <= pooled + 1e-9,
            "invariant : accord {acc} > plafond {pooled}"
        );
        if variant == RecallVariant::Hybrid {
            assert!(
                (acc - pooled).abs() < 1e-9,
                "rerank exact : accord {acc} ≠ plafond {pooled}"
            );
        }
        ceiling += pooled;
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
    for n in [250usize, 500, 1000, 2000, 4000] {
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

    println!("\n-- pente observée T(2N)/T(N) : coût de l'INDEX seul, puis du chemin complet --");
    println!(
        "{:<26} {:>22} {:>22}",
        "index", "index seul", "chemin complet"
    );
    for kind in [Kind::Brute, Kind::Cascade, Kind::CascadeBudget] {
        let mut cells_idx: Vec<String> = Vec::new();
        let mut cells_path: Vec<String> = Vec::new();
        for pair in [
            (250usize, 500usize),
            (500, 1000),
            (1000, 2000),
            (2000, 4000),
        ] {
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
    println!("   (250→500, 500→1000, 1000→2000, 2000→4000)");
    println!(
        "   → AUCUNE complexité n'est déduite de ces rapports. Les paliers L0 balaient TOUS les"
    );
    println!(
        "     vecteurs (16 o, popcount) : c'est linéaire. Un rapport ~1,0 sur l'index seul ne"
    );
    println!(
        "     prouve pas une sous-linéarité — il dit que le balayage L0 est encore SOUS le coût"
    );
    println!(
        "     fixe (projection de la requête en 384d, allocations) aux tailles mesurées. Le point"
    );
    println!(
        "     où le balayage redevient visible n'est pas atteint ici : le dire exigerait N ≫ 4000."
    );
    println!(
        "     Le chemin complet, lui, suit l'index : la validation exacte de {N_PROBE} candidats"
    );
    println!("     est un coût constant qui pèse surtout à petit N.");

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

    // ---- 5) Ce que ces chiffres ne disent pas ----
    println!("\n-- 5. Ce que ces chiffres ne disent pas --");
    println!("a. Aucune complexité. L0 balaie tous les vecteurs : c'est linéaire en n. Sur des");
    println!("   signatures 64d la cascade est même PLUS LENTE que la force brute (elle projette");
    println!(
        "   en 384) ; son avantage n'apparaît qu'à la dimension pour laquelle les paliers sont"
    );
    println!("   faits. Un index réellement sous-linéaire (graphe navigable, IVF) demanderait un");
    println!("   autre banc, avec un rappel à budget fixé qui ne s'effondre pas — non mesuré ici.");
    println!("b. Le cascade est construit EN MÉMOIRE (`SubstrateTieredIndex::new`). Sa");
    println!("   persistance en zones, son chargement mmap et la repermutation de slots ne sont");
    println!("   pas mesurés : le coût d'ouverture d'une base réelle n'entre dans aucun chiffre.");
    println!("c. Signatures 64d, paliers figés à 384 : chaque insertion paie une projection");
    println!("   128×384 + 512×384 sur un vecteur rembourré — coût de construction qui ne");
    println!("   index). Un encodeur natif en 384d ne paierait pas ce rembourrage ; nous, si.");
    println!("d. Le rerank exact NATIF des paliers (`SourceNative`) n'est PAS branché : le score");
    println!("   qui sort du cascade est le cosinus f16 du palier L2, arrondi. Brancher la source");
    println!("   native changerait la fidélité mesurée — non mesuré ici.");
    println!("e. Corpus synthétiques. Une distribution réelle de co-activations n'est ni");
    println!("   clusterisée ni zipf : les valeurs par distribution disent la TENDANCE.");
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
