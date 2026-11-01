//! M1 — obrain-membench: multi-hop recall@k harness (plan f6569c2f).
//!
//! THE GAP THIS CLOSES (note ee3bf6f8): obrain had perf gates (µs, RSS,
//! p99) but ZERO memory-quality benchmark. Every peer system (SYNAPSE,
//! HeLa-Mem, TiMem, HippoRAG) ships recall@k / multi-hop curves. Without
//! a quality number, no cognitive mechanism can be verified and no
//! cognitive constant can be tuned empirically — so this harness is the
//! parent prerequisite of the whole membench plan.
//!
//! WHAT THIS IS: the *measurement machinery* — recall@1/5/10, MRR, and
//! multi-hop success — over the `EngramRetriever` (composite: BM25 direct
//! match + engram Hopfield recall + synapse spreading activation). The
//! corpus is a small self-contained 2-hop dataset embedded below so the
//! bench is reproducible and CI-runnable with zero external download.
//! `load_jsonl_corpus` is the pluggable seam to point it at a real
//! HotpotQA / 2WikiMultiHopQA export later (M1 acceptance: dataset
//! ingested, metrics journaled, baseline established).
//!
//! Not-in-scope for M1 (deferred): LLM-in-the-loop end-to-end answer
//! accuracy (that is M3), and the composite 532d kernel/hilbert/st vector
//! path (lives in obrain-chat, out of this repo) — here we measure the
//! in-repo engram+BM25+synapse retriever, which is the schema-agnostic
//! recall substrate.

use std::sync::Arc;

use obrain_cognitive::engram::EngramStore;
use obrain_common::types::{NodeId, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_rag::traits::Retriever;
use obrain_rag::{EngramRetriever, RagConfig};
use obrain_substrate::SubstrateStore;

/// One corpus passage: an id (for gold references) and its text.
struct Passage {
    key: &'static str,
    text: &'static str,
}

/// One evaluation query: the question text, and the keys of the passages
/// that must be retrieved to answer it (the "gold supporting facts").
/// `hops` records how many passages are jointly required — a multi-hop
/// question needs ≥2.
struct EvalQuery {
    question: &'static str,
    gold_keys: &'static [&'static str],
    hops: usize,
}

/// Self-contained 2-hop corpus. Each question's answer lives in one
/// passage but is only *reachable* by first identifying an entity named
/// in another passage — the canonical multi-hop shape. Deliberately built
/// so lexical overlap alone is insufficient for the second hop (the
/// bridge entity, not the answer term, is what the question shares with
/// the first passage).
const CORPUS: &[Passage] = &[
    Passage {
        key: "eiffel",
        text: "The Eiffel Tower is a wrought-iron lattice tower on the Champ de Mars in Paris. It was designed by the engineer Gustave Eiffel.",
    },
    Passage {
        key: "gustave",
        text: "Gustave Eiffel was a French civil engineer. He graduated from the Ecole Centrale des Arts et Manufactures and later built many bridges.",
    },
    Passage {
        key: "ecole",
        text: "The Ecole Centrale des Arts et Manufactures is a graduate engineering school located in France, founded in 1829.",
    },
    Passage {
        key: "statue",
        text: "The internal structure of the Statue of Liberty was engineered by Gustave Eiffel using a flexible iron framework.",
    },
    Passage {
        key: "panama",
        text: "The Panama Canal is a waterway in Panama connecting the Atlantic and Pacific oceans. An early French effort involved Gustave Eiffel's company.",
    },
    Passage {
        key: "curie",
        text: "Marie Curie was a physicist who conducted pioneering research on radioactivity in Paris. She won two Nobel Prizes.",
    },
    Passage {
        key: "sorbonne",
        text: "The University of Paris, known as the Sorbonne, is where Marie Curie became the first female professor.",
    },
    Passage {
        key: "radium",
        text: "Radium is a radioactive element discovered by Marie Curie and her husband Pierre Curie in 1898.",
    },
    Passage {
        key: "nobel",
        text: "The Nobel Prize is an award established by the will of Alfred Nobel, the inventor of dynamite.",
    },
    Passage {
        key: "dynamite",
        text: "Dynamite is an explosive made using nitroglycerin, invented and patented by Alfred Nobel in 1867.",
    },
];

