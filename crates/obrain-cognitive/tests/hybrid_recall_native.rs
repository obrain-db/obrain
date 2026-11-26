//! Rappel hybride NATIF — banc du chemin réel (suite de #422).
//!
//! Ce banc ne balaie pas des signatures : il fait tourner le chemin
//! `indices → encodeur → VectorIndex::nearest → validation exacte → classement`
//! sur un corpus reproductible, et compare quatre variantes à une référence
//! exacte calculée indépendamment (balayage exhaustif du store, réimplémenté
//! ici pour ne pas se juger avec le code qu'on mesure).
//!
//! ## Ce que le banc prouve (assertions)
//!
//! 1. La référence exacte se reproduit exactement (`accord@10 == 1.0`).
//! 2. Avec un budget couvrant tout le corpus, l'hybride retrouve exactement le
//!    classement de référence (`accord@10 == 1.0`) — le rerank est correct.
//! 3. Chaque `exact_score` rendu par l'hybride égale le cosinus pondéré
//!    recalculé par le banc (1e-12) — la validation est réelle, pas un
//!    recopiage de la distance d'index.
//! 4. `accord@k ≤ plafond candidat` pour chaque requête — le classement ne
//!    peut pas dépasser son vivier : c'est l'invariant qui dit où se perd le
//!    rappel.
//! 5. Le chemin existant `RecallEngine::recall` n'appelle **jamais**
//!    `VectorIndex::nearest` dès que la matrice Hopfield est non vide (index
//!    instrumenté), alors que le chemin hybride l'appelle une fois par requête.
//! 6. Les identifiants écrits par `EngramManager` (`"engram:N"`) sont
//!    illisibles pour `id_str.parse::<u64>()` — l'ancien consommateur jette
//!    donc en silence tout ce que le producteur a indexé.
//!
//! ## Ce que le banc mesure (imprimé, jamais asserté)
//!
//! rappel/accord@10, précision@10, plafond candidat, coût (candidats, validations
//! exactes), latence médiane, mémoire des signatures, et les ambiguïtés
//! (scores exacts nuls, ex æquo à la frontière du top-k, collisions de
//! signature). Aucun chiffre de latence n'est asserté : une machine chargée
//! n'est pas une régression.

#![cfg(feature = "engram")]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use obrain_cognitive::engram::traits::{InMemoryVectorIndex, VectorIndex};
use obrain_cognitive::engram::{
    Engram, EngramId, EngramStore, HybridRecallConfig, RecallEngine, RecallVariant,
    SpectralEncoder, parse_engram_id,
};
use obrain_common::types::NodeId;

// ---------------------------------------------------------------------------
// Corpus reproductible
// ---------------------------------------------------------------------------

const N_ENGRAMS: usize = 600;
const N_CLUSTERS: usize = 20;
const CLUSTER_SIZE: usize = 20;
const N_QUERIES: usize = 40;
const K: usize = 10;
const PROBES: [usize; 6] = [5, 10, 25, 50, 100, 200];
const LATENCY_REPS: usize = 7;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Générateur congruentiel linéaire : reproductible, sans dépendance.
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

/// Corpus structuré : chaque engramme tire la moitié de ses membres de son
/// cluster d'appartenance, le reste du vocabulaire. Sans ce recouvrement intra-
/// cluster, tous les cosinus exacts vaudraient zéro et la référence n'aurait
/// aucun sens.
fn make_corpus() -> Vec<Vec<(NodeId, f64)>> {
    let mut rng = Lcg::new(SEED);
    let mut corpus = Vec::with_capacity(N_ENGRAMS);
    for i in 0..N_ENGRAMS {
        let cluster = i % N_CLUSTERS;
        let size = 3 + rng.below(6); // 3..8
        let mut ensemble: Vec<(NodeId, f64)> = Vec::with_capacity(size);
        let mut used: HashSet<NodeId> = HashSet::new();
        let home = size.div_ceil(2).max(2);
        let mut tries = 0;
        while ensemble.len() < home && tries < 8 * size {
            tries += 1;
            let node = NodeId((cluster * CLUSTER_SIZE + rng.below(CLUSTER_SIZE) + 1) as u64);
            if used.insert(node) {
                ensemble.push((node, rng.weight()));
            }
        }
        tries = 0;
        while ensemble.len() < size && tries < 8 * size {
            tries += 1;
            let node = NodeId((rng.below(N_CLUSTERS * CLUSTER_SIZE) + 1) as u64);
            if used.insert(node) {
                ensemble.push((node, rng.weight()));
            }
        }
        corpus.push(ensemble);
    }
    corpus
}

/// Requêtes : 2 à 4 indices tirés du cluster `q * 7 mod N_CLUSTERS`.
fn make_queries() -> Vec<Vec<NodeId>> {
    let mut rng = Lcg::new(SEED ^ 0xDEAD_BEEF);
    (0..N_QUERIES)
        .map(|q| {
            let cluster = (q * 7) % N_CLUSTERS;
            let n = 2 + rng.below(3);
            let mut cues: Vec<NodeId> = Vec::with_capacity(n);
            let mut used: HashSet<NodeId> = HashSet::new();
            let mut tries = 0;
            while cues.len() < n && tries < 16 {
                tries += 1;
                let node = NodeId((cluster * CLUSTER_SIZE + rng.below(CLUSTER_SIZE) + 1) as u64);
                if used.insert(node) {
                    cues.push(node);
                }
            }
            cues
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Index instrumenté — le chemin réel, avec un compteur
// ---------------------------------------------------------------------------

/// `VectorIndex` natif (force brute) enveloppé d'un compteur : le chemin mesuré
/// ne change pas, on observe seulement s'il touche l'index.
#[derive(Default)]
struct CountingIndex {
    inner: InMemoryVectorIndex,
    nearest_calls: AtomicUsize,
    upserts: AtomicUsize,
}

impl CountingIndex {
    fn nearest_calls(&self) -> usize {
        self.nearest_calls.load(Ordering::SeqCst)
    }

    fn upserts(&self) -> usize {
        self.upserts.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.nearest_calls.store(0, Ordering::SeqCst);
    }
}

impl VectorIndex for CountingIndex {
    fn upsert(&self, id: &str, vector: &[f64]) {
        self.upserts.fetch_add(1, Ordering::SeqCst);
        self.inner.upsert(id, vector);
    }

    fn nearest(&self, query: &[f64], k: usize) -> Vec<(String, f64)> {
        self.nearest_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.nearest(query, k)
    }

    fn remove(&self, id: &str) {
        self.inner.remove(id);
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

// ---------------------------------------------------------------------------
// Référence exacte, calculée SANS le code mesuré
// ---------------------------------------------------------------------------

/// Cosinus pondéré exact, réimplémenté dans le banc : la mesure ne se juge pas
/// avec la fonction qu'elle mesure.
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

/// Classement exact de tout le corpus pour une requête.
struct Reference {
    /// `(engramme, score exact)` triés par score décroissant puis par identifiant.
    ranked: Vec<(EngramId, f64)>,
    /// Engrammes dont le score exact est nul (aucun nœud partagé).
    zeros: usize,
    /// Vrai si le score du k-ième et du (k+1)-ième sont égaux (frontière ambiguë).
    tie_at_k: bool,
    /// Vrai si les `k` premiers scores sont tous nuls (référence dégénérée).
    degenerate: bool,
}

fn reference(store: &EngramStore, cues: &[NodeId]) -> Reference {
    let query: Vec<(NodeId, f64)> = cues.iter().map(|&n| (n, 1.0)).collect();
    let mut ranked: Vec<(EngramId, f64)> = Vec::new();
    let mut zeros = 0usize;
    store.for_each(|engram| {
        let score = harness_cosine(&query, &engram.ensemble);
        if score <= 0.0 {
            zeros += 1;
        }
        ranked.push((engram.id, score));
    });
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.0.cmp(&b.0.0))
    });
    let tie_at_k = ranked.len() > K && (ranked[K - 1].1 - ranked[K].1).abs() < 1e-12;
    let degenerate = ranked.iter().take(K).all(|(_, s)| *s <= 0.0);
    Reference {
        ranked,
        zeros,
        tie_at_k,
        degenerate,
    }
}

// ---------------------------------------------------------------------------
// Mesure d'une variante
// ---------------------------------------------------------------------------

/// Une ligne de mesure pour une variante et un budget.
#[derive(Clone)]
struct Row {
    variant: &'static str,
    n_probe: usize,
    accord: f64,
    precision: f64,
    ceiling: f64,
    candidates: f64,
    validations: f64,
    latency_us: f64,
    returned: f64,
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

/// Précision@10 du banc : fraction des hits qui partagent au moins un nœud avec
/// la requête (score exact du banc > 0).
fn harness_precision(store: &EngramStore, hits: &[EngramId], query: &[(NodeId, f64)]) -> f64 {
    if hits.is_empty() {
        return 0.0;
    }
    let relevant = hits
        .iter()
        .filter(|id| {
            store
                .get(**id)
                .is_some_and(|e| harness_cosine(query, &e.ensemble) > 0.0)
        })
        .count();
    relevant as f64 / hits.len() as f64
}

fn ids(hits: &[obrain_cognitive::engram::HybridHit]) -> Vec<EngramId> {
    hits.iter().map(|h| h.engram_id).collect()
}

// ---------------------------------------------------------------------------
// Le banc
// ---------------------------------------------------------------------------

#[test]
fn banc_rappel_hybride_natif() {
    let bench_started = Instant::now();
    let corpus = make_corpus();
    let queries = make_queries();
    let encoder = SpectralEncoder::new();
    let dims = encoder.dimensions();

    let store = EngramStore::new(None);
    let index = CountingIndex::default();
    let mut ensemble_bytes = 0usize;
    for (i, ensemble) in corpus.iter().enumerate() {
        let mut engram = Engram::new(EngramId(i as u64 + 1), ensemble.clone());
        engram.spectral_signature = encoder.encode(ensemble);
        ensemble_bytes +=
            ensemble.len() * (std::mem::size_of::<NodeId>() + std::mem::size_of::<f64>());
        store.insert(engram);
    }
    // Indexé comme le fait `EngramManager` : `EngramId::to_string()` → "engram:N".
    for engram in store.list() {
        index.upsert(&engram.id.to_string(), &engram.spectral_signature);
    }

    let engine = RecallEngine::new();
    let signature_bytes = N_ENGRAMS * dims * std::mem::size_of::<f64>();

    // --- Références exactes, une par requête ---
    let refs: Vec<Reference> = queries.iter().map(|cues| reference(&store, cues)).collect();

    // --- Référence croisée : la variante `Exact` doit reproduire le banc ---
    let mut exact_accord = Vec::new();
    let mut exact_latency = Vec::new();
    let mut ref_top1_mismatch = 0usize;
    let cfg_full = HybridRecallConfig {
        k: N_ENGRAMS,
        n_probe: 1,
        ..HybridRecallConfig::default()
    };
    for (q, cues) in queries.iter().enumerate() {
        let outcome = engine
            .hybrid_recall(
                &store,
                cues,
                &index,
                &encoder,
                RecallVariant::Exact,
                &cfg_full,
            )
            .expect("la référence exacte s'exécute");
        assert_eq!(
            outcome.stats.index_calls, 0,
            "la variante exacte ne doit pas interroger l'index"
        );
        let top: Vec<EngramId> = ids(&outcome.hits).into_iter().take(K).collect();
        let ref_top: Vec<EngramId> = refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
        exact_accord.push(overlap(&top, &ref_top) as f64 / ref_top.len() as f64);
        if top.first() != ref_top.first() {
            ref_top1_mismatch += 1;
        }
        let t0 = Instant::now();
        let _ = engine
            .hybrid_recall(
                &store,
                cues,
                &index,
                &encoder,
                RecallVariant::Exact,
                &cfg_full,
            )
            .expect("la référence exacte s'exécute");
        exact_latency.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    assert_eq!(
        ref_top1_mismatch, 0,
        "la variante exacte doit retrouver le top-1 du banc pour toutes les requêtes"
    );
    for accord in &exact_accord {
        assert!(
            (accord - 1.0).abs() < 1e-12,
            "la référence exacte doit se reproduire exactement, mesuré {accord}"
        );
    }

    // --- Le chemin actuel, tel qu'il est, sur le même corpus ---
    let mut current_accord = Vec::new();
    let mut current_precision = Vec::new();
    let mut current_latency = Vec::new();
    let mut current_nearest_calls = 0usize;
    for (q, cues) in queries.iter().enumerate() {
        index.reset();
        let t0 = Instant::now();
        let results = engine.recall(&store, cues, &index, &encoder, K);
        current_latency.push(t0.elapsed().as_secs_f64() * 1e6);
        current_nearest_calls += index.nearest_calls();
        let top: Vec<EngramId> = results.iter().map(|r| r.engram_id).take(K).collect();
        let ref_top: Vec<EngramId> = refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
        current_accord.push(overlap(&top, &ref_top) as f64 / ref_top.len() as f64);
        let query: Vec<(NodeId, f64)> = cues.iter().map(|&n| (n, 1.0)).collect();
        current_precision.push(harness_precision(&store, &top, &query));
    }
    assert_eq!(
        current_nearest_calls, 0,
        "constat : tant que la matrice Hopfield est non vide, RecallEngine::recall \
         n'interroge jamais l'index natif (mesuré sur {} requêtes)",
        N_QUERIES
    );

    // --- Les quatre variantes, par budget de candidats ---
    let mut rows: Vec<Row> = Vec::new();
    let mut checked_exact_scores = 0usize;
    let mut structural_violations = 0usize;

    for n_probe in PROBES {
        for variant in [
            RecallVariant::Spectral,
            RecallVariant::Hybrid,
            RecallVariant::HybridStructural,
        ] {
            let cfg = HybridRecallConfig {
                k: n_probe,
                n_probe,
                ..HybridRecallConfig::default()
            };
            let mut accord = Vec::new();
            let mut precision = Vec::new();
            let mut ceiling = Vec::new();
            let mut candidates = Vec::new();
            let mut validations = Vec::new();
            let mut latency = Vec::new();
            let mut returned = Vec::new();

            for (q, cues) in queries.iter().enumerate() {
                let mut samples = Vec::with_capacity(LATENCY_REPS);
                let mut outcome = None;
                for _ in 0..LATENCY_REPS {
                    let t0 = Instant::now();
                    let o = engine
                        .hybrid_recall(&store, cues, &index, &encoder, variant, &cfg)
                        .expect("le chemin s'exécute");
                    samples.push(t0.elapsed().as_secs_f64() * 1e6);
                    outcome = Some(o);
                }
                let outcome = outcome.expect("au moins une exécution");
                latency.push(median(samples));

                assert_eq!(
                    outcome.stats.index_calls,
                    1,
                    "{} doit interroger l'index natif une fois",
                    variant.label()
                );
                assert_eq!(
                    outcome.stats.unknown_ids, 0,
                    "les identifiants « engram:N » du manager doivent être lisibles"
                );
                assert_eq!(
                    outcome.stats.stale_ids, 0,
                    "index et store doivent coïncider"
                );

                let top_all: Vec<EngramId> = ids(&outcome.hits);
                // Avec `exact_weight = 1.0`, le rerank exact ordonne les candidats par
                // score exact : l'accord doit ÉGALER le plafond du vivier. Ce n'est pas
                // un résultat empirique mais une propriété du chemin — l'assertion
                // vérifie que le code la tient réellement.
                let top: Vec<EngramId> = top_all.iter().copied().take(K).collect();
                let ref_top: Vec<EngramId> =
                    refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
                let pool_hit = overlap(&top_all, &ref_top);
                let pooled = pool_hit as f64 / ref_top.len() as f64;
                let acc = overlap(&top, &ref_top) as f64 / ref_top.len() as f64;
                if variant == RecallVariant::Hybrid {
                    assert!(
                        (acc - pooled).abs() < 1e-9,
                        "rerank exact : accord {acc} ≠ plafond {pooled} — le classement \
                         ne restitue pas l'ordre exact des candidats"
                    );
                }
                assert!(
                    acc <= pooled + 1e-9,
                    "invariant violé : accord {acc} > plafond candidat {pooled} \
                     (le classement dépasserait son vivier)"
                );

                let query: Vec<(NodeId, f64)> = cues.iter().map(|&n| (n, 1.0)).collect();
                precision.push(harness_precision(&store, &top, &query));
                accord.push(acc);
                ceiling.push(pooled);
                candidates.push(outcome.stats.candidates as f64);
                validations.push(outcome.stats.exact_validations as f64);
                returned.push(outcome.hits.len() as f64);

                // La validation exacte du chemin est comparée au calcul du banc.
                for hit in &outcome.hits {
                    if let Some(score) = hit.exact_score {
                        let engram = store.get(hit.engram_id).expect("engramme présent");
                        let expected = harness_cosine(&query, &engram.ensemble);
                        assert!(
                            (score - expected).abs() < 1e-12,
                            "{} : exact_score {} ≠ banc {}",
                            variant.label(),
                            score,
                            expected
                        );
                        checked_exact_scores += 1;
                    }
                }
                if variant == RecallVariant::HybridStructural {
                    for hit in &outcome.hits {
                        if hit.closure_shared.unwrap_or(0) < 1 {
                            structural_violations += 1;
                        }
                    }
                }
            }

            rows.push(Row {
                variant: variant.label(),
                n_probe,
                accord: moyenne(&accord),
                precision: moyenne(&precision),
                ceiling: moyenne(&ceiling),
                candidates: moyenne(&candidates),
                validations: moyenne(&validations),
                latency_us: moyenne(&latency),
                returned: moyenne(&returned),
            });
        }
    }

    // --- Témoin : validation exacte payée mais neutralisée (exact_weight = 0) ---
    let cfg_temoin = HybridRecallConfig {
        k: 50,
        n_probe: 50,
        exact_weight: 0.0,
        ..HybridRecallConfig::default()
    };
    let mut temoin_accord = Vec::new();
    let mut temoin_validations = 0.0;
    let mut temoin_latency = Vec::new();
    for (q, cues) in queries.iter().enumerate() {
        let t0 = Instant::now();
        let outcome = engine
            .hybrid_recall(
                &store,
                cues,
                &index,
                &encoder,
                RecallVariant::Hybrid,
                &cfg_temoin,
            )
            .expect("le témoin s'exécute");
        temoin_latency.push(t0.elapsed().as_secs_f64() * 1e6);
        temoin_validations += outcome.stats.exact_validations as f64;
        let top: Vec<EngramId> = ids(&outcome.hits).into_iter().take(K).collect();
        let ref_top: Vec<EngramId> = refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
        temoin_accord.push(overlap(&top, &ref_top) as f64 / ref_top.len() as f64);
    }
    let temoin_validations = temoin_validations / N_QUERIES as f64;

    // --- Porte structurelle : profondeur × seuil ---
    //
    // À `hops >= 1` et `min_shared = 1`, la clôture contient les indices, donc
    // tout candidat de score exact > 0 y partage au moins un nœud : la porte ne
    // peut retirer que des candidats de score exact nul. Elle est alors
    // ÉQUIVALENTE à filtrer sur `exact_score > 0`, et ne peut pas faire perdre
    // d'accord — c'est une propriété, pas une mesure. Ce que la profondeur
    // ajoute vraiment se lit à `min_shared >= 2`, où elle peut retirer des
    // candidats pourtant pertinents.
    let cfg_gate_ref = HybridRecallConfig {
        k: 50,
        n_probe: 50,
        ..HybridRecallConfig::default()
    };
    let mut gate_ref_accord = Vec::new();
    for (q, cues) in queries.iter().enumerate() {
        let outcome = engine
            .hybrid_recall(
                &store,
                cues,
                &index,
                &encoder,
                RecallVariant::Hybrid,
                &cfg_gate_ref,
            )
            .expect("la référence sans porte s'exécute");
        let top: Vec<EngramId> = ids(&outcome.hits).into_iter().take(K).collect();
        let ref_top: Vec<EngramId> = refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
        gate_ref_accord.push(overlap(&top, &ref_top) as f64 / ref_top.len() as f64);
    }
    let gate_ref_accord_mean = moyenne(&gate_ref_accord);

    struct GateRow {
        hops: usize,
        min_shared: usize,
        min_overlap: f64,
        accord: f64,
        precision: f64,
        returned: f64,
        closure: f64,
        rejected: f64,
    }
    let mut gate_rows: Vec<GateRow> = Vec::new();
    for (hops, min_shared, min_overlap) in [
        (1usize, 1usize, 0.0f64),
        (2, 1, 0.0),
        (1, 1, 0.2),
        (1, 1, 0.4),
        (1, 1, 0.6),
    ] {
        let cfg_gate = HybridRecallConfig {
            k: 50,
            n_probe: 50,
            structural_hops: hops,
            structural_min_shared: min_shared,
            structural_min_overlap: min_overlap,
            ..HybridRecallConfig::default()
        };
        let mut accord = Vec::new();
        let mut precision = Vec::new();
        let mut returned = Vec::new();
        let mut closure = Vec::new();
        let mut rejected = Vec::new();
        for (q, cues) in queries.iter().enumerate() {
            let outcome = engine
                .hybrid_recall(
                    &store,
                    cues,
                    &index,
                    &encoder,
                    RecallVariant::HybridStructural,
                    &cfg_gate,
                )
                .expect("la porte structurelle s'exécute");
            let top: Vec<EngramId> = ids(&outcome.hits).into_iter().take(K).collect();
            let ref_top: Vec<EngramId> = refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
            let acc = overlap(&top, &ref_top) as f64 / ref_top.len() as f64;
            if min_shared == 1 && min_overlap == 0.0 {
                // Propriété : la porte ne peut pas retirer un candidat à score exact > 0.
                assert!(
                    (acc - gate_ref_accord[q]).abs() < 1e-9,
                    "porte à min_shared = 1 : accord {acc} ≠ sans porte {} — un candidat                      pertinent a été retiré",
                    gate_ref_accord[q]
                );
            }
            let query: Vec<(NodeId, f64)> = cues.iter().map(|&n| (n, 1.0)).collect();
            precision.push(harness_precision(&store, &top, &query));
            accord.push(acc);
            returned.push(outcome.hits.len() as f64);
            closure.push(outcome.stats.closure_nodes as f64);
            rejected.push(outcome.stats.rejected_structural as f64);
        }
        gate_rows.push(GateRow {
            hops,
            min_shared,
            min_overlap,
            accord: moyenne(&accord),
            precision: moyenne(&precision),
            returned: moyenne(&returned),
            closure: moyenne(&closure),
            rejected: moyenne(&rejected),
        });
    }

    // --- Récupération parfaite : budget = corpus entier ---
    let cfg_exhaustif = HybridRecallConfig {
        k: N_ENGRAMS,
        n_probe: N_ENGRAMS,
        ..HybridRecallConfig::default()
    };
    let mut full_accord = Vec::new();
    for (q, cues) in queries.iter().enumerate() {
        let outcome = engine
            .hybrid_recall(
                &store,
                cues,
                &index,
                &encoder,
                RecallVariant::Hybrid,
                &cfg_exhaustif,
            )
            .expect("le chemin hybride s'exécute");
        let top: Vec<EngramId> = ids(&outcome.hits).into_iter().take(K).collect();
        let ref_top: Vec<EngramId> = refs[q].ranked.iter().take(K).map(|(id, _)| *id).collect();
        full_accord.push(overlap(&top, &ref_top) as f64 / ref_top.len() as f64);
    }
    assert_eq!(
        outcome_full_label(&full_accord),
        "1.000",
        "avec un budget couvrant le corpus, l'hybride doit retrouver exactement \
         le classement de référence (accord {:?})",
        full_accord
    );
    assert!(
        checked_exact_scores > 0,
        "au moins une validation exacte doit avoir été comparée"
    );
    assert_eq!(
        structural_violations, 0,
        "la porte structurelle ne doit laisser passer aucun hit sans recouvrement"
    );

    // --- Collisions de signature : deux engrammes distincts, même projection ? ---
    let mut signatures: HashMap<Vec<u64>, usize> = HashMap::new();
    for engram in store.list() {
        let key: Vec<u64> = engram
            .spectral_signature
            .iter()
            .map(|v| v.to_bits())
            .collect();
        *signatures.entry(key).or_insert(0) += 1;
    }
    let collided = signatures
        .values()
        .filter(|count| **count > 1)
        .sum::<usize>();

    // --- Rapport ---
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!("\n=== RAPPEL HYBRIDE NATIF — banc du chemin réel ===");
    println!(
        "corpus : {N_ENGRAMS} engrammes, {N_CLUSTERS}×{CLUSTER_SIZE} vocables, tailles 3..8, \
         poids 0.5..1.5 | requêtes : {N_QUERIES} (2..4 indices) | k = {K} | dims = {dims}"
    );
    println!("charge machine au moment du test : {}", load.trim());
    println!(
        "mémoire des signatures (modèle) : store {} o + index {} o = {} o ; \
         ensembles {} o",
        signature_bytes,
        signature_bytes,
        signature_bytes * 2,
        ensemble_bytes
    );
    println!(
        "index instrumenté : {} upserts, {} entrées distinctes de signature sur {} engrammes \
         ({} engrammes dans une classe de collision)",
        index.upserts(),
        signatures.len(),
        N_ENGRAMS,
        collided
    );

    println!("\n-- chemin existant RecallEngine::recall (direct + Hopfield) --");
    println!(
        "accord@{} = {:.3} | précision@{} = {:.3} | appels à VectorIndex::nearest = {} | \
         latence médiane = {:.1} µs",
        K,
        moyenne(&current_accord),
        K,
        moyenne(&current_precision),
        current_nearest_calls,
        median(current_latency)
    );

    println!(
        "\n-- récupération parfaite (hybride, budget = corpus entier) : accord@{K} = {:.3} --",
        moyenne(&full_accord)
    );
    println!("\n-- variante de référence (balayage exact exhaustif) --");
    println!(
        "accord@{K} = {:.3} | latence médiane = {:.1} µs | validations exactes par requête = {}",
        moyenne(&exact_accord),
        median(exact_latency),
        N_ENGRAMS
    );

    println!(
        "\n-- variantes indexées (budget de candidats) --\n\
         {:<20} {:>7} {:>9} {:>12} {:>11} {:>8} {:>11} {:>13} {:>12}",
        "variante",
        "budget",
        "accord@10",
        "plafond cand",
        "précision",
        "rendus",
        "candidats",
        "validations",
        "latence µs"
    );
    for row in &rows {
        println!(
            "{:<20} {:>7} {:>9.3} {:>12.3} {:>10.3} {:>8.1} {:>11.1} {:>13.1} {:>12.1}",
            row.variant,
            row.n_probe,
            row.accord,
            row.ceiling,
            row.precision,
            row.returned,
            row.candidates,
            row.validations,
            row.latency_us
        );
    }

    println!("\n-- témoin : validation calculée mais neutralisée (exact_weight = 0, budget 50) --");
    println!(
        "accord@{K} = {:.3} | validations exactes payées = {:.0} | latence = {:.1} µs",
        moyenne(&temoin_accord),
        temoin_validations,
        moyenne(&temoin_latency)
    );

    println!(
        "\n-- porte structurelle (budget 50 ; sans porte : accord@{K} = {gate_ref_accord_mean:.3}) --"
    );
    println!(
        "{:<7} {:>10} {:>11} {:>9} {:>10} {:>8} {:>9} {:>9}",
        "sauts",
        "min_partagé",
        "min_recouv",
        "accord@10",
        "précision",
        "rendus",
        "clôture",
        "rejetés"
    );
    for row in &gate_rows {
        println!(
            "{:<7} {:>10} {:>11.2} {:>9.3} {:>10.3} {:>8.1} {:>9.1} {:>9.1}",
            row.hops,
            row.min_shared,
            row.min_overlap,
            row.accord,
            row.precision,
            row.returned,
            row.closure,
            row.rejected
        );
    }

    println!("\n-- ambiguïtés de la référence exacte --");
    let zeros: Vec<f64> = refs.iter().map(|r| r.zeros as f64).collect();
    println!(
        "scores exacts nuls par requête : moyenne {:.1} / {} engrammes ({:.1} %)",
        moyenne(&zeros),
        N_ENGRAMS,
        100.0 * moyenne(&zeros) / N_ENGRAMS as f64
    );
    println!(
        "requêtes avec ex æquo à la frontière du top-{K} : {} / {}",
        refs.iter().filter(|r| r.tie_at_k).count(),
        N_QUERIES
    );
    println!(
        "requêtes à référence dégénérée (top-{K} entièrement nul) : {} / {}",
        refs.iter().filter(|r| r.degenerate).count(),
        N_QUERIES
    );

    println!("\n-- limites expérimentales --");
    println!(
        "1. L'index sous-jacent est à force brute (InMemoryVectorIndex) : nearest coûte O(n·d),"
    );
    println!("   donc les latences ci-dessus ne montrent AUCUN gain d'index — elles comparent des");
    println!(
        "   chemins, pas des complexités. Un index sous-linéaire (paliers substrate) est requis"
    );
    println!("   pour que le gain candidat se traduise en temps.");
    println!("2. L'encodeur est une projection par hachage : deux ensembles disjoints sont");
    println!("   quasi orthogonaux et le score exact est nul (voir « ambiguïtés »). Le plafond");
    println!("   candidat mesure cette perte avant tout classement.");
    println!("3. Corpus synthétique structuré (clusters) : le recouvrement intra-cluster rend le");
    println!("   rappel exact non dégénéré. Un graphe réel a une distribution de degrés et de");
    println!("   co-activations différente ; les valeurs ne se transportent pas telles quelles.");
    println!("4. Les latences sont mesurées en profil de test optimisé sur une machine partagée :");
    println!("   elles servent d'ordre de grandeur relatif, pas de chiffre de production.");
    println!("5. Aucun dictionnaire lexical n'intervient : la porte structurelle lit le graphe");
    println!("   (co-appartenance aux engrammes), pas le texte.");

    assert_eq!(
        rows.len(),
        PROBES.len() * 3,
        "trois variantes indexées × chaque budget"
    );
    let last = PROBES.last().copied().unwrap_or(0);
    let full = rows
        .iter()
        .find(|r| r.n_probe == last && r.variant == "hybride")
        .expect("ligne hybride au budget maximal");
    println!(
        "\nrésumé : à budget {last}, l'hybride restitue exactement le plafond du vivier \
         ({:.3}) ; ce qui reste perdu l'est à la GÉNÉRATION des candidats, pas au classement.",
        full.accord
    );
    println!("durée totale du banc : {:?}", bench_started.elapsed());

    // Contrôle du format d'identifiant : ce que le manager écrit n'est pas ce que
    // l'ancien consommateur savait lire.
    assert!(
        "engram:7".parse::<u64>().is_err(),
        "« engram:7 » n'est pas un entier : l'ancien consommateur jetait ces entrées"
    );
    assert_eq!(parse_engram_id("engram:7"), Some(EngramId(7)));
}

/// Rend la moyenne d'accords formatée à trois décimales — sert aux assertions
/// de récupération parfaite, où l'on veut voir la valeur mesurée en cas d'échec.
fn outcome_full_label(accords: &[f64]) -> String {
    format!("{:.3}", moyenne(accords))
}

fn overlap(a: &[EngramId], b: &[EngramId]) -> usize {
    let set: HashSet<EngramId> = b.iter().copied().collect();
    a.iter().filter(|id| set.contains(id)).count()
}

fn moyenne(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}