/// Evaluation queries. `hops=2` questions require chaining two passages
/// via a bridge entity.
const QUERIES: &[EvalQuery] = &[
    // 2-hop: "who designed the Eiffel Tower" (eiffel) → "where did he
    // study" (gustave→ecole). The answer school is never co-mentioned
    // with "Eiffel Tower".
    EvalQuery {
        question: "Which engineering school did the designer of the Eiffel Tower attend?",
        gold_keys: &["eiffel", "gustave", "ecole"],
        hops: 2,
    },
    // 2-hop: Statue of Liberty internal engineer → his nationality.
    EvalQuery {
        question: "What was the nationality of the engineer of the Statue of Liberty internal structure?",
        gold_keys: &["statue", "gustave"],
        hops: 2,
    },
    // 2-hop: element discovered by Marie Curie → her university.
    EvalQuery {
        question: "At which university did the discoverer of radium become a professor?",
        gold_keys: &["radium", "curie", "sorbonne"],
        hops: 2,
    },
    // 2-hop: Nobel Prize founder → what he invented.
    EvalQuery {
        question: "What explosive did the founder of the Nobel Prize invent?",
        gold_keys: &["nobel", "dynamite"],
        hops: 2,
    },
    // 1-hop control: single-passage lexical question (sanity floor).
    EvalQuery {
        question: "What oceans does the Panama Canal connect?",
        gold_keys: &["panama"],
        hops: 1,
    },
];

/// Aggregate metrics over a query set.
#[derive(Default)]
struct Metrics {
    n: usize,
    recall_at_1: f64,
    recall_at_5: f64,
    recall_at_10: f64,
    mrr: f64,
    multihop_success: f64, // fraction of multi-hop queries whose gold set is fully in top-10
    multihop_n: usize,
}

fn recall_at_k(ranked: &[NodeId], gold: &[NodeId], k: usize) -> f64 {
    if gold.is_empty() {
        return 0.0;
    }
    let topk = &ranked[..ranked.len().min(k)];
    let hit = gold.iter().filter(|g| topk.contains(g)).count();
    hit as f64 / gold.len() as f64
}

/// Reciprocal rank of the FIRST gold hit (standard MRR numerator).
fn reciprocal_rank(ranked: &[NodeId], gold: &[NodeId]) -> f64 {
    for (i, nid) in ranked.iter().enumerate() {
        if gold.contains(nid) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

fn run_bench(passages: &[Passage], queries: &[EvalQuery]) -> Metrics {
    // Build a fresh substrate graph: one node per passage, text in a
    // `content` property. Bridge edges are added between passages sharing
    // a named entity so spreading activation has a second-hop path.
    let store = SubstrateStore::open_tempfile().expect("open substrate");
    let mut key_to_node: std::collections::HashMap<&str, NodeId> = std::collections::HashMap::new();
    for p in passages {
        let nid = store.create_node(&["Passage"]);
        store.set_node_property(nid, "content", Value::String(p.text.into()));
        store.set_node_property(nid, "key", Value::String(p.key.into()));
        key_to_node.insert(p.key, nid);
    }
    // Entity bridge edges (undirected pair → two directed edges), so the
    // second hop is reachable via synapse/relation spreading. These encode
    // the same "shared entity" links a KG builder would extract.
    let bridges: &[(&str, &str)] = &[
        ("eiffel", "gustave"),
        ("gustave", "ecole"),
        ("gustave", "statue"),
        ("gustave", "panama"),
        ("curie", "radium"),
        ("curie", "sorbonne"),
        ("nobel", "dynamite"),
    ];
    for (a, b) in bridges {
        let (na, nb) = (key_to_node[*a], key_to_node[*b]);
        store.create_edge(na, nb, "MENTIONS");
        store.create_edge(nb, na, "MENTIONS");
    }

    let typed: Arc<SubstrateStore> = Arc::new(store);
    let graph: Arc<dyn GraphStore> = Arc::clone(&typed) as Arc<dyn GraphStore>;
    let engram_store = Arc::new(EngramStore::new(None));
    // No synapse store here: with an empty engram store the retriever's
    // recall path is BM25 direct-match + relation context. That IS the
    // honest M1 baseline — later ablations (M3) add engrams/synapses and
    // measure the delta against this number.
    let retriever = EngramRetriever::with_defaults(graph, engram_store, None);

    // Thorough preset: max_context_nodes=50 so top-10 is never truncated
    // away, activation_depth=3 for the 2-hop reach.
    let config = RagConfig::thorough();

    let mut m = Metrics::default();
    for q in queries {
        let gold: Vec<NodeId> = q.gold_keys.iter().map(|k| key_to_node[*k]).collect();
        let res = retriever.retrieve(q.question, &config).expect("retrieve");
        // DETERMINISM: the retriever collects nodes from a HashMap, so
        // equal-score ties would otherwise resolve in nondeterministic
        // order and make recall@k flap between runs. Re-sort with a stable
        // secondary key (node_id ascending) so the baseline is
        // reproducible regardless of hash iteration order.
        let mut scored: Vec<(f64, NodeId)> =
            res.nodes.iter().map(|n| (n.score, n.node_id)).collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.0.cmp(&b.1.0))
        });
        let ranked: Vec<NodeId> = scored.into_iter().map(|(_, id)| id).collect();

        let r1 = recall_at_k(&ranked, &gold, 1);
        let r5 = recall_at_k(&ranked, &gold, 5);
        let r10 = recall_at_k(&ranked, &gold, 10);
        let rr = reciprocal_rank(&ranked, &gold);

        m.recall_at_1 += r1;
        m.recall_at_5 += r5;
        m.recall_at_10 += r10;
        m.mrr += rr;
        if q.hops >= 2 {
            m.multihop_n += 1;
            // full-set success: every gold passage present in top-10.
            let topk: Vec<NodeId> = ranked.iter().take(10).copied().collect();
            if gold.iter().all(|g| topk.contains(g)) {
                m.multihop_success += 1.0;
            }
        }
        println!(
            "  [{:>4}] r@1={:.2} r@5={:.2} r@10={:.2} rr={:.2}  | {}",
            format!("{}h", q.hops),
            r1,
            r5,
            r10,
            rr,
            q.question
        );
    }
    let n = queries.len() as f64;
    m.n = queries.len();
    m.recall_at_1 /= n;
    m.recall_at_5 /= n;
    m.recall_at_10 /= n;
    m.mrr /= n;
    if m.multihop_n > 0 {
        m.multihop_success /= m.multihop_n as f64;
    }
    m
}

// ── Metric self-tests: the metrics are foundational, so they are
// verified against hand-computed values BEFORE being trusted on real
// retrieval output. A bug here would silently poison M2/M3.
#[cfg(test)]
mod metric_tests {
    use super::{recall_at_k, reciprocal_rank};
    use obrain_common::types::NodeId;

    fn ids(xs: &[u64]) -> Vec<NodeId> {
        xs.iter().map(|&x| NodeId(x)).collect()
    }

    #[test]
    fn recall_at_k_partial_and_full() {
        // ranked = [10, 20, 30, 40, 50], gold = {20, 40, 99}
        let ranked = ids(&[10, 20, 30, 40, 50]);
        let gold = ids(&[20, 40, 99]);
        // @1: top=[10] → 0 of 3
        assert_eq!(recall_at_k(&ranked, &gold, 1), 0.0);
        // @2: top=[10,20] → 20 hit → 1/3
        assert!((recall_at_k(&ranked, &gold, 2) - 1.0 / 3.0).abs() < 1e-9);
        // @4: top=[10,20,30,40] → 20,40 → 2/3
        assert!((recall_at_k(&ranked, &gold, 4) - 2.0 / 3.0).abs() < 1e-9);
        // @10 (k>len): all 5 → still only 20,40 present (99 absent) → 2/3
        assert!((recall_at_k(&ranked, &gold, 10) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_monotonic_nondecreasing() {
        let ranked = ids(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let gold = ids(&[3, 7, 10]);
        let (r1, r5, r10) = (
            recall_at_k(&ranked, &gold, 1),
            recall_at_k(&ranked, &gold, 5),
            recall_at_k(&ranked, &gold, 10),
        );
        assert!(
            r1 <= r5 && r5 <= r10,
            "recall@k must be non-decreasing in k"
        );
        assert_eq!(r1, 0.0); // 1 not gold
        assert!((r5 - 1.0 / 3.0).abs() < 1e-9); // only 3 in top-5
        assert!((r10 - 1.0).abs() < 1e-9); // 3,7,10 all in top-10
    }

    #[test]
    fn recall_empty_gold_is_zero() {
        assert_eq!(recall_at_k(&ids(&[1, 2]), &ids(&[]), 5), 0.0);
    }

    #[test]
    fn reciprocal_rank_positions() {
        let ranked = ids(&[10, 20, 30, 40]);
        // first gold at rank 1 → 1.0
        assert_eq!(reciprocal_rank(&ranked, &ids(&[10])), 1.0);
        // first gold at rank 3 → 1/3
        assert!((reciprocal_rank(&ranked, &ids(&[30])) - 1.0 / 3.0).abs() < 1e-9);
        // earliest of {30,40} is 30 at rank 3 → 1/3 (not 1/4)
        assert!((reciprocal_rank(&ranked, &ids(&[40, 30])) - 1.0 / 3.0).abs() < 1e-9);
        // no gold present → 0.0
        assert_eq!(reciprocal_rank(&ranked, &ids(&[99])), 0.0);
    }
}

#[test]
fn membench_recall_baseline() {
    println!("\n=== M1 membench — recall@k on the embedded 2-hop corpus ===");
    let m = run_bench(CORPUS, QUERIES);

    println!(
        "\n  AGGREGATE (n={}): recall@1={:.3} recall@5={:.3} recall@10={:.3} MRR={:.3} multihop_success={:.3} (of {} multihop)",
        m.n, m.recall_at_1, m.recall_at_5, m.recall_at_10, m.mrr, m.multihop_success, m.multihop_n
    );

    // Journal the baseline as JSON, mirroring feedback_baseline.rs.
    let json = format!(
        "{{\"bench\":\"membench_recall\",\"plan\":\"f6569c2f\",\"corpus\":\"embedded_2hop\",\"n\":{},\"recall_at_1\":{:.4},\"recall_at_5\":{:.4},\"recall_at_10\":{:.4},\"mrr\":{:.4},\"multihop_success\":{:.4},\"multihop_n\":{}}}",
        m.n, m.recall_at_1, m.recall_at_5, m.recall_at_10, m.mrr, m.multihop_success, m.multihop_n
    );
    let _ = std::fs::write("/tmp/membench-recall.json", &json);
    println!("  report → /tmp/membench-recall.json");

    // Sanity floor (NOT a quality target — this is the "harness works and
    // finds something" gate; the real value is the journaled numbers that
    // M2/M3 will move). At minimum, direct-match recall must find the
    // lexically-obvious first hop of every query.
    assert!(
        m.recall_at_10 > 0.0,
        "recall@10 is zero — the harness retrieved nothing, machinery is broken"
    );
    assert!(
        m.recall_at_5 >= m.recall_at_1,
        "recall@5 must be ≥ recall@1 by monotonicity — metric bug"
    );
}
